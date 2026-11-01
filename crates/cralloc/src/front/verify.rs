//! Kani proofs of the twins: each routine does what its specification says on every state of a
//! small slab, for every class word, marker and fill level, and an abort before any step leaves
//! the visible state as it was.
//!
//! The slab array has one slab and the magazine has four slots (two objects), the smallest the
//! slot tests allow; the routines' arithmetic does not depend on the size beyond the mask,
//! which `classes::verify` covers for every size. Pop, push and the stack routines take several
//! seconds each, so they are in `verify_slow.rs`.

use super::*;
use crate::classes::{Class, MARKER, MARKER_SHIFT, SLOT_BYTES};
use crate::os::{Aborted, Machine, Step};
use core::cell::Cell;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};

#[repr(C, align(32))]
pub(super) struct Magazine(pub(super) [AtomicPtr<u8>; 4]);

pub(super) fn class() -> SmallClass {
    Class::LARGEST_SMALL.small().unwrap()
}

pub(super) fn object() -> NonNull<u8> {
    let address: usize = kani::any();
    kani::assume(address != 0);
    NonNull::new(ptr::without_provenance_mut(address)).unwrap()
}

pub(super) fn any_marker() -> *mut u8 {
    let generation: usize = kani::any();
    kani::assume(generation < 1 << (usize::BITS - MARKER_SHIFT));
    ptr::without_provenance_mut(generation << MARKER_SHIFT | MARKER)
}

/// A machine on CPU 0 that, if `abort_at` is in range, aborts once before that step.
pub(super) struct OneAbort {
    steps: Cell<usize>,
    abort_at: usize,
}

impl Machine for OneAbort {
    fn cpu_id(&self) -> i32 {
        0
    }

    fn step(&self, _: Step) -> Result<(), Aborted> {
        let step = self.steps.get();
        self.steps.set(step + 1);
        if step == self.abort_at {
            Err(Aborted)
        } else {
            Ok(())
        }
    }
}

pub(super) fn machine() -> OneAbort {
    OneAbort {
        steps: Cell::new(0),
        abort_at: kani::any(),
    }
}

pub(super) fn slab0(slabs: &Slabs) -> &Slab {
    slabs.slab(slabs.cpu(0).unwrap())
}

pub(super) fn slot_address(magazine: &Magazine, slot: usize) -> *mut u8 {
    ptr::from_ref(&magazine.0[0])
        .cast_mut()
        .cast::<u8>()
        .wrapping_add(slot * SLOT_BYTES)
}

/// Store-if-equal stores exactly when the field holds the expected address.
#[kani::proof]
#[kani::unwind(5)]
fn store_if_equal_follows_its_specification() {
    let slabs = Slabs::<[Slab; 1]>::new();
    let field = Field::hot(class());
    let current: usize = kani::any();
    let expected: usize = kani::any();
    let new: usize = kani::any();
    let word = slab0(&slabs).field(field);
    word.store(ptr::without_provenance_mut(current), Relaxed);
    let machine = machine();
    // SAFETY: the harness runs no routine after this one, so nothing dereferences `new`;
    // its arbitrary value only exercises the comparison.
    let outcome = unsafe {
        Twin::new(&slabs, &machine).store_if_equal(
            field,
            ptr::without_provenance_mut(expected),
            ptr::without_provenance_mut(new),
        )
    };
    if current == expected {
        assert!(outcome == StoreIfEqual::Stored && word.load(Relaxed).addr() == new);
    } else {
        assert!(outcome == StoreIfEqual::Mismatch(ptr::without_provenance_mut(current)));
        assert!(word.load(Relaxed).addr() == current);
    }
}

/// A CPU number out of range, negative ones included, reaches no slab.
#[kani::proof]
fn no_valid_cpu_reaches_no_slab() {
    let slabs = Slabs::<[Slab; 2]>::new();
    let cpu_id: i32 = kani::any();
    let slabs: &Slabs = &slabs;
    match slabs.cpu(cpu_id) {
        Some(cpu) => assert!(cpu_id >= 0 && (cpu.index()) < 2 && cpu.get() as i32 == cpu_id),
        None => assert!(!(0..2).contains(&cpu_id)),
    }
}
