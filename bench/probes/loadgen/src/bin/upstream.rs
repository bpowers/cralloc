//! The upstream a proxy job forwards to: an HTTP/1.1 server on loopback that answers every
//! request with the same response, as fast as it can, so that the proxy in front of it is
//! what the run measures. Built with glibc beside the load generator, and never the program
//! under test.
//!
//!   upstream [--cpus LIST] [--body BYTES]
//!
//! One thread per CPU of `--cpus` (default: the CPUs it may run on), each pinned and each
//! accepting from the same listening socket. A request is read to the end of its body
//! (`Content-Length`; requests are never chunked here), and answered with 200 and a body of
//! `--body` bytes (default 4096). Connections are kept alive and requests may be pipelined.
//! Prints `BENCH_LISTEN addr` once it listens, and runs until it is killed.
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::process::exit;

const LISTENER: Token = Token(usize::MAX);

fn die(msg: &str) -> ! {
    eprintln!("upstream: {msg}");
    exit(2)
}

fn main() {
    let mut cpus: Vec<usize> = allowed_cpus();
    let mut body = 4096usize;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let v = args.next().unwrap_or_else(|| die(&format!("{flag} needs a value")));
        match flag.as_str() {
            "--cpus" => cpus = parse_cpus(&v).unwrap_or_else(|| die(&format!("bad CPU list `{v}`"))),
            "--body" => body = v.parse().unwrap_or_else(|_| die(&format!("bad size `{v}`"))),
            _ => die("usage: upstream [--cpus LIST] [--body BYTES]"),
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| die(&format!("bind: {e}")));
    listener.set_nonblocking(true).unwrap_or_else(|e| die(&format!("nonblocking: {e}")));
    let addr = listener.local_addr().unwrap_or_else(|e| die(&format!("local addr: {e}")));
    let mut response =
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {body}\r\n\r\n").into_bytes();
    // A JSON string of the right length, so that a proxy that looks at it sees a plausible body.
    response.push(b'"');
    response.extend(std::iter::repeat_n(b'x', body.saturating_sub(2)));
    if body >= 2 {
        response.push(b'"');
    }
    let response: &'static [u8] = Box::leak(response.into_boxed_slice());
    let threads: Vec<_> = cpus
        .iter()
        .map(|&cpu| {
            let l = listener.try_clone().unwrap_or_else(|e| die(&format!("clone listener: {e}")));
            std::thread::spawn(move || serve(cpu, TcpListener::from_std(l), response))
        })
        .collect();
    println!("BENCH_LISTEN {addr}");
    let _ = std::io::stdout().flush();
    for t in threads {
        let _ = t.join();
    }
}

struct Conn {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
}

fn serve(cpu: usize, mut listener: TcpListener, response: &'static [u8]) {
    pin(cpu);
    let mut poll = Poll::new().unwrap_or_else(|e| die(&format!("poll: {e}")));
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)
        .unwrap_or_else(|e| die(&format!("register: {e}")));
    let mut conns: HashMap<usize, Conn> = HashMap::new();
    let mut next = 0usize;
    let mut events = Events::with_capacity(256);
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        if let Err(e) = poll.poll(&mut events, None) {
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            die(&format!("poll: {e}"));
        }
        for ev in events.iter() {
            if ev.token() == LISTENER {
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nodelay(true);
                            let t = next;
                            next += 1;
                            if poll.registry().register(&mut stream, Token(t), Interest::READABLE).is_ok() {
                                conns.insert(t, Conn { stream, input: Vec::new(), output: Vec::new(), written: 0 });
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
                continue;
            }
            let t = ev.token().0;
            let Some(c) = conns.get_mut(&t) else { continue };
            let open = step(c, &mut chunk, response);
            let want =
                if c.written < c.output.len() { Interest::READABLE | Interest::WRITABLE } else { Interest::READABLE };
            if (!open || poll.registry().reregister(&mut c.stream, Token(t), want).is_err())
                && let Some(mut c) = conns.remove(&t)
            {
                let _ = poll.registry().deregister(&mut c.stream);
            }
        }
    }
}

/// Reads what has arrived, answers every complete request, and writes what it can. Returns
/// whether the connection is still open.
fn step(c: &mut Conn, chunk: &mut [u8], response: &[u8]) -> bool {
    loop {
        match c.stream.read(chunk) {
            Ok(0) => return false,
            Ok(n) => c.input.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    let mut used = 0;
    while let Some(len) = request_len(&c.input[used..]) {
        used += len;
        c.output.extend_from_slice(response);
    }
    c.input.drain(..used);
    while c.written < c.output.len() {
        match c.stream.write(&c.output[c.written..]) {
            Ok(n) => c.written += n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    if c.written == c.output.len() {
        c.output.clear();
        c.written = 0;
    }
    true
}

/// The length of the first complete request in `b`, head and body, if all of it is there.
fn request_len(b: &[u8]) -> Option<usize> {
    let head = b.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let mut body = 0;
    for line in b[..head].split(|&x| x == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(colon) = line.iter().position(|&x| x == b':')
            && line[..colon].eq_ignore_ascii_case(b"content-length")
        {
            body = std::str::from_utf8(&line[colon + 1..]).ok()?.trim().parse().ok()?;
        }
    }
    (b.len() >= head + body).then_some(head + body)
}

fn allowed_cpus() -> Vec<usize> {
    // SAFETY: cpu_set_t is plain data; sched_getaffinity fills it for this thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return vec![0];
        }
        (0..libc::CPU_SETSIZE as usize).filter(|&c| libc::CPU_ISSET(c, &set)).collect()
    }
}

fn parse_cpus(s: &str) -> Option<Vec<usize>> {
    let mut v = Vec::new();
    for part in s.split(',') {
        match part.split_once('-') {
            Some((a, b)) => v.extend(a.parse::<usize>().ok()?..=b.parse().ok()?),
            None => v.push(part.parse().ok()?),
        }
    }
    (!v.is_empty()).then_some(v)
}

fn pin(cpu: usize) {
    // SAFETY: cpu_set_t is plain data that CPU_ZERO/CPU_SET initialise in place, and
    // sched_setaffinity(0, ...) only reads it, for the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            die(&format!("cannot pin to CPU {cpu}: {}", std::io::Error::last_os_error()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::request_len;

    #[test]
    fn finds_the_end_of_a_request() {
        assert_eq!(request_len(b"GET / HTTP/1.1\r\nHost: h\r\n\r\n"), Some(27));
        let post = b"POST / HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}GET";
        assert_eq!(request_len(post), Some(post.len() - 3));
        assert_eq!(request_len(b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\n{}"), None);
        assert_eq!(request_len(b"GET / HTTP/1.1\r\n"), None);
    }
}
