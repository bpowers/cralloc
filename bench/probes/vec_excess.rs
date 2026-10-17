//! Does `Vec<T, A>` use the extra bytes an `Allocator` returns?
//!
//! `Allocator::allocate` returns a slice that may be longer than the request, and
//! `deallocate` accepts any size between the request and that length. This allocator rounds
//! every request up to a power of two of at least 64 bytes and reports the full length. If
//! `Vec` used the excess, `capacity()` would be 64 and no reallocation would happen at the
//! eleventh push; the sizes printed from `deallocate` show which size std passes back.
//!
//! The trait is stable from Rust 1.100. On 1.99 this file fails to compile with E0658, which
//! is itself the check that the trait is still gated there.
use std::alloc::{AllocError, Allocator, Layout, System};
use std::ptr::NonNull;

struct RoundUp;

fn rounded(l: Layout) -> Layout {
    Layout::from_size_align(l.size().next_power_of_two().max(64), l.align()).unwrap()
}

unsafe impl Allocator for RoundUp {
    fn allocate(&self, l: Layout) -> Result<NonNull<[u8]>, AllocError> {
        System.allocate(rounded(l))
    }
    unsafe fn deallocate(&self, p: NonNull<u8>, l: Layout) {
        eprintln!("dealloc size={} align={}", l.size(), l.align());
        // Both the requested and the returned size round to the same block, which is the
        // property a size-class allocator needs from the "fit" rule.
        unsafe { System.deallocate(p, rounded(l)) }
    }
}

fn main() {
    let mut v: Vec<u8, RoundUp> = Vec::with_capacity_in(10, RoundUp);
    println!("cap after with_capacity_in(10): {}", v.capacity());
    for i in 0..11 {
        v.push(i);
    }
    println!("cap after 11 pushes: {}", v.capacity());
    let b: Box<[u64; 3], RoundUp> = Box::new_in([1, 2, 3], RoundUp);
    drop(b);
}
