#![warn(clippy::all, clippy::nursery, clippy::pedantic, clippy::perf)]
#![allow(clippy::significant_drop_tightening)]
use std::{
    collections::HashMap, convert::Infallible, net::SocketAddr, process::exit, str::FromStr,
    sync::OnceLock, time::Duration,
};

use axum::{
    extract::Query,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        Html, IntoResponse, Sse,
    },
    routing::{get, post},
    Json, Router, Server,
};
use base64ct::{Base64UrlUnpadded, Encoding};
use futures::Stream;
use hyper::{header, Body, Client};
use hyper_rustls::HttpsConnectorBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{from_str, Value};
use tokio::sync::{mpsc::Sender, RwLock};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::{error, info, Level};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    prelude::*,
};
use web_push_native::{jwt_simple::prelude::ES256KeyPair, p256::PublicKey, Auth, WebPushBuilder};

#[derive(Deserialize)]
struct UserInfo {
    user_id: String,
}

#[derive(Deserialize)]
struct SendData {
    user_id: String,
    data: String
}

#[derive(Deserialize, Debug)]
struct UserRegistrationRequest {
    user_id: String,
    endpoint: String,
    keys: UserRegistrationKey,
}

#[derive(Deserialize, Debug)]
struct UserRegistrationKey {
    p256dh: String,
    auth: String,
}

#[derive(Debug)]
struct UserRegistration {
    sse_sender: Option<Sender<String>>,
    endpoint: String,
    p256dh: String,
    auth: String,
}

impl From<UserRegistrationRequest> for UserRegistration {
    fn from(value: UserRegistrationRequest) -> Self {
        Self {
            sse_sender: None,
            endpoint: value.endpoint,
            p256dh: value.keys.p256dh,
            auth: value.keys.auth,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct VapidKey {
    subject: String,
    public_key: String,
    private_key: String,
}

impl FromStr for VapidKey {
    type Err = serde_json::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        from_str::<Self>(s)
    }
}

static CHANNELS: OnceLock<RwLock<HashMap<String, UserRegistration>>> = OnceLock::new();
static VAPID: OnceLock<VapidKey> = OnceLock::new();

#[tokio::main]
async fn main() {
    let tracing_filter = Targets::new()
        .with_target("tower_http::trace::on_response", Level::DEBUG)
        .with_target("tower_http::trace::on_request", Level::DEBUG)
        .with_target("tower_http::trace::make_span", Level::DEBUG)
        .with_target("rustls::*", LevelFilter::OFF)
        .with_default(Level::INFO);

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_filter)
        .init();

    CHANNELS.get_or_init(|| RwLock::new(HashMap::new()));
    VAPID.get_or_init(|| {
        VapidKey::from_str(include_str!("vapid.json"))
            .expect("VAPID key could not be deserialized.")
    });

    let router = Router::new()
        .route(
            "/",
            get(|| async { Html::from(include_str!("index.html")) }),
        )
        .route("/vapid.json", get(|| async { Json(VAPID.get().unwrap()) }))
        .route(
            "/manifest.json",
            get(|| async {
                let json = from_str::<Value>(include_str!("manifest.json")).unwrap_or_default();
                Json(json)
            }),
        )
        .route(
            "/service_worker.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/javascript")],
                    include_bytes!("service_worker.js"),
                )
            }),
        )
        .route(
            "/index.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/javascript")],
                    include_bytes!("index.js"),
                )
            }),
        )
        .route("/sse", get(sse))
        .route("/register", post(register))
        .route("/send", post(send))
        .into_make_service();

    let addr = SocketAddr::from(([0, 0, 0, 0], 13700));
    info!("Listening on {addr}");

    Server::bind(&addr)
        .serve(router)
        .await
        .expect("Server startup failed.");
}

async fn register(Json(user_reg): Json<UserRegistrationRequest>) -> impl IntoResponse {
    let Some(channel) = CHANNELS.get() else {
        error!("CACHE not found.");
        exit(1)
    };

    channel
        .write()
        .await
        .insert(user_reg.user_id.clone(), UserRegistration::from(user_reg));
    (StatusCode::OK, "Success".to_owned())
}

async fn sse(
    Query(user_info): Query<UserInfo>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let Some(channel) = CHANNELS.get() else {
        error!("CACHE not found.");
        exit(1)
    };
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let mut channel = channel.write().await;
    let Some(user) = channel.get_mut(&user_info.user_id) else {
        error!("User {} not found.", user_info.user_id);
        return Err(StatusCode::NOT_FOUND);
    };
    user.sse_sender = Some(tx);

    let stream = ReceiverStream::new(rx)
        .map(|data| Ok(Event::default().data(data)))
        .throttle(Duration::from_secs(10));

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keep-alive-text"),
    ))
}

async fn send(Json(send): Json<SendData>) -> impl IntoResponse {
    if let Some(channel) = CHANNELS.get() {
        let reader = channel.read().await;
        let Some(reg) = reader.get(&send.user_id) else {
            return (StatusCode::NOT_FOUND, "User not found".to_owned());
        };
        if let Some(vapid) = VAPID.get() {
            let key_pair = ES256KeyPair::from_bytes(
                &Base64UrlUnpadded::decode_vec(&vapid.private_key).unwrap(),
            )
            .unwrap();
            let builder = WebPushBuilder::new(
                reg.endpoint.parse().unwrap(),
                PublicKey::from_sec1_bytes(&Base64UrlUnpadded::decode_vec(&reg.p256dh).unwrap())
                    .unwrap(),
                Auth::clone_from_slice(&Base64UrlUnpadded::decode_vec(&reg.auth).unwrap()),
            )
            .with_vapid(&key_pair, &vapid.subject);
            if let Ok(request) = builder
                .build(send.data.clone())
                .map(|req| req.map(std::convert::Into::into))
            {
                let https = HttpsConnectorBuilder::new()
                    .with_native_roots()
                    .https_only()
                    .enable_http1()
                    .build();
                let client: Client<_, Body> = Client::builder().build(https);
                if let Err(error) = client.request(request).await {
                    error!("{error}");
                };
            }
        };

        if let Some(sender) = &reg.sse_sender {
            match sender.send(send.data).await {
                Ok(_) => (StatusCode::OK, "Sent".to_owned()),
                Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:?}")),
            }
        } else {
            (
                StatusCode::OK,
                "Sent without sending event due to no channel available.".to_owned(),
            )
        }
    } else {
        error!("CACHE not found.");
        exit(1)
    }
}
