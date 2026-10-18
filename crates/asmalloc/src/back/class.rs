//! Per-class operations: filling and laundering magazines, single-slot allocation and free, and
//! assigning regions to a class.
//!
//! A class's regions are found through its summary tree, which visits them in address order. New
//! slots are carved from one region at a time, the class's carving region. When it is exhausted,
//! the thread that notices picks the lowest unassigned region, commits its fixed metadata (so
//! that the region never carries a class tag without a readable header), takes it with a
//! compare-and-swap on the region table, gives it its huge-page advice, and publishes it with a
//! compare-and-swap on the class's carving word. If another thread published first, the loser
//! zeroes what it wrote and gives its region back with a compare-and-swap; no other thread has
//! named it, because nothing points into a region until it has been carved from.

use super::group::{self, Group, Open};
use super::host::ClassHost;
use super::layout::{Grow, Hints, Layout};
use super::policy::{large_retained_cap, move_piece, wants_huge};
use super::region::{self, Carve};
use super::tree::{self, Host};
use super::word::{BITS, Word, position};
use super::{Back, Block, Kind};
use crate::classes::{Class, SmallClass};
use crate::os::{HugePages, Os};
use crate::space::{RegionId, RegionTag};
use core::ptr::NonNull;
use core::sync::atomic::Ordering::SeqCst;

impl<'a, O: Os> Back<'a, O> {
    /// Writes up to `out.len()` pointers to free slots of `class` into `out`, and returns how
    /// many. Freed slots come first, lowest address first, then never-used ones carved from the
    /// class's newest region. Fewer than asked means the heap is out of address space or the OS
    /// refused memory; zero means not even one slot could be had.
    pub fn fill(&self, class: SmallClass, out: &mut [*mut u8]) -> usize {
        let Some(host) = self.host(class.class()) else {
            return 0;
        };
        let mut got = 0;
        let (mut resident, mut revived) = (0, 0);
        while got < out.len() {
            let want = (out.len() - got).min(BITS as usize) as u32;
            let Some(taken) = tree::claim(&host, want) else {
                break;
            };
            let settled = region::settle(&host, taken.region, taken.word, taken.bits);
            resident += settled.resident;
            revived += settled.revived;
            let base = taken.word * BITS;
            let high = base + (BITS - 1 - taken.bits.leading_zeros());
            let mut bits = taken.bits;
            let slots = core::iter::from_fn(|| {
                let bit = (bits != 0).then(|| bits.trailing_zeros())?;
                bits &= bits - 1;
                Some(base + bit)
            });
            got = self.put(host.layout(), out, got, taken.region, high, slots);
        }
        self.unretain(class.class(), resident);
        self.revive(class.class(), revived);
        while got < out.len() {
            let want = (out.len() - got).min(u32::MAX as usize) as u32;
            let Some((region, first, count)) = self.carve(&host, class.class(), want) else {
                break;
            };
            let high = first + count - 1;
            got = self.put(host.layout(), out, got, region, high, first..first + count);
        }
        got
    }

    /// Writes the addresses of `slots` of `region`, every one of them at most `high`, at
    /// `out[at..]`, and returns the next position. `high` is checked to be one of the region's
    /// slots, so every slot below it is too.
    #[inline(always)]
    fn put(
        &self,
        layout: &Layout,
        out: &mut [*mut u8],
        mut at: usize,
        region: RegionId,
        high: u32,
        slots: impl Iterator<Item = u32>,
    ) -> usize {
        if high >= layout.slots() {
            self.abort("a slot index outside its region");
        }
        let base = self.space.region(region).as_ptr();
        for slot in slots {
            if let Some(entry) = out.get_mut(at) {
                *entry = base.wrapping_add(layout.slot_bytes(slot));
            }
            at += 1;
        }
        at
    }

    /// Returns free slots of `class` to their bitmaps, grouped by bitmap word as they come
    /// (`group`), with one atomic OR per group and one walk up the summary tree for the batch.
    /// Aborts the process if a pointer is not the start of a slot of `class` that has been
    /// handed out, if a slot is already free, or if a slot appears twice within a part of 64
    /// pointers (`group::MAX_SLOTS`): those are double frees or frees of foreign pointers.
    pub fn launder(&self, class: SmallClass, objects: &[*mut u8]) {
        self.release_all(class.class(), objects, true);
    }

    /// `launder` for any class. With `count`, the slots are added to the retained count; a
    /// caller that has just returned their pages to the OS says no.
    fn release_all(&self, class: Class, objects: &[*mut u8], count: bool) {
        let Some(&first) = objects.first() else {
            return;
        };
        let Some(host) = self.host(class) else {
            self.abort("free of a pointer into a class that has no regions");
        };
        let checker = Checker::new(self, &host, class);
        let mut met = checker.meet(first);
        let mut release = tree::Release::new(&host, met.region);
        let mut group = Group::new();
        for part in objects.chunks(group::MAX_SLOTS) {
            let slot = checker.check(part[0], &mut met);
            let mut held = Open::slot(met.region.index(), slot);
            for &object in &part[1..] {
                let slot = checker.check(object, &mut met);
                let next = Open::slot(met.region.index(), slot);
                if next.key == held.key {
                    held.add(&release, next.bits);
                } else {
                    held = group.switch(&mut release, held, next);
                }
            }
            group.finish(&mut release, held);
        }
        release.finish();
        if count {
            self.retain(class, objects.len().saturating_mul(class.size()));
        }
    }

    /// For a large slot the program is freeing and still holds: if keeping it resident would
    /// take the large classes' retained count over its cap, returns its pages to the OS first
    /// and says so. Aborts as `launder` does, and if the slot is free already.
    ///
    /// The count is read here and raised later, by the release, so the cap is per thread:
    /// threads freeing at the same moment can each see room and together pass the cap by one
    /// slot for every thread but the first. The next free sees the true count.
    fn purge_before_release(&self, ptr: NonNull<u8>, class: Class) -> bool {
        let cap = large_retained_cap(self.space.plan().geometry());
        if self.retained_large().saturating_add(class.size()) <= cap {
            return false;
        }
        let Some(host) = self.host(class) else {
            self.abort("free of a pointer into a class that has no regions");
        };
        let (region, slot) = self.validate(&host, ptr.as_ptr(), class);
        let (word, bit) = position(slot);
        // The slot's bit is clear while the program holds it. If it is set, this is a second
        // free of a free slot, which the release below would also catch; catching it here
        // keeps the purge away from a slot an allocation may be about to claim.
        if tree::word(&host, tree::Node::leaf(region, word)).load() & (1u64 << bit) != 0 {
            self.abort("double free: a large object was freed while it was free");
        }
        // SAFETY: BACK-CLASS-01 in docs/safety/asmalloc/back/class-safety.md. The program is
        // freeing the slot and has not been given it back, its bit was clear a moment ago,
        // and the slot is below the watermark (`validate`).
        unsafe { region::purge_held(&host, region, slot) }
    }

    /// The region and slot of a pointer the caller says is a handed-out slot of `class`, or an
    /// abort if it is not (see [`Checker::check`]).
    fn validate(
        &self,
        host: &ClassHost<'a, O>,
        object: *const u8,
        class: Class,
    ) -> (RegionId, u32) {
        let checker = Checker::new(self, host, class);
        let mut met = checker.meet(object);
        let slot = checker.check(object, &mut met);
        (met.region, slot)
    }

    /// Allocates one slot of `class` straight from the bitmaps: a freed slot if the class has
    /// one, lowest address first, else a carved one. For a large class, records `size` as the
    /// object's current size (see [`Back::size`]); it must be at most the class size, or the
    /// process aborts. Small classes record nothing and ignore `size`. `None` if the heap is out
    /// of address space, the OS refused memory, or a slot of the class does not fit in a region
    /// (use [`Back::alloc_run`]).
    pub fn alloc(&self, class: Class, size: usize) -> Option<Block> {
        if size > class.size() {
            self.abort("an allocation larger than its class");
        }
        let host = self.host(class)?;
        let (region, slot, zeroed) = if let Some(taken) = tree::claim(&host, 1) {
            let settled = region::settle(&host, taken.region, taken.word, taken.bits);
            self.unretain(class, settled.resident);
            self.revive(class, settled.revived);
            let slot = taken.word * BITS + taken.bits.trailing_zeros();
            (taken.region, slot, settled.zeroed != 0)
        } else {
            let (region, first, _) = self.carve(&host, class, 1)?;
            (region, first, true)
        };
        if let Some(at) = host.layout().size_word(slot) {
            host.meta(region, at).store(size as u64);
        }
        if !class.is_small() {
            // As for a large free: see `Back::free`.
            self.heartbeat();
        }
        Some(Block {
            ptr: self.space.slot(region, class, slot),
            zeroed,
        })
    }

    /// Records `size` as the current size of the large object or run starting at `ptr`, after
    /// the caller resized it in place. Aborts if `ptr` is not the start of a handed-out large
    /// slot or a run, or if `size` exceeds the slot or run. Small slots record no size, so for
    /// them this does nothing.
    pub fn resize(&self, ptr: NonNull<u8>, size: usize) {
        match self.lookup(ptr.as_ptr()) {
            Some(Kind::Slot(class)) if class.is_small() => {}
            Some(Kind::Slot(class)) => {
                let Some(host) = self.host(class) else {
                    self.abort("a resize in a class that has no regions");
                };
                let (region, slot) = self.validate(&host, ptr.as_ptr(), class);
                if size > class.size() {
                    self.abort("a resize beyond the object's slot");
                }
                if let Some(at) = host.layout().size_word(slot) {
                    host.meta(region, at).store(size as u64);
                }
            }
            Some(Kind::Run(regions)) => {
                if size > regions << self.space.plan().region_shift() {
                    self.abort("a resize beyond the object's run");
                }
                self.set_run_size(ptr, size);
            }
            None => self.abort("a resize of a pointer that is not the start of a heap object"),
        }
    }

    /// Records `size` as the current size of the large object or run starting at `ptr`, which
    /// the caller is resizing in place, and, if that shrinks it, returns to the OS the whole
    /// pages between its new usable size and its old one (the usable size is the recorded size
    /// rounded up to a page, which is what the program may have written). Aborts as
    /// [`Back::resize`] does. For a small slot this does nothing.
    ///
    /// The size is recorded first: only the owner asks for a live object's size, and the owner
    /// is here. The pages were the live object's, so they were never in a retained count and
    /// none changes; no bitmap changes either. If the object later grows back in place the
    /// pages read as zero, which is as good as the stale bytes a grow may otherwise expose.
    ///
    /// There is no hysteresis: every shrink that crosses a page boundary is one system call,
    /// so a program that shrinks a block a page at a time pays a call each time, as it does
    /// with glibc's `realloc` above the `mmap` threshold. A shrink within a page makes none.
    pub fn resize_in_place(&self, ptr: NonNull<u8>, size: usize) {
        let page = self.space.plan().geometry().page_size();
        let tail = |old: usize, limit: usize| {
            let from = size.next_multiple_of(page);
            let to = old.next_multiple_of(page).min(limit);
            (from < to).then(|| (from, to - from))
        };
        match self.lookup(ptr.as_ptr()) {
            Some(Kind::Slot(class)) if class.is_small() => {}
            Some(Kind::Slot(class)) => {
                let Some(host) = self.host(class) else {
                    self.abort("a resize in a class that has no regions");
                };
                let (region, slot) = self.validate(&host, ptr.as_ptr(), class);
                if size > class.size() {
                    self.abort("a resize beyond the object's slot");
                }
                let Some(at) = host.layout().size_word(slot) else {
                    return;
                };
                let old = host.meta(region, at).load() as usize;
                host.meta(region, at).store(size as u64);
                if let Some((from, len)) = tail(old, class.size()) {
                    // SAFETY: BACK-CLASS-02 in docs/safety/asmalloc/back/class-safety.md. The
                    // caller owns the object in this slot and has just given up everything
                    // past `from`, which lies inside the slot.
                    let _ = unsafe { region::purge_part(&host, region, slot, from, len) };
                }
            }
            Some(Kind::Run(regions)) => {
                let whole = regions << self.space.plan().region_shift();
                if size > whole {
                    self.abort("a resize beyond the object's run");
                }
                let old = self.run_size(ptr.as_ptr());
                self.set_run_size(ptr, size);
                if let Some((from, len)) = tail(old, whole) {
                    let start = ptr.map_addr(|a| a.saturating_add(from));
                    // SAFETY: BACK-CLASS-02. The caller owns the run and has just given up
                    // everything past `from`, which lies inside the run's regions.
                    let _ = unsafe { self.space.purge(start, len) };
                }
            }
            None => self.abort("a resize of a pointer that is not the start of a heap object"),
        }
    }

    /// Moves the contents of the large object or run at `from` to `to` and frees `from`: copies
    /// `len` bytes and gives the block up, as a copy followed by [`Back::free_by_address`]
    /// would. A block of more than one piece (`policy::move_piece`) is copied a piece at a
    /// time, and the whole pages of `from` behind each piece go back to the OS before the next
    /// is copied, so the two copies are both resident for one piece at most. Aborts if `from`
    /// is not the start of a handed-out large slot or a run.
    ///
    /// When every page of a slot went back this way, the slot is released as purged memory:
    /// zero, clean and not counted as retained, exactly as a free over the large cap leaves
    /// it, so the free neither purges nor counts those pages again.
    ///
    /// # Safety
    ///
    /// `from` is a large block of this heap that the caller owns and gives up, readable for
    /// `len` bytes; `to` is writable for `len` bytes and does not overlap it; nothing else
    /// reads or writes either range during the call.
    pub unsafe fn move_large(&self, from: NonNull<u8>, to: NonNull<u8>, len: usize) {
        // SAFETY: the caller's promise, passed on.
        unsafe { self.move_large_with(from, to, len, || {}) }
    }

    /// [`Back::move_large`], calling `observe` after each piece is copied and the pages
    /// behind it returned, so that a test can watch the move.
    ///
    /// # Safety
    ///
    /// As for [`Back::move_large`].
    pub(crate) unsafe fn move_large_with(
        &self,
        from: NonNull<u8>,
        to: NonNull<u8>,
        len: usize,
        mut observe: impl FnMut(),
    ) {
        let piece = move_piece(self.space.plan().geometry());
        let kind = self.lookup(from.as_ptr());
        let slot = match kind {
            Some(Kind::Slot(class)) if !class.is_small() => Some(class),
            Some(Kind::Run(_)) => None,
            _ => self.abort("a move of a pointer that is not a large block of the heap"),
        };
        if len <= piece {
            // SAFETY: BACK-CLASS-03: the caller's promise about the two ranges.
            unsafe { from.as_ptr().copy_to_nonoverlapping(to.as_ptr(), len) };
            return self.free_by_address(from);
        }
        // What `from` may have resident: its whole slot or run. Past the bytes copied that
        // is the rest of the old object and whatever an earlier owner of the slot left.
        let (whole, host) = match (slot, kind) {
            (Some(class), _) => {
                let Some(host) = self.host(class) else {
                    self.abort("a move in a class that has no regions");
                };
                let (region, index) = self.validate(&host, from.as_ptr(), class);
                let (word, bit) = position(index);
                if tree::word(&host, tree::Node::leaf(region, word)).load() & (1u64 << bit) != 0 {
                    self.abort("a move of a large object that is free");
                }
                (class.size(), Some((host, region, index)))
            }
            (None, Some(Kind::Run(regions))) => (regions << self.space.plan().region_shift(), None),
            _ => return,
        };
        // Returns `from`'s pages in `start..end`, both piece or page boundaries.
        let give_back = |start: usize, end: usize| -> bool {
            if start >= end {
                return true;
            }
            match &host {
                // SAFETY: BACK-CLASS-03: the caller holds the slot, and the range is inside it
                // and is either copied already or past everything that is copied.
                Some((host, region, index)) => unsafe {
                    region::purge_part(host, *region, *index, start, end - start)
                },
                None => {
                    let at = from.map_addr(|a| a.saturating_add(start));
                    // SAFETY: BACK-CLASS-03, likewise for a run the caller owns.
                    unsafe { self.space.purge(at, end - start) }.is_ok()
                }
            }
        };
        let (mut done, mut all) = (0, true);
        while done < len {
            let step = piece.min(len - done);
            // SAFETY: BACK-CLASS-03: `done + step <= len`, inside both ranges, which do not
            // overlap; the bytes of `from` read here have not been returned.
            unsafe {
                from.as_ptr()
                    .add(done)
                    .copy_to_nonoverlapping(to.as_ptr().add(done), step);
            }
            // Only whole pieces go back here: the last may end inside a page.
            if step == piece {
                all &= give_back(done, done + step);
            }
            done += step;
            observe();
        }
        all &= give_back(len - len % piece, whole);
        match host {
            Some((host, region, index)) => {
                let Some(class) = slot else {
                    return;
                };
                if all {
                    region::mark_purged(&host, region, index);
                }
                self.release_all(class, &[from.as_ptr()], !all);
                self.heartbeat();
            }
            None => self.free_run(from),
        }
    }

    /// The current size of the object starting at `ptr`: the recorded size of a large object or
    /// run (what it was last allocated or resized to), the class size of a small one. `None` if
    /// `ptr` is not the start of a slot or run. Only meaningful for an object the caller holds.
    pub fn size(&self, ptr: *const u8) -> Option<usize> {
        match self.lookup(ptr)? {
            Kind::Slot(class) if class.is_small() => Some(class.size()),
            Kind::Slot(class) => {
                let host = self.host(class)?;
                let location = self.space.locate(ptr)?;
                let slot = self.space.plan().slot_at(location, class)?;
                let at = host.layout().size_word(slot)?;
                Some(host.meta(location.region, at).load() as usize)
            }
            Kind::Run(_) => Some(self.run_size(ptr)),
        }
    }

    /// Frees one slot of `class` to its bitmap. Aborts as [`Back::launder`] does. A large slot's
    /// recorded size is left as it was; the next allocation of the slot writes it.
    ///
    /// A free of a large slot that would take the large classes' retained count over its cap
    /// returns that slot's pages to the OS first, while the caller still holds the slot, and
    /// then releases it uncounted (`purge`).
    pub fn free(&self, ptr: NonNull<u8>, class: Class) {
        if class.is_small() {
            return self.release_all(class, &[ptr.as_ptr()], true);
        }
        let purged = self.purge_before_release(ptr, class);
        self.release_all(class, &[ptr.as_ptr()], !purged);
        // A large free is a slow path with no depot behind it, so it beats the heart itself:
        // a program that only moves large blocks still gets its decay.
        self.heartbeat();
    }

    /// Frees whatever `ptr` is the start of: a slot, whose class the region table gives, or a
    /// run. Aborts if it is neither. This is the C `free` of an uncached object.
    pub fn free_by_address(&self, ptr: NonNull<u8>) {
        match self.lookup(ptr.as_ptr()) {
            Some(Kind::Slot(class)) => self.free(ptr, class),
            Some(Kind::Run(_)) => self.free_run(ptr),
            None => self.abort("free of a pointer that is not the start of a heap object"),
        }
    }

    /// What `ptr` is the start of, if anything: a slot of a class (handed out or not), or a run
    /// of regions. One load from the region table, then arithmetic.
    pub fn lookup(&self, ptr: *const u8) -> Option<Kind> {
        let location = self.space.locate(ptr)?;
        match self.state.table.get(location.region) {
            RegionTag::Class(class) => {
                self.space.plan().slot_at(location, class)?;
                Some(Kind::Slot(class))
            }
            RegionTag::RunHead if location.offset == 0 => {
                Some(Kind::Run(self.run_length(location.region)))
            }
            _ => None,
        }
    }

    /// The usable size of the object starting at `ptr`, for `malloc_usable_size`: the class
    /// size of a small slot; for a large slot or a run, its recorded size rounded up to a page,
    /// never the whole slot, so a caller that fills what it may use touches about what it asked
    /// for.
    pub fn usable_size(&self, ptr: *const u8) -> Option<usize> {
        let size = self.size(ptr)?;
        match self.lookup(ptr)? {
            Kind::Slot(class) if class.is_small() => Some(size),
            _ => Some(size.next_multiple_of(self.space.plan().geometry().page_size())),
        }
    }

    /// Carves up to `n` slots from the class's carving region, assigning a new one when it is
    /// exhausted. Returns the region, the first slot and the count.
    fn carve(&self, host: &ClassHost<'a, O>, class: Class, n: u32) -> Option<(RegionId, u32, u32)> {
        loop {
            let seen = host.class.carving.load(SeqCst);
            if seen != 0 {
                let Some(region) = self.space.plan().region(seen as usize - 1) else {
                    self.abort("a class's carving region is outside the heap");
                };
                match region::carve(host, region, n) {
                    Carve::Got { first, count } => return Some((region, first, count)),
                    Carve::Failed => return None,
                    Carve::Exhausted => {}
                }
            }
            if !self.assign(host, class, seen) {
                return None;
            }
        }
    }

    /// Replaces the class's carving word, read as `seen`, with a fresh region. Returns `true` if
    /// the word has changed since, by this thread or another, and `false` if no region could be
    /// had.
    fn assign(&self, host: &ClassHost<'a, O>, class: Class, seen: u32) -> bool {
        let plan = self.space.plan();
        // A class that has filled a region before has carved far past the huge-page threshold,
        // so its new region uses huge pages from the start, if its class gets them at all.
        let huge = seen != 0 && wants_huge(class.size(), host.layout().page_size());
        let layout = host.layout();
        let (start, end) = layout.fixed_commit();
        for region in plan.regions() {
            if host.class.carving.load(SeqCst) != seen {
                return true;
            }
            if self.state.table.get(region) != RegionTag::Unassigned {
                continue;
            }
            // Commit the fixed metadata before the region can carry this class's tag, so that
            // whoever learns the tag (a free of any pointer into the region) can read the header.
            // Committing is idempotent, so doing it for a region another thread then takes
            // costs only commit charge.
            if !host.commit_meta(region, start, end - start) {
                return false;
            }
            if self
                .state
                .table
                .replace(region, RegionTag::Unassigned, RegionTag::Class(class))
                .is_err()
            {
                continue;
            }
            self.prepare(host, region, huge);
            let published = region.index() as u32 + 1;
            if host
                .class
                .carving
                .compare_exchange(seen, published, SeqCst, SeqCst)
                .is_ok()
            {
                return true;
            }
            self.unprepare(host, region, huge);
            let _ =
                self.state
                    .table
                    .replace(region, RegionTag::Class(class), RegionTag::Unassigned);
            return true;
        }
        false
    }

    /// Readies a region this thread has just taken from the table, whose fixed metadata is
    /// committed: advises the region for or against huge pages, recording a huge-page part from
    /// the start if `huge`.
    fn prepare(&self, host: &ClassHost<'a, O>, region: RegionId, huge: bool) {
        let layout = host.layout();
        let pages = layout.region_pages() as usize;
        let advice = if huge {
            let hints = Hints {
                huge_from: Some(0),
                cursor: 0,
            };
            host.meta(region, layout.hints_word())
                .fetch_or(hints.pack());
            HugePages::Prefer
        } else {
            HugePages::Avoid
        };
        host.advise(region, 0, pages * layout.page_size(), advice);
    }

    /// Undoes `prepare`'s writes, so that the region's metadata is all zero again.
    fn unprepare(&self, host: &ClassHost<'a, O>, region: RegionId, huge: bool) {
        if huge {
            host.meta(region, host.layout().hints_word()).fetch_and(0);
        }
    }
}

/// Checks pointers that a caller says are handed-out slots of one class, as a free or a launder
/// must before it touches a bitmap. A pointer passes if it lies in a region of the class,
/// starts a slot there, and lies below the region's watermark; anything else aborts.
///
/// A batch of pointers reads the region table and the watermark once for each region it meets,
/// not once per pointer, and the arithmetic uses the class's layout, worked out once. Both are
/// safe to remember for a batch. A region seen with a class keeps it: a tag goes back to
/// unassigned only in the race `Back::assign` loses, before anything has been carved there. A
/// watermark only rises, so the one remembered is never above the true one, and every pointer
/// of the batch was handed out before the batch began.
struct Checker<'c, 'a, O: Os> {
    back: &'c Back<'a, O>,
    host: &'c ClassHost<'a, O>,
    class: Class,
    layout: Layout,
    base: usize,
    region_shift: u32,
}

/// A region a batch has met, with its watermark, at most the region's slot count.
#[derive(Clone, Copy, Debug)]
struct Met {
    region: RegionId,
    limit: u32,
}

impl<'c, 'a, O: Os> Checker<'c, 'a, O> {
    #[inline]
    fn new(back: &'c Back<'a, O>, host: &'c ClassHost<'a, O>, class: Class) -> Self {
        Checker {
            back,
            host,
            class,
            layout: *host.layout(),
            base: back.space.base().addr().get(),
            region_shift: back.space.plan().region_shift(),
        }
    }

    /// The slot `object` starts, or an abort if it is not a handed-out slot of the class.
    /// `met` is the region the batch met last, which this replaces if `object` lies in another.
    #[inline(always)]
    fn check(&self, object: *const u8, met: &mut Met) -> u32 {
        let delta = object.addr().wrapping_sub(self.base);
        if delta >> self.region_shift != met.region.index() {
            *met = self.meet(object);
        }
        let offset = delta & ((1 << self.region_shift) - 1);
        let slot = self.layout.slot_at(offset);
        if self.layout.slot_bytes(slot) != offset {
            self.back
                .abort("free of a pointer that is not the start of a slot");
        }
        if slot >= met.limit {
            self.back.abort("free of a slot that was never handed out");
        }
        slot
    }

    /// The region `object` lies in, checked to be the class's, with its watermark.
    #[inline(never)]
    fn meet(&self, object: *const u8) -> Met {
        let Some(location) = self.back.space.locate(object) else {
            self.back.abort("free of a pointer outside the heap");
        };
        let region = location.region;
        if self.back.state.table.get(region) != RegionTag::Class(self.class) {
            self.back.abort("free of a pointer with the wrong class");
        }
        let grow = self.host.meta(region, self.layout.grow_word());
        let limit = Grow::unpack(grow.load()).carved.min(self.layout.slots());
        Met { region, limit }
    }
}
