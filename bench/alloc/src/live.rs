//! The bytes a program holds live, counted by the global allocator (feature `live-bytes`), so
//! that an allocator's resident memory can be put over what the program asked for. A
//! measurement option on top of asmalloc, never a build that is compared: it adds an atomic
//! add to every call. The program's live bytes do not depend on the allocator, so one run of
//! this build gives the denominator for every allocator's runs of the same job.
//!
//! The count is a table of 64 counters, one cache line each, that threads add to by a hash of
//! their stack address, read from outside through `/proc/PID/mem` by its symbol,
//! `BENCH_LIVE_BYTES`: the sum of the 64 `i64`s at a 64-byte stride is the bytes allocated and
//! not yet freed, by the sizes the program asked for.
use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicI64, Ordering::Relaxed};

#[repr(align(64))]
pub struct Line(AtomicI64);

#[unsafe(no_mangle)]
#[used]
pub static BENCH_LIVE_BYTES: [Line; 64] = [const { Line(AtomicI64::new(0)) }; 64];

#[inline]
fn add(n: i64) {
    let here = 0u8;
    // Threads' stacks are megabytes apart; the bits above 20 tell them apart cheaply.
    let slot = ((&raw const here as usize) >> 20) % 64;
    BENCH_LIVE_BYTES[slot].0.fetch_add(n, Relaxed);
}

pub struct Counting;

// SAFETY: forwards every call to asmalloc unchanged; the counting touches only the table.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { asmalloc::Asmalloc.alloc(l) };
        if !p.is_null() {
            add(l.size() as i64);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        add(-(l.size() as i64));
        unsafe { asmalloc::Asmalloc.dealloc(p, l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { asmalloc::Asmalloc.alloc_zeroed(l) };
        if !p.is_null() {
            add(l.size() as i64);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { asmalloc::Asmalloc.realloc(p, l, new) };
        if !q.is_null() {
            add(new as i64 - l.size() as i64);
        }
        q
    }
}
