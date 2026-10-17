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
//! What a test controls:
//!
//! - Any call can be made to fail on demand ([`SimOs::set_faults`]).
//! - The rseq registration it reports can be changed ([`SimOs::set_rseq`]).
//! - Simulated threads run twins on CPUs the test chooses, and the test can interrupt their
//!   sequences; `fence` aborts the ones on its CPU ([`SimOs::thread`]).
//! - A fork happens in place ([`SimOs::fork`]), or only its wiping of marked pages
//!   ([`SimOs::wipe_for_fork`]).
//! - Random numbers come from a seeded generator, so a test is repeatable.
//! - `abort` panics, so a test can observe it.
//!
//! The heap's geometry comes from a [`Shape`]: [`Standard`] for tests, Miri and fuzzing,
//! [`Native`] for large classes, [`Tiny`] for Kani harnesses. Other crates may define their own.
//!
//! The simulator takes no locks: every piece of state is an atomic, and racing reservations
//! claim pages by compare-and-swap.

mod cpus;
mod faults;
mod memory;
mod pages;
#[cfg(kani)]
mod verify_slow;

pub use cpus::{SIM_THREADS, SimThread};
pub use faults::{Call, Faults};

use self::cpus::Cpus;
use self::faults::Injector;
use self::memory::Memory;
use self::pages::{HUGE, NO_HUGE, PURGED, Pages, RESERVED, START, WIPE, WRITABLE};
use super::{HugePages, Os, OsError, RseqArea};
use crate::classes::MagazineSize;
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

/// The shape for tests, Miri and fuzzing: 1 KiB pages, 8 KiB huge pages, regions of 64 KiB
/// (32 KiB at the floor), 232 object regions (enough for every class), 16 magazine regions,
/// about 16 MiB in all.
///
/// The page size is deliberately not 4 KiB, so that code which assumes 4 KiB pages instead of
/// asking the geometry fails here. Large classes above 64 KiB do not fit a region; use
/// [`Native`] to test large classes with many slots.
pub enum Standard {}

impl Shape for Standard {
    const GEOMETRY: Geometry = Geometry::new(10, 13, 16, 15, 256, 16, MagazineSize::MAX);
}

/// The shape for native tests of large classes: 4 KiB pages, 64 KiB huge pages, regions of
/// 1 MiB (128 KiB at the floor), 247 object regions, about 256 MiB of address space.
///
/// The host commits the simulated memory lazily, so the size costs nothing until it is touched;
/// it is too large for Miri.
pub enum Native {}

impl Shape for Native {
    const GEOMETRY: Geometry = Geometry::new(12, 16, 20, 17, 256, 1, MagazineSize::MAX);
}

/// The shape for Kani: 64-byte pages, 256-byte huge pages, 2 KiB regions, two object regions,
/// one magazine region, 8 KiB in all, and magazines capped at the smallest size, so that 64 of
/// them fit and magazine exchange can be exercised.
///
/// Kani's cost grows with every byte of simulated memory (zeroing 32 KiB already takes it half
/// a gigabyte), so this shape is as small as the geometry rules allow. Its regions hold only the
/// classes up to 2 KiB.
pub enum Tiny {}

impl Shape for Tiny {
    const GEOMETRY: Geometry = Geometry::new(6, 8, 11, 11, 4, 1, MagazineSize::MIN);
}

/// Bytes of simulated memory that hold one reservation of a shape's largest plan, rounded up to
/// the largest region size.
pub const fn capacity_for<S: Shape>() -> usize {
    let plan = S::GEOMETRY.largest_plan();
    plan.size().next_multiple_of(plan.region_size())
}

/// What glibc reports on the reference host: the area 320 bytes below the thread pointer, with
/// the full 33-byte feature set.
const DEFAULT_RSEQ: Option<RseqArea> = RseqArea::new(-320, 33);

/// The random generator's step (splitmix64's increment: the odd number nearest `2^64 / phi`).
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// A simulated OS. See the module documentation.
pub struct SimOs<S: Shape = Standard> {
    memory: Memory,
    pages: Pages,
    faults: Injector,
    rseq: AtomicU64,
    fence_registered: AtomicBool,
    last_fence: AtomicU64,
    random: AtomicU64,
    cpus: Cpus<SIM_THREADS>,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> SimOs<S> {
    /// A simulated OS with room for one reservation of the largest plan.
    pub fn new() -> SimOs<S> {
        SimOs::with_capacity(capacity_for::<S>())
    }

    /// A simulated OS with `bytes` of memory, a non-zero multiple of the largest region size.
    /// Room for two largest plans lets a test race two reservations; less makes
    /// [`crate::space::Space`] settle for smaller regions.
    pub fn with_capacity(bytes: usize) -> SimOs<S> {
        let region = S::GEOMETRY.largest_plan().region_size();
        assert!(
            bytes > 0 && bytes.is_multiple_of(region),
            "simulated memory must be a non-zero number of the largest regions"
        );
        let geometry = S::GEOMETRY;
        SimOs {
            memory: Memory::new(bytes, region),
            pages: Pages::new(bytes / geometry.page_size()),
            faults: Injector::new(),
            rseq: AtomicU64::new(encode_rseq(DEFAULT_RSEQ)),
            fence_registered: AtomicBool::new(false),
            last_fence: AtomicU64::new(0),
            random: AtomicU64::new(0),
            cpus: Cpus::new(),
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

    /// The CPU the last successful [`Os::fence`] was aimed at, if there was one.
    pub fn last_fence(&self) -> Option<u32> {
        match self.last_fence.load(Ordering::Acquire) {
            0 => None,
            cpu => Some((cpu - 1) as u32),
        }
    }

    /// Restarts the random generator from `seed`, so that a test draws a known sequence.
    pub fn set_random_seed(&self, seed: u64) {
        self.random.store(seed, Ordering::Release);
    }

    /// Simulated thread `index`, below [`SIM_THREADS`], to run twins on. Thread `i` starts on
    /// CPU `i`; [`Os::fence`] aborts the sequences in flight on the CPU it names.
    pub fn thread(&self, index: usize) -> SimThread<'_> {
        self.cpus.thread(index)
    }

    /// Forks the process in place: the simulator becomes the child. Pages marked by
    /// [`Os::wipe_on_fork`] read as zero, every simulated thread but `survivor` vanishes (running
    /// a twin on one panics), and the survivor's sequence in flight, if any, is aborted. The
    /// rest of memory, with every pointer in it, is as the parent left it.
    ///
    /// The parent cannot continue alongside the child: the child's heap must sit at the same
    /// addresses as the parent's, and one process cannot hold two copies there. A test of the
    /// parent's side simply does not call this.
    ///
    /// # Safety
    ///
    /// No thread reads or writes a marked page during the call.
    pub unsafe fn fork(&self, survivor: usize) {
        // SAFETY: passed through from the caller.
        unsafe { self.wipe_for_fork() };
        self.cpus.fork(survivor);
    }

    /// Zeroes every page marked by [`Os::wipe_on_fork`], as the child of a fork would see them.
    /// A test that simulates a fork calls this at the instant of the fork.
    ///
    /// # Safety
    ///
    /// No thread reads or writes a marked page during the call.
    pub unsafe fn wipe_for_fork(&self) {
        let page = S::GEOMETRY.page_size();
        for index in self.pages.with(RESERVED | WIPE) {
            // SAFETY: the page is inside `memory` (it has a state), and the caller promises
            // that nothing touches it meanwhile.
            unsafe { self.memory.zero(index << self.page_shift(), page) };
        }
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
// - The only restartable sequences are twins on simulated threads. `fence` aborts every one in
//   flight on its CPU before returning; under cooperative scheduling that is all of them (see
//   `cpus.rs` for the one window real threads leave).
// - `wipe_on_fork` only records; nothing forks the simulator, so the promise is about what
//   `wipe_for_fork` shows a test, and it zeroes exactly the marked pages.
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

    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::WipeOnFork, start, len);
        self.faults.enter(Call::WipeOnFork)?;
        self.pages.update(first, count, WIPE, 0);
        Ok(())
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::Unmap, start, len);
        self.faults.enter(Call::Unmap)?;
        // SAFETY: `checked_range` put the range inside `memory`, and the caller promises that
        // nothing touches it once the call begins. Zeroing keeps unreserved memory zero.
        // What remains of a reservation after its head is unmapped is a reservation of its own,
        // as munmap leaves it. This is decided while the range is still reserved: then a
        // reserved page right after it without `START` can only be this reservation's tail,
        // never a reservation another thread is making. Doing it after the release would race
        // with such a thread.
        let next = first + count;
        if next < self.pages.len() {
            self.pages.mark_start(next);
        }
        // SAFETY: `checked_range` put the range inside `memory`, and the caller promises that
        // nothing touches it once the call begins. Zeroing keeps unreserved memory zero.
        unsafe { self.memory.zero(self.memory.offset_of(start), len) };
        self.pages.release(first, count);
        Ok(())
    }

    fn register_fence(&self) -> Result<(), OsError> {
        self.faults.enter(Call::RegisterFence)?;
        self.fence_registered.store(true, Ordering::Release);
        Ok(())
    }

    fn fence(&self, cpu: u32) -> Result<(), OsError> {
        self.faults.enter(Call::Fence)?;
        if self.fence_registered() {
            self.last_fence.store(cpu as u64 + 1, Ordering::Release);
            self.cpus.fence(cpu);
            Ok(())
        } else {
            Err(OsError::EPERM)
        }
    }

    fn rseq(&self) -> Option<RseqArea> {
        decode_rseq(self.rseq.load(Ordering::Acquire))
    }

    fn random(&self) -> u64 {
        // splitmix64: each call takes the next point of a Weyl sequence and scrambles it.
        let mut z = self
            .random
            .fetch_add(GAMMA, Ordering::AcqRel)
            .wrapping_add(GAMMA);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
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
        Sim::GEOMETRY.largest_plan().region_size()
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
        assert_eq!(os.fence(3), Err(OsError::EPERM));
        os.set_faults(Call::RegisterFence, Faults::once_after(0));
        assert_eq!(os.register_fence(), Err(OsError::EINVAL));
        assert!(!os.fence_registered());
        os.register_fence().unwrap();
        assert_eq!(os.last_fence(), None);
        assert_eq!(os.fence(3), Ok(()));
        assert_eq!(os.last_fence(), Some(3));
        assert_eq!(os.calls(Call::Fence), 2);
    }

    #[test]
    fn a_fence_aborts_the_twins_in_flight_on_its_cpu_and_a_failed_one_none() {
        use crate::os::{Aborted, Machine, Step};
        let os = Sim::new();
        os.register_fence().unwrap();
        let (a, b) = (os.thread(0), os.thread(1));
        a.step(Step::ReadCpu).unwrap();
        b.step(Step::ReadCpu).unwrap();
        os.fence(0).unwrap();
        assert_eq!(a.step(Step::Load), Err(Aborted));
        assert_eq!(b.step(Step::Load), Ok(()));
        a.step(Step::ReadCpu).unwrap();
        os.set_faults(Call::Fence, Faults::once_after(0));
        assert!(os.fence(0).is_err());
        assert_eq!(
            a.step(Step::Load),
            Ok(()),
            "a failed fence interrupts nothing"
        );
    }

    #[test]
    fn a_fork_keeps_memory_wipes_marked_pages_and_one_thread() {
        use crate::os::{Machine, Step};
        let os = Sim::with_capacity(region());
        let base = os.reserve(2 * page(), region()).unwrap();
        let second = base.map_addr(|a| a.saturating_add(page()));
        let survivor = os.thread(3);
        // SAFETY: whole reserved pages that nothing else uses.
        unsafe {
            os.commit(base, 2 * page()).unwrap();
            os.wipe_on_fork(second, page()).unwrap();
            base.as_ptr().write(1);
            second.as_ptr().write(2);
            os.fork(3);
            assert_eq!(base.as_ptr().read(), 1);
            assert_eq!(second.as_ptr().read(), 0);
        }
        assert_eq!(survivor.step(Step::ReadCpu), Ok(()));
        let lost = std::panic::catch_unwind(|| os.thread(0).cpu_id());
        assert!(lost.is_err());
    }

    #[test]
    fn the_rseq_registration_can_be_changed() {
        let os = Sim::new();
        assert_eq!(os.rseq(), DEFAULT_RSEQ);
        os.set_rseq(None);
        assert_eq!(os.rseq(), None);
        let legacy = RseqArea::new(-4096, 20);
        os.set_rseq(legacy);
        assert_eq!(os.rseq(), legacy);
    }

    #[test]
    fn marked_pages_are_wiped_as_a_fork_would() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(2 * page(), region()).unwrap();
        let second = base.map_addr(|a| a.saturating_add(page()));
        // SAFETY: whole reserved pages that nothing else uses.
        unsafe {
            os.commit(base, 2 * page()).unwrap();
            os.wipe_on_fork(second, page()).unwrap();
            base.as_ptr().write(1);
            second.as_ptr().write(2);
            os.wipe_for_fork();
            assert_eq!(base.as_ptr().read(), 1);
            assert_eq!(second.as_ptr().read(), 0);
        }
    }

    #[test]
    fn random_numbers_follow_the_seed() {
        let os = Sim::new();
        let first: Vec<u64> = (0..4).map(|_| os.random()).collect();
        os.set_random_seed(0);
        let again: Vec<u64> = (0..4).map(|_| os.random()).collect();
        assert_eq!(first, again);
        let mut distinct = first.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 4);
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
