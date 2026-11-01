//! The allocator the suite judges.
//!
//! Every test and fuzz target reaches the allocator through this module. `GlobalAlloc` and
//! `Allocator` go to cralloc: the process's heap with the generated routines (`Cralloc`), or
//! under Miri, which cannot run assembly or make system calls, a heap over the simulated OS run
//! with twins (`SimCralloc`). The kit's own bookkeeping stays on the system allocator.
//!
//! The C functions are cralloc's (`cralloc::c`), called by their Rust paths: exported as
//! `malloc` they would replace the C library's for the whole test process, and the kit's own
//! bookkeeping would then run on the allocator it is judging. errno is the C library's, which
//! is where cralloc sets it. Under Miri, which has no cralloc C layer to run, they are Miri's.

#[cfg(not(miri))]
pub use cralloc::Cralloc as Global;
#[cfg(miri)]
pub use cralloc::SimCralloc as Global;

use crate::subject::{CFns, ViaC, ViaGlobalAlloc};

/// The C functions under test.
#[cfg(not(miri))]
pub fn c_fns() -> CFns {
    use cralloc::c;
    CFns {
        name: "cralloc",
        malloc: c::malloc,
        calloc: c::calloc,
        realloc: c::realloc,
        free: c::free,
        posix_memalign: c::posix_memalign,
        aligned_alloc: c::aligned_alloc,
        malloc_usable_size: Some(c::malloc_usable_size),
        sized_free: Some(crate::subject::SizedFree {
            free_sized: c::free_sized,
            free_aligned_sized: c::free_aligned_sized,
        }),
        reallocarray: Some(c::reallocarray),
        obsolete: Some(crate::subject::Obsolete {
            memalign: c::memalign,
            valloc: c::valloc,
            pvalloc: c::pvalloc,
        }),
        errno_location: CFns::LIBC.errno_location,
    }
}

/// The C functions under test with `malloc`, `calloc`, `free` and the sized frees their slow
/// paths alone (`cralloc::c::slow`), the Rust path that completes every call the generated
/// routines miss; the rest as in [`c_fns`], whose `malloc` and friends are the routines.
#[cfg(not(miri))]
pub fn c_fns_slow() -> CFns {
    use cralloc::c;
    CFns {
        name: "cralloc (slow paths)",
        malloc: c::slow::malloc_slow,
        calloc: c::slow::calloc_slow_args,
        free: c::slow::free_slow,
        sized_free: Some(crate::subject::SizedFree {
            free_sized: c::slow::free_sized_slow,
            free_aligned_sized: c::slow::free_aligned_sized_slow,
        }),
        ..c_fns()
    }
}
/// The C functions under test: under Miri, Miri's own.
#[cfg(miri)]
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
