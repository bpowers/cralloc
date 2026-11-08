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
//!
//! # Racing reservations
//!
//! Between one thread's reservation and its compare-and-swap, the address space it holds is
//! not available to another thread starting at the same moment, and only a few full
//! reservations fit in the address space at once. So a failed reservation does not by itself
//! mean that address space is short. The cell counts the reservers in flight in one word, and a
//! thread whose reservation fails while another is counted looks for the published space and
//! tries again at the same size, after a short bounded spin. Only a failure with no other
//! reserver in flight, or once its patience for the whole start is used up, halves the region
//! size, and only a failure at the floor is an `Err` (a null return to the caller). Nothing
//! waits without bound and nothing is locked; a race costs at worst a heap with smaller regions,
//! never memory unsafety.
//!
//! A fork while a thread reserves gives the child a copy of a reservation that no published
//! word names, and a count that no thread of the child will take back. The reservation is never
//! used and costs the child address space only; the count costs the child's first start its
//! patience, bounded, and then it halves as it would have.

use super::{Location, Plan, RegionId};
use crate::classes::Class;
use crate::os::{HugePages, Os, OsError};
use core::fmt;
use core::num::NonZeroUsize;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// A live reservation obtained from `os`.
///
/// Invariant: the `plan().size()` bytes at `base` are one reservation that `os` returned and
/// that has not been unmapped, and `base` is aligned to the plan's region size. Only
/// [`Space::reserve`] and [`SpaceCell`] make one, and only the unsafe [`Space::unmap`] and the
/// cell's drop end it. The borrow of `os` keeps the OS value, and so the memory, alive for as
/// long as the `Space` exists.
///
/// Every field is a whole word. Every slow path makes a `Space` from the published word and
/// copies it around, and a [`Plan`], whose geometry is a dozen bytes of odd-sized fields, was
/// written a few bytes at a time and read back eight at a time: a load that spans several
/// stores waits for all of them to retire instead of taking its value from the store buffer,
/// which cost each slow path about fifty cycles. So a `Space` keeps only the region shift, and
/// [`Space::plan`] rebuilds the plan from the geometry, which is fixed for the process (a
/// constant on x86-64, one load on aarch64), and the shift, which folds to arithmetic on one
/// register.
pub struct Space<'os, O: Os> {
    base: NonNull<u8>,
    /// `log2` of the region size: the plan's, kept as a whole word (see above).
    region_shift: usize,
    os: &'os O,
    /// The first region's metadata block: worked out once, since every operation on the
    /// bitmaps reaches its words through it.
    metadata: NonNull<u8>,
}

impl<O: Os> Clone for Space<'_, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O: Os> Copy for Space<'_, O> {}

impl<O: Os> PartialEq for Space<'_, O> {
    fn eq(&self, other: &Self) -> bool {
        self.base == other.base && self.region_shift == other.region_shift
    }
}

impl<O: Os> Eq for Space<'_, O> {}

impl<O: Os> fmt::Debug for Space<'_, O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Space")
            .field("base", &self.base)
            .field("region_shift", &self.region_shift)
            .finish()
    }
}

impl<'os, O: Os> Space<'os, O> {
    #[inline(always)]
    fn new(base: NonNull<u8>, plan: Plan, os: &'os O) -> Space<'os, O> {
        // Reservations lie far below the top of the address space, as in `at`.
        let metadata = base.map_addr(|base| base.saturating_add(plan.metadata_start()));
        Space {
            base,
            region_shift: plan.region_shift() as usize,
            os,
            metadata,
        }
    }

    /// Reserves the largest plan the OS allows: regions of the geometry's largest size, else
    /// half that, and so on down to the floor, with the same number of regions each time. The
    /// error is the OS's answer at the floor: below it the heap cannot start.
    pub fn reserve(os: &'os O) -> Result<Space<'os, O>, OsError> {
        let geometry = O::geometry();
        let mut shift = geometry.max_region_shift();
        loop {
            let Some(plan) = geometry.plan(shift) else {
                return Err(OsError::EINVAL);
            };
            match os.reserve(plan.size(), plan.region_size()) {
                Ok(base) => return Ok(Space::new(base, plan, os)),
                Err(error) if shift == geometry.min_region_shift() => return Err(error),
                Err(_) => shift -= 1,
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

    /// The layout, including the region size this reservation got: the OS's geometry with this
    /// reservation's region shift. Every `Space` is made on `O::geometry()`, by
    /// [`Space::reserve`] and [`SpaceCell`], whose plans come from it.
    #[inline(always)]
    pub fn plan(&self) -> Plan {
        Plan::new(O::geometry(), self.region_shift as u8)
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
        self.plan()
            .locate(ptr.addr().wrapping_sub(self.base.addr().get()))
    }

    /// The pointer for an address inside the reservation, with the reservation's provenance, or
    /// `None` outside it. This is how an address a routine returns as an integer becomes a
    /// pointer again without an integer-to-pointer cast.
    #[inline(always)]
    pub fn pointer(&self, address: usize) -> Option<NonNull<u8>> {
        let offset = address.wrapping_sub(self.base.addr().get());
        if offset < self.plan().size() {
            Some(self.at(offset))
        } else {
            None
        }
    }

    /// The first byte of an object region. Every `RegionId` is an object region of every plan of
    /// the geometry (see [`RegionId`]); one that is not aborts.
    #[inline(always)]
    pub fn region(&self, region: RegionId) -> NonNull<u8> {
        self.check_region(region);
        self.at(self.plan().region_offset(region))
    }

    /// Slot `index` of `class` in `region`. Aborts unless the slot lies wholly inside the region
    /// ([`Plan::slot_fits`]), so the pointer is always one of the region's slots.
    #[inline(always)]
    pub fn slot(&self, region: RegionId, class: Class, index: u32) -> NonNull<u8> {
        self.check_region(region);
        if !self.plan().slot_fits(class, index) {
            self.os.abort("a slot index outside its region");
        }
        self.at(self.plan().slot_offset(region, class, index))
    }

    /// A region's metadata block, [`Plan::metadata_size`] bytes.
    #[inline(always)]
    pub fn metadata(&self, region: RegionId) -> NonNull<u8> {
        self.check_region(region);
        let offset = region.index() << self.plan().metadata_shift();
        self.metadata.map_addr(|start| start.saturating_add(offset))
    }

    /// Byte `offset` of a region's metadata block, as a raw pointer with the reservation's
    /// provenance: [`Space::metadata`] for a caller that reaches a word of the block on every
    /// operation, with no saturating arithmetic. `offset` is the caller's to keep inside the
    /// block; past it the pointer is merely not one to dereference.
    #[inline(always)]
    pub fn metadata_at(&self, region: RegionId, offset: usize) -> *mut u8 {
        self.check_region(region);
        let at = (region.index() << self.plan().metadata_shift()).wrapping_add(offset);
        self.metadata.as_ptr().wrapping_byte_add(at)
    }

    /// The magazine area, [`Plan::magazine_area_size`] bytes, aligned to the region size.
    #[inline(always)]
    pub fn magazines(&self) -> NonNull<u8> {
        self.at(self.plan().magazines_offset())
    }

    /// The process page, one page that a child forked after [`Os::wipe_on_fork`] marked it reads
    /// as zero.
    #[inline(always)]
    pub fn process_page(&self) -> NonNull<u8> {
        self.at(self.plan().process_page_offset())
    }

    fn check_region(&self, region: RegionId) {
        if region.index() >= self.plan().objects() {
            self.os.abort("a region id outside the heap");
        }
    }

    /// Whether `start..start + len` is a range the OS calls accept: inside this reservation,
    /// page-aligned, a whole number of pages and not empty.
    pub fn is_callable_range(&self, start: NonNull<u8>, len: usize) -> bool {
        let page = O::geometry().page_size();
        let offset = start.addr().get().wrapping_sub(self.base.addr().get());
        let size = self.plan().size();
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
        // SAFETY: RESERVATION-01. `check` returned, so the range is a non-empty, page-aligned run of whole pages
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
        // A plan without huge pages reaches here with its regions' units, which are no huge
        // pages to the kernel.
        if advice == HugePages::Prefer && !self.plan().geometry().huge_pages_usable() {
            return Ok(());
        }
        // SAFETY: RESERVATION-01. as in `commit`.
        unsafe { self.os.advise_huge(start, len, advice) }
    }

    /// Asks the OS to split the huge pages a range of the reservation covers in part
    /// ([`Os::split_huge`]). Aborts if the range is not one [`Space::is_callable_range`]
    /// accepts.
    pub fn split_huge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: RESERVATION-01. as in `commit`.
        unsafe { self.os.split_huge(start, len) }
    }

    /// How many pages of a range of the reservation are resident. Aborts if the range is not
    /// one [`Space::is_callable_range`] accepts.
    ///
    /// # Safety
    ///
    /// No thread writes the range during the call.
    pub unsafe fn resident(&self, start: NonNull<u8>, len: usize) -> Result<usize, OsError> {
        self.check(start, len);
        // SAFETY: RESERVATION-06. the range is valid as in `commit`; the caller promises nothing
        // writes it during the call.
        unsafe { self.os.resident(start, len) }
    }

    /// Marks a range of the reservation to read as zero in a forked child. Aborts if the range
    /// is not one [`Space::is_callable_range`] accepts.
    pub fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: RESERVATION-01. as in `commit`; the call changes nothing in this process.
        unsafe { self.os.wipe_on_fork(start, len) }
    }

    /// Returns a range's memory to the OS; it reads as zero afterwards. Aborts if the range is
    /// not one [`Space::is_callable_range`] accepts.
    ///
    /// # Safety
    ///
    /// No thread reads or writes the range during the call, and nothing in it is needed.
    pub unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.check(start, len);
        // SAFETY: RESERVATION-02. the range is valid as in `commit`; the caller promises nothing uses its
        // contents or touches it during the call.
        unsafe { self.os.purge(start, len) }
    }

    /// Unmaps the whole reservation.
    ///
    /// # Safety
    ///
    /// Nothing reads or writes the reservation, and no copy of this `Space` is used, after the
    /// call begins. In particular it must not be a space a [`SpaceCell`] has published; the
    /// cell unmaps that one itself when it is dropped.
    pub unsafe fn unmap(self) -> Result<(), OsError> {
        // SAFETY: RESERVATION-03. the whole reservation is a page-aligned, non-empty range of itself (the plan's
        // size is a whole number of pages), live by the invariant; the caller promises that
        // nothing uses it afterwards.
        unsafe { self.os.unmap(self.base, self.plan().size()) }
    }
}

/// An OS value and the one [`Space`] published over it, or none yet.
///
/// The space is one atomic word: the base address with the region shift in its low six bits,
/// which are free because the base is aligned to a region of at least 2 KiB. Zero means nothing
/// is published. Only this cell writes the word, and only with a reservation of its own OS, which
/// lives as long as the cell.
///
/// A second word counts the threads that are reserving in [`SpaceCell::get_or_reserve`]: each
/// adds one before its `reserve` and takes it away after the call fails, or after it has
/// published or unmapped what the call gave it.
pub struct SpaceCell<O: Os> {
    os: O,
    word: AtomicPtr<u8>,
    reserving: AtomicUsize,
}

/// Failed reservations, over one start, that a thread retries at the same size because other
/// reservers were in flight. Bounded, so that a stale count (a child forked while a parent's
/// thread was reserving) costs a bounded delay and then the ordinary halving.
const PATIENCE: u32 = 1 << 10;

/// How long a retry waits, in spins, for another reserver to publish or finish.
const SPINS: u32 = 1 << 10;

impl<O: Os> SpaceCell<O> {
    /// A cell over `os` with nothing published.
    pub const fn new(os: O) -> SpaceCell<O> {
        SpaceCell {
            os,
            word: AtomicPtr::new(ptr::null_mut()),
            reserving: AtomicUsize::new(0),
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
    /// thread publishes first, this one unmaps its own reservation and returns the winner. See
    /// the module documentation for how racing threads tell a full address space from one that
    /// other reservers hold for a moment.
    pub fn get_or_reserve(&self) -> Result<Space<'_, O>, OsError> {
        if let Some(space) = self.get() {
            return Ok(space);
        }
        let geometry = O::geometry();
        let mut shift = geometry.max_region_shift();
        let mut patience = PATIENCE;
        loop {
            let Some(plan) = geometry.plan(shift) else {
                return Err(OsError::EINVAL);
            };
            self.reserving.fetch_add(1, Ordering::AcqRel);
            let error = match self.os.reserve(plan.size(), plan.region_size()) {
                Ok(base) => {
                    let space = self.publish(Space::new(base, plan, &self.os));
                    self.reserving.fetch_sub(1, Ordering::AcqRel);
                    return Ok(space);
                }
                Err(error) => error,
            };
            // This thread is still counted, so anyone else makes the count more than one.
            let others = self.reserving.fetch_sub(1, Ordering::AcqRel) > 1;
            if let Some(space) = self.get() {
                return Ok(space);
            }
            if others && patience > 0 {
                patience -= 1;
                self.wait_for_others();
                continue;
            }
            if shift == geometry.min_region_shift() {
                return Err(error);
            }
            shift -= 1;
        }
    }

    /// Spins, a bounded while, until another reserver publishes or none is left in flight.
    fn wait_for_others(&self) {
        for _ in 0..SPINS {
            if self.is_published() || self.reserving.load(Ordering::Acquire) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Publishes `mine` unless another space was published first, in which case `mine` is
    /// unmapped and the winner returned.
    fn publish<'a>(&'a self, mine: Space<'a, O>) -> Space<'a, O> {
        let word = encode(mine);
        loop {
            match self.word.compare_exchange(
                ptr::null_mut(),
                word,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return mine,
                // A strong compare-and-swap from null fails only on a non-null word, so this
                // returns on the first pass.
                Err(current) => {
                    if let Some(current) = NonNull::new(current) {
                        // SAFETY: RESERVATION-04. `mine` was never published, so no other thread has its
                        // address, and this thread drops it here. A failed unmap leaves the
                        // range reserved, which wastes address space and nothing else.
                        let _ = unsafe { mine.unmap() };
                        return self.decode(current);
                    }
                }
            }
        }
    }

    fn is_published(&self) -> bool {
        !self.word.load(Ordering::Acquire).is_null()
    }

    /// The space a non-null word stands for. Only `publish` stores words, and only
    /// `encode` of a reservation of `self.os`.
    fn decode(&self, word: NonNull<u8>) -> Space<'_, O> {
        let (_, shift) = unpack(word.addr().get());
        // Clearing the shift leaves the base, which is not zero. `unwrap_or` keeps this total
        // for a word no `encode` made, which cannot be stored.
        let base = word.map_addr(|w| NonZeroUsize::new(unpack(w.get()).0).unwrap_or(w));
        Space::new(base, Plan::new(O::geometry(), shift as u8), &self.os)
    }
}

impl<O: Os> Drop for SpaceCell<O> {
    /// Unmaps the published space, so a cell made and dropped (in a test, say) does not leak
    /// it. A cell in a static is never dropped.
    fn drop(&mut self) {
        if let Some(word) = NonNull::new(*self.word.get_mut()) {
            let space = self.decode(word);
            // SAFETY: RESERVATION-05. `&mut self` means no `Space` borrowed from this cell is alive, and none
            // can be made after the drop, so nothing uses the reservation again. It is this
            // cell's own published space, which the unmap contract allows only here, where
            // the cell itself is going away.
            let _ = unsafe { space.unmap() };
        }
    }
}

/// The published word for a space: `base | region_shift`.
fn encode<O: Os>(space: Space<'_, O>) -> *mut u8 {
    let shift = space.plan().region_shift() as usize;
    space.base.as_ptr().map_addr(|base| pack(base, shift))
}

/// The bits of a published word that hold the region shift.
const SHIFT_BITS: usize = 63;

/// A base address aligned to at least 64 bytes and a region shift below 64, as one word.
pub(super) const fn pack(base: usize, region_shift: usize) -> usize {
    base | region_shift
}

/// The base address and region shift in a word `pack` made.
pub(super) const fn unpack(word: usize) -> (usize, usize) {
    (word & !SHIFT_BITS, word & SHIFT_BITS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::sim::{Call, Faults, SimOs, Standard};
    use std::thread;
    use std::vec::Vec;

    type Sim = SimOs<Standard>;

    fn largest() -> Plan {
        Sim::geometry().largest_plan()
    }

    #[test]
    fn a_reservation_takes_the_largest_plan() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        assert_eq!(space.plan(), largest());
        assert_eq!(space.base().addr().get() % largest().region_size(), 0);
        // SAFETY: nothing else uses the reservation.
        unsafe { space.unmap().unwrap() };
        assert_eq!(os.reserved_bytes(), 0);
    }

    #[test]
    fn a_reservation_shrinks_its_regions_until_the_os_agrees() {
        // Room for regions a quarter of the largest size, so the first two attempts fail.
        use crate::os::sim::{Native, Shape};
        let g = Native::GEOMETRY;
        let quarter = g.plan(g.max_region_shift() - 2).unwrap();
        let region = g.largest_plan().region_size();
        let os = SimOs::<Native>::with_capacity(quarter.size().next_multiple_of(region));
        let space = Space::reserve(&os).unwrap();
        assert_eq!(space.plan(), quarter);
        assert_eq!(space.plan().objects(), g.max_objects());
    }

    #[test]
    fn below_the_floor_the_reservation_fails() {
        let os = Sim::new();
        os.set_faults(Call::Reserve, Faults::ALWAYS);
        assert_eq!(Space::reserve(&os), Err(OsError::ENOMEM));
        let g = Sim::geometry();
        let attempts = g.max_region_shift() - g.min_region_shift() + 1;
        assert_eq!(os.calls(Call::Reserve), attempts as u64);
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
        assert_eq!(
            space.process_page().addr().get(),
            base + plan.process_page_offset()
        );
        assert_eq!(space.locate(space.metadata(region).as_ptr()), None);
        assert_eq!(space.locate(ptr::null()), None);
        assert_eq!(space.pointer(slot.addr().get()), Some(slot));
        assert_eq!(space.pointer(base + plan.size()), None);
        assert_eq!(space.pointer(base.wrapping_sub(1)), None);
    }

    #[test]
    #[should_panic(expected = "slot index outside its region")]
    fn a_slot_outside_its_region_aborts() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let region = space.plan().region(0).unwrap();
        let class = Class::of_size(8).unwrap();
        let _ = space.slot(region, class, u32::MAX);
    }

    #[test]
    fn commit_makes_memory_writable_and_purge_zeroes_it() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let region = space.region(space.plan().region(0).unwrap());
        let page = Sim::geometry().page_size();
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
    fn the_process_page_is_wiped_by_a_fork() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let page = space.process_page();
        let size = Sim::geometry().page_size();
        space.commit(page, size).unwrap();
        space.wipe_on_fork(page, size).unwrap();
        // SAFETY: committed, and nothing else uses it.
        unsafe { page.as_ptr().write(5) };
        // SAFETY: no other thread touches the page.
        unsafe { os.fork(0) };
        // SAFETY: still committed.
        assert_eq!(unsafe { page.as_ptr().read() }, 0);
    }

    #[test]
    fn ranges_outside_the_reservation_are_refused() {
        let os = Sim::new();
        let space = Space::reserve(&os).unwrap();
        let page = Sim::geometry().page_size();
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
        let cell = SpaceCell::new(Sim::with_capacity(
            2 * largest().size().next_multiple_of(largest().region_size()),
        ));
        assert_eq!(cell.get(), None);
        let first = cell.get_or_reserve().unwrap();
        assert_eq!(cell.get(), Some(first));
        assert_eq!(cell.get_or_reserve(), Ok(first));
        assert_eq!(first.plan(), largest());
        assert_eq!(cell.os().reserved_bytes(), first.plan().size());
    }

    #[test]
    fn the_published_word_keeps_a_shrunken_region_size() {
        let g = Sim::geometry();
        let floor = g.smallest_plan();
        let cell = SpaceCell::new(Sim::with_capacity(
            floor.size().next_multiple_of(largest().region_size()),
        ));
        let space = cell.get_or_reserve().unwrap();
        assert_eq!(space.plan(), floor);
        assert_eq!(
            cell.get().unwrap().plan().region_shift(),
            g.min_region_shift()
        );
    }

    #[test]
    fn racing_threads_publish_one_space_and_losers_unmap() {
        // Room for twice the threads' reservations: racing first-fit claims can push one another
        // past free space while a partial claim is being rolled back. The tiny shape keeps the
        // losers' unmaps, which check every uncommitted page, cheap under Miri.
        use crate::os::sim::{Tiny, capacity_for};
        let cell = SpaceCell::new(SimOs::<Tiny>::with_capacity(8 * capacity_for::<Tiny>()));
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
        assert_eq!(cell.reserving.load(Ordering::Relaxed), 0);
    }

    /// Room for regions half the largest size, as under an address-space limit.
    fn half_room() -> SpaceCell<Sim> {
        let g = Sim::geometry();
        let half = g.plan(g.max_region_shift() - 1).unwrap();
        SpaceCell::new(Sim::with_capacity(
            half.size().next_multiple_of(largest().region_size()),
        ))
    }

    #[test]
    fn a_failure_while_another_reserves_is_retried_at_the_same_size() {
        // Another thread is between its reservation and its publish; the first three attempts
        // fail for want of the room it holds, then it unmaps and the fourth fits.
        let cell = SpaceCell::new(Sim::new());
        cell.reserving.store(1, Ordering::Relaxed);
        cell.os().set_faults(
            Call::Reserve,
            Faults {
                pass: 0,
                fail: 3,
                partial: false,
            },
        );
        let space = cell.get_or_reserve().unwrap();
        assert_eq!(
            space.plan(),
            largest(),
            "it did not settle for smaller regions"
        );
        assert_eq!(cell.os().calls(Call::Reserve), 4);
        assert_eq!(
            cell.reserving.load(Ordering::Relaxed),
            1,
            "its own count is back"
        );
    }

    #[test]
    fn a_failure_with_no_other_reserver_halves_at_once() {
        let cell = half_room();
        let space = cell.get_or_reserve().unwrap();
        assert_eq!(
            space.plan().region_shift(),
            Sim::geometry().max_region_shift() - 1
        );
        assert_eq!(cell.os().calls(Call::Reserve), 2);
        assert_eq!(cell.reserving.load(Ordering::Relaxed), 0);
    }

    /// A child forked while its parent's thread was reserving inherits a count that nothing
    /// will take back. Its patience runs out, and then it halves as if alone.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_stale_count_costs_bounded_patience_and_then_halves() {
        let cell = half_room();
        cell.reserving.store(1, Ordering::Relaxed);
        let space = cell.get_or_reserve().unwrap();
        assert_eq!(
            space.plan().region_shift(),
            Sim::geometry().max_region_shift() - 1
        );
        assert_eq!(cell.os().calls(Call::Reserve), u64::from(PATIENCE) + 2);
    }

    #[test]
    fn threads_racing_for_room_for_one_heap_all_get_the_largest() {
        let rounds = if cfg!(miri) { 2 } else { 100 };
        for _ in 0..rounds {
            let cell = SpaceCell::new(Sim::new());
            let plans: Vec<Plan> = thread::scope(|scope| {
                let handles: Vec<_> = (0..4)
                    .map(|_| scope.spawn(|| cell.get_or_reserve().unwrap().plan()))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(plans.iter().all(|&plan| plan == largest()), "{plans:?}");
            assert_eq!(cell.reserving.load(Ordering::Relaxed), 0);
        }
    }
}
