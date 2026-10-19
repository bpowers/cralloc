//! asmalloc as an `LD_PRELOAD` library: `LD_PRELOAD=libasmalloc_preload.so program` runs any
//! dynamically linked program with asmalloc as its `malloc`.
//!
//! The library is the allocator crate and the C names it exports (`asmalloc::export_c_abi!`),
//! and a constructor that starts the allocator's background thread if the environment asks
//! for it (`ASMALLOC_BACKGROUND`, `background.rs`); C++ reaches it through its runtime's calls
//! to `malloc` and `free`. It needs no initialization, has no destructor and keeps no thread
//! state, so it works from the dynamic loader's first call of `malloc` (after relocation, when
//! glibc has already registered rseq for the main thread) to the last `free` of the last
//! thread, and in a forked child. It is never unloaded, which the rseq descriptors of its
//! routines require: a thread's `rseq_cs` may point into it until the thread is next
//! interrupted.
//!
//! It is `no_std` and must not unwind. A panic cannot happen while serving a request (the
//! allocator rules out every one it can reach), and if one did it would end the process by the
//! allocator's own abort, which raises SIGABRT and never allocates.

// A test build (clippy's `--all-targets` makes one, though the library has no unit tests)
// links std, which brings its own panic handler.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]

mod background;

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

/// EXPERIMENT (domain-affine back end): the allocator's counters, one line per domain that
/// counted anything, written to standard error when the process exits.
#[cfg(feature = "x-count")]
mod xdump {
    use asmalloc::xcount::{NAMES, snapshot};

    unsafe extern "C" {
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    }

    struct Line {
        buf: [u8; 1024],
        len: usize,
    }

    impl Line {
        fn text(&mut self, s: &str) {
            for &b in s.as_bytes() {
                if self.len < self.buf.len() {
                    self.buf[self.len] = b;
                    self.len += 1;
                }
            }
        }

        fn number(&mut self, mut n: u64) {
            let mut digits = [0u8; 20];
            let mut i = digits.len();
            loop {
                i -= 1;
                digits[i] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 {
                    break;
                }
            }
            for &b in &digits[i..] {
                if self.len < self.buf.len() {
                    self.buf[self.len] = b;
                    self.len += 1;
                }
            }
        }
    }

    extern "C" fn dump() {
        for (d, counts) in snapshot().iter().enumerate() {
            if counts.iter().all(|&c| c == 0) {
                continue;
            }
            let mut line = Line {
                buf: [0; 1024],
                len: 0,
            };
            line.text("XCOUNT domain=");
            line.number(d as u64);
            for (name, &c) in NAMES.iter().zip(counts) {
                line.text(" ");
                line.text(name);
                line.text("=");
                line.number(c);
            }
            line.text("\n");
            // SAFETY: an experiment's diagnostic; the buffer is valid for `len` bytes.
            let _ = unsafe { write(2, line.buf.as_ptr(), line.len) };
        }
    }

    #[used]
    #[unsafe(link_section = ".fini_array")]
    static DUMP: extern "C" fn() = dump;
}
