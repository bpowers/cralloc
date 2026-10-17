//! The allocator the suite judges.
//!
//! Every test and fuzz target reaches the allocator through this module. `GlobalAlloc` and
//! `Allocator` go to asmalloc: the process's heap with the generated routines (`Asmalloc`), or
//! under Miri, which cannot run assembly or make system calls, a heap over the simulated OS run
//! with twins (`SimAsmalloc`). The kit's own bookkeeping stays on the system allocator.
//!
//! The C functions are still the C library's until asmalloc's C layer exists. Then [`c_fns`]
//! should return them, exported under prefixed names: an unprefixed `malloc` would replace the
//! C library's for the whole test process, and the kit's own bookkeeping would then run on the
//! allocator it is judging.

#[cfg(not(miri))]
pub use asmalloc::Asmalloc as Global;
#[cfg(miri)]
pub use asmalloc::SimAsmalloc as Global;

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
