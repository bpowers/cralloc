//! Raw objects for the cross-thread workloads: allocated and freed through the global
//! allocator with their real `Layout`, as a `Box<[u8]>` would be, and carrying their own size
//! and a tag so the receiving thread can free them correctly and tell where they came from.
use std::alloc::{Layout, alloc, dealloc};
use std::cell::Cell;

/// A pointer to an object made by [`make`]. Sent between threads by value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Obj(pub *mut u8);

// SAFETY: an Obj is owned by exactly one thread at a time; the workloads hand it over
// through queues with release/acquire ordering.
unsafe impl Send for Obj {}

impl Obj {
    pub const NULL: Obj = Obj(std::ptr::null_mut());
}

/// Allocates `size` bytes (at least 16) and writes the size and `tag` into the first two
/// words, as a message header would be written.
#[inline]
pub fn make(size: usize, tag: u64) -> Obj {
    debug_assert!(size >= 16);
    // SAFETY: size is non-zero and the alignment is a power of two.
    unsafe {
        let p = alloc(Layout::from_size_align_unchecked(size, 8));
        if p.is_null() {
            std::alloc::handle_alloc_error(Layout::from_size_align_unchecked(size, 8));
        }
        let w = p.cast::<u64>();
        w.write(size as u64);
        w.add(1).write(tag);
        Obj(p)
    }
}

/// Reads the header, as a receiver would, checks it, and frees the object. `from` is the tag
/// the sender wrote, or `None` when any sender may have made it.
///
/// A header that is wrong means the allocator handed out memory that something else was
/// using; the process aborts rather than freeing with a wrong layout.
///
/// # Safety
/// `o` came from [`make`] and has not been freed.
#[inline]
pub unsafe fn consume(o: Obj, from: Option<u64>) {
    unsafe {
        let w = o.0.cast::<u64>();
        let size = w.read() as usize;
        let tag = w.add(1).read();
        if !(16..=MAX_SIZE).contains(&size) || from.is_some_and(|f| f != tag) {
            corrupt(o, size, tag, from);
        }
        dealloc(o.0, Layout::from_size_align_unchecked(size, 8));
    }
}

/// Larger than any object a workload makes, and than any size a stray pointer or a free-list
/// link read as a size is likely to be.
const MAX_SIZE: usize = 1 << 32;

#[cold]
#[inline(never)]
fn corrupt(o: Obj, size: usize, tag: u64, from: Option<u64>) -> ! {
    eprintln!("corrupt object at {:p}: header says size {size} tag {tag}, expected tag {from:?}", o.0);
    std::process::abort()
}

thread_local! {
    /// The index of the worker thread running this code, for counting frees that happen on
    /// a different thread from the allocation. `u32::MAX` outside worker threads.
    static WORKER: Cell<u32> = const { Cell::new(u32::MAX) };
}

pub fn set_worker(i: usize) {
    WORKER.with(|w| w.set(i as u32));
}

pub fn worker() -> u32 {
    WORKER.with(|w| w.get())
}
