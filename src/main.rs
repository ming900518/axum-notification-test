#![warn(clippy::all, clippy::nursery, clippy::pedantic, clippy::perf)]
#![allow(clippy::significant_drop_tightening)]
use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    process::exit,
    sync::OnceLock,
    time::Duration,
};

use axum::{
    extract::Query,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    routing::{get, post},
    Json, Router, Server,
};
use futures::Stream;
use serde::Deserialize;
use tokio::sync::{mpsc::Sender, RwLock};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tracing::{error, info, Level};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    prelude::*,
};

const HINT: &str = r#"var eventSource = new EventSource('/sse?user_id=');

eventSource.onmessage = function(event) {
    console.log('Message from server ', event.data);
}"#;

#[derive(Deserialize)]
struct UserInfo {
    user_id: String,
}

#[derive(Deserialize)]
struct SendData {
    user_id: String,
    data: String,
}

static CHANNELS: OnceLock<RwLock<HashMap<String, Sender<String>>>> = OnceLock::new();

#[tokio::main]
async fn main() {
    CHANNELS.get_or_init(|| RwLock::new(HashMap::new()));

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

    let router = Router::new()
        .route("/", get(|| async { HINT }))
        .route("/sse", get(sse))
        .route("/send", post(send))
        .into_make_service();

    let addr = SocketAddr::from(([0, 0, 0, 0], 13700));
    info!("Listening on {addr}");

    Server::bind(&addr)
        .serve(router)
        .await
        .expect("Server startup failed.");
}

async fn sse(
    Query(user_info): Query<UserInfo>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    info!("Client connected: {}", user_info.user_id);
    let Some(channel) = CHANNELS.get() else {
        error!("CACHE not found.");
        exit(1)
    };
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    if channel.read().await.get(&user_info.user_id).is_none() {
        channel.write().await.insert(user_info.user_id.clone(), tx);
    }

    let stream = ReceiverStream::new(rx)
        .map(|data| Ok(Event::default().data(data)))
        .throttle(Duration::from_secs(10));

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keep-alive-text"),
    )
}

async fn send(Json(send): Json<SendData>) -> (StatusCode, String) {
    if let Some(channel) = CHANNELS.get() {
        let reader = channel.read().await;
        let Some(tx) = reader.get(&send.user_id) else {
            return (StatusCode::NOT_FOUND, "User not found".to_owned());
        };
        match tx.send(send.data).await {
            Ok(_) => (StatusCode::OK, "Sent".to_owned()),
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#?}"))
        }
    } else {
        error!("CACHE not found.");
        exit(1)
    }
}
