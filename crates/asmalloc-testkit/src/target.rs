//! The allocator the suite judges.
//!
//! Every test and fuzz target reaches the allocator through this module. To point them at
//! asmalloc, add the dependency, change the `use` below to `pub use asmalloc::Asmalloc as
//! Global;` (the zero-sized handle that implements `GlobalAlloc` and `Allocator`), and have
//! [`c_fns`] return asmalloc's C functions. Those should be exported under prefixed names for
//! the kit: an unprefixed `malloc` would replace the C library's for the whole test process,
//! and the kit's own bookkeeping would then run on the allocator it is judging.

pub use std::alloc::System as Global;

use crate::subject::{CFns, ViaC, ViaGlobalAlloc};

/// The C functions under test.
pub fn c_fns() -> CFns {
    CFns::LIBC
}

/// The `GlobalAlloc` interface under test.
pub fn global_alloc() -> ViaGlobalAlloc<Global> {
    ViaGlobalAlloc(Global)
}

/// The C interface under test.
pub fn c_abi() -> ViaC<CFns> {
    ViaC(c_fns())
}

/// The `Allocator` interface under test.
#[cfg(feature = "allocator-api")]
pub fn allocator() -> crate::subject::ViaAllocator<Global> {
    crate::subject::ViaAllocator(Global)
}
