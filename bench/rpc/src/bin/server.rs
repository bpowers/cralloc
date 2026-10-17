//! connect-rust's echo_server and log_server in one binary, with the global allocator from
//! bench-alloc.
//!
//!   rpc-server <echo|log> [workers=N]
//!
//! Workers are pinned to `BENCH_CPUS` (default: as many workers as CPUs listed). Prints
//! `BENCH_LISTEN addr` once bound; on SIGINT or SIGTERM it stops and prints a `BENCH_RESULT`
//! line naming its allocator and mode. The runner measures it from outside.
use bench_harness::{Args, Cpus, Report};
use bench_rpc::connect::bench::v1::{EchoServiceServer, LogIngestServiceServer};
use bench_rpc::{EchoImpl, LogIngestImpl, runtime};
use connectrpc::ConnectRpcService;
use tokio::signal::unix::{SignalKind, signal};

fn main() {
    let (service, args) = Args::from_env();
    let cpus = Cpus::from_env();
    let workers = args.usize("workers", cpus.len());
    let rt = runtime(&cpus, workers);
    rt.block_on(async {
        let bound = connectrpc::server::Server::bind("127.0.0.1:0").await.expect("bind");
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
            _ => panic!("usage: rpc-server <echo|log> [workers=N]"),
        }
    });
    Report::new(&format!("rpc-server-{service}"), &args).param("workers", workers as u64).emit();
}
