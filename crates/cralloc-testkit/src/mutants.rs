//! Allocators broken on purpose, to show that the model catches each kind of bug, and how fast.
//!
//! Each mutant wraps a correct [`Subject`] (any interface: `GlobalAlloc`, `Allocator`, C) and
//! introduces exactly one class of bug. The bugs are real in the sense that matters: the model
//! sees only what a program would see. They are also memory-safe for the model's own accesses.
//! Memory a mutant hands out wrongly is always memory the inner allocator gave it, and it stays
//! allocated until every handout of it is freed, so a mutant run ends in a clean [`Failure`]
//! rather than a crash.
//!
//! [`Failure`]: crate::failure::Failure

use core::ptr::{self, NonNull};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};

use crate::failure::FailureKind;
use crate::subject::{Grant, Held, Refused, Request, Resized, Subject, ZeroSize};

/// The bug classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Every 16th allocation returns memory inside a live block.
    Overlapping,
    /// Blocks promised 16-byte or greater alignment are 8 bytes off.
    Misaligned,
    /// Small blocks are 8 bytes shorter than requested: each starts 8 bytes before the
    /// previous one ends, as a size-class table with an off-by-one would place them.
    Short,
    /// Zeroed allocations and zeroed growth are not zeroed.
    NoZeroFill,
    /// Resizes copy one byte too few.
    LosesContents,
    /// A free from a thread other than the allocating one puts the block on two free lists, so
    /// it is handed out twice: the classic remote-free race.
    DoubleHandout,
    /// Every 10th free flips a byte in another live block, as a free-list write through a
    /// stale pointer would.
    Scribbler,
    /// Every 12th block is also put back on the free list while still live, and handed out
    /// again eight allocations later.
    PrematureReuse,
}

impl Kind {
    /// Every mutant.
    pub const ALL: [Kind; 8] = [
        Kind::Overlapping,
        Kind::Misaligned,
        Kind::Short,
        Kind::NoZeroFill,
        Kind::LosesContents,
        Kind::DoubleHandout,
        Kind::Scribbler,
        Kind::PrematureReuse,
    ];

    /// A short name, for reports and environment variables.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Overlapping => "overlapping",
            Kind::Misaligned => "misaligned",
            Kind::Short => "short",
            Kind::NoZeroFill => "no_zero_fill",
            Kind::LosesContents => "loses_contents",
            Kind::DoubleHandout => "double_handout",
            Kind::Scribbler => "scribbler",
            Kind::PrematureReuse => "premature_reuse",
        }
    }

    /// The mutant called `name`.
    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The failure kinds the model may report for this mutant.
    pub fn caught_as(self) -> &'static [FailureKind] {
        match self {
            // The memory handed out wrongly can go to a resize of the very block it belongs
            // to, which then comes back with its contents gone rather than overlapping.
            Kind::Overlapping | Kind::DoubleHandout | Kind::PrematureReuse => {
                &[FailureKind::Overlap, FailureKind::LostContents]
            }
            Kind::Misaligned => &[FailureKind::Misaligned],
            // The overlap is caught when the next block is placed, unless a free in between
            // lets the earlier block's damaged tail be checked first.
            Kind::Short => &[FailureKind::Overlap, FailureKind::Corrupted],
            Kind::NoZeroFill => &[FailureKind::NotZeroed],
            Kind::LosesContents => &[FailureKind::LostContents],
            // On another thread the write can land between an allocation and its zero check,
            // or during a resize, which the model reports as such.
            Kind::Scribbler => &[
                FailureKind::Corrupted,
                FailureKind::NotZeroed,
                FailureKind::LostContents,
            ],
        }
    }

    /// Whether the mutant reads memory it never initialized, which Miri reports as undefined
    /// behaviour before the model can.
    pub fn reads_uninit(self) -> bool {
        self == Kind::NoZeroFill
    }

    /// Whether the mutant writes into memory another thread may be using at the same moment,
    /// a data race Miri reports before the model can.
    pub fn races(self) -> bool {
        self == Kind::Scribbler
    }

    /// `inner` with this bug.
    pub fn wrap<S: Subject + 'static>(self, inner: S) -> Box<dyn Subject> {
        match self {
            Kind::Overlapping => Box::new(Overlapping::new(inner)),
            Kind::Misaligned => Box::new(Misaligned::new(inner)),
            Kind::Short => Box::new(Short::new(inner)),
            Kind::NoZeroFill => Box::new(NoZeroFill::new(inner)),
            Kind::LosesContents => Box::new(LosesContents::new(inner)),
            Kind::DoubleHandout => Box::new(DoubleHandout::new(inner)),
            Kind::Scribbler => Box::new(Scribbler::new(inner)),
            Kind::PrematureReuse => Box::new(PrematureReuse::new(inner)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Bookkeeping shared by the mutants
// ---------------------------------------------------------------------------------------------

/// A block of the inner allocator, and how many references keep it allocated: handouts to the
/// model, plus entries on a mutant's own lists.
struct InnerBlock {
    held: Held,
    refs: usize,
    thread: ThreadId,
}

#[derive(Default)]
struct Book {
    inner: HashMap<usize, InnerBlock>,
    /// Handed-out address to (inner block, handouts at this address).
    outer: HashMap<usize, (usize, usize)>,
    /// Inner blocks in the order they were booked, newest last, for picking victims.
    recent: VecDeque<usize>,
}

enum Release {
    /// The address was never booked (zero-length blocks are not).
    Unknown,
    /// Other references remain.
    Shared,
    /// That was the last reference: free this inner block.
    Last(Held),
}

const RECENT: usize = 64;

impl Book {
    /// Record a fresh inner block with no references yet; returns its address.
    fn insert(&mut self, g: &Grant, size: usize) -> usize {
        let base = g.ptr.addr().get();
        self.inner.insert(
            base,
            InnerBlock {
                held: Held::new(g, size),
                refs: 0,
                thread: thread::current().id(),
            },
        );
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(base);
        base
    }

    /// Hand out `outer`, which lies in the inner block at `base`.
    fn hand_out(&mut self, base: usize, outer: usize) {
        self.inner.get_mut(&base).expect("booked").refs += 1;
        self.outer.entry(outer).or_insert((base, 0)).1 += 1;
    }

    fn retain(&mut self, base: usize) {
        self.inner.get_mut(&base).expect("booked").refs += 1;
    }

    fn release(&mut self, base: usize) -> Release {
        let b = self.inner.get_mut(&base).expect("booked");
        b.refs -= 1;
        if b.refs > 0 {
            return Release::Shared;
        }
        let b = self.inner.remove(&base).expect("present");
        Release::Last(b.held)
    }

    /// Drop the handout at `outer` without releasing its reference; returns its inner block.
    fn forget_handout(&mut self, outer: usize) -> Option<usize> {
        let entry = self.outer.get_mut(&outer)?;
        let base = entry.0;
        entry.1 -= 1;
        if entry.1 == 0 {
            self.outer.remove(&outer);
        }
        Some(base)
    }

    fn take_back(&mut self, outer: usize) -> Release {
        match self.forget_handout(outer) {
            Some(base) => self.release(base),
            None => Release::Unknown,
        }
    }

    /// The inner block at `outer` if it is a plain block handed out once and nothing else.
    fn take_sole(&mut self, outer: usize) -> Option<Held> {
        let sole = self.outer.get(&outer) == Some(&(outer, 1))
            && self.inner.get(&outer).is_some_and(|b| b.refs == 1);
        if !sole {
            return None;
        }
        self.outer.remove(&outer);
        Some(self.inner.remove(&outer).expect("checked").held)
    }
}

/// The correct allocator plus the book of what has been handed out.
struct Base<S> {
    inner: S,
    book: Mutex<Book>,
}

impl<S: Subject> Base<S> {
    fn new(inner: S) -> Self {
        Base {
            inner,
            book: Mutex::new(Book::default()),
        }
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn name(&self, kind: Kind) -> String {
        format!("{} mutant of {}", kind.name(), self.inner.name())
    }

    /// Record `g` (for `size` bytes) as a block handed out at its own address.
    fn book_plain(&self, g: &Grant, size: usize) {
        if g.usable > 0 {
            let mut b = self.book();
            let base = b.insert(g, size);
            b.hand_out(base, base);
        }
    }

    /// A correct allocation, booked.
    ///
    /// # Safety
    /// As [`Subject::alloc`].
    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        // SAFETY: caller contract.
        let g = unsafe { self.inner.alloc(req) }?;
        if let Some(g) = &g {
            self.book_plain(g, req.size);
        }
        Ok(g)
    }

    /// Give back one handout, freeing the inner block with the last reference.
    ///
    /// # Safety
    /// As [`Subject::free`].
    unsafe fn free(&self, outer: Held, variant: u32) {
        let release = if outer.usable == 0 {
            Release::Unknown
        } else {
            self.book().take_back(outer.addr())
        };
        // SAFETY: an unbooked block was allocated by the inner allocator exactly as described;
        // a last reference names an inner block that nothing else refers to.
        unsafe {
            match release {
                Release::Unknown => self.inner.free(outer, variant),
                Release::Shared => {}
                Release::Last(h) => self.inner.free(h, variant),
            }
        }
    }

    /// Release a reference the mutant itself held.
    ///
    /// # Safety
    /// `base` is a booked inner block on which the caller holds a reference.
    unsafe fn release(&self, base: usize) {
        let release = self.book().release(base);
        if let Release::Last(h) = release {
            // SAFETY: the last reference to an inner block.
            unsafe { self.inner.free(h, 0) };
        }
    }

    /// A correct resize: in place through the inner allocator for a plain block, by copy
    /// through `alloc` otherwise.
    ///
    /// # Safety
    /// As [`Subject::resize`].
    unsafe fn resize(
        &self,
        outer: Held,
        req: Request,
        alloc: impl FnOnce(Request) -> Result<Option<Grant>, Refused>,
    ) -> Result<Resized, Refused> {
        let sole = if outer.usable == 0 {
            Some(outer)
        } else {
            self.book().take_sole(outer.addr())
        };
        let Some(h) = sole else {
            // SAFETY: caller contract.
            return unsafe { self.resize_by_copy(outer, req, alloc, 0) };
        };
        // SAFETY: `h` is the inner block itself, now private to this call.
        match unsafe { self.inner.resize(h, req) } {
            Ok(r) => {
                self.book_plain(&r.grant, req.size);
                Ok(r)
            }
            Err(e) => {
                self.book_plain(&grant_of(&h), h.size);
                Err(e)
            }
        }
    }

    /// Resize as allocate, copy, free, carrying over `lose` bytes fewer than promised.
    ///
    /// # Safety
    /// As [`Subject::resize`]; `alloc` honours [`Subject::alloc`]'s contract.
    unsafe fn resize_by_copy(
        &self,
        outer: Held,
        req: Request,
        alloc: impl FnOnce(Request) -> Result<Option<Grant>, Refused>,
        lose: usize,
    ) -> Result<Resized, Refused> {
        let g = alloc(req)?.ok_or(Refused)?;
        let keep = outer.size.min(req.size).min(g.usable);
        // SAFETY: both blocks hold at least `keep` bytes. They may overlap (an overlapping
        // mutant can hand out memory inside the old block), so this is a memmove.
        unsafe {
            ptr::copy(
                outer.ptr.as_ptr(),
                g.ptr.as_ptr(),
                keep.saturating_sub(lose),
            )
        };
        // SAFETY: the old block is live (caller contract) and its contents are copied.
        unsafe { self.free(outer, req.variant) };
        Ok(Resized {
            grant: Grant { zeroed: 0, ..g },
            preserved: keep,
            zeroed_from: (req.zeroed && g.zeroed >= g.usable).then_some(keep),
        })
    }
}

fn grant_of(h: &Held) -> Grant {
    Grant {
        ptr: h.ptr,
        usable: h.usable,
        align: h.align,
        zeroed: 0,
        tag: h.tag,
    }
}

/// Hand out an existing inner block again. A zeroed request gets no zero-fill promise: zeroing
/// would write into memory its rightful owner may be using on another thread, a race that
/// would end the run under Miri before the model sees the reuse.
fn reissue(block: &Held, req: &Request) -> Grant {
    Grant {
        ptr: block.ptr,
        usable: block.usable,
        align: req.align,
        zeroed: 0,
        tag: block.tag,
    }
}

fn fits(block: &Held, req: &Request) -> bool {
    req.size > 0 && block.usable >= req.size && block.addr().is_multiple_of(req.align)
}

/// Implements the `Subject` methods a mutant does not change by forwarding to `Base`.
macro_rules! forward {
    (name) => {
        fn name(&self) -> String {
            self.base.name(Self::KIND)
        }
        fn zero_size(&self) -> ZeroSize {
            self.base.inner.zero_size()
        }
    };
    (alloc) => {
        unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
            // SAFETY: same contract.
            unsafe { self.base.alloc(req) }
        }
    };
    (free) => {
        unsafe fn free(&self, block: Held, variant: u32) {
            // SAFETY: same contract.
            unsafe { self.base.free(block, variant) }
        }
    };
    (resize) => {
        unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
            // SAFETY: same contract; `self.alloc` honours it too.
            unsafe { self.base.resize(block, req, |r| self.alloc(r)) }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// The mutants
// ---------------------------------------------------------------------------------------------

/// See [`Kind::Overlapping`].
pub struct Overlapping<S> {
    base: Base<S>,
    allocs: AtomicUsize,
}

impl<S: Subject> Overlapping<S> {
    const KIND: Kind = Kind::Overlapping;
    const PERIOD: usize = 16;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        Overlapping {
            base: Base::new(inner),
            allocs: AtomicUsize::new(0),
        }
    }

    /// A pointer as deep into some live block as `req` allows, booked as a handout of it.
    fn inside_live_block(&self, req: &Request) -> Option<Grant> {
        let mut book = self.base.book();
        let (base, at) = book.recent.iter().rev().find_map(|&base| {
            let b = book.inner.get(&base)?;
            let h = &b.held;
            let last = h.addr().checked_add(h.usable)?.checked_sub(req.size)?;
            let at = last & !(req.align - 1);
            (at >= h.addr() && b.refs > 0).then_some((base, at))
        })?;
        let ptr = book.inner[&base].held.ptr;
        book.hand_out(base, at);
        drop(book);
        Some(Grant {
            ptr: ptr.with_addr(core::num::NonZeroUsize::new(at).expect("inside a block")),
            usable: req.size,
            align: req.align,
            zeroed: 0,
            tag: 0,
        })
    }
}

impl<S: Subject> Subject for Overlapping<S> {
    forward!(name);
    forward!(free);
    forward!(resize);

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let n = self.allocs.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(Self::PERIOD)
            && req.size > 0
            && let Some(g) = self.inside_live_block(&req)
        {
            return Ok(Some(g));
        }
        // SAFETY: same contract.
        unsafe { self.base.alloc(req) }
    }
}

/// See [`Kind::Misaligned`].
pub struct Misaligned<S> {
    base: Base<S>,
}

impl<S: Subject> Misaligned<S> {
    const KIND: Kind = Kind::Misaligned;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        Misaligned {
            base: Base::new(inner),
        }
    }
}

impl<S: Subject> Subject for Misaligned<S> {
    forward!(name);
    forward!(free);
    forward!(resize);

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let padded = Request {
            size: req.size + 8,
            ..req
        };
        // SAFETY: same contract; eight more bytes keep the size far from overflow.
        let Some(g) = (unsafe { self.base.inner.alloc(padded) })? else {
            return Ok(None);
        };
        if g.align < 16 {
            self.base.book_plain(&g, padded.size);
            return Ok(Some(g));
        }
        let mut book = self.base.book();
        let base = book.insert(&g, padded.size);
        book.hand_out(base, base + 8);
        drop(book);
        Ok(Some(Grant {
            // SAFETY: the inner block has at least eight bytes more than requested.
            ptr: unsafe { g.ptr.add(8) },
            usable: g.usable - 8,
            zeroed: g.zeroed.saturating_sub(8),
            ..g
        }))
    }
}

/// See [`Kind::Short`].
pub struct Short<S: Subject> {
    base: Base<S>,
    arena: Mutex<Arena>,
}

/// The chunk small blocks are carved from.
#[derive(Default)]
struct Arena {
    chunk: Option<NonNull<u8>>,
    cursor: usize,
    end: usize,
}

// SAFETY: the arena names memory the mutant owns; it is only touched under the mutex.
unsafe impl Send for Arena {}

impl<S: Subject> Short<S> {
    const KIND: Kind = Kind::Short;
    /// Blocks never cross the end of a chunk; they are short because each overlaps the next.
    const CHUNK: usize = 256 * 1024;
    const SMALL: usize = 2048;
    const SHORTFALL: usize = 8;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        Short {
            base: Base::new(inner),
            arena: Mutex::new(Arena::default()),
        }
    }

    /// Carve a block from the arena, opening a new chunk when the current one is full.
    ///
    /// # Safety
    /// As [`Subject::alloc`].
    unsafe fn carve(&self, req: &Request) -> Result<Option<Grant>, Refused> {
        let mut arena = self.arena.lock().unwrap_or_else(PoisonError::into_inner);
        let align = req.align.max(8);
        let mut start = arena.cursor.next_multiple_of(align);
        if arena.chunk.is_none() || start + req.size > arena.end {
            let chunk_req = Request {
                size: Self::CHUNK,
                align: 4096,
                zeroed: false,
                variant: 0,
            };
            // SAFETY: a well-formed request.
            let g = unsafe { self.base.inner.alloc(chunk_req) }?.ok_or(Refused)?;
            let base = {
                let mut book = self.base.book();
                let base = book.insert(&g, chunk_req.size);
                book.retain(base);
                base
            };
            if let Some(old) = arena.chunk.replace(g.ptr) {
                // SAFETY: the arena held a reference on its previous chunk.
                unsafe { self.base.release(old.addr().get()) };
            }
            arena.cursor = base;
            arena.end = base + Self::CHUNK;
            start = base.next_multiple_of(align);
        }
        let chunk = arena.chunk.expect("opened above");
        arena.cursor = start + req.size.saturating_sub(Self::SHORTFALL).max(1);
        drop(arena);
        let ptr = chunk.with_addr(core::num::NonZeroUsize::new(start).expect("inside the chunk"));
        self.base.book().hand_out(chunk.addr().get(), start);
        Ok(Some(Grant {
            ptr,
            usable: req.size,
            align: req.align,
            zeroed: 0,
            tag: 0,
        }))
    }
}

impl<S: Subject> Subject for Short<S> {
    forward!(name);
    forward!(free);
    forward!(resize);

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        if req.size == 0 || req.size > Self::SMALL || req.align > 64 {
            // SAFETY: same contract.
            return unsafe { self.base.alloc(req) };
        }
        // SAFETY: same contract.
        unsafe { self.carve(&req) }
    }
}

impl<S: Subject> Drop for Short<S> {
    fn drop(&mut self) {
        let arena = self.arena.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(chunk) = arena.chunk.take() {
            // SAFETY: the arena held a reference on its chunk. A chunk with blocks still out
            // (after a failed run abandoned them) stays allocated.
            unsafe { self.base.release(chunk.addr().get()) };
        }
    }
}

/// See [`Kind::NoZeroFill`].
pub struct NoZeroFill<S> {
    base: Base<S>,
}

impl<S: Subject> NoZeroFill<S> {
    const KIND: Kind = Kind::NoZeroFill;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        NoZeroFill {
            base: Base::new(inner),
        }
    }
}

impl<S: Subject> Subject for NoZeroFill<S> {
    forward!(name);
    forward!(free);

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let plain = Request {
            zeroed: false,
            ..req
        };
        // SAFETY: same contract.
        let g = unsafe { self.base.alloc(plain) }?;
        if let Some(g) = &g
            && req.zeroed
            && req.size > 0
        {
            // The byte a reused block would still hold had the bug skipped clearing it. Written
            // here so the bug shows whatever the allocator's history: an allocator that hands
            // out fresh memory, which reads as zero, would otherwise hide it, and a replay of a
            // caught script would not fail again.
            // SAFETY: the block is live and at least `req.size` long.
            unsafe { g.ptr.as_ptr().add(req.size - 1).write(0xa5) };
        }
        Ok(g.map(|g| Grant {
            zeroed: if req.zeroed {
                req.size.min(g.usable)
            } else {
                0
            },
            ..g
        }))
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        let plain = Request {
            zeroed: false,
            ..req
        };
        // SAFETY: same contract.
        let r = unsafe { self.base.resize(block, plain, |r| self.alloc(r)) }?;
        Ok(Resized {
            zeroed_from: req.zeroed.then_some(r.preserved),
            ..r
        })
    }
}

/// See [`Kind::LosesContents`].
pub struct LosesContents<S> {
    base: Base<S>,
}

impl<S: Subject> LosesContents<S> {
    const KIND: Kind = Kind::LosesContents;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        LosesContents {
            base: Base::new(inner),
        }
    }
}

impl<S: Subject> Subject for LosesContents<S> {
    forward!(name);
    forward!(alloc);
    forward!(free);

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        // The new block is zeroed, so the byte not copied is defined (zero) rather than
        // whatever the allocator left there.
        let zeroed = Request {
            zeroed: true,
            ..req
        };
        // SAFETY: same contract.
        unsafe {
            self.base
                .resize_by_copy(block, req, |_| self.base.alloc(zeroed), 1)
        }
    }
}

/// See [`Kind::DoubleHandout`].
pub struct DoubleHandout<S: Subject> {
    base: Base<S>,
    /// Inner blocks on the mutant's free lists; each entry holds a reference.
    lists: Mutex<VecDeque<usize>>,
}

impl<S: Subject> DoubleHandout<S> {
    const KIND: Kind = Kind::DoubleHandout;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        DoubleHandout {
            base: Base::new(inner),
            lists: Mutex::new(VecDeque::new()),
        }
    }

    fn lists(&self) -> MutexGuard<'_, VecDeque<usize>> {
        self.lists.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<S: Subject> Subject for DoubleHandout<S> {
    forward!(name);
    forward!(resize);

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let mut lists = self.lists();
        let mut book = self.base.book();
        let found = lists
            .iter()
            .position(|base| book.inner.get(base).is_some_and(|b| fits(&b.held, &req)));
        if let Some(i) = found {
            let base = lists.remove(i).expect("found");
            // The list's reference becomes the handout's.
            let b = book
                .inner
                .get_mut(&base)
                .expect("listed blocks stay booked");
            b.refs -= 1;
            b.thread = thread::current().id();
            let held = b.held;
            book.hand_out(base, base);
            drop((book, lists));
            return Ok(Some(reissue(&held, &req)));
        }
        drop((book, lists));
        // SAFETY: same contract.
        unsafe { self.base.alloc(req) }
    }

    unsafe fn free(&self, block: Held, variant: u32) {
        if block.usable > 0 {
            let mut book = self.base.book();
            let addr = block.addr();
            let remote = book.outer.get(&addr).is_some_and(|&(base, _)| base == addr)
                && book.inner[&addr].thread != thread::current().id();
            if remote {
                book.forget_handout(addr);
                // The handout's reference moves to the first list entry; the second needs one.
                book.retain(addr);
                drop(book);
                let mut lists = self.lists();
                lists.push_back(addr);
                lists.push_back(addr);
                return;
            }
        }
        // SAFETY: same contract.
        unsafe { self.base.free(block, variant) }
    }
}

impl<S: Subject> Drop for DoubleHandout<S> {
    fn drop(&mut self) {
        let lists = std::mem::take(self.lists.get_mut().unwrap_or_else(PoisonError::into_inner));
        for base in lists {
            // SAFETY: each list entry held a reference.
            unsafe { self.base.release(base) };
        }
    }
}

/// See [`Kind::Scribbler`].
pub struct Scribbler<S> {
    base: Base<S>,
    frees: AtomicUsize,
}

impl<S: Subject> Scribbler<S> {
    const KIND: Kind = Kind::Scribbler;
    const PERIOD: usize = 10;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        Scribbler {
            base: Base::new(inner),
            frees: AtomicUsize::new(0),
        }
    }
}

impl<S: Subject> Subject for Scribbler<S> {
    forward!(name);
    forward!(alloc);
    forward!(resize);

    unsafe fn free(&self, block: Held, variant: u32) {
        let n = self.frees.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(Self::PERIOD) {
            let book = self.base.book();
            let victim = book.recent.iter().rev().find_map(|base| {
                let b = book.inner.get(base)?;
                (b.refs > 0 && b.held.usable > 0 && *base != block.addr()).then_some(b.held)
            });
            if let Some(v) = victim {
                let offset = (n / Self::PERIOD * 31) % v.usable.min(4096);
                // SAFETY: inside a block the book keeps allocated while its lock is held. The
                // write races with the block's owner, which is the bug being modelled.
                unsafe {
                    let p = v.ptr.as_ptr().add(offset);
                    p.write_volatile(p.read_volatile() ^ 0x80);
                }
            }
        }
        // SAFETY: same contract.
        unsafe { self.base.free(block, variant) }
    }
}

/// See [`Kind::PrematureReuse`].
pub struct PrematureReuse<S: Subject> {
    base: Base<S>,
    allocs: AtomicUsize,
    /// Live inner blocks wrongly on the free list, with the allocation count from which they
    /// may be handed out again; each entry holds a reference.
    pool: Mutex<VecDeque<(usize, usize)>>,
}

impl<S: Subject> PrematureReuse<S> {
    const KIND: Kind = Kind::PrematureReuse;
    const PERIOD: usize = 12;
    const DELAY: usize = 8;

    /// `inner` with the bug.
    pub fn new(inner: S) -> Self {
        PrematureReuse {
            base: Base::new(inner),
            allocs: AtomicUsize::new(0),
            pool: Mutex::new(VecDeque::new()),
        }
    }
}

impl<S: Subject> Subject for PrematureReuse<S> {
    forward!(name);
    forward!(free);
    forward!(resize);

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let n = self.allocs.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            let mut book = self.base.book();
            let ready = pool.iter().position(|&(base, at)| {
                at <= n && book.inner.get(&base).is_some_and(|b| fits(&b.held, &req))
            });
            if let Some(i) = ready {
                let (base, _) = pool.remove(i).expect("found");
                let b = book
                    .inner
                    .get_mut(&base)
                    .expect("pooled blocks stay booked");
                b.refs -= 1;
                let held = b.held;
                book.hand_out(base, base);
                drop((book, pool));
                return Ok(Some(reissue(&held, &req)));
            }
        }
        // SAFETY: same contract.
        let g = unsafe { self.base.alloc(req) }?;
        if let Some(g) = &g
            && n.is_multiple_of(Self::PERIOD)
            && g.usable > 0
        {
            let base = g.ptr.addr().get();
            self.base.book().retain(base);
            let mut pool = self.pool.lock().unwrap_or_else(PoisonError::into_inner);
            pool.push_back((base, n + Self::DELAY));
        }
        Ok(g)
    }
}

impl<S: Subject> Drop for PrematureReuse<S> {
    fn drop(&mut self) {
        let pool = std::mem::take(self.pool.get_mut().unwrap_or_else(PoisonError::into_inner));
        for (base, _) in pool {
            // SAFETY: each pool entry held a reference.
            unsafe { self.base.release(base) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_every_kind_has_an_expected_failure() {
        for k in Kind::ALL {
            assert_eq!(Kind::from_name(k.name()), Some(k));
            assert!(!k.caught_as().is_empty());
        }
    }
}
