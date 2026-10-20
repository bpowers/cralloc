//! The order service as a server for the load generator, on one of two frameworks:
//!
//!   orders <axum|actix>
//!
//! axum runs on hyper and a multi-threaded tokio runtime with work stealing; actix-web runs one
//! single-threaded runtime per worker thread, so blocks cross threads only through the shared
//! store. Each sizes its workers from the CPUs the process may run on. The server binds an
//! ephemeral port on loopback, prints `BENCH_LISTEN addr`, serves `POST /orders` until SIGTERM,
//! then prints its `BENCH_RESULT` line; the runner measures it from outside.
mod store;

use bench_app::{Driver, fail};
use std::net::TcpListener;
use std::sync::Arc;
use store::{NewOrder, Store};

fn main() {
    let d = Driver::start("orders");
    // Bound here, before either framework starts, so that the address can be announced the
    // same way for both.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| fail(&format!("bind: {e}")));
    let addr = listener.local_addr().unwrap_or_else(|e| fail(&format!("local addr: {e}")));
    match d.job.as_str() {
        "axum" => axum_serve(listener, addr),
        "actix" => actix_serve(listener, addr),
        _ => fail("usage: orders <axum|actix>"),
    }
    d.finish_server();
}

fn announce(addr: std::net::SocketAddr) {
    println!("BENCH_LISTEN {addr}");
}

#[tokio::main]
async fn axum_serve(listener: TcpListener, addr: std::net::SocketAddr) {
    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    async fn create(State(store): State<Arc<Store>>, Json(new): Json<NewOrder>) -> String {
        store.create(new)
    }
    let store = Arc::new(Store::default());
    let app = Router::new().route("/orders", post(create)).with_state(store);
    listener.set_nonblocking(true).unwrap_or_else(|e| fail(&format!("nonblocking: {e}")));
    let listener = tokio::net::TcpListener::from_std(listener).unwrap_or_else(|e| fail(&format!("listener: {e}")));
    announce(addr);
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .unwrap_or_else(|e| fail(&format!("signal: {e}")));
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            term.recv().await;
        })
        .await
        .unwrap_or_else(|e| fail(&format!("serve: {e}")));
}

#[actix_web::main]
async fn actix_serve(listener: TcpListener, addr: std::net::SocketAddr) {
    use actix_web::{App, HttpServer, web};
    async fn create(store: web::Data<Store>, new: web::Json<NewOrder>) -> String {
        store.create(new.into_inner())
    }
    let store = web::Data::new(Store::default());
    // actix-web stops gracefully on SIGTERM by itself. Its defaults close a connection that
    // has been idle for 5 s and one whose request has not arrived within 5 s of its accept;
    // under the generator's plan some of the 64 pooled connections were closed that way and
    // reopened, which costs the server work that hyper (axum) never does. Long limits, as a
    // service behind a pooled client is configured, keep the comparison to the allocator.
    let server = HttpServer::new(move || App::new().app_data(store.clone()).route("/orders", web::post().to(create)))
        .keep_alive(std::time::Duration::from_secs(300))
        .client_request_timeout(std::time::Duration::ZERO)
        .listen(listener)
        .unwrap_or_else(|e| fail(&format!("listen: {e}")))
        .run();
    announce(addr);
    server.await.unwrap_or_else(|e| fail(&format!("serve: {e}")));
}
