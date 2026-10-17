//! The global allocator under test, chosen by cargo feature, and what it says about itself.
//!
//! A binary gets the allocator by linking this crate (any use of [`describe`] does). With no
//! feature it gets Rust's `System` allocator, which is glibc's malloc.
//!
//! Each wrapper calls its allocator the way the usual Rust crate for it does, so that what we
//! measure is what a Rust program would get, sized frees included:
//!
//! | Feature | Crate it follows | Free |
//! |---|---|---|
//! | `mi` | `mimalloc` 0.1.52 | `mi_free` |
//! | `mi-sized` | the same, with the size passed back | `mi_free_size_aligned` |
//! | `je` | `tikv-jemallocator` 0.6 | `sdallocx` |
//! | `tc` | `tcmalloc-better` 0.1.19 | `TCMallocInternalDeleteSizedAligned` |
//!
//! `mimalloc` and `tikv-jemallocator` also route `realloc` and `alloc_zeroed` to the
//! allocator's own functions. `tcmalloc-better` uses the generic fallbacks (allocate, copy,
//! free; allocate, memset); the `tc` wrapper keeps the memset but resizes in place when the
//! size class allows, so that tcmalloc is not judged by a wrapper's shortcut (see tc.rs for
//! why it cannot simply call tcmalloc's `realloc`).

#[cfg(any(feature = "mi", feature = "mi-sized"))]
mod mi;
#[cfg(any(feature = "mi", feature = "mi-sized"))]
#[global_allocator]
static GLOBAL: mi::Mimalloc = mi::Mimalloc;

#[cfg(feature = "je")]
mod je;
#[cfg(feature = "je")]
#[global_allocator]
static GLOBAL: je::Jemalloc = je::Jemalloc;

#[cfg(feature = "tc")]
mod tc;
#[cfg(feature = "tc")]
#[global_allocator]
static GLOBAL: tc::Tcmalloc = tc::Tcmalloc;

/// The name of the allocator this binary was built with.
pub const NAME: &str = if cfg!(feature = "mi-sized") {
    "mimalloc-sized"
} else if cfg!(feature = "mi") {
    "mimalloc"
} else if cfg!(feature = "je") {
    "jemalloc"
} else if cfg!(feature = "tc") {
    "tcmalloc"
} else {
    "glibc"
};

/// What the running process can tell about its allocator.
#[derive(Clone, Debug)]
pub struct Description {
    /// The allocator family: glibc, mimalloc, mimalloc-sized, jemalloc or tcmalloc.
    pub allocator: &'static str,
    /// A mode the allocator chose at run time, where it has one: tcmalloc's "per-cpu" or
    /// "per-thread". Empty otherwise.
    pub mode: &'static str,
    /// The size of the rseq area glibc registered for this thread; 0 if it registered none.
    pub glibc_rseq_size: u32,
}

/// Reports the allocator and the mode it is running in. Call it after the allocator has
/// served some requests: tcmalloc decides its mode on first use.
pub fn describe() -> Description {
    Description { allocator: NAME, mode: mode(), glibc_rseq_size: glibc_rseq_size() }
}

#[cfg(feature = "tc")]
fn mode() -> &'static str {
    if tc::per_cpu_caches_active() { "per-cpu" } else { "per-thread" }
}

#[cfg(not(feature = "tc"))]
fn mode() -> &'static str {
    ""
}

fn glibc_rseq_size() -> u32 {
    unsafe extern "C" {
        // Exported by glibc 2.35 and later.
        static __rseq_size: u32;
    }
    // SAFETY: a plain read of an initialized constant that glibc exports.
    unsafe { __rseq_size }
}
