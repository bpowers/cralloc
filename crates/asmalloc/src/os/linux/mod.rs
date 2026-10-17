//! The OS interface on Linux, as raw system calls.
//!
//! The allocator links against no C library function: every request to the kernel goes through
//! one `syscall` instruction ([`syscall`]), and the only C library data it reads are glibc's two
//! rseq symbols ([`glibc`]). [`LinuxOs`] is the `Os` implementation built on them.

mod glibc;
mod kernel;
mod syscall;

pub use kernel::LinuxOs;
