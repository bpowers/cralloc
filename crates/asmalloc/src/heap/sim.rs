//! A heap over the simulated OS, run with twins: what Miri, and tests that want a fresh or
//! instrumented heap, use instead of the process's heap.
//!
//! The twins run on the simulator's threads (`SimOs::thread`), each on a CPU of its own, whose
//! sequences the simulator's fence aborts as the kernel's would. A real thread borrows one of
//! them for the length of one call (a *lease*, claimed with a compare-and-swap) and gives it back
//! at the end, so no simulated thread is ever used by two real threads at once, and the twins'
//! sequences on one CPU never interleave. A real thread that finds every simulated one taken
//! runs as a thread without an rseq registration, straight on the back end: it never waits.
//!
//! The ledger for the unsafe blocks here is `docs/safety/asmalloc/heap/sim-safety.md`.

use super::{Heap, Zero};
use crate::front::{Cpu, Field, Routines, Slab, Slabs, StackPop, StackPush, StoreIfEqual, Twin};
use crate::os::sim::{SIM_THREADS, Shape, SimOs, SimThread, Standard};
use crate::os::{Aborted, Machine, Step};
use alloc::boxed::Box;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

/// A heap over a [`SimOs`], with its own slabs, run with twins. See the module documentation.
pub struct SimHeap<S: Shape = Standard> {
    pub(super) heap: Heap<SimOs<S>>,
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

    fn committed(&self) {
        if let Processor::Thread(thread) = self {
            thread.committed();
        }
    }
}

/// The twins on a heap's slabs, for one call: a copyable form of [`Twin`].
#[derive(Clone, Copy)]
struct SimRoutines<'a> {
    slabs: &'a Slabs,
    processor: Processor<'a>,
}

impl<'a> SimRoutines<'a> {
    fn twin(self) -> Twin<'a, Processor<'a>> {
        Twin::new(self.slabs, self.processor)
    }
}

// SAFETY: SIM-01. Every method is the twin's, on the same slabs and processor.
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
        // SAFETY: the caller's promise is the twin's.
        unsafe { self.twin().store_if_equal(field, expected, new) }
    }

    unsafe fn stack_push(&self, field: Field, magazine: NonNull<u8>) -> StackPush {
        // SAFETY: the caller's promise is the twin's.
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
    /// A heap over a fresh simulated OS, not yet started. The OS has room for two
    /// reservations of the largest plan, as Linux has room for many: threads that start the
    /// heap together each reserve before one publishes, and with room for only one the others
    /// would find nothing and fail their first allocation.
    pub fn new() -> SimHeap<S> {
        let os = SimOs::with_capacity(2 * crate::os::sim::capacity_for::<S>());
        SimHeap::with_os(os)
    }

    /// A heap over `os`, not yet started.
    pub fn with_os(os: SimOs<S>) -> SimHeap<S> {
        SimHeap {
            heap: Heap::new(os),
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

    /// Runs `f` with the heap and the twins of a leased simulated thread.
    fn with<T>(&self, f: impl FnOnce(&Heap<SimOs<S>>, SimRoutines<'_>) -> T) -> T {
        let lease = self.lease();
        let processor = match &lease {
            Some(lease) => Processor::Thread(self.heap.os().thread(lease.index)),
            None => Processor::Unregistered,
        };
        let routines = SimRoutines {
            slabs: &*self.slabs,
            processor,
        };
        let result = f(&self.heap, routines);
        drop(lease);
        result
    }
}

impl<S: Shape> Default for SimHeap<S> {
    fn default() -> SimHeap<S> {
        SimHeap::new()
    }
}

// SAFETY: SIM-02. Every method is the heap's, with twins on this heap's own slabs.
unsafe impl<S: Shape> GlobalAlloc for SimHeap<S> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.alloc(routines, layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.with(|heap, routines| {
            // SAFETY: SIM-03.
            unsafe { heap.alloc_grant(routines, layout, Zero::Requested) }
                .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.dealloc(routines, ptr, layout) })
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let (Some(block), Ok(new)) = (
            NonNull::new(ptr),
            Layout::from_size_align(new_size, layout.align()),
        ) else {
            return ptr::null_mut();
        };
        self.with(|heap, routines| {
            // SAFETY: SIM-03.
            unsafe { heap.resize(routines, block, layout, new, false) }
                .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
        })
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: SIM-02.
unsafe impl<S: Shape> core::alloc::Allocator for SimHeap<S> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.allocate(routines, layout, false) })
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.allocate(routines, layout, true) })
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.deallocate(routines, ptr, layout) })
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.grow(routines, ptr, old, new, false) })
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.grow(routines, ptr, old, new, true) })
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: SIM-03.
        self.with(|heap, routines| unsafe { heap.shrink(routines, ptr, old, new) })
    }
}

/// A zero-sized handle to one [`SimHeap`] shared by the whole process, made on first use: the
/// simulated counterpart of `Asmalloc`, for code that needs a handle with no state (the test
/// kit's target under Miri).
#[derive(Clone, Copy, Debug, Default)]
pub struct SimAsmalloc;

/// The shared heap, or null before the first use.
static SHARED: AtomicPtr<SimHeap> = AtomicPtr::new(ptr::null_mut());

impl SimAsmalloc {
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
                    // SAFETY: SIM-04.
                    drop(unsafe { Box::from_raw(mine) });
                    current = winner;
                }
            }
        }
        // SAFETY: SIM-04.
        unsafe { &*current }
    }
}

// SAFETY: SIM-02, for the shared heap.
unsafe impl GlobalAlloc for SimAsmalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().realloc(ptr, layout, new_size) }
    }
}

#[cfg(feature = "allocator-api")]
// SAFETY: SIM-02, for the shared heap.
unsafe impl core::alloc::Allocator for SimAsmalloc {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        SimAsmalloc::heap().allocate(layout)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        SimAsmalloc::heap().allocate_zeroed(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().deallocate(ptr, layout) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().grow(ptr, old, new) }
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().grow_zeroed(ptr, old, new) }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, core::alloc::AllocError> {
        // SAFETY: the caller's promise is the shared heap's.
        unsafe { SimAsmalloc::heap().shrink(ptr, old, new) }
    }
}
