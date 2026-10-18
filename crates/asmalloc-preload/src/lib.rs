//! asmalloc as an `LD_PRELOAD` library: `LD_PRELOAD=libasmalloc_preload.so program` runs any
//! dynamically linked program with asmalloc as its `malloc`.
//!
//! The library is the allocator crate and the C names it exports (`asmalloc::export_c_abi!`),
//! nothing more; C++ reaches it through its runtime's calls to `malloc` and `free`. It has no
//! constructor and no destructor and keeps no thread state, so it works
//! from the dynamic loader's first call of `malloc` (after relocation, when glibc has already
//! registered rseq for the main thread) to the last `free` of the last thread, and in a forked
//! child. It is never unloaded, which the rseq descriptors of its routines require: a thread's
//! `rseq_cs` may point into it until the thread is next interrupted.
//!
//! It is `no_std` and must not unwind. A panic cannot happen while serving a request (the
//! allocator rules out every one it can reach), and if one did it would end the process by the
//! allocator's own abort, which raises SIGABRT and never allocates.

// A test build (clippy's `--all-targets` makes one, though the library has no unit tests)
// links std, which brings its own panic handler.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]

// Only the C names. The C++ operators stay the C++ runtime's, which calls this `malloc` and
// `free`: exporting some of them without the throwing `new` forms is unsound when another
// library defines the operators (design: "Interfaces", C++). Replacing the C library's
// allocator this way is PRELOAD-01.
asmalloc::export_c_abi!();

// The library has no Rust allocations of its own, but cargo unifies features across the
// workspace, and one that pulls in the `alloc` crate (the test kit's `sim`) would then need a
// global allocator to link. This is the one heap's handle, so it is the right one if so.
#[global_allocator]
static GLOBAL: asmalloc::Asmalloc = asmalloc::Asmalloc;

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    asmalloc::abort("asmalloc-preload: panic")
}

/// What the unwinder would call as the personality routine of Rust frames.
///
/// The precompiled `core` is built to unwind, and its unwind tables name `rust_eh_personality`,
/// so the loader refuses the library unless that name resolves. Nothing in the library unwinds
/// (it is built with `panic = "abort"`, and no foreign exception can start below its frames),
/// so the unwinder never calls it; if it did, aborting is the only answer that keeps the heap
/// intact. `build.rs` binds `rust_eh_personality` to this function at link time, which keeps
/// that name out of the library's exports. Exported, it would take the place of the real one
/// for every Rust shared library loaded after this one that looks its own up dynamically
/// (`librustc_driver` does), and their panics would abort instead of unwinding.
// PRELOAD-02: why exporting this symbol, and binding the personality name to it, is sound.
#[unsafe(no_mangle)]
extern "C" fn asmalloc_preload_no_unwinding() -> ! {
    asmalloc::abort("asmalloc-preload: unwinding is not supported")
}
