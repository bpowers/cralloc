//! Magazines: where they live, how they are made and named, and what an unloaded one records.
//!
//! Magazines are carved from the magazine area of the reservation by advancing a counter, and
//! are never unmapped, so a stale reference to one reads harmless memory. The area's first unit
//! (the smallest magazine's size) is left unused, so that a magazine, named by its offset in the
//! area divided by that unit, has a number that fits in 32 bits and is never zero, and the depot
//! can keep a number and a version counter in one word.
//!
//! A magazine of `n` slots: slot 0 is the link (the depot's or a spare stack's), slots 1 to
//! `n - 2` hold objects, and while the magazine is not loaded the last slot records its fill
//! level and, in its high half, a spare stack's depth below it (a hint for the spare budget).
//!
//! [`Magazine`] is the handle the slow paths hold. One is only ever made for an address that is
//! inside the committed part of the area and aligned to its size, checked when it is made, so
//! every slot access through it is in bounds whatever a slab word or a stale link says. Outside
//! the crate a handle can only be had uniquely, from [`Area::create`] or a stack pop; the
//! lookups that turn an address or a number into a handle are the crate's own. Every
//! access to a slot is atomic. The ledger for the unsafe block is
//! `docs/safety/asmalloc/depot/magazine-safety.md`.

use super::Depot;
use crate::classes::{MagazineSize, SLOT_BYTES};
use crate::front::sync::AtomicPtr;
use crate::os::Os;
use crate::space::Space;
use core::marker::PhantomData;
use core::num::{NonZeroU32, NonZeroUsize};
use core::ptr::{self, NonNull};
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};

/// Bytes of the magazine area committed at a time, beyond what a new magazine needs, so that
/// most magazines are made without a system call.
const COMMIT_AHEAD: usize = 64 << 10;

/// The unit of magazine numbers: the smallest magazine.
pub(crate) const UNIT: usize = MagazineSize::MIN.bytes();

/// A magazine's number: its offset in the magazine area in units of the smallest magazine.
/// Never zero, because the first unit is not used.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct MagazineId(NonZeroU32);

impl MagazineId {
    /// The number, for a depot head.
    #[inline(always)]
    pub(crate) const fn get(self) -> u32 {
        self.0.get()
    }

    /// The id with this number, if it is not zero.
    #[inline(always)]
    pub(crate) const fn new(number: u32) -> Option<MagazineId> {
        match NonZeroU32::new(number) {
            Some(n) => Some(MagazineId(n)),
            None => None,
        }
    }

    const fn offset(self) -> usize {
        self.0.get() as usize * UNIT
    }
}

/// A magazine this thread holds: not loaded in any slab, on no stack. See the module
/// documentation. It borrows the reservation it lies in, so it cannot outlive it.
///
/// One word: the address of slot 0, which is aligned to the magazine's size and so to at least
/// [`UNIT`], with the size's ordinal in the low bits that alignment leaves clear. The slow paths
/// pass handles between functions that are not inlined, and the Rust ABI returns a value of up
/// to two words, such as an `Option` of a handle or a handle with a flag, in registers. A
/// handle of three fields went through memory, written a field at a time by the callee and read
/// back sixteen bytes at a time by the caller, and a load that spans several stores waits for
/// them to retire instead of taking its value from the store buffer: about twenty cycles of
/// every exchange. The number, which only a stack push and the age limit need, is worked out
/// from the address by the area ([`Area::id`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Magazine<'a> {
    word: NonNull<u8>,
    area: PhantomData<&'a ()>,
}

/// The low bits of a handle's word that hold its size's ordinal: those the smallest magazine's
/// alignment leaves clear.
const SIZE_BITS: usize = UNIT - 1;
const _: () = assert!(MagazineSize::COUNT <= SIZE_BITS + 1);
// What the slow paths return in registers (see `Magazine`).
const _: () = assert!(size_of::<Magazine<'_>>() == 8 && size_of::<Option<Magazine<'_>>>() == 8);
const _: () = assert!(size_of::<Option<(Magazine<'_>, bool)>>() <= 16);
const _: () = assert!(size_of::<Option<(Magazine<'_>, usize)>>() <= 16);

/// What the last slot of an unloaded magazine records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Record {
    /// Objects in slots 1 to `level`.
    pub level: usize,
    /// Magazines below this one on the spare stack it was pushed on.
    pub depth: u32,
}

impl Magazine<'_> {
    /// The size.
    #[inline]
    pub(crate) fn size(&self) -> MagazineSize {
        // Every handle is made with a size's ordinal (`Area::magazine_at`); the fallback, never
        // taken, is the smallest size, whose slots every handle's memory holds.
        MagazineSize::from_ordinal(self.word.addr().get() & SIZE_BITS).unwrap_or(MagazineSize::MIN)
    }

    /// The address of slot 0, which is what spare stacks hold.
    #[inline]
    pub(crate) fn base(&self) -> NonNull<u8> {
        // Slot 0 lies past the area's first unit, so clearing the size leaves it non-zero; the
        // fallback is never taken.
        self.word
            .map_addr(|word| NonZeroUsize::new(word.get() & !SIZE_BITS).unwrap_or(word))
    }

    /// Slot `index`, which must be below the slot count.
    fn slot(&self, index: usize) -> &AtomicPtr<u8> {
        let index = index & (self.size().slots() - 1);
        let address = self.base().as_ptr().wrapping_add(index * SLOT_BYTES);
        // SAFETY: MAGAZINE-01.
        unsafe { AtomicPtr::from_ptr(address.cast::<*mut u8>()) }
    }

    /// The object slots, 1 to `capacity`, as one slice, for a fill or a launder of a magazine
    /// this thread holds: the back end reads and writes them in place. The plain `core` atomic
    /// type, without the interleaving checker's switch points, which a magazine nobody else
    /// can reach does not need.
    pub(crate) fn objects(&self) -> &[core::sync::atomic::AtomicPtr<u8>] {
        let first = self
            .base()
            .as_ptr()
            .wrapping_add(SLOT_BYTES)
            .cast::<core::sync::atomic::AtomicPtr<u8>>();
        // SAFETY: MAGAZINE-02 in docs/safety/asmalloc/depot/magazine-safety.md.
        unsafe { core::slice::from_raw_parts(first, self.size().capacity()) }
    }

    /// The link, slot 0, as the stacks use it.
    pub(super) fn link(&self) -> &AtomicPtr<u8> {
        self.slot(0)
    }

    /// The level and depth recorded in the last slot.
    pub(crate) fn record(&self) -> Record {
        let word = self.slot(self.size().slots() - 1).load(Acquire).addr() as u64;
        Record {
            level: ((word as u32) as usize).min(self.size().capacity()),
            depth: (word >> 32) as u32,
        }
    }

    /// Records a level and a spare depth in the last slot. A level above the capacity is
    /// recorded as the capacity, as `record` would read it anyway.
    pub(crate) fn set_record(&self, record: Record) {
        let level = record.level.min(self.size().capacity());
        let word = (record.depth as u64) << 32 | level as u64;
        self.slot(self.size().slots() - 1)
            .store(ptr::without_provenance_mut(word as usize), Release);
    }

    /// The level recorded in the last slot.
    pub(crate) fn level(&self) -> usize {
        self.record().level
    }

    /// Records a level, with no spare depth.
    pub(crate) fn set_level(&self, level: usize) {
        self.set_record(Record { level, depth: 0 });
    }

    /// The object in slot `index`, `1..=capacity`. Any other index reads null, which every
    /// caller already refuses, rather than the link or the record.
    pub(crate) fn object(&self, index: usize) -> *mut u8 {
        if (1..=self.size().capacity()).contains(&index) {
            self.slot(index).load(Relaxed)
        } else {
            ptr::null_mut()
        }
    }

    /// Puts an object in slot `index`, `1..=capacity`. Any other index is refused, so that
    /// no call can overwrite the link or the record, which stacks and the slow paths trust.
    pub(crate) fn set_object(&self, index: usize, object: NonNull<u8>) {
        if (1..=self.size().capacity()).contains(&index) {
            self.slot(index).store(object.as_ptr(), Relaxed);
        }
    }

    /// Moves the top `n` of this magazine's `level` objects, in their order, into slots 1 to
    /// `n` of `to`, and records both levels: `level - n` here and `n` there. Both are this
    /// thread's: neither is loaded or on a stack, so the copy is not a commit and nothing else
    /// sees it until one of them is installed or pushed. `n` is cut to what `level` and `to`'s
    /// capacity allow.
    pub(crate) fn move_top_into(&self, level: usize, to: &Magazine<'_>, n: usize) {
        let level = level.min(self.size().capacity());
        let n = n.min(level).min(to.size().capacity());
        let from = self.objects();
        let into = to.objects();
        if let (Some(from), Some(into)) = (from.get(level - n..level), into.get(..n)) {
            for (to, from) in into.iter().zip(from) {
                to.store(from.load(Relaxed), Relaxed);
            }
        }
        self.set_level(level - n);
        to.set_level(n);
    }

    /// The slab word that loads this magazine at `level`: the address of slot `level + 1`. A
    /// level above the capacity is taken as the capacity, so the word always names a slot of
    /// this magazine.
    pub(crate) fn word(&self, level: usize) -> *mut u8 {
        let level = level.min(self.size().capacity());
        self.base().as_ptr().wrapping_add((level + 1) * SLOT_BYTES)
    }
}

/// Whether a magazine of `size` can be at `offset` in an area whose magazines start at `start`
/// and whose first `committed` bytes are writable: past the start, aligned to its size, and
/// wholly inside the committed part. Every magazine handle is made only for an offset this
/// admits, which is what makes its slot accesses safe (`verify.rs` proves it for all inputs).
pub(super) const fn admits(
    offset: usize,
    size: MagazineSize,
    start: usize,
    committed: usize,
) -> bool {
    offset >= start
        && offset.is_multiple_of(size.bytes())
        && offset <= committed
        && size.bytes() <= committed - offset
}

/// The magazine area of a heap, with the depot's counters for it.
pub(crate) struct Area<'a, O: Os> {
    space: Space<'a, O>,
    depot: &'a Depot,
}

impl<O: Os> Clone for Area<'_, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O: Os> Copy for Area<'_, O> {}

impl<'a, O: Os> Area<'a, O> {
    pub(super) fn new(space: Space<'a, O>, depot: &'a Depot) -> Area<'a, O> {
        Area { space, depot }
    }

    /// Where magazines begin: one unit in, so that no magazine has the number zero.
    fn start() -> usize {
        UNIT
    }

    /// The area's size in this reservation, capped so that every offset in it has a 32-bit
    /// number.
    fn end(&self) -> usize {
        self.space
            .plan()
            .magazine_area_size()
            .min(u32::MAX as usize * UNIT)
    }

    /// The pointer `offset` bytes into the area, with the reservation's provenance.
    fn at(&self, offset: usize) -> NonNull<u8> {
        self.space
            .magazines()
            .map_addr(|base| base.saturating_add(offset))
    }

    /// The magazine of `size` at `offset`, if one can be there: past the first unit,
    /// aligned to its size, and inside the committed part.
    fn magazine_at(&self, offset: usize, size: MagazineSize) -> Option<Magazine<'a>> {
        let committed = self.depot.growth.committed.load(Acquire);
        let fits = admits(offset, size, Self::start(), committed);
        // A number must fit the depot's 32 bits (`Area::end` keeps the area within them).
        MagazineId::new(u32::try_from(offset / UNIT).ok()?)?;
        fits.then(|| Magazine {
            word: self.at(offset).map_addr(|base| base | size.ordinal()),
            area: PhantomData,
        })
    }

    /// The number of a magazine of this area: its offset in units, which `magazine_at` checked
    /// fits 32 bits and is not zero.
    pub(crate) fn id(&self, magazine: &Magazine<'_>) -> MagazineId {
        let offset = magazine
            .base()
            .addr()
            .get()
            .wrapping_sub(self.space.magazines().addr().get());
        match MagazineId::new((offset / UNIT) as u32) {
            Some(id) => id,
            None => self.space.os().abort("a magazine outside its area"),
        }
    }

    /// The magazine of `size` whose slot 0 is at `address`, if one can be there.
    pub(crate) fn magazine(&self, address: usize, size: MagazineSize) -> Option<Magazine<'a>> {
        let offset = address.wrapping_sub(self.space.magazines().addr().get());
        self.magazine_at(offset, size)
    }

    /// The magazine with this number.
    pub(crate) fn by_id(&self, id: MagazineId, size: MagazineSize) -> Option<Magazine<'a>> {
        self.magazine_at(id.offset(), size)
    }

    /// The magazine a slab word names, and the level the word gives it: the word is the
    /// address of slot `level + 1`.
    pub(crate) fn loaded(&self, word: usize, size: MagazineSize) -> Option<(Magazine<'a>, usize)> {
        let slot = size.slot_of(word);
        let magazine = self.magazine(word - slot * SLOT_BYTES, size)?;
        (slot >= 1 && slot < size.slots()).then_some((magazine, slot - 1))
    }

    /// Makes `start..end` of the area writable if it is not already.
    fn commit_to(&self, end: usize) -> bool {
        let page = O::GEOMETRY.page_size();
        loop {
            let committed = self.depot.growth.committed.load(Acquire);
            if committed >= end {
                return true;
            }
            let target = (end + COMMIT_AHEAD).next_multiple_of(page).min(self.end());
            if self
                .space
                .commit(self.at(committed), target - committed)
                .is_err()
            {
                return false;
            }
            // Racing committers may cover the same pages; making them writable twice is
            // harmless, and the mark only moves up, after the memory under it is writable.
            self.depot.growth.committed.fetch_max(target, AcqRel);
        }
    }

    /// The generation page: the reservation's process page, which a forked child reads as
    /// zero. `generation::prepare` commits it.
    pub(super) fn generation_page(&self) -> NonNull<u8> {
        self.space.process_page()
    }

    /// The space, for the OS calls on the generation page.
    pub(super) fn space(&self) -> Space<'a, O> {
        self.space
    }

    /// Counts a magazine the area could not make, for tests.
    fn refuse(&self) {
        #[cfg(test)]
        self.depot
            .refused
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// A new, empty magazine of `size`, or `None` if the area is used up or cannot be made
    /// writable. Its memory is fresh, so it reads as an empty magazine.
    pub(crate) fn create(&self, size: MagazineSize) -> Option<Magazine<'a>> {
        let bytes = size.bytes();
        let mut carved = self.depot.growth.carved.load(Acquire);
        let start = loop {
            let start = (Self::start() + carved).next_multiple_of(bytes);
            if start + bytes > self.end() {
                self.refuse();
                return None;
            }
            match self.depot.growth.carved.compare_exchange(
                carved,
                start + bytes - Self::start(),
                AcqRel,
                Acquire,
            ) {
                Ok(_) => break start,
                Err(now) => carved = now,
            }
        };
        if !self.commit_to(start + bytes) {
            self.refuse();
            return None;
        }
        let magazine = self.magazine_at(start, size)?;
        magazine.set_level(0);
        #[cfg(test)]
        self.depot
            .created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(magazine)
    }
}
