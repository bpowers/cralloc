//! A live reservation: a [`Plan`] at a base address, and the OS calls on parts of it.
//!
//! [`Space`] turns plan offsets into pointers that carry the reservation's provenance, and wraps
//! the OS's memory calls with the range check their contracts require, so code above it calls
//! `commit` and `advise_huge` without `unsafe`. A `Space` borrows the OS value it came from, so
//! it cannot outlive the memory it describes.
//!
//! [`SpaceCell`] owns an OS value and publishes one `Space` over it in a single word, the way
//! start-up needs: two threads may race to reserve, one compare-and-swap picks the winner, and
//! the loser unmaps its own.

use super::{Geometry, Location, Plan, RegionId};
use crate::classes::Class;
use crate::os::{HugePages, Os, OsError};
use core::fmt;
use core::num::NonZeroUsize;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering};

/// A live reservation obtained from `os`.
///
/// Invariant: the `plan.size()` bytes at `base` are one reservation that `os` returned and that
/// has not been unmapped, and `base` is aligned to the region size. Only [`Space::reserve`] and
/// [`SpaceCell`] make one, and only the unsafe [`Space::unmap`] ends it. The borrow of `os`
/// keeps the OS value, and so the memory, alive for as long as the `Space` exists.
pub struct Space<'os, O: Os> {
    base: NonNull<u8>,
    plan: Plan,
    os: &'os O,
}

impl<O: Os> Clone for Space<'_, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O: Os> Copy for Space<'_, O> {}

impl<O: Os> PartialEq for Space<'_, O> {
    fn eq(&self, other: &Self) -> bool {
        self.base == other.base && self.plan == other.plan
    }
}

impl<O: Os> Eq for Space<'_, O> {}

impl<O: Os> fmt::Debug for Space<'_, O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Space")
            .field("base", &self.base)
            .field("objects", &self.plan.objects())
            .finish()
    }
}

impl<'os, O: Os> Space<'os, O> {
    /// Reserves the largest plan the OS allows: the geometry's maximum, else half as many object
    /// regions, and so on down to one. The design asks for "as much as the process is allowed";
    /// halving finds that within a dozen calls.
    pub fn reserve(os: &'os O) -> Result<Space<'os, O>, OsError> {
        let geometry = O::GEOMETRY;
        let mut objects = geometry.max_objects();
        loop {
            let Some(plan) = geometry.plan(objects) else {
                return Err(OsError::EINVAL);
            };
            match os.reserve(plan.size(), geometry.region_size()) {
                Ok(base) => {
                    debug_assert!(base.addr().get() & (geometry.region_size() - 1) == 0);
                    return Ok(Space { base, plan, os });
                }
                Err(error) if objects == 1 => return Err(error),
                Err(_) => objects /= 2,
            }
        }
    }

    /// The OS the reservation came from.
    #[inline(always)]
    pub fn os(&self) -> &'os O {
        self.os
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

    /// The first byte of an object region. `region` must be one of this plan's object regions,
    /// as [`Plan::region`] and [`Plan::locate`] make; otherwise the pointer is outside them.
    #[inline(always)]
    pub fn region(&self, region: RegionId) -> NonNull<u8> {
        debug_assert!(region.index() < self.plan.objects());
        self.at(self.plan.region_offset(region))
    }

    /// Slot `index` of `class` in `region`. The slot must fit in the region (see
    /// [`Plan::slot_at`] and [`Geometry::slots_per_region`]); otherwise the pointer is outside
    /// it.
    #[inline(always)]
    pub fn slot(&self, region: RegionId, class: Class, index: u32) -> NonNull<u8> {
        debug_assert!(region.index() < self.plan.objects());
        debug_assert!(
            class.slot_offset(index) + class.size() <= self.plan.geometry().region_size()
        );
        self.at(self.plan.slot_offset(region, class, index))
    }

    /// A region's metadata block, [`Geometry::metadata_size`] bytes. `region` must be one of
    /// this plan's object regions.
    #[inline(always)]
    pub fn metadata(&self, region: RegionId) -> NonNull<u8> {
        debug_assert!(region.index() < self.plan.objects());
        self.at(self.plan.metadata_offset(region))
    }

    /// The magazine area, [`Geometry::magazine_area_size`] bytes.
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

    fn check(&self, start: NonNull<u8>, len: usize) {
        if !self.is_callable_range(start, len) {
            self.os
                .abort("an OS call on a range outside the heap or not page-aligned");
        }
    }

    /// Makes a range of the reservation readable and writable. Aborts if the range is not one
    /// [`Space::is_callable_range`] accepts.
    pub fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: `check` returned, so the range is a non-empty, page-aligned run of whole pages
        // inside this reservation, which `self.os` made and which is live by the type's
        // invariant.
        unsafe { self.os.commit(start, len) }
    }

    /// Advises for or against huge pages over a range of the reservation. Aborts if the range is
    /// not one [`Space::is_callable_range`] accepts.
    pub fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: as in `commit`.
        unsafe { self.os.advise_huge(start, len, advice) }
    }

    /// Returns a range's memory to the OS; it reads as zero afterwards. Aborts if the range is
    /// not one [`Space::is_callable_range`] accepts.
    ///
    /// # Safety
    ///
    /// No thread reads or writes the range during the call, and nothing in it is needed.
    pub unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: the range is valid as in `commit`; the caller promises nothing uses its
        // contents or touches it during the call.
        unsafe { self.os.purge(start, len) }
    }

    /// Unmaps the whole reservation.
    ///
    /// # Safety
    ///
    /// Nothing reads or writes the reservation, and no copy of this `Space` is used, after the
    /// call begins. In particular it must not be a space a [`SpaceCell`] has published.
    pub unsafe fn unmap(self) -> Result<(), OsError> {
        // SAFETY: the whole reservation is a page-aligned, non-empty range of itself (the plan's
        // size is a whole number of pages), live by the invariant; the caller promises that
        // nothing uses it afterwards.
        unsafe { self.os.unmap(self.base, self.plan.size()) }
    }
}

/// An OS value and the one [`Space`] published over it, or none yet.
///
/// The space is one atomic word: the base address with the number of object regions in its low
/// bits, which are free because the base is region-aligned and the count is less than the region
/// size. Zero means nothing is published. Only this cell writes the word, and only with a
/// reservation of its own OS, which lives as long as the cell.
pub struct SpaceCell<O: Os> {
    os: O,
    word: AtomicPtr<u8>,
}

impl<O: Os> SpaceCell<O> {
    /// A cell over `os` with nothing published.
    pub const fn new(os: O) -> SpaceCell<O> {
        SpaceCell {
            os,
            word: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// The OS.
    #[inline(always)]
    pub fn os(&self) -> &O {
        &self.os
    }

    /// The published space, if there is one. Acquire, so the caller sees everything the
    /// publishing thread wrote before publishing.
    #[inline(always)]
    pub fn get(&self) -> Option<Space<'_, O>> {
        NonNull::new(self.word.load(Ordering::Acquire)).map(|word| self.decode(word))
    }

    /// The published space, reserving and publishing one first if there is none. If another
    /// thread publishes first, this one unmaps its own reservation and returns the winner.
    pub fn get_or_reserve(&self) -> Result<Space<'_, O>, OsError> {
        if let Some(space) = self.get() {
            return Ok(space);
        }
        let mine = Space::reserve(&self.os)?;
        let word = encode(mine);
        loop {
            match self.word.compare_exchange(
                ptr::null_mut(),
                word,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(mine),
                // A strong compare-and-swap from null fails only on a non-null word, so this
                // returns on the first pass.
                Err(current) => {
                    if let Some(current) = NonNull::new(current) {
                        // SAFETY: `mine` was never published, so no other thread has its
                        // address, and this thread drops it here. A failed unmap leaves the
                        // range reserved, which wastes address space and nothing else.
                        let _ = unsafe { mine.unmap() };
                        return Ok(self.decode(current));
                    }
                }
            }
        }
    }

    /// The space a non-null word stands for. Only `get_or_reserve` stores words, and only
    /// `encode` of a reservation of `self.os`.
    fn decode(&self, word: NonNull<u8>) -> Space<'_, O> {
        let geometry = O::GEOMETRY;
        let (_, objects) = unpack(geometry, word.addr().get());
        // Clearing the count leaves the base, which is not zero. `unwrap_or` keeps this total
        // for a word no `encode` made, which cannot be stored.
        let base = word.map_addr(|w| NonZeroUsize::new(unpack(geometry, w.get()).0).unwrap_or(w));
        Space {
            base,
            plan: Plan::new(geometry, objects as u16),
            os: &self.os,
        }
    }
}

/// The published word for a space: `base | objects`.
fn encode<O: Os>(space: Space<'_, O>) -> *mut u8 {
    let objects = space.plan.objects();
    space.base.as_ptr().map_addr(|base| pack(base, objects))
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
    use crate::os::sim::{Call, Faults, SimOs, Standard};
    use std::thread;
    use std::vec::Vec;

    type Sim = SimOs<Standard>;

    #[test]
    fn a_reservation_takes_the_largest_plan() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        assert_eq!(space.plan().objects(), Sim::GEOMETRY.max_objects());
        assert_eq!(space.base().addr().get() % Sim::GEOMETRY.region_size(), 0);
        // SAFETY: nothing else uses the reservation.
        unsafe { space.unmap().unwrap() };
        assert_eq!(os.reserved_bytes(), 0);
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
        assert_eq!(location.region, region);
        assert_eq!(plan.slot_at(location, class), Some(5));
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
        space.commit(region, 2 * page).unwrap();
        assert!(os.is_writable(region, 2 * page));
        // SAFETY: the two pages were just committed and nothing else uses them.
        unsafe { region.as_ptr().add(page + 3).write(7) };
        // SAFETY: nothing uses the pages during or after the purge.
        unsafe { space.purge(region, 2 * page).unwrap() };
        // SAFETY: still committed, and purged memory reads as zero.
        assert_eq!(unsafe { region.as_ptr().add(page + 3).read() }, 0);
        space.advise_huge(region, page, HugePages::Prefer).unwrap();
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
        let _ = space.commit(space.base(), 1);
    }

    #[test]
    fn the_cell_publishes_one_space() {
        let cell = SpaceCell::new(Sim::with_capacity(2 * Sim::GEOMETRY.max_reservation()));
        assert_eq!(cell.get(), None);
        let first = cell.get_or_reserve().unwrap();
        assert_eq!(cell.get(), Some(first));
        assert_eq!(cell.get_or_reserve(), Ok(first));
        assert_eq!(cell.os().reserved_bytes(), first.plan().size());
    }

    #[test]
    fn racing_threads_publish_one_space_and_losers_unmap() {
        // Room for every thread's reservation at once, so each reserves the largest plan.
        let cell = SpaceCell::new(Sim::with_capacity(4 * Sim::GEOMETRY.max_reservation()));
        let bases: Vec<usize> = thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| cell.get_or_reserve().unwrap().base().addr().get()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(bases.iter().all(|&b| b == bases[0]));
        let space = cell.get().unwrap();
        assert_eq!(cell.os().reserved_bytes(), space.plan().size());
    }

    #[test]
    fn a_failed_reservation_publishes_nothing() {
        let cell = SpaceCell::new(Sim::new());
        cell.os().set_faults(Call::Reserve, Faults::ALWAYS);
        assert_eq!(cell.get_or_reserve(), Err(OsError::ENOMEM));
        assert_eq!(cell.get(), None);
    }
}
