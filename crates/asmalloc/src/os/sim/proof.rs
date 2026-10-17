//! A simulated OS cheap enough for Kani.
//!
//! [`super::SimOs`] keeps an atomic per page, so a Kani harness that so much as starts a heap
//! on it unrolls loops over every page and runs out of memory. [`ProofOs`] holds one reservation
//! at a time, at the start of its memory, and keeps a fixed handful of words about it, whatever
//! its size: the reservation's length, one range marked to be wiped on fork, the fault plans and
//! the simulated threads. Its memory is that of one reservation of the shape's largest plan.
//!
//! What it gives up: it does not record which pages are committed, purged or advised, so it
//! cannot catch a write to uncommitted memory, and it unmaps only whole reservations. Tests that
//! need those use `SimOs`; proofs about the heap's logic use this.

use super::cpus::{Cpus, SimThread};
use super::faults::Injector;
use super::memory::Memory;
use super::{Call, DEFAULT_RSEQ, Faults, Shape, Tiny, capacity_for, splitmix};
use crate::os::{HugePages, Os, OsError, RseqArea};
use crate::space::Geometry;
use core::fmt;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The number of simulated threads a [`ProofOs`] has.
pub const PROOF_THREADS: usize = 2;

/// A simulated OS for Kani harnesses. See the module documentation.
pub struct ProofOs<S: Shape = Tiny> {
    memory: Memory,
    /// The reservation's length; zero when there is none. It starts at offset 0.
    reserved: AtomicUsize,
    /// The range marked to be wiped on fork, as an offset and a length; a length of zero means
    /// none.
    wipe_offset: AtomicUsize,
    wipe_len: AtomicUsize,
    faults: Injector,
    fence_registered: AtomicBool,
    random: AtomicU64,
    cpus: Cpus<PROOF_THREADS>,
    shape: PhantomData<fn() -> S>,
}

impl<S: Shape> ProofOs<S> {
    /// A simulated OS with room for one reservation of the shape's largest plan.
    pub fn new() -> ProofOs<S> {
        let region = S::GEOMETRY.largest_plan().region_size();
        ProofOs {
            memory: Memory::new(capacity_for::<S>(), region, false),
            reserved: AtomicUsize::new(0),
            wipe_offset: AtomicUsize::new(0),
            wipe_len: AtomicUsize::new(0),
            faults: Injector::new(),
            fence_registered: AtomicBool::new(false),
            random: AtomicU64::new(0),
            cpus: Cpus::new(),
            shape: PhantomData,
        }
    }

    /// Sets when `call` fails from now on. A partial failure acts on nothing here except for
    /// `purge`, which zeroes the first half of its range.
    pub fn set_faults(&self, call: Call, faults: Faults) {
        self.faults.set(call, faults);
    }

    /// Simulated thread `index`, below [`PROOF_THREADS`], to run twins on.
    pub fn thread(&self, index: usize) -> SimThread<'_> {
        self.cpus.thread(index)
    }

    /// Forks in place, as [`super::SimOs::fork`] does: the marked range reads as zero and every
    /// thread but `survivor` vanishes.
    ///
    /// # Safety
    ///
    /// No thread reads or writes the marked range during the call.
    pub unsafe fn fork(&self, survivor: usize) {
        let len = self.wipe_len.load(Ordering::Acquire);
        if len != 0 {
            // SAFETY: `wipe_on_fork` recorded a range inside the reservation, which is inside
            // `memory`, and the caller promises nothing touches it meanwhile.
            unsafe {
                self.memory
                    .zero(self.wipe_offset.load(Ordering::Acquire), len)
            };
        }
        self.cpus.fork(survivor);
    }

    /// The offset of a range a memory call was given, which must be whole pages inside the
    /// reservation. Panics otherwise.
    fn checked(&self, call: Call, start: NonNull<u8>, len: usize) -> usize {
        let page = S::GEOMETRY.page_size();
        let offset = self.memory.offset_of(start);
        let reserved = self.reserved.load(Ordering::Acquire);
        assert!(
            len != 0
                && offset < reserved
                && len <= reserved - offset
                && (offset | len) & (page - 1) == 0,
            "simulated {call:?} on a range that is not whole pages of the reservation"
        );
        offset
    }
}

impl<S: Shape> Default for ProofOs<S> {
    fn default() -> ProofOs<S> {
        ProofOs::new()
    }
}

impl<S: Shape> fmt::Debug for ProofOs<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofOs")
            .field("reserved", &self.reserved.load(Ordering::Acquire))
            .finish()
    }
}

// SAFETY: every promise of `Os` holds.
// - There is at most one reservation, claimed by compare-and-swap of `reserved` from zero, so
//   it overlaps no other; it is the start of `memory`, which this value owns, aligned to the
//   largest region size, which is the most any plan asks for, and it carries the allocation's
//   provenance. Memory outside a reservation is zero: the allocation starts zeroed, and `unmap`
//   zeroes the reservation before releasing it.
// - `commit` and `advise_huge` change nothing; `purge` zeroes (the first half of) its range;
//   `wipe_on_fork` only records. A failed call has acted on no more than its range.
// - A range must lie in the reservation; `checked` panics before touching anything otherwise.
// - `fence` aborts every twin sequence in flight on its CPU, waiting out committing ones.
// - `abort` panics. All state is atomic.
unsafe impl<S: Shape> Os for ProofOs<S> {
    const GEOMETRY: Geometry = S::GEOMETRY;

    fn reserve(&self, len: usize, align: usize) -> Result<NonNull<u8>, OsError> {
        self.faults.enter(Call::Reserve)?;
        let page = S::GEOMETRY.page_size();
        let largest = S::GEOMETRY.largest_plan().region_size();
        if len == 0 || !len.is_multiple_of(page) || !align.is_power_of_two() || align < page {
            return Err(OsError::EINVAL);
        }
        if len > self.memory.len() || align > largest {
            return Err(OsError::ENOMEM);
        }
        match self
            .reserved
            .compare_exchange(0, len, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(self.memory.at(0)),
            Err(_) => Err(OsError::ENOMEM),
        }
    }

    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        self.checked(Call::Commit, start, len);
        self.faults.enter(Call::Commit)
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let offset = self.checked(Call::Purge, start, len);
        let page = S::GEOMETRY.page_size();
        let (zeroed, result) = match self.faults.enter_range(Call::Purge) {
            Ok(()) => (len, Ok(())),
            Err(failure) if failure.partial => ((len / page / 2) * page, Err(failure.error)),
            Err(failure) => (0, Err(failure.error)),
        };
        // SAFETY: `checked` put the range inside the reservation, inside `memory`, and the
        // caller promises no thread touches it during the call.
        unsafe { self.memory.zero(offset, zeroed) };
        result
    }

    unsafe fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        _advice: HugePages,
    ) -> Result<(), OsError> {
        self.checked(Call::AdviseHuge, start, len);
        self.faults.enter(Call::AdviseHuge)
    }

    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let offset = self.checked(Call::WipeOnFork, start, len);
        self.faults.enter(Call::WipeOnFork)?;
        self.wipe_offset.store(offset, Ordering::Release);
        self.wipe_len.store(len, Ordering::Release);
        Ok(())
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        let offset = self.checked(Call::Unmap, start, len);
        assert!(
            offset == 0 && len == self.reserved.load(Ordering::Acquire),
            "the proof simulator unmaps whole reservations only"
        );
        self.faults.enter(Call::Unmap)?;
        // SAFETY: the reservation is inside `memory`, and the caller promises nothing touches
        // it once the call begins. Zeroing keeps unreserved memory zero.
        unsafe { self.memory.zero(0, len) };
        self.wipe_len.store(0, Ordering::Release);
        self.reserved.store(0, Ordering::Release);
        Ok(())
    }

    fn register_fence(&self) -> Result<(), OsError> {
        self.faults.enter(Call::RegisterFence)?;
        self.fence_registered.store(true, Ordering::Release);
        Ok(())
    }

    fn fence(&self, cpu: u32) -> Result<(), OsError> {
        self.faults.enter(Call::Fence)?;
        if self.fence_registered.load(Ordering::Acquire) {
            self.cpus.fence(cpu);
            Ok(())
        } else {
            Err(OsError::EPERM)
        }
    }

    fn rseq(&self) -> Option<RseqArea> {
        DEFAULT_RSEQ
    }

    fn random(&self) -> u64 {
        splitmix(&self.random)
    }

    fn abort(&self, message: &str) -> ! {
        panic!("asmalloc: {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{Space, SpaceCell};

    #[test]
    fn a_heap_starts_commits_and_is_unmapped() {
        let cell = SpaceCell::new(ProofOs::<Tiny>::new());
        let space = cell.get_or_reserve().unwrap();
        let page = Tiny::GEOMETRY.page_size();
        let meta = space.metadata(space.plan().region(0).unwrap());
        space.commit(meta, page).unwrap();
        // SAFETY: committed, and nothing else uses it.
        unsafe {
            meta.as_ptr().write(9);
            space.purge(meta, page).unwrap();
            assert_eq!(meta.as_ptr().read(), 0);
        }
        assert_eq!(
            Space::reserve(cell.os()),
            Err(OsError::ENOMEM),
            "one reservation at a time"
        );
    }

    #[test]
    fn the_process_page_is_wiped_by_a_fork() {
        let os = ProofOs::<Tiny>::new();
        let space = Space::reserve(&os).unwrap();
        let page = space.process_page();
        let size = Tiny::GEOMETRY.page_size();
        space.commit(page, size).unwrap();
        space.wipe_on_fork(page, size).unwrap();
        // SAFETY: nothing else uses the page.
        unsafe {
            page.as_ptr().write(4);
            os.fork(0);
            assert_eq!(page.as_ptr().read(), 0);
        }
    }
}
