//! Floor allocators: the cheapest code that can serve the workloads, written in plain Rust, to
//! show what rustc generates and what it costs. Sixteen-byte size classes up to 1 KiB, one
//! intrusive free list per class, 64 KiB refills that are never returned.
//!
//! The two variants differ only in where the list heads live:
//!   floor_tls   in a `thread_local!` array, which rustc addresses as `fs:[constant]`
//!   floor_heap  in a heap struct behind a thread-local pointer, addressed as `[reg + offset]`
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr::null_mut;

const CLASSES: usize = 65;

#[cfg(feature = "floor_tls")]
mod imp {
    use super::*;

    thread_local! {
        static HEADS: [Cell<*mut u8>; CLASSES] = const { [const { Cell::new(null_mut()) }; CLASSES] };
    }

    #[inline(always)]
    pub unsafe fn head(class: usize) -> *mut u8 {
        HEADS.with(|h| h.get_unchecked(class).get())
    }

    #[inline(always)]
    pub unsafe fn set_head(class: usize, p: *mut u8) {
        HEADS.with(|h| h.get_unchecked(class).set(p))
    }
}

#[cfg(feature = "floor_heap")]
mod imp {
    use super::*;

    pub struct Heap {
        heads: [Cell<*mut u8>; CLASSES],
    }

    // A shared empty heap stands in until the thread has its own. Popping from it always misses,
    // and the miss path installs the real one. Nothing is freed before the first allocation.
    struct Empty(Heap);
    unsafe impl Sync for Empty {}
    static EMPTY: Empty = Empty(Heap { heads: [const { Cell::new(null_mut()) }; CLASSES] });

    thread_local! {
        static HEAP: Cell<*const Heap> = const { Cell::new(&EMPTY.0 as *const Heap) };
    }

    #[inline(always)]
    pub unsafe fn head(class: usize) -> *mut u8 {
        (*HEAP.get()).heads.get_unchecked(class).get()
    }

    #[inline(always)]
    pub unsafe fn set_head(class: usize, p: *mut u8) {
        let mut h = HEAP.get();
        if std::ptr::eq(h, &EMPTY.0) {
            h = System.alloc_zeroed(Layout::new::<Heap>()) as *const Heap;
            HEAP.set(h);
        }
        (*h).heads.get_unchecked(class).set(p)
    }
}

use imp::{head, set_head};

#[cold]
#[inline(never)]
unsafe fn refill(class: usize) -> *mut u8 {
    let size = class * 16;
    let chunk = System.alloc(Layout::from_size_align_unchecked(65536, 4096));
    let n = 65536 / size;
    for i in 1..n - 1 {
        (chunk.add(i * size) as *mut *mut u8).write(chunk.add((i + 1) * size));
    }
    (chunk.add((n - 1) * size) as *mut *mut u8).write(null_mut());
    set_head(class, chunk.add(size));
    chunk
}

pub struct Floor;

unsafe impl GlobalAlloc for Floor {
    #[inline(always)]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.size() <= 1024 && l.align() <= 16 {
            let class = (l.size() + 15) >> 4;
            let p = head(class);
            if !p.is_null() {
                set_head(class, (p as *mut *mut u8).read());
                p
            } else {
                refill(class)
            }
        } else {
            System.alloc(l)
        }
    }

    #[inline(always)]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if l.size() <= 1024 && l.align() <= 16 {
            let class = (l.size() + 15) >> 4;
            (p as *mut *mut u8).write(head(class));
            set_head(class, p);
        } else {
            System.dealloc(p, l)
        }
    }
}
