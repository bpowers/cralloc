//! The heap: a back end, a depot and a slab array working as one allocator, and the operations
//! its interfaces share.
//!
//! A [`Heap`] is the back end's state and the depot. The slab array belongs to the routines it
//! is run with: the generated routines ([`crate::front::Real`]) on the static `front::SLABS` in
//! the process's heap, or twins on a small array in a simulated heap. Everything here is
//! generic over the routines, so the two heaps share every line but the fast-path seam.
//!
//! - **Small objects** take their class from the `Layout`, with one compare and a table load
//!   (`SmallClass::of_nonzero_layout`), which a constant layout folds to a constant. Allocation
//!   is the pop routine and, when it misses, the depot's slow path, which exchanges magazines.
//!   Free is the push routine and its slow path. Everything but the lookup and the routine is
//!   out of line and cold, so that the inlined fast path stays small enough to inline into
//!   every caller: the slow paths, large requests, and zero-size ones, which the lookup turns
//!   away although they have a small class.
//! - **Start-up.** The slabs start zero, which the routines read as "no magazine", so the first
//!   call of each class takes the slow path, and the first slow path of the process reserves the
//!   address space ([`crate::back::State::get_or_reserve`]). The fast path never checks.
//! - **No valid CPU number.** The routines miss for a thread whose CPU number has no slab (one
//!   without an rseq registration reads a negative number), and the slow path then serves it
//!   straight from the back end (`depot::slow`).
//! - **Large objects** (`large.rs`) skip the caches and take their class from the address. A
//!   large allocation or free counts as a slow path and runs a heartbeat.
//! - **`collect`** returns everything that can be returned now: the depot's sweep of every
//!   slab and stack (`Cache::collect`), then the back end's purge of what is free.

mod address;
mod large;
mod source;

#[cfg(feature = "allocator-api")]
mod allocator;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
mod global;
#[cfg(miri)]
mod miri;
#[cfg(any(test, feature = "sim"))]
mod sim;

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub use global::Asmalloc;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use global::{pop_slow, push_slow};
#[cfg(miri)]
pub use miri::Asmalloc;
#[cfg(any(test, feature = "sim"))]
pub use sim::{SimAsmalloc, SimHeap};

pub(crate) use address::Found;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use address::process;

use self::source::BackSource;
use crate::back::{Back, State};
use crate::classes::{Class, SmallClass};
use crate::depot::{Cache, Depot};
use crate::front::Routines;
use crate::os::Os;
use core::alloc::Layout;
use core::ptr::{self, NonNull};

/// The back end and the depot of one heap. All zero but the OS value, so it can be a static.
pub(crate) struct Heap<O: Os> {
    back: State<O>,
    depot: Depot,
}

/// A block, and the bytes of it the caller may use: the whole slot or run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grant {
    pub(crate) ptr: NonNull<u8>,
    // Only `Allocator` returns it.
    #[cfg_attr(not(feature = "allocator-api"), allow(dead_code))]
    pub(crate) len: usize,
    /// All `len` bytes are known to read as zero: fresh or purged memory, or cleared.
    pub(crate) zeroed: bool,
}

/// Which bytes of a new block must read as zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Zero {
    /// None.
    No,
    /// The bytes the layout asked for (`GlobalAlloc::alloc_zeroed`).
    Requested,
    /// The whole block (`Allocator::allocate_zeroed`, which returns the whole block).
    Whole,
}

/// The small class of a layout, if it has one.
#[inline(always)]
pub(crate) const fn small(layout: Layout) -> Option<SmallClass> {
    match Class::of_layout(layout) {
        Some(class) => class.small(),
        None => None,
    }
}

impl<O: Os> Heap<O> {
    /// A heap over `os` that has not started.
    pub(crate) const fn new(os: O) -> Heap<O> {
        Heap {
            back: State::new(os),
            depot: Depot::new(),
        }
    }

    /// The OS.
    #[inline]
    pub(crate) fn os(&self) -> &O {
        self.back.os()
    }

    /// The back end, if the heap has started.
    #[inline]
    pub(crate) fn back(&self) -> Option<Back<'_, O>> {
        self.back.get()
    }

    /// The back end, starting the heap if no thread has. `None` if the address space cannot be
    /// reserved.
    fn start(&self) -> Option<Back<'_, O>> {
        let back = match self.back.get() {
            Some(back) => back,
            None => self.reserve()?,
        };
        // The generated routines hand object addresses over as integers. Exposing the
        // reservation's provenance gives every object address one to take back. Every slow
        // path exposes it (it costs no instruction), so no thread can pop an object before
        // some exposure it is ordered after.
        let _ = back.space().base().expose_provenance();
        Some(back)
    }

    /// Reserves the heap's address space, or finds the reservation another thread made.
    ///
    /// Threads that start the heap together each reserve, and one publishes. Where the OS has
    /// room for only one reservation, the others fail, and they may fail before the winner has
    /// published, which would turn a race into an out-of-memory error. So a failure is retried
    /// a few times, each looking for the winner's reservation first. The bound keeps start-up
    /// from waiting on a thread that never publishes (one that stopped in a fork, say); past it
    /// the request fails as out of memory, which is all a real shortage can do.
    #[cold]
    fn reserve(&self) -> Option<Back<'_, O>> {
        const ATTEMPTS: usize = 4;
        for _ in 0..ATTEMPTS {
            if let Ok(back) = self.back.get_or_reserve() {
                return Some(back);
            }
        }
        self.back.get()
    }

    /// Whether `ptr` lies in the heap's object regions. A block of this heap does; so may an
    /// interior pointer or a slot that is free.
    pub(crate) fn contains(&self, ptr: *const u8) -> bool {
        self.back()
            .is_some_and(|back| back.space().locate(ptr).is_some())
    }

    /// The bytes the caller may use of the block starting at `ptr`: the class size of a small
    /// one; for a large one, the size last asked for rounded up to a page, never the whole slot
    /// or run, which may be far bigger (`Back::usable_size`). `None` if no slot or run starts
    /// there.
    pub(crate) fn usable_size(&self, ptr: *const u8) -> Option<usize> {
        self.back()?.usable_size(ptr)
    }

    /// Walks the heap for `visitor` (`crate::stats`): read-only, and a racy snapshot beside
    /// running threads. Reports nothing before the heap has started. `slabs` is the slab array
    /// this heap is run with.
    pub(crate) fn inspect(
        &self,
        slabs: &crate::front::Slabs,
        visitor: &mut impl crate::stats::Visitor,
    ) {
        if let Some(back) = self.back() {
            back.inspect(visitor);
            crate::depot::inspect(&self.depot, back.space(), slabs, visitor);
        }
    }

    /// Allocates a block for `layout`; null if there is no memory.
    ///
    /// # Safety
    ///
    /// The slab array of `routines` is used only with this heap (`Cache::new`'s contract).
    #[inline(always)]
    pub(crate) unsafe fn alloc<R: Routines + Copy>(&self, routines: R, layout: Layout) -> *mut u8 {
        match SmallClass::of_nonzero_layout(layout) {
            // SAFETY: passed through from the caller.
            Some(class) => unsafe { self.alloc_small(routines, class) },
            // SAFETY: passed through from the caller.
            None => unsafe { self.alloc_other(routines, layout) },
        }
    }

    /// Allocates an object of a small class: the pop routine, and its slow path on a miss.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[inline(always)]
    unsafe fn alloc_small<R: Routines + Copy>(&self, routines: R, class: SmallClass) -> *mut u8 {
        match routines.pop(class) {
            Some(object) => object.as_ptr(),
            // SAFETY: passed through from the caller.
            None => unsafe { self.alloc_slow(routines, class) },
        }
    }

    /// A request the inlined lookup turns away: a large one, or one of zero bytes, which has a
    /// small class.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[cold]
    #[inline(never)]
    unsafe fn alloc_other<R: Routines + Copy>(&self, routines: R, layout: Layout) -> *mut u8 {
        match small(layout) {
            // SAFETY: passed through from the caller.
            Some(class) => unsafe { self.alloc_small(routines, class) },
            // SAFETY: passed through from the caller.
            None => unsafe { self.alloc_large_slow(routines, layout, Zero::No) }
                .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr()),
        }
    }

    /// Allocates a block for `layout` with the bytes `zero` names zeroed, and says how much of
    /// it the caller may use.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[inline(always)]
    pub(crate) unsafe fn alloc_grant<R: Routines + Copy>(
        &self,
        routines: R,
        layout: Layout,
        zero: Zero,
    ) -> Option<Grant> {
        match SmallClass::of_nonzero_layout(layout) {
            // SAFETY: passed through from the caller.
            Some(class) => unsafe { self.grant_small(routines, class, layout, zero) },
            // SAFETY: passed through from the caller.
            None => unsafe { self.grant_other(routines, layout, zero) },
        }
    }

    /// [`Heap::alloc_grant`] for a request the inlined lookup turns away.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[cold]
    #[inline(never)]
    unsafe fn grant_other<R: Routines + Copy>(
        &self,
        routines: R,
        layout: Layout,
        zero: Zero,
    ) -> Option<Grant> {
        match small(layout) {
            // SAFETY: passed through from the caller.
            Some(class) => unsafe { self.grant_small(routines, class, layout, zero) },
            // SAFETY: passed through from the caller.
            None => unsafe { self.alloc_large_slow(routines, layout, zero) },
        }
    }

    /// [`Heap::alloc_grant`] of an object of a small class.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[inline(always)]
    unsafe fn grant_small<R: Routines + Copy>(
        &self,
        routines: R,
        class: SmallClass,
        layout: Layout,
        zero: Zero,
    ) -> Option<Grant> {
        // SAFETY: passed through from the caller.
        let ptr = NonNull::new(unsafe { self.alloc_small(routines, class) })?;
        let len = class.size();
        let zeroed = match zero {
            Zero::No => 0,
            Zero::Requested => layout.size(),
            Zero::Whole => len,
        };
        // SAFETY: HEAP-03. Small slots are never known to be zero, so they are always cleared.
        unsafe { ptr.as_ptr().write_bytes(0, zeroed) };
        Some(Grant {
            ptr,
            len,
            zeroed: zero == Zero::Whole,
        })
    }

    /// Frees a block.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`]; and `ptr` is a block this heap handed out that the caller owns and
    /// gives up, described by `layout`: the layout it was allocated with, or one whose size lies
    /// between that and the length the heap returned, with the same alignment.
    #[inline(always)]
    pub(crate) unsafe fn dealloc<R: Routines + Copy>(
        &self,
        routines: R,
        ptr: *mut u8,
        layout: Layout,
    ) {
        // SAFETY: HEAP-01.
        let object = unsafe { NonNull::new_unchecked(ptr) };
        match SmallClass::of_nonzero_layout(layout) {
            // SAFETY: passed through from the caller.
            Some(class) => unsafe { self.free_small(routines, class, object) },
            // SAFETY: passed through from the caller.
            None => unsafe { self.dealloc_other(routines, object, layout) },
        }
    }

    /// Frees an object of a small class: the push routine, and its slow path on a miss.
    ///
    /// # Safety
    ///
    /// As for [`Heap::dealloc`], `object` being of `class`.
    #[inline(always)]
    unsafe fn free_small<R: Routines + Copy>(
        &self,
        routines: R,
        class: SmallClass,
        object: NonNull<u8>,
    ) {
        if !routines.push(class, object) {
            // SAFETY: passed through from the caller.
            unsafe { self.free_slow(routines, class, object) }
        }
    }

    /// [`Heap::dealloc`] for a layout the inlined lookup turns away.
    ///
    /// # Safety
    ///
    /// As for [`Heap::dealloc`].
    #[cold]
    #[inline(never)]
    unsafe fn dealloc_other<R: Routines + Copy>(
        &self,
        routines: R,
        object: NonNull<u8>,
        layout: Layout,
    ) {
        match small(layout) {
            // SAFETY: passed through from the caller.
            Some(class) => unsafe { self.free_small(routines, class, object) },
            // SAFETY: passed through from the caller.
            None => unsafe { self.free_large_slow(routines, object) },
        }
    }

    /// The slow path of a small allocation: start the heap if need be, then exchange magazines.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn alloc_slow<R: Routines + Copy>(
        &self,
        routines: R,
        class: SmallClass,
    ) -> *mut u8 {
        let Some(back) = self.start() else {
            return ptr::null_mut();
        };
        let source = BackSource(back);
        // SAFETY: HEAP-02.
        let cache = unsafe { Cache::new(&self.depot, back.space(), routines, &source) };
        cache
            .alloc_slow(class)
            .map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    /// The slow path of a small free.
    ///
    /// # Safety
    ///
    /// As for [`Heap::dealloc`].
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn free_slow<R: Routines + Copy>(
        &self,
        routines: R,
        class: SmallClass,
        object: NonNull<u8>,
    ) {
        let Some(back) = self.back() else {
            self.os().abort("free of a block when no heap exists");
        };
        let _ = back.space().base().expose_provenance();
        let source = BackSource(back);
        // SAFETY: HEAP-02.
        let cache = unsafe { Cache::new(&self.depot, back.space(), routines, &source) };
        cache.free_slow(class, object);
    }

    /// The heartbeat of a slow path that served no small class: a large allocation or free.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn heartbeat<R: Routines + Copy>(&self, routines: R) {
        let Some(back) = self.back() else {
            return;
        };
        let _ = back.space().base().expose_provenance();
        let source = BackSource(back);
        // SAFETY: HEAP-02.
        let cache = unsafe { Cache::new(&self.depot, back.space(), routines, &source) };
        cache.maintain();
    }

    /// Returns everything that can be returned now: drains every slab a slow path has used
    /// (the caller's own through its routines), launders every depot stack, then purges every
    /// wholly free page. Returns the bytes purged that had been resident, as the back end
    /// counts them. Nothing before the heap has started.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    pub(crate) unsafe fn collect<R: Routines + Copy>(&self, routines: R) -> usize {
        let Some(back) = self.back() else {
            return 0;
        };
        let _ = back.space().base().expose_provenance();
        let source = BackSource(back);
        // SAFETY: HEAP-02.
        let cache = unsafe { Cache::new(&self.depot, back.space(), routines, &source) };
        cache.collect();
        back.trim()
    }
}

#[cfg(test)]
mod tests;
