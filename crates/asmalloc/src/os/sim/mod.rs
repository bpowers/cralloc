//! A simulated OS over ordinary memory, for tests, fuzzing, Kani and Miri.
//!
//! [`SimOs`] carves reservations out of one zeroed host allocation and records, per page,
//! whether it is reserved, writable, purged and advised for or against huge pages. It behaves as
//! Linux does wherever the allocator could tell the difference: reserved memory reads as zero,
//! commit keeps contents, purge zeroes, and unmapped memory comes back zero when reserved again.
//! It is stricter than Linux where that catches bugs: a call on memory that is not reserved, or
//! on a range that is not whole pages, panics. [`SimOs::is_writable`] lets a test check a block
//! before writing, [`SimOs::resident_bytes`] measures what the heap keeps resident, and
//! [`SimOs::is_purged`] turns false once purged memory is written again.
//!
//! Natively, a shape whose pages are whole host pages gets *guarded* memory: uncommitted pages
//! are inaccessible, so touching one faults at once, as on Linux. Otherwise the memory is
//! ordinary, and a write to an uncommitted page is caught only later: such a page must still be
//! zero when it is committed or purged, or the call panics.
//!
//! Known limits of the unguarded simulator (every shape under Miri and Kani, and [`Tiny`]
//! natively), which accepts what Linux would refuse:
//!
//! - a read of memory that was never committed;
//! - a write to it that is undone before the page is committed, purged or unmapped;
//! - use of a page whose commit failed;
//! - use of memory after it is unmapped.
//!
//! Tests of those bugs run natively on a guarded shape. Simulated threads that share a CPU need
//! a cooperative scheduler; see [`SimOs::thread`].
//!
//! What a test controls:
//!
//! - Any call can be made to fail on demand ([`SimOs::set_faults`]).
//! - Simulated threads run twins on CPUs the test chooses, and the test can interrupt their
//!   sequences; `fence` aborts the ones on its CPU ([`SimOs::thread`]).
//! - A fork happens in place ([`SimOs::fork`]), or only its wiping of marked pages
//!   ([`SimOs::wipe_for_fork`]).
//! - Random numbers come from a seeded generator, so a test is repeatable.
//! - The clock [`Os::ticks`] reads stands still until the test advances it
//!   ([`SimOs::advance_clock`]), so tests of idleness need no sleeping.
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

pub use cpus::{SIM_THREADS, SimThread};
pub use faults::{Call, Faults};

use self::cpus::Cpus;
use self::faults::Injector;
use self::memory::Memory;
use self::pages::{CHECKING, HUGE, NO_HUGE, PURGED, Pages, RESERVED, START, WIPE, WRITABLE};
use super::{HugePages, Os, OsError, Topology};
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

/// The shape for tests, Miri and fuzzing: 4 KiB pages, 64 KiB huge pages, regions of 256 KiB
/// (128 KiB at the floor), 232 object regions (enough for every class), 16 magazine regions
/// (2 MiB at the floor: 255 of the largest magazines, which hold 1,022 objects each, so that
/// the tests that churn many threads through many classes never run out),
/// 64 MiB of address space that the host commits only as it is touched.
///
/// The pages are host pages, so natively the simulator's memory is guarded: touching a page the
/// heap has not committed faults. A page is 1/64 of a region, so the free bitmap of the
/// smallest class fills a page of its own and can be committed apart from the rest of the
/// region's metadata. Large classes above 256 KiB do not fit a region; use [`Native`] to test
/// large classes with many slots.
pub enum Standard {}

impl Shape for Standard {
    const GEOMETRY: Geometry = Geometry::new(12, 16, 18, 17, 256, 16, MagazineSize::MAX);
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

/// The random generator's step (splitmix64's increment: the odd number nearest `2^64 / phi`).
const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// A simulated OS. See the module documentation.
pub struct SimOs<S: Shape = Standard> {
    memory: Memory,
    pages: Pages,
    faults: Injector,
    fence_registered: AtomicBool,
    last_fence: AtomicU64,
    random: AtomicU64,
    clock: AtomicU64,
    cpus: Cpus<SIM_THREADS>,
    topology: AtomicU64,
    /// What the background thread's calls did: waits, wakes, and fences of every CPU.
    sleeps: Sleeps,
    shape: PhantomData<fn() -> S>,
}

/// The simulated [`Os::wait`], [`Os::wake`] and [`Os::fence_all`]: counted, and a wait
/// returns at once, so that a test drives the thread's loop one step at a time.
struct Sleeps {
    waits: AtomicU64,
    timed: AtomicU64,
    wakes: AtomicU64,
    yields: AtomicU64,
    /// The low half of the word the last timed wait expected, plus one; zero for none.
    expected: AtomicU64,
    /// The last wait found the word as it expected, as a futex that sleeps does, and no wake
    /// has come since.
    blocked: core::sync::atomic::AtomicBool,
    fences: AtomicU64,
    /// The C library's record that the program has created a thread, as [`SimOs::set_threads`]
    /// sets it; a fork leaves it as it is, as glibc's.
    threads: core::sync::atomic::AtomicBool,
    /// Whether the calling thread is not the process's first ([`SimOs::set_other_thread`]).
    other_thread: core::sync::atomic::AtomicBool,
}

/// The CPU topology a [`SimOs`] reports through [`Os::topology`]. Its CPUs are the simulated
/// threads' numbers, `0..SIM_THREADS`, all online.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SimTopology {
    /// No topology files, as in a sandbox or without sysfs: every read fails. The default.
    Missing,
    /// L3s of `n` consecutive CPUs, listed as ranges (`0-3`).
    Blocks(u32),
    /// `n` L3s with CPU `c` in L3 `c % n`, listed one CPU at a time (`0,2,4`).
    Stripes(u32),
    /// The online list reads, but no CPU has an L3 file.
    NoL3,
    /// The L3 lists hold text that is not a CPU list.
    Garbage,
}

impl SimTopology {
    fn pack(self) -> u64 {
        match self {
            SimTopology::Missing => 0,
            SimTopology::Blocks(n) => 1 | (n as u64) << 8,
            SimTopology::Stripes(n) => 2 | (n as u64) << 8,
            SimTopology::NoL3 => 3,
            SimTopology::Garbage => 4,
        }
    }

    fn unpack(word: u64) -> SimTopology {
        let n = (word >> 8) as u32;
        match word & 0xff {
            1 => SimTopology::Blocks(n.max(1)),
            2 => SimTopology::Stripes(n.max(1)),
            3 => SimTopology::NoL3,
            4 => SimTopology::Garbage,
            _ => SimTopology::Missing,
        }
    }
}

/// Writes CPU-list text into a buffer, as the kernel prints it.
struct ListWriter<'b> {
    buf: &'b mut [u8],
    len: usize,
    fits: bool,
}

impl ListWriter<'_> {
    fn byte(&mut self, b: u8) {
        match self.buf.get_mut(self.len) {
            Some(slot) => {
                *slot = b;
                self.len += 1;
            }
            None => self.fits = false,
        }
    }

    fn number(&mut self, n: u32) {
        if n >= 10 {
            self.number(n / 10);
        }
        self.byte(b'0' + (n % 10) as u8);
    }

    fn range(&mut self, first: u32, last: u32) {
        if self.len > 0 {
            self.byte(b',');
        }
        self.number(first);
        if last != first {
            self.byte(b'-');
            self.number(last);
        }
    }

    fn done(mut self) -> Option<usize> {
        self.byte(b'\n');
        self.fits.then_some(self.len)
    }
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
            // Guarded where each simulated page is whole host pages, so a stray access faults.
            memory: Memory::new(bytes, region, geometry.page_size().is_multiple_of(4096)),
            pages: Pages::new(bytes / geometry.page_size()),
            faults: Injector::new(),
            fence_registered: AtomicBool::new(false),
            last_fence: AtomicU64::new(0),
            random: AtomicU64::new(0),
            clock: AtomicU64::new(0),
            cpus: Cpus::new(),
            topology: AtomicU64::new(0),
            sleeps: Sleeps {
                waits: AtomicU64::new(0),
                timed: AtomicU64::new(0),
                wakes: AtomicU64::new(0),
                yields: AtomicU64::new(0),
                expected: AtomicU64::new(0),
                blocked: core::sync::atomic::AtomicBool::new(false),
                fences: AtomicU64::new(0),
                threads: core::sync::atomic::AtomicBool::new(false),
                other_thread: core::sync::atomic::AtomicBool::new(false),
            },
            shape: PhantomData,
        }
    }

    /// How many [`Os::wait`] calls there have been without a timeout, and with one.
    pub fn waits(&self) -> (u64, u64) {
        (
            self.sleeps.waits.load(Ordering::Acquire),
            self.sleeps.timed.load(Ordering::Acquire),
        )
    }

    /// How many [`Os::wake`] calls there have been.
    pub fn wakes(&self) -> u64 {
        self.sleeps.wakes.load(Ordering::Acquire)
    }

    /// The low half of the word the last timed [`Os::wait`] expected, which the futex compares:
    /// a wait that finds the word so sleeps until its timeout or a wake.
    pub fn last_timed_wait(&self) -> Option<u32> {
        match self.sleeps.expected.load(Ordering::Acquire) {
            0 => None,
            e => Some((e - 1) as u32),
        }
    }

    /// Whether the last [`Os::wait`] found the word as it expected, and so would sleep until a
    /// wake or its timeout, and no [`Os::wake`] has come since.
    pub fn blocked(&self) -> bool {
        self.sleeps.blocked.load(Ordering::SeqCst)
    }

    /// How many [`Os::yield_cpu`] calls there have been.
    pub fn yields(&self) -> u64 {
        self.sleeps.yields.load(Ordering::Acquire)
    }

    /// How many [`Os::fence_all`] calls have succeeded.
    pub fn fences_of_all(&self) -> u64 {
        self.sleeps.fences.load(Ordering::Acquire)
    }

    /// Sets the C library's record that the program has created a thread of its own, which
    /// [`Os::more_than_one_thread`] answers outside a fork child.
    pub fn set_threads(&self, created: bool) {
        self.sleeps.threads.store(created, Ordering::Release);
    }

    /// Sets whether the calling thread is not the process's first, which
    /// [`Os::more_than_one_thread`] answers in a fork child.
    pub fn set_other_thread(&self, other: bool) {
        self.sleeps.other_thread.store(other, Ordering::Release);
    }

    /// Moves the clock that [`Os::ticks`] reads forward by `ticks`. It starts at zero and moves
    /// only when a test moves it.
    pub fn advance_clock(&self, ticks: u64) {
        self.clock.fetch_add(ticks, Ordering::AcqRel);
    }

    /// Sets the CPU topology [`Os::topology`] reports from now on.
    pub fn set_topology(&self, topology: SimTopology) {
        self.topology.store(topology.pack(), Ordering::Release);
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

    /// Simulated thread `index`, below [`SIM_THREADS`], to run twins on. Thread `i` starts on
    /// CPU `i`; [`Os::fence`] aborts the sequences in flight on the CPU it names.
    ///
    /// Threads on different CPUs may run free on OS threads. Threads that share a CPU must be
    /// switched cooperatively, from one OS thread: run free, two of them can both commit, which
    /// one real CPU never allows.
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
        // The thread that forked is the child's only one, and so its first.
        self.sleeps.other_thread.store(false, Ordering::Release);
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

    /// Whether every page touching `start..start + len` is reserved, has been purged since it
    /// was last committed, and still reads as zero, so is not resident: a write after the purge
    /// would have faulted it back.
    ///
    /// # Safety
    ///
    /// Nothing writes those pages during the call, and every byte written to them since they
    /// were last purged was initialized: it reads their contents, and a stored value with
    /// padding or `MaybeUninit` leaves bytes no read may inspect.
    pub unsafe fn is_purged(&self, start: NonNull<u8>, len: usize) -> bool {
        self.covering(start, len).is_some_and(|(first, count)| {
            self.pages.all(first, count, RESERVED | PURGED)
                // SAFETY: the caller's conditions, for each page in turn.
                && (first..first + count).all(|index| unsafe { self.page_reads_zero(index) })
        })
    }

    /// Bytes the heap would have resident: committed pages holding a nonzero byte. A lower
    /// bound, since a page written only with zeros counts as not resident.
    ///
    /// # Safety
    ///
    /// Nothing writes the memory during the call, and every byte of a committed page is
    /// initialized: it reads them all, and a stored value with padding or `MaybeUninit` leaves
    /// bytes no read may inspect.
    pub unsafe fn resident_bytes(&self) -> usize {
        let page = S::GEOMETRY.page_size();
        let resident = (0..self.pages.len())
            .filter(|&index| self.pages.get(index) & (RESERVED | WRITABLE) == RESERVED | WRITABLE)
            // SAFETY: the page is inside `memory` and open, and the caller promises it is
            // initialized and not written meanwhile.
            .filter(|&index| !unsafe { self.memory.is_zero(index << self.page_shift(), page) })
            .count();
        resident * page
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

    /// Panics if a page of `first..first + count` that is not committed holds a nonzero byte:
    /// the allocator wrote memory it had not committed, which Linux would have refused with a
    /// fault. Checked here when such a page is purged; `commit` checks each page it is first to
    /// commit. In guarded memory the write itself faults, so nothing is read. Unmap does not
    /// check: reading a whole reservation made Miri, which needs this check, too slow.
    fn check_untouched(&self, call: Call, first: usize, count: usize) {
        // A test aid, not part of the `Os` contract, and word reads of memory a harness wrote at
        // a symbolic byte cost CBMC minutes.
        if cfg!(kani) {
            return;
        }
        for index in first..first + count {
            if self.pages.get(index) & WRITABLE == 0 {
                self.check_page(call, index);
            }
        }
    }

    /// Whether page `index` reads as zero. A closed page of guarded memory is zero (an access
    /// would have faulted) and is not read.
    ///
    /// # Safety
    ///
    /// Nothing writes the page during the call, and its bytes are initialized.
    unsafe fn page_reads_zero(&self, index: usize) -> bool {
        if self.memory.guarded() && self.pages.get(index) & WRITABLE == 0 {
            return true;
        }
        let page = S::GEOMETRY.page_size();
        // SAFETY: the page is inside `memory` (it has a state) and accessible (open, or the
        // memory is not guarded); the caller promises it is initialized and not written.
        unsafe { self.memory.is_zero(index << self.page_shift(), page) }
    }

    /// Panics if page `index`, which is not committed, holds a nonzero byte.
    fn check_page(&self, call: Call, index: usize) {
        // Guarded memory faults at the write itself, and its uncommitted pages cannot be read.
        if cfg!(kani) || self.memory.guarded() {
            return;
        }
        let page = S::GEOMETRY.page_size();
        // SAFETY: the page is inside `memory` (it has a state). It is not committed, so no
        // correct caller writes it, and a buggy one is what this looks for: `purge` and `unmap`
        // exclude concurrent access by contract, and `commit` holds the page in `CHECKING`. Its
        // bytes are initialized: the memory starts zeroed, and every way a page stops being
        // committed zeroes it.
        let clean = unsafe { self.memory.is_zero(index << self.page_shift(), page) };
        assert!(
            clean,
            "simulated {call:?}: page {index} was written before it was committed"
        );
    }

    /// Counts a call on `count` pages and applies its fault plan: how many pages the call acts
    /// on (all, half for a partial failure, none for a whole one) and its result.
    fn admit(&self, call: Call, count: usize) -> (usize, Result<(), OsError>) {
        match self.faults.enter_range(call) {
            Ok(()) => (count, Ok(())),
            Err(failure) if failure.partial => (count / 2, Err(failure.error)),
            Err(failure) => (0, Err(failure.error)),
        }
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

/// The next number of a splitmix64 generator whose state is `state`: the next point of a Weyl
/// sequence, scrambled.
fn splitmix(state: &AtomicU64) -> u64 {
    let mut z = state.fetch_add(GAMMA, Ordering::AcqRel).wrapping_add(GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

// SAFETY: every promise of `Os` holds for the simulator.
// - Reservations are runs of pages claimed by compare-and-swap from the one allocation `memory`
//   owns, so they are disjoint from each other and from every other allocation, carry the
//   allocation's provenance, and are aligned as asked (the search steps through aligned
//   addresses). Pages that are not reserved are always zero: the allocation starts zeroed and
//   `unmap` zeroes a range before releasing its pages, with release ordering that the next
//   claim's acquire pairs with. So reserved memory reads as zero.
// - The memory is always readable and writable, so `commit` needs only to record it, and
//   changes no contents. `purge` zeroes its range. `advise_huge` only records, and `resident`
//   only reads.
// - An injected failure changes nothing, or with `Faults::partially` acts on the first half of
//   its range, never beyond it.
// - A range must lie in one live reservation of this value. `checked_range` panics before
//   touching anything if it does not: outside this value's memory, not whole reserved pages,
//   or spanning two reservations.
// - The only restartable sequences are twins on simulated threads. `fence` aborts every one in
//   flight on its CPU and waits out any that is committing or in the middle of its magazine
//   access (see `cpus.rs`), so none it aborts writes anything after it returns.
// - `wipe_on_fork` only records; nothing forks the simulator, so the promise is about what
//   `wipe_for_fork` shows a test, and it zeroes exactly the marked pages.
// - `abort` panics, so it does not return.
// - `ticks` reads a clock that only a test moves; the trait asks nothing of it.
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
        let (applied, result) = self.admit(Call::Commit, count);
        // SAFETY: `checked_range` put the pages inside `memory`.
        unsafe {
            self.memory
                .open(first << self.page_shift(), applied << self.page_shift())
        };
        for index in first..first + applied {
            // Overlapping commits may race; only the first checks a page, and it holds the page
            // in `CHECKING` until it is writable, so no correct caller writes it meanwhile.
            if self.pages.begin_commit(index) {
                self.check_page(Call::Commit, index);
            }
            self.pages.update(index, 1, WRITABLE, PURGED | CHECKING);
        }
        result
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::Purge, start, len);
        self.check_untouched(Call::Purge, first, count);
        let (applied, result) = self.admit(Call::Purge, count);
        // SAFETY: `checked_range` put the range inside `memory`, and the caller promises that
        // no thread touches it during the call.
        unsafe {
            self.memory
                .zero(first << self.page_shift(), applied << self.page_shift())
        };
        self.pages.update(first, applied, PURGED, 0);
        result
    }

    unsafe fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::AdviseHuge, start, len);
        let (applied, result) = self.admit(Call::AdviseHuge, count);
        match advice {
            HugePages::Prefer => self.pages.update(first, applied, HUGE, NO_HUGE),
            HugePages::Avoid => self.pages.update(first, applied, NO_HUGE, HUGE),
        }
        result
    }

    unsafe fn resident(&self, start: NonNull<u8>, len: usize) -> Result<usize, OsError> {
        let (first, count) = self.checked_range(Call::Resident, start, len);
        self.admit(Call::Resident, count).1?;
        // A page Linux would have resident is one written since it was committed or purged.
        // The simulator sees writes only by what they leave, so it counts a committed page
        // that holds a nonzero byte, as `resident_bytes` does.
        let resident = (first..first + count)
            .filter(|&index| self.pages.get(index) & WRITABLE != 0)
            // SAFETY: the caller promises nothing writes the range during the call, and the
            // allocator stores only integers, so its bytes are initialized.
            .filter(|&index| !unsafe { self.page_reads_zero(index) })
            .count();
        Ok(resident)
    }

    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::WipeOnFork, start, len);
        let (applied, result) = self.admit(Call::WipeOnFork, count);
        self.pages.update(first, applied, WIPE, 0);
        result
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let (first, count) = self.checked_range(Call::Unmap, start, len);
        let (applied, result) = self.admit(Call::Unmap, count);
        if applied == 0 {
            return result;
        }
        // What remains of a reservation after its head is unmapped is a reservation of its own,
        // as munmap leaves it. This is decided while the range is still reserved: then a
        // reserved page right after the unmapped part without `START` can only be this
        // reservation's tail, never a reservation another thread is making. Doing it after the
        // release would race with such a thread.
        let next = first + applied;
        if next < self.pages.len() {
            self.pages.mark_start(next);
        }
        // SAFETY: `checked_range` put the range inside `memory`, and the caller promises that
        // nothing touches it once the call begins. Zeroing keeps unreserved memory zero.
        unsafe {
            self.memory
                .zero(first << self.page_shift(), applied << self.page_shift());
            self.memory
                .close(first << self.page_shift(), applied << self.page_shift());
        }
        self.pages.release(first, applied);
        result
    }

    fn register_fence(&self) -> Result<(), OsError> {
        self.faults.enter(Call::RegisterFence)?;
        self.fence_registered.store(true, Ordering::Release);
        Ok(())
    }

    fn fence_all(&self) -> Result<(), OsError> {
        if self.fence_registered() {
            self.sleeps.fences.fetch_add(1, Ordering::AcqRel);
            Ok(())
        } else {
            Err(OsError::EPERM)
        }
    }

    fn wait(&self, word: &core::sync::atomic::AtomicU64, expected: u64, timeout_ns: Option<u64>) {
        let sleeps = word.load(Ordering::SeqCst) as u32 == expected as u32;
        self.sleeps.blocked.store(sleeps, Ordering::SeqCst);
        // A futex that sleeps waits for a wake: under the interleaving checker, the other
        // threads run first, for a bounded while, and a wait no wake reached ends as a timeout
        // would, with `blocked` left set for the check after the execution.
        #[cfg(test)]
        for _ in 0..64 {
            if !self.sleeps.blocked.load(Ordering::SeqCst) {
                break;
            }
            crate::front::check::wait_point();
        }
        if timeout_ns.is_some() {
            self.sleeps
                .expected
                .store(u64::from(expected as u32) + 1, Ordering::Release);
        }
        let count = match timeout_ns {
            Some(_) => &self.sleeps.timed,
            None => &self.sleeps.waits,
        };
        count.fetch_add(1, Ordering::AcqRel);
    }

    fn wake(&self, _: &core::sync::atomic::AtomicU64) {
        self.sleeps.blocked.store(false, Ordering::SeqCst);
        self.sleeps.wakes.fetch_add(1, Ordering::AcqRel);
    }

    fn yield_cpu(&self) {
        self.sleeps.yields.fetch_add(1, Ordering::AcqRel);
        // Where another thread could take the CPU, the interleaving checker may switch.
        #[cfg(test)]
        crate::front::check::switch_point();
    }

    fn more_than_one_thread(&self, forked: bool) -> bool {
        if forked {
            self.sleeps.other_thread.load(Ordering::Acquire)
        } else {
            self.sleeps.threads.load(Ordering::Acquire)
        }
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

    fn random(&self) -> u64 {
        splitmix(&self.random)
    }

    fn ticks(&self) -> u64 {
        self.clock.load(Ordering::Acquire)
    }

    fn topology(&self, which: Topology, buf: &mut [u8]) -> Option<usize> {
        let topology = SimTopology::unpack(self.topology.load(Ordering::Acquire));
        let last = SIM_THREADS as u32 - 1;
        let mut out = ListWriter {
            buf,
            len: 0,
            fits: true,
        };
        match (topology, which) {
            (SimTopology::Missing, _) => return None,
            (_, Topology::Online) => out.range(0, last),
            (_, Topology::SharedL3(cpu)) if cpu > last => return None,
            (SimTopology::NoL3, Topology::SharedL3(_)) => return None,
            (SimTopology::Garbage, Topology::SharedL3(_)) => {
                for &b in b"0-x" {
                    out.byte(b);
                }
            }
            (SimTopology::Blocks(n), Topology::SharedL3(cpu)) => {
                let first = cpu / n * n;
                out.range(first, (first + n - 1).min(last));
            }
            (SimTopology::Stripes(n), Topology::SharedL3(cpu)) => {
                let mut other = cpu % n;
                while other <= last {
                    out.range(other, other);
                    other += n;
                }
            }
        }
        out.done()
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
        // SAFETY: each call's range is whole reserved pages, nothing else touches them, and
        // they are written only with bytes.
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

    /// Ordinary memory (the tiny shape's pages are smaller than a host page, so it is never
    /// guarded) cannot fault, so the simulator checks contents at the first commit.
    #[test]
    #[should_panic(expected = "written before it was committed")]
    fn writing_before_committing_is_caught_at_commit_in_ordinary_memory() {
        let os = SimOs::<Tiny>::new();
        let page = Tiny::GEOMETRY.page_size();
        let base = os.reserve(page, page).unwrap();
        // SAFETY: deliberately breaks the rule; on Linux this write would fault.
        unsafe {
            base.as_ptr().write(7);
            os.commit(base, page).unwrap();
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri)))]
    const GUARD_CHILD: &str = "ASMALLOC_GUARD_CHILD";

    /// Run on its own by `touching_uncommitted_guarded_memory_faults`; does nothing otherwise.
    /// Writes a page it reserved but never committed, and a page it committed, purged and
    /// unmapped, which must fault like the first.
    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri)))]
    #[test]
    fn guard_child() {
        let Some(mode) = std::env::var_os(GUARD_CHILD) else {
            return;
        };
        let os = Sim::with_capacity(region());
        let base = os.reserve(2 * page(), region()).unwrap();
        let target = if mode == "unmapped" {
            // SAFETY: whole reserved pages that nothing else uses.
            unsafe {
                os.commit(base, 2 * page()).unwrap();
                base.as_ptr().write(1);
                os.unmap(base, 2 * page()).unwrap();
            }
            base
        } else {
            base.map_addr(|a| a.saturating_add(page()))
        };
        std::eprintln!("about to write");
        // SAFETY: deliberately breaks the rule, to see the fault.
        unsafe { target.as_ptr().write(7) };
        std::eprintln!("the write did not fault");
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri)))]
    #[test]
    fn touching_uncommitted_guarded_memory_faults() {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            Sim::new().memory.guarded(),
            "the standard shape is guarded natively"
        );
        for mode in ["never-committed", "unmapped"] {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg("ulimit -c 0; exec \"$0\" \"$@\"")
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "os::sim::tests::guard_child", "--nocapture"])
                .env(GUARD_CHILD, mode)
                .output()
                .unwrap();
            let stderr = std::string::String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("about to write"), "{mode}: {stderr}");
            assert!(!stderr.contains("did not fault"), "{mode}: {stderr}");
            assert_eq!(output.status.signal(), Some(11), "{mode}: {output:?}");
        }
    }

    #[test]
    fn a_partial_fault_acts_on_half_the_range() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(4 * page(), region()).unwrap();
        let third = base.map_addr(|a| a.saturating_add(2 * page()));
        os.set_faults(Call::Commit, Faults::once_after(0).partially());
        // SAFETY: whole reserved pages that nothing else uses.
        unsafe {
            assert_eq!(os.commit(base, 4 * page()), Err(OsError::ENOMEM));
        }
        assert!(os.is_writable(base, 2 * page()));
        assert!(!os.is_writable(third, page()));
        assert_eq!(os.faults(Call::Commit), Faults::NONE.partially());
        // SAFETY: as above.
        unsafe {
            os.set_faults(Call::Unmap, Faults::once_after(0).partially());
            assert!(os.unmap(base, 4 * page()).is_err());
        }
        assert!(!os.is_reserved(base, page()));
        assert!(os.is_reserved(third, 2 * page()));
        // The remaining half is a reservation of its own.
        // SAFETY: as above.
        unsafe { os.commit(third, 2 * page()).unwrap() };
    }

    #[test]
    fn purged_memory_stops_counting_as_purged_once_written() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(2 * page(), region()).unwrap();
        // SAFETY: whole reserved pages that nothing else uses.
        unsafe {
            os.commit(base, 2 * page()).unwrap();
            base.as_ptr().write(1);
            base.as_ptr().add(page()).write(1);
        }
        // SAFETY: whole reserved pages; this thread alone writes them, and only with bytes,
        // as the queries need.
        unsafe {
            assert_eq!(os.resident_bytes(), 2 * page());
            os.purge(base, page()).unwrap();
            assert!(os.is_purged(base, page()));
            assert_eq!(os.resident_bytes(), page());
            base.as_ptr().write(3);
            assert!(
                !os.is_purged(base, page()),
                "a write faults the page back in"
            );
            assert_eq!(os.resident_bytes(), 2 * page());
        }
    }

    /// `resident` counts the committed pages a write left nonzero, as Linux counts those
    /// written since the last purge; it fails as injected.
    #[test]
    fn resident_counts_written_pages() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(4 * page(), region()).unwrap();
        let p = base.as_ptr();
        // SAFETY: whole reserved pages; this thread alone writes them, and only with bytes,
        // as the queries need.
        unsafe {
            os.commit(base, 3 * page()).unwrap();
            assert_eq!(os.resident(base, 4 * page()), Ok(0));
            p.write(1);
            p.add(2 * page() + 5).write(1);
            assert_eq!(os.resident(base, 4 * page()), Ok(2));
            os.purge(base, page()).unwrap();
            assert_eq!(os.resident(base, 4 * page()), Ok(1));
            os.set_faults(Call::Resident, Faults::once_after(0));
            assert_eq!(os.resident(base, page()), Err(OsError::ENOMEM));
        }
    }

    /// Review R1: a page marked to be wiped on fork can still be committed (the marks once
    /// shared a bit with a commit in progress, and the commit never returned).
    #[test]
    fn a_page_marked_to_be_wiped_can_be_committed() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(page(), page()).unwrap();
        // SAFETY: whole reserved pages that nothing else uses.
        unsafe {
            os.wipe_on_fork(base, page()).unwrap();
            os.commit(base, page()).unwrap();
        }
        assert!(os.is_writable(base, page()));
    }

    /// Review R2: committing a marked page again keeps the mark, as on Linux.
    #[test]
    fn a_second_commit_keeps_the_wipe_mark() {
        let os = Sim::with_capacity(region());
        let base = os.reserve(page(), page()).unwrap();
        // SAFETY: whole reserved pages that nothing else uses.
        unsafe {
            os.commit(base, page()).unwrap();
            os.wipe_on_fork(base, page()).unwrap();
            os.commit(base, page()).unwrap();
            base.as_ptr().write(7);
            os.fork(0);
            assert_eq!(
                base.as_ptr().read(),
                0,
                "the child reads a marked page as zero"
            );
        }
    }

    /// Review R7: a drain in miniature on free-running threads. If the drainer still finds its
    /// marker after the fence, no twin may have committed over it.
    #[test]
    fn no_twin_commits_over_a_marker_the_fence_left_in_place() {
        use crate::os::{Machine, Step};
        use core::sync::atomic::AtomicUsize;
        const OLD: usize = 0x1000;
        let marker = crate::classes::MARKER;
        let os = SimOs::<Tiny>::new();
        os.register_fence().unwrap();
        let slab = AtomicUsize::new(OLD);
        let (owned, committed) = thread::scope(|scope| {
            let drainer = scope.spawn(|| {
                let old = slab.swap(marker, Ordering::AcqRel);
                os.fence(0).unwrap();
                old == OLD && slab.load(Ordering::Acquire) == marker
            });
            let twin = scope.spawn(|| {
                let t = os.thread(0);
                loop {
                    if t.step(Step::ReadCpu).is_err() || t.step(Step::Load).is_err() {
                        continue;
                    }
                    let word = slab.load(Ordering::Acquire);
                    if word == marker {
                        return false;
                    }
                    if t.step(Step::Commit).is_err() {
                        continue;
                    }
                    slab.store(word + 8, Ordering::Release);
                    t.committed();
                    return true;
                }
            });
            (drainer.join().unwrap(), twin.join().unwrap())
        });
        assert!(
            !(owned && committed),
            "the drainer owned the word and a twin committed over it"
        );
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
    fn random_numbers_differ() {
        let os = Sim::new();
        let mut distinct: Vec<u64> = (0..4).map(|_| os.random()).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 4);
    }

    #[test]
    fn the_clock_moves_only_when_advanced() {
        let os = Sim::new();
        assert_eq!(os.ticks(), 0);
        assert_eq!(os.ticks(), 0);
        os.advance_clock(5);
        os.advance_clock(7);
        assert_eq!(os.ticks(), 12);
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
