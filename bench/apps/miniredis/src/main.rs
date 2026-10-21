//! mini-redis 0.4.1 (tokio's teaching Redis server: one `Mutex<HashMap>` behind a
//! multi-threaded tokio runtime) as a server for the load generator:
//!
//!   miniredis serve [preload=N]
//!
//! What its `mini-redis-server` does, on an ephemeral port: bind, run `mini_redis::server::run`
//! until SIGTERM, then print the result line. With `preload=N` it first sets keys `key:00000000`
//! to `key:{N-1}` through its own client, over 16 connections, each to a value of 64 to 4,096
//! bytes (log-uniform, about 1 KiB on average, from a fixed sequence), so that the server
//! starts serving with a heap of about N KiB live; only then does it print `BENCH_LISTEN addr`.
use bench_app::{Driver, fail};
use tokio::signal::unix::{SignalKind, signal};

/// The size of key `i`'s preloaded value: 64 << (6 u) for u from a hash of `i`, so sizes are
/// spread evenly on a log scale from 64 bytes to 4 KiB.
fn value_size(i: u64) -> usize {
    let mut x = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    let u = (x >> 11) as f64 / (1u64 << 53) as f64;
    (64.0 * (6.0 * u).exp2()) as usize
}

async fn preload(addr: std::net::SocketAddr, keys: u64) {
    const CONNECTIONS: u64 = 16;
    let tasks: Vec<_> = (0..CONNECTIONS)
        .map(|c| {
            tokio::spawn(async move {
                let mut client =
                    mini_redis::client::connect(addr).await.unwrap_or_else(|e| fail(&format!("preload connect: {e}")));
                let mut i = c;
                while i < keys {
                    let value = bytes::Bytes::from(vec![b'v'; value_size(i)]);
                    client
                        .set(&format!("key:{i:08}"), value)
                        .await
                        .unwrap_or_else(|e| fail(&format!("preload set: {e}")));
                    i += CONNECTIONS;
                }
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap_or_else(|e| fail(&format!("preload task: {e}")));
    }
}

fn main() {
    let d = Driver::start("miniredis");
    if d.job != "serve" {
        fail("usage: miniredis serve [preload=N]");
    }
    let keys = d.args.num("preload", 0);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    rt.block_on(async {
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap_or_else(|e| fail(&format!("bind: {e}")));
        let addr = listener.local_addr().unwrap_or_else(|e| fail(&format!("local addr: {e}")));
        let mut term = signal(SignalKind::terminate()).unwrap_or_else(|e| fail(&format!("signal: {e}")));
        let server = tokio::spawn(mini_redis::server::run(listener, async move { term.recv().await }));
        if keys > 0 {
            preload(addr, keys).await;
        }
        println!("BENCH_LISTEN {addr}");
        server
            .await
            .unwrap_or_else(|e| fail(&format!("server task: {e}")))
            .unwrap_or_else(|e| fail(&format!("serve: {e}")));
    });
    drop(rt);
    d.finish_server();
}
