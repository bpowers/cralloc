//! What it costs to make fresh anonymous memory resident, and what huge-page advice costs
//! around a block: the measurements behind populate-ahead and huge pages for grown blocks
//! (`docs/research/zen5-roofline.md`, section 7).
//!
//! ```text
//! faults single  [MiB] [rounds]      demand faults, MADV_POPULATE_WRITE stretches, huge pages
//! faults threads [MiB] [rounds]      the same with 1, 2 and 4 threads at once
//! faults advice  [cycles]            MADV_HUGEPAGE/NOHUGEPAGE around a slot: cost, VMA count
//! faults split                       DONTNEED inside huge pages: what the cgroup gets back
//! faults khuge   [seconds]           does khugepaged collapse sparse advised memory
//! ```
//!
//! Every variant writes one byte to every page of a fresh mapping, so all of them end with
//! the same memory resident and written; they differ only in how it got there. Variants run
//! interleaved, in a new order each round, and each figure is the median over rounds. Cycles
//! (user and kernel) and page faults come from per-thread counters. `RF_CPUS` lists the CPUs
//! to pin threads to (default 13); the first is the single-thread CPU.
use std::sync::{Arc, Barrier};
use std::time::Instant;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
    fn syscall(n: i64, ...) -> i64;
    fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
    fn ioctl(fd: i32, req: u64, arg: u64) -> i32;
    fn close(fd: i32) -> i32;
    fn __errno_location() -> *mut i32;
}

const RW: i32 = 3;
const ANON: i32 = 0x22;
const NORESERVE: i32 = 0x4000;
const PAGE: usize = 4096;
const HUGE: usize = 2 << 20;
const DONTNEED: i32 = 4;
const HUGEPAGE: i32 = 14;
const NOHUGEPAGE: i32 = 15;
const POPULATE_WRITE: i32 = 23;

fn errno() -> i32 {
    unsafe { *__errno_location() }
}

fn adv(p: *mut u8, len: usize, advice: i32) -> i32 {
    if unsafe { madvise(p, len, advice) } != 0 {
        errno()
    } else {
        0
    }
}

/// A fresh private anonymous mapping of `len` bytes aligned to a huge page, and the raw
/// mapping to unmap later.
struct Map {
    raw: *mut u8,
    raw_len: usize,
    p: *mut u8,
    len: usize,
}

unsafe impl Send for Map {}

impl Map {
    fn new(len: usize) -> Map {
        let raw_len = len + HUGE;
        let raw = unsafe { mmap(std::ptr::null_mut(), raw_len, RW, ANON | NORESERVE, -1, 0) };
        assert!(raw as isize != -1, "mmap: {}", errno());
        let p = ((raw as usize + HUGE - 1) & !(HUGE - 1)) as *mut u8;
        Map { raw, raw_len, p, len }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        unsafe { munmap(self.raw, self.raw_len) };
    }
}

fn pin(cpu: usize) {
    let mut mask = [0u64; 16];
    mask[cpu / 64] |= 1 << (cpu % 64);
    let r = unsafe { sched_setaffinity(0, std::mem::size_of_val(&mask), mask.as_ptr()) };
    assert_eq!(r, 0, "pin to CPU {cpu}");
}

fn cpus() -> Vec<usize> {
    std::env::var("RF_CPUS")
        .unwrap_or_else(|_| "13".into())
        .split(',')
        .map(|s| s.trim().parse().expect("RF_CPUS"))
        .collect()
}

/// Counters on the calling thread: cycles in both modes, cycles in user mode, page faults.
struct Counters([i32; 3]);

impl Counters {
    fn open() -> Counters {
        let open = |ty: u64, config: u64, user_only: bool| -> i32 {
            // struct perf_event_attr, version 8 (128 bytes).
            let mut attr = [0u64; 16];
            attr[0] = ty | (128u64 << 32);
            attr[1] = config;
            let disabled = 1u64;
            let exclude_kernel = 1u64 << 5;
            let exclude_hv = 1u64 << 6;
            attr[5] = disabled | exclude_hv | if user_only { exclude_kernel } else { 0 };
            const PERF_EVENT_OPEN: i64 = 298;
            let fd = unsafe { syscall(PERF_EVENT_OPEN, attr.as_ptr(), 0i32, -1i32, -1i32, 8u64) };
            assert!(fd >= 0, "perf_event_open: {}", errno());
            fd as i32
        };
        Counters([open(0, 0, false), open(0, 0, true), open(1, 2, false)])
    }

    fn start(&self) {
        for &fd in &self.0 {
            unsafe {
                ioctl(fd, 0x2403, 0);
                ioctl(fd, 0x2400, 0);
            }
        }
    }

    /// (cycles, user cycles, page faults) since `start`.
    fn stop(&self) -> [u64; 3] {
        let mut out = [0u64; 3];
        for (i, &fd) in self.0.iter().enumerate() {
            unsafe { ioctl(fd, 0x2401, 0) };
            let mut b = [0u8; 8];
            let n = unsafe { read(fd, b.as_mut_ptr().cast(), 8) };
            assert_eq!(n, 8);
            out[i] = u64::from_ne_bytes(b);
        }
        out
    }
}

impl Drop for Counters {
    fn drop(&mut self) {
        for &fd in &self.0 {
            unsafe { close(fd) };
        }
    }
}

/// How a variant makes a fresh mapping resident before (or as) it writes every page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum How {
    /// Write each page; one fault per page.
    Demand,
    /// `MADV_POPULATE_WRITE` this many pages at a time, then write them.
    Populate(usize),
    /// `MADV_HUGEPAGE`, then write each page; one fault per 2 MiB.
    Huge,
    /// Write each page of memory that is already resident: the floor under all of the above.
    Present,
}

impl How {
    fn name(self) -> String {
        match self {
            How::Demand => "demand fault".into(),
            How::Populate(n) => format!("populate {n} pages"),
            How::Huge => "huge-page fault".into(),
            How::Present => "already resident".into(),
        }
    }
}

/// One measurement: (ns, cycles, user cycles, faults) for making `len` bytes resident.
fn run_one(how: How, len: usize, counters: &Counters, barrier: Option<&Barrier>) -> [f64; 4] {
    let m = Map::new(len);
    let advice = if how == How::Huge { HUGEPAGE } else { NOHUGEPAGE };
    assert_eq!(adv(m.p, m.len, advice), 0);
    if how == How::Present {
        assert_eq!(adv(m.p, m.len, POPULATE_WRITE), 0);
    }
    if let Some(b) = barrier {
        b.wait();
    }
    let t = Instant::now();
    counters.start();
    let step = match how {
        How::Populate(n) => n * PAGE,
        _ => len,
    };
    let mut at = 0;
    while at < len {
        let chunk = step.min(len - at);
        let q = unsafe { m.p.add(at) };
        if let How::Populate(_) = how {
            let e = adv(q, chunk, POPULATE_WRITE);
            assert_eq!(e, 0, "populate");
        }
        let mut o = 0;
        while o < chunk {
            unsafe { q.add(o).write_volatile(1) };
            o += PAGE;
        }
        at += chunk;
    }
    let c = counters.stop();
    let ns = t.elapsed().as_nanos() as f64;
    if let Some(b) = barrier {
        b.wait();
    }
    drop(m);
    [ns, c[0] as f64, c[1] as f64, c[2] as f64]
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// A small deterministic shuffle so each round runs the variants in a new order.
fn shuffle<T>(v: &mut [T], seed: &mut u64) {
    for i in (1..v.len()).rev() {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        v.swap(i, (*seed % (i as u64 + 1)) as usize);
    }
}

const VARIANTS: [How; 9] = [
    How::Demand,
    How::Populate(1),
    How::Populate(4),
    How::Populate(16),
    How::Populate(64),
    How::Populate(512),
    How::Populate(4096),
    How::Huge,
    How::Present,
];

fn report(how: How, per_page: &[Vec<[f64; 4]>]) {
    // per_page[thread][round]
    let all: Vec<[f64; 4]> = per_page.iter().flatten().copied().collect();
    let col = |i: usize| median(all.iter().map(|r| r[i]).collect());
    let lo = all.iter().map(|r| r[0]).fold(f64::MAX, f64::min);
    let hi = all.iter().map(|r| r[0]).fold(0.0, f64::max);
    let (ns, cyc, user, faults) = (col(0), col(1), col(2), col(3));
    println!(
        "  {:<22} {:>7.0} ns/page ({:>4.0}-{:<4.0})  {:>6.0} cycles/page, kernel {:>5.0}  faults/page {:.4}",
        how.name(),
        ns,
        lo,
        hi,
        cyc,
        cyc - user,
        faults
    );
}

fn single(mib: usize, rounds: usize) {
    let cpu = cpus()[0];
    pin(cpu);
    let len = mib << 20;
    let pages = (len / PAGE) as f64;
    let counters = Counters::open();
    let mut results: Vec<Vec<[f64; 4]>> = vec![Vec::new(); VARIANTS.len()];
    // RF_ONLY=text keeps the variants whose name contains it, for profiling one.
    let only = std::env::var("RF_ONLY").ok();
    let mut order: Vec<usize> = (0..VARIANTS.len())
        .filter(|&i| only.as_ref().is_none_or(|o| VARIANTS[i].name().contains(o.as_str())))
        .collect();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for round in 0..=rounds {
        shuffle(&mut order, &mut seed);
        for &i in &order {
            let r = run_one(VARIANTS[i], len, &counters, None);
            // Round 0 warms the kernel's paths and the page allocator's lists.
            if round > 0 {
                results[i].push(r.map(|x| x / pages));
            }
        }
    }
    println!("single thread on CPU {cpu}, {mib} MiB per run, median of {rounds} interleaved rounds");
    for &i in &order {
        report(VARIANTS[i], &[results[i].clone()]);
    }
}

fn threads(mib: usize, rounds: usize) {
    let cpus = cpus();
    let len = mib << 20;
    let pages = (len / PAGE) as f64;
    let variants = [How::Demand, How::Populate(16), How::Populate(64), How::Populate(512), How::Huge];
    for &n in &[1usize, 2, 4] {
        if n > cpus.len() {
            break;
        }
        let barrier = Arc::new(Barrier::new(n));
        let mut order: Vec<usize> = (0..variants.len()).collect();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut plan = Vec::new();
        for _ in 0..=rounds {
            shuffle(&mut order, &mut seed);
            plan.extend(order.iter().copied());
        }
        let handles: Vec<_> = (0..n)
            .map(|t| {
                let barrier = barrier.clone();
                let plan = plan.clone();
                let cpu = cpus[t];
                std::thread::spawn(move || {
                    pin(cpu);
                    let counters = Counters::open();
                    let mut out: Vec<Vec<[f64; 4]>> = vec![Vec::new(); variants.len()];
                    for (k, &i) in plan.iter().enumerate() {
                        let r = run_one(variants[i], len, &counters, Some(&barrier));
                        if k >= variants.len() {
                            out[i].push(r.map(|x| x / pages));
                        }
                    }
                    out
                })
            })
            .collect();
        let per_thread: Vec<Vec<Vec<[f64; 4]>>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        println!("{n} thread(s) on CPUs {:?}, each its own {mib} MiB mapping, all at once", &cpus[..n]);
        for (i, &how) in variants.iter().enumerate() {
            let rows: Vec<Vec<[f64; 4]>> = per_thread.iter().map(|t| t[i].clone()).collect();
            report(how, &rows);
        }
    }
}

fn maps_lines() -> usize {
    std::fs::read_to_string("/proc/self/maps").unwrap().lines().count()
}

/// Huge-page advice around a slot, as a block that grew would get it: advise, fill, purge,
/// take the advice back. Over many cycles and slot positions, with neighbours resident in
/// 4 KiB pages, does the mapping count come back, and what does each call cost?
fn advice(cycles: usize) {
    pin(cpus()[0]);
    // A region as the heap has one: committed, advised against huge pages, its slots of
    // 16 MiB, with 4 KiB pages resident in every other slot so that the VMA has an anon_vma
    // and the neighbours of an advised slot are in use.
    let slot = 16usize << 20;
    let slots = 32;
    let reserve = Map::new(slots * slot + (64 << 20));
    let region = reserve.p;
    assert_eq!(unsafe { mprotect(region, slots * slot + (64 << 20), 0) }, 0);
    assert_eq!(unsafe { mprotect(region, slots * slot, RW) }, 0);
    assert_eq!(adv(region, slots * slot, NOHUGEPAGE), 0);
    for s in (0..slots).step_by(2) {
        unsafe { region.add(s * slot).write_volatile(1) };
    }
    let base = maps_lines();
    let mut seed = 0x1234_5678u64;
    let (mut t_adv, mut t_fill, mut t_purge, mut t_unadv) = (0f64, 0f64, 0f64, 0f64);
    let (mut most, mut live): (usize, Vec<usize>) = (base, Vec::new());
    // Slots of a large class are at region + index * size; a 2.5 MiB class shows a slot whose
    // interior is advised and whose edges are not. Both sizes, interleaved.
    for c in 0..cycles {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let index = (seed % slots as u64) as usize;
        if live.contains(&index) {
            continue;
        }
        let (start, len) = if c % 2 == 0 {
            (index * slot, slot)
        } else {
            // A 5 MiB slot that starts 1.25 MiB in: advise its whole huge pages only.
            let s = region as usize + index * slot + 5 * HUGE / 8;
            let lo = (s + HUGE - 1) & !(HUGE - 1);
            let hi = (s + 5 * HUGE / 2) & !(HUGE - 1);
            (lo - region as usize, hi - lo)
        };
        let p = unsafe { region.add(start) };
        let t = Instant::now();
        assert_eq!(adv(p, len, HUGEPAGE), 0);
        t_adv += t.elapsed().as_nanos() as f64;
        let t = Instant::now();
        let mut o = 0;
        while o < len {
            unsafe { p.add(o).write_volatile(2) };
            o += PAGE;
        }
        t_fill += t.elapsed().as_nanos() as f64 / (len / HUGE) as f64;
        live.push(index);
        most = most.max(maps_lines());
        // Free a random earlier slot, so up to four are advised at once.
        if live.len() > 4 || c + 1 == cycles {
            while let Some(index) = (live.len() > if c + 1 == cycles { 0 } else { 4 }).then(|| live.remove(0)) {
                let (start, len) = (index * slot, slot);
                let p = unsafe { region.add(start) };
                let t = Instant::now();
                assert_eq!(adv(p, len, DONTNEED), 0);
                t_purge += t.elapsed().as_nanos() as f64;
                let t = Instant::now();
                assert_eq!(adv(p, len, NOHUGEPAGE), 0);
                t_unadv += t.elapsed().as_nanos() as f64;
                if index % 2 == 0 {
                    unsafe { p.write_volatile(1) };
                }
            }
        }
    }
    for &index in &live {
        let p = unsafe { region.add(index * slot) };
        adv(p, slot, DONTNEED);
        adv(p, slot, NOHUGEPAGE);
    }
    let n = cycles as f64;
    println!("advice around a slot, {cycles} cycles, up to 5 slots advised at once");
    println!("  maps lines: before {base}, most {most}, after {}", maps_lines());
    println!(
        "  MADV_HUGEPAGE {:.2} us, fill {:.1} us per 2 MiB, DONTNEED {:.2} us per slot, MADV_NOHUGEPAGE {:.2} us",
        t_adv / n / 1e3,
        t_fill / n / 1e3,
        t_purge / n / 1e3,
        t_unadv / n / 1e3
    );
}

fn cgroup_current() -> Option<u64> {
    let cg = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = cg.lines().next()?.strip_prefix("0::")?;
    std::fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.current")).ok()?.trim().parse().ok()
}

fn smaps(key: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn vmstat(key: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/vmstat").unwrap_or_default();
    s.lines()
        .find(|l| l.split_whitespace().next() == Some(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Purging part of a huge page: does the memory go back, or stay charged until the kernel
/// splits the page later?
fn split() {
    pin(cpus()[0]);
    let len = 256usize << 20;
    let show = |what: &str| {
        println!(
            "  {:<44} rss {:>4} MiB  huge {:>4} MiB  cgroup {:>5} MiB  deferred_split {} split {}",
            what,
            smaps("Rss:") >> 10,
            smaps("AnonHugePages:") >> 10,
            cgroup_current().map_or(-1, |b| (b >> 20) as i64),
            vmstat("thp_deferred_split_page"),
            vmstat("thp_split_page"),
        )
    };
    println!("DONTNEED inside huge pages, {} MiB", len >> 20);
    let m = Map::new(len);
    show("before");
    assert_eq!(adv(m.p, len, HUGEPAGE), 0);
    let mut o = 0;
    while o < len {
        unsafe { m.p.add(o).write_volatile(1) };
        o += PAGE;
    }
    show("filled in huge pages");
    let t = Instant::now();
    let mut o = 0;
    while o < len {
        assert_eq!(adv(unsafe { m.p.add(o) }, HUGE / 2, DONTNEED), 0);
        o += HUGE;
    }
    let d = t.elapsed().as_nanos() as f64 / (len / HUGE) as f64;
    show(&format!("first half of each purged ({:.1} us/call)", d / 1e3));
    std::thread::sleep(std::time::Duration::from_secs(2));
    show("2 s later");
    let mut o = 0;
    while o < len {
        assert_eq!(adv(unsafe { m.p.add(o + HUGE / 2) }, HUGE / 2, DONTNEED), 0);
        o += HUGE;
    }
    show("second half purged too");
    drop(m);
    let m = Map::new(len);
    assert_eq!(adv(m.p, len, HUGEPAGE), 0);
    let mut o = 0;
    while o < len {
        unsafe { m.p.add(o).write_volatile(1) };
        o += PAGE;
    }
    show("filled again");
    let t = Instant::now();
    assert_eq!(adv(m.p, len, DONTNEED), 0);
    let d = t.elapsed().as_nanos() as f64 / (len / HUGE) as f64;
    show(&format!("purged in whole huge pages ({:.1} us per 2 MiB)", d / 1e3));
}

/// Memory touched sparsely in 4 KiB pages, then advised for huge pages: does khugepaged
/// collapse it, and so make all of it resident?
fn khuge(seconds: u64) {
    let len = 64usize << 20;
    let m = Map::new(len);
    assert_eq!(adv(m.p, len, NOHUGEPAGE), 0);
    let mut o = 0;
    while o < len {
        unsafe { m.p.add(o).write_volatile(1) };
        o += HUGE;
    }
    assert_eq!(adv(m.p, len, HUGEPAGE), 0);
    println!(
        "one page written per 2 MiB of {} MiB, then MADV_HUGEPAGE; max_ptes_none {}",
        len >> 20,
        std::fs::read_to_string("/sys/kernel/mm/transparent_hugepage/khugepaged/max_ptes_none")
            .unwrap_or_default()
            .trim()
    );
    let t = Instant::now();
    loop {
        let (rss, huge) = (smaps("Rss:") >> 10, smaps("AnonHugePages:") >> 10);
        println!("  {:>4} s: rss {rss} MiB, huge {huge} MiB", t.elapsed().as_secs());
        if t.elapsed().as_secs() >= seconds || huge >= (len >> 20) as u64 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    match args.get(1).map(String::as_str) {
        Some("single") => single(arg(2, 64), arg(3, 9)),
        Some("threads") => threads(arg(2, 64), arg(3, 7)),
        Some("advice") => advice(arg(2, 4000)),
        Some("split") => split(),
        Some("khuge") => khuge(arg(2, 120) as u64),
        _ => eprintln!("usage: faults single|threads|advice|split|khuge [args]"),
    }
}
