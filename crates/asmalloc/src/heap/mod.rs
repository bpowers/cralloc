//! The heap: a back end, a depot and a slab array working as one allocator, and the operations
//! its interfaces share.
//!
//! A [`Heap`] is the back end's state and the depot. The slab array belongs to the routines it
//! is run with: the generated routines (`front::Real`) on the static `front::SLABS` in the
//! process's heap, or twins on a small array in a simulated heap. A [`Bound`] is a heap with its
//! routines, made by the one `unsafe` constructor whose contract is that the two belong
//! together. Everything that allocates or maintains the heap is a safe method of it; only frees
//! and resizes, which take a block, are `unsafe`. Everything here is generic over the routines,
//! so the two heaps share every line but the fast-path seam.
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
//! - **`maintain`** is one round of the clocked maintenance for the whole heap
//!   (`Cache::maintain`, which ends with the back end's clocked purge pass): what the
//!   heartbeats of an interval would have done, for a background thread (`background.rs`,
//!   feature `background`) or a program's idle hook.

mod address;
mod large;
mod source;

#[cfg(feature = "allocator-api")]
mod allocator;
// Also in the crate's own tests, so that the gate builds, lints and runs it.
#[cfg(all(
    any(test, feature = "background"),
    target_os = "linux",
    target_arch = "x86_64",
    not(miri),
    not(kani)
))]
pub(crate) mod background;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
mod global;
#[cfg(any(test, feature = "sim"))]
mod sim;

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub use global::Asmalloc;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use global::{c_alloc_small, c_free_small};
#[cfg(any(test, feature = "sim"))]
pub use sim::{SimAsmalloc, SimHeap};

pub(crate) use address::Found;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use address::process;

use crate::back::{Back, State};
use crate::classes::{Class, SmallClass};
use crate::depot::{Cache, Depot, thread};
use crate::front::Routines;
use crate::os::Os;
use crate::thread_word::{self, AWAKE, Snapshot, ThreadWord};
use core::alloc::Layout;
use core::ptr::{self, NonNull};

/// The back end and the depot of one heap. All zero but the OS value, so it can be a static.
pub(crate) struct Heap<O: Os> {
    back: State<O>,
    depot: Depot,
    /// What starts the background thread when a heartbeat wants one, called after a slow path
    /// that came through the Rust interface (`background`); `None` for a heap that never starts
    /// one by default.
    start: Option<fn()>,
}

/// A block, and the bytes of it the caller may use: its usable size (`Heap::usable_size`).
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
        Heap::starting_with(os, None)
    }

    /// A heap whose Rust-interface slow paths call `start`, if given, when a heartbeat wants
    /// the background thread.
    pub(crate) const fn starting_with(os: O, start: Option<fn()>) -> Heap<O> {
        Heap {
            back: State::new(os),
            depot: Depot::new(),
            start,
        }
    }

    /// The OS.
    #[inline]
    pub(crate) fn os(&self) -> &O {
        self.back.os()
    }

    /// Puts CPU number `c` in cache domain `c / width` (`Depot::split_domains`).
    #[cfg(any(test, feature = "internals"))]
    pub(crate) fn split_domains(&self, width: usize) {
        self.depot.split_domains(self.os(), width);
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
        Some(exposed(back))
    }

    /// The back end, if the heap has started, for a slow path that hands no object out.
    fn started(&self) -> Option<Back<'_, O>> {
        self.back.get().map(exposed)
    }

    /// Reserves the heap's address space, or finds the reservation another thread made. Threads
    /// that start the heap together each reserve, and one publishes; `SpaceCell` keeps a
    /// failure caused by the others' reservations from shrinking the heap or failing the call.
    #[cold]
    fn reserve(&self) -> Option<Back<'_, O>> {
        self.back.get_or_reserve().ok()
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
}

/// `back`, with its reservation's provenance exposed. The generated routines hand object
/// addresses over as integers, and exposing the reservation's provenance gives every object
/// address one to take back. Every slow path exposes it (it costs no instruction), so no thread
/// can pop an object before some exposure it is ordered after.
fn exposed<O: Os>(back: Back<'_, O>) -> Back<'_, O> {
    let _ = back.space().base().expose_provenance();
    back
}

/// A heap and the routines that write its slab array: what every interface runs on. A copy is
/// one reference and the routines, which for the process's heap are zero-sized.
pub(crate) struct Bound<'a, O: Os, R> {
    heap: &'a Heap<O>,
    routines: R,
}

impl<O: Os, R: Copy> Clone for Bound<'_, O, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<O: Os, R: Copy> Copy for Bound<'_, O, R> {}

impl<'a, O: Os, R: Routines + Copy> Bound<'a, O, R> {
    /// `heap`, run with `routines`.
    ///
    /// # Safety
    ///
    /// The slab array of `routines` belongs to `heap`: while either is used, it is used with no
    /// other heap, and nothing writes it but `routines` and `heap`'s own slow paths and drains.
    /// Its words then name only `heap`'s magazines, as `Cache::new` asks. Ledger: HEAP-02.
    #[inline(always)]
    pub(crate) const unsafe fn new(heap: &'a Heap<O>, routines: R) -> Self {
        Bound { heap, routines }
    }

    /// The heap.
    #[inline(always)]
    pub(crate) fn heap(self) -> &'a Heap<O> {
        self.heap
    }

    /// The caches and the depot as this thread sees them, over `back`, this heap's back end.
    fn cache<'s>(self, back: &'s Back<'a, O>) -> Cache<'s, O, R, Back<'a, O>> {
        // SAFETY: HEAP-02. The depot and the back end are the two halves of one `Heap`, whose
        // state publishes one space for its whole life, and the routines' slab array belongs
        // to this heap by the constructor's contract.
        unsafe { Cache::new(&self.heap.depot, back.space(), self.routines, back) }
    }

    /// Walks the heap for `visitor` (`crate::stats`): read-only, and a racy snapshot beside
    /// running threads. Reports nothing before the heap has started.
    pub(crate) fn inspect(self, visitor: &mut impl crate::stats::Visitor) {
        if let Some(back) = self.heap.back() {
            back.inspect(visitor);
            crate::depot::inspect(
                &self.heap.depot,
                back.space(),
                self.routines.slabs(),
                visitor,
            );
        }
    }

    /// Allocates a block for `layout`; null if there is no memory.
    #[inline(always)]
    pub(crate) fn alloc(self, layout: Layout) -> *mut u8 {
        match SmallClass::of_nonzero_layout(layout) {
            Some(class) => self.alloc_small(class),
            None => self.alloc_other(layout),
        }
    }

    /// Allocates an object of a small class: the pop routine, and its slow path on a miss.
    #[inline(always)]
    fn alloc_small(self, class: SmallClass) -> *mut u8 {
        match self.routines.pop(class) {
            Some(object) => object.as_ptr(),
            None => self.rust_alloc_slow(class),
        }
    }

    /// [`Bound::alloc_slow`] for a call through the Rust interface.
    #[cold]
    #[inline(never)]
    fn rust_alloc_slow(self, class: SmallClass) -> *mut u8 {
        let (object, seen) = self.alloc_slow_seeing(class);
        self.after_rust_slow_path(seen);
        object
    }

    /// [`Bound::free_slow`] for a call through the Rust interface.
    ///
    /// # Safety
    ///
    /// As for [`Bound::free_slow`].
    #[cold]
    #[inline(never)]
    unsafe fn rust_free_slow(self, class: SmallClass, object: NonNull<u8>) {
        // SAFETY: passed through.
        let seen = unsafe { self.free_slow(class, object) };
        self.after_rust_slow_path(seen);
    }

    /// Starts the background thread if it is wanted, after a slow path that came through the
    /// Rust interface, from `seen`, the background thread's word as that slow path's one load
    /// read or left it. The C interface never starts it from a slow path, since its caller may
    /// be the C library under its own locks (design: "Heartbeat").
    #[inline]
    fn after_rust_slow_path(self, seen: Snapshot) {
        if let Some(start) = self.heap.start
            && seen.wanted()
        {
            start();
        }
    }

    /// The background thread's word.
    #[inline(always)]
    fn word(self) -> &'a ThreadWord {
        self.heap.back.thread()
    }

    /// A request the inlined lookup turns away: a large one, or one of zero bytes, which has a
    /// small class.
    #[cold]
    #[inline(never)]
    fn alloc_other(self, layout: Layout) -> *mut u8 {
        match small(layout) {
            Some(class) => self.alloc_small(class),
            None => self
                .alloc_large_slow(layout, Zero::No)
                .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr()),
        }
    }

    /// Allocates a block for `layout` with the bytes `zero` names zeroed, and says how much of
    /// it the caller may use.
    #[inline(always)]
    pub(crate) fn alloc_grant(self, layout: Layout, zero: Zero) -> Option<Grant> {
        match SmallClass::of_nonzero_layout(layout) {
            Some(class) => self.grant_small(class, layout, zero),
            None => self.grant_other(layout, zero),
        }
    }

    /// [`Bound::alloc_grant`] for a request the inlined lookup turns away.
    #[cold]
    #[inline(never)]
    fn grant_other(self, layout: Layout, zero: Zero) -> Option<Grant> {
        match small(layout) {
            Some(class) => self.grant_small(class, layout, zero),
            None => self.alloc_large_slow(layout, zero),
        }
    }

    /// [`Bound::alloc_grant`] of an object of a small class.
    #[inline(always)]
    fn grant_small(self, class: SmallClass, layout: Layout, zero: Zero) -> Option<Grant> {
        let ptr = NonNull::new(self.alloc_small(class))?;
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
    /// `ptr` is a block this heap handed out that the caller owns and gives up, described by
    /// `layout`: the layout it was allocated with, or one whose size lies between that and the
    /// length the heap returned, with the same alignment.
    #[inline(always)]
    pub(crate) unsafe fn dealloc(self, ptr: *mut u8, layout: Layout) {
        // SAFETY: HEAP-01.
        let object = unsafe { NonNull::new_unchecked(ptr) };
        match SmallClass::of_nonzero_layout(layout) {
            // SAFETY: a small block is a slot of its layout's class, and every layout that
            // describes it has that class (the fit rule), so the block is of `class`.
            Some(class) => unsafe { self.free_small(class, object) },
            // SAFETY: the block is the caller's, described by `layout`, and not null.
            None => unsafe { self.dealloc_other(object, layout) },
        }
    }

    /// Frees an object of a small class: the push routine, and its slow path on a miss.
    ///
    /// # Safety
    ///
    /// `object` is a block of `class` this heap handed out, which the caller owns and gives up.
    #[inline(always)]
    unsafe fn free_small(self, class: SmallClass, object: NonNull<u8>) {
        if !self.routines.push(class, object) {
            // SAFETY: a push that misses stores nothing, so the block is still the caller's.
            unsafe { self.rust_free_slow(class, object) }
        }
    }

    /// [`Bound::dealloc`] for a layout the inlined lookup turns away.
    ///
    /// # Safety
    ///
    /// As for [`Bound::dealloc`].
    #[cold]
    #[inline(never)]
    unsafe fn dealloc_other(self, object: NonNull<u8>, layout: Layout) {
        match small(layout) {
            // SAFETY: as in `dealloc`, the layout's small class is the block's.
            Some(class) => unsafe { self.free_small(class, object) },
            None => {
                // SAFETY: a layout with no small class describes a large block (GLOBAL-03),
                // which the caller gives up.
                let seen = unsafe { self.free_large_slow(object) };
                self.after_rust_slow_path(seen);
            }
        }
    }

    /// The slow path of a small allocation: start the heap if need be, then exchange magazines.
    #[cold]
    #[inline(never)]
    pub(crate) fn alloc_slow(self, class: SmallClass) -> *mut u8 {
        self.alloc_slow_seeing(class).0
    }

    /// [`Bound::alloc_slow`], and the background thread's word as the slow path left it.
    fn alloc_slow_seeing(self, class: SmallClass) -> (*mut u8, Snapshot) {
        let Some(back) = self.heap.start() else {
            return (ptr::null_mut(), Snapshot(0));
        };
        let cache = self.cache(&back);
        let object = cache
            .alloc_slow(class)
            .map_or(ptr::null_mut(), NonNull::as_ptr);
        (object, cache.thread_seen())
    }

    /// The slow path of a small free. Returns the background thread's word as the slow path
    /// left it.
    ///
    /// # Safety
    ///
    /// `object` is a block of `class` this heap handed out, which the caller owns and gives up.
    #[cold]
    #[inline(never)]
    pub(crate) unsafe fn free_slow(self, class: SmallClass, object: NonNull<u8>) -> Snapshot {
        let Some(back) = self.heap.started() else {
            self.heap.os().abort("free of a block when no heap exists");
        };
        let cache = self.cache(&back);
        cache.free_slow(class, object);
        cache.thread_seen()
    }

    /// The heartbeat of a slow path that served no small class: a large allocation or free.
    /// It counts the slow path first, and builds what the chores need only when they are due:
    /// on the other calls it loads the background thread's word itself, and builds them only
    /// if the word asks for a look. Returns the word as it left it.
    #[cold]
    #[inline(never)]
    pub(crate) fn heartbeat(self) -> Snapshot {
        match crate::depot::lone_count(&self.heap.depot, &self.routines) {
            crate::depot::Lone::Beat(turn) => match self.heap.started() {
                Some(back) => {
                    let cache = self.cache(&back);
                    cache.lone_beat(turn);
                    cache.thread_seen()
                }
                None => self.word().load(),
            },
            crate::depot::Lone::Counted => {
                let seen = self.word().load();
                match self.heap.started() {
                    Some(back) if seen.needs_attention() => self.cache(&back).attend_thread(),
                    _ => seen,
                }
            }
        }
    }

    /// The background thread's life once started: rounds of `maintain` a step at a time, then
    /// a sleep, or a park until a slow path wakes it (`depot::thread`). A dormant thread waits
    /// until free memory is over the floor, and looks at the level itself each time before it
    /// waits, so that a heap already over the floor when it was created does not wait for a
    /// crossing that has passed. Never returns.
    pub(crate) fn run_thread(self, every_ns: u64) -> ! {
        let word = self.word();
        let os = self.heap.os();
        let mut seen = thread::Seen::default();
        let mut key = 0;
        loop {
            let now = word.load();
            let Some(back) = self.heap.started() else {
                os.wait(word.futex(), now.0, Some(every_ns));
                continue;
            };
            let cache = self.cache(&back);
            if key == 0 {
                key = cache.thread_key();
            }
            match now.state() {
                thread_word::DORMANT => {
                    if !(back.over_floor() && word.claim(key, thread_word::DORMANT, AWAKE)) {
                        os.wait(word.futex(), now.0, None);
                    }
                    continue;
                }
                thread_word::PARKED => {
                    os.wait(word.futex(), now.0, None);
                    continue;
                }
                _ => {}
            }
            let mut round = None;
            while let Some(step) = cache.maintain_step_with_work() {
                round = Some(round.unwrap_or(false) | step.work);
                if !step.more {
                    break;
                }
            }
            match cache.rest(&mut seen, key, round) {
                thread::Rest::Park => os.wait(word.futex(), key | thread_word::PARKED, None),
                thread::Rest::Sleep => os.wait(word.futex(), word.load().0, Some(every_ns)),
            }
        }
    }

    /// Returns everything that can be returned now: drains every slab a slow path has used
    /// (the caller's own through its routines), launders every depot stack, then purges every
    /// wholly free page. Returns the bytes purged that had been resident, as the back end
    /// counts them. Nothing before the heap has started.
    pub(crate) fn collect(self) -> usize {
        let Some(back) = self.heap.started() else {
            return 0;
        };
        self.cache(&back).collect();
        back.trim()
    }

    /// One round of the clocked maintenance for the whole heap: the idle look at every slab
    /// in use, every depot stack's shedding, and the clocked purge pass to its end. Respects
    /// the clock, so a second call within an interval does next to nothing. Nothing before the
    /// heap has started.
    pub(crate) fn maintain(self) {
        if let Some(back) = self.heap.started() {
            self.cache(&back).maintain();
        }
    }

    /// This process's generation as the background thread's word keys it, starting the heap
    /// and readying the magazine area if no slow path has; `None` if either cannot be done.
    fn thread_key(self) -> Option<u64> {
        let back = self.heap.start()?;
        let key = self.cache(&back).thread_key();
        (key != 0).then_some(key)
    }

    /// Moves the background thread's word from `from` to `to` for this process; whether it
    /// moved.
    pub(crate) fn claim_thread(self, from: u64, to: u64) -> bool {
        self.thread_key()
            .is_some_and(|key| self.word().claim(key, from, to))
    }

    /// Starts the background thread in state `to` (running, or dormant until the floor) with
    /// `spawn(create)`, unless one is already running in this process. A failure to create
    /// it leaves the default off, so that nothing tries again. `Err(None)` if the process has
    /// no generation to key the word with, which only a heap that could not start lacks.
    pub(crate) fn start_thread<C, E>(
        self,
        to: u64,
        spawn: impl FnOnce(C) -> Result<(), E>,
        create: C,
    ) -> Result<(), Option<E>> {
        let key = self.thread_key().ok_or(None)?;
        if !self.word().claim_start(self.heap.os(), key, to) {
            return Ok(());
        }
        spawn(create).map_err(|error| {
            self.word().claim(key, to, thread_word::OFF);
            Some(error)
        })
    }

    /// One bounded step of the round of maintenance, as `Cache::maintain_step`; whether the
    /// round has more. `false` before the heap has started.
    pub(crate) fn maintain_step(self) -> bool {
        match self.heap.started() {
            Some(back) => self.cache(&back).maintain_step(),
            None => false,
        }
    }

    /// Drains the slab of CPU number `cpu` now, as the heartbeat's idle check would once it had
    /// been quiet for an interval, and says how the drain ended. `None` if the heap has not
    /// started or the slab array has no such CPU.
    #[cfg(any(test, feature = "sim"))]
    pub(crate) fn drain(self, cpu: usize) -> Option<crate::depot::Drained> {
        let back = self.heap.back()?;
        let cpu = self.routines.slabs().cpu(i32::try_from(cpu).ok()?)?;
        Some(self.cache(&back).drain(cpu))
    }
}

#[cfg(test)]
mod tests;
