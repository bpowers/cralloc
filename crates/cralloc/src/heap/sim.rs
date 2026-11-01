//! A heap over the simulated OS, run with twins: what Miri, and tests that want a fresh or
//! instrumented heap, use instead of the process's heap.
//!
//! The twins run on the simulator's threads (`SimOs::thread`), each on a CPU of its own, whose
//! sequences the simulator's fence aborts as the kernel's would: once it returns, a sequence it
//! aborted writes nothing more. A real thread borrows one of
//! them for the length of one call (a *lease*, claimed with a compare-and-swap) and gives it back
//! at the end, so no simulated thread is ever used by two real threads at once, and the twins'
//! sequences on one CPU never interleave. A real thread that finds every simulated one taken
//! runs as a thread without an rseq registration, straight on the back end: it never waits.
//!
//! The heap here is test scaffolding: its unsafe code has no ledger entries, and its
//! `SAFETY` comments say why each block is sound.

use super::{Bound, Heap, Zero};
use crate::depot::Drained;
use crate::front::{Cpu, Field, Routines, Slab, Slabs, StackPop, StackPush, StoreIfEqual, Twin};
use crate::os::sim::{SIM_THREADS, Shape, SimOs, SimThread, Standard};
use crate::os::{Aborted, Machine, Step};
use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

/// A heap over a [`SimOs`], with its own slabs, run with twins. See the module documentation.
pub struct SimHeap<S: Shape = Standard> {
    // Boxed, so that a test that keeps a heap on its stack does not keep the back end's state
    // there too: with every region's words it is hundreds of kilobytes.
    pub(super) heap: Box<Heap<SimOs<S>>>,
    slabs: Box<Slabs<[Slab; SIM_THREADS]>>,
    leases: [AtomicBool; SIM_THREADS],
}

/// The processor a lease gives a call: a simulated thread, or none.
#[derive(Clone, Copy)]
enum Processor<'a> {
    Thread(SimThread<'a>),
    Unregistered,
}

impl Machine for Processor<'_> {
    fn cpu_id(&self) -> i32 {
        match self {
            Processor::Thread(thread) => thread.cpu_id(),
            Processor::Unregistered => -2,
        }
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        match self {
            Processor::Thread(thread) => thread.step(step),
            Processor::Unregistered => Ok(()),
        }
    }

    fn accessed(&self) {
        if let Processor::Thread(thread) = self {
            thread.accessed();
        }
    }

    fn committed(&self) {
        if let Processor::Thread(thread) = self {
            thread.committed();
        }
    }
}

/// The twins on a heap's slabs, for one call: a copyable form of [`Twin`].
#[derive(Clone, Copy)]
pub(super) struct SimRoutines<'a> {
    slabs: &'a Slabs,
    processor: Processor<'a>,
}

impl<'a> SimRoutines<'a> {
    fn twin(self) -> Twin<'a, Processor<'a>> {
        Twin::new(self.slabs, self.processor)
    }
}

// SAFETY: every method is the twin's, on the same slabs and processor, so each keeps the
// twin's contract for the simulated thread, whose machine the simulator keeps.
unsafe impl Routines for SimRoutines<'_> {
    fn slabs(&self) -> &Slabs {
        self.slabs
    }

    fn cpu(&self) -> Option<Cpu> {
        self.twin().cpu()
    }

    fn pop(&self, class: crate::classes::SmallClass) -> Option<NonNull<u8>> {
        self.twin().pop(class)
    }

    fn push(&self, class: crate::classes::SmallClass, object: NonNull<u8>) -> bool {
        self.twin().push(class, object)
    }

    unsafe fn store_if_equal(&self, field: Field, expected: *mut u8, new: *mut u8) -> StoreIfEqual {
        // SAFETY: the twin's method has the trait's contract, on these same slabs.
        unsafe { self.twin().store_if_equal(field, expected, new) }
    }

    unsafe fn stack_push(&self, field: Field, magazine: NonNull<u8>) -> StackPush {
        // SAFETY: the twin's method has the trait's contract, on these same slabs.
        unsafe { self.twin().stack_push(field, magazine) }
    }

    fn stack_pop(&self, field: Field) -> StackPop {
        self.twin().stack_pop(field)
    }
}

/// A simulated thread borrowed for one call; given back when dropped.
pub(super) struct Lease<'a> {
    taken: &'a AtomicBool,
    index: usize,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.taken.store(false, Ordering::Release);
    }
}

impl<S: Shape> SimHeap<S> {
    /// A heap over a fresh simulated OS, not yet started. The OS has room for one reservation
    /// of the largest plan, so threads that start the heap together race for it as they do for
    /// the few that fit in a real address space (`SpaceCell::get_or_reserve`).
    pub fn new() -> SimHeap<S> {
        SimHeap::with_os(SimOs::new())
    }

    /// How many magazines the heap's magazine area could not make, so that a test meant to
    /// exercise the magazines can check that none of its calls went to the back end instead.
    #[cfg(test)]
    pub(crate) fn refused_magazines(&self) -> usize {
        self.heap.depot.refused_magazines()
    }

    /// A heap over `os`, not yet started.
    pub fn with_os(os: SimOs<S>) -> SimHeap<S> {
        SimHeap {
            heap: Box::new(Heap::new(os)),
            slabs: Box::new(Slabs::new()),
            leases: [const { AtomicBool::new(false) }; SIM_THREADS],
        }
    }

    /// The simulated OS, for tests that inject faults or inspect memory.
    pub fn os(&self) -> &SimOs<S> {
        self.heap.os()
    }

    /// Whether `ptr` lies in the heap's object regions.
    pub fn contains(&self, ptr: *const u8) -> bool {
        self.heap.contains(ptr)
    }

    /// The bytes the block starting at `ptr` may use, if a slot or run starts there.
    pub fn usable_size(&self, ptr: *const u8) -> Option<usize> {
        self.heap.usable_size(ptr)
    }

    /// The heap itself, for tests that call what the C layer calls or read the back end.
    #[cfg(test)]
    pub(crate) fn heap(&self) -> &Heap<SimOs<S>> {
        &self.heap
    }

    /// Walks the heap for `visitor`, as `Cralloc::inspect` does the process's heap.
    pub fn inspect(&self, visitor: &mut impl crate::stats::Visitor) {
        self.with(|heap| heap.inspect(visitor));
    }

    /// Returns everything the heap can return now, as `Cralloc::collect` does for the
    /// process's heap, on a leased simulated thread. Returns the bytes purged that the heap
    /// believed resident.
    pub fn collect(&self) -> usize {
        self.with(|heap| heap.collect())
    }

    /// One round of the heap's clocked maintenance, as `Cralloc::maintain` does for the
    /// process's heap, on a leased simulated thread.
    pub fn maintain(&self) {
        self.with(|heap| heap.maintain());
    }

    /// Borrows a simulated thread that no other call is using, starting the search at one
    /// picked from the caller's stack address, so that concurrent callers spread out.
    pub(super) fn lease(&self) -> Option<Lease<'_>> {
        let here = 0u8;
        let start = (ptr::from_ref(&here).addr() >> 12) % SIM_THREADS;
        (0..SIM_THREADS)
            .map(|step| (start + step) % SIM_THREADS)
            .find_map(|index| {
                let taken = &self.leases[index];
                // Lazily: a `Lease` dropped on a failed claim would release another caller's.
                taken
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                    .then(|| Lease { taken, index })
            })
    }

    /// Drains the slab of simulated CPU `cpu` now, as the heartbeat's idle check would once the
    /// slab had been quiet for an interval, and says how the drain ended. `None` if the heap
    /// has not started or there is no such CPU.
    ///
    /// A drain is safe at any moment, so a test that wants drains to race the threads using a
    /// slab asks for them here, where the idle check, paced by the clock and by how often slow
    /// paths look, would give it a handful.
    pub fn drain(&self, cpu: usize) -> Option<Drained> {
        self.with(|heap| heap.drain(cpu))
    }

    /// Grows the block at `ptr` from `old` to `new` with the bytes past `old` reading as zero,
    /// by what `Allocator::grow_zeroed` runs (`Bound::grow_block`), for tests on stable Rust.
    /// Returns the block and the length the caller may use, or `None` if there was no memory.
    ///
    /// # Safety
    ///
    /// As for `GlobalAlloc::realloc` with `ptr` and `old`, which is not empty; `new` is at
    /// least as large.
    #[cfg(test)]
    pub(crate) unsafe fn grow_zeroed_for_test(
        &self,
        ptr: *mut u8,
        old: Layout,
        new: Layout,
    ) -> Option<(*mut u8, usize)> {
        let ptr = NonNull::new(ptr)?;
        // SAFETY: `ptr` is a live block of this heap that `old` describes, which the caller
        // gives up unless the grow fails, and `old` is not empty.
        let grant = self.with(|heap| unsafe { heap.grow_block(ptr, old, new, true) })?;
        Some((grant.ptr.as_ptr(), grant.len))
    }

    /// Shrinks the block at `ptr` from `old` to `new`, by what `Allocator::shrink` runs for a
    /// size that is not zero (`Bound::shrink_block`), for tests on stable Rust. Returns the
    /// block and the length the caller may use, or `None` if there was no memory.
    ///
    /// # Safety
    ///
    /// As for `GlobalAlloc::realloc` with `ptr` and `old`; `new` is not empty and at most as
    /// large.
    #[cfg(test)]
    pub(crate) unsafe fn shrink_for_test(
        &self,
        ptr: *mut u8,
        old: Layout,
        new: Layout,
    ) -> Option<(*mut u8, usize)> {
        let ptr = NonNull::new(ptr)?;
        // SAFETY: as in `grow_zeroed_for_test`, and `new` is not empty.
        let grant = self.with(|heap| unsafe { heap.shrink_block(ptr, old, new) })?;
        Some((grant.ptr.as_ptr(), grant.len))
    }

    /// Runs `f` with the heap and the twins of a leased simulated thread.
    pub(super) fn with<T>(&self, f: impl FnOnce(Bound<'_, SimOs<S>, SimRoutines<'_>>) -> T) -> T {
        let lease = self.lease();
        let processor = match &lease {
            Some(lease) => Processor::Thread(self.heap.os().thread(lease.index)),
            None => Processor::Unregistered,
        };
        let routines = SimRoutines {
            slabs: &*self.slabs,
            processor,
        };
        // SAFETY: the twins run on `self.slabs`, which this heap owns, never hands out and
        // pairs only with `self.heap`, so every `Bound` over those slabs names this heap.
        let result = f(unsafe { Bound::new(&self.heap, routines) });
        drop(lease);
        result
    }
}

impl<S: Shape> Default for SimHeap<S> {
    fn default() -> SimHeap<S> {
        SimHeap::new()
    }
}

// SAFETY: as the process heap's `GlobalAlloc` and `Allocator`, for this heap and its twins.
// Every method is the heap's, with twins on this heap's own slabs.
unsafe impl<S: Shape> GlobalAlloc for SimHeap<S> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with(|heap| heap.alloc(layout))
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.with(|heap| {
            heap.alloc_grant(layout, Zero::Requested)
                .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the trait method's own contract, for a block this heap returned.
        self.with(|heap| unsafe { heap.dealloc(ptr, layout) })
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the trait method's own contract, for a block this heap returned.
        self.with(|heap| unsafe { heap.realloc(ptr, layout, new_size) })
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: as the process heap's `GlobalAlloc` and `Allocator`, for this heap and its twins.
unsafe impl<S: Shape> core::alloc::Allocator for SimHeap<S> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        self.with(|heap| heap.allocate(layout, false))
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        self.with(|heap| heap.allocate(layout, true))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: the trait method's own contract, for a block this heap returned.
        self.with(|heap| unsafe { heap.deallocate(ptr, layout) })
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the trait method's own contract, for a block this heap returned.
        self.with(|heap| unsafe { heap.grow(ptr, old, new, false) })
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the trait method's own contract, for a block this heap returned.
        self.with(|heap| unsafe { heap.grow(ptr, old, new, true) })
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the trait method's own contract, for a block this heap returned.
        self.with(|heap| unsafe { heap.shrink(ptr, old, new) })
    }
}

/// A zero-sized handle to one [`SimHeap`] shared by the whole process, made on first use: the
/// simulated counterpart of `Cralloc`, for code that needs a handle with no state (the test
/// kit's target under Miri).
#[derive(Clone, Copy, Debug, Default)]
pub struct SimCralloc;

/// The shared heap, or null before the first use.
static SHARED: AtomicPtr<SimHeap> = AtomicPtr::new(ptr::null_mut());

impl SimCralloc {
    /// The shared heap, made by the first caller. Racing callers each make one and the loser
    /// drops its own.
    pub fn heap() -> &'static SimHeap {
        let mut current = SHARED.load(Ordering::Acquire);
        if current.is_null() {
            let mine = Box::into_raw(Box::new(SimHeap::new()));
            match SHARED.compare_exchange(
                ptr::null_mut(),
                mine,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => current = mine,
                Err(winner) => {
                    // SAFETY: `mine` is this thread's until published, and a published heap
                    // is never freed.
                    drop(unsafe { Box::from_raw(mine) });
                    current = winner;
                }
            }
        }
        // SAFETY: `mine` is this thread's until published, and a published heap is never
        // freed.
        unsafe { &*current }
    }
}

// SAFETY: as the process heap's `GlobalAlloc` and `Allocator`, for the one shared heap and its
// twins.
unsafe impl GlobalAlloc for SimCralloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        SimCralloc::heap().with(|heap| heap.alloc(layout))
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        SimCralloc::heap().with(|heap| {
            heap.alloc_grant(layout, Zero::Requested)
                .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the trait method's own contract, for a block of the one shared heap.
        unsafe { SimCralloc::heap().dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the trait method's own contract, for a block of the one shared heap.
        unsafe { SimCralloc::heap().realloc(ptr, layout, new_size) }
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: as the process heap's `GlobalAlloc` and `Allocator`, for the one shared heap and its
// twins.
unsafe impl core::alloc::Allocator for SimCralloc {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        SimCralloc::heap().allocate(layout)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        SimCralloc::heap().allocate_zeroed(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: the trait method's own contract, for a block of the one shared heap.
        unsafe { SimCralloc::heap().deallocate(ptr, layout) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the trait method's own contract, for a block of the one shared heap.
        unsafe { SimCralloc::heap().grow(ptr, old, new) }
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the trait method's own contract, for a block of the one shared heap.
        unsafe { SimCralloc::heap().grow_zeroed(ptr, old, new) }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the trait method's own contract, for a block of the one shared heap.
        unsafe { SimCralloc::heap().shrink(ptr, old, new) }
    }
}
