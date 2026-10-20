//! A closed-loop RESP load client: CONNS connections, each sending batches of PIPELINE commands
//! and reading every reply before the next batch, until TOTAL commands have been sent in all.
//! Half SETs of values from 16 bytes to 4 KiB, half GETs, over KEYS keys, from a fixed seed
//! per connection, so the server sees the same work every run.
//!
//! Usage: resp-load HOST:PORT CONNS PIPELINE TOTAL [KEYS]
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

async fn conn(addr: String, seed: u64, pipeline: usize, batches: u64, keys: u64) -> u64 {
    let stream = TcpStream::connect(&addr).await.expect("connect");
    stream.set_nodelay(true).unwrap();
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    let mut s = seed * 0x9e37_79b9_7f4a_7c15 + 1;
    let mut buf = Vec::new();
    let mut line = String::new();
    let mut hits = 0;
    let value = vec![b'v'; 4096];
    for _ in 0..batches {
        buf.clear();
        for _ in 0..pipeline {
            let key = format!("key:{}", next(&mut s) % keys);
            if next(&mut s) % 2 == 0 {
                let len = 16 + (next(&mut s) % 4080) as usize;
                buf.extend_from_slice(
                    format!("*3\r\n$3\r\nSET\r\n${}\r\n{}\r\n${}\r\n", key.len(), key, len).as_bytes(),
                );
                buf.extend_from_slice(&value[..len]);
                buf.extend_from_slice(b"\r\n");
            } else {
                buf.extend_from_slice(format!("*2\r\n$3\r\nGET\r\n${}\r\n{}\r\n", key.len(), key).as_bytes());
            }
        }
        w.write_all(&buf).await.expect("write");
        for _ in 0..pipeline {
            line.clear();
            r.read_line(&mut line).await.expect("read");
            if let Some(n) = line.strip_prefix('$') {
                let n: i64 = n.trim().parse().expect("bulk length");
                if n >= 0 {
                    let mut body = vec![0; n as usize + 2];
                    r.read_exact(&mut body).await.expect("bulk");
                    hits += 1;
                }
            } else if line.starts_with('-') {
                panic!("server error: {line}");
            }
        }
    }
    hits
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let addr = a[1].clone();
    let conns: u64 = a[2].parse().unwrap();
    let pipeline: usize = a[3].parse().unwrap();
    let total: u64 = a[4].parse().unwrap();
    let keys: u64 = a.get(5).map(|k| k.parse().unwrap()).unwrap_or(100_000);
    let batches = total / conns / pipeline as u64;
    let start = std::time::Instant::now();
    let tasks: Vec<_> = (0..conns).map(|i| tokio::spawn(conn(addr.clone(), i, pipeline, batches, keys))).collect();
    let mut hits = 0;
    for t in tasks {
        hits += t.await.unwrap();
    }
    let secs = start.elapsed().as_secs_f64();
    let sent = batches * conns * pipeline as u64;
    println!("commands {sent} hits {hits} seconds {secs:.3} rate {:.0}", sent as f64 / secs);
}
