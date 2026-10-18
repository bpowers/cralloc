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
use super::layout::{Grow, Hints, Layout, Recorded};
use super::policy::{large_retained_cap, move_piece, wants_huge};
use super::region::{self, Carve};
use super::tree::{self, Host};
use super::word::{BITS, Word, position};
use super::{Back, Block, Kind};
use crate::classes::{Class, SmallClass};
use crate::os::{HugePages, Os};
use crate::space::{RegionId, RegionTag};
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::SeqCst;

/// How a caller of [`Back::fill`] and [`Back::launder`] keeps slot pointers, so that the back
/// end reads and writes the caller's own slices with no copy between: raw pointers, or the
/// depot's `NonNull` and `Option<NonNull>`.
pub trait SlotPointer: Copy {
    /// The pointer to a slot, which is never null.
    fn slot(slot: NonNull<u8>) -> Self;
    /// The address the pointer holds; null for `None`.
    fn address(self) -> *mut u8;
}

impl SlotPointer for *mut u8 {
    #[inline(always)]
    fn slot(slot: NonNull<u8>) -> Self {
        slot.as_ptr()
    }

    #[inline(always)]
    fn address(self) -> *mut u8 {
        self
    }
}

impl SlotPointer for NonNull<u8> {
    #[inline(always)]
    fn slot(slot: NonNull<u8>) -> Self {
        slot
    }

    #[inline(always)]
    fn address(self) -> *mut u8 {
        self.as_ptr()
    }
}

impl SlotPointer for Option<NonNull<u8>> {
    #[inline(always)]
    fn slot(slot: NonNull<u8>) -> Self {
        Some(slot)
    }

    #[inline(always)]
    fn address(self) -> *mut u8 {
        self.map_or(core::ptr::null_mut(), NonNull::as_ptr)
    }
}

impl<'a, O: Os> Back<'a, O> {
    /// Writes up to `out.len()` pointers to free slots of `class` into `out`, and returns how
    /// many. Freed slots come first, lowest address first, then never-used ones carved from the
    /// class's newest region. Fewer than asked means the heap is out of address space or the OS
    /// refused memory; zero means not even one slot could be had. The pointers are written as
    /// whatever the caller keeps them as: raw pointers, or the depot's `Option<NonNull<u8>>`.
    pub fn fill<P: SlotPointer>(&self, class: SmallClass, out: &mut [P]) -> usize {
        let Some(host) = self.host(class.class()) else {
            return 0;
        };
        let mut got = 0;
        let (mut resident, mut revived) = (0, 0);
        let mut cursor = None;
        while got < out.len() {
            let want = (out.len() - got).min(BITS as usize) as u32;
            let Some(taken) = tree::claim_next(&host, want, &mut cursor) else {
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
        tree::finish_run(&host, cursor);
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
    fn put<P: SlotPointer>(
        &self,
        layout: &Layout,
        out: &mut [P],
        mut at: usize,
        region: RegionId,
        high: u32,
        slots: impl Iterator<Item = u32>,
    ) -> usize {
        if high >= layout.slots() {
            self.abort("a slot index outside its region");
        }
        let base = self.space.region(region);
        for slot in slots {
            if let Some(entry) = out.get_mut(at) {
                let offset = layout.slot_bytes(slot);
                *entry = P::slot(base.map_addr(|a| a.saturating_add(offset)));
            }
            at += 1;
        }
        at
    }

    /// Returns free slots of `class` to their bitmaps, grouped by bitmap word as they come
    /// (`group`), with one atomic OR per group and one walk up the summary tree for the batch.
    /// Aborts the process if a pointer is not the start of a slot of `class` that has been
    /// handed out, if a slot is already free, or if a slot appears twice within a part (up to 64
    /// pointers, fewer when they are scattered; see `group`): those are double frees or frees
    /// of foreign pointers.
    ///
    /// # Safety
    ///
    /// Every pointer in `objects` that is a handed-out slot of `class` is one the caller owns
    /// and gives up: a slot laundered while another still uses it is handed out again, and then
    /// has two owners. (Anything else aborts, as above.)
    pub unsafe fn launder<P: SlotPointer>(&self, class: SmallClass, objects: &[P]) {
        self.release_all(class.class(), objects, true);
    }

    /// `launder` for any class. With `count`, the slots are added to the retained count; a
    /// caller that has just returned their pages to the OS says no.
    fn release_all<P: SlotPointer>(&self, class: Class, objects: &[P], count: bool) {
        let Some(&first) = objects.first() else {
            return;
        };
        let Some(host) = self.host(class) else {
            self.abort("free of a pointer into a class that has no regions");
        };
        let checker = Checker::new(self, &host, class);
        let mut met = checker.meet(first.address());
        let mut release = tree::Release::new(&host, met.region);
        let mut group = Group::new();
        let mut rest = objects;
        while let Some((&first, tail)) = rest.split_first() {
            let slot = checker.check(first.address(), &mut met);
            let mut held = Open::slot(met.region.index(), slot);
            let (part, after) = tail.split_at(tail.len().min(group::MAX_SLOTS - 1));
            rest = after;
            for (at, &object) in part.iter().enumerate() {
                let slot = checker.check(object.address(), &mut met);
                let next = Open::slot(met.region.index(), slot);
                if next.key == held.key {
                    held.add(&release, next.bits);
                } else {
                    held = group.switch(&mut release, held, next);
                    if group.is_full() {
                        // The part ends here; the rest of this chunk starts the next.
                        rest = &tail[at + 1..];
                        break;
                    }
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
    ///
    /// A slot advised for huge pages because its object grew is always purged here, and its
    /// advice taken back, so that no free slot is advised: the next block in it may be used
    /// sparsely, and a partial purge of a huge page would return nothing.
    fn purge_before_release(&self, ptr: NonNull<u8>, class: Class) -> bool {
        let geometry = self.space.plan().geometry();
        let over =
            self.retained_large().saturating_add(class.size()) > large_retained_cap(geometry);
        // Only a slot that holds a whole huge page can have been advised.
        if !over && class.size() < geometry.huge_page_size() {
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
        let huge = region::recorded(&host, region, slot).is_some_and(|r| r.huge);
        if !over && !huge {
            return false;
        }
        // SAFETY: BACK-CLASS-01 in docs/safety/asmalloc/back/class-safety.md. The program is
        // freeing the slot and has not been given it back, its bit was clear a moment ago,
        // and the slot is below the watermark (`validate`).
        let purged = unsafe { region::purge_held(&host, region, slot) };
        region::unadvise(&host, region, slot);
        purged
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

    /// How far into slot `slot` of `class` in `region` the block at `object` starts: zero, or
    /// the slot's stagger. `object` is one `validate` accepted for that slot.
    fn offset_in_slot(
        &self,
        region: RegionId,
        class: Class,
        slot: u32,
        object: *const u8,
    ) -> usize {
        object.addr() - self.space.slot(region, class, slot).addr().get()
    }

    /// Allocates one slot of `class` straight from the bitmaps: a freed slot if the class has
    /// one, lowest address first, else a carved one. For a large class, records `size` as the
    /// object's current size (see [`Back::size`]); it must be at most the class size, or the
    /// process aborts. Small classes record nothing and ignore `size`. `None` if the heap is out
    /// of address space, the OS refused memory, or a slot of the class does not fit in a region
    /// (use [`Back::alloc_run`]).
    pub fn alloc(&self, class: Class, size: usize) -> Option<Block> {
        self.alloc_at(class, size, false)
    }

    /// [`Back::alloc`] for a large block whose alignment is at most `policy::STAGGER_STEP`: it
    /// starts at its slot's stagger (`Layout::stagger`) if the object fits there, so that large
    /// blocks of a class do not all start at the same offset in the caches. A small class is
    /// never staggered.
    pub fn alloc_staggered(&self, class: Class, size: usize) -> Option<Block> {
        self.alloc_at(class, size, true)
    }

    fn alloc_at(&self, class: Class, size: usize, stagger: bool) -> Option<Block> {
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
        if let Some(before) = region::recorded(&host, region, slot) {
            region::record(&host, region, slot, Recorded { size, ..before });
            // The last free could not take the slot's huge-page advice back; try again, so
            // that this block, which has not grown, does not get huge pages.
            region::unadvise(&host, region, slot);
        }
        if !class.is_small() {
            // As for a large free: see `Back::free`.
            self.heartbeat();
        }
        let into = host.layout().stagger(slot);
        let into = if stagger && size + into <= class.size() {
            into
        } else {
            0
        };
        let start = self.space.slot(region, class, slot);
        Some(Block {
            ptr: start.map_addr(|a| a.saturating_add(into)),
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
                let into = self.offset_in_slot(region, class, slot, ptr.as_ptr());
                if size + into > class.size() {
                    self.abort("a resize beyond the object's slot");
                }
                if let Some(before) = region::recorded(&host, region, slot) {
                    region::record(&host, region, slot, Recorded { size, ..before });
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
    /// In a slot advised for huge pages the tail is returned in whole huge pages only. A block
    /// that grows in place by appending densely has the advice extended to its new size
    /// (`region::advise_grown`).
    ///
    /// # Safety
    ///
    /// `ptr` is a live large block of this heap that the caller owns, and nothing reads or
    /// writes it during the call; if `size` is smaller than its recorded size, the caller gives
    /// up every byte past `size`.
    pub unsafe fn resize_in_place(&self, ptr: NonNull<u8>, size: usize) {
        let page = self.space.plan().geometry().page_size();
        // The whole pages between the new end and the old one, counted from the start of the
        // slot or run, which the block starts `into` bytes past.
        let tail = |old: usize, limit: usize, into: usize| {
            let from = (into + size).next_multiple_of(page);
            let to = (into + old).next_multiple_of(page).min(limit);
            (from < to).then(|| (from, to - from))
        };
        match self.lookup(ptr.as_ptr()) {
            Some(Kind::Slot(class)) if class.is_small() => {}
            Some(Kind::Slot(class)) => {
                let Some(host) = self.host(class) else {
                    self.abort("a resize in a class that has no regions");
                };
                let (region, slot) = self.validate(&host, ptr.as_ptr(), class);
                let into = self.offset_in_slot(region, class, slot, ptr.as_ptr());
                if size + into > class.size() {
                    self.abort("a resize beyond the object's slot");
                }
                let Some(before) = region::recorded(&host, region, slot) else {
                    return;
                };
                let old = before.size;
                region::record(&host, region, slot, Recorded { size, ..before });
                let piece = move_piece(self.space.plan().geometry());
                let last = old.saturating_sub(piece) & !(page - 1);
                let written = ptr.map_addr(|a| a.saturating_add(last));
                // SAFETY: BACK-CLASS-02: the caller owns the block and does not use it during
                // the resize; `last..old` lies inside its first `old` bytes.
                let dense = || unsafe { mostly_data(written.as_ptr(), old - last, page) };
                region::advise_grown(&host, region, slot, into, old, size, dense);
                if let Some((from, len)) = tail(old, class.size(), into) {
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
                if let Some((from, len)) = tail(old, whole, 0) {
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
    /// The piecewise copy skips source pages that are all zero, so a block written sparsely
    /// stays sparse: reading an untouched page maps the kernel's shared zero page and costs no
    /// memory. A skipped page must read as zero at `to`, so unless the caller says `to` reads
    /// as zero (`zeroed`), its pages are purged once before the copy.
    ///
    /// # Safety
    ///
    /// `from` is a large block of this heap that the caller owns and gives up, readable for
    /// `len` bytes; `to` is a block of this heap the caller owns, writable for `len` bytes,
    /// whose contents it does not need, and does not overlap `from`; nothing else reads or
    /// writes either range during the call. If `zeroed`, the `len` bytes at `to` read as zero.
    pub unsafe fn move_large(&self, from: NonNull<u8>, to: NonNull<u8>, len: usize, zeroed: bool) {
        // SAFETY: the caller's promise, passed on.
        unsafe { self.move_large_with(from, to, len, zeroed, || {}) }
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
        zeroed: bool,
        mut observe: impl FnMut(),
    ) {
        let piece = move_piece(self.space.plan().geometry());
        let kind = self.lookup(from.as_ptr());
        let slot = match kind {
            Some(Kind::Slot(class)) if !class.is_small() => Some(class),
            Some(Kind::Run(_)) => None,
            _ => self.abort("a move of a pointer that is not a large block of the heap"),
        };
        // Before the copy, so that the copy already faults huge pages.
        // SAFETY: BACK-CLASS-03: `from` is readable for `len` bytes and nothing writes it.
        unsafe { self.grown_into(from, to, len) };
        if len <= piece {
            // SAFETY: BACK-CLASS-03: the caller's promise about the two ranges.
            unsafe { from.as_ptr().copy_to_nonoverlapping(to.as_ptr(), len) };
            return self.free_by_address(from);
        }
        // What `from` may have resident: its whole slot or run. Past the bytes copied that
        // is the rest of the old object and whatever an earlier owner of the slot left.
        // Ranges below are counted from the start of `from`'s slot or run, which `from` is `into`
        // bytes past.
        let (whole, into, host) = match (slot, kind) {
            (Some(class), _) => {
                let Some(host) = self.host(class) else {
                    self.abort("a move in a class that has no regions");
                };
                let (region, index) = self.validate(&host, from.as_ptr(), class);
                let (word, bit) = position(index);
                if tree::word(&host, tree::Node::leaf(region, word)).load() & (1u64 << bit) != 0 {
                    self.abort("a move of a large object that is free");
                }
                let into = self.offset_in_slot(region, class, index, from.as_ptr());
                (class.size(), into, Some((host, region, index)))
            }
            (None, Some(Kind::Run(regions))) => {
                (regions << self.space.plan().region_shift(), 0, None)
            }
            _ => return,
        };
        // Returns the slot's or run's pages in `start..end`, both page boundaries.
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
                    // A run's block starts at the run.
                    let at = from.map_addr(|a| a.saturating_add(start));
                    // SAFETY: BACK-CLASS-03, likewise for a run the caller owns.
                    unsafe { self.space.purge(at, end - start) }.is_ok()
                }
            }
        };
        let page = self.space.plan().geometry().page_size();
        // Whether `to` reads as zero, so that the copy may skip the source's all-zero pages. A
        // purge can fail (`MADV_DONTNEED` on locked memory always does), and then the old
        // bytes stay and every page must be copied.
        let mut zero_to = zeroed;
        if !zeroed {
            // The pages that `to`'s `len` bytes touch. `to` holds more than a piece, so it is a
            // large slot or a run, which are whole pages; a staggered block starts less than a
            // page into its slot, so the first page is the slot's own, and its end fits the
            // slot, so the last page is too.
            let first = to.map_addr(|a| NonZeroUsize::new(a.get() & !(page - 1)).unwrap_or(a));
            let end = (to.addr().get() + len).next_multiple_of(page);
            // SAFETY: BACK-CLASS-03: the caller owns `to`'s slot or run, nothing else uses it,
            // and its contents are not needed.
            zero_to = unsafe { self.space.purge(first, end - first.addr().get()) }.is_ok();
        }
        // Pieces end at multiples of the piece in the address space, not in the block, so that
        // a piece that goes back is whole huge pages wherever the block has them: purging
        // part of a huge page would return nothing.
        let base = from.addr().get();
        let (mut done, mut all) = (0, true);
        while done < len {
            let boundary = (base + done + 1).next_multiple_of(piece) - base;
            let step = (boundary - done).min(len - done);
            let mut at = done;
            while at < done + step {
                // To the next page of the address space, so a skipped run is a source page.
                let n = ((base + at + 1).next_multiple_of(page) - base - at).min(done + step - at);
                // SAFETY: BACK-CLASS-03: `at + n <= len`, inside both ranges, which do not
                // overlap; the bytes of `from` read here have not been returned. A page left
                // uncopied is all zero, and `to` reads as zero there (`zero_to`).
                unsafe {
                    let src = from.as_ptr().add(at);
                    if !zero_to || !is_zero(src, n) {
                        src.copy_to_nonoverlapping(to.as_ptr().add(at), n);
                    }
                }
                at += n;
            }
            // Only pieces that reach a boundary go back here: the last may end inside a page.
            if done + step == boundary {
                all &= give_back((into + done) & !(page - 1), into + boundary);
            }
            done += step;
            observe();
        }
        let last = ((base + len) & !(piece - 1)).saturating_sub(base - into);
        all &= give_back(last, whole);
        match host {
            Some((host, region, index)) => {
                let Some(class) = slot else {
                    return;
                };
                if all {
                    region::mark_purged(&host, region, index);
                }
                region::unadvise(&host, region, index);
                self.release_all(class, &[from.as_ptr()], !all);
                self.heartbeat();
            }
            None => self.free_run(from),
        }
    }

    /// Before the `len` bytes at `from` move into the block at `to`: if `to` is a large slot
    /// whose object grew, and the last piece of those bytes holds data in at least half its
    /// pages, it gets huge pages up to its new size (`region::advise_grown`). Growth alone is
    /// not enough: a buffer grown by doubling and written one stripe a megabyte became wholly
    /// resident with them.
    ///
    /// # Safety
    ///
    /// The `len` bytes at `from` are readable, and nothing writes them during the call.
    unsafe fn grown_into(&self, from: NonNull<u8>, to: NonNull<u8>, len: usize) {
        let Some(Kind::Slot(class)) = self.lookup(to.as_ptr()) else {
            return;
        };
        if class.is_small() || class.size() < self.space.plan().geometry().huge_page_size() {
            return;
        }
        let Some(host) = self.host(class) else {
            return;
        };
        let (region, index) = self.validate(&host, to.as_ptr(), class);
        let into = self.offset_in_slot(region, class, index, to.as_ptr());
        let page = self.space.plan().geometry().page_size();
        let piece = move_piece(self.space.plan().geometry());
        // What the program wrote last: the last piece of what moves.
        let last = len.saturating_sub(piece) & !(page - 1);
        // SAFETY: BACK-CLASS-04, for pages of the `len` readable bytes at `from`.
        let dense = || unsafe { mostly_data(from.as_ptr().add(last), len - last, page) };
        if let Some(now) = region::recorded(&host, region, index) {
            region::advise_grown(&host, region, index, into, len, now.size, dense);
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
                let (slot, _) = host.layout().block_at(location.offset as usize)?;
                Some(region::recorded(&host, location.region, slot)?.size)
            }
            Kind::Run(_) => Some(self.run_size(ptr)),
        }
    }

    /// The bytes from `ptr` to the end of the slot or run it is the start of: what the block
    /// may grow to in place. A staggered block has its stagger less than its slot. `None` if
    /// `ptr` is not the start of a block.
    pub fn room(&self, ptr: *const u8) -> Option<usize> {
        match self.lookup(ptr)? {
            Kind::Slot(class) => {
                let host = self.host(class)?;
                let location = self.space.locate(ptr)?;
                let (_, into) = host.layout().block_at(location.offset as usize)?;
                Some(class.size() - into)
            }
            Kind::Run(regions) => Some(regions << self.space.plan().region_shift()),
        }
    }

    /// Frees one slot of `class` to its bitmap. Aborts as [`Back::launder`] does. A large slot's
    /// recorded size is left as it was; the next allocation of the slot writes it.
    ///
    /// A free of a large slot that would take the large classes' retained count over its cap
    /// returns that slot's pages to the OS first, while the caller still holds the slot, and
    /// then releases it uncounted (`purge`).
    ///
    /// Crate-private, because the purge rests on the caller owning the slot and giving it up
    /// (BACK-CLASS-01): every caller in the crate frees a block its own caller gave up.
    pub(crate) fn free(&self, ptr: NonNull<u8>, class: Class) {
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
    /// run. Aborts if it is neither. This is the C `free` of an uncached object. Crate-private
    /// as [`Back::free`] is.
    pub(crate) fn free_by_address(&self, ptr: NonNull<u8>) {
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
                let host = self.host(class)?;
                let (slot, _) = host.layout().block_at(location.offset as usize)?;
                (slot < host.layout().slots()).then_some(Kind::Slot(class))
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
        let page = self.space.plan().geometry().page_size();
        match self.lookup(ptr)? {
            Kind::Slot(class) if class.is_small() => Some(size),
            // To the end of the page the object ends in, which for a staggered block is not a
            // multiple of a page from its start.
            _ => {
                let into = ptr.addr() & (page - 1);
                Some((into + size).next_multiple_of(page) - into)
            }
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
        let Some((slot, _)) = self.layout.block_at(offset) else {
            self.back
                .abort("free of a pointer that is not the start of a slot");
        };
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

/// Whether at least half the pages of the `len` bytes at `ptr` hold a byte that is not zero.
/// Stops as soon as the answer is known, so a dense block costs a word a page for half of it.
///
/// # Safety
///
/// As for `is_zero`.
unsafe fn mostly_data(ptr: *const u8, len: usize, page: usize) -> bool {
    let pages = len.div_ceil(page);
    let (mut data, mut empty) = (0, 0);
    let mut at = 0;
    while at < len {
        let n = page.min(len - at);
        // SAFETY: BACK-CLASS-04: inside the caller's readable bytes.
        if unsafe { is_zero(ptr.add(at), n) } {
            empty += 1;
            if empty * 2 > pages {
                return false;
            }
        } else {
            data += 1;
            if data * 2 >= pages {
                return true;
            }
        }
        at += n;
    }
    false
}

/// Whether the `len` bytes at `ptr` are all zero, stopping at the first that is not.
///
/// # Safety
///
/// The bytes are readable and nothing writes them during the call.
unsafe fn is_zero(ptr: *const u8, len: usize) -> bool {
    // SAFETY: BACK-CLASS-04: the caller's promise; `u8` has no invalid values.
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    // SAFETY: BACK-CLASS-04: every bit pattern is a valid `u64`, and `align_to` itself puts
    // only aligned, whole words in the middle part.
    let (head, words, tail) = unsafe { bytes.align_to::<u64>() };
    head.iter().all(|&b| b == 0) && words.iter().all(|&w| w == 0) && tail.iter().all(|&b| b == 0)
}
