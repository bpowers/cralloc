//! asmalloc: a memory allocator for multithreaded Rust programs on modern Linux. It serves
//! `GlobalAlloc`, `Allocator` and the C `malloc` family from one heap, with per-CPU caches
//! under restartable sequences and fast paths in assembly proved correct in Lean.
//!
//! # Using it
//!
//! As the global allocator, with fat LTO so that the fast path is inlined into its callers:
//!
//! ```toml
//! [dependencies]
//! asmalloc = { git = "https://github.com/bpowers/asmalloc" }
//!
//! [profile.release]
//! lto = "fat"
//! ```
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: asmalloc::Asmalloc = asmalloc::Asmalloc;
//! ```
//!
//! A binary can add the `executable` feature, which saves a load on the fast path and does not
//! link into a shared library; a musl build always reads the area's offset that way. The `allocator-api` feature implements the `Allocator` trait for
//! [`Asmalloc`], on toolchains that have the trait (nightly, until it is stable).
//! [`Asmalloc::collect`] returns to the OS everything the heap can give back now, for a program
//! that knows it is idle, and [`Asmalloc::inspect`] reports where the heap's memory is
//! ([`stats`]).
//!
//! The heap's maintenance (returning idle memory, emptying idle caches) runs on its slow paths,
//! so a program whose threads all stay inside their caches never runs it and keeps what it
//! held when it went quiet. [`Asmalloc::maintain`] runs one round of it for the whole heap, for
//! a program with an idle hook of its own. The `background` feature adds
//! `Asmalloc::start_background`, which starts a thread that calls it about twice a second:
//!
//! ```ignore
//! fn main() {
//!     asmalloc::Asmalloc::start_background().expect("a thread for the allocator");
//!     // ...
//! }
//! ```
//!
//! For the C `malloc` family, [`export_c_abi!`] exports it under its C names from the crate
//! that invokes it (the `c-abi` feature does the same from this crate); the `asmalloc-preload`
//! library does this for any dynamically linked program.
//!
//! Limits worth knowing:
//!
//! - x86-64 Linux, with glibc 2.35 or later (which registers restartable sequences for its
//!   threads), dynamically or statically linked, or with musl in an executable: musl registers
//!   none, so there the allocator registers its own area for each thread at its first slow path
//!   and unregisters it as the thread exits. Statically linked musl executables are tested;
//!   dynamically linked ones are expected to work and untested. A musl shared library, such as
//!   a preload library, is not supported. A thread without a registration, as under valgrind
//!   or a sandbox that refuses one, is served correctly but slowly.
//! - A shared object that contains the allocator must be linked `-z nodelete`, as the preload
//!   library is: unloading it kills the process, because a thread's restartable-sequence area
//!   goes on naming code inside it.
//! - One copy per process: two copies have separate heaps and cannot free each other's blocks.
//! - [`Asmalloc`] does not exist under Miri, which cannot run its routines or system calls: a
//!   program checked under Miri puts `#[cfg(not(miri))]` on its `#[global_allocator]` and gets
//!   Miri's own allocator.
//!
//! `docs/design.md` in the repository says what the allocator is and why.
//!
//! # Inside
//!
//! Each component is a module directory whose root documents its contract. Components depend
//! downward only, and everything rests on three foundations: `classes` (size classes and
//! magazine shapes, pure arithmetic), `space` (where regions, metadata and magazines live in the
//! reserved address space) and `os` (what the allocator asks of the operating system, real or
//! simulated). `back` keeps the class regions and their free bitmaps, and moves memory to and
//! from the OS; `depot` exchanges magazines; `front` holds the per-CPU slabs and the routines.
//! The heap joins them into one allocator behind `Asmalloc`, and behind `SimAsmalloc` and
//! `SimHeap` (feature `sim`), the same heap over the simulated OS with twins. The components
//! are private; the hidden feature `internals` makes them public for the test kit, the fuzz
//! targets and the benchmarks, and is no part of the API.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
// The `Allocator` trait is stable from 1.100, above the version the crate otherwise needs; the
// feature that implements it is for toolchains that have it.
#![cfg_attr(feature = "allocator-api", allow(clippy::incompatible_msrv))]
// Without `internals` the components are private, and what only the test kit, the fuzz targets
// and the benchmarks use, and the re-exports that serve them, are unused.
#![cfg_attr(not(feature = "internals"), allow(dead_code, unused_imports))]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("asmalloc supports 64-bit targets only");

#[cfg(any(test, kani, feature = "sim"))]
extern crate alloc;
#[cfg(any(test, feature = "background"))]
extern crate std;

#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod back;
#[cfg(not(feature = "internals"))]
mod back;
#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod classes;
#[cfg(not(feature = "internals"))]
mod classes;
#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod depot;
#[cfg(not(feature = "internals"))]
mod depot;
#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod os;
#[cfg(not(feature = "internals"))]
mod os;
#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod space;
#[cfg(not(feature = "internals"))]
mod space;

// Public for what `export_c_abi!` expands to, which names the C functions, the slow paths and
// tables the generated C routines jump to and read, and `front::SLABS`. Without `internals`
// nothing else in them is reachable, and nothing reachable writes allocator state.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
#[doc(hidden)]
pub mod c;
#[doc(hidden)]
pub mod front;

mod heap;
mod lined;
#[cfg(feature = "x-count")]
#[doc(hidden)]
pub mod xcount;
/// The constants of the specification the routines were proved against, as the Lean emitter
/// writes them, for every target: the layout the Rust side builds from or asserts against.
#[path = "front/generated/constants.rs"]
#[rustfmt::skip]
pub(crate) mod spec;
pub mod stats;

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub use heap::Asmalloc;
#[cfg(any(test, feature = "sim"))]
#[doc(hidden)]
pub use heap::{SimAsmalloc, SimHeap};

/// Ends the process at once, writing `message` to standard error: for the panic handler of a
/// `no_std` library that exports the C interface with [`export_c_abi!`], which must not
/// unwind.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
#[cold]
pub fn abort(message: &str) -> ! {
    heap::process::abort(message)
}
