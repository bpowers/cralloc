//! The OS interface on Linux, as raw system calls.
//!
//! The allocator calls no C library function of its own: every request to the kernel goes
//! through one `syscall` instruction (`syscall.rs`), and the only C library data it reads are
//! glibc's two rseq symbols (`glibc.rs`). The compiler may still emit calls to `memcpy` and
//! `memset`, as for any Rust code. [`LinuxOs`] is the `Os` implementation built on them.

mod glibc;
mod kernel;
mod syscall;

// For the generated routines, which Miri does not build.
#[cfg(not(miri))]
pub(crate) use glibc::__rseq_offset;
pub use kernel::LinuxOs;
#[cfg(any(test, feature = "sim"))]
pub(crate) use kernel::protect_none;
