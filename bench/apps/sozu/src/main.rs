//! Sōzu 2.3.3, the HTTP reverse proxy, as a server for the load generator:
//!
//!   sozu proxy upstream=HOST:PORT [workers=N]
//!
//! runs Sōzu's own main process (`sozu::command::begin_main_process`, what `sozu start` runs)
//! with a configuration this driver writes: one HTTP listener on a loopback port, one cluster
//! whose backend is `upstream`, `workers` worker processes (default: one per CPU it may run
//! on), and Sōzu's defaults otherwise (buffers, timeouts, connection limits). The main process
//! forks its workers and each re-executes this binary as `sozu worker ...`, which runs Sōzu's
//! worker entry, compiled here from the `sozu` crate's own source (`build.rs`). So every
//! process of the proxy runs the allocator this binary was built with, and the runner's
//! counters, which follow forks, count all of them.
//!
//! When the proxy answers a request on its port (forwarded to the upstream), the driver prints
//! `BENCH_LISTEN addr`. On SIGTERM Sōzu's main process stops its workers and returns, and the
//! driver prints its result line.
//!
//! Two differences from the `sozu` binary, neither on the path that serves requests: the
//! allocator (the suite's, not jemalloc), and no `harden_process`, which makes every process
//! non-dumpable, so that the runner could not read the workers' memory.
// Sōzu's logging macros (`info!`, `trace!`), as its binary imports them.
#[macro_use]
extern crate sozu_command_lib;

// The `sozu` crate's `src/worker.rs`, unchanged, which its library keeps private. Only the
// worker entry runs from here: the rest of the file forks a worker, which the main process
// does from the library's own copy.
#[allow(dead_code)]
mod worker {
    include!(env!("SOZU_WORKER_RS"));
}

use bench_app::{Driver, fail};
use clap::Parser;
use sozu::cli::{Args, SubCmd};
// The worker module names its crate's `util` as `crate::util`.
use sozu::util;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("worker") {
        run_worker();
        return;
    }
    let mut d = Driver::start("sozu");
    match d.job.as_str() {
        "proxy" => proxy(&mut d),
        _ => fail("usage: sozu proxy upstream=HOST:PORT [workers=N]"),
    }
    d.finish_server();
}

/// A worker the main process started, as Sōzu's `main` runs one.
fn run_worker() {
    bench_alloc::init();
    let args = Args::parse();
    let SubCmd::Worker { fd, scm, configuration_state_fd, id, command_buffer_size, max_command_buffer_size } = args.cmd
    else {
        fail("expected a worker command")
    };
    // Sōzu's default for a worker started without an explicit maximum.
    let max = max_command_buffer_size.unwrap_or(command_buffer_size * 2);
    if let Err(e) = worker::begin_worker_process(fd, scm, configuration_state_fd, id, command_buffer_size, max) {
        fail(&format!("worker {id}: {e}"));
    }
}

fn proxy(d: &mut Driver) {
    let upstream: SocketAddr =
        d.args.str("upstream", "").parse().unwrap_or_else(|_| fail("upstream=HOST:PORT is required"));
    // The CPUs it may run on, which the runner sets before exec.
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let workers = d.args.usize("workers", cpus);
    d.report().param("workers", workers as u64);

    let dir = std::env::temp_dir().join(format!("bench-sozu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    // A free port, found by binding and letting go: nothing else binds loopback ports here.
    let addr = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap_or_else(|e| fail(&format!("finding a port: {e}")));
    let config = dir.join("config.toml");
    std::fs::write(&config, configuration(&dir, addr, upstream, workers))
        .unwrap_or_else(|e| fail(&format!("{}: {e}", config.display())));

    let ready = std::thread::spawn(move || announce_when_ready(addr));
    let args = Args { config: Some(config.display().to_string()), timeout: None, json: false, cmd: SubCmd::Start };
    let result = sozu::command::begin_main_process(&args);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        fail(&format!("Sōzu: {e}"));
    }
    if ready.join().is_err() {
        fail("the proxy never answered");
    }
}

/// The configuration: Sōzu's defaults but for the listener, the one cluster, the worker
/// count, and logging only errors (to stdout, which the runner reads).
fn configuration(dir: &std::path::Path, addr: SocketAddr, upstream: SocketAddr, workers: usize) -> String {
    format!(
        r#"command_socket = "{socket}"
worker_count = {workers}
worker_automatic_restart = false
log_level = "error"
log_target = "stdout"
activate_listeners = true

[[listeners]]
protocol = "http"
address = "{addr}"

[clusters.bench]
protocol = "http"
load_balancing = "ROUND_ROBIN"
frontends = [{{ address = "{addr}", hostname = "{host}" }}]
backends = [{{ address = "{upstream}" }}]
"#,
        socket = dir.join("sozu.sock").display(),
        host = addr.ip(),
    )
}

/// Prints `BENCH_LISTEN` once a request through the proxy comes back 200.
fn announce_when_ready(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let request = format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(addr) {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut head = [0u8; 12];
            if s.write_all(request.as_bytes()).is_ok() && s.read_exact(&mut head).is_ok() && &head == b"HTTP/1.1 200" {
                println!("BENCH_LISTEN {addr}");
                let _ = std::io::stdout().flush();
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no answer from {addr}");
}
