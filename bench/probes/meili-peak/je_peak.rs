//! tikv-jemallocator's own jemalloc: the meili-peak variant of bench-alloc's je.rs
//! (bench/probes/meili-peak/peak_edit.py copies it over je.rs in a scratch tree;
//! docs/research/meili-peak.md). Not product code.
//!
//! With the `je-stats` feature the global allocator is [`Counting`], jemalloc with the bytes
//! the program asked for counted, and jemalloc is built with statistics. With
//! `JEMALLOC_MEMSTAT=<file>`, a thread writes to `<file>.<pid>` every `JEMALLOC_MEMSTAT_MS`
//! (default 250) a line `je` with the process's anonymous memory, glibc's arenas
//! (`mallinfo2`), what the program asked for (`req`), and jemalloc's totals (allocated,
//! active, resident, mapped, retained, metadata, and the dirty and muzzy pages); and every
//! `JEMALLOC_MEMSTAT_DETAIL`th sample (default 4) its small bins (`jeb`: size, regions in use,
//! slabs, regions a slab, slab size), large extents in use (`jel`: size, count) and free
//! extents by size (`jex`: dirty, muzzy and retained bytes).
use std::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    fn _rjem_mallocx(size: usize, flags: c_int) -> *mut c_void;
    fn _rjem_mallctl(
        name: *const c_char,
        old: *mut c_void,
        oldlen: *mut usize,
        new: *mut c_void,
        newlen: usize,
    ) -> c_int;
}

pub const NAME: &str = "jemalloc";

pub fn entry() -> *const c_void {
    _rjem_mallocx as *const c_void
}

pub fn version() -> String {
    // SAFETY: mallctl with the signature jemalloc documents.
    unsafe { crate::je_version(_rjem_mallctl) }
}

#[repr(C)]
#[derive(Default)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

unsafe extern "C" {
    fn mallinfo2() -> Mallinfo2;
}

fn read_usize(name: &str) -> u64 {
    let c = std::ffi::CString::new(name).unwrap();
    let mut v: usize = 0;
    let mut len = size_of::<usize>();
    // SAFETY: mallctl with the signature jemalloc documents; these statistics are size_t.
    let rc = unsafe { _rjem_mallctl(c.as_ptr(), (&raw mut v).cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc == 0 { v as u64 } else { u64::MAX }
}

fn read_u32(name: &str) -> u64 {
    let c = std::ffi::CString::new(name).unwrap();
    let mut v: u32 = 0;
    let mut len = size_of::<u32>();
    // SAFETY: as above, for unsigned controls.
    let rc = unsafe { _rjem_mallctl(c.as_ptr(), (&raw mut v).cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc == 0 { v as u64 } else { u64::MAX }
}

fn read_ssize(name: &str) -> i64 {
    let c = std::ffi::CString::new(name).unwrap();
    let mut v: isize = 0;
    let mut len = size_of::<isize>();
    // SAFETY: as above, for ssize_t controls.
    let rc = unsafe { _rjem_mallctl(c.as_ptr(), (&raw mut v).cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc == 0 { v as i64 } else { i64::MIN }
}

fn refresh() {
    let mut epoch: u64 = 1;
    let mut len = size_of::<u64>();
    // SAFETY: "epoch" reads and writes a u64.
    unsafe {
        _rjem_mallctl(c"epoch".as_ptr(), (&raw mut epoch).cast(), &mut len, (&raw mut epoch).cast(), len);
    }
}

fn anon() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let mut f = statm.split(' ').map(|s| s.trim().parse::<u64>().unwrap_or(0));
    let (_, resident, shared) = (f.next(), f.next().unwrap_or(0), f.next().unwrap_or(0));
    resident.saturating_sub(shared) * 4096
}

pub fn start_stats() {
    let Ok(path) = std::env::var("JEMALLOC_MEMSTAT") else {
        return;
    };
    let ms = std::env::var("JEMALLOC_MEMSTAT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(250u64);
    let detail = std::env::var("JEMALLOC_MEMSTAT_DETAIL").ok().and_then(|v| v.parse().ok()).unwrap_or(4u64).max(1);
    let path = format!("{path}.{}", std::process::id());
    std::thread::spawn(move || {
        use std::io::Write as _;
        let start = std::time::Instant::now();
        let Ok(mut out) = std::fs::File::create(&path) else {
            return;
        };
        let _ = writeln!(
            out,
            "jeopt dirty_decay_ms={} muzzy_decay_ms={} narenas={} nbins={} nlextents={}",
            read_ssize("opt.dirty_decay_ms"),
            read_ssize("opt.muzzy_decay_ms"),
            read_u32("opt.narenas"),
            read_u32("arenas.nbins"),
            read_u32("arenas.nlextents"),
        );
        let nbins = read_u32("arenas.nbins");
        let nlextents = read_u32("arenas.nlextents");
        let bins: Vec<(u64, u64, u64)> = (0..nbins)
            .map(|j| {
                (
                    read_usize(&format!("arenas.bin.{j}.size")),
                    read_u32(&format!("arenas.bin.{j}.nregs")),
                    read_usize(&format!("arenas.bin.{j}.slab_size")),
                )
            })
            .collect();
        let lsizes: Vec<u64> = (0..nlextents).map(|j| read_usize(&format!("arenas.lextent.{j}.size"))).collect();
        let mut n = 0u64;
        loop {
            refresh();
            let a = "stats.arenas.4096";
            // SAFETY: glibc's mallinfo2 takes no arguments and returns a struct by value.
            let mi = unsafe { mallinfo2() };
            let _ = writeln!(
                out,
                "je t_ms={} anon={} g_arena={} g_mmap={} g_used={} g_free={} g_top={} req={} allocated={} active={} resident={} mapped={} retained={} metadata={} pactive={} pdirty={} pmuzzy={} small={} large={}",
                start.elapsed().as_millis(),
                anon(),
                mi.arena,
                mi.hblkhd,
                mi.uordblks,
                mi.fordblks,
                mi.keepcost,
                requested(),
                read_usize("stats.allocated"),
                read_usize("stats.active"),
                read_usize("stats.resident"),
                read_usize("stats.mapped"),
                read_usize("stats.retained"),
                read_usize("stats.metadata"),
                read_usize(&format!("{a}.pactive")).saturating_mul(4096),
                read_usize(&format!("{a}.pdirty")).saturating_mul(4096),
                read_usize(&format!("{a}.pmuzzy")).saturating_mul(4096),
                read_usize(&format!("{a}.small.allocated")),
                read_usize(&format!("{a}.large.allocated")),
            );
            if n.is_multiple_of(detail) {
                let mut line = format!("jeb t_ms={}", start.elapsed().as_millis());
                for (j, (size, nregs, slab)) in bins.iter().enumerate() {
                    let regs = read_usize(&format!("{a}.bins.{j}.curregs"));
                    let slabs = read_usize(&format!("{a}.bins.{j}.curslabs"));
                    if regs != 0 || slabs != 0 {
                        line += &format!(" {size}:{regs}:{slabs}:{nregs}:{slab}");
                    }
                }
                let _ = writeln!(out, "{line}");
                let mut line = format!("jel t_ms={}", start.elapsed().as_millis());
                for (j, size) in lsizes.iter().enumerate() {
                    let cur = read_usize(&format!("{a}.lextents.{j}.curlextents"));
                    if cur != 0 && cur != u64::MAX {
                        line += &format!(" {size}:{cur}");
                    }
                }
                let _ = writeln!(out, "{line}");
                // Free extents by page-size class (jemalloc's psz classes, SC_NPSIZES of them).
                let mut line = format!("jex t_ms={}", start.elapsed().as_millis());
                for j in 0..256u32 {
                    let d = read_usize(&format!("{a}.extents.{j}.dirty_bytes"));
                    if d == u64::MAX {
                        break;
                    }
                    let m = read_usize(&format!("{a}.extents.{j}.muzzy_bytes"));
                    let r = read_usize(&format!("{a}.extents.{j}.retained_bytes"));
                    if d != 0 || m != 0 || r != 0 {
                        line += &format!(" {j}:{d}:{m}:{r}");
                    }
                }
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
            n += 1;
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    });
}

/// What the program holds as it asked for it, in shards so that threads do not share lines.
#[repr(align(128))]
struct Shard(std::sync::atomic::AtomicI64);

const NSHARDS: usize = 64;
static SHARDS: [Shard; NSHARDS] = [const { Shard(std::sync::atomic::AtomicI64::new(0)) }; NSHARDS];
static NEXT_SHARD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

thread_local! {
    static MY_SHARD: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

fn add(bytes: i64) {
    use std::sync::atomic::Ordering::Relaxed;
    let i = MY_SHARD.with(|s| {
        let mut i = s.get();
        if i == usize::MAX {
            i = NEXT_SHARD.fetch_add(1, Relaxed) % NSHARDS;
            s.set(i);
        }
        i
    });
    SHARDS[i].0.fetch_add(bytes, Relaxed);
}

fn requested() -> i64 {
    SHARDS.iter().map(|s| s.0.load(std::sync::atomic::Ordering::Relaxed)).sum()
}

/// jemalloc, counting what the program asks for.
pub struct Counting;

// SAFETY: every call goes to tikv-jemallocator unchanged; the counting touches no block.
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { tikv_jemallocator::Jemalloc.alloc(layout) };
        if !p.is_null() {
            add(layout.size() as i64);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { tikv_jemallocator::Jemalloc.alloc_zeroed(layout) };
        if !p.is_null() {
            add(layout.size() as i64);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        add(-(layout.size() as i64));
        // SAFETY: the caller's promise, passed on.
        unsafe { tikv_jemallocator::Jemalloc.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new: usize) -> *mut u8 {
        // SAFETY: the caller's promise, passed on.
        let p = unsafe { tikv_jemallocator::Jemalloc.realloc(ptr, layout, new) };
        if !p.is_null() {
            add(new as i64 - layout.size() as i64);
        }
        p
    }
}
