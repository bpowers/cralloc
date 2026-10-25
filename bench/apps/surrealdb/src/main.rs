//! SurrealDB 3.3.4 with its in-memory engine, as a server for the load generator:
//!
//!   surrealdb serve check=FILE [records=N]
//!
//! runs SurrealDB as its own binary does (`surrealdb_server::init`, the whole of the `surreal`
//! binary's `main`) in a child process, this binary again with SurrealDB's own command line
//! (`surreal start memory`, on a loopback port, without authentication, warnings only), since
//! SurrealDB reads its command line itself; the runner's counters and memory follow it. Before
//! it announces the address, the driver fills table `user` with `records` records (default
//! 750,000: a name, an e-mail address with a unique index, an age, a country with an index,
//! three tags and a paragraph of text) through SurrealDB's HTTP API, then sends every distinct
//! query of `check` (whole HTTP requests, the load's own file) and requires each to succeed;
//! it checks them again after the load, on SIGTERM, before it stops SurrealDB. (One distinct
//! query in 97: the load has thousands.)
//!
//! What differs from the `surreal` binary: the allocator (the suite's, not jemalloc: its
//! `allocator` feature is off), and only the in-memory engine and HTTP built in.
// As the `surreal` binary's main.rs sets it: the layout of SurrealDB's async query engine
// nests deeper than the default limit.
#![recursion_limit = "256"]
use bench_app::{Driver, fail};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

const COUNTRIES: &[&str] = &[
    "AR", "AT", "AU", "BE", "BR", "CA", "CH", "CL", "CN", "CO", "CZ", "DE", "DK", "EG", "ES", "FI", "FR", "GB", "GR",
    "HU", "ID", "IE", "IL", "IN", "IT", "JP", "KE", "KR", "MX", "MY", "NG", "NL", "NO", "NZ", "PE", "PH", "PK", "PL",
    "PT", "RO", "SE", "SG", "TH", "TR", "TW", "UA", "US", "VN", "ZA", "ZM",
];
const WORDS: &[&str] = &[
    "river", "stone", "quiet", "garden", "north", "signal", "paper", "window", "summer", "copper", "letter", "harbor",
    "orange", "valley", "engine", "silver", "market", "forest", "winter", "candle", "bridge", "travel", "music",
    "island", "pocket", "morning", "planet", "yellow", "anchor", "violet", "thunder", "meadow",
];

fn main() -> ExitCode {
    // SurrealDB's process: this binary run again with SurrealDB's own command line.
    if std::env::args().nth(1).as_deref() == Some("start") {
        bench_alloc::init();
        return surrealdb_server::init(surrealdb_core::CommunityComposer());
    }
    let mut d = Driver::start("surrealdb");
    match d.job.as_str() {
        "serve" => serve(&mut d),
        _ => fail("usage: surrealdb serve check=FILE [records=N]"),
    }
    d.finish_server();
    ExitCode::SUCCESS
}

fn serve(d: &mut Driver) {
    let records = d.args.num("records", 750_000);
    let checks = distinct_bodies(Path::new(&d.args.str("check", "")));
    let addr = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap_or_else(|e| fail(&format!("finding a port: {e}")));

    // SIGTERM is taken with sigwait below, so it is blocked from here on in this thread, the
    // only one; SurrealDB's process gets the default mask back before it runs.
    let term = sigset(libc::SIGTERM);
    // SAFETY: a valid signal set, and a null old set.
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &term, std::ptr::null_mut()) };
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(&format!("current_exe: {e}")));
    let mut cmd = Command::new(exe);
    cmd.args(["start", "--bind", &addr.to_string(), "--unauthenticated", "--log", "warn", "--no-banner", "memory"]);
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
    let mut child = cmd.spawn().unwrap_or_else(|e| fail(&format!("starting SurrealDB: {e}")));
    let pid = child.id() as libc::pid_t;
    let kill = || {
        // SAFETY: kill on our own child, which has not been reaped.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while !matches!(call(addr, "GET", "/health", b""), Ok((200, _))) {
        if let Ok(Some(st)) = child.try_wait() {
            fail(&format!("SurrealDB exited as it started: {st}"));
        }
        if Instant::now() > deadline {
            kill();
            fail("SurrealDB never answered /health");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Err(e) = fill(addr, records) {
        kill();
        fail(&format!("filling the table: {e}"));
    }
    if let Err(e) = check(addr, &checks) {
        kill();
        fail(&format!("before the load: {e}"));
    }
    d.report().param("records", records).param("checked_queries", checks.len() as u64);
    println!("BENCH_LISTEN {addr}");
    let _ = std::io::stdout().flush();

    let mut sig = 0;
    // SAFETY: a valid blocked set and an out pointer.
    unsafe { libc::sigwait(&term, &mut sig) };
    let after = check(addr, &checks);
    // SAFETY: kill on our own child, which has not been reaped.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let st = child.wait().unwrap_or_else(|e| fail(&format!("waiting for SurrealDB: {e}")));
    if let Err(e) = after {
        fail(&format!("after the load: {e}"));
    }
    if !st.success() {
        fail(&format!("SurrealDB stopped with {st}"));
    }
}

/// The schema and `records` records, in batches of 1,000.
fn fill(addr: SocketAddr, records: u64) -> Result<(), String> {
    sql(
        addr,
        "DEFINE NAMESPACE IF NOT EXISTS bench; USE NS bench; \
         DEFINE DATABASE IF NOT EXISTS bench; USE DB bench; \
         DEFINE TABLE user SCHEMALESS; \
         DEFINE INDEX user_email ON user FIELDS email UNIQUE; \
         DEFINE INDEX user_country ON user FIELDS country;",
    )?;
    let mut q = String::new();
    let mut n = 0;
    while n < records {
        let end = (n + 1000).min(records);
        q.clear();
        q.push_str("INSERT INTO user [");
        for i in n..end {
            if i > n {
                q.push(',');
            }
            record(&mut q, i);
        }
        q.push_str("] RETURN NONE;");
        sql(addr, &q)?;
        n = end;
    }
    Ok(())
}

fn record(q: &mut String, i: u64) {
    let mut x = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5eed;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let country = COUNTRIES[(next() % COUNTRIES.len() as u64) as usize];
    let age = 18 + next() % 70;
    let name = WORDS[(next() % WORDS.len() as u64) as usize];
    let tags: Vec<&str> = (0..3).map(|_| WORDS[(next() % WORDS.len() as u64) as usize]).collect();
    let words = 25 + next() % 40;
    let mut bio = String::new();
    for k in 0..words {
        if k > 0 {
            bio.push(' ');
        }
        bio.push_str(WORDS[(next() % WORDS.len() as u64) as usize]);
    }
    let _ = write!(
        q,
        "{{id:{i},name:'{name} {i}',email:'user{i}@example.com',age:{age},country:'{country}',\
         tags:['{}','{}','{}'],bio:'{bio}',visits:0}}",
        tags[0], tags[1], tags[2]
    );
}

/// Runs SurrealQL statements and requires every one to succeed.
fn sql(addr: SocketAddr, q: &str) -> Result<String, String> {
    let (status, body) = call(addr, "POST", "/sql", q.as_bytes())?;
    if status != 200 || body.contains("\"status\":\"ERR\"") || !body.contains("\"status\":\"OK\"") {
        let shown: String = body.chars().take(400).collect();
        let query: String = q.chars().take(200).collect();
        return Err(format!("{status}: {query}: {shown}"));
    }
    Ok(body)
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

/// One HTTP/1.1 request on a connection of its own, with SurrealDB's namespace and database
/// headers; the status and body.
fn call(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nsurreal-ns: bench\r\n\
         surreal-db: bench\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| format!("send: {e}"))?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| format!("receive: {e}"))?;
    let text = String::from_utf8_lossy(&out);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("no response head")?;
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("no status")?;
    Ok((status, body.to_string()))
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
    // Every distinct query would be thousands; a fixed sample of them is the check.
    set.into_iter().step_by(97).collect()
}

fn check(addr: SocketAddr, bodies: &[Vec<u8>]) -> Result<(), String> {
    for b in bodies {
        sql(addr, &String::from_utf8_lossy(b))?;
    }
    Ok(())
}
