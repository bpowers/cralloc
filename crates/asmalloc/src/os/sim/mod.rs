//! A simulated OS over ordinary memory, for tests, fuzzing, Kani and Miri.
//!
//! [`SimOs`] carves reservations out of one zeroed host allocation and records, per page,
//! whether it is reserved, writable, purged and advised for or against huge pages. It behaves as
//! Linux does wherever the allocator could tell the difference: reserved memory reads as zero,
//! commit keeps contents, purge zeroes, and unmapped memory comes back zero when reserved again.
//! It is stricter than Linux where that catches bugs: a call on memory that is not reserved, or
//! on a range that is not whole pages, panics. It cannot stop the allocator from writing memory
//! it never committed; tests check for that with [`SimOs::is_writable`].
//!
//! Any call can be made to fail on demand ([`SimOs::set_faults`]), the rseq registration it
//! reports can be changed ([`SimOs::set_rseq`]), and `abort` panics so that a test can observe
//! it. The heap's geometry comes from a [`Shape`]: [`Standard`] for tests and fuzzing, [`Tiny`]
//! for Kani harnesses. Other crates may define their own.
//!
//! The simulator takes no locks: every piece of state is an atomic, and racing reservations
//! claim pages by compare-and-swap.

mod faults;
mod memory;
mod pages;
#[cfg(kani)]
mod verify_slow;

pub use faults::{Call, Faults};

use self::faults::Injector;
use self::memory::Memory;
use self::pages::{HUGE, NO_HUGE, PURGED, Pages, RESERVED, START, WRITABLE};
use super::{HugePages, Os, OsError, RseqArea};
use crate::space::Geometry;
use core::fmt;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The heap geometry a [`SimOs`] hosts.
pub trait Shape: 'static {
    /// The geometry.
    const GEOMETRY: Geometry;
}

/// The shape for tests and fuzzing: 1 KiB pages, 8 KiB huge pages, 64 KiB regions, 16 MiB in
/// all, with 232 object regions (enough for every class) and a 1 MiB magazine area.
///
/// The page size is deliberately not 4 KiB, so that code which assumes 4 KiB pages instead of
/// asking the geometry fails here.
pub enum Standard {}

impl Shape for Standard {
    const GEOMETRY: Geometry = Geometry::new(10, 13, 16, 256, 20);
}

/// The shape for Kani: 64-byte pages, 256-byte huge pages, 2 KiB regions, 8 KiB in all, with two
/// object regions and a 2 KiB magazine area.
///
/// Kani's cost grows with every byte of simulated memory (zeroing 32 KiB already takes it half
/// a gigabyte), so this shape is as small as the geometry rules allow. Its regions hold only the
/// classes up to 2 KiB.
pub enum Tiny {}

impl Shape for Tiny {
    const GEOMETRY: Geometry = Geometry::new(6, 8, 11, 4, 11);
}

/// What glibc reports on the reference host: the area 320 bytes below the thread pointer, with
/// the full 33-byte feature set, so a concurrency ID.
const DEFAULT_RSEQ: Option<RseqArea> = RseqArea::new(-320, 33);

/// A simulated OS. See the module documentation.
pub struct SimOs<S: Shape = Standard> {
    memory: Memory,
    pages: Pages,
    faults: Injector,
    rseq: AtomicU64,
    fence_registered: AtomicBool,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> SimOs<S> {
    /// A simulated OS with room for one reservation of the largest plan.
    pub fn new() -> SimOs<S> {
        SimOs::with_capacity(S::GEOMETRY.max_reservation())
    }

    /// A simulated OS with `bytes` of memory, a non-zero multiple of the region size. Room for
    /// two largest plans lets a test race two reservations; less makes [`crate::space::Space`]
    /// settle for a smaller plan.
    pub fn with_capacity(bytes: usize) -> SimOs<S> {
        let geometry = S::GEOMETRY;
        assert!(
            bytes > 0 && bytes.is_multiple_of(geometry.region_size()),
            "simulated memory must be a non-zero number of regions"
        );
        SimOs {
            memory: Memory::new(bytes, geometry.region_size()),
            pages: Pages::new(bytes / geometry.page_size()),
            faults: Injector::new(),
            rseq: AtomicU64::new(encode_rseq(DEFAULT_RSEQ)),
            fence_registered: AtomicBool::new(false),
            shape: PhantomData,
        }
    }

    /// Sets when `call` fails from now on.
    pub fn set_faults(&self, call: Call, faults: Faults) {
        self.faults.set(call, faults);
    }

    /// The failures still planned for `call`.
    pub fn faults(&self, call: Call) -> Faults {
        self.faults.get(call)
    }

    /// How many times `call` has been made, including failed attempts and not counting calls
    /// that panicked on a bad range.
    pub fn calls(&self, call: Call) -> u64 {
        self.faults.count(call)
    }

    /// Sets the rseq registration [`Os::rseq`] reports, for example `None` to simulate valgrind
    /// or a registration without a concurrency ID. The offset must fit in an `i32`.
    pub fn set_rseq(&self, area: Option<RseqArea>) {
        self.rseq.store(encode_rseq(area), Ordering::Release);
    }

    /// Whether [`Os::register_fence`] has succeeded.
    pub fn fence_registered(&self) -> bool {
        self.fence_registered.load(Ordering::Acquire)
    }

    fn page_shift(&self) -> u32 {
        S::GEOMETRY.page_size().trailing_zeros()
    }

    /// The pages that cover `start..start + len`, if the range is inside simulated memory.
    fn covering(&self, start: NonNull<u8>, len: usize) -> Option<(usize, usize)> {
        let offset = self.memory.offset_of(start);
        if offset >= self.memory.len() || len > self.memory.len() - offset {
            return None;
        }
        let first = offset >> self.page_shift();
        let end = (offset + len).div_ceil(S::GEOMETRY.page_size());
        Some((first, end - first))
    }

    fn covering_all(&self, start: NonNull<u8>, len: usize, mask: u8) -> bool {
        self.covering(start, len)
            .is_some_and(|(first, count)| self.pages.all(first, count, mask))
    }

    /// Whether every page touching `start..start + len` is reserved.
    pub fn is_reserved(&self, start: NonNull<u8>, len: usize) -> bool {
        self.covering_all(start, len, RESERVED)
    }

    /// Whether every page touching `start..start + len` is reserved and committed.
    pub fn is_writable(&self, start: NonNull<u8>, len: usize) -> bool {
        self.covering_all(start, len, RESERVED | WRITABLE)
    }

    /// Whether every page touching `start..start + len` is reserved and has been purged since it
    /// was last committed.
    pub fn is_purged(&self, start: NonNull<u8>, len: usize) -> bool {
        self.covering_all(start, len, RESERVED | PURGED)
    }

    /// The huge-page advice last given for the page holding `at`, if any.
    pub fn huge_pages(&self, at: NonNull<u8>) -> Option<HugePages> {
        let (first, _) = self.covering(at, 1)?;
        pages::advice(self.pages.get(first))
    }

    /// Bytes currently reserved.
    pub fn reserved_bytes(&self) -> usize {
        self.pages.count(RESERVED) << self.page_shift()
    }

    /// Bytes currently committed.
    pub fn writable_bytes(&self) -> usize {
        self.pages.count(RESERVED | WRITABLE) << self.page_shift()
    }

    /// The first page and page count of a range a memory call was given, which the `Os`
    /// contract requires to be whole reserved pages. Panics otherwise: a bad range is an
    /// allocator bug, and a test should stop at it.
    fn checked_range(&self, call: Call, start: NonNull<u8>, len: usize) -> (usize, usize) {
        let page = S::GEOMETRY.page_size();
        let offset = self.memory.offset_of(start);
        let inside = offset < self.memory.len() && len <= self.memory.len() - offset;
        assert!(
            inside && len != 0 && (offset | len) & (page - 1) == 0,
            "simulated {call:?} on a range that is not whole pages of simulated memory: \
             {start:?} + {len:#x}"
        );
        let (first, count) = (offset >> self.page_shift(), len >> self.page_shift());
        assert!(
            self.pages.all(first, count, RESERVED),
            "simulated {call:?} on memory that is not reserved: {start:?} + {len:#x}"
        );
        assert!(
            !self.pages.any(first + 1, count - 1, START),
            "simulated {call:?} on a range spanning two reservations: {start:?} + {len:#x}"
        );
        (first, count)
    }
}

impl<S: Shape> Default for SimOs<S> {
    fn default() -> SimOs<S> {
        SimOs::new()
    }
}

impl<S: Shape> fmt::Debug for SimOs<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimOs")
            .field("geometry", &S::GEOMETRY)
            .field("capacity", &self.memory.len())
            .field("reserved", &self.reserved_bytes())
            .field("writable", &self.writable_bytes())
            .finish()
    }
}

fn encode_rseq(area: Option<RseqArea>) -> u64 {
    match area {
        None => 0,
        Some(area) => {
            let offset = i32::try_from(area.offset()).expect("simulated rseq offset fits in i32");
            (offset as u32 as u64) << 32 | area.size() as u64
        }
    }
}

fn decode_rseq(word: u64) -> Option<RseqArea> {
    RseqArea::new((word >> 32) as u32 as i32 as isize, word as u32)
}

// SAFETY: every promise of `Os` holds for the simulator.
// - Reservations are runs of pages claimed by compare-and-swap from the one allocation `memory`
//   owns, so they are disjoint from each other and from every other allocation, carry the
//   allocation's provenance, and are aligned as asked (the search steps through aligned
//   addresses). Pages that are not reserved are always zero: the allocation starts zeroed and
//   `unmap` zeroes a range before releasing its pages, with release ordering that the next
//   claim's acquire pairs with. So reserved memory reads as zero.
// - The memory is always readable and writable, so `commit` needs only to record it, and
//   changes no contents. `purge` zeroes its range. `advise_huge` only records.
// - An injected failure changes nothing.
// - A range must lie in one live reservation of this value. `checked_range` panics before
//   touching anything if it does not: outside this value's memory, not whole reserved pages,
//   or spanning two reservations.
// - There are no restartable sequences, so `fence` trivially waits for all of them.
// - `abort` panics, so it does not return.
// - All state is atomic, so any thread may call at any time; nothing here allocates after
//   construction except the formatting of a panic message.
unsafe impl<S: Shape> Os for SimOs<S> {
    const GEOMETRY: Geometry = S::GEOMETRY;

    fn reserve(&self, len: usize, align: usize) -> Result<NonNull<u8>, OsError> {
        self.faults.enter(Call::Reserve)?;
        let page = S::GEOMETRY.page_size();
        if len == 0 || !len.is_multiple_of(page) || !align.is_power_of_two() || align < page {
            return Err(OsError::EINVAL);
        }
        let Some(last) = self.memory.len().checked_sub(len) else {
            return Err(OsError::ENOMEM);
        };
        let misalignment = self.memory.at(0).addr().get() & (align - 1);
        let mut offset = (align - misalignment) & (align - 1);
        while offset <= last {
            if self
                .pages
                .claim(offset >> self.page_shift(), len >> self.page_shift())
            {
                return Ok(self.memory.at(offset));
            }
            let Some(next) = offset.checked_add(align) else {
                break;
            };
            offset = next;
        }
        Err(OsError::ENOMEM)
    }

    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::Commit, start, len);
        self.faults.enter(Call::Commit)?;
        self.pages.update(first, count, WRITABLE, PURGED);
        Ok(())
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::Purge, start, len);
        self.faults.enter(Call::Purge)?;
        // SAFETY: `checked_range` put the range inside `memory`, and the caller promises that
        // no thread touches it during the call.
        unsafe { self.memory.zero(self.memory.offset_of(start), len) };
        self.pages.update(first, count, PURGED, 0);
        Ok(())
    }

    unsafe fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::AdviseHuge, start, len);
        self.faults.enter(Call::AdviseHuge)?;
        match advice {
            HugePages::Prefer => self.pages.update(first, count, HUGE, NO_HUGE),
            HugePages::Avoid => self.pages.update(first, count, NO_HUGE, HUGE),
        }
        Ok(())
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::Unmap, start, len);
        self.faults.enter(Call::Unmap)?;
        // SAFETY: `checked_range` put the range inside `memory`, and the caller promises that
        // nothing touches it once the call begins. Zeroing keeps unreserved memory zero.
        unsafe { self.memory.zero(self.memory.offset_of(start), len) };
        self.pages.release(first, count);
        // What remains of a reservation after its head is unmapped is a reservation of its own,
        // as munmap leaves it. The page after the range already starts one if it is reserved
        // and belongs to another.
        let next = first + count;
        if next < self.pages.len() && self.pages.get(next) & RESERVED != 0 {
            self.pages.update(next, 1, START, 0);
        }
        Ok(())
    }

    fn register_fence(&self) -> Result<(), OsError> {
        self.faults.enter(Call::RegisterFence)?;
        self.fence_registered.store(true, Ordering::Release);
        Ok(())
    }

    fn fence(&self) -> Result<(), OsError> {
        self.faults.enter(Call::Fence)?;
        if self.fence_registered() {
            Ok(())
        } else {
            Err(OsError::EPERM)
        }
    }

    fn rseq(&self) -> Option<RseqArea> {
        decode_rseq(self.rseq.load(Ordering::Acquire))
    }

    fn abort(&self, message: &str) -> ! {
        panic!("asmalloc: {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::vec::Vec;

    type Sim = SimOs<Standard>;

    fn page() -> usize {
        Sim::GEOMETRY.page_size()
    }

    fn region() -> usize {
        Sim::GEOMETRY.region_size()
    }

    #[test]
    fn reservations_are_aligned_disjoint_and_zero() {
        let os = Sim::with_capacity(4 * region());
        let a = os.reserve(region(), region()).unwrap();
        let b = os.reserve(2 * page(), region()).unwrap();
        assert_eq!(a.addr().get() % region(), 0);
        assert_eq!(b.addr().get() % region(), 0);
        assert_eq!(b.addr().get() - a.addr().get(), region());
        assert!(os.is_reserved(a, region()) && os.is_reserved(b, 2 * page()));
        assert!(!os.is_writable(a, 1));
        assert_eq!(os.reserved_bytes(), region() + 2 * page());
        // A smaller alignment fits after `b` within its region.
        let c = os.reserve(page(), page()).unwrap();
        assert_eq!(c.addr().get(), b.addr().get() + 2 * page());
        assert_eq!(os.reserve(4 * region(), region()), Err(OsError::ENOMEM));
        assert_eq!(os.reserve(0, region()), Err(OsError::EINVAL));
        assert_eq!(os.reserve(page() + 1, region()), Err(OsError::EINVAL));
        assert_eq!(os.reserve(page(), page() / 2), Err(OsError::EINVAL));
    }

    #[test]
    fn commit_purge_and_unmap_keep_the_zero_promise() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(region(), region()).unwrap();
        let p = base.as_ptr();
        // SAFETY: each call's range is whole reserved pages, and nothing else touches them.
        unsafe {
            os.commit(base, 2 * page()).unwrap();
            assert!(os.is_writable(base, 2 * page()));
            assert!(!os.is_writable(base, 2 * page() + 1));
            p.add(5).write(1);
            p.add(page() + 5).write(2);
            os.purge(base, page()).unwrap();
            assert_eq!(p.add(5).read(), 0);
            assert_eq!(p.add(page() + 5).read(), 2, "purge stays inside its range");
            assert!(os.is_purged(base, page()) && !os.is_purged(base, 2 * page()));
            os.commit(base, page()).unwrap();
            assert!(!os.is_purged(base, page()));
            os.unmap(base, region()).unwrap();
        }
        assert_eq!(os.reserved_bytes(), 0);
        let again = os.reserve(region(), region()).unwrap();
        assert_eq!(again, base);
        // SAFETY: as above.
        unsafe {
            os.commit(again, 2 * page()).unwrap();
            assert_eq!(
                again.as_ptr().add(page() + 5).read(),
                0,
                "reserved memory is zero"
            );
        }
    }

    #[test]
    fn huge_page_advice_is_recorded_per_page() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(region(), region()).unwrap();
        let second = base.map_addr(|a| a.saturating_add(page()));
        assert_eq!(os.huge_pages(base), None);
        // SAFETY: whole reserved pages.
        unsafe {
            os.advise_huge(base, region(), HugePages::Avoid).unwrap();
            os.advise_huge(second, page(), HugePages::Prefer).unwrap();
        }
        assert_eq!(os.huge_pages(base), Some(HugePages::Avoid));
        assert_eq!(os.huge_pages(second), Some(HugePages::Prefer));
    }

    #[test]
    fn any_call_can_fail_on_demand_and_a_failure_changes_nothing() {
        let os = Sim::with_capacity(region());
        os.set_faults(Call::Reserve, Faults::once_after(0));
        assert_eq!(os.reserve(region(), region()), Err(OsError::ENOMEM));
        assert_eq!(os.reserved_bytes(), 0);
        let base = os.reserve(region(), region()).unwrap();
        os.set_faults(Call::Commit, Faults::ALWAYS);
        // SAFETY: whole reserved pages.
        unsafe {
            assert_eq!(os.commit(base, page()), Err(OsError::ENOMEM));
            assert!(!os.is_writable(base, page()));
            os.set_faults(Call::Commit, Faults::NONE);
            os.commit(base, page()).unwrap();
            base.as_ptr().write(9);
            os.set_faults(Call::Purge, Faults::once_after(0));
            assert!(os.purge(base, page()).is_err());
            assert_eq!(base.as_ptr().read(), 9);
        }
        assert_eq!(os.calls(Call::Commit), 2);
        assert_eq!(os.calls(Call::Reserve), 2);
        for call in Call::ALL {
            assert_eq!(os.faults(call), Faults::NONE, "{call:?}");
        }
    }

    #[test]
    fn fences_need_registration() {
        let os = Sim::new();
        assert_eq!(os.fence(), Err(OsError::EPERM));
        os.set_faults(Call::RegisterFence, Faults::once_after(0));
        assert_eq!(os.register_fence(), Err(OsError::EINVAL));
        assert!(!os.fence_registered());
        os.register_fence().unwrap();
        assert_eq!(os.fence(), Ok(()));
        assert_eq!(os.calls(Call::Fence), 2);
    }

    #[test]
    fn the_rseq_registration_can_be_changed() {
        let os = Sim::new();
        assert!(os.rseq().unwrap().has_cid());
        os.set_rseq(None);
        assert_eq!(os.rseq(), None);
        let legacy = RseqArea::new(-4096, 20);
        os.set_rseq(legacy);
        assert_eq!(os.rseq(), legacy);
        assert!(!os.rseq().unwrap().has_cid());
    }

    #[test]
    #[should_panic(expected = "asmalloc: double free")]
    fn abort_panics_with_the_message() {
        Sim::new().abort("double free");
    }

    #[test]
    #[should_panic(expected = "not reserved")]
    fn a_call_on_unreserved_memory_panics() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(page(), region()).unwrap();
        // SAFETY: deliberately breaks the contract; the simulator detects it before acting.
        unsafe { os.commit(base, 2 * page()).unwrap() };
    }

    #[test]
    #[should_panic(expected = "spanning two reservations")]
    fn a_call_spanning_two_reservations_panics() {
        let os = Sim::with_capacity(region());
        let a = os.reserve(page(), page()).unwrap();
        let b = os.reserve(page(), page()).unwrap();
        assert_eq!(b.addr().get(), a.addr().get() + page());
        // SAFETY: deliberately breaks the contract; the simulator detects it before acting.
        unsafe { os.commit(a, 2 * page()).unwrap() };
    }

    #[test]
    fn the_tail_of_a_partly_unmapped_reservation_stands_alone() {
        let os = Sim::with_capacity(region());
        let a = os.reserve(3 * page(), page()).unwrap();
        let tail = a.map_addr(|x| x.saturating_add(page()));
        // SAFETY: whole reserved pages that nothing uses.
        unsafe {
            os.unmap(a, page()).unwrap();
            os.commit(tail, 2 * page()).unwrap();
        }
        let b = os.reserve(page(), page()).unwrap();
        assert_eq!(b, a, "the unmapped head is free again");
        assert!(os.is_writable(tail, 2 * page()));
    }

    #[test]
    #[should_panic(expected = "not whole pages")]
    fn a_call_on_another_simulators_memory_panics() {
        let a = Sim::with_capacity(region());
        let b = Sim::with_capacity(region());
        let base = a.reserve(region(), region()).unwrap();
        // SAFETY: deliberately breaks the contract; the simulator detects it before acting.
        unsafe { b.commit(base, page()).unwrap() };
    }

    #[test]
    fn racing_reservations_never_overlap() {
        let os = Sim::with_capacity(8 * region());
        let bases: Vec<usize> = thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| os.reserve(region(), region()).unwrap().addr().get()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut sorted = bases.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8);
        assert_eq!(os.reserved_bytes(), 8 * region());
    }
}
