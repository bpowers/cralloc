//! The Apollo Router 2.19.0, the GraphQL federation router, as a server for the load generator:
//!
//!   router serve subgraphs=HOST:PORT schema=FILE check=FILE
//!
//! runs the Router as its own binary does (`apollo_router::main`, the whole of the `router`
//! binary's `main`), with the supergraph `schema` and a configuration this driver writes: the
//! GraphQL endpoint on a loopback port, every subgraph at its path on `subgraphs` (the
//! subgraphs process the runner starts first, `subgraphs/`), no health check, homepage or
//! sandbox, client connections kept open between requests for up to 10 minutes, and the
//! Router's defaults otherwise (telemetry, caches, traffic shaping).
//!
//! The Router reads its command line itself, so the driver runs it in a child process, this
//! binary again with the Router's own arguments; the runner's counters and memory follow it.
//! Before it announces the Router's address the driver sends it every distinct query of
//! `check` (whole HTTP requests, the load's own file) and requires an answer with data and no
//! GraphQL errors, which a 200 status alone does not show; it checks them again after the load,
//! on SIGTERM, before it stops the Router.
//!
//! What differs from the `router` binary: the allocator (the suite's, not jemalloc wrapped in
//! the Router's per-task byte counter: its `global-allocator` feature is off), and anonymous
//! usage reporting off (it would reach the network).
use bench_app::{Driver, fail};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn main() {
    // The Router's process: this binary run again with the Router's own command line.
    if std::env::args().nth(1).is_some_and(|a| a.starts_with("--")) {
        bench_alloc::init();
        if let Err(e) = apollo_router::main() {
            eprintln!("router: {e}");
            std::process::exit(1);
        }
        return;
    }
    let mut d = Driver::start("router");
    match d.job.as_str() {
        "serve" => serve(&mut d),
        _ => fail("usage: router serve subgraphs=HOST:PORT schema=FILE check=FILE"),
    }
    d.finish_server();
}

fn serve(d: &mut Driver) {
    let subgraphs: SocketAddr =
        d.args.str("subgraphs", "").parse().unwrap_or_else(|_| fail("subgraphs=HOST:PORT is required"));
    let schema = PathBuf::from(d.args.str("schema", ""));
    if !schema.is_file() {
        fail("schema=FILE must name the supergraph");
    }
    let checks = distinct_bodies(Path::new(&d.args.str("check", "")));
    let dir = std::env::temp_dir().join(format!("bench-router-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    // A free port, found by binding and letting go: nothing else binds loopback ports here.
    let addr = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap_or_else(|e| fail(&format!("finding a port: {e}")));
    let config = dir.join("router.yaml");
    std::fs::write(&config, configuration(addr, subgraphs))
        .unwrap_or_else(|e| fail(&format!("{}: {e}", config.display())));

    // SIGTERM is taken with sigwait below, so it is blocked from here on in this thread, the
    // only one; the Router's process gets the default mask back before it runs.
    let term = sigset(libc::SIGTERM);
    // SAFETY: a valid signal set, and a null old set.
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &term, std::ptr::null_mut()) };
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(&format!("current_exe: {e}")));
    let mut cmd = Command::new(exe);
    cmd.arg(format!("--supergraph={}", schema.display()))
        .arg(format!("--config={}", config.display()))
        .arg("--anonymous-telemetry-disabled")
        .arg("--log=error");
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                let empty = sigset(0);
                libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn().unwrap_or_else(|e| fail(&format!("starting the Router: {e}")));
    let pid = child.id() as libc::pid_t;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(Some(st)) = child.try_wait() {
            fail(&format!("the Router exited as it started: {st}"));
        }
        match check(addr, &checks) {
            Ok(()) => break,
            Err(e) if Instant::now() > deadline => {
                // SAFETY: kill on our own child.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                fail(&format!("the Router never answered as it should: {e}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    d.report().param("checked_queries", checks.len() as u64);
    println!("BENCH_LISTEN {addr}");
    let _ = std::io::stdout().flush();

    let mut sig = 0;
    // SAFETY: a valid blocked set and an out pointer.
    unsafe { libc::sigwait(&term, &mut sig) };
    let after = check(addr, &checks);
    // SAFETY: kill on our own child, which has not been reaped.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let st = child.wait().unwrap_or_else(|e| fail(&format!("waiting for the Router: {e}")));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = after {
        fail(&format!("after the load: {e}"));
    }
    if !st.success() {
        fail(&format!("the Router stopped with {st}"));
    }
}

fn sigset(sig: libc::c_int) -> libc::sigset_t {
    // SAFETY: sigset_t is plain data that sigemptyset initialises.
    unsafe {
        let mut s: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut s);
        if sig != 0 {
            libc::sigaddset(&mut s, sig);
        }
        s
    }
}

/// The configuration: the Router's defaults but for where it listens and fetches, the
/// endpoints a benchmark has no use for, and how long a client's connection may wait between
/// requests. The Router closes one after 10 s (`header_read_timeout`), and at a low rate the
/// generator leaves some of its 64 connections that long unused; a pooled client would
/// reconnect, but the generator counts a closed connection as a broken run.
fn configuration(addr: SocketAddr, subgraphs: SocketAddr) -> String {
    let url = |name: &str| format!("    {name}: http://{subgraphs}/{name}\n");
    format!(
        "supergraph:\n  listen: {addr}\n  introspection: false\n\
         server:\n  http:\n    header_read_timeout: 10m\n\
         health_check:\n  enabled: false\n\
         homepage:\n  enabled: false\n\
         sandbox:\n  enabled: false\n\
         override_subgraph_url:\n{}{}{}{}",
        url("accounts"),
        url("inventory"),
        url("products"),
        url("reviews"),
    )
}

/// The distinct bodies of the whole HTTP requests in `file`, written back to back.
fn distinct_bodies(file: &Path) -> Vec<Vec<u8>> {
    let all = std::fs::read(file).unwrap_or_else(|e| fail(&format!("check={}: {e}", file.display())));
    let mut set = BTreeSet::new();
    let mut rest = &all[..];
    while !rest.is_empty() {
        let Some(head) = rest.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4) else {
            fail("check file: a request without the end of its head")
        };
        let len = std::str::from_utf8(&rest[..head])
            .ok()
            .and_then(|h| {
                h.lines().find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                })
            })
            .unwrap_or_else(|| fail("check file: a request without Content-Length"));
        if rest.len() < head + len {
            fail("check file: a request cut short");
        }
        set.insert(rest[head..head + len].to_vec());
        rest = &rest[head + len..];
    }
    if set.is_empty() {
        fail("check file: no requests");
    }
    set.into_iter().collect()
}

/// Sends each body as a GraphQL request and requires data and no errors in every answer.
fn check(addr: SocketAddr, bodies: &[Vec<u8>]) -> Result<(), String> {
    for body in bodies {
        let mut s = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let head = format!(
            "POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| format!("send: {e}"))?;
        let mut out = Vec::new();
        s.read_to_end(&mut out).map_err(|e| format!("receive: {e}"))?;
        let text = String::from_utf8_lossy(&out);
        let (head, body_text) = text.split_once("\r\n\r\n").ok_or("no response head")?;
        let query = String::from_utf8_lossy(body);
        if !head.starts_with("HTTP/1.1 200") {
            return Err(format!("{}: {query}", head.lines().next().unwrap_or("")));
        }
        if !body_text.contains("{\"data\":{") || body_text.contains("\"errors\"") {
            let shown: String = body_text.chars().take(400).collect();
            return Err(format!("{query}: {shown}"));
        }
    }
    Ok(())
}
