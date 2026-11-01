//! Kani proofs of the twins too slow for the merge gate: pop, push and the spare stacks, on the
//! small slab of `verify.rs`. Run on demand with `scripts/kani-all verify_slow`.

use super::verify::{Magazine, any_marker, class, machine, object, slab0, slot_address};
use super::*;
use crate::classes::{SLOT_BYTES, is_marker};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};

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
