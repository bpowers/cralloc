//! [`Real`]: the routines as the assembly generated from Lean (`generated/`).
//!
//! The generated functions take and return plain words; this module turns them into
//! [`Routines`] and nothing more. Pop and push are the inlined forms, which take the class and
//! its mask as constants: a `match` over the small classes picks the instance, so a class known
//! at compile time leaves one routine and nothing of the `match`, and a class known only at run
//! time costs a jump through a table. The out-of-line forms, which take the class in a register
//! and tail-call `pop_slow` and `push_slow` on a miss, are for callers that want one call per
//! operation (the C entry points); the store-if-equal and stack routines exist only out of
//! line.
//!
//! The generated code names its surroundings by path: `front::SLABS`, `front::MAG_MASKS`,
//! `front::__rseq_offset`, `front::pop_slow` and `front::push_slow` (see `super`). The ledger
//! for the unsafe blocks here is `docs/safety/asmalloc/front/real-safety.md`.
//!
//! Object pointers cross the assembly as integers. A pushed pointer's provenance is exposed, and
//! a popped address takes the exposed provenance back; the heap exposes its reservation's
//! provenance when it starts, so every object address has some to take.

use super::generated::x86_64::slab as outline;
use super::generated::x86_64_inline::slab as inline;
use super::routines::{Routines, StackPop, StackPush, StoreIfEqual};
use super::slab::{Cpu, Field, Slabs};
use super::{MASKS, SLABS};
use crate::classes::{SMALL_COUNT, SmallClass};
use core::ptr::{self, NonNull};

/// The generated routines, on the static [`SLABS`]. A zero-sized handle, private to the crate:
/// only the process heap may run them, since they write the one slab array it owns.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Real;

/// Instantiates the inlined pop and push for every small class, by class index. The list must
/// be `1..=SMALL_COUNT`; the constant below checks it.
macro_rules! by_class {
    ($($index:literal)*) => {
        const _: () = {
            let indexes = [$($index),*];
            assert!(indexes.len() == SMALL_COUNT);
            let mut i = 0;
            while i < indexes.len() {
                assert!(indexes[i] == i + 1);
                i += 1;
            }
        };

        /// The inlined pop for `class`: the object's address, or 0 on a miss.
        #[inline(always)]
        fn pop_inline(class: SmallClass) -> usize {
            match class.class().index() {
                $(
                    // SAFETY: REAL-01.
                    $index => unsafe { inline::slab_pop::<$index, { MASKS[$index] }>() },
                )*
                _ => 0,
            }
        }

        /// The inlined push for `class`: whether the object went in.
        #[inline(always)]
        fn push_inline(class: SmallClass, object: usize) -> bool {
            match class.class().index() {
                $(
                    // SAFETY: REAL-01.
                    $index => unsafe { inline::slab_push::<$index, { MASKS[$index] }>(object) },
                )*
                _ => false,
            }
        }
    };
}

by_class!(
    1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22
    23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44
);

impl Real {
    /// Pops an object of `class` with the out-of-line routine, which on a miss continues in the
    /// heap's slow path (`pop_slow`). Null if the heap has no memory.
    #[inline]
    pub(crate) fn pop_out_of_line(self, class: SmallClass) -> *mut u8 {
        // SAFETY: REAL-02.
        let address = unsafe { outline::slab_pop(class.class().index() as usize) };
        ptr::with_exposed_provenance_mut(address)
    }

    /// Pushes an object of `class` with the out-of-line routine, which on a miss continues in
    /// the heap's slow path (`push_slow`).
    ///
    /// # Safety
    ///
    /// `object` is a block of `class` that the heap handed out and the caller owns; it gives
    /// it up.
    #[inline]
    pub(crate) unsafe fn push_out_of_line(self, class: SmallClass, object: NonNull<u8>) {
        // SAFETY: REAL-02.
        unsafe {
            outline::slab_push(
                class.class().index() as usize,
                object.as_ptr().expose_provenance(),
            )
        }
    }
}

// SAFETY: REAL-03. Each method is one generated routine whose Lean contract is the method's
// specification, decoded by the `from_raw` functions the trait module provides.
unsafe impl Routines for Real {
    #[inline(always)]
    fn slabs(&self) -> &Slabs {
        &SLABS
    }

    #[inline]
    fn cpu(&self) -> Option<Cpu> {
        // SAFETY: REAL-01.
        let cpu_id = unsafe { inline::current_cpu() };
        self.slabs().cpu(cpu_id as i32)
    }

    #[inline(always)]
    fn pop(&self, class: SmallClass) -> Option<NonNull<u8>> {
        NonNull::new(ptr::with_exposed_provenance_mut(pop_inline(class)))
    }

    #[inline(always)]
    fn push(&self, class: SmallClass, object: NonNull<u8>) -> bool {
        push_inline(class, object.as_ptr().expose_provenance())
    }

    #[inline]
    unsafe fn store_if_equal(&self, field: Field, expected: *mut u8, new: *mut u8) -> StoreIfEqual {
        // SAFETY: REAL-02; the caller keeps the slab invariant with `new`.
        let read = unsafe { outline::slab_cas(field.offset(), expected.addr(), new.addr()) };
        StoreIfEqual::from_raw(ptr::without_provenance_mut(read), expected)
    }

    #[inline]
    unsafe fn stack_push(&self, field: Field, magazine: NonNull<u8>) -> StackPush {
        // SAFETY: REAL-02; the caller holds the magazine.
        let read = unsafe { outline::slab_stack_push(field.offset(), magazine.addr().get()) };
        StackPush::from_raw(read)
    }

    #[inline]
    fn stack_pop(&self, field: Field) -> StackPop {
        // SAFETY: REAL-02.
        let read = unsafe { outline::slab_stack_pop(field.offset()) };
        StackPop::from_raw(ptr::without_provenance_mut(read))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::Class;

    #[test]
    fn this_thread_reads_a_cpu_with_a_slab() {
        let cpu = Real
            .cpu()
            .expect("glibc registered rseq and the CPU has a slab");
        assert!(cpu.index() < Real.slabs().len());
    }

    #[test]
    fn the_selftest_routine_runs() {
        // SAFETY: the self-test has no safety requirements.
        let sum = unsafe { super::super::generated::x86_64::selftest::vg_selftest_add(2, 3) };
        assert_eq!(sum, 5);
    }

    #[test]
    fn every_class_has_an_inlined_instance() {
        // A class index outside the small classes misses without touching a slab.
        for class in SmallClass::all() {
            assert_eq!(class.class().index() as usize, class.ordinal() + 1);
        }
        assert!(Class::LARGEST_SMALL.small().is_some());
    }
}
