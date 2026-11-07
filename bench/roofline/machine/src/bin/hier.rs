//! The memory hierarchy from one CPU: latency, bandwidth, TLB reach, what the prefetchers follow,
//! and the caches' sets and ways.
//!
//! ```text
//! hier latency CPU small|huge [max_MiB]    random chase over 128-byte lines, by working set
//! hier tlb CPU                             one line a page, random order: small against huge pages
//! hier tlbsets CPU                         pages at a stride of 2^k pages, by how many: the sets
//! hier sets CPU                            lines at a stride, by how many: the caches' sets and ways
//! hier unit CPU                            what a cache indexes by: its line, for that purpose
//! hier prefetch CPU small|huge [MiB]       walks ascending, descending, strided against random
//! hier run CPU [MiB]                       random jump, then k lines ascending: how fast they engage
//! hier bandwidth CPU[,CPU...]              read, zero, copy by size, on each CPU listed at once
//! ```
//!
//! Every figure is the fastest of a few repetitions after a warm-up. Cycles come from the CPU's own
//! PMU; nanoseconds from the generic timer. A huge-page row prints how much huge pages backed.
use roofline_machine::*;
use std::sync::{Barrier, OnceLock};

static LINE: OnceLock<usize> = OnceLock::new();

/// Bytes between the lines a probe lays out.
fn ln() -> usize {
    *LINE.get().expect("set in main")
}

fn parse_pages(s: Option<&String>) -> Pages {
    match s.map(String::as_str) {
        Some("huge") => Pages::Huge,
        _ => Pages::Small,
    }
}

/// Addresses of `count` things `stride` bytes apart, in random order.
fn shuffled(map: &Map, stride: usize, count: usize, seed: u64) -> Vec<*mut u8> {
    let mut at: Vec<*mut u8> = (0..count).map(|i| map.p.wrapping_add(i * stride)).collect();
    Rng(seed).shuffle(&mut at);
    at
}

fn latency(cpu: usize, pages: Pages, max_mib: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    println!("# random chase, {}-byte lines, CPU {cpu}, {pages:?} pages", ln());
    println!("# size_KiB  cycles_a_load  ns_a_load  GHz  huge_MiB");
    let mut size = 16usize << 10;
    while size <= max_mib << 20 {
        let map = Map::resident(size.max(huge()), pages);
        let lines = size / ln();
        let at = shuffled(&map, ln(), lines, 0x1357_9bdf_0246_8ace);
        let start = link(&at);
        let loads = lines.clamp(1 << 18, 1 << 24);
        let t = time_chase(start, loads, lines.max(1 << 16), 3, &cycles);
        println!(
            "{:9} {:8.2} {:8.2} {:6.3} {:6}",
            size >> 10,
            t.cycles,
            t.ns,
            t.cycles / t.ns,
            huge_mib()
        );
        // Halves and doubles: enough points to see each step.
        size = if size.is_power_of_two() { size + size / 2 } else { size / 3 * 4 };
    }
}

/// One line in each of `count` pages, in random order, spread over the sets of the L1 so that the
/// caches do not limit it before the TLBs do.
fn page_walk(map: &Map, page: usize, stride_pages: usize, count: usize, seed: u64) -> *const usize {
    // Random colours: a linear rule has a short period, which in physically contiguous memory, as
    // inside a huge page, falls on a few sets of the L2.
    let mut colours = Rng(seed ^ 0xc010_0c01);
    let mut at: Vec<*mut u8> = (0..count)
        .map(|i| {
            let colour = (colours.next() as usize % (page / ln())) * ln();
            map.p.wrapping_add(i * stride_pages * page + colour)
        })
        .collect();
    Rng(seed).shuffle(&mut at);
    link(&at)
}

fn tlb(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let page = page();
    let counts = [
        16usize, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 320, 384, 512, 768, 1024, 1536, 2048, 2560,
        3072, 3328, 3584, 4096, 5120, 6144, 8192, 12288, 16384, 32768, 65536,
    ];
    println!("# one line a page, random order, CPU {cpu}, page {page}: cycles a load");
    println!("# pages  span_MiB  small  huge  extra  small_ns  huge_ns  extra_ns");
    let most = counts[counts.len() - 1] * page;
    let small = Map::resident(most, Pages::Small);
    let large = Map::resident(most, Pages::Huge);
    println!("# huge pages back {} MiB of {} MiB", huge_mib(), most >> 20);
    for &n in &counts {
        let loads = (n * 4).clamp(1 << 18, 1 << 22);
        let t_small = time_chase(page_walk(&small, page, 1, n, 0x5555_1234), loads, n * 2, 3, &cycles);
        let t_large = time_chase(page_walk(&large, page, 1, n, 0x5555_1234), loads, n * 2, 3, &cycles);
        println!(
            "{n:6} {:7.1} {:7.2} {:7.2} {:7.2} {:7.2} {:7.2} {:7.2}",
            (n * page) as f64 / (1 << 20) as f64,
            t_small.cycles,
            t_large.cycles,
            t_small.cycles - t_large.cycles,
            t_small.ns,
            t_large.ns,
            t_small.ns - t_large.ns
        );
    }
}

/// Pages at a stride of `2^k` pages: when `n` of them no longer fit a TLB, that stride indexes one
/// set of it, and `n` is past its ways.
fn tlbsets(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let page = page();
    let counts = [2usize, 4, 6, 8, 10, 12, 14, 16, 20, 24, 32, 48, 64, 96, 128, 192, 256];
    println!("# one line a page, random order, CPU {cpu}: ns a load, by stride in pages (rows) and pages (columns)");
    print!("# stride");
    for n in counts {
        print!(" {n:6}");
    }
    println!();
    for k in 0..=13u32 {
        let stride = 1usize << k;
        let span = counts[counts.len() - 1] * stride * page;
        // Sparse small pages: only the pages named below are resident.
        let map = Map::new(span + page, Pages::Small);
        print!("{stride:8}");
        for &n in &counts {
            let start = page_walk(&map, page, stride, n, 0x7777_1111 + n as u64);
            let t = time_chase(start, 1 << 18, n * 4, 3, &cycles);
            print!(" {:6.2}", t.ns);
        }
        println!();
    }
}

/// `n` lines at a stride, inside huge pages so that virtual and physical addresses agree below the
/// huge page: when `n` exceeds a cache's ways at that stride, the chase misses it.
fn sets(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let counts = [2usize, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 24, 28, 32, 40, 48, 64, 96, 128, 192, 256];
    let span = 1usize << 30;
    let map = Map::resident(span, Pages::Huge);
    println!("# lines at a stride, random order, CPU {cpu}, huge pages back {} MiB", huge_mib());
    print!("# stride");
    for n in counts {
        print!(" {n:6}");
    }
    println!();
    for k in ln().trailing_zeros()..=24 {
        let stride = 1usize << k;
        print!("{stride:8}");
        for &n in &counts {
            if n * stride > span {
                print!(" {:>6}", "-");
                continue;
            }
            let at = shuffled(&map, stride, n, 0x4242_4242 + n as u64);
            let t = time_chase(link(&at), 1 << 17, n * 8, 3, &cycles);
            print!(" {:6.1}", t.cycles);
        }
        println!();
    }
}

/// The unit a cache indexes by. `n` lines a way's span apart share a set, and past the ways they
/// miss. Moving every second one by `unit` splits them into two sets only if `unit` reaches the set
/// index, so the smallest unit at which `n` fits again is the cache's line for this purpose.
fn index_unit(cpu: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let map = Map::resident(1 << 30, Pages::Huge);
    println!("# twice the ways at a way's span, every second line moved by a unit, CPU {cpu}, huge pages back {} MiB", huge_mib());
    let units: Vec<usize> = std::iter::once(0).chain((3..=19).map(|k| 1usize << k)).collect();
    print!("# way_KiB  lines by unit");
    for u in &units {
        print!(" {u:6}");
    }
    println!();
    let probe = |span: usize, n: usize| {
        print!("{:8} {n:6}        ", span >> 10);
        for &unit in &units {
            if unit >= span {
                print!(" {:>6}", "-");
                continue;
            }
            let mut at: Vec<*mut u8> =
                (0..n).map(|i| map.p.wrapping_add(i * span + (i % 2) * unit)).collect();
            Rng(0x3141_5926 + n as u64).shuffle(&mut at);
            let t = time_chase(link(&at), 1 << 17, n * 8, 3, &cycles);
            print!(" {:6.1}", t.cycles);
        }
        println!();
    };
    // A little under twice the ways that `sets` finds, so that two sets hold them with room.
    for (span, n) in [(16usize << 10, 12usize), (16 << 10, 16), (1 << 20, 18), (1 << 20, 22), (2 << 20, 18)] {
        probe(span, n);
    }
}

/// Passes over the mapping, each at `stride` in one direction from a different line offset, so that
/// every line is new until the whole of it has been walked: the prefetchers' stream against a
/// random walk over the same lines.
fn walk(map: &Map, stride: isize, passes: usize) -> Vec<*mut u8> {
    let abs = stride.unsigned_abs();
    let per_pass = map.len / abs;
    let mut at = Vec::with_capacity(per_pass * passes);
    let mut offsets: Vec<usize> = (0..abs / ln()).map(|i| i * ln()).collect();
    Rng(0x0dd0_beef).shuffle(&mut offsets);
    for &off in offsets.iter().take(passes) {
        for i in 0..per_pass {
            let i = if stride > 0 { i } else { per_pass - 1 - i };
            at.push(map.p.wrapping_add(i * abs + off));
        }
    }
    at
}

fn prefetch(cpu: usize, pages: Pages, mib: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let len = mib << 20;
    let map = Map::resident(len, pages);
    println!("# walks over {mib} MiB, {pages:?} pages, huge pages back {} MiB, CPU {cpu}", huge_mib());
    println!("# stride  cycles_a_load  ns_a_load  share_of_random");
    let lines = len / ln();
    let mut all = shuffled(&map, ln(), lines, 0x2468_ace0);
    let random = {
        let start = link(&all);
        time_chase(start, 1 << 22, 1 << 22, 3, &cycles)
    };
    println!("random {:8.2} {:8.2} {:6.3}", random.cycles, random.ns, 1.0);
    all.clear();
    for stride in [
        64isize, -64, 128, -128, 192, -192, 256, -256, 320, -320, 384, -384, 512, -512, 640, -640, 768, -768, 1024, -1024, 1536,
        -1536, 2048, -2048, 3072, -3072, 4096, -4096, 8192, -8192, 16384, -16384,
    ] {
        // Enough passes for the walk to leave every cache behind at each stride.
        let abs = stride.unsigned_abs();
        if abs % ln() != 0 {
            continue;
        }
        let per_pass = len / abs;
        let passes = (abs / ln()).max(1).min((1usize << 22).div_ceil(per_pass));
        let at = walk(&map, stride, passes);
        let start = link(&at);
        let loads = at.len().min(1 << 22);
        let t = time_chase(start, loads, loads, 3, &cycles);
        println!("{stride:6} {:8.2} {:8.2} {:6.3}", t.cycles, t.ns, t.cycles / random.cycles);
    }
}

/// A random jump to a fresh place, then `k` lines ascending, or descending: how long a stream has to
/// run before the prefetchers help.
fn run_length(cpu: usize, mib: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let len = mib << 20;
    let map = Map::resident(len, Pages::Huge);
    println!("# a jump to a random 64 KiB block, then k lines; {mib} MiB, huge pages back {} MiB, CPU {cpu}", huge_mib());
    println!("# k  up_cycles_a_load  down_cycles_a_load  up_ns  down_ns");
    let block = 64usize << 10;
    let blocks = len / block;
    let mut order: Vec<usize> = (0..blocks).collect();
    Rng(0xabcd_ef01_2345).shuffle(&mut order);
    for k in [1usize, 2, 3, 4, 5, 6, 8, 12, 16, 24, 32, 64, 128, 256, 512] {
        let mut row = vec![];
        for up in [true, false] {
            let mut at = Vec::with_capacity(blocks * k);
            for &b in &order {
                for i in 0..k {
                    let i = if up { i } else { k - 1 - i };
                    at.push(map.p.wrapping_add(b * block + i * ln()));
                }
            }
            let start = link(&at);
            let loads = at.len().min(1 << 22);
            row.push(time_chase(start, loads, loads, 2, &cycles));
        }
        println!("{k:4} {:8.2} {:8.2} {:8.2} {:8.2}", row[0].cycles, row[1].cycles, row[0].ns, row[1].ns);
    }
}

/// Reads `size` bytes with the widest loads there are, four 16-byte registers at a time.
///
/// # Safety
///
/// `size` bytes at `p` are readable, and `size` is a multiple of 64.
#[inline(never)]
unsafe fn read_pass(p: *const u8, size: usize) {
    // SAFETY: the caller's range; only the clobbered registers change.
    unsafe {
        std::arch::asm!(
            "2:",
            "ldp q0, q1, [{p}]",
            "ldp q2, q3, [{p}, #32]",
            "add {p}, {p}, #64",
            "subs {n}, {n}, #64",
            "b.ne 2b",
            p = inout(reg) p => _,
            n = inout(reg) size => _,
            out("v0") _, out("v1") _, out("v2") _, out("v3") _,
            options(nostack, readonly),
        );
    }
}

/// GiB/s from a pass over `len` bytes by `f`, the fastest of a few, on each CPU at once.
fn bandwidth(cpus: &[usize]) {
    println!("# CPUs {cpus:?} at once; GiB/s for each and all together");
    println!("# size_KiB  read  zero  fill  copy  (per CPU, sum of medians)");
    let sizes = [32usize << 10, 64 << 10, 96 << 10, 256 << 10, 1 << 20, 4 << 20, 8 << 20, 32 << 20, 64 << 20, 256 << 20, 1 << 30];
    for &size in &sizes {
        let barrier = Barrier::new(cpus.len());
        let results: Vec<[f64; 4]> = std::thread::scope(|s| {
            let handles: Vec<_> = cpus
                .iter()
                .map(|&cpu| {
                    let barrier = &barrier;
                    s.spawn(move || {
                        pin(cpu);
                        let a = Map::resident(size.max(huge()), Pages::Huge);
                        let b = Map::resident(size.max(huge()), Pages::Huge);
                        let passes = ((1usize << 31) / size).clamp(1, 1 << 14);
                        let mut out = [0f64; 4];
                        for (slot, kind) in [0, 1, 2, 3].into_iter().enumerate() {
                            barrier.wait();
                            let t0 = ticks();
                            for _ in 0..passes {
                                // SAFETY: both buffers hold `size` bytes.
                                unsafe {
                                    match kind {
                                        0 => read_pass(a.p, size),
                                        1 => std::ptr::write_bytes(a.p, 0, size),
                                        2 => std::ptr::write_bytes(a.p, 0xa5, size),
                                        _ => std::ptr::copy_nonoverlapping(a.p, b.p, size),
                                    }
                                }
                                std::hint::black_box(a.p);
                            }
                            let ns = ns_of(ticks() - t0);
                            out[slot] = (size * passes) as f64 / ns * 1e9 / (1u64 << 30) as f64;
                        }
                        out
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let sum = |k: usize| results.iter().map(|r| r[k]).sum::<f64>();
        println!(
            "{:9} {:7.1} {:7.1} {:7.1} {:7.1}",
            size >> 10,
            sum(0),
            sum(1),
            sum(2),
            sum(3)
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let cpu = num(2, 8);
    LINE.set(line(cpu)).unwrap();
    match args.get(1).map(String::as_str) {
        Some("latency") => latency(cpu, parse_pages(args.get(3)), num(4, 4096)),
        Some("tlb") => tlb(cpu),
        Some("tlbsets") => tlbsets(cpu),
        Some("sets") => sets(cpu),
        Some("unit") => index_unit(cpu),
        Some("prefetch") => prefetch(cpu, parse_pages(args.get(3)), num(4, 2048)),
        Some("run") => run_length(cpu, num(3, 2048)),
        Some("bandwidth") => bandwidth(&parse_cpus(args.get(2).map_or("8", String::as_str))),
        _ => eprintln!("usage: hier latency|tlb|tlbsets|sets|prefetch|run|bandwidth CPU ..."),
    }
}
