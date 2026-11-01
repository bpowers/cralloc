//! cralloc: a memory allocator for multithreaded Rust programs on modern Linux. It serves
//! `GlobalAlloc`, `Allocator` and the C `malloc` family from one heap, with per-CPU caches
//! under restartable sequences and fast paths in assembly proved correct in Lean.
//!
//! # Using it
//!
//! As the global allocator, with fat LTO so that the fast path is inlined into its callers:
//!
//! ```toml
//! [dependencies]
//! cralloc = { git = "https://github.com/bpowers/cralloc" }
//!
//! [profile.release]
//! lto = "fat"
//! ```
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: cralloc::Cralloc = cralloc::Cralloc;
//! ```
//!
//! A binary can add the `executable` feature, which saves a load on the fast path and does not
//! link into a shared library; a musl build always reads the area's offset that way. The `allocator-api` feature implements the `Allocator` trait for
//! [`Cralloc`], on toolchains that have the trait (nightly, until it is stable).
//! [`Cralloc::collect`] returns to the OS everything the heap can give back now, for a program
//! that knows it is idle, and [`Cralloc::inspect`] reports where the heap's memory is
//! ([`stats`]).
//!
//! The heap's maintenance (returning idle memory, emptying idle caches) runs on its slow paths,
//! so a program whose threads all stay inside their caches would keep what it held when it
//! went quiet. The `background` feature, on by default, adds a thread that maintains the heap
//! during silences: it starts by default once the heap holds more than the floor of the
//! allowance (32 MiB) and the program has created a thread of its own, and parks while the
//! process is quiet. While it runs, returning memory to the OS is its work and not a slow
//! path's, so that no request waits for it. `CRALLOC_BACKGROUND=0` in the environment the
//! process was started with turns it off (a `setenv` in the program does not), and
//! `Cralloc::start_background` starts it at once. [`Cralloc::maintain`] runs one round of
//! the maintenance for a program with a timer of its own, which then gets no default thread:
//!
//! ```ignore
//! fn main() {
//!     cralloc::Cralloc::start_background().expect("a thread for the allocator");
//!     // ...
//! }
//! ```
//!
//! For the C `malloc` family, [`export_c_abi!`] exports it under its C names from the crate
//! that invokes it (the `c-abi` feature does the same from this crate); the `cralloc-preload`
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
//! - [`Cralloc`] does not exist under Miri, which cannot run its routines or system calls: a
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
//! The heap joins them into one allocator behind `Cralloc`, and behind `SimCralloc` and
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
compile_error!("cralloc supports 64-bit targets only");

#[cfg(any(test, kani, feature = "sim"))]
extern crate alloc;
#[cfg(test)]
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

mod cells;
#[cfg(feature = "internals")]
#[doc(hidden)]
pub mod clock;
#[cfg(not(feature = "internals"))]
mod clock;
pub(crate) mod environment;
#[cfg(all(
    test,
    target_os = "linux",
    target_arch = "x86_64",
    not(miri),
    not(kani)
))]
mod fresh;
mod heap;
mod lined;
#[cfg(test)]
mod scale;
/// The constants of the specification the routines were proved against, as the Lean emitter
/// writes them, for every target: the layout the Rust side builds from or asserts against.
#[path = "front/generated/constants.rs"]
#[rustfmt::skip]
pub(crate) mod spec;
pub mod stats;
pub(crate) mod thread_word;

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub use heap::Cralloc;

/// The background thread's controls (feature `background`, on by default): what the
/// environment asks for, and the error of a failed start. See `Cralloc::start_background`.
#[cfg(all(
    feature = "background",
    target_os = "linux",
    target_arch = "x86_64",
    not(miri),
    not(kani)
))]
pub mod background {
    #[doc(hidden)]
    pub use crate::heap::background::CreateThread;
    pub use crate::heap::background::{Choice, StartError, choice};
}
#[cfg(any(test, feature = "sim"))]
#[doc(hidden)]
pub use heap::{SimCralloc, SimHeap};

/// Ends the process at once, writing `message` to standard error: for the panic handler of a
/// `no_std` library that exports the C interface with [`export_c_abi!`], which must not
/// unwind.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
#[cold]
pub fn abort(message: &str) -> ! {
    heap::process::abort(message)
}
