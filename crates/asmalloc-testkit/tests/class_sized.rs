//! An `Allocator` shaped like asmalloc's must pass: every block is a whole 32-byte class, the
//! slice returned is the class, and a resize within the class happens in place. `grow_zeroed`
//! in place zeroes only past the old class, which the trait allows, so the model must not
//! demand zeros from the size passed in.
#![cfg(feature = "allocator-api")]

mod common;

use core::alloc::{AllocError, Allocator, Layout};
use core::ptr::NonNull;
use std::alloc::System;

use asmalloc_testkit::subject::ViaAllocator;
use asmalloc_testkit::{Config, Handoff, Profile, check};
use common::{balanced, fit, ops, ops_for, profiles};

struct ClassSized;

const CLASS: usize = 32;

fn class(l: Layout) -> Layout {
    let size = l.size().max(1).next_multiple_of(CLASS);
    Layout::from_size_align(size, l.align().max(CLASS)).unwrap()
}

impl ClassSized {
    /// # Safety
    /// `p` is live with a layout fitting `old`.
    unsafe fn resize(
        &self,
        p: NonNull<u8>,
        old: Layout,
        new: Layout,
        zero: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        let nc = class(new);
        if class(old) == nc {
            return Ok(NonNull::slice_from_raw_parts(p, nc.size()));
        }
        let q = if zero {
            self.allocate_zeroed(new)?
        } else {
            self.allocate(new)?
        };
        let keep = old.size().min(new.size());
        // SAFETY: both blocks hold `keep` bytes and are distinct; `p` is freed after.
        unsafe {
            core::ptr::copy_nonoverlapping(p.as_ptr(), q.cast::<u8>().as_ptr(), keep);
            self.deallocate(p, old);
        }
        Ok(q)
    }
}

// SAFETY: blocks come from `System` with their class layout and go back with it; every layout
// that fits a block maps to its class, since the trait allows only sizes up to the slice length.
unsafe impl Allocator for ClassSized {
    fn allocate(&self, l: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let c = class(l);
        let p = System.allocate(c)?;
        Ok(NonNull::slice_from_raw_parts(p.cast(), c.size()))
    }

    unsafe fn deallocate(&self, p: NonNull<u8>, l: Layout) {
        // SAFETY: allocated by `allocate` with `class(l)`.
        unsafe { System.deallocate(p, class(l)) }
    }

    unsafe fn grow(
        &self,
        p: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: the caller's contract for `grow`.
        unsafe { self.resize(p, old, new, false) }
    }

    unsafe fn grow_zeroed(
        &self,
        p: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: the caller's contract for `grow_zeroed`; in place, the old class's bytes are
        // kept and there are none past it to zero.
        unsafe { self.resize(p, old, new, true) }
    }

    unsafe fn shrink(
        &self,
        p: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: the caller's contract for `shrink`.
        unsafe { self.resize(p, old, new, false) }
    }
}

#[test]
fn class_sized_slices_and_in_place_growth_are_accepted() {
    let s = ViaAllocator(ClassSized);
    for p in profiles() {
        balanced(&check(&s, &Config::single(p, 6, ops_for(&p))), p.name);
    }
    let cfg = Config::threads(fit(Profile::MIXED), 4, Handoff::AllToAll, 6, ops(3000, 80));
    balanced(&check(&s, &cfg), "threads");
}
