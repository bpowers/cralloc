//! HTTP/2 over cleartext with prior knowledge (h2c), for servers that speak it on loopback:
//! the same open-loop discipline as HTTP/1.1, with each connection carrying up to `streams`
//! requests at once. A request takes a free stream slot when it comes due, so the slots play
//! the part HTTP/1.1's connections do; latency still runs from when it was due.
//!
//! Just enough of the protocol for one fixed request: the request's header block is encoded
//! once, statelessly (literals without indexing), and sent with its body in DATA frames within
//! the peer's flow-control windows; responses are decoded with HPACK (`loona-hpack`), which
//! keeps the per-connection table in step. A response succeeds when its `:status` is 2xx and,
//! if its trailers carry `grpc-status`, that is 0. The client opens its receive windows wide
//! at the start and tops up the connection's after each gigabyte, so it never limits the
//! server.

use crate::plan::{self, Arrivals, Kind};
use crate::worker::{CLOSED_LOOP, Config, Counters, ERROR, MISSED, Rec};
use mio::net::TcpStream;
use mio::{Events, Interest, Poll, Token};
use std::collections::{HashMap, VecDeque};
use std::io::{self, ErrorKind, Read, Write};
use std::time::{Duration, Instant};

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const DATA: u8 = 0;
const HEADERS: u8 = 1;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const CONTINUATION: u8 = 9;
const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PADDED: u8 = 0x8;
const PRIORITY: u8 = 0x20;
const MAX_WINDOW: u32 = (1 << 31) - 1;
const DEFAULT_WINDOW: i64 = 65_535;
/// Record flags beside worker's: the request's body waited for the connection's send window,
/// or for its stream's.
pub const CONN_WINDOW: u16 = 8;
pub const STREAM_WINDOW: u16 = 16;

/// The request: its header block, already HPACK-encoded, and its body.
pub struct Request {
    pub block: Vec<u8>,
    pub body: Vec<u8>,
    pub streams: usize,
}

/// A header field as a literal without indexing, with a new name: stateless, so the same
/// bytes serve every request on every connection.
fn literal(out: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    out.push(0x00);
    int(out, 0, 7, name.len());
    out.extend_from_slice(name);
    int(out, 0, 7, value.len());
    out.extend_from_slice(value);
}

/// An HPACK integer with an `n`-bit prefix, after the bits `first` of its first octet.
fn int(out: &mut Vec<u8>, first: u8, n: u32, mut v: usize) {
    let max = (1usize << n) - 1;
    if v < max {
        out.push(first | v as u8);
        return;
    }
    out.push(first | max as u8);
    v -= max;
    while v >= 128 {
        out.push((v % 128) as u8 | 0x80);
        v /= 128;
    }
    out.push(v as u8);
}

/// The request's header block: `POST path` with the given content type and headers (names
/// lowercased, as HTTP/2 requires) and the body's length.
pub fn request(host: &str, path: &str, content_type: &str, headers: &[String], body: &[u8], streams: usize) -> Request {
    let mut b = Vec::new();
    b.push(0x83); // :method POST, static index 3
    b.push(0x86); // :scheme http, static index 6
    literal(&mut b, b":path", path.as_bytes());
    literal(&mut b, b":authority", host.as_bytes());
    if !content_type.is_empty() {
        literal(&mut b, b"content-type", content_type.as_bytes());
    }
    literal(&mut b, b"content-length", body.len().to_string().as_bytes());
    for h in headers {
        let (k, v) = h.split_once(':').unwrap_or((h, ""));
        literal(&mut b, k.trim().to_ascii_lowercase().as_bytes(), v.trim().as_bytes());
    }
    Request { block: b, body: body.to_vec(), streams }
}

fn frame(out: &mut Vec<u8>, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    let n = payload.len();
    out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8, kind, flags]);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
}

#[derive(Clone, Copy)]
struct Inflight {
    due: u64,
    seg: u16,
    flags: u16,
}

#[derive(Default)]
struct Slot {
    inflight: Option<Inflight>,
    id: u32,
    status: u16,
    ok: bool,
    /// Body bytes still to send, and this stream's send window.
    left: usize,
    window: i64,
}

struct Conn {
    stream: Option<TcpStream>,
    rbuf: Vec<u8>,
    wbuf: Vec<u8>,
    wpos: usize,
    decoder: loona_hpack::Decoder<'static>,
    next_id: u32,
    window: i64,
    initial: i64,
    max_frame: usize,
    ids: HashMap<u32, usize>,
    /// Slots whose body waits for window.
    blocked: VecDeque<usize>,
    /// A header block in progress (HEADERS then CONTINUATION), and its stream and flags.
    partial: Option<(u32, u8, Vec<u8>)>,
    received: u64,
}

impl Conn {
    fn new() -> Conn {
        Conn {
            stream: None,
            rbuf: Vec::with_capacity(64 << 10),
            wbuf: Vec::with_capacity(64 << 10),
            wpos: 0,
            decoder: loona_hpack::Decoder::new(),
            next_id: 1,
            window: DEFAULT_WINDOW,
            initial: DEFAULT_WINDOW,
            max_frame: 16_384,
            ids: HashMap::new(),
            blocked: VecDeque::new(),
            partial: None,
            received: 0,
        }
    }
}

struct Driver<'a> {
    cfg: &'a Config<'a>,
    req: &'a Request,
    poll: Poll,
    conns: Vec<Conn>,
    slots: Vec<Slot>,
    idle: Vec<usize>,
    recs: Vec<Rec>,
    counters: Counters,
}

impl Driver<'_> {
    fn now(&self) -> u64 {
        self.cfg.start.elapsed().as_nanos() as u64
    }

    fn per(&self) -> usize {
        self.req.streams
    }

    fn attach(&mut self, c: usize) -> io::Result<()> {
        let s = std::net::TcpStream::connect(self.cfg.addr)?;
        s.set_nodelay(true)?;
        s.set_nonblocking(true)?;
        let mut s = TcpStream::from_std(s);
        self.poll.registry().register(&mut s, Token(c), Interest::READABLE | Interest::WRITABLE)?;
        let mut conn = Conn::new();
        conn.stream = Some(s);
        conn.wbuf.extend_from_slice(PREFACE);
        // No push; stream windows as wide as they go; then the connection's too.
        let mut settings = Vec::new();
        settings.extend_from_slice(&2u16.to_be_bytes());
        settings.extend_from_slice(&0u32.to_be_bytes());
        settings.extend_from_slice(&4u16.to_be_bytes());
        settings.extend_from_slice(&MAX_WINDOW.to_be_bytes());
        frame(&mut conn.wbuf, SETTINGS, 0, 0, &settings);
        frame(&mut conn.wbuf, WINDOW_UPDATE, 0, 0, &(MAX_WINDOW - DEFAULT_WINDOW as u32).to_be_bytes());
        self.conns[c] = conn;
        self.flush(c);
        Ok(())
    }

    fn send(&mut self, slot: usize, f: Inflight) {
        let c = slot / self.per();
        let conn = &mut self.conns[c];
        let id = conn.next_id;
        conn.next_id += 2;
        conn.ids.insert(id, slot);
        let body = self.req.body.len();
        let flags = END_HEADERS | if body == 0 { END_STREAM } else { 0 };
        frame(&mut conn.wbuf, HEADERS, flags, id, &self.req.block);
        let window = conn.initial;
        self.slots[slot] = Slot { inflight: Some(f), id, status: 0, ok: true, left: body, window };
        self.send_data(slot);
        self.flush(c);
    }

    /// Sends what the windows allow of `slot`'s body; queues the rest.
    fn send_data(&mut self, slot: usize) {
        let c = slot / self.per();
        let body = &self.req.body;
        loop {
            let (conn, s) = (&mut self.conns[c], &mut self.slots[slot]);
            if s.left == 0 {
                return;
            }
            let room = conn.window.min(s.window).min(conn.max_frame as i64);
            if room <= 0 {
                // Which window held the body back, kept in the request's record: the server's
                // flow control is part of its latency, and the flags say where.
                if let Some(f) = s.inflight.as_mut() {
                    f.flags |= if conn.window <= 0 { CONN_WINDOW } else { STREAM_WINDOW };
                }
                conn.blocked.push_back(slot);
                return;
            }
            let n = (s.left as i64).min(room) as usize;
            let at = body.len() - s.left;
            s.left -= n;
            conn.window -= n as i64;
            s.window -= n as i64;
            frame(&mut conn.wbuf, DATA, if s.left == 0 { END_STREAM } else { 0 }, s.id, &body[at..at + n]);
        }
    }

    fn unblock(&mut self, c: usize) {
        let blocked: Vec<usize> = self.conns[c].blocked.drain(..).collect();
        for slot in blocked {
            self.send_data(slot);
        }
        self.flush(c);
    }

    fn flush(&mut self, c: usize) {
        let conn = &mut self.conns[c];
        let Some(s) = conn.stream.as_mut() else { return };
        while conn.wpos < conn.wbuf.len() {
            match s.write(&conn.wbuf[conn.wpos..]) {
                Ok(n) => conn.wpos += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.fail(c);
                    return;
                }
            }
        }
        conn.wbuf.clear();
        conn.wpos = 0;
    }

    fn record(&mut self, f: Inflight, end: u64, flags: u16) {
        self.recs.push(Rec { due: f.due, lat: end.saturating_sub(f.due), seg: f.seg, flags: f.flags | flags });
    }

    fn finish(&mut self, slot: usize, now: u64, error: bool) {
        let s = std::mem::take(&mut self.slots[slot]);
        let c = slot / self.per();
        self.conns[c].ids.remove(&s.id);
        if let Some(f) = s.inflight {
            let good = !error && s.ok && (200..300).contains(&s.status);
            self.record(f, now, if good { 0 } else { ERROR });
            self.idle.push(slot);
        }
    }

    /// The connection broke: charge its streams' requests as errors and replace it.
    fn fail(&mut self, c: usize) {
        let now = self.now();
        let per = self.per();
        if let Some(mut s) = self.conns[c].stream.take() {
            let _ = self.poll.registry().deregister(&mut s);
        }
        for slot in c * per..(c + 1) * per {
            if let Some(f) = self.slots[slot].inflight.take() {
                self.record(f, now, ERROR);
            }
            self.slots[slot] = Slot::default();
        }
        self.idle.retain(|&s| s / per != c);
        self.counters.reconnects += 1;
        if self.attach(c).is_ok() {
            self.idle.extend(c * per..(c + 1) * per);
        }
    }

    fn read(&mut self, c: usize, tmp: &mut [u8]) {
        loop {
            let Some(s) = self.conns[c].stream.as_mut() else { return };
            match s.read(tmp) {
                Ok(0) => {
                    self.fail(c);
                    return;
                }
                Ok(n) => self.conns[c].rbuf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.fail(c);
                    return;
                }
            }
        }
        let now = self.now();
        let mut at = 0;
        loop {
            let b = &self.conns[c].rbuf[at..];
            if b.len() < 9 {
                break;
            }
            let len = (b[0] as usize) << 16 | (b[1] as usize) << 8 | b[2] as usize;
            if b.len() < 9 + len {
                break;
            }
            let (kind, flags) = (b[3], b[4]);
            let id = u32::from_be_bytes([b[5], b[6], b[7], b[8]]) & 0x7fff_ffff;
            let payload = b[9..9 + len].to_vec();
            at += 9 + len;
            if !self.frame_in(c, kind, flags, id, payload, now) {
                return;
            }
        }
        self.conns[c].rbuf.drain(..at);
        self.flush(c);
    }

    /// Handles one frame; false when the connection was failed.
    fn frame_in(&mut self, c: usize, kind: u8, flags: u8, id: u32, mut p: Vec<u8>, now: u64) -> bool {
        match kind {
            DATA => {
                let conn = &mut self.conns[c];
                conn.received += p.len() as u64;
                if conn.received > 1 << 30 {
                    let top = conn.received as u32;
                    frame(&mut conn.wbuf, WINDOW_UPDATE, 0, 0, &top.to_be_bytes());
                    conn.received = 0;
                }
                if flags & END_STREAM != 0
                    && let Some(&slot) = self.conns[c].ids.get(&id)
                {
                    self.finish(slot, now, false);
                }
            }
            HEADERS | CONTINUATION => {
                if kind == HEADERS {
                    let mut start = 0;
                    let mut end = p.len();
                    if flags & PADDED != 0 {
                        let pad = *p.first().unwrap_or(&0) as usize;
                        start = 1;
                        end = end.saturating_sub(pad);
                    }
                    if flags & PRIORITY != 0 {
                        start += 5;
                    }
                    p = p.get(start..end).unwrap_or(&[]).to_vec();
                    self.conns[c].partial = Some((id, flags, p));
                } else {
                    match self.conns[c].partial.as_mut() {
                        Some((_, f, block)) => {
                            block.extend_from_slice(&p);
                            *f |= flags & END_HEADERS;
                        }
                        None => {
                            self.fail(c);
                            return false;
                        }
                    }
                }
                let Some((sid, f, _)) = &self.conns[c].partial else { return true };
                if f & END_HEADERS == 0 {
                    return true;
                }
                let (sid, end_stream) = (*sid, f & END_STREAM != 0);
                let (_, _, block) = self.conns[c].partial.take().unwrap();
                let (mut status, mut grpc_bad) = (None, false);
                let decoded = self.conns[c].decoder.decode_with_cb(&block, |k, v| {
                    if &*k == b":status" {
                        status = std::str::from_utf8(&v).ok().and_then(|s| s.parse::<u16>().ok());
                    } else if &*k == b"grpc-status" && &*v != b"0" {
                        grpc_bad = true;
                    }
                });
                if decoded.is_err() {
                    self.fail(c);
                    return false;
                }
                if let Some(&slot) = self.conns[c].ids.get(&sid) {
                    let s = &mut self.slots[slot];
                    if let Some(st) = status {
                        s.status = st;
                    }
                    if grpc_bad {
                        s.ok = false;
                    }
                    if end_stream {
                        self.finish(slot, now, false);
                    }
                }
            }
            RST_STREAM => {
                if let Some(&slot) = self.conns[c].ids.get(&id) {
                    self.finish(slot, now, true);
                }
            }
            SETTINGS if flags & ACK == 0 => {
                for kv in p.as_chunks::<6>().0 {
                    let key = u16::from_be_bytes([kv[0], kv[1]]);
                    let v = u32::from_be_bytes([kv[2], kv[3], kv[4], kv[5]]);
                    match key {
                        4 => {
                            let at = (self.cfg.thread * self.cfg.conns + c) as u16;
                            self.counters.windows.push((now, at, 1, v));
                            let conn = &mut self.conns[c];
                            let delta = v as i64 - conn.initial;
                            conn.initial = v as i64;
                            let per = self.req.streams;
                            for slot in c * per..(c + 1) * per {
                                if self.slots[slot].inflight.is_some() {
                                    self.slots[slot].window += delta;
                                }
                            }
                        }
                        5 => self.conns[c].max_frame = v as usize,
                        _ => {}
                    }
                }
                frame(&mut self.conns[c].wbuf, SETTINGS, ACK, 0, &[]);
                self.unblock(c);
            }
            PING if flags & ACK == 0 => frame(&mut self.conns[c].wbuf, PING, ACK, 0, &p),
            GOAWAY => {
                self.fail(c);
                return false;
            }
            WINDOW_UPDATE if p.len() == 4 => {
                let inc = (u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff) as i64;
                if id == 0 {
                    let at = (self.cfg.thread * self.cfg.conns + c) as u16;
                    self.counters.windows.push((now, at, 0, inc as u32));
                    self.conns[c].window += inc;
                } else if let Some(&slot) = self.conns[c].ids.get(&id) {
                    self.slots[slot].window += inc;
                }
                self.unblock(c);
            }
            _ => {}
        }
        true
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

/// Runs this thread's share of the plan over HTTP/2, as `worker::run` does over HTTP/1.1.
pub fn run(cfg: &Config, req: &Request) -> io::Result<(Vec<Rec>, Counters)> {
    let per = req.streams;
    let slots = cfg.conns * per;
    let mut d = Driver {
        cfg,
        req,
        poll: Poll::new()?,
        conns: (0..cfg.conns).map(|_| Conn::new()).collect(),
        slots: (0..slots).map(|_| Slot::default()).collect(),
        idle: (0..slots).rev().collect(),
        recs: Vec::new(),
        counters: Counters::default(),
    };
    for c in 0..cfg.conns {
        d.attach(c)?;
    }
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
    d.recs.resize(expected as usize + 4096, Rec { due: 0, lat: 0, seg: 0, flags: 0 });
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
                Some(s) => {
                    let flags = if head_missed { MISSED } else { 0 };
                    head_missed = false;
                    d.send(s, Inflight { due, seg: seg as u16, flags });
                    arrivals.advance();
                }
                None => {
                    head_missed = true;
                    break;
                }
            }
        }
        let cur = plan::at(cfg.segs, now);
        let all_idle = d.idle.len() == slots;
        match cur.map(|i| cfg.segs[i].kind) {
            Some(Kind::Closed) => {
                while let Some(s) = d.idle.pop() {
                    d.send(s, Inflight { due: d.now(), seg: cur.unwrap() as u16, flags: CLOSED_LOOP });
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
            for slot in 0..slots {
                if let Some(f) = d.slots[slot].inflight.take() {
                    d.record(f, now, ERROR);
                }
            }
            while let Some((due, seg)) = arrivals.peek() {
                d.recs.push(Rec { due, lat: now.saturating_sub(due), seg: seg as u16, flags: MISSED | ERROR });
                arrivals.advance();
            }
            break;
        }
        d.poll.poll(&mut events, Some(Duration::ZERO))?;
        for ev in events.iter() {
            let c = ev.token().0;
            if ev.is_writable() {
                d.flush(c);
            }
            if ev.is_readable() || ev.is_read_closed() || ev.is_error() {
                d.read(c, &mut tmp);
            }
        }
    }
    d.close_all();
    Ok((d.recs, d.counters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers() {
        let mut v = Vec::new();
        int(&mut v, 0, 7, 10);
        assert_eq!(v, [10]);
        v.clear();
        int(&mut v, 0, 5, 1337);
        assert_eq!(v, [31, 154, 10]);
    }

    #[test]
    fn header_block_decodes() {
        let r = request("h:1", "/p", "application/grpc", &["TE: trailers".into()], b"abc", 8);
        let mut d = loona_hpack::Decoder::new();
        let h = d.decode(&r.block).unwrap();
        let h: Vec<(String, String)> =
            h.into_iter().map(|(k, v)| (String::from_utf8(k).unwrap(), String::from_utf8(v).unwrap())).collect();
        assert_eq!(h[0], (":method".into(), "POST".into()));
        assert_eq!(h[1], (":scheme".into(), "http".into()));
        assert_eq!(h[2], (":path".into(), "/p".into()));
        assert!(h.contains(&("content-length".into(), "3".into())));
        assert!(h.contains(&("te".into(), "trailers".into())));
    }
}
