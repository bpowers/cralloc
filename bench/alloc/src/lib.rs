//! The global allocator under test, chosen by cargo feature, and what it says about itself.
//!
//! A binary gets the allocator by linking this crate (any use of [`describe`] does). With no
//! feature it gets Rust's `System` allocator, which is glibc's malloc.
//!
//! Each feature gives the binary its allocator the way a Rust program usually gets it, so
//! that what we measure is what a Rust program would get, sized frees included:
//!
//! | Feature | Allocator code | Called as | Free |
//! |---|---|---|---|
//! | `mi` | `libmimalloc-sys`'s static build | the `mimalloc` crate 0.1.52 | `mi_free` |
//! | `mi-sized` | the same | the same, with the size passed back | `mi_free_size_aligned` |
//! | `mi-crate` | the `mimalloc` crate itself, with the mimalloc it bundles | itself | `mi_free` |
//! | `cralloc` | `cralloc::Cralloc`, `executable` feature | itself | its `dealloc` |
//! | `je` | `tikv-jemallocator` 0.6 itself, default features | itself | `sdallocx` |
//! | `tc` | google/tcmalloc, static | as `tcmalloc-better` 0.1.19 | `TCMallocInternalDeleteSizedAligned` |
//! | `sn` | `snmalloc-rs` 0.7 itself, default features (cmake build of its bundled snmalloc) | itself | `sn_rust_dealloc` |
//!
//! `je` is exactly what a service that depends on `tikv-jemallocator` gets: jemalloc built by
//! `tikv-jemalloc-sys`, static and with the `_rjem_` prefix, so C code in the process keeps
//! glibc's malloc. (Before 2026-10-17 `je` linked Fedora's shared libjemalloc instead; see
//! docs/research/measuring.md, "What else has gone wrong", for what differs.)
//!
//! `mimalloc` and `tikv-jemallocator` also route `realloc` and `alloc_zeroed` to the
//! allocator's own functions. `tcmalloc-better` uses the generic fallbacks (allocate, copy,
//! free; allocate, memset), and so does the `tc` wrapper: tcmalloc's own `realloc` is
//! `malloc`'s family, and a block grown or shrunk in place trips tcmalloc's check of a sampled
//! block's size at a sized free (see tc.rs).
//!
//! Every allocator module provides `NAME`, the allocator family a run must report, and
//! `entry()` and `version()` for [`describe`]. Adding one is a module, the four lines below
//! that select it, and a feature in Cargo.toml.

#[cfg(any(feature = "mi", feature = "mi-sized"))]
mod mi;
#[cfg(any(feature = "mi", feature = "mi-sized"))]
use mi as chosen;
#[cfg(any(feature = "mi", feature = "mi-sized"))]
#[global_allocator]
static GLOBAL: mi::Mimalloc = mi::Mimalloc;

#[cfg(feature = "mi-crate")]
mod mi_crate;
#[cfg(feature = "mi-crate")]
use mi_crate as chosen;
#[cfg(feature = "mi-crate")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(bench_layout_pad)]
mod pad;

#[cfg(feature = "cralloc")]
mod asm;
#[cfg(feature = "cralloc")]
use asm as chosen;
#[cfg(all(feature = "cralloc", not(feature = "cralloc-memstat"), not(feature = "live-bytes")))]
#[global_allocator]
static GLOBAL: cralloc::Cralloc = cralloc::Cralloc;
#[cfg(feature = "live-bytes")]
mod live;
#[cfg(feature = "live-bytes")]
#[global_allocator]
static GLOBAL: live::Counting = live::Counting;
#[cfg(feature = "cralloc-memstat")]
#[global_allocator]
static GLOBAL: memstat::Counting = memstat::Counting;
#[cfg(feature = "memacct")]
mod memacct;
#[cfg(feature = "cralloc-memstat")]
mod memstat;

#[cfg(feature = "je")]
mod je;
#[cfg(feature = "je")]
use je as chosen;
#[cfg(feature = "je")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "tc")]
mod tc;
#[cfg(feature = "tc")]
use tc as chosen;
#[cfg(feature = "tc")]
#[global_allocator]
static GLOBAL: tc::Tcmalloc = tc::Tcmalloc;

#[cfg(feature = "sn")]
mod sn;
#[cfg(feature = "sn")]
use sn as chosen;
#[cfg(feature = "sn")]
#[global_allocator]
static GLOBAL: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

// build.rs sets `bench_system` when no allocator feature is chosen.
#[cfg(bench_system)]
mod system;
#[cfg(bench_system)]
use system as chosen;

/// The allocator family this binary was built with.
pub const NAME: &str = chosen::NAME;

/// Starts what the allocator needs started before the workload runs: cralloc's background
/// thread, with the `cralloc-background` feature, and the cache domains of
/// `CRALLOC_DOMAIN_WIDTH`, with `cralloc-domains`. Nothing for any other build.
pub fn init() {
    #[cfg(feature = "je")]
    je::start_stats();
    #[cfg(feature = "cralloc-background")]
    cralloc::Cralloc::start_background().expect("cralloc's background thread starts");
    #[cfg(feature = "cralloc-domains")]
    if let Some(width) = std::env::var("CRALLOC_DOMAIN_WIDTH").ok().and_then(|w| w.parse().ok()) {
        cralloc::Cralloc::split_domains(width);
    }
}

/// Whether this build can break its heap down on demand with [`snapshot`]: cralloc's
/// `cralloc-memstat` build, with its sampler on (`CRALLOC_MEMSTAT`).
pub fn snapshot_supported() -> bool {
    #[cfg(feature = "cralloc-memstat")]
    return memstat::active();
    #[cfg(not(feature = "cralloc-memstat"))]
    false
}

/// Writes the heap's full breakdown now, tagged `tag` (non-zero, a new one each call), where
/// [`snapshot_supported`]; does nothing elsewhere.
pub fn snapshot(tag: u64) {
    #[cfg(feature = "cralloc-memstat")]
    memstat::snapshot(tag);
    #[cfg(not(feature = "cralloc-memstat"))]
    let _ = tag;
}

/// What the running process can tell about its allocator.
#[derive(Clone, Debug)]
pub struct Description {
    /// The allocator family: glibc, mimalloc, mimalloc-sized, jemalloc, tcmalloc or snmalloc.
    pub allocator: &'static str,
    /// A mode the allocator chose at run time, where it has one: tcmalloc's "per-cpu" or
    /// "per-thread". Empty otherwise.
    pub mode: &'static str,
    /// Where the allocator's code is: "executable" when it is linked into the binary, else
    /// the canonical path of the shared library that holds it. This is what tells apart two
    /// builds of one allocator, such as tikv-jemallocator's and the distribution's.
    pub library: String,
    /// The version the allocator reports at run time, where it has one.
    pub version: String,
    /// The size of the rseq area glibc registered for this thread; 0 if it registered none.
    pub glibc_rseq_size: u32,
}

/// Reports the allocator and the mode it is running in. Call it after the allocator has
/// served some requests: tcmalloc decides its mode on first use.
pub fn describe() -> Description {
    Description {
        allocator: allocator(),
        mode: mode(),
        library: library_of(chosen::entry()),
        version: chosen::version(),
        glibc_rseq_size: glibc_rseq_size(),
    }
}

/// `NAME`, unless the allocator can check at run time that it serves this process and finds
/// it does not.
fn allocator() -> &'static str {
    #[cfg(feature = "cralloc")]
    if !asm::serves() {
        return "not served by cralloc";
    }
    NAME
}

#[cfg(feature = "tc")]
fn mode() -> &'static str {
    if tc::per_cpu_caches_active() { "per-cpu" } else { "per-thread" }
}

#[cfg(not(feature = "tc"))]
fn mode() -> &'static str {
    ""
}

/// "executable" if `f` is in the same object as this crate's code, which is always linked
/// into the binary; otherwise the canonical path of the shared object that contains `f`.
fn library_of(f: *const std::ffi::c_void) -> String {
    let object = |p: *const std::ffi::c_void| {
        // SAFETY: Dl_info is plain data, and dladdr only reads the dynamic linker's tables.
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        if unsafe { libc::dladdr(p, &mut info) } == 0 {
            return None;
        }
        Some(info)
    };
    let Some(target) = object(f) else { return "unknown".into() };
    let here = object(describe as *const std::ffi::c_void);
    if here.is_some_and(|h| h.dli_fbase == target.dli_fbase) {
        return "executable".into();
    }
    if target.dli_fname.is_null() {
        return "unknown".into();
    }
    // SAFETY: dladdr returned a NUL-terminated path owned by the dynamic linker.
    let name = unsafe { std::ffi::CStr::from_ptr(target.dli_fname) };
    let path = std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(name.to_bytes()));
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).display().to_string()
}

/// jemalloc's version string, through whichever `mallctl` the build exports.
///
/// # Safety
/// `mallctl` must be jemalloc's `mallctl`.
#[cfg(feature = "je")]
unsafe fn je_version(
    mallctl: unsafe extern "C" fn(
        *const std::ffi::c_char,
        *mut std::ffi::c_void,
        *mut usize,
        *mut std::ffi::c_void,
        usize,
    ) -> std::ffi::c_int,
) -> String {
    let mut v: *const std::ffi::c_char = std::ptr::null();
    let mut len = std::mem::size_of_val(&v);
    // SAFETY: "version" is a const char * control; v and len describe room for one.
    let r = unsafe { mallctl(c"version".as_ptr(), (&raw mut v).cast(), &mut len, std::ptr::null_mut(), 0) };
    if r != 0 || v.is_null() {
        return String::new();
    }
    // SAFETY: jemalloc returns a static NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(v) }.to_string_lossy().into_owned()
}

fn glibc_rseq_size() -> u32 {
    unsafe extern "C" {
        // Exported by glibc 2.35 and later.
        static __rseq_size: u32;
    }
    // SAFETY: a plain read of an initialized constant that glibc exports.
    unsafe { __rseq_size }
}
