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
//! rules fault CPU [n]      the first touch of `n` huge pages under advice, with the kernel's own
//!                          count of what it did: what a huge-page rule charges a request when
//!                          free blocks of that size are short or plentiful
//! rules request CPU [MiB]  each allocation and first write of 1 KiB blocks, carved past the class's
//!                          huge-page threshold: how long the worst requests take, as the program
//!                          sees it
//! rules step CPU SLOT MiB clock|grow|rush|large|maintain single|bg [small]
//!                          a class of SLOT bytes carved and freed, then requests in another class
//!                          while steps of returning memory reach its wholly free huge pages: how
//!                          long the worst requests take. `single` wants CRALLOC_BACKGROUND=0 in
//!                          the environment; `bg` starts the thread before pinning to CPU, so run
//!                          it under `taskset` with other CPUs to spare; `small` forbids the process
//!                          huge pages, so that the same units are mapped by small pages
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
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: isize) -> *mut u8;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn clock_gettime(clock: i32, time: *mut [i64; 2]) -> i32;
    fn prctl(option: i32, a: u64, b: u64, c: u64, d: u64) -> i32;
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

/// One line on standard error, as a mark in a trace of system calls, in one write.
fn mark(what: &str) {
    let line = format!("MARK {what} rss {:.1} MiB retained {:.1} MiB\n", mib(rss()), mib(seen().retained));
    eprint!("{line}");
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

/// The kernel's counters that say what a huge-page fault did.
fn vmstat() -> Vec<(String, u64)> {
    let want = ["compact_stall", "compact_fail", "compact_success", "thp_fault_alloc", "thp_fault_fallback"];
    let text = std::fs::read_to_string("/proc/vmstat").unwrap();
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(name, _)| want.contains(name))
        .map(|(name, value)| (name.to_string(), value.parse().unwrap()))
        .collect()
}

/// Free blocks of the kernel's largest orders, for what a huge page needs.
fn free_blocks() -> String {
    let text = std::fs::read_to_string("/proc/buddyinfo").unwrap();
    text.lines().next().unwrap().split_whitespace().skip(4).collect::<Vec<_>>().join(" ")
}

/// The first touch of `count` huge pages under `MADV_HUGEPAGE` in a mapping of its own, a byte each.
/// The huge page's size is `page^2 / 8`.
fn fault(cpu: usize, count: usize) {
    pin(cpu);
    let huge = page() * page() / 8;
    println!("# fault: {count} huge pages of {} MiB on CPU {cpu}", huge / MIB);
    println!("# free blocks by order: {}", free_blocks());
    let before = vmstat();
    let len = (count + 1) * huge;
    const PROT_RW: i32 = 3;
    const PRIVATE_ANON: i32 = 0x22;
    const MADV_HUGEPAGE: i32 = 14;
    // SAFETY: a new private mapping of `len` bytes, which nothing else knows of.
    let base = unsafe { mmap(std::ptr::null_mut(), len, PROT_RW, PRIVATE_ANON, -1, 0) };
    assert!(base as isize != -1, "mmap");
    let aligned = base.wrapping_add(base.addr().next_multiple_of(huge) - base.addr());
    // SAFETY: whole huge pages inside the mapping.
    assert_eq!(unsafe { madvise(aligned, count * huge, MADV_HUGEPAGE) }, 0);
    let mut ns = Vec::with_capacity(count);
    for i in 0..count {
        let at = aligned.wrapping_add(i * huge);
        let start = Instant::now();
        // SAFETY: inside the mapping, which is writable and this thread's alone.
        unsafe { at.write_volatile(1) };
        ns.push(start.elapsed().as_nanos() as u64);
    }
    let mapped = anon_huge();
    let after = vmstat();
    // SAFETY: the mapping made above, not used again.
    unsafe { munmap(base, len) };
    ns.sort_unstable();
    let q = |p: usize| ns[(ns.len() - 1) * p / 100] as f64 / 1e3;
    println!(
        "# first touch, us: min {:.0} median {:.0} p90 {:.0} p99 {:.0} max {:.0}",
        q(0),
        q(50),
        q(90),
        q(99),
        q(100)
    );
    println!("# huge pages mapped: {} of {count}", mapped / huge);
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        println!("# {name}: +{}", b - a);
    }
    println!("# free blocks by order after: {}", free_blocks());
}

/// Allocates and writes `total` bytes of 1 KiB blocks one at a time, timing each. The huge pages
/// past the threshold fault on whichever request first writes them.
fn request(cpu: usize, total: usize) {
    pin(cpu);
    println!("# request: {} MiB of 1 KiB blocks on CPU {cpu}", total / MIB);
    println!("# free blocks by order: {}", free_blocks());
    let before = vmstat();
    let count = total / KIB;
    let mut live = Vec::with_capacity(count);
    let mut slow = Vec::new();
    let mut spent = 0u64;
    for i in 0..count {
        let start = Instant::now();
        let mut block = Vec::<u8>::with_capacity(KIB);
        // SAFETY: inside the block's capacity.
        unsafe { block.as_mut_ptr().write_volatile(1) };
        let ns = start.elapsed().as_nanos() as u64;
        spent += ns;
        if ns >= 20_000 {
            slow.push((i * KIB / MIB, ns));
        }
        live.push(block);
    }
    let after = vmstat();
    println!("# {} ns a request over all", spent / count as u64);
    println!("# requests over 20 us: {} ; carved_MiB us", slow.len());
    for (at, ns) in &slow {
        println!("{at:6} {:9.0}", *ns as f64 / 1e3);
    }
    println!("# anon huge {:.1} MiB", mib(anon_huge()));
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        println!("# {name}: +{}", b - a);
    }
    println!("# free blocks by order after: {}", free_blocks());
    std::hint::black_box(&live);
}

/// Bytes resident, by the kernel's count, read without calling the heap: a look between timed
/// requests must not take a step of the heap's maintenance that no timer sees.
fn rss_quiet() -> usize {
    use std::io::Read;
    let mut buf = [0u8; 128];
    let read = std::fs::File::open("/proc/self/statm").unwrap().read(&mut buf).unwrap();
    let text = std::str::from_utf8(&buf[..read]).unwrap();
    text.split_ascii_whitespace().nth(1).unwrap().parse::<usize>().unwrap() * page()
}

/// Nanoseconds this thread has spent on a CPU.
fn on_cpu() -> u64 {
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut time = [0i64; 2];
    // SAFETY: `time` is a valid timespec for the call to fill.
    unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time[0] as u64 * 1_000_000_000 + time[1] as u64
}

/// How long requests took: a count for each power of two of nanoseconds, and each one over 5 us
/// with when it ran, how far resident memory fell since the last such request, and how long the
/// thread was off its CPU since then, which says that a request was preempted and not working.
struct Times {
    start: Instant,
    by_log2: [u64; 48],
    over: Vec<(u64, u64, isize, u64)>,
    rss: usize,
    looked: (Instant, u64),
}

impl Times {
    fn new() -> Times {
        Times {
            start: Instant::now(),
            by_log2: [0; 48],
            over: Vec::with_capacity(1 << 18),
            rss: rss_quiet(),
            looked: (Instant::now(), on_cpu()),
        }
    }

    /// Starts counting from here. Separate from `new`, whose list is large enough to be growth,
    /// and growth over the cap pays: it goes before the class is freed.
    fn begin(&mut self) {
        self.start = Instant::now();
        self.rss = rss_quiet();
        self.looked = (Instant::now(), on_cpu());
    }

    #[inline]
    fn time<T>(&mut self, request: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = request();
        let ns = start.elapsed().as_nanos() as u64;
        self.by_log2[(64 - ns.leading_zeros()) as usize] += 1;
        if ns >= 5_000 && self.over.len() < self.over.capacity() {
            // Off the CPU since the last look, a millisecond ago at most, up to the request's end.
            let cpu = on_cpu();
            let wall = (start + Duration::from_nanos(ns) - self.looked.0).as_nanos() as u64;
            let off = wall.saturating_sub(cpu - self.looked.1);
            let rss = rss_quiet();
            let at = self.start.elapsed().as_millis() as u64;
            self.over.push((ns, at, self.rss as isize - rss as isize, off));
            self.rss = rss;
            self.looked = (Instant::now(), on_cpu());
        } else if (start - self.looked.0).as_millis() >= 1 {
            self.looked = (Instant::now(), on_cpu());
        }
        out
    }

    fn report(mut self, huge: usize) {
        let requests: u64 = self.by_log2.iter().sum();
        let over = |us: u64| self.over.iter().filter(|o| o.0 >= us * 1000).count();
        let left = self.over.iter().filter(|o| o.2 >= huge as isize / 2).count();
        println!(
            "# {requests} requests in {:?}: over 20 us {}, 100 us {}, 1 ms {}; {left} saw a huge page leave",
            self.start.elapsed(),
            over(20),
            over(100),
            over(1000)
        );
        for (log2, count) in self.by_log2.iter().enumerate().skip(11) {
            if *count != 0 {
                println!("# under {:9.1} us: {count}", (1u64 << log2) as f64 / 1e3);
            }
        }
        let line = |(ns, at, fell, off): &(u64, u64, isize, u64)| {
            println!(
                "{:9.1} {at:7} {:9.1} {:9.1}",
                *ns as f64 / 1e3,
                *fell as f64 / MIB as f64,
                *off as f64 / 1e3
            );
        };
        println!("# those that saw a huge page leave: us at_ms resident_fell_MiB off_cpu_us");
        self.over.iter().filter(|o| o.2 >= huge as isize / 2).for_each(line);
        self.over.sort_unstable();
        println!("# slowest:");
        self.over.iter().rev().take(16).for_each(line);
        self.over.sort_unstable_by_key(|o| o.0.saturating_sub(o.3));
        println!("# slowest on the CPU:");
        self.over.iter().rev().take(16).for_each(line);
    }
}

/// `total` bytes of `slot`-byte blocks, written once and freed last to first, so that what stays
/// parked in caches is the region's lowest; then requests in a class of their own.
///
/// `clock`: blocks of 8 KiB allocated and freed in swings past their caches, for 16 s, which keeps
/// heartbeats running through the intervals the frontier makes the pass wait. `grow`: the same
/// blocks kept and written, 256 MiB of them, which pays for growth over the cap at once. `rush`:
/// `grow` unwritten, so that growth outruns a thread paying for it. `large`: blocks of 64 MiB
/// unwritten, each of which owes more than a thread's limit in one carve. `maintain`: no requests,
/// but `Cralloc::maintain_step` for 16 s, as a program's timer calls it, each step timed.
fn step(cpu: usize, slot: usize, total: usize, mode: &str, thread: &str, small: bool) {
    if small {
        const PR_SET_THP_DISABLE: i32 = 41;
        // SAFETY: the call takes these four words and only sets a flag on this process.
        assert_eq!(unsafe { prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0) }, 0);
    }
    if thread == "bg" {
        Cralloc::start_background().expect("a thread for the allocator");
    }
    pin(cpu);
    let huge = page() * page() / 8;
    println!("# step: CPU {cpu}, {slot}-byte slots, {} MiB, {mode}, {thread}", total / MIB);
    println!("# free blocks by order: {}", free_blocks());
    let mut live = Vec::with_capacity(1 << 16);
    let mut times = Times::new();
    let layout = Layout::from_size_align(slot, slot.min(8)).unwrap();
    let count = total / slot;
    // The pointers lie outside the heap: on the heap they would be a class as large as this one.
    let len = (count + 1) * size_of::<*mut u8>();
    const PROT_RW: i32 = 3;
    const PRIVATE_ANON: i32 = 0x22;
    // SAFETY: a new private mapping of `len` bytes, which nothing else knows of.
    let list = unsafe { mmap(std::ptr::null_mut(), len, PROT_RW, PRIVATE_ANON, -1, 0) };
    assert!(list as isize != -1, "mmap");
    let list = list.cast::<*mut u8>();
    let started = Instant::now();
    for i in 0..count {
        // SAFETY: the layout's size is not zero, and `i` is inside the mapping.
        unsafe {
            let block = HEAP.alloc(layout);
            assert!(!block.is_null());
            block.write_volatile(1);
            list.add(i).write(block);
        }
    }
    let carved = seen().regions.iter().find(|r| r.0 == slot).map_or((0, None), |r| (r.1, r.2));
    println!(
        "# carved in {:?} to {:.1} MiB, {:.1} MiB into its huge page; huge from {:?} MiB; anon huge {:.1} MiB, RSS {:.1} MiB",
        started.elapsed(),
        mib(carved.0),
        mib(carved.0 % huge),
        carved.1.map(|f| f / MIB),
        mib(anon_huge()),
        mib(rss())
    );
    let started = Instant::now();
    for i in (0..count).rev() {
        // SAFETY: allocated above with this layout, and `i` is inside the mapping.
        unsafe { HEAP.dealloc(list.add(i).read(), layout) };
    }
    // SAFETY: the mapping made above, not used again.
    unsafe { munmap(list.cast(), len) };
    println!(
        "# freed in {:?}: retained {:.1} MiB, RSS {:.1} MiB; threads {}",
        started.elapsed(),
        mib(seen().retained),
        mib(rss()),
        std::fs::read_dir("/proc/self/task").unwrap().count()
    );

    let block = Layout::from_size_align(8 * KIB, 8).unwrap();
    times.begin();
    match mode {
        "clock" => {
            let mut held = [std::ptr::null_mut(); 256];
            while times.start.elapsed() < Duration::from_secs(16) {
                for p in &mut held {
                    // SAFETY: the layout's size is not zero.
                    *p = times.time(|| unsafe { HEAP.alloc(block) });
                }
                for &p in held.iter().rev() {
                    // SAFETY: allocated just above with this layout.
                    times.time(|| unsafe { HEAP.dealloc(p, block) });
                }
            }
        }
        "grow" | "rush" => {
            for _ in 0..256 * MIB / block.size() {
                // SAFETY: the layout's size is not zero.
                let p = times.time(|| unsafe { HEAP.alloc(block) });
                if mode == "grow" {
                    // SAFETY: the block is this thread's, and at least a byte.
                    unsafe { p.write_volatile(1) };
                }
                live.push(p);
            }
        }
        "large" => {
            let large = Layout::from_size_align(64 * MIB, 8).unwrap();
            for _ in 0..64 {
                // SAFETY: the layout's size is not zero.
                live.push(times.time(|| unsafe { HEAP.alloc(large) }));
            }
        }
        "maintain" => {
            while times.start.elapsed() < Duration::from_secs(16) {
                while times.time(Cralloc::maintain_step) {}
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        _ => panic!("no mode {mode}"),
    }
    times.report(huge);
    println!(
        "# at the end: retained {:.1} MiB, RSS {:.1} MiB, anon huge {:.1} MiB",
        mib(seen().retained),
        mib(rss()),
        mib(anon_huge())
    );
    std::hint::black_box(&live);
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
        Some("fault") => fault(
            arg(2).map_or(8, |c| c.parse().unwrap()),
            arg(3).map_or(64, |c| c.parse().unwrap()),
        ),
        Some("request") => request(
            arg(2).map_or(8, |c| c.parse().unwrap()),
            arg(3).map_or(768, |c| c.parse().unwrap()) * MIB,
        ),
        Some("step") => step(
            arg(2).unwrap().parse().unwrap(),
            arg(3).unwrap().parse().unwrap(),
            arg(4).unwrap().parse::<usize>().unwrap() * MIB,
            arg(5).unwrap(),
            arg(6).unwrap(),
            arg(7) == Some("small"),
        ),
        _ => {
            eprintln!(
                "usage: rules cap | floor | huge [class] | split single|bg | stagger CPU [n] | fault CPU [n] | request CPU [MiB] | step CPU SLOT MiB clock|grow|rush|large|maintain single|bg [small]"
            );
            std::process::exit(2);
        }
    }
}
