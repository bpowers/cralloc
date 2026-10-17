//! The allocator the suite judges.
//!
//! Every test in this crate reaches the allocator through this module. To point the suite at
//! asmalloc, add the dependency and change the `use` below to
//! `pub use asmalloc::Asmalloc as Global;` (the zero-sized handle that implements `GlobalAlloc`
//! and `Allocator`), and [`C_ABI`] to asmalloc's C exports.

pub use std::alloc::System as Global;

use crate::subject::{CFns, ViaC, ViaGlobalAlloc};

/// The C functions under test.
pub const C_ABI: CFns = CFns::LIBC;

/// The `GlobalAlloc` interface under test.
pub fn global_alloc() -> ViaGlobalAlloc<Global> {
    ViaGlobalAlloc(Global)
}

/// The C interface under test.
pub fn c_abi() -> ViaC<CFns> {
    ViaC(C_ABI)
}

/// The `Allocator` interface under test.
#[cfg(feature = "allocator-api")]
pub fn allocator() -> crate::subject::ViaAllocator<Global> {
    crate::subject::ViaAllocator(Global)
}
