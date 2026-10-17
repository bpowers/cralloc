//! The load generators of connect-rust's echo_bench and log_bench, against one server.
//!
//!   rpc-client <echo|log> addr=HOST:PORT [concurrency=64] [conns=8] [records=50]
//!              [warmup_ms=1000] [measure_ms=3000] [workers=N]
//!
//! `concurrency` tasks each issue one gRPC call at a time in a loop. echo goes through one
//! pooled `HttpClient`, as echo_bench's default pass does; log spreads the tasks over `conns`
//! HTTP/2 connections, as log_bench does. Prints `BENCH_MARK warm` when the warm-up ends and
//! `BENCH_MARK done` when the measurement ends, so the runner can read the server's counters
//! over exactly that window, then a `BENCH_RESULT` line.
use bench_harness::{Args, Better, Cpus, Report};
use bench_rpc::connect::bench::v1::{EchoServiceClient, LogIngestServiceClient};
use bench_rpc::proto::bench::v1::EchoRequest;
use bench_rpc::{PAYLOAD, log_request, runtime};
use connectrpc::Protocol;
use connectrpc::client::{ClientConfig, Http2Connection, HttpClient, SharedHttp2Connection};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAX_LATENCY_SAMPLES: usize = 500_000;

struct Outcome {
    requests: u64,
    elapsed: Duration,
    p50_us: u64,
    p99_us: u64,
}

/// Runs `concurrency` copies of `call` in a loop: warm-up, then measurement.
async fn drive<C, F>(concurrency: usize, warmup: Duration, measure: Duration, make: impl Fn(usize) -> C) -> Outcome
where
    C: FnMut() -> F + Send + 'static,
    F: Future<Output = bool> + Send,
{
    let running = Arc::new(AtomicBool::new(true));
    let count = Arc::new(AtomicU64::new(0));
    let latencies = Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(MAX_LATENCY_SAMPLES)));
    let mut handles = Vec::new();
    for i in 0..concurrency {
        let mut call = make(i);
        let (running, count, latencies) = (running.clone(), count.clone(), latencies.clone());
        handles.push(tokio::spawn(async move {
            while running.load(Ordering::Relaxed) {
                let start = Instant::now();
                if call().await {
                    let elapsed = start.elapsed();
                    let n = count.fetch_add(1, Ordering::Relaxed);
                    if n.is_multiple_of(10) {
                        let mut lats = latencies.lock().await;
                        if lats.len() < MAX_LATENCY_SAMPLES {
                            lats.push(elapsed.as_micros() as u64);
                        }
                    }
                }
            }
        }));
    }
    tokio::time::sleep(warmup).await;
    let warmed = count.swap(0, Ordering::Relaxed);
    assert!(warmed > 0, "no request succeeded during warm-up");
    latencies.lock().await.clear();
    println!("BENCH_MARK warm");
    let t0 = Instant::now();
    tokio::time::sleep(measure).await;
    let requests = count.load(Ordering::Relaxed);
    let elapsed = t0.elapsed();
    println!("BENCH_MARK done");
    running.store(false, Ordering::Relaxed);
    for h in handles {
        let _ = h.await;
    }
    let mut lats = latencies.lock().await;
    lats.sort_unstable();
    let q =
        |f: f64| lats.get(((lats.len() as f64 * f) as usize).min(lats.len().saturating_sub(1))).copied().unwrap_or(0);
    Outcome { requests, elapsed, p50_us: q(0.5), p99_us: q(0.99) }
}

async fn run(service: &str, args: &Args) -> Outcome {
    let addr = args.str("addr", "");
    assert!(!addr.is_empty(), "addr=HOST:PORT is required");
    let uri: http::Uri = format!("http://{addr}").parse().expect("valid server URL");
    let config = ClientConfig::new(uri.clone()).with_protocol(Protocol::Grpc);
    let concurrency = args.usize("concurrency", 64);
    let warmup = Duration::from_millis(args.num("warmup_ms", 1000));
    let measure = Duration::from_millis(args.num("measure_ms", 3000));
    match service {
        "echo" => {
            let http = HttpClient::plaintext_http2_only();
            let request = EchoRequest { message: PAYLOAD.to_string(), ..Default::default() };
            drive(concurrency, warmup, measure, |_| {
                let client = EchoServiceClient::new(http.clone(), config.clone());
                let request = request.clone();
                move || {
                    let (client, request) = (client.clone(), request.clone());
                    async move { client.echo(request).await.is_ok() }
                }
            })
            .await
        }
        "log" => {
            let n_conns = args.usize("conns", 8);
            let mut conns: Vec<SharedHttp2Connection> = Vec::with_capacity(n_conns);
            for _ in 0..n_conns {
                conns.push(Http2Connection::connect_plaintext(uri.clone()).await.expect("connect").shared(1024));
            }
            let request = log_request(args.usize("records", 50));
            drive(concurrency, warmup, measure, |i| {
                let client = LogIngestServiceClient::new(conns[i % n_conns].clone(), config.clone());
                let request = request.clone();
                move || {
                    let (client, request) = (client.clone(), request.clone());
                    async move { client.ingest(request).await.is_ok() }
                }
            })
            .await
        }
        _ => panic!("usage: rpc-client <echo|log> addr=HOST:PORT ..."),
    }
}

fn main() {
    let (service, args) = Args::from_env();
    let cpus = Cpus::from_env();
    let rt = runtime(&cpus, args.usize("workers", cpus.len()));
    let o = rt.block_on(run(&service, &args));
    let rps = o.requests as f64 / o.elapsed.as_secs_f64();
    Report::new(&format!("rpc-client-{service}"), &args)
        .metric("requests", o.requests)
        .metric("measure_ns", o.elapsed.as_nanos() as u64)
        .metric("p50_us", o.p50_us)
        .metric("p99_us", o.p99_us)
        .primary("req/s", rps, Better::Higher)
        .emit();
}
