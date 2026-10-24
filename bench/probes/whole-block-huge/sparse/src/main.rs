//! Large blocks allocated whole and then touched in the ways that decide whether huge pages
//! pay (docs/research/whole-block-huge.md), with asmalloc as the global allocator and whatever
//! `ASMALLOC_WBH` sets.
//!
//!   sparse key=value...
//!
//! - `case=random` (default): each block takes one 8-byte store at each of `pages/touch`
//!   random offsets: a hash table sized far beyond its use. `case=prefix`: each block is
//!   written from its start for `touch` KiB: a buffer used only in a short prefix.
//!   `case=dense`: every page of each block is written, then `reads` random 8-byte loads go
//!   over the whole block: the case huge pages are for.
//! - `size=<MiB>` (default 64), `blocks=<n>` held at once (default 4), `touch` (default 64,
//!   a 64th of the pages, or KiB for `prefix`), `zero=1` allocates with `alloc_zeroed`,
//!   `lives=<n>` rounds of allocate, touch, hold, free (default 6), `hold=<ms>` held before the
//!   second sample (default 0), `reads=<n>` (default 0).
//!
//! One line per life: the time to allocate and touch (TSC kcycles a block), the time of the
//! reads (TSC cycles a read), the process's anonymous and huge-page-backed memory (MiB) while
//! the blocks are held, again after `hold`, and the minor faults of the life.
use std::alloc::{GlobalAlloc, Layout};
use std::time::Duration;

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

fn case() -> String {
    for a in std::env::args().skip(1) {
        if let Some(v) = a.strip_prefix("case=") {
            return v.to_string();
        }
    }
    "random".into()
}

/// (anonymous, AnonHugePages) of this process, MiB.
fn rollup() -> (f64, f64) {
    let s = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    let field = |k: &str| {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 1024.0
    };
    (field("Anonymous:"), field("AnonHugePages:"))
}

fn minflt() -> u64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let after = s.rsplit_once(')').map_or("", |(_, r)| r);
    after.split_whitespace().nth(7).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn tsc() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn main() {
    let case = case();
    let size = arg("size", 64) << 20;
    let blocks = arg("blocks", 4);
    let touch = arg("touch", 64);
    let zero = arg("zero", 0) != 0;
    let lives = arg("lives", 6);
    let hold = arg("hold", 0);
    let reads = arg("reads", 0);
    let page = 4096;
    let pages = size / page;
    let layout = Layout::from_size_align(size, 64).unwrap();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    println!(
        "# case={case} size={}MiB blocks={blocks} touch={touch} zero={} lives={lives} hold={hold} reads={reads} wbh={}",
        size >> 20,
        zero as u8,
        std::env::var("ASMALLOC_WBH").unwrap_or_default()
    );
    let mut sink = 0u64;
    for life in 0..lives {
        let f0 = minflt();
        let t0 = tsc();
        let mut held = Vec::with_capacity(blocks);
        for _ in 0..blocks {
            // SAFETY: a non-zero layout.
            let p = unsafe { if zero { A.alloc_zeroed(layout) } else { A.alloc(layout) } };
            assert!(!p.is_null());
            match case.as_str() {
                "random" => {
                    for _ in 0..pages / touch {
                        let at = (rng.next() as usize % (size / 8)) * 8;
                        // SAFETY: inside the block.
                        unsafe { p.add(at).cast::<u64>().write(at as u64) };
                    }
                }
                "prefix" => {
                    // SAFETY: inside the block.
                    unsafe { p.write_bytes(1, (touch << 10).min(size)) };
                }
                "dense" => {
                    for i in 0..pages {
                        // SAFETY: inside the block.
                        unsafe { p.add(i * page).cast::<u64>().write(i as u64) };
                    }
                }
                _ => panic!("case={case}"),
            }
            held.push(p);
        }
        let t1 = tsc();
        let mut rc = 0;
        if reads > 0 {
            let r0 = tsc();
            for _ in 0..reads {
                let b = held[rng.next() as usize % blocks];
                let at = (rng.next() as usize % (size / 8)) * 8;
                // SAFETY: inside the block; every page of it written (dense) or never written
                // (reads of untouched memory read zero).
                sink = sink.wrapping_add(unsafe { b.add(at).cast::<u64>().read_volatile() });
            }
            rc = (tsc() - r0) / reads as u64;
        }
        let (anon, huge) = rollup();
        let (anon2, huge2) = if hold > 0 {
            std::thread::sleep(Duration::from_millis(hold as u64));
            rollup()
        } else {
            (anon, huge)
        };
        for p in held {
            // SAFETY: allocated above with this layout.
            unsafe { A.dealloc(p, layout) };
        }
        let faults = minflt() - f0;
        println!(
            "life {life} kcyc_per_block {:.0} cyc_per_read {rc} anon {anon:.1} huge {huge:.1} anon_after {anon2:.1} huge_after {huge2:.1} faults {faults}",
            (t1 - t0) as f64 / 1e3 / blocks as f64
        );
    }
    let (anon, huge) = rollup();
    println!("end anon {anon:.1} huge {huge:.1} sink {}", sink & 1);
}
