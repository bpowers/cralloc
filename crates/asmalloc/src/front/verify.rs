//! Kani proofs of the twins: each routine does what its specification says on every state of a
//! small slab, for every class word, marker and fill level, and an abort before any step leaves
//! the visible state as it was.
//!
//! The slab array has one slab and the magazine has four slots (two objects), the smallest the
//! slot tests allow; the routines' arithmetic does not depend on the size beyond the mask,
//! which `classes::verify` covers for every size.

use super::*;
use crate::classes::{Class, MARKER, MARKER_SHIFT, SLOT_BYTES, is_marker};
use crate::os::{Aborted, Machine, Step};
use core::cell::Cell;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};

#[repr(C, align(32))]
struct Magazine([AtomicPtr<u8>; 4]);

fn class() -> SmallClass {
    Class::LARGEST_SMALL.small().unwrap()
}

fn object() -> NonNull<u8> {
    let address: usize = kani::any();
    kani::assume(address != 0);
    NonNull::new(ptr::without_provenance_mut(address)).unwrap()
}

fn any_marker() -> *mut u8 {
    let generation: usize = kani::any();
    kani::assume(generation < 1 << (usize::BITS - MARKER_SHIFT));
    ptr::without_provenance_mut(generation << MARKER_SHIFT | MARKER)
}

/// A machine on CPU 0 that, if `abort_at` is in range, aborts once before that step.
struct OneAbort {
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

fn machine() -> OneAbort {
    OneAbort {
        steps: Cell::new(0),
        abort_at: kani::any(),
    }
}

fn slab0(slabs: &Slabs) -> &Slab {
    slabs.slab(slabs.cpu(0).unwrap())
}

fn slot_address(magazine: &Magazine, slot: usize) -> *mut u8 {
    ptr::from_ref(&magazine.0[0])
        .cast_mut()
        .cast::<u8>()
        .wrapping_add(slot * SLOT_BYTES)
}

/// Pop and push on a hot word that is zero, a marker, or any slot of a magazine: each takes or
/// adds exactly the top object, or misses and changes nothing, even with an abort before any
/// step.
#[kani::proof]
#[kani::unwind(6)]
fn pop_and_push_follow_their_specifications() {
    let slabs = Slabs::<[Slab; 1]>::new();
    let magazine = Magazine([const { AtomicPtr::new(ptr::null_mut()) }; 4]);
    let objects = [object(), object()];
    magazine.0[1].store(objects[0].as_ptr(), Relaxed);
    magazine.0[2].store(objects[1].as_ptr(), Relaxed);
    let slot: usize = kani::any();
    kani::assume(slot < 4);
    let word = match kani::any::<u8>() {
        0 => ptr::null_mut(),
        1 => any_marker(),
        _ => slot_address(&magazine, slot),
    };
    let hot = slab0(&slabs).field(Field::hot(class()));
    hot.store(word, Relaxed);
    let machine = machine();
    let twin = Twin::new(&slabs, &machine);

    if kani::any() {
        let popped = twin.pop(class());
        if word.addr() & class().magazine().mask() == 0 {
            assert!(popped.is_none() && hot.load(Relaxed) == word);
        } else {
            assert!(slot == 2 || slot == 3);
            assert!(popped == Some(objects[slot - 2]));
            assert!(hot.load(Relaxed) == word.wrapping_sub(SLOT_BYTES));
        }
    } else {
        let pushed = object();
        let ok = twin.push(class(), pushed);
        if word.is_null() || is_marker(word.addr()) || slot == 0 || slot == 3 {
            assert!(!ok && hot.load(Relaxed) == word);
        } else {
            assert!(ok);
            assert!(magazine.0[slot].load(Relaxed) == pushed.as_ptr());
            assert!(hot.load(Relaxed) == word.wrapping_add(SLOT_BYTES));
        }
    }
    // Nothing else in the magazine changed: pop writes no slot, and push writes only the slot
    // its word named.
    let written = if hot.load(Relaxed) == word.wrapping_add(SLOT_BYTES) {
        slot
    } else {
        4
    };
    for (i, expected) in [(1, objects[0]), (2, objects[1])] {
        if i != written {
            assert!(magazine.0[i].load(Relaxed) == expected.as_ptr());
        }
    }
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

/// The stacks: a push links the old head into the magazine and makes it the head unless the
/// head is a marker; a pop returns the head and makes its link the head unless the head is
/// zero or a marker.
#[kani::proof]
#[kani::unwind(5)]
fn the_stack_routines_follow_their_specifications() {
    let slabs = Slabs::<[Slab; 1]>::new();
    let below = Magazine([const { AtomicPtr::new(ptr::null_mut()) }; 4]);
    let pushed = Magazine([const { AtomicPtr::new(ptr::null_mut()) }; 4]);
    let link = match kani::any::<u8>() {
        0 => ptr::null_mut(),
        _ => slot_address(&pushed, 0),
    };
    below.0[0].store(link, Relaxed);
    let head = match kani::any::<u8>() {
        0 => ptr::null_mut(),
        1 => any_marker(),
        _ => slot_address(&below, 0),
    };
    let field = Field::spare(class(), Spare::Empty);
    let word = slab0(&slabs).field(field);
    word.store(head, Relaxed);
    let machine = machine();
    let twin = Twin::new(&slabs, &machine);
    if kani::any() {
        let magazine = NonNull::new(slot_address(&pushed, 0)).unwrap();
        // SAFETY: `pushed` is a magazine of the harness's own, alive for the whole harness.
        let outcome = unsafe { twin.stack_push(field, magazine) };
        if is_marker(head.addr()) {
            assert!(outcome == StackPush::Marker && word.load(Relaxed) == head);
        } else {
            assert!(outcome == StackPush::Pushed);
            assert!(word.load(Relaxed) == magazine.as_ptr());
            assert!(pushed.0[0].load(Relaxed) == head);
        }
    } else {
        let outcome = twin.stack_pop(field);
        if head.is_null() || is_marker(head.addr()) {
            assert!(outcome == StackPop::Empty && word.load(Relaxed) == head);
        } else {
            assert!(outcome == StackPop::Popped(NonNull::new(head).unwrap()));
            assert!(word.load(Relaxed) == link);
        }
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
