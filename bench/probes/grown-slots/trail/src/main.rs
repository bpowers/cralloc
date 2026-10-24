//! The trail that a rule for grown blocks' slots leaves (docs/research/grown-slots.md): many
//! buffers growing at once, on a heap that already holds a ballast of live memory, with
//! asmalloc as the global allocator and whatever `ASMALLOC_GROWN` sets.
//!
//!   trail key=value...
//!
//! - `ballast=<MiB>` live memory held throughout, half in 1 KiB boxes and half in 1 MiB blocks
//!   allocated whole (default 1024), so that the heap is large and its large classes hold
//!   free memory of their own: `churn=<n>` of the 1 MiB blocks are freed and allocated again
//!   between steps (default 4), as a server's request buffers would be;
//! - `n=<count>` buffers (default 512) grown round-robin by `chunk=<KiB>` (default 64) to
//!   `size=<KiB>` each (default 2048: a doubling past a move, the worst ratio of outgrown slot
//!   to live buffer);
//! - `rounds=<k>` times (default 3): grow them all, keep them for `keep_ms` (default 400),
//!   sampling, then free them all and wait `gap_ms` (default 400), sampling;
//! - `beat=1` (default) makes one small large allocation and free every millisecond while it
//!   waits, so that heartbeats run as in a busy program; `beat=0` leaves the program silent;
//! - `bg=1` starts asmalloc's background thread, which a server with threads of its own has;
//! - `collect=1` calls `Asmalloc::collect` at the end of each gap, so that the next round's
//!   buffers grow into returned slots, which is reuse and pays nothing toward the cap: the
//!   hardest case for a rule that keeps outgrown slots.
//!
//! Each round's summary line gives the peak while the buffers grew less what was live at the
//! end of the growth: the trail, at its largest.
//!
//! Prints one line per sample: round, phase, ms since the phase began, anonymous resident MiB,
//! live MiB (ballast and buffers), resident over live.
use std::time::{Duration, Instant};

#[global_allocator]
static A: asmalloc::Asmalloc = asmalloc::Asmalloc;

fn arg(name: &str, default: usize) -> usize {
    for a in std::env::args().skip(1) {
        if let Some(v) = a.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return v.parse().expect("a number");
        }
    }
    default
}

fn rss_anon() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("RssAnon:") {
            let kib: f64 = v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0.0);
            return kib / 1024.0;
        }
    }
    0.0
}

struct Sampler {
    peak: f64,
    peak_over: f64,
}

impl Sampler {
    fn sample(&mut self, round: usize, phase: &str, t: Instant, live: f64, print: bool) {
        let rss = rss_anon();
        if rss > self.peak {
            self.peak = rss;
            self.peak_over = rss - live;
        }
        if print {
            println!(
                "{round} {phase} {:.1} {rss:.1} {live:.1} {:.3}",
                t.elapsed().as_secs_f64() * 1e3,
                rss / live.max(1.0)
            );
        }
    }
}

fn wait(ms: usize, beat: bool, round: usize, phase: &str, live: f64, s: &mut Sampler) {
    let t = Instant::now();
    let mut next = 0.0;
    while t.elapsed() < Duration::from_millis(ms as u64) {
        if beat {
            let b = std::hint::black_box(vec![1u8; 48 << 10]);
            drop(b);
        }
        let el = t.elapsed().as_secs_f64() * 1e3;
        if el >= next {
            s.sample(round, phase, t, live, true);
            next += if el < 50.0 { 5.0 } else { 25.0 };
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    let ballast = arg("ballast", 1024);
    let churn = arg("churn", 4);
    let n = arg("n", 512);
    let chunk = arg("chunk", 64) << 10;
    let size = arg("size", 2048) << 10;
    let rounds = arg("rounds", 3);
    let keep_ms = arg("keep_ms", 400);
    let gap_ms = arg("gap_ms", 400);
    let beat = arg("beat", 1) != 0;
    let collect = arg("collect", 0) != 0;
    if arg("bg", 0) != 0 {
        asmalloc::Asmalloc::start_background().expect("the background thread starts");
    }
    let mib = 1usize << 20;
    let small: Vec<Box<[u8; 1024]>> = (0..ballast * 512).map(|i| Box::new([i as u8; 1024])).collect();
    let mut big: Vec<Vec<u8>> = (0..ballast / 2).map(|i| vec![i as u8; mib]).collect();
    let base = rss_anon();
    let ballast_live = (small.len() * 1024 + big.len() * mib) as f64 / mib as f64;
    println!("# ballast {ballast_live:.0} MiB, rss {base:.1}; n {n} size {} KiB chunk {} KiB", size >> 10, chunk >> 10);
    let mut s = Sampler { peak: 0.0, peak_over: 0.0 };
    let mut k = 0usize;
    let fill = vec![0x5au8; chunk];
    for round in 0..rounds {
        let t = Instant::now();
        let mut bufs: Vec<Vec<u8>> = (0..n).map(|_| Vec::new()).collect();
        let mut len = 0;
        let mut step = 0;
        while len < size {
            len += chunk;
            for b in bufs.iter_mut() {
                b.extend_from_slice(&fill);
            }
            for _ in 0..churn {
                let i = k % big.len().max(1);
                k += 1;
                if let Some(slot) = big.get_mut(i) {
                    *slot = Vec::new();
                    *slot = vec![k as u8; mib];
                }
            }
            step += 1;
            let live = ballast_live + (n * len) as f64 / mib as f64;
            s.sample(round, "grow", t, live, step % 4 == 0);
        }
        let live = ballast_live + (n * len) as f64 / mib as f64;
        wait(keep_ms, beat, round, "keep", live, &mut s);
        let grown_peak = s.peak;
        drop(bufs);
        wait(gap_ms, beat, round, "gap", ballast_live, &mut s);
        let after = rss_anon();
        if collect {
            asmalloc::Asmalloc::collect();
        }
        println!(
            "# round {round} peak {grown_peak:.1} over live {:.1}; end of gap {after:.1}; after collect {:.1} (ballast rss {base:.1})",
            grown_peak - (ballast_live + (n * size) as f64 / mib as f64),
            rss_anon()
        );
        s = Sampler { peak: 0.0, peak_over: 0.0 };
    }
    std::hint::black_box((&small, &big));
}
