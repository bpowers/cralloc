//! A static program in which the routines exported under their C names serve every call, including
//! the standard library's own: on musl those are copies apart from the ones the crate's own tests
//! run. Prints `ok` and what it did, or panics.

// The optimizer knows these names, and would fold a comparison of two results or drop a pair of calls.
#![no_builtins]

use core::ffi::{CStr, c_char, c_void};
use std::sync::mpsc;

cralloc::export_c_abi!();

/// The names as any C program reaches them, through the linker.
mod c {
    use super::{c_char, c_void};
    unsafe extern "C" {
        pub fn malloc(size: usize) -> *mut c_void;
        pub fn calloc(count: usize, size: usize) -> *mut c_void;
        pub fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        pub fn free(ptr: *mut c_void);
        pub fn free_sized(ptr: *mut c_void, size: usize);
        pub fn malloc_usable_size(ptr: *mut c_void) -> usize;
        pub fn cralloc_version() -> *const c_char;
    }
}

/// Every small size and some large ones.
fn sizes() -> impl Iterator<Item = usize> + Clone {
    (1..=2048)
        .step_by(7)
        .chain([4096, 8 << 10, 32 << 10, 64 << 10, 1 << 20, 3 << 20])
}

fn filled(ptr: *mut c_void, size: usize, byte: u8) -> bool {
    // SAFETY: the caller's block holds `size` bytes.
    unsafe { core::slice::from_raw_parts(ptr.cast::<u8>(), size) }
        .iter()
        .all(|&b| b == byte)
}

/// One thread's turn at every entry point, several times over so that a magazine runs out and
/// fills. Blocks are held for a while, so that frees miss too.
fn every_entry() {
    for _ in 0..4 {
        let mut held = Vec::new();
        for size in sizes() {
            // SAFETY: each block is used within its size and freed once, by the call that matches.
            unsafe {
                let p = c::malloc(size);
                assert!(!p.is_null());
                assert!(c::malloc_usable_size(p) >= size);
                if size >= 16 {
                    assert_eq!(p.addr() % 16, 0, "malloc({size})");
                }
                p.cast::<u8>().write_bytes(0xa5, size);
                held.push((p, size));
                let q = c::malloc(size);
                q.cast::<u8>().write_bytes(0x5a, size);
                c::free(q);
                // A block just written comes back zero.
                let z = c::calloc(size, 1);
                assert!(!z.is_null());
                assert!(filled(z, size, 0), "calloc({size}, 1)");
                z.cast::<u8>().write_bytes(0x77, size);
                c::free_sized(z, size);
                let up = c::realloc(c::malloc(size), 2 * size);
                assert!(!up.is_null());
                up.cast::<u8>().write_bytes(0x33, 2 * size);
                let down = c::realloc(up, size);
                assert!(filled(down, size, 0x33), "realloc to {size}");
                c::free(down);
            }
        }
        for (p, size) in held {
            assert!(filled(p, size, 0xa5), "a block of {size} changed");
            // SAFETY: allocated above, freed once.
            unsafe { c::free(p) };
        }
    }
}

/// A block freed on another thread, as a program hands work over.
struct Sent(*mut c_void);
// SAFETY: a block belongs to whichever thread has it.
unsafe impl Send for Sent {}

fn main() {
    // SAFETY: a call with no precondition, which returns a static string.
    let version = unsafe { CStr::from_ptr(c::cralloc_version()) };
    assert!(!version.is_empty());
    every_entry();
    // On one CPU a magazine is a stack, so a freed block comes straight back. Only most of the time,
    // since the thread may move.
    let mut same = 0;
    for _ in 0..1000 {
        // SAFETY: each block is freed once.
        unsafe {
            let p = c::malloc(64);
            c::free(p);
            let q = c::malloc(64);
            same += usize::from(p == q);
            c::free_sized(q, 64);
        }
    }
    assert!(same > 900, "{same}");
    // Threads: the standard library's own allocations go through the exports as well.
    let (send, receive) = mpsc::channel::<Sent>();
    let makers: Vec<_> = (0..4)
        .map(|_| {
            let send = send.clone();
            std::thread::spawn(move || {
                every_entry();
                for size in sizes().cycle().take(20_000) {
                    // SAFETY: a valid call; the receiver frees the block.
                    let p = unsafe { c::malloc(size.min(4096)) };
                    send.send(Sent(p)).unwrap();
                }
            })
        })
        .collect();
    drop(send);
    let freer = std::thread::spawn(move || {
        let mut count = 0;
        for block in receive {
            // SAFETY: allocated by a maker and given up to this thread.
            unsafe { c::free(block.0) };
            count += 1;
        }
        count
    });
    for maker in makers {
        maker.join().unwrap();
    }
    println!("ok {} blocks crossed threads", freer.join().unwrap());
}
