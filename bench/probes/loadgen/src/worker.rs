//! One generator thread: a pool of keep-alive connections, each carrying one request at a
//! time, driven by a spin loop so that a request goes out within a microsecond or two of
//! when it is due and a response is timed within a microsecond or two of its arrival.
//!
//! A request that comes due while every connection of the pool is busy waits for one, and
//! its latency still counts from when it was due: a stall in the server is charged to every
//! request it holds up, not only to the one it was serving.

use crate::http;
use crate::plan::{self, Arrivals, Kind, Segment, Spacing};
use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Token};
use std::io::{self, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The request found no idle connection when it came due.
pub const MISSED: u16 = 1;
/// The response was not a 2xx, or the connection failed or the run ended first.
pub const ERROR: u16 = 2;
/// Sent by a closed-loop segment; latency is from the send.
pub const CLOSED_LOOP: u16 = 4;

#[derive(Clone, Copy, Debug)]
pub struct Rec {
    /// When the request was due (open loop) or sent (closed loop), from the run's start.
    pub due: u64,
    /// From `due` to the end of the response.
    pub lat: u64,
    pub seg: u16,
    pub flags: u16,
}

#[derive(Clone, Copy)]
struct Inflight {
    due: u64,
    seg: u16,
    flags: u16,
}

struct Conn {
    stream: Option<TcpStream>,
    buf: Vec<u8>,
    /// Bytes of the request written so far, while a write is pending.
    wpos: Option<usize>,
    inflight: Option<Inflight>,
}

pub struct Config<'a> {
    pub addr: SocketAddr,
    pub request: &'a [u8],
    pub segs: &'a [Segment],
    pub conns: usize,
    pub threads: usize,
    pub thread: usize,
    pub spacing: Spacing,
    pub seed: u64,
    /// How long after the plan ends to wait for responses still outstanding.
    pub drain: Duration,
    pub start: Instant,
}

#[derive(Default, Debug)]
pub struct Counters {
    pub reconnects: u64,
    pub unexpected: u64,
}

fn open(addr: SocketAddr) -> io::Result<TcpStream> {
    let s = std::net::TcpStream::connect(addr)?;
    s.set_nodelay(true)?;
    s.set_nonblocking(true)?;
    Ok(TcpStream::from_std(s))
}

struct Driver<'a> {
    cfg: &'a Config<'a>,
    poll: Poll,
    conns: Vec<Conn>,
    idle: Vec<usize>,
    recs: Vec<Rec>,
    counters: Counters,
}

impl<'a> Driver<'a> {
    fn now(&self) -> u64 {
        self.cfg.start.elapsed().as_nanos() as u64
    }

    fn attach(&mut self, i: usize) -> io::Result<()> {
        let mut s = open(self.cfg.addr)?;
        self.poll.registry().register(&mut s, Token(i), Interest::READABLE | Interest::WRITABLE)?;
        let c = &mut self.conns[i];
        c.stream = Some(s);
        c.buf.clear();
        c.wpos = None;
        Ok(())
    }

    fn send(&mut self, i: usize, f: Inflight) {
        self.conns[i].inflight = Some(f);
        self.conns[i].wpos = Some(0);
        self.write(i);
    }

    fn write(&mut self, i: usize) {
        let req = self.cfg.request;
        let c = &mut self.conns[i];
        let (Some(s), Some(mut at)) = (c.stream.as_mut(), c.wpos) else { return };
        loop {
            match s.write(&req[at..]) {
                Ok(n) => {
                    at += n;
                    if at == req.len() {
                        c.wpos = None;
                        return;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    c.wpos = Some(at);
                    return;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.fail(i);
                    return;
                }
            }
        }
    }

    fn record(&mut self, f: Inflight, end: u64, flags: u16) {
        self.recs.push(Rec { due: f.due, lat: end.saturating_sub(f.due), seg: f.seg, flags: f.flags | flags });
    }

    /// The connection broke: charge its request as an error and replace it.
    fn fail(&mut self, i: usize) {
        let now = self.now();
        if let Some(f) = self.conns[i].inflight.take() {
            self.record(f, now, ERROR);
        }
        if let Some(mut s) = self.conns[i].stream.take() {
            let _ = self.poll.registry().deregister(&mut s);
        }
        self.idle.retain(|&c| c != i);
        self.counters.reconnects += 1;
        if self.attach(i).is_ok() {
            self.idle.push(i);
        }
    }

    fn read(&mut self, i: usize, tmp: &mut [u8]) {
        loop {
            let Some(s) = self.conns[i].stream.as_mut() else { return };
            match s.read(tmp) {
                Ok(0) => {
                    self.fail(i);
                    return;
                }
                Ok(n) => self.conns[i].buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.fail(i);
                    return;
                }
            }
        }
        let now = self.now();
        loop {
            match http::parse(&self.conns[i].buf) {
                http::Parse::Incomplete => return,
                http::Parse::Bad => {
                    self.fail(i);
                    return;
                }
                http::Parse::Done { status, len, close } => {
                    self.conns[i].buf.drain(..len);
                    match self.conns[i].inflight.take() {
                        Some(f) => {
                            self.record(f, now, if (200..300).contains(&status) { 0 } else { ERROR });
                            self.idle.push(i);
                        }
                        None => self.counters.unexpected += 1,
                    }
                    if close {
                        self.fail(i);
                        return;
                    }
                }
            }
        }
    }

    fn close_all(&mut self) {
        for c in &mut self.conns {
            if let Some(mut s) = c.stream.take() {
                let _ = self.poll.registry().deregister(&mut s);
            }
        }
        self.idle.clear();
    }
}

/// Runs this thread's share of the plan; returns every request it finished or gave up on.
pub fn run(cfg: &Config) -> io::Result<(Vec<Rec>, Counters)> {
    let mut d = Driver {
        cfg,
        poll: Poll::new()?,
        conns: (0..cfg.conns)
            .map(|_| Conn { stream: None, buf: Vec::with_capacity(16 << 10), wpos: None, inflight: None })
            .collect(),
        idle: (0..cfg.conns).rev().collect(),
        recs: Vec::new(),
        counters: Counters::default(),
    };
    for i in 0..cfg.conns {
        d.attach(i)?;
    }
    // Growing the record vector mid-run would copy it while requests wait, so it is sized and
    // touched up front, assuming closed-loop segments run no faster than CLOSED_MAX.
    const CLOSED_MAX: f64 = 1.5e6;
    let expected: f64 = cfg
        .segs
        .iter()
        .map(|s| {
            let secs = (s.end_ns - s.begin_ns) as f64 / 1e9;
            match s.kind {
                Kind::Rate(r) => r * secs * 1.01 / cfg.threads as f64,
                Kind::Closed => CLOSED_MAX * secs / cfg.threads as f64,
                _ => 0.0,
            }
        })
        .sum();
    let cap = expected as usize + 4096;
    d.recs.resize(cap, Rec { due: 0, lat: 0, seg: 0, flags: 0 });
    d.recs.clear();
    let mut arrivals = Arrivals::new(cfg.segs, cfg.threads, cfg.thread, cfg.spacing, cfg.seed);
    let plan_end = cfg.segs.last().map_or(0, |s| s.end_ns);
    let drain = cfg.drain.as_nanos() as u64;
    let mut events = Events::with_capacity(256);
    let mut tmp = vec![0u8; 64 << 10];
    let mut head_missed = false;
    let mut closed = false;
    while Instant::now() < cfg.start {
        std::hint::spin_loop();
    }
    loop {
        let now = d.now();
        while let Some((due, seg)) = arrivals.peek() {
            if due > now {
                break;
            }
            match d.idle.pop() {
                Some(c) => {
                    let flags = if head_missed { MISSED } else { 0 };
                    head_missed = false;
                    d.send(c, Inflight { due, seg: seg as u16, flags });
                    arrivals.advance();
                }
                None => {
                    head_missed = true;
                    break;
                }
            }
        }
        let cur = plan::at(cfg.segs, now);
        let all_idle = d.idle.len() == cfg.conns;
        match cur.map(|i| cfg.segs[i].kind) {
            Some(Kind::Closed) => {
                while let Some(c) = d.idle.pop() {
                    d.send(c, Inflight { due: d.now(), seg: cur.unwrap() as u16, flags: CLOSED_LOOP });
                }
            }
            Some(Kind::Close) if !closed && all_idle && arrivals.peek().is_none() => {
                d.close_all();
                closed = true;
            }
            _ => {}
        }
        if now >= plan_end && arrivals.peek().is_none() && (closed || all_idle) {
            break;
        }
        if now >= plan_end + drain {
            for i in 0..cfg.conns {
                if let Some(f) = d.conns[i].inflight.take() {
                    d.record(f, now, ERROR);
                }
            }
            // Requests never sent count as failures too, with their latency to this point.
            while let Some((due, seg)) = arrivals.peek() {
                d.recs.push(Rec { due, lat: now.saturating_sub(due), seg: seg as u16, flags: MISSED | ERROR });
                arrivals.advance();
            }
            break;
        }
        d.poll.poll(&mut events, Some(Duration::ZERO))?;
        for ev in events.iter() {
            let i = ev.token().0;
            if ev.is_writable() {
                d.write(i);
            }
            if ev.is_readable() || ev.is_read_closed() || ev.is_error() {
                d.read(i, &mut tmp);
            }
        }
    }
    d.close_all();
    Ok((d.recs, d.counters))
}
