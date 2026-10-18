//! Memory shapes, for docs/research/memory.md: what an allocator keeps resident after a
//! peak, in idle threads' caches, and behind sparsely touched large blocks. They are not
//! ranked and print no `BENCH_RESULT`; each prints `MEM` lines with the process's anonymous
//! resident memory, page-table memory and huge-page-backed memory, read from `/proc/self`.
//!
//! - `quiet`: allocate `live` objects (`sizes`), free all but `keep` percent in random order
//!   (`order=fifo` for allocation order), then for `secs` seconds run light work and report
//!   every 250 ms. `after=burst` allocates `burst` objects and frees them, once a millisecond,
//!   which crosses cache boundaries if the burst is larger than a cache (`after_sizes` picks
//!   the sizes of this phase; with twelve sizes a burst of 2000 stays inside asmalloc's
//!   magazines and never reaches a slow path); `after=pairs` allocates and frees one object at a time,
//!   which never does; `after=none` sleeps.
//! - `touch`: `threads` pinned threads each allocate, write and free objects of every size
//!   class up to 32 KiB (up to `bytes` per class; `classes=N` uses only N of the classes). Then all but the first block, and the
//!   first runs `quiet`'s burst work on one size for `secs` seconds.
//! - `sparse`: allocate `n` blocks of `size` bytes and write the first `touch` bytes of each.
//! - `tiny`: a process that allocates almost nothing.
use bench_harness::{Args, Cpus, Rng, SizeMix};
use std::alloc::{Layout, alloc, dealloc};
use std::sync::Barrier;
use std::time::{Duration, Instant};

fn field(text: &str, key: &str) -> u64 {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

fn report(phase: &str, start: Instant) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    println!(
        "MEM alloc={} phase={phase} t_ms={} rss_anon={} pte={} thp={}",
        bench_alloc::NAME,
        start.elapsed().as_millis(),
        field(&status, "RssAnon:"),
        field(&status, "VmPTE:"),
        field(&rollup, "AnonHugePages:")
    );
}

fn make(size: usize) -> (*mut u8, usize) {
    // SAFETY: the size is not zero and 8 is a power of two; the block is written only within
    // its size.
    unsafe {
        let p = alloc(Layout::from_size_align_unchecked(size, 8));
        assert!(!p.is_null());
        p.write_bytes(1, size);
        (p, size)
    }
}

fn drop_one((p, size): (*mut u8, usize)) {
    // SAFETY: `make` allocated the block with this layout.
    unsafe { dealloc(p, Layout::from_size_align_unchecked(size, 8)) }
}

fn size_mix(args: &Args) -> SizeMix {
    let s = args.str("sizes", "small");
    SizeMix::parse(&s).unwrap_or_else(|| panic!("sizes={s} is not a size mix"))
}

/// Light work for `secs` seconds, reporting every 250 ms.
fn after(args: &Args, mix: SizeMix, rng: &mut Rng, start: Instant) {
    let (secs, burst) = (args.num("secs", 5), args.usize("burst", 2000));
    let mode = args.str("after", "burst");
    let begin = Instant::now();
    let mut next = Duration::from_millis(250);
    let mut held = Vec::with_capacity(burst);
    while begin.elapsed() < Duration::from_secs(secs) {
        match mode.as_str() {
            "burst" => {
                held.extend((0..burst).map(|_| make(mix.sample(rng))));
                held.drain(..).for_each(drop_one);
            }
            "pairs" => (0..burst).for_each(|_| drop_one(make(mix.sample(rng)))),
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(1));
        if begin.elapsed() >= next {
            report("after", start);
            next += Duration::from_millis(250);
        }
    }
}

pub fn quiet(args: &Args) {
    Cpus::from_env().pin(0);
    let start = Instant::now();
    let (live, keep, mix) = (args.usize("live", 1_000_000), args.num("keep", 0), size_mix(args));
    let mut rng = Rng::new(7);
    report("start", start);
    let mut objects: Vec<_> = (0..live).map(|_| make(mix.sample(&mut rng))).collect();
    report("peak", start);
    if args.str("order", "random") == "random" {
        for i in (1..objects.len()).rev() {
            objects.swap(i, rng.below(i as u64 + 1) as usize);
        }
    }
    let kept = objects.split_off(objects.len() - objects.len() * keep as usize / 100);
    objects.into_iter().for_each(drop_one);
    report("freed", start);
    let later = args.str("after_sizes", &mix.name());
    let later = SizeMix::parse(&later).unwrap_or_else(|| panic!("after_sizes={later} is not a size mix"));
    after(args, later, &mut rng, start);
    kept.into_iter().for_each(drop_one);
    report("end", start);
}

/// Every size class up to 32 KiB: multiples of 8 to 64, then four per doubling.
fn class_sizes() -> Vec<usize> {
    let mut sizes: Vec<usize> = (1..=8).map(|w| w * 8).collect();
    let mut base = 64;
    while base < 32 << 10 {
        sizes.extend((1..=4).map(|q| base + q * base / 4));
        base *= 2;
    }
    sizes
}

pub fn touch(args: &Args) {
    let cpus = Cpus::from_env();
    let (threads, bytes, count) = (args.usize("threads", 4), args.usize("bytes", 256 << 10), args.usize("count", 300));
    let classes = args.usize("classes", usize::MAX);
    let start = Instant::now();
    let (touched, done) = (Barrier::new(threads + 1), Barrier::new(threads));
    report("start", start);
    std::thread::scope(|s| {
        for i in 0..threads {
            let (cpus, touched, done) = (&cpus, &touched, &done);
            s.spawn(move || {
                cpus.pin(i);
                let sizes = class_sizes();
                // `classes=N` keeps N of them, spread evenly over the range.
                let step = (sizes.len() / classes.clamp(1, sizes.len())).max(1);
                for size in sizes.into_iter().step_by(step) {
                    let n = (bytes / size).clamp(2, count);
                    let held: Vec<_> = (0..n).map(|_| make(size)).collect();
                    held.into_iter().for_each(drop_one);
                }
                touched.wait();
                if i == 0 {
                    after(args, size_mix(args), &mut Rng::new(1), start);
                }
                done.wait();
            });
        }
        touched.wait();
        report("touched", start);
    });
    report("end", start);
}

pub fn sparse(args: &Args) {
    Cpus::from_env().pin(0);
    let start = Instant::now();
    let (n, size, touch) = (args.usize("n", 256), args.usize("size", 1 << 20), args.usize("touch", 4096));
    report("start", start);
    let layout = Layout::from_size_align(size, 8).unwrap();
    let blocks: Vec<*mut u8> = (0..n)
        .map(|_| {
            // SAFETY: the layout is not empty, and the write stays inside the block.
            unsafe {
                let p = alloc(layout);
                assert!(!p.is_null());
                p.write_bytes(1, touch.min(size));
                p
            }
        })
        .collect();
    report("sparse", start);
    // SAFETY: each block was allocated with this layout.
    blocks.into_iter().for_each(|p| unsafe { dealloc(p, layout) });
    report("end", start);
}

pub fn tiny(_: &Args) {
    let start = Instant::now();
    let held: Vec<_> = [16, 64, 256].into_iter().map(make).collect();
    report("tiny", start);
    held.into_iter().for_each(drop_one);
}
