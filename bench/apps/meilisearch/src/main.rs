//! Meilisearch 1.55.0, the search engine, as a server for the load generator:
//!
//!   meilisearch serve data=DIR [docs=N]
//!
//! starts Meilisearch's HTTP server as its own `main` does (`setup_meilisearch`, `create_app`;
//! most of `run_http` is copied below), on a loopback port, with a fresh database in
//! `DIR/db.<pid>` that it removes at exit. Before it announces its address, it indexes the
//! first `docs` hundred thousand documents (default 4, of 5) of Hacker News from `DIR`
//! (`hn-1.ndjson` ...), with title, text and author searchable, as Meilisearch's own benchmark
//! of that dataset configures them, through its own HTTP API; then prints `BENCH_LISTEN addr`.
//! The load (`fetch.sh` writes it) is searches with now and then a batch of document updates,
//! so the server both answers and indexes while it is measured. On SIGTERM it stops the server
//! and prints its result line.
//!
//! What differs from the `meilisearch` binary: the allocator (the suite's, not jemalloc, and the
//! jemalloc crate Meilisearch links is replaced by an empty one, `no-jemalloc/`, so that its
//! `malloc` override cannot take the C allocations), no launch banner, no analytics, no master
//! key (the development environment), logging at warnings only, and indexing threads and
//! memory set to the server's CPUs and 2 GiB rather than to the whole machine.
use actix_web::HttpServer;
use actix_web::http::KeepAlive;
use actix_web::web::Data;
use bench_app::{Driver, fail};
use clap::Parser;
use meilisearch::personalization::PersonalizationService;
use meilisearch::search_queue::SearchQueue;
use meilisearch::{
    LogRouteHandle, LogRouteType, LogStderrHandle, LogStderrType, Opt, ServicesData, SubscriberForSecondLayer,
    analytics, create_app, setup_meilisearch,
};
use std::io::{LineWriter, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::thread::available_parallelism;
use std::time::{Duration, Instant};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt as _;

fn main() {
    let mut d = Driver::start("meilisearch");
    match d.job.as_str() {
        "serve" => serve(&mut d),
        _ => fail("usage: meilisearch serve data=DIR [docs=N]"),
    }
    d.finish_server();
}

fn serve(d: &mut Driver) {
    let data = PathBuf::from(d.args.str("data", ""));
    if !data.join("hn-1.ndjson").is_file() {
        fail("data=DIR must hold hn-1.ndjson ... (bench/apps/fetch meilisearch)");
    }
    let docs = d.args.usize("docs", 4);
    let db = data.join(format!("db.{}", std::process::id()));
    let cpus = available_parallelism().map_or(1, |n| n.get());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| fail(&format!("bind: {e}")));
    let addr = listener.local_addr().unwrap_or_else(|e| fail(&format!("local addr: {e}")));
    let opt = Opt::parse_from([
        "meilisearch".to_string(),
        format!("--db-path={}", db.display()),
        format!("--http-addr={addr}"),
        "--env=development".into(),
        "--no-analytics".into(),
        "--log-level=WARN".into(),
        format!("--max-indexing-threads={cpus}"),
        "--max-indexing-memory=2GiB".into(),
    ]);
    let result = actix_web::rt::System::new().block_on(async move {
        let server = start(opt, listener)?;
        let handle = server.handle();
        let running = actix_web::rt::spawn(server);
        let loaded = tokio::task::spawn_blocking(move || load(addr, &data, docs)).await;
        if let Err(e) = loaded.map_err(|e| e.to_string()).and_then(|r| r) {
            handle.stop(false).await;
            anyhow::bail!("loading the index: {e}");
        }
        println!("BENCH_LISTEN {addr}");
        let _ = std::io::stdout().flush();
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        term.recv().await;
        handle.stop(true).await;
        let _ = running.await;
        anyhow::Ok(())
    });
    let _ = std::fs::remove_dir_all(&db);
    if let Err(e) = result {
        fail(&format!("Meilisearch: {e:#}"));
    }
    d.report().param("docs", (docs * 100_000) as u64);
}

/// Meilisearch's `setup` (logging), as its `main` runs it.
fn setup(opt: &Opt) -> (LogRouteHandle, LogStderrHandle) {
    let route = None.with_filter(tracing_subscriber::filter::Targets::new().with_target("", LevelFilter::OFF));
    let (route_layer, route_handle) = tracing_subscriber::reload::Layer::<LogRouteType, _>::new(route);
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(|| LineWriter::new(std::io::stderr()))
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE);
    let stderr: LogStderrType =
        (Box::new(stderr) as Box<dyn tracing_subscriber::Layer<SubscriberForSecondLayer> + Send + Sync>).with_filter(
            tracing_subscriber::filter::Targets::new()
                .with_target("", LevelFilter::from_str(&opt.log_level.to_string()).unwrap()),
        );
    let (stderr_layer, stderr_handle) = tracing_subscriber::reload::Layer::new(stderr);
    let subscriber = tracing_subscriber::registry().with(route_layer).with(stderr_layer);
    tracing::subscriber::set_global_default(subscriber).unwrap();
    (route_handle, stderr_handle)
}

/// Meilisearch's `try_main` and `run_http`, to the point where the server runs, on `listener`.
fn start(opt: Opt, listener: TcpListener) -> anyhow::Result<actix_web::dev::Server> {
    let (logs_route_handle, logs_stderr_handle) = setup(&opt);
    let (index_scheduler, auth_controller) = setup_meilisearch(&opt, tokio::runtime::Handle::current())?;
    let analytics = Arc::new(analytics::Analytics::no_analytics());
    let enable_dashboard = &opt.env == "development";
    let index_scheduler = Data::from(index_scheduler);
    let personalization_service = Data::new(PersonalizationService::disabled());
    let search_queue = SearchQueue::new(
        opt.experimental_search_queue_size,
        available_parallelism()
            .unwrap_or(NonZeroUsize::new(2).unwrap())
            .checked_mul(opt.experimental_nb_searches_per_core)
            .unwrap_or(NonZeroUsize::MAX),
    )
    .with_time_to_abort(Duration::from_secs(usize::from(opt.experimental_drop_search_after) as u64));
    let services = ServicesData {
        index_scheduler,
        auth: Data::from(auth_controller),
        search_queue: Data::new(search_queue),
        personalization_service,
        logs_route_handle: Data::new(logs_route_handle),
        logs_stderr_handle: Data::new(logs_stderr_handle),
        analytics: Data::from(analytics),
    };
    let server = HttpServer::new(move || create_app(services.clone(), opt.clone(), enable_dashboard))
        .disable_signals()
        .keep_alive(KeepAlive::Os)
        .listen(listener)?
        .run();
    Ok(server)
}

/// Indexes the first `docs` files of `data` through the server's API, waiting for each.
fn load(addr: SocketAddr, data: &Path, docs: usize) -> Result<(), String> {
    let settings = br#"{"searchableAttributes":["title","text","by"],"filterableAttributes":["by","type"],"sortableAttributes":["score","time"]}"#;
    wait(addr, call(addr, "PATCH", "/indexes/hn/settings", "application/json", settings)?)?;
    for i in 1..=docs {
        let p = data.join(format!("hn-{i}.ndjson"));
        let body = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        wait(addr, call(addr, "POST", "/indexes/hn/documents?primaryKey=id", "application/x-ndjson", &body)?)?;
    }
    Ok(())
}

/// Sends one request and returns the response body, failing on anything but 2xx.
fn call(addr: SocketAddr, method: &str, path: &str, ctype: &str, body: &[u8]) -> Result<String, String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| format!("send: {e}"))?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| format!("receive: {e}"))?;
    let (head, body) = out.split_once("\r\n\r\n").ok_or("no response head")?;
    if !head.starts_with("HTTP/1.1 2") {
        return Err(format!("{method} {path}: {}", head.lines().next().unwrap_or("")));
    }
    Ok(body.to_string())
}

/// Waits for the task a response names to succeed.
fn wait(addr: SocketAddr, enqueued: String) -> Result<(), String> {
    let uid = number_after(&enqueued, "\"taskUid\":").ok_or_else(|| format!("no task in {enqueued}"))?;
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline {
        let t = call(addr, "GET", &format!("/tasks/{uid}"), "application/json", b"")?;
        if t.contains("\"status\":\"succeeded\"") {
            return Ok(());
        }
        if t.contains("\"status\":\"failed\"") {
            return Err(format!("task {uid} failed: {t}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("task {uid} did not finish"))
}

fn number_after(s: &str, key: &str) -> Option<u64> {
    let rest = &s[s.find(key)? + key.len()..];
    rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}
