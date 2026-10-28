//! tikv-jemallocator's own jemalloc, which this crate uses as the global allocator unchanged.
//! Its symbols carry the `_rjem_` prefix that tikv-jemalloc-sys configures, so naming them
//! here also checks at link time that the prefixed static build is the one linked.
//!
//! The trough-gap probe's copy (`bench/probes/trough-gap/`, docs/research/trough-gap.md), copied
//! over bench/alloc/src/je.rs in a scratch tree by `tg_edit.py` and built with jemalloc's
//! statistics on (`je-stats`). With `JEMALLOC_ACCT=<file>`, [`snapshot`] appends jemalloc's own
//! account of its memory to the file at a moment the workload chooses (every thread stopped):
//! the totals, the merged arenas' page counts, and every small bin's regions and slabs.
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt::Write as _;

unsafe extern "C" {
    fn _rjem_mallocx(size: usize, flags: c_int) -> *mut c_void;
    fn _rjem_mallctl(
        name: *const c_char,
        old: *mut c_void,
        oldlen: *mut usize,
        new: *mut c_void,
        newlen: usize,
    ) -> c_int;
    fn _rjem_malloc_stats_print(
        write_cb: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
        opaque: *mut c_void,
        opts: *const c_char,
    );
}

pub const NAME: &str = "jemalloc";

pub fn entry() -> *const c_void {
    _rjem_mallocx as *const c_void
}

pub fn version() -> String {
    // SAFETY: mallctl with the signature jemalloc documents.
    unsafe { crate::je_version(_rjem_mallctl) }
}

/// Unused here: the original's peak sampler.
pub fn start_stats() {}

/// Whether [`snapshot`] writes anything: `JEMALLOC_ACCT` names a file.
pub fn snapshot_supported() -> bool {
    std::env::var_os("JEMALLOC_ACCT").is_some()
}

fn read<T: Copy + Default>(name: &str) -> Option<T> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut v = T::default();
    let mut len = size_of::<T>();
    // SAFETY: mallctl with the signature jemalloc documents; `v` has room for a `T`, which the
    // caller names as the control's type.
    let rc = unsafe { _rjem_mallctl(name.as_ptr(), (&raw mut v).cast(), &mut len, std::ptr::null_mut(), 0) };
    (rc == 0 && len == size_of::<T>()).then_some(v)
}

fn refresh() {
    let mut epoch: u64 = 1;
    let mut len = size_of::<u64>();
    // SAFETY: as in `read`; "epoch" reads and writes a u64.
    unsafe {
        _rjem_mallctl(c"epoch".as_ptr(), (&raw mut epoch).cast(), &mut len, (&raw mut epoch).cast(), len);
    }
}

unsafe extern "C" fn print_cb(opaque: *mut c_void, text: *const c_char) {
    // SAFETY: `opaque` is the String `snapshot` passed, and jemalloc passes a NUL-terminated
    // string.
    let out = unsafe { &mut *opaque.cast::<String>() };
    out.push_str(&unsafe { CStr::from_ptr(text) }.to_string_lossy());
}

/// Appends jemalloc's account of its memory, tagged `tag`, to `JEMALLOC_ACCT`, and
/// `malloc_stats_print`'s report of the merged arenas.
pub fn snapshot(tag: u64) {
    let Some(path) = std::env::var_os("JEMALLOC_ACCT") else { return };
    refresh();
    let mut s = String::new();
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let rss = statm.split(' ').nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 4096;
    let _ = write!(s, "je-acct tag={tag} rss={rss}");
    for name in ["allocated", "active", "resident", "mapped", "retained", "metadata", "metadata_thp"] {
        let v = read::<usize>(&format!("stats.{name}")).map_or(-1, |v| v as i64);
        let _ = write!(s, " {name}={v}");
    }
    for name in ["pactive", "pdirty", "pmuzzy", "mapped", "retained", "base", "internal", "resident", "tcache_bytes", "small.allocated", "large.allocated"] {
        let v = read::<usize>(&format!("stats.arenas.4096.{name}")).map_or(-1, |v| v as i64);
        let _ = write!(s, " a.{name}={v}");
    }
    for name in ["dirty_decay_ms", "muzzy_decay_ms"] {
        let v = read::<isize>(&format!("opt.{name}")).map_or(-2, |v| v as i64);
        let _ = write!(s, " opt.{name}={v}");
    }
    let narenas = read::<u32>("arenas.narenas").unwrap_or(0);
    let _ = write!(s, " narenas={narenas}");
    let _ = writeln!(s, " background_thread={}", read::<bool>("opt.background_thread").map_or(-1, i64::from));
    let nbins = read::<u32>("arenas.nbins").unwrap_or(0);
    for j in 0..nbins {
        let size = read::<usize>(&format!("arenas.bin.{j}.size")).unwrap_or(0);
        let nregs = read::<u32>(&format!("arenas.bin.{j}.nregs")).unwrap_or(0);
        let slab = read::<usize>(&format!("arenas.bin.{j}.slab_size")).unwrap_or(0);
        let curregs = read::<usize>(&format!("stats.arenas.4096.bins.{j}.curregs")).unwrap_or(0);
        let curslabs = read::<usize>(&format!("stats.arenas.4096.bins.{j}.curslabs")).unwrap_or(0);
        let nonfull = read::<usize>(&format!("stats.arenas.4096.bins.{j}.nonfull_slabs")).unwrap_or(0);
        if curslabs == 0 && curregs == 0 {
            continue;
        }
        let _ = writeln!(
            s,
            "je-bin tag={tag} bin={j} size={size} nregs={nregs} slab_size={slab} curregs={curregs} curslabs={curslabs} nonfull_slabs={nonfull}"
        );
    }
    let nlextents = read::<u32>("arenas.nlextents").unwrap_or(0);
    for j in 0..nlextents {
        let cur = read::<usize>(&format!("stats.arenas.4096.lextents.{j}.curlextents")).unwrap_or(0);
        if cur == 0 {
            continue;
        }
        let size = read::<usize>(&format!("arenas.lextent.{j}.size")).unwrap_or(0);
        let _ = writeln!(s, "je-large tag={tag} lextent={j} size={size} cur={cur}");
    }
    // The full report for the merged arenas, without each arena's and the mutexes'.
    let _ = writeln!(s, "je-report tag={tag}");
    // SAFETY: the callback matches the signature jemalloc calls, and `opaque` is a String that
    // outlives the call.
    unsafe { _rjem_malloc_stats_print(Some(print_cb), (&raw mut s).cast(), c"ax".as_ptr()) };
    let _ = writeln!(s, "je-report-end tag={tag}");
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(s.as_bytes());
    }
}
