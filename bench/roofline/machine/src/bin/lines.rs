//! The true coherence granule, and whether a neighbouring line comes along with a miss.
//!
//! ```text
//! lines regs                        what CTR_EL0 and DCZID_EL0 say
//! lines granule A B [ms] [rounds]   two threads on CPUs A and B write words at chosen offsets
//! lines neighbour [MiB]             a miss, then a load `d` bytes away: does it find the line there?
//! ```
//!
//! `granule` prints nanoseconds an operation for each pair of offsets, against one thread alone.
//! Pairs that share a coherence unit run many times slower. The boundary pairs (120/128, 248/256
//! and so on) sit one word apart across a power of two, which tells a unit of that size from a
//! distance rule.
use roofline_machine::*;
use std::arch::asm;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    /// A load and a store of the same word.
    Rmw,
    /// `ldadd`.
    Ldadd,
}

/// Nanoseconds an operation on a word, until the leader's clock says `ms`.
fn run(word: *mut u64, op: Op, stop: &AtomicBool, leader: bool, ms: u64) -> f64 {
    let limit = ms * tick_rate() / 1000;
    let t0 = ticks();
    let mut count = 0u64;
    // SAFETY: the caller's word is aligned and lives as long as this call.
    let atomic = unsafe { AtomicU64::from_ptr(word) };
    loop {
        match op {
            Op::Rmw => {
                for _ in 0..256 {
                    // SAFETY: inside the caller's live mapping; only this thread writes this word.
                    unsafe { word.write_volatile(word.read_volatile().wrapping_add(1)) };
                }
            }
            Op::Ldadd => {
                for _ in 0..256 {
                    atomic.fetch_add(1, Relaxed);
                }
            }
        }
        count += 256;
        if leader {
            if ticks() - t0 >= limit {
                stop.store(true, Relaxed);
                break;
            }
        } else if stop.load(Relaxed) {
            break;
        }
    }
    ns_of(ticks() - t0) / count as f64
}

fn pair(base: &Map, cpus: Option<(usize, usize)>, a: usize, b: usize, op: Op, ms: u64) -> (f64, f64) {
    let stop_map = Map::resident(1 << 16, Pages::Small);
    // SAFETY: inside a live mapping, aligned and zero or one; the mapping outlives the threads.
    let stop = unsafe { AtomicBool::from_ptr(stop_map.p.add(1 << 15).cast()) };
    stop.store(false, Relaxed);
    let Some((ca, cb)) = cpus else {
        return (run(base.p.wrapping_add(a).cast(), op, stop, true, ms), 0.0);
    };
    let barrier = Barrier::new(2);
    let (wa, wb) = (base.p as usize + a, base.p as usize + b);
    std::thread::scope(|s| {
        let (barrier_b, barrier_a) = (&barrier, &barrier);
        let hb = s.spawn(move || {
            pin(cb);
            barrier_b.wait();
            run(wb as *mut u64, op, stop, false, ms)
        });
        let ha = s.spawn(move || {
            pin(ca);
            barrier_a.wait();
            run(wa as *mut u64, op, stop, true, ms)
        });
        (ha.join().unwrap(), hb.join().unwrap())
    })
}

fn granule(a: usize, b: usize, ms: u64, rounds: usize) {
    let map = Map::resident(1 << 20, Pages::Small);
    // Offsets from the start of a page, chosen so a pair is one word apart across each boundary.
    let pairs = [
        (0, 8),
        (0, 16),
        (0, 32),
        (0, 56),
        (0, 64),
        (0, 96),
        (0, 120),
        (0, 128),
        (0, 192),
        (0, 256),
        (0, 512),
        (0, 1024),
        (0, 4096),
        (56, 64),
        (120, 128),
        (248, 256),
        (504, 512),
        (1016, 1024),
        (4088, 4096),
    ];
    println!("# CPUs {a} and {b}, {ms} ms a cell, median of {rounds} rounds, page {}", page());
    println!("# offsets   rmw_alone  rmw_a  rmw_b   ldadd_alone  ldadd_a  ldadd_b  (ns an operation)");
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut order: Vec<usize> = (0..pairs.len()).collect();
    let mut cells = vec![vec![vec![0f64; rounds]; 4]; pairs.len()];
    pin(a);
    let alone = |op| -> f64 {
        let mut v: Vec<f64> = (0..rounds).map(|_| pair(&map, None, 0, 0, op, ms).0).collect();
        median(&mut v)
    };
    let (alone_rmw, alone_ldadd) = (alone(Op::Rmw), alone(Op::Ldadd));
    for r in 0..rounds {
        rng.shuffle(&mut order);
        for &i in &order {
            let (oa, ob) = pairs[i];
            let (x, y) = pair(&map, Some((a, b)), oa, ob, Op::Rmw, ms);
            cells[i][0][r] = x;
            cells[i][1][r] = y;
            let (x, y) = pair(&map, Some((a, b)), oa, ob, Op::Ldadd, ms);
            cells[i][2][r] = x;
            cells[i][3][r] = y;
        }
    }
    for (i, &(oa, ob)) in pairs.iter().enumerate() {
        println!(
            "{oa:5}/{ob:<5} {alone_rmw:6.2} {:7.2} {:7.2}    {alone_ldadd:6.2} {:7.2} {:7.2}",
            median(&mut cells[i][0]),
            median(&mut cells[i][1]),
            median(&mut cells[i][2]),
            median(&mut cells[i][3]),
        );
    }
}

/// A chase over anchors in random order, each load missing every cache, with a second load `d`
/// bytes from each anchor. If the first miss brought the neighbour, the second is cheap.
fn neighbour(cpu: usize, mib: usize) {
    pin(cpu);
    let len = mib << 20;
    let map = Map::resident(len, Pages::Huge);
    println!("# {mib} MiB, huge pages back {} MiB, CPU {cpu}", huge_mib());
    let cycles = Counter::cycles(cpu);
    const SPACING: usize = 8 << 10;
    let anchors = len / SPACING;
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut order: Vec<usize> = (0..anchors).collect();
    rng.shuffle(&mut order);
    println!("# {anchors} anchors {SPACING} bytes apart in random order");
    println!("# d   cycles_a_step  ns_a_step  second_load_cycles");
    let run = |d: Option<isize>| -> Timed {
        let mut at = Vec::with_capacity(anchors * 2);
        for &i in &order {
            // An anchor mid-spacing, line aligned, so a negative distance stays in the mapping.
            let anchor = map.p.wrapping_add(i * SPACING + SPACING / 2);
            at.push(anchor);
            if let Some(d) = d {
                at.push(anchor.wrapping_offset(d));
            }
        }
        let start = link(&at);
        let loads = at.len();
        let t = time_chase(start, loads, loads, 2, &cycles);
        Timed { cycles: t.cycles * loads as f64 / anchors as f64, ns: t.ns * loads as f64 / anchors as f64 }
    };
    let base = run(None);
    println!(" none {:8.1} {:8.1}", base.cycles, base.ns);
    for d in [8isize, 56, 64, 72, 120, 128, 136, 192, 256, 384, 512, 1024, 2048, -8, -64, -128, -192, -256, -512, -1024] {
        let t = run(Some(d));
        println!("{d:5} {:8.1} {:8.1} {:8.1}", t.cycles, t.ns, t.cycles - base.cycles);
    }
}

fn regs() {
    let (ctr, dczid, midr): (u64, u64, u64);
    // SAFETY: readable at EL0 (the kernel traps and emulates where it must).
    unsafe {
        asm!("mrs {c}, ctr_el0", "mrs {d}, dczid_el0", c = out(reg) ctr, d = out(reg) dczid, options(nostack, nomem));
        asm!("mrs {m}, midr_el1", m = out(reg) midr, options(nostack, nomem));
    }
    let dline = 4usize << ((ctr >> 16) & 0xf);
    let iline = 4usize << (ctr & 0xf);
    let cwg = (ctr >> 24) & 0xf;
    let erg = (ctr >> 20) & 0xf;
    let zva = 4usize << (dczid & 0xf);
    println!(
        "ctr_el0 {ctr:#x}: DminLine {dline} bytes, IminLine {iline}, ERG field {erg} ({} bytes), CWG field {cwg} ({} bytes)",
        4usize << erg,
        4usize << cwg
    );
    println!("dczid_el0 {dczid:#x}: DC ZVA block {zva} bytes, prohibited {}", (dczid >> 4) & 1);
    println!("midr_el1 {midr:#x}");
    println!("cntfrq_el0 {} Hz", tick_rate());
    println!("page {} huge {}", page(), huge());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    match args.get(1).map(String::as_str) {
        Some("regs") => regs(),
        Some("granule") => granule(num(2, 8), num(3, 9), num(4, 30) as u64, num(5, 3)),
        Some("neighbour") => neighbour(num(2, 8), num(3, 2048)),
        _ => eprintln!("usage: lines regs|granule|neighbour"),
    }
}
