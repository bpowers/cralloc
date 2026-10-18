//! What it costs to move a large block by `mremap` against copying it, by size, for blocks
//! written densely and sparsely, with both ends aligned to 2 MiB and not, and what either does
//! to another thread of the process while it runs. Wall-clock, pinned to RF_CPU (default 13);
//! the other thread is pinned to RF_CPU2 (default 12). Results are in
//! `docs/research/zen5-roofline.md`, section 7.
//!
//! usage: remap_cost [sizes|placements|stall] [max size in MiB]
//!
//! `sizes` times, from the same starting state each time (source written, destination clean):
//! - remap: the two probes and the `mremap` itself, each timed alone, and the fresh `mmap` and
//!   advice that a moved-in range needs when it is given up;
//! - copy: the allocator's present move, 2 MiB pieces with the source's pages returned behind
//!   each, into a destination whose pages are not resident and into one whose pages are;
//! - sparse copy: the same, skipping source pages that are all zero.
//!
//! `placements` times the `mremap` alone by how the two ends are aligned.
//!
//! `stall` runs a second thread that takes page faults, or only computes, and reports the
//! longest pause it saw while the first thread moved a block.

#[path = "../remap_sys.rs"]
mod sys;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;
use sys::*;

const REGION: usize = 4 * GB;
const REGIONS: usize = 6;
const PIECE: usize = HUGE;

fn ns(f: impl FnOnce()) -> u64 {
    let t = Instant::now();
    f();
    t.elapsed().as_nanos() as u64
}

fn us(v: u64) -> f64 {
    v as f64 / 1e3
}

/// The fastest and the middle of a set of times.
fn low_mid(v: &mut [u64]) -> (u64, u64) {
    v.sort_unstable();
    (v[0], v[v.len() / 2])
}

/// Writes the block: every page, or one byte in every MiB (and always the first page).
fn write_block(p: *mut u8, len: usize, dense: bool) {
    touch(p, len, if dense { PAGE } else { MB }, 0xA5);
}

/// The allocator's present move: copy a piece, return the source's pages behind it.
unsafe fn copy_pieces(from: *mut u8, to: *mut u8, len: usize, sparse: bool) {
    let mut done = 0;
    while done < len {
        let step = PIECE.min(len - done);
        unsafe {
            if sparse {
                copy_sparse(from.add(done), to.add(done), step);
            } else {
                std::ptr::copy_nonoverlapping(from.add(done), to.add(done), step);
            }
            if len > PIECE {
                madvise(from.add(done), step, MADV_DONTNEED);
            }
        }
        done += step;
    }
}

struct Bench {
    base: *mut u8,
}

impl Bench {
    fn new() -> Bench {
        let base = reserve(REGIONS * REGION, REGION);
        for r in 0..REGIONS {
            prepare_region(unsafe { base.add(r * REGION) }, REGION, REGION);
        }
        Bench { base }
    }

    /// Source and destination for a block of `len`, both in the middle of their regions'
    /// mappings. Not aligned means the two differ in their offset within a 2 MiB unit, so the
    /// kernel cannot move whole page tables.
    fn ends(&self, len: usize, aligned: bool) -> (*mut u8, *mut u8) {
        assert!(len + 2 * HUGE <= REGION);
        let (a, b) = if aligned { (HUGE, HUGE) } else { (HUGE + MB, HUGE + 512 * KB) };
        unsafe { (self.base.add(a), self.base.add(REGION + b)) }
    }

    fn clean(&self, src: *mut u8, dst: *mut u8, len: usize, healed: bool) {
        unsafe {
            madvise(src, len, MADV_DONTNEED);
            madvise(dst, len, MADV_DONTNEED);
        }
        if healed {
            assert_eq!(heal(dst, len), 0);
        }
    }
}

fn reps(len: usize) -> usize {
    match len {
        l if l <= 16 * MB => 40,
        l if l <= 256 * MB => 10,
        l if l <= GB => 5,
        _ => 3,
    }
}

fn label(len: usize) -> String {
    if len >= GB {
        format!("{:.2} GiB", len as f64 / GB as f64)
    } else if len >= MB {
        format!("{} MiB", len / MB)
    } else {
        format!("{} KiB", len / KB)
    }
}

/// The `mremap` alone for dense blocks, by how the two ends sit against the 2 MiB and 1 GiB
/// units whose page tables the kernel can move whole, and into a destination whose own pages
/// are still resident (which the call must free first).
fn placements(max: usize) {
    let bench = Bench::new();
    println!("mremap of a dense block, microseconds: fastest (middle)");
    println!(
        "{:>9} | {:>17} {:>17} {:>17} {:>17} | {:>22}",
        "size", "both at 1 GiB", "both at 2 MiB", "same odd offset", "different offsets", "2 MiB, resident dest"
    );
    for len in [8 * MB, 64 * MB, GB, 3 * GB] {
        if len > max {
            continue;
        }
        let n = if len >= GB { 5 } else { 20 };
        let mut cells = vec![];
        for (a, b, dirty) in [
            (0, 0, false),
            (HUGE, HUGE, false),
            (HUGE + MB, HUGE + MB, false),
            (HUGE + MB, HUGE + 512 * KB, false),
            (HUGE, HUGE, true),
        ] {
            let (src, dst) = unsafe { (bench.base.add(a), bench.base.add(REGION + b)) };
            let mut times = vec![];
            for _ in 0..n {
                write_block(src, len, true);
                if dirty {
                    touch(dst, len, PAGE, 1);
                }
                times.push(ns(|| assert_eq!(remap(src, dst, len), 0)));
                bench.clean(src, dst, len, true);
            }
            let (low, mid) = low_mid(&mut times);
            cells.push(format!("{:.1} ({:.1})", us(low), us(mid)));
        }
        println!(
            "{:>9} | {:>17} {:>17} {:>17} {:>17} | {:>22}",
            label(len),
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            cells[4]
        );
    }
}

fn sizes(max: usize) {
    let bench = Bench::new();
    let list = [
        64 * KB,
        128 * KB,
        256 * KB,
        512 * KB,
        MB,
        2 * MB,
        4 * MB,
        8 * MB,
        16 * MB,
        64 * MB,
        256 * MB,
        GB,
        4 * GB - 4 * HUGE,
    ];
    println!("times in microseconds: fastest (middle) of the repeats");
    println!(
        "{:>8} {:>6} {:>9} | {:>15} {:>13} {:>13} {:>15} {:>15} | {:>19} {:>19} {:>19} | {:>10}",
        "size",
        "fill",
        "ends",
        "mremap",
        "probe",
        "cold probe",
        "heal, purged",
        "purge",
        "copy, fresh dest",
        "copy, resident dest",
        "sparse copy",
        "RSS after"
    );
    for &len in list.iter().filter(|&&l| l <= max) {
        for dense in [true, false] {
            for aligned in [true, false] {
                let (src, dst) = bench.ends(len, aligned);
                let n = reps(len);
                let (mut t_remap, mut t_probe, mut t_cold, mut t_heal, mut t_purge) =
                    (vec![], vec![], vec![], vec![], vec![]);
                let (mut t_copy, mut t_copy_res, mut t_sparse) = (vec![], vec![], vec![]);
                let mut rss = [0u64; 3];
                for _ in 0..n {
                    write_block(src, len, dense);
                    t_probe.push(ns(|| assert_eq!(probe(src, len), ENOMEM)));
                    t_cold.push(ns(|| assert_eq!(unsafe { madvise(src, PAGE, MADV_COLD) }, 0)));
                    t_remap.push(ns(|| assert_eq!(remap(src, dst, len), 0)));
                    rss[0] = rss_kb();
                    t_purge.push(ns(|| assert_eq!(unsafe { madvise(dst, len, MADV_DONTNEED) }, 0)));
                    t_heal.push(ns(|| assert_eq!(heal(dst, len), 0)));
                    bench.clean(src, dst, len, false);

                    write_block(src, len, dense);
                    t_copy.push(ns(|| unsafe { copy_pieces(src, dst, len, false) }));
                    rss[1] = rss_kb();
                    bench.clean(src, dst, len, false);

                    // A destination whose pages are still resident, as a retained slot's are.
                    // Only worth timing where a whole copy of the block fits beside it.
                    if len <= 256 * MB {
                        write_block(src, len, dense);
                        touch(dst, len, PAGE, 1);
                        t_copy_res.push(ns(|| unsafe { copy_pieces(src, dst, len, false) }));
                        bench.clean(src, dst, len, false);
                    }

                    write_block(src, len, dense);
                    t_sparse.push(ns(|| unsafe { copy_pieces(src, dst, len, true) }));
                    rss[2] = rss_kb();
                    assert_eq!(unsafe { *dst }, 0xA5);
                    bench.clean(src, dst, len, false);
                }
                let cell = |v: &mut Vec<u64>| -> String {
                    if v.is_empty() {
                        return "-".to_string();
                    }
                    let (low, mid) = low_mid(v);
                    format!("{:.1} ({:.1})", us(low), us(mid))
                };
                println!(
                    "{:>8} {:>6} {:>9} | {:>15} {:>13} {:>13} {:>15} {:>15} | {:>19} {:>19} {:>19} | {}/{}/{} MiB",
                    label(len),
                    if dense { "dense" } else { "sparse" },
                    if aligned { "aligned" } else { "unaligned" },
                    cell(&mut t_remap),
                    cell(&mut t_probe),
                    cell(&mut t_cold),
                    cell(&mut t_heal),
                    cell(&mut t_purge),
                    cell(&mut t_copy),
                    cell(&mut t_copy_res),
                    cell(&mut t_sparse),
                    rss[0] / 1024,
                    rss[1] / 1024,
                    rss[2] / 1024
                );
            }
        }
    }
}

fn tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// The longest gap between two iterations of the second thread, in ticks, since it was last
/// cleared, and how many iterations it has done.
static GAP: AtomicU64 = AtomicU64::new(0);
static ITERATIONS: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Victim {
    /// Only computes: a pause is an interrupt (the TLB shootdown) or the scheduler.
    Compute,
    /// Takes page faults in a mapping of its own.
    FaultElsewhere,
    /// Takes page faults in another part of the mapping the block is moved out of.
    FaultSource,
    /// Takes page faults in another part of the mapping the block is moved into.
    FaultDest,
    /// Makes and removes a small mapping, which needs the address-space lock for writing.
    Map,
}

fn victim(kind: Victim, area: usize, cpu: usize) {
    pin(cpu);
    let area = area as *mut u8;
    let len = 32 * MB;
    let mut page = 0;
    let mut last = tsc();
    while !STOP.load(Relaxed) {
        match kind {
            Victim::Compute => {
                for _ in 0..64 {
                    std::hint::spin_loop();
                }
            }
            Victim::Map => unsafe {
                let p = mmap(std::ptr::null_mut(), PAGE, RW, ANON, -1, 0);
                munmap(p, PAGE);
            },
            _ => {
                unsafe { area.add(page * PAGE).write_volatile(1) };
                page += 1;
                if page == len / PAGE {
                    page = 0;
                    // Not counted as a pause: start the clock again after recycling.
                    unsafe { madvise(area, len, MADV_DONTNEED) };
                    last = tsc();
                }
            }
        }
        let now = tsc();
        GAP.fetch_max(now - last, Relaxed);
        last = now;
        ITERATIONS.fetch_add(1, Relaxed);
    }
}

/// Runs `op` with the gap cleared just before, and returns its duration in ns and the longest
/// gap the second thread saw across it, in ticks.
fn watch(op: impl FnOnce()) -> (u64, u64) {
    GAP.store(0, Relaxed);
    let t = ns(op);
    // Let the iteration that was in flight finish and be recorded.
    let seen = ITERATIONS.load(Relaxed);
    let wait = Instant::now();
    while ITERATIONS.load(Relaxed) < seen + 2 && wait.elapsed().as_millis() < 100 {
        std::hint::spin_loop();
    }
    (t, GAP.load(Relaxed))
}

fn stall(cpu2: usize, max: usize) {
    let bench = Bench::new();
    // Ticks per nanosecond.
    let (t0, c0) = (Instant::now(), tsc());
    std::thread::sleep(std::time::Duration::from_millis(200));
    let rate = (tsc() - c0) as f64 / t0.elapsed().as_nanos() as f64;
    let own = unsafe { mmap(std::ptr::null_mut(), 32 * MB, RW, ANON, -1, 0) };
    println!("the other thread's longest pause while the first moves a dense block, microseconds: fastest / middle / slowest of the repeats (the move's own time, middle)");
    println!(
        "{:>16} {:>26} | {:>28} {:>28} {:>28} {:>28}",
        "other thread", "block", "idle, as long as the last", "mremap", "copy in pieces", "purge, fresh mmap, advice"
    );
    for kind in [Victim::Compute, Victim::FaultElsewhere, Victim::FaultSource, Victim::FaultDest, Victim::Map] {
        let area = match kind {
            Victim::FaultSource => unsafe { bench.base.add(REGION / 2) },
            Victim::FaultDest => unsafe { bench.base.add(REGION + REGION / 2) },
            _ => own,
        } as usize;
        STOP.store(false, Relaxed);
        let handle = std::thread::spawn(move || victim(kind, area, cpu2));
        std::thread::sleep(std::time::Duration::from_millis(50));
        for (len, aligned) in [(4 * MB, false), (64 * MB, true), (64 * MB, false), (GB, true), (GB, false)] {
            if len > max {
                continue;
            }
            let (src, dst) = bench.ends(len, aligned);
            let n = if len >= GB { 7 } else { 25 };
            let mut rows: [(Vec<u64>, Vec<u64>); 4] = Default::default();
            for _ in 0..n {
                write_block(src, len, true);
                let (t, g) = watch(|| assert_eq!(remap(src, dst, len), 0));
                rows[1].0.push(t);
                rows[1].1.push(g);
                // Giving the moved-in range up: purge it, then a fresh mapping and its advice.
                let (t, g) = watch(|| {
                    unsafe { madvise(dst, len, MADV_DONTNEED) };
                    assert_eq!(heal(dst, len), 0);
                });
                rows[3].0.push(t);
                rows[3].1.push(g);
                bench.clean(src, dst, len, false);
                // A quiet window as long as the move was.
                let (t, g) = watch(|| {
                    let start = Instant::now();
                    while (start.elapsed().as_nanos() as u64) < t {
                        std::hint::spin_loop();
                    }
                });
                rows[0].0.push(t);
                rows[0].1.push(g);
                write_block(src, len, true);
                let (t, g) = watch(|| unsafe { copy_pieces(src, dst, len, false) });
                rows[2].0.push(t);
                rows[2].1.push(g);
                bench.clean(src, dst, len, false);
            }
            let cell = |row: &mut (Vec<u64>, Vec<u64>)| -> String {
                let (_, mid) = low_mid(&mut row.0);
                row.1.sort_unstable();
                let g = &row.1;
                let to_us = |ticks: u64| ticks as f64 / rate / 1e3;
                format!(
                    "{:.1} / {:.1} / {:.0} ({:.0})",
                    to_us(g[0]),
                    to_us(g[g.len() / 2]),
                    to_us(g[g.len() - 1]),
                    us(mid)
                )
            };
            let block = format!("{} {}", label(len), if aligned { "aligned" } else { "unaligned" });
            let [a, b, c, d] = &mut rows;
            println!(
                "{:>16} {:>26} | {:>28} {:>28} {:>28} {:>28}",
                format!("{kind:?}"),
                block,
                cell(a),
                cell(b),
                cell(c),
                cell(d)
            );
        }
        STOP.store(true, Relaxed);
        handle.join().unwrap();
    }
}

fn main() {
    let cpu: usize = std::env::var("RF_CPU").ok().and_then(|s| s.parse().ok()).unwrap_or(13);
    let cpu2: usize = std::env::var("RF_CPU2").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    pin(cpu);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let max = args.get(1).and_then(|v| v.parse::<usize>().ok()).map_or(usize::MAX, |mb| mb * MB);
    match args.first().map(String::as_str) {
        Some("stall") => stall(cpu2, max),
        Some("placements") => placements(max),
        _ => sizes(max),
    }
}
