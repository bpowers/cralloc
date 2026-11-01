//! Tables of shared words that Miri can check at a usable speed.
//!
//! The back end keeps several tables with an entry per region (`MAX_REGIONS`, 4096 of them):
//! tags, run sizes, headers, frontiers, tails. As plain arrays of atomics they cost nothing
//! natively, but Miri retags a shared reference by walking every interior-mutable range of its
//! pointee, and a `&back::State` covered tens of thousands of them: each retag cost Miri about
//! 100 ms, and the tests of the back end and the heap ran for hours (`scripts/miri`). An array
//! held in one `UnsafeCell` is one such range, so the retag costs what any small one does.
//!
//! [`Cells`] is that array. It hands out a shared reference to one entry and nothing else, so it
//! is what an array of atomics is to every reader, and the code it compiles to is the same. Do
//! not replace it by the plain array it wraps: Miri, which checks the paths that hand out
//! memory for undefined behaviour, then cannot finish.

use core::cell::UnsafeCell;

#[cfg(kani)]
mod verify;

/// `N` entries of `T`, read through shared references only. `N` is a power of two, and an
/// index is reduced modulo `N`, so a lookup has no bounds check and cannot panic: callers pass
/// indices below `N` (a `RegionId`, say) and the reduction changes nothing for them.
#[repr(transparent)]
pub(crate) struct Cells<T, const N: usize>(UnsafeCell<[T; N]>);

// SAFETY: CELLS-01 in docs/safety/cralloc/cells/mod-safety.md. A `&Cells` gives out only `&T`,
// as a `&[T; N]` does, so sharing it across threads is sound exactly when sharing `&T` is.
unsafe impl<T: Sync, const N: usize> Sync for Cells<T, N> {}

impl<T, const N: usize> Cells<T, N> {
    const POWER_OF_TWO: () = assert!(N.is_power_of_two(), "a table's length is a power of two");

    pub(crate) const fn new(entries: [T; N]) -> Cells<T, N> {
        let () = Self::POWER_OF_TWO;
        Cells(UnsafeCell::new(entries))
    }

    /// The entry at `index` modulo `N`.
    #[inline(always)]
    pub(crate) fn get(&self, index: usize) -> &T {
        let () = Self::POWER_OF_TWO;
        // SAFETY: Kani: `cells::verify::every_index_names_an_entry`. `index & (N - 1)` is below
        // `N`, so the place is an entry of the array the cell holds, which lives as long as
        // `self`; nothing makes a `&mut` to the array or to an entry (no method does, and the
        // field is private), so a shared reference to an entry aliases only other shared ones.
        // The array is indexed as a place, not through a reference to it, which Miri would
        // retag whole; and as an array, so that the code is what indexing the plain array was.
        unsafe { &(*self.0.get())[index & (N - 1)] }
    }
}
