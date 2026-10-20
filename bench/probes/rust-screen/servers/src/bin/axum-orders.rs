//! The order service on axum (hyper, a multi-threaded tokio runtime with work stealing).
//!
//! Built with the `asmalloc` feature, asmalloc is the global allocator, and `ORDERS_MAINTAIN`
//! picks what runs its maintenance while the server is silent (docs/design.md, "Heartbeat"):
//! `task` a task on the runtime (`asmalloc_tokio::spawn_maintenance`), `thread` asmalloc's
//! background thread, and anything else nothing.
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use screen_servers::{NewOrder, Store, port};
use std::sync::Arc;

async fn create(State(store): State<Arc<Store>>, Json(new): Json<NewOrder>) -> String {
    store.create(new)
}

#[cfg(feature = "asmalloc")]
#[global_allocator]
static GLOBAL: asmalloc::Asmalloc = asmalloc::Asmalloc;

#[tokio::main]
async fn main() {
    #[cfg(feature = "asmalloc")]
    match std::env::var("ORDERS_MAINTAIN").as_deref() {
        Ok("task") => drop(asmalloc_tokio::spawn_maintenance()),
        Ok("thread") => asmalloc::Asmalloc::start_background().expect("asmalloc's background thread starts"),
        _ => {}
    }
    let store = Arc::new(Store::default());
    let app = Router::new().route("/orders", post(create)).with_state(store);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port())).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
