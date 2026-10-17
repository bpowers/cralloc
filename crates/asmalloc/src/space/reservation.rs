//! A live reservation: a [`Plan`] at a base address, and the OS calls on parts of it.
//!
//! [`Space`] turns plan offsets into pointers that carry the reservation's provenance, and wraps
//! the OS's memory calls with the range check their contracts require, so code above it calls
//! `commit` and `advise_huge` without `unsafe`. [`SpaceCell`] publishes one `Space` for a whole
//! process in a single word, the way start-up needs: two threads may race to reserve, one
//! compare-and-swap picks the winner, and the loser unmaps its own.

use super::{Geometry, Location, Plan, RegionId};
use crate::classes::Class;
use crate::os::{HugePages, Os, OsError};
use core::fmt;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering};

/// A live reservation obtained from an `O`.
///
/// Invariant: the `plan.size()` bytes at `base` are one reservation that some value of `O`
/// returned and that has not been unmapped, and `base` is aligned to the region size. Only
/// [`Space::reserve`] and [`SpaceCell`] make one, and only the unsafe [`Space::unmap`] ends it.
pub struct Space<O: Os> {
    base: NonNull<u8>,
    plan: Plan,
    os: PhantomData<fn() -> O>,
}

impl<O: Os> Clone for Space<O> {
    fn clone(&self) -> Space<O> {
        *self
    }
}

impl<O: Os> Copy for Space<O> {}

impl<O: Os> PartialEq for Space<O> {
    fn eq(&self, other: &Space<O>) -> bool {
        self.base == other.base && self.plan == other.plan
    }
}

impl<O: Os> Eq for Space<O> {}

impl<O: Os> fmt::Debug for Space<O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Space")
            .field("base", &self.base)
            .field("objects", &self.plan.objects())
            .finish()
    }
}

impl<O: Os> Space<O> {
    /// Reserves the largest plan the OS allows: the geometry's maximum, else half as many object
    /// regions, and so on down to one. The design asks for "as much as the process is allowed";
    /// halving finds that within a dozen calls.
    pub fn reserve(os: &O) -> Result<Space<O>, OsError> {
        let geometry = O::GEOMETRY;
        let mut objects = geometry.max_objects();
        loop {
            let Some(plan) = geometry.plan(objects) else {
                return Err(OsError::EINVAL);
            };
            match os.reserve(plan.size(), geometry.region_size()) {
                Ok(base) => {
                    debug_assert!(base.addr().get() & (geometry.region_size() - 1) == 0);
                    return Ok(Space {
                        base,
                        plan,
                        os: PhantomData,
                    });
                }
                Err(error) if objects == 1 => return Err(error),
                Err(_) => objects /= 2,
            }
        }
    }

    /// The first byte of the reservation, which is the first byte of region 0.
    #[inline(always)]
    pub fn base(&self) -> NonNull<u8> {
        self.base
    }

    /// The layout.
    #[inline(always)]
    pub fn plan(&self) -> Plan {
        self.plan
    }

    #[inline(always)]
    fn at(&self, offset: usize) -> NonNull<u8> {
        // Reservations lie far below the top of the address space, so the sum never saturates;
        // saturating keeps the arithmetic total without a panic.
        self.base.map_addr(|base| base.saturating_add(offset))
    }

    /// Where a pointer falls, if it is inside an object region.
    #[inline(always)]
    pub fn locate(&self, ptr: *const u8) -> Option<Location> {
        self.plan
            .locate(ptr.addr().wrapping_sub(self.base.addr().get()))
    }

    /// The first byte of an object region.
    #[inline(always)]
    pub fn region(&self, region: RegionId) -> NonNull<u8> {
        self.at(self.plan.region_offset(region))
    }

    /// Slot `index` of `class` in `region`.
    #[inline(always)]
    pub fn slot(&self, region: RegionId, class: Class, index: u32) -> NonNull<u8> {
        self.at(self.plan.slot_offset(region, class, index))
    }

    /// A region's metadata block, [`super::Geometry::metadata_size`] bytes.
    #[inline(always)]
    pub fn metadata(&self, region: RegionId) -> NonNull<u8> {
        self.at(self.plan.metadata_offset(region))
    }

    /// The magazine area, [`super::Geometry::magazine_area_size`] bytes.
    #[inline(always)]
    pub fn magazines(&self) -> NonNull<u8> {
        self.at(self.plan.magazines_offset())
    }

    /// Whether `start..start + len` is a range the OS calls accept: inside this reservation,
    /// page-aligned, a whole number of pages and not empty.
    pub fn is_callable_range(&self, start: NonNull<u8>, len: usize) -> bool {
        let page = O::GEOMETRY.page_size();
        let offset = start.addr().get().wrapping_sub(self.base.addr().get());
        let size = self.plan.size();
        // The base is region-aligned, so an aligned offset is an aligned address.
        len != 0 && offset < size && len <= size - offset && (offset | len) & (page - 1) == 0
    }

    fn check(&self, os: &O, start: NonNull<u8>, len: usize) {
        if !self.is_callable_range(start, len) {
            os.abort("an OS call on a range outside the heap or not page-aligned");
        }
    }

    /// Makes a range of the reservation readable and writable. Aborts if the range is not one
    /// [`Space::is_callable_range`] accepts.
    pub fn commit(&self, os: &O, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(os, start, len);
        // SAFETY: `check` returned, so the range is a non-empty, page-aligned run of whole pages
        // inside this reservation, which is live by the type's invariant. `Os` makes every value
        // of `O` accept ranges reserved through any other.
        unsafe { os.commit(start, len) }
    }

    /// Advises for or against huge pages over a range of the reservation. Aborts if the range is
    /// not one [`Space::is_callable_range`] accepts.
    pub fn advise_huge(
        &self,
        os: &O,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError> {
        self.check(os, start, len);
        // SAFETY: as in `commit`.
        unsafe { os.advise_huge(start, len, advice) }
    }

    /// Returns a range's memory to the OS; it reads as zero afterwards. Aborts if the range is
    /// not one [`Space::is_callable_range`] accepts.
    ///
    /// # Safety
    ///
    /// No thread reads or writes the range during the call, and nothing in it is needed.
    pub unsafe fn purge(&self, os: &O, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(os, start, len);
        // SAFETY: the range is valid as in `commit`; the caller promises nothing uses its
        // contents or touches it during the call.
        unsafe { os.purge(start, len) }
    }

    /// Unmaps the whole reservation.
    ///
    /// # Safety
    ///
    /// Nothing reads or writes the reservation, and no copy of this `Space` is used, after the
    /// call begins. In particular it must not be the space a [`SpaceCell`] has published.
    pub unsafe fn unmap(self, os: &O) -> Result<(), OsError> {
        // SAFETY: the whole reservation is a page-aligned, non-empty range of itself (the plan's
        // size is a whole number of pages), live by the invariant; the caller promises that
        // nothing uses it afterwards.
        unsafe { os.unmap(self.base, self.plan.size()) }
    }
}

/// One published [`Space`], or none yet, in one atomic word.
///
/// The word is the base address with the number of object regions in its low bits, which are
/// free because the base is region-aligned and the count is less than the region size. Zero
/// means nothing is published, so an all-zero cell is valid.
pub struct SpaceCell<O: Os> {
    word: AtomicPtr<u8>,
    os: PhantomData<fn() -> O>,
}

impl<O: Os> SpaceCell<O> {
    /// A cell with nothing published.
    pub const fn new() -> SpaceCell<O> {
        SpaceCell {
            word: AtomicPtr::new(ptr::null_mut()),
            os: PhantomData,
        }
    }

    /// The published space, if there is one. Acquire, so the caller sees everything the
    /// publishing thread wrote before publishing.
    #[inline(always)]
    pub fn get(&self) -> Option<Space<O>> {
        NonNull::new(self.word.load(Ordering::Acquire)).map(decode)
    }

    /// Publishes `space` unless a space is already published. Returns the published space:
    /// `Ok(space)` if this call published it, `Err(winner)` if another was there first, in which
    /// case the caller still owns `space` and should unmap it.
    pub fn publish(&self, space: Space<O>) -> Result<Space<O>, Space<O>> {
        let word = encode(space);
        loop {
            match self.word.compare_exchange(
                ptr::null_mut(),
                word,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(space),
                // A strong compare-and-swap from null fails only on a non-null word, so this
                // returns on the first pass.
                Err(current) => {
                    if let Some(current) = NonNull::new(current) {
                        return Err(decode(current));
                    }
                }
            }
        }
    }
}

impl<O: Os> Default for SpaceCell<O> {
    fn default() -> SpaceCell<O> {
        SpaceCell::new()
    }
}

/// The published word for a space: `base | objects`.
fn encode<O: Os>(space: Space<O>) -> *mut u8 {
    let objects = space.plan.objects();
    space.base.as_ptr().map_addr(|base| pack(base, objects))
}

/// The space a non-null published word stands for.
fn decode<O: Os>(word: NonNull<u8>) -> Space<O> {
    let geometry = O::GEOMETRY;
    let (_, objects) = unpack(geometry, word.addr().get());
    // Clearing the count leaves the base, which is not zero. `unwrap_or` keeps this total for a
    // word no `encode` made, which cannot be stored.
    let base = word.map_addr(|w| NonZeroUsize::new(unpack(geometry, w.get()).0).unwrap_or(w));
    Space {
        base,
        plan: Plan::new(geometry, objects as u16),
        os: PhantomData,
    }
}

/// A region-aligned base address and an object-region count as one word.
pub(super) const fn pack(base: usize, objects: usize) -> usize {
    base | objects
}

/// The base address and object-region count in a word `pack` made.
pub(super) const fn unpack(geometry: Geometry, word: usize) -> (usize, usize) {
    let mask = geometry.region_size() - 1;
    (word & !mask, word & mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::sim::{SimOs, Standard};
    use std::vec::Vec;

    type Sim = SimOs<Standard>;

    #[test]
    fn a_reservation_takes_the_largest_plan() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        assert_eq!(space.plan().objects(), Sim::GEOMETRY.max_objects());
        assert_eq!(space.base().addr().get() % Sim::GEOMETRY.region_size(), 0);
        // SAFETY: nothing else uses the reservation.
        unsafe { space.unmap(&os).unwrap() };
    }

    #[test]
    fn a_reservation_halves_until_the_os_agrees() {
        // Room for a quarter of the largest plan, so the first two attempts fail.
        let geometry = Sim::GEOMETRY;
        let quarter = geometry.plan(geometry.max_objects() / 4).unwrap();
        let os = Sim::with_capacity(quarter.size().next_multiple_of(geometry.region_size()));
        let space = Space::reserve(&os).unwrap();
        assert_eq!(space.plan().objects(), geometry.max_objects() / 4);
    }

    #[test]
    fn pointers_carry_the_plan_offsets() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let plan = space.plan();
        let region = plan.region(3).unwrap();
        let class = Class::of_size(48).unwrap();
        let base = space.base().addr().get();
        assert_eq!(
            space.region(region).addr().get(),
            base + plan.region_offset(region)
        );
        let slot = space.slot(region, class, 5);
        assert_eq!(slot.addr().get(), base + plan.slot_offset(region, class, 5));
        let location = space.locate(slot.as_ptr()).unwrap();
        assert_eq!((location.region, location.slot(class)), (region, 5));
        assert_eq!(
            space.metadata(region).addr().get(),
            base + plan.metadata_offset(region)
        );
        assert_eq!(
            space.magazines().addr().get(),
            base + plan.magazines_offset()
        );
        assert_eq!(space.locate(space.metadata(region).as_ptr()), None);
        assert_eq!(space.locate(ptr::null()), None);
    }

    #[test]
    fn commit_makes_memory_writable_and_purge_zeroes_it() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let region = space.region(space.plan().region(0).unwrap());
        let page = Sim::GEOMETRY.page_size();
        space.commit(&os, region, 2 * page).unwrap();
        assert!(os.is_writable(region, 2 * page));
        // SAFETY: the two pages were just committed and nothing else uses them.
        unsafe { region.as_ptr().add(page + 3).write(7) };
        // SAFETY: nothing uses the pages during or after the purge.
        unsafe { space.purge(&os, region, 2 * page).unwrap() };
        // SAFETY: still committed, and purged memory reads as zero.
        assert_eq!(unsafe { region.as_ptr().add(page + 3).read() }, 0);
        space
            .advise_huge(&os, region, page, HugePages::Prefer)
            .unwrap();
        assert_eq!(os.huge_pages(region), Some(HugePages::Prefer));
    }

    #[test]
    fn ranges_outside_the_reservation_are_refused() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let page = Sim::GEOMETRY.page_size();
        let base = space.base();
        let size = space.plan().size();
        assert!(space.is_callable_range(base, page));
        assert!(space.is_callable_range(base, size));
        let refused: Vec<(NonNull<u8>, usize)> = Vec::from([
            (base, 0),
            (base, page + 1),
            (base, size + page),
            (base.map_addr(|a| a.saturating_add(8)), page),
            (base.map_addr(|a| a.saturating_add(size)), page),
            (
                base.map_addr(|a| NonZeroUsize::new(a.get() - page).unwrap()),
                page,
            ),
        ]);
        for (start, len) in refused {
            assert!(!space.is_callable_range(start, len), "{start:?} {len}");
        }
    }

    #[test]
    #[should_panic(expected = "outside the heap")]
    fn a_bad_commit_aborts() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let _ = space.commit(&os, space.base(), 1);
    }

    #[test]
    fn the_cell_publishes_one_space() {
        let os = Sim::with_capacity(3 * Sim::GEOMETRY.max_reservation());
        let cell = SpaceCell::<Sim>::new();
        assert_eq!(cell.get(), None);
        let first = Space::reserve(&os).unwrap();
        let second = Space::reserve(&os).unwrap();
        assert_ne!(first, second);
        assert_eq!(cell.publish(first), Ok(first));
        assert_eq!(cell.get(), Some(first));
        assert_eq!(cell.publish(second), Err(first));
        // SAFETY: the losing space was never published and nothing uses it.
        unsafe { second.unmap(&os).unwrap() };
        assert_eq!(cell.get(), Some(first));
    }
}
