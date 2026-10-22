//! connect-rust's echo_server and log_server in one binary, with the global allocator from
//! bench-alloc.
//!
//!   rpc-server <echo|log|store> [workers=N] [keep=N]
//!
//! `store` serves the log store (`bench_rpc::store`): ingest keeps records, `ServerStream`
//! tails them; it starts with `keep` records (default 1,000,000) before it listens.
//!
//! Built with the `maintenance` feature (and asmalloc as the allocator), `BENCH_MAINTAIN` picks
//! what runs asmalloc's maintenance while the server is silent: `task` a task on the runtime,
//! `thread` asmalloc's background thread, anything else nothing.
//!
//! Workers are pinned to `BENCH_CPUS` (default: as many workers as CPUs listed). Prints
//! `BENCH_LISTEN addr` once bound; on SIGINT or SIGTERM it stops and prints a `BENCH_RESULT`
//! line naming its allocator and mode. The runner measures it from outside.
use bench_harness::{Args, Cpus, Report};
use bench_rpc::connect::bench::v1::{BenchServiceExt, EchoServiceServer, LogIngestServiceExt, LogIngestServiceServer};
use bench_rpc::store::{Store, StoreIngest, StoreTail};
use bench_rpc::{EchoImpl, LogIngestImpl, runtime};
use connectrpc::ConnectRpcService;
use std::sync::Arc;
use tokio::signal::unix::{SignalKind, signal};

fn main() {
    let (service, args) = Args::from_env();
    let cpus = Cpus::from_env();
    let workers = args.usize("workers", cpus.len());
    let rt = runtime(&cpus, workers);
    let keep = args.usize("keep", 1_000_000);
    // Filled on the runtime's own threads, as ingest would fill it.
    let store = (service == "store").then(|| rt.block_on(async { Arc::new(Store::new(keep)) }));
    #[cfg(feature = "maintenance")]
    if std::env::var("BENCH_MAINTAIN").as_deref() == Ok("thread") {
        asmalloc::Asmalloc::start_background().expect("asmalloc's background thread starts");
    }
    rt.block_on(async {
        #[cfg(feature = "maintenance")]
        if std::env::var("BENCH_MAINTAIN").as_deref() == Ok("task") {
            drop(asmalloc_tokio::spawn_maintenance().expect("a runtime with a timer"));
        }
        let mut bound = connectrpc::server::Server::bind("127.0.0.1:0").await.expect("bind");
        // connectrpc closes a keep-alive HTTP/1.1 connection that waits 30 s for its next
        // request. The store's requests are quick, so at 70% the load generator leaves most of
        // its 64 connections idle that long, and each close would be a reconnect, which makes
        // the run untrustworthy. A service behind a pooled client is configured with a long
        // limit; so is this one.
        if service == "store" {
            bound = bound.with_header_read_timeout(std::time::Duration::from_secs(300));
        }
        println!("BENCH_LISTEN {}", bound.local_addr().expect("local addr"));
        let mut term = signal(SignalKind::terminate()).unwrap();
        let stop = async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        };
        match service.as_str() {
            "echo" => {
                let svc = ConnectRpcService::new(EchoServiceServer::new(EchoImpl));
                tokio::select! { r = bound.serve_with_service(svc) => r.expect("serve"), _ = stop => {} }
            }
            "log" => {
                let svc = ConnectRpcService::new(LogIngestServiceServer::new(LogIngestImpl));
                tokio::select! { r = bound.serve_with_service(svc) => r.expect("serve"), _ = stop => {} }
            }
            "store" => {
                let store = store.expect("built above");
                let router = connectrpc::Router::new();
                let router = Arc::new(StoreIngest(store.clone())).register(router);
                let router = Arc::new(StoreTail(store)).register(router);
                let svc = ConnectRpcService::new(router);
                tokio::select! { r = bound.serve_with_service(svc) => r.expect("serve"), _ = stop => {} }
            }
            _ => panic!("usage: rpc-server <echo|log|store> [workers=N] [keep=N]"),
        }
    });
    let mut report = Report::new(&format!("rpc-server-{service}"), &args);
    report.param("workers", workers as u64);
    if service == "store" {
        report.param("keep", keep as u64);
    }
    report.emit();
}
