//! What the heap's rules do on the kernel this runs on, through the real allocator: where the
//! retained cap and the floor act, whether huge pages are used, who splits them, and what the
//! stagger does for buffers written in turn. `docs/research/large-blocks.md` and
//! `docs/research/memory.md` hold what these found.
//!
//! ```text
//! rules cap                free a class, grow another: retained memory by bytes grown
//! rules floor              free a class, go quiet: retained memory and RSS over `maintain`'s rounds
//! rules huge [class]       a class of `class` bytes past its huge-page threshold, by bytes carved
//! rules split single|bg    a fragmented huge-page part, under heartbeats, then `collect`
//! rules stagger CPU [n]    `n` large buffers written in turn: ns an element, by request size
//! ```
//!
//! For `split`, run under `strace -f -e trace=madvise,write` to see which thread calls
//! `madvise(.., MADV_COLD)` and on which side of the marks this prints.

use cralloc::Cralloc;
use std::alloc::{GlobalAlloc, Layout};
use std::time::{Duration, Instant};

#[global_allocator]
static HEAP: Cralloc = Cralloc;

const KIB: usize = 1 << 10;
const MIB: usize = 1 << 20;

unsafe extern "C" {
    fn getpagesize() -> i32;
    fn sched_setaffinity(pid: i32, size: usize, mask: *const u64) -> i32;
}

fn page() -> usize {
    // SAFETY: no arguments and no preconditions.
    unsafe { getpagesize() as usize }
}

fn pin(cpu: usize) {
    let mut mask = [0u64; 16];
    mask[cpu / 64] = 1 << (cpu % 64);
    // SAFETY: the mask lives across the call, and its size is passed.
    let done = unsafe { sched_setaffinity(0, size_of_val(&mask), mask.as_ptr()) };
    assert_eq!(done, 0, "pinning to CPU {cpu}");
}

/// Bytes resident, by the kernel's count.
fn rss() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    statm.split_whitespace().nth(1).unwrap().parse::<usize>().unwrap() * page()
}

/// Bytes of anonymous memory mapped by huge pages.
fn anon_huge() -> usize {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    smaps
        .lines()
        .filter_map(|line| line.strip_prefix("AnonHugePages:"))
        .map(|rest| rest.trim().trim_end_matches("kB").trim().parse::<usize>().unwrap() * KIB)
        .sum()
}

#[derive(Default)]
struct Seen {
    retained: usize,
    /// For each class region: its class size, where it is carved to and its huge-page part.
    regions: Vec<(usize, usize, Option<usize>)>,
}

impl cralloc::stats::Visitor for Seen {
    fn retained(&mut self, bytes: usize) {
        self.retained = bytes;
    }
    fn region(&mut self, region: &cralloc::stats::Region) {
        self.regions.push((region.class.size(), region.carved, region.huge_from));
    }
}

fn seen() -> Seen {
    let mut seen = Seen::default();
    Cralloc::inspect(&mut seen);
    seen
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / MIB as f64
}

/// `total` bytes of blocks of `size`, each written at every 256 bytes.
fn blocks(size: usize, total: usize) -> Vec<Vec<u8>> {
    let mut all = Vec::with_capacity(total / size + 1);
    for _ in 0..total / size {
        let mut block = vec![0u8; size];
        for byte in block.iter_mut().step_by(256) {
            *byte = 1;
        }
        all.push(block);
    }
    all
}

/// Free 512 MiB of one small class, then grow another. Over the cap, each byte carved is paid for
/// by a byte returned, so retained memory falls with growth until it reaches the cap. Fast enough
/// that the clocked pass, whose frontiers keep what the last two intervals reached, has no part.
fn cap() {
    println!("# cap: page {} KiB; one thread, 512 MiB of 16 KiB blocks freed, then 4 KiB blocks", page() / KIB);
    let start = Instant::now();
    let freed = blocks(16 * KIB, 512 * MIB);
    drop(freed);
    let at = seen().retained;
    println!("# freed in {:?}: retained {:.1} MiB, RSS {:.1} MiB", start.elapsed(), mib(at), mib(rss()));
    println!("# grown_MiB retained_MiB rss_MiB ms");
    let mut live = Vec::new();
    for step in 1..=16 {
        live.push(blocks(4 * KIB, 32 * MIB));
        let s = seen();
        println!(
            "{:9} {:12.1} {:7.1} {:6}",
            step * 32,
            mib(s.retained),
            mib(rss()),
            start.elapsed().as_millis()
        );
    }
    std::hint::black_box(&live);
}

/// Free 256 MiB of one small class and go quiet: the clocked pass, driven by `maintain`, takes
/// retained memory down to the floor and stops there.
fn floor() {
    println!("# floor: page {} KiB; one thread, 256 MiB of 16 KiB blocks freed, then rounds of maintain", page() / KIB);
    let freed = blocks(16 * KIB, 256 * MIB);
    drop(freed);
    let start = Instant::now();
    println!("# seconds retained_MiB rss_MiB");
    for tick in 0..60 {
        Cralloc::maintain();
        if tick % 5 == 0 {
            println!("{:7.1} {:12.1} {:7.1}", start.elapsed().as_secs_f64(), mib(seen().retained), mib(rss()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("{:7.1} {:12.1} {:7.1}", start.elapsed().as_secs_f64(), mib(seen().retained), mib(rss()));
}

/// A class carved past its huge-page threshold: where the part starts, and what the kernel maps.
fn huge(class: usize) {
    println!("# huge: page {} KiB; blocks of {class} bytes, written at every 256 bytes", page() / KIB);
    println!("# carved_MiB huge_from_MiB anon_huge_MiB rss_MiB");
    let mut live = Vec::new();
    for step in 1..=12 {
        live.push(blocks(class, 64 * MIB));
        let s = seen();
        let (carved, from) = s
            .regions
            .iter()
            .filter(|r| r.0 == class)
            .map(|r| (r.1, r.2))
            .max()
            .unwrap_or((0, None));
        println!(
            "{:10} {:>13} {:13.1} {:7.1}",
            mib(carved) as usize,
            from.map_or("none".into(), |f| format!("{}", f / MIB)),
            mib(anon_huge()),
            mib(rss())
        );
        let _ = step;
    }
    std::hint::black_box(&live);
}

/// One line on standard error, as a mark in a trace of system calls.
fn mark(what: &str) {
    eprintln!("MARK {what} rss {:.1} MiB retained {:.1} MiB", mib(rss()), mib(seen().retained));
}

/// Allocation and free in one small class with magazines' worth of swings, which keeps slow paths
/// and their heartbeats running without reaching the class under test.
fn requests(until: Instant) -> u64 {
    let mut count = 0;
    let mut held = Vec::with_capacity(4096);
    while Instant::now() < until {
        for _ in 0..4096 {
            held.push(vec![0u8; 8 * KIB]);
        }
        held.clear();
        count += 1;
    }
    count
}

/// A huge-page part left a third full: a live block in every three pages' worth above the frontier.
/// Heartbeats may not split a huge page larger than the rule's limit; the background thread and
/// `collect` may. `bg` starts a second thread, so that the background thread comes.
fn split(bg: bool) {
    println!("# split: page {} KiB; 1 KiB blocks, {}", page() / KIB, if bg { "with the background thread" } else { "one thread" });
    let mut all = blocks(KIB, 640 * MIB);
    mark("carved");
    println!("# anon huge after carving: {:.1} MiB", mib(anon_huge()));
    let stride = 3 * page() / KIB;
    let mut kept = Vec::with_capacity(all.len() / stride + 1);
    for (i, block) in all.drain(..).enumerate() {
        if i % stride == 0 {
            kept.push(block);
        }
    }
    all.shrink_to_fit();
    if bg {
        std::thread::spawn(|| std::thread::sleep(Duration::from_millis(1))).join().unwrap();
    }
    mark("freed");
    let start = Instant::now();
    let beats = requests(start + Duration::from_secs(10));
    mark("after requests");
    println!(
        "# {beats} rounds of requests: retained {:.1} MiB, RSS {:.1} MiB, anon huge {:.1} MiB",
        mib(seen().retained),
        mib(rss()),
        mib(anon_huge())
    );
    let returned = Cralloc::collect();
    mark("after collect");
    println!(
        "# collect returned {:.1} MiB: retained {:.1} MiB, RSS {:.1} MiB, anon huge {:.1} MiB",
        mib(returned),
        mib(seen().retained),
        mib(rss()),
        mib(anon_huge())
    );
    std::hint::black_box(&kept);
}

/// Writes `words` words to each buffer in turn, as vectors filled together are: the best of three
/// passes, in nanoseconds an element.
fn in_turn(starts: &[*mut u64], words: usize) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        for w in 0..words {
            for &s in starts {
                // SAFETY: each buffer holds at least `words` words, by the caller.
                unsafe { s.add(w).write_volatile(w as u64) };
            }
        }
        best = best.min(start.elapsed().as_nanos() as f64 / (words * starts.len()) as f64);
    }
    best
}

/// `count` large buffers of each size written in turn, at most 1 MiB of each.
fn stagger(cpu: usize, count: usize) {
    pin(cpu);
    println!("# stagger: page {} KiB, CPU {cpu}, {count} buffers written in turn", page() / KIB);
    println!("# request_KiB pages_apart lines_apart ns_an_element");
    let sizes = [
        48 * KIB, 64 * KIB, 96 * KIB, 128 * KIB, 192 * KIB, 256 * KIB, 384 * KIB, 512 * KIB, MIB,
        2 * MIB, 3 * MIB, 4 * MIB, 8 * MIB, 16 * MIB, 32 * MIB,
    ];
    for &size in &sizes {
        let layout = Layout::from_size_align(size, 8).unwrap();
        // SAFETY: the layout's size is not zero.
        let starts: Vec<*mut u64> =
            (0..count).map(|_| unsafe { HEAP.alloc(layout) }.cast()).collect();
        assert!(starts.iter().all(|s| !s.is_null()));
        let mut lines: Vec<usize> = starts.iter().map(|s| s.addr() % page() / 64).collect();
        lines.sort_unstable();
        lines.dedup();
        let mut pages: Vec<usize> = starts.iter().map(|s| s.addr() / page() % 64).collect();
        pages.sort_unstable();
        pages.dedup();
        let words = size.min(MIB) / 8;
        let ns = in_turn(&starts, words);
        println!("{:11} {:11} {:11} {ns:13.2}", size / KIB, pages.len(), lines.len());
        for &s in &starts {
            // SAFETY: allocated above with this layout.
            unsafe { HEAP.dealloc(s.cast(), layout) };
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(1) {
        Some("cap") => cap(),
        Some("floor") => floor(),
        Some("huge") => huge(arg(2).map_or(KIB, |c| c.parse().unwrap())),
        Some("split") => split(arg(2) == Some("bg")),
        Some("stagger") => stagger(
            arg(2).map_or(8, |c| c.parse().unwrap()),
            arg(3).map_or(64, |c| c.parse().unwrap()),
        ),
        _ => {
            eprintln!("usage: rules cap | floor | huge [class] | split single|bg | stagger CPU [n]");
            std::process::exit(2);
        }
    }
}
