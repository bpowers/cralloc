//! Cost of the kernel memory-management primitives an allocator back end is built from:
//! mapping, first-touch faults with 4 KiB and 2 MiB pages, the madvise family, mprotect, guard
//! pages and zeroing. Wall-clock times, single run, pinned to RF_CPU (default 2).
//!
//! madvise advice numbers used below: 4 DONTNEED, 8 FREE, 14 HUGEPAGE, 15 NOHUGEPAGE,
//! 23 POPULATE_WRITE, 25 COLLAPSE, 102 GUARD_INSTALL, 103 GUARD_REMOVE.
use std::time::Instant;
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
    fn __errno_location() -> *mut i32;
}
const RW: i32 = 3;
const ANON: i32 = 0x22;
const NORESERVE: i32 = 0x4000;
const POPULATE: i32 = 0x8000;
const MB2: usize = 2 << 20;

fn map(len: usize, flags: i32) -> *mut u8 {
    let p = unsafe { mmap(std::ptr::null_mut(), len, RW, ANON | flags, -1, 0) };
    assert!(p as isize != -1);
    p
}
fn map_aligned(len: usize) -> *mut u8 {
    let p = map(len + MB2, NORESERVE) as usize;
    ((p + MB2 - 1) & !(MB2 - 1)) as *mut u8
}
fn touch(p: *mut u8, len: usize, step: usize) {
    let mut o = 0;
    while o < len {
        unsafe { p.add(o).write_volatile(1) };
        o += step;
    }
}
fn adv(p: *mut u8, len: usize, a: i32) -> i32 {
    let r = unsafe { madvise(p, len, a) };
    if r != 0 { unsafe { *__errno_location() } } else { 0 }
}
fn rss_huge() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    let get = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()).unwrap_or(0);
    (get("Rss:"), get("AnonHugePages:"))
}

fn main() {
    let cpu: usize = std::env::var("RF_CPU").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let mask = [1u64 << cpu, 0, 0, 0];
    unsafe { sched_setaffinity(0, 32, mask.as_ptr()) };
    let len = 512usize << 20;
    let pages = len / 4096;

    {
        let n = 20000;
        // The first pass warms up the kernel paths.
        for pass in 0..2 {
            let t = Instant::now();
            for _ in 0..n {
                let p = map(MB2, 0);
                unsafe { munmap(p, MB2) };
            }
            if pass == 1 {
                println!("mmap+munmap 2MiB untouched: {:.0} ns/pair", t.elapsed().as_nanos() as f64 / n as f64);
            }
        }
    }
    {
        let n = 2000;
        let t = Instant::now();
        for _ in 0..n {
            let p = map(MB2, 0);
            touch(p, 65536, 4096);
            unsafe { munmap(p, MB2) };
        }
        println!("mmap + touch 16 pages + munmap: {:.0} ns/cycle", t.elapsed().as_nanos() as f64 / n as f64);
    }

    let p = map_aligned(len);
    let t = Instant::now();
    touch(p, len, 4096);
    let d = t.elapsed().as_nanos() as f64;
    println!("first touch, 4K pages: {:.0} ns/page ({:.2} GiB/s)  rss/hugepages kB {:?}", d / pages as f64, len as f64 / d * 1e9 / (1u64 << 30) as f64, rss_huge());

    let t = Instant::now();
    touch(p, len, 4096);
    println!("second touch (no fault): {:.1} ns/page", t.elapsed().as_nanos() as f64 / pages as f64);

    let t = Instant::now();
    let e = adv(p, len, 4);
    let d = t.elapsed().as_nanos() as f64;
    println!("MADV_DONTNEED 512MiB of 4K pages (one call): {:.0} ns/page errno {}", d / pages as f64, e);
    touch(p, len, 4096);
    let t = Instant::now();
    let mut o = 0;
    while o < len { adv(unsafe { p.add(o) }, MB2, 4); o += MB2; }
    let d = t.elapsed().as_nanos() as f64;
    println!("MADV_DONTNEED per 2MiB chunk of 4K pages: {:.0} ns/call, {:.0} ns/page", d / (len / MB2) as f64, d / pages as f64);
    let t = Instant::now();
    touch(p, len, 4096);
    println!("refault after DONTNEED, 4K: {:.0} ns/page", t.elapsed().as_nanos() as f64 / pages as f64);

    let t = Instant::now();
    let mut o = 0;
    while o < len { adv(unsafe { p.add(o) }, MB2, 8); o += MB2; }
    let d = t.elapsed().as_nanos() as f64;
    println!("MADV_FREE per 2MiB chunk of 4K pages: {:.0} ns/call, {:.0} ns/page", d / (len / MB2) as f64, d / pages as f64);
    let t = Instant::now();
    touch(p, len, 4096);
    println!("re-touch after MADV_FREE (not reclaimed): {:.0} ns/page  rss {:?}", t.elapsed().as_nanos() as f64 / pages as f64, rss_huge());

    // single-page DONTNEED + refault round trips
    let t = Instant::now();
    let n = 200000;
    for i in 0..n {
        let q = unsafe { p.add((i % 1024) * 4096) };
        adv(q, 4096, 4);
        unsafe { q.write_volatile(1) };
    }
    println!("single page DONTNEED + refault: {:.0} ns/round trip", t.elapsed().as_nanos() as f64 / n as f64);

    let t = Instant::now();
    let e = adv(p, len, 25);
    let d = t.elapsed().as_nanos() as f64;
    println!("MADV_COLLAPSE 512MiB of touched 4K pages: {:.0} us per 2MiB errno {}  rss {:?}", d / (len / MB2) as f64 / 1e3, e, rss_huge());
    let t = Instant::now();
    touch(p, len, 4096);
    println!("touch after collapse: {:.1} ns/page", t.elapsed().as_nanos() as f64 / pages as f64);
    let t = Instant::now();
    adv(p, len, 4);
    println!("MADV_DONTNEED 512MiB of THP: {:.0} ns per 2MiB", t.elapsed().as_nanos() as f64 / (len / MB2) as f64);

    // THP first touch
    let e = adv(p, len, 14);
    let t = Instant::now();
    touch(p, len, 4096);
    let d = t.elapsed().as_nanos() as f64;
    println!("first touch with MADV_HUGEPAGE: {:.0} us per 2MiB = {:.0} ns per 4K ({:.2} GiB/s) errno {} rss {:?}", d / (len / MB2) as f64 / 1e3, d / pages as f64, len as f64 / d * 1e9 / (1u64 << 30) as f64, e, rss_huge());
    // partial DONTNEED inside a THP (splits it)
    let t = Instant::now();
    let n = len / MB2;
    for i in 0..n { adv(unsafe { p.add(i * MB2 + 4096 * 8) }, 4096 * 16, 4); }
    println!("DONTNEED 64KiB inside each THP: {:.0} ns/call  rss {:?}", t.elapsed().as_nanos() as f64 / n as f64, rss_huge());
    adv(p, len, 4);
    adv(p, len, 15);

    // MADV_POPULATE_WRITE on 4K
    let t = Instant::now();
    let e = adv(p, len, 23);
    let d = t.elapsed().as_nanos() as f64;
    println!("MADV_POPULATE_WRITE 4K pages: {:.0} ns/page ({:.2} GiB/s) errno {} rss {:?}", d / pages as f64, len as f64 / d * 1e9 / (1u64 << 30) as f64, e, rss_huge());
    adv(p, len, 4);
    // populate in 64 KiB batches, as a refill path would
    let t = Instant::now();
    let mut o = 0;
    while o < len { adv(unsafe { p.add(o) }, 65536, 23); o += 65536; }
    let d = t.elapsed().as_nanos() as f64;
    println!("MADV_POPULATE_WRITE in 64KiB calls: {:.0} ns/call = {:.0} ns/page", d / (len / 65536) as f64, d / pages as f64);
    adv(p, len, 4);

    // mprotect toggles on a 2MiB chunk inside the mapping (splits and merges VMAs)
    let t = Instant::now();
    let n = 20000;
    for _ in 0..n {
        unsafe { mprotect(p.add(MB2 * 8), MB2, 0); mprotect(p.add(MB2 * 8), MB2, RW); }
    }
    println!("mprotect NONE+RW 2MiB (untouched): {:.0} ns/pair", t.elapsed().as_nanos() as f64 / n as f64);

    // guard pages without VMA splits (MADV_GUARD_INSTALL=102, REMOVE=103)
    let t = Instant::now();
    let mut e = 0;
    for _ in 0..n {
        e |= adv(unsafe { p.add(MB2 * 8) }, 4096, 102);
        e |= adv(unsafe { p.add(MB2 * 8) }, 4096, 103);
    }
    println!("MADV_GUARD_INSTALL+REMOVE one page: {:.0} ns/pair errno {}", t.elapsed().as_nanos() as f64 / n as f64, e);

    // MAP_POPULATE
    let t = Instant::now();
    let q = map(64 << 20, POPULATE);
    let d = t.elapsed().as_nanos() as f64;
    println!("mmap MAP_POPULATE 64MiB: {:.0} ns/page", d / ((64 << 20) / 4096) as f64);
    unsafe { munmap(q, 64 << 20) };

    // zeroing rates for calloc-style paths
    for (name, sz) in [("32KiB (L1)", 32usize << 10), ("512KiB (L2)", 512 << 10), ("16MiB (L3)", 16 << 20), ("256MiB (DRAM)", 256 << 20)] {
        let b = map(sz, 0);
        touch(b, sz, 4096);
        let reps = ((1usize << 31) / sz).max(1);
        let t = Instant::now();
        for _ in 0..reps { unsafe { std::ptr::write_bytes(b, 0, sz) }; std::hint::black_box(b); }
        let d = t.elapsed().as_nanos() as f64;
        println!("memset zero {}: {:.1} GiB/s", name, (sz * reps) as f64 / d * 1e9 / (1u64 << 30) as f64);
        unsafe { munmap(b, sz) };
    }
}
