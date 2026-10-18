//! asmalloc: a memory allocator for multithreaded Rust programs on modern Linux.
//!
//! `docs/design.md` says what it is and why. Each component is a module directory whose root
//! documents its contract. Components depend downward only, and everything rests on three
//! foundations: [`classes`] (size classes and magazine shapes, pure arithmetic), [`space`] (where
//! regions, metadata and magazines live in the reserved address space) and [`os`] (what the
//! allocator asks of the operating system, real or simulated). [`back`] keeps the class regions
//! and their free bitmaps, and moves memory to and from the OS; [`depot`] exchanges magazines;
//! [`front`] holds the per-CPU slabs and the routines. The heap joins them into one allocator
//! behind `Asmalloc`, the handle a program installs with `#[global_allocator]`, and behind
//! `SimAsmalloc` and `SimHeap` (feature `sim`), the same heap over the simulated OS with twins.
//!
//! A shared object that contains the allocator must be linked `-z nodelete`, as the preload
//! library is: unloading one kills the process, because a thread's restartable-sequence area
//! goes on naming code inside it.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
// The `Allocator` trait is stable from 1.100, above the version the crate otherwise needs; the
// feature that implements it is for toolchains that have it.
#![cfg_attr(feature = "allocator-api", allow(clippy::incompatible_msrv))]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("asmalloc supports 64-bit targets only");

#[cfg(any(test, kani, feature = "sim"))]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod back;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub mod c;
pub mod classes;
pub mod depot;
pub mod front;
mod heap;
pub mod os;
pub mod space;
/// The constants of the specification the routines were proved against, as the Lean emitter
/// writes them, for every target: the layout the Rust side builds from or asserts against.
#[path = "front/generated/constants.rs"]
#[rustfmt::skip]
pub(crate) mod spec;
pub mod stats;

#[cfg(any(miri, all(target_os = "linux", target_arch = "x86_64", not(kani))))]
pub use heap::Asmalloc;
#[cfg(any(test, feature = "sim"))]
pub use heap::{SimAsmalloc, SimHeap};
