//! [`Real`]: the routines as the assembly generated from Lean (`generated/`).
//!
//! On aarch64 the routines are hand-written until Lean emits them (`provisional/`). The generated
//! functions take and return plain words; this module turns them into
//! [`Routines`] and nothing more. Pop and push are the inlined forms. They name no class: each
//! is given the address of its class's word in slab 0 and the class's mask, which a class known
//! at compile time folds to an address and a constant, and a class known only at run time
//! computes with an add and a table load. The store-if-equal and stack routines exist only out
//! of line.
//!
//! The generated code names its surroundings by path: `front::SLABS` and
//! `front::__rseq_offset` (see `super`). The ledger for the unsafe blocks here is
//! `docs/safety/cralloc/front/real-safety.md`.
//!
//! Object pointers cross the assembly as integers. A pushed pointer's provenance is exposed, and
//! a popped address takes the exposed provenance back; the heap exposes its reservation's
//! provenance when it starts, so every object address has some to take.

use super::SLABS;
use super::inlined::slab as inline;
use super::outlined::slab as outline;
use super::routines::{Routines, StackPop, StackPush, StoreIfEqual};
use super::slab::{Cpu, Field, Slabs};
use crate::classes::SmallClass;
use core::ptr::{self, NonNull};

/// The generated routines, on the static [`SLABS`]. A zero-sized handle, private to the crate:
/// only the process heap may run them, since they write the one slab array it owns.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Real;

// SAFETY: REAL-03. Each method is one generated routine whose Lean contract is the method's
// specification, decoded by the `from_raw` functions the trait module provides.
unsafe impl Routines for Real {
    #[inline(always)]
    fn slabs(&self) -> &Slabs {
        SLABS.slabs()
    }

    #[inline]
    fn cpu(&self) -> Option<Cpu> {
        // SAFETY: REAL-01.
        let cpu_id = unsafe { inline::current_cpu() };
        // On musl the area is the crate's own, and a thread that has never tried registers it
        // here, at its first slow path (`area`).
        #[cfg(target_env = "musl")]
        if cpu_id == super::area::UNREGISTERED {
            super::area::register();
            // SAFETY: REAL-01.
            return self.slabs().cpu(unsafe { inline::current_cpu() } as i32);
        }
        self.slabs().cpu(cpu_id as i32)
    }

    #[inline(always)]
    fn pop(&self, class: SmallClass) -> Option<NonNull<u8>> {
        // SAFETY: REAL-01.
        let address = unsafe { inline::slab_pop(SLABS.word(class), class.mask()) };
        NonNull::new(ptr::with_exposed_provenance_mut(address))
    }

    #[inline(always)]
    fn push(&self, class: SmallClass, object: NonNull<u8>) -> bool {
        let object = object.as_ptr().expose_provenance();
        // SAFETY: REAL-01.
        unsafe { inline::slab_push(SLABS.word(class), class.mask(), object) }
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
        let sum = unsafe { super::super::outlined::selftest::vg_selftest_add(2, 3) };
        assert_eq!(sum, 5);
    }

    #[test]
    fn every_class_word_is_in_slab_zero_of_the_routines_slabs() {
        let base = ptr::from_ref(Real.slabs()).addr();
        for class in SmallClass::all() {
            assert_eq!(SLABS.word(class), base + Field::hot(class).offset());
        }
    }
}
