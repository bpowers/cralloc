//! An `Allocator` shaped like asmalloc's must pass: every block is a whole 32-byte class, the
//! slice returned is the class, and a resize within the class happens in place. `grow_zeroed`
//! in place must zero from the old layout's size to the class's end, the slack the caller was
//! given included (the trait's contract as stabilised in Rust 1.100), and the model must
//! catch one that zeroes only past the old class, as the trait's unstable wording allowed.
#![cfg(feature = "allocator-api")]

mod common;

use core::alloc::{AllocError, Allocator, Layout};
use core::ptr::NonNull;
use std::alloc::System;

use asmalloc_testkit::failure::FailureKind;
use asmalloc_testkit::subject::ViaAllocator;
use asmalloc_testkit::{Config, Handoff, Profile, check, run};
use common::{balanced, fit, ops, ops_for, profiles};

/// `keeps_slack`: a `grow_zeroed` in place leaves the old layout's slack as the caller wrote
/// it, which the stable contract forbids.
struct ClassSized {
    keeps_slack: bool,
}

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
            if zero && !self.keeps_slack {
                // SAFETY: the block is the caller's whole class, and `old` fits it, so
                // `old.size() <= nc.size()`.
                unsafe {
                    p.as_ptr()
                        .add(old.size())
                        .write_bytes(0, nc.size() - old.size())
                };
            }
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
// The one that keeps its slack breaks only the zeroing promise, which the model checks and
// nothing here relies on.
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
        // SAFETY: the caller's contract for `grow_zeroed`.
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
    let s = ViaAllocator(ClassSized { keeps_slack: false });
    for p in profiles() {
        balanced(&check(&s, &Config::single(p, 6, ops_for(&p))), p.name);
    }
    let cfg = Config::threads(fit(Profile::MIXED), 4, Handoff::AllToAll, 6, ops(3000, 80));
    balanced(&check(&s, &cfg), "threads");
}

#[test]
fn an_in_place_zeroed_grow_that_keeps_the_slack_is_caught() {
    let s = ViaAllocator(ClassSized { keeps_slack: true });
    let p = fit(Profile::MIXED);
    let caught = (0..6).find_map(|seed| run(&s, &Config::single(p, seed, ops_for(&p))).err());
    let f = caught.expect("an in-place grow_zeroed that keeps the slack is caught");
    assert_eq!(f.kind, FailureKind::NotZeroed, "{f}");
}
