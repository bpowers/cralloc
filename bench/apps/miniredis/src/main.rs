//! mini-redis 0.4.1 (tokio's teaching Redis server: one `Mutex<HashMap>` behind a
//! multi-threaded tokio runtime) as a server for the load generator:
//!
//!   miniredis serve
//!
//! What its `mini-redis-server` does, on an ephemeral port: bind, print `BENCH_LISTEN addr`,
//! run `mini_redis::server::run` until SIGTERM, then print the result line.
use bench_app::{Driver, fail};
use tokio::signal::unix::{SignalKind, signal};

fn main() {
    let d = Driver::start("miniredis");
    if d.job != "serve" {
        fail("usage: miniredis serve");
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    rt.block_on(async {
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap_or_else(|e| fail(&format!("bind: {e}")));
        let addr = listener.local_addr().unwrap_or_else(|e| fail(&format!("local addr: {e}")));
        println!("BENCH_LISTEN {addr}");
        let mut term = signal(SignalKind::terminate()).unwrap_or_else(|e| fail(&format!("signal: {e}")));
        mini_redis::server::run(listener, async move { term.recv().await })
            .await
            .unwrap_or_else(|e| fail(&format!("serve: {e}")));
    });
    drop(rt);
    d.finish_server();
}
