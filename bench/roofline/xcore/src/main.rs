//! Cost of moving a cache line between cores: strict ping-pong, a contended atomic counter,
//! and a producer whose 64-byte objects are read and written by a consumer before it reuses
//! them. Wall-clock times, single run.
//!
//! CPUs: RF_CPU (default 2) against its SMT sibling RF_SMT (18), a core in the same L3
//! RF_NEAR (3) and a core in the other L3 RF_FAR (10).
use std::sync::atomic::{AtomicU64, Ordering::*};
use std::sync::Arc;
use std::time::Instant;
extern "C" {
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
}
fn pin(cpu: usize) {
    let mask = [1u64 << cpu, 0, 0, 0];
    unsafe { sched_setaffinity(0, 32, mask.as_ptr()) };
}
#[repr(align(128))]
struct Line(AtomicU64);

// Strict ping-pong: each side waits for its parity, then stores. One line transfer per handoff.
fn pingpong(a: usize, b: usize, n: u64) -> f64 {
    let x = Arc::new(Line(AtomicU64::new(0)));
    let y = x.clone();
    let t = std::thread::spawn(move || {
        pin(b);
        let mut i = 1;
        while i < 2 * n {
            while y.0.load(Relaxed) != i { std::hint::spin_loop() }
            y.0.store(i + 1, Relaxed);
            i += 2;
        }
    });
    pin(a);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let t0 = Instant::now();
    let mut i = 0;
    while i < 2 * n {
        while x.0.load(Relaxed) != i { std::hint::spin_loop() }
        x.0.store(i + 1, Relaxed);
        i += 2;
    }
    t.join().unwrap();
    t0.elapsed().as_nanos() as f64 / (2 * n) as f64
}

// Both sides hammer fetch_add on one line; ns per op per thread.
fn hammer(a: usize, b: usize, n: u64) -> f64 {
    let x = Arc::new(Line(AtomicU64::new(0)));
    let y = x.clone();
    let t = std::thread::spawn(move || {
        pin(b);
        for _ in 0..n { y.0.fetch_add(1, Relaxed); }
    });
    pin(a);
    let t0 = Instant::now();
    for _ in 0..n { x.0.fetch_add(1, Relaxed); }
    let d = t0.elapsed().as_nanos() as f64 / n as f64;
    t.join().unwrap();
    d
}

// Producer writes 64-byte objects' first word, consumer reads then writes it (a remote free
// into an intrusive list touches the object). Objects flow through a ring of pointers.
fn handoff(a: usize, b: usize, n: usize) -> f64 {
    let buf: Vec<Line> = (0..4096).map(|_| Line(AtomicU64::new(0))).collect();
    let buf = Arc::new(buf);
    let b2 = buf.clone();
    let head = Arc::new(Line(AtomicU64::new(0)));
    let h2 = head.clone();
    let t = std::thread::spawn(move || {
        pin(b);
        let mut seen = 0u64;
        let mut acc = 0u64;
        while (seen as usize) < n {
            let h = h2.0.load(Acquire);
            while seen < h {
                let o = &b2[(seen % 4096) as usize].0;
                acc += o.load(Relaxed);
                o.store(acc, Relaxed);
                seen += 1;
            }
            std::hint::spin_loop();
        }
        acc
    });
    pin(a);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let t0 = Instant::now();
    for i in 0..n as u64 {
        // stay at most 2048 objects ahead so the consumer always works on lines we last wrote
        buf[(i % 4096) as usize].0.store(i, Relaxed);
        if i % 64 == 63 { head.0.store(i + 1, Release); }
    }
    head.0.store(n as u64, Release);
    let d = t0.elapsed().as_nanos() as f64 / n as f64;
    t.join().unwrap();
    d
}

fn env(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn main() {
    let a = env("RF_CPU", 2);
    for (name, b) in [("SMT sibling", env("RF_SMT", 18)), ("same L3", env("RF_NEAR", 3)), ("other L3", env("RF_FAR", 10))] {
        let name = format!("{name} ({a},{b})");
        let pp = pingpong(a, b, 2_000_000);
        let hm = hammer(a, b, 20_000_000);
        let ho = handoff(a, b, 20_000_000);
        println!("{name:<22} ping-pong {pp:>6.1} ns/handoff   contended fetch_add {hm:>6.1} ns/op   producer cost with consumer touching objects {ho:>5.2} ns/obj");
    }
    pin(a);
    let x = Line(AtomicU64::new(0));
    let t0 = Instant::now();
    for _ in 0..50_000_000u64 { x.0.fetch_add(1, Relaxed); }
    println!("uncontended fetch_add: {:.2} ns/op", t0.elapsed().as_nanos() as f64 / 5e7);
}
