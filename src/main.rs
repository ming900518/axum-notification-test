use std::{convert::Infallible, net::SocketAddr, time::Duration};

use axum::{
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    routing::get,
    Router, Server,
};
use futures::{stream::repeat_with, Stream};
use serde::Deserialize;
use tokio_stream::StreamExt as _;
use tracing::{info, Level};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    prelude::*,
};

const HINT: &str = r#"var eventSource = new EventSource('/sse');

eventSource.onmessage = function(event) {
    console.log('Message from server ', event.data);
}"#;

#[derive(Deserialize)]
struct SendRequest {}

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

    let router = Router::new()
        .route("/", get(|| async { HINT }))
        .route("/sse", get(sse))
        .into_make_service();
    let addr = SocketAddr::from(([0, 0, 0, 0], 13700));
    info!("Listening on {addr}");
    Server::bind(&addr)
        .serve(router)
        .await
        .expect("Server startup failed.");
}

async fn sse(header: HeaderMap) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    info!("Client connected: {header:#?}");
    let mut i = 0;
    let stream = repeat_with(move || {
        i += 1;
        info!("Message sent.");
        Event::default().data(format!("Server sent: {}", &i))
    })
    .map(Ok)
    .throttle(Duration::from_millis(10));

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(1))
            .text("keep-alive-text"),
    )
}
