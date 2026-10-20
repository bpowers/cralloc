//! The order service on axum (hyper, a multi-threaded tokio runtime with work stealing).
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use screen_servers::{NewOrder, Store, port};
use std::sync::Arc;

async fn create(State(store): State<Arc<Store>>, Json(new): Json<NewOrder>) -> String {
    store.create(new)
}

#[tokio::main]
async fn main() {
    let store = Arc::new(Store::default());
    let app = Router::new().route("/orders", post(create)).with_state(store);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port())).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
