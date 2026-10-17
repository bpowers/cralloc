//! asmalloc: a memory allocator for multithreaded Rust programs on modern Linux.
//!
//! `docs/design.md` says what it is and why. Each component is a module directory whose root
//! documents its contract. Components depend downward only, and everything rests on three
//! foundations: [`classes`] (size classes and magazine shapes, pure arithmetic), [`space`] (where
//! regions, metadata and magazines live in the reserved address space) and [`os`] (what the
//! allocator asks of the operating system, real or simulated). [`back`] keeps the class regions
//! and their free bitmaps, and moves memory to and from the OS.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("asmalloc supports 64-bit targets only");

#[cfg(any(test, kani, feature = "sim"))]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod back;
pub mod classes;
pub mod depot;
pub mod front;
#[cfg(test)]
pub(crate) mod generated;
pub mod os;
pub mod space;
