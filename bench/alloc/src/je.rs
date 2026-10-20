//! tikv-jemallocator's own jemalloc, which this crate uses as the global allocator unchanged.
//! Its symbols carry the `_rjem_` prefix that tikv-jemalloc-sys configures, so naming them
//! here also checks at link time that the prefixed static build is the one linked.
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

/// With `JEMALLOC_MEMSTAT=<file>`, a thread that every `JEMALLOC_MEMSTAT_MS` (default 20) reads
/// jemalloc's own statistics and the resident set, and keeps in the file the sample with the
/// largest resident set: allocated (what the program holds), active (pages of active runs),
/// resident, mapped and retained, metadata (these need jemalloc built with statistics, which
/// tikv-jemallocator's default is not: they read as all ones), and the active, dirty and muzzy
/// pages of all arenas, which it always keeps. For
/// setting beside asmalloc's sampler at the same point (`docs/research/memory.md`).
pub fn start_stats() {
    let Ok(path) = std::env::var("JEMALLOC_MEMSTAT") else {
        return;
    };
    let ms = std::env::var("JEMALLOC_MEMSTAT_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(20u64);
    std::thread::spawn(move || {
        let mut best_rss = 0;
        loop {
            let sample = stats();
            let rss = sample.iter().find(|(k, _)| *k == "rss").map_or(0, |s| s.1);
            if rss >= best_rss {
                best_rss = rss;
                let text: String = sample.iter().map(|(k, v)| format!("{k}={v} ")).collect();
                let _ = std::fs::write(&path, format!("jemalloc-peak {text}\n"));
            }
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    });
}

fn stats() -> Vec<(&'static str, u64)> {
    let mut epoch: u64 = 1;
    let mut len = size_of::<u64>();
    // SAFETY: mallctl with the signature jemalloc documents; "epoch" reads and writes a u64.
    unsafe {
        _rjem_mallctl(c"epoch".as_ptr(), (&raw mut epoch).cast(), &mut len, (&raw mut epoch).cast(), len);
    }
    let read = |name: &std::ffi::CStr| -> u64 {
        let mut v: usize = 0;
        let mut len = size_of::<usize>();
        // SAFETY: as above; these statistics are size_t.
        let rc = unsafe { _rjem_mallctl(name.as_ptr(), (&raw mut v).cast(), &mut len, std::ptr::null_mut(), 0) };
        if rc == 0 { v as u64 } else { u64::MAX }
    };
    let page = 4096;
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let rss = statm.split(' ').nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * page;
    vec![
        ("rss", rss),
        ("allocated", read(c"stats.allocated")),
        ("active", read(c"stats.active")),
        ("resident", read(c"stats.resident")),
        ("mapped", read(c"stats.mapped")),
        ("retained", read(c"stats.retained")),
        ("metadata", read(c"stats.metadata")),
        ("active_pages", read(c"stats.arenas.4096.pactive").saturating_mul(page)),
        ("dirty", read(c"stats.arenas.4096.pdirty").saturating_mul(page)),
        ("muzzy", read(c"stats.arenas.4096.pmuzzy").saturating_mul(page)),
    ]
}
