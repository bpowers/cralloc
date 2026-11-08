//! The OS interface on Linux, as raw system calls.
//!
//! The allocator calls no C library function of its own: every request to the kernel goes
//! through one `syscall` instruction (`syscall.rs`), and the only C library datum it reads is
//! glibc's `__rseq_offset`, which the routines load (`front`). On musl, which registers no rseq
//! area, the allocator registers its own (`rseq.rs`, `front::area`). The compiler may still
//! emit calls to `memcpy` and `memset`, as for any Rust code. [`LinuxOs`] is the `Os`
//! implementation built on them.

mod kernel;
#[cfg(target_env = "musl")]
pub(crate) mod rseq;
mod syscall;

#[cfg(test)]
pub(crate) use kernel::CACHE_INDEXES;
pub use kernel::LinuxOs;
#[cfg(any(test, feature = "sim"))]
pub(crate) use kernel::protect_none;
