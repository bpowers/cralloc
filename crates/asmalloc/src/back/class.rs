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

use super::host::ClassHost;
use super::layout::{Grow, Hints};
use super::policy::wants_huge;
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
            let mut bits = taken.bits;
            while bits != 0 {
                let slot = taken.word * BITS + bits.trailing_zeros();
                bits &= bits - 1;
                got = self.put(out, got, taken.region, class.class(), slot);
            }
        }
        self.unretain(class.class(), resident);
        self.revive(class.class(), revived);
        while got < out.len() {
            let want = (out.len() - got).min(u32::MAX as usize) as u32;
            let Some((region, first, count)) = self.carve(&host, class.class(), want) else {
                break;
            };
            for slot in first..first + count {
                got = self.put(out, got, region, class.class(), slot);
            }
        }
        got
    }

    /// Writes slot `slot`'s address at `out[at]` and returns the next position.
    #[inline(always)]
    fn put(
        &self,
        out: &mut [*mut u8],
        at: usize,
        region: RegionId,
        class: Class,
        slot: u32,
    ) -> usize {
        if let Some(entry) = out.get_mut(at) {
            *entry = self.space.slot(region, class, slot).as_ptr();
        }
        at + 1
    }

    /// Returns free slots of `class` to their bitmaps: one atomic OR per bitmap word they touch.
    /// Sorts `objects` in place to group them by word. Aborts the process if a pointer is not
    /// the start of a slot of `class` that has been handed out, or if a slot is already free or
    /// appears twice: those are double frees or frees of foreign pointers.
    pub fn launder(&self, class: SmallClass, objects: &mut [*mut u8]) {
        self.release_all(class.class(), objects);
    }

    /// `launder` for any class.
    fn release_all(&self, class: Class, objects: &mut [*mut u8]) {
        if objects.is_empty() {
            return;
        }
        let Some(host) = self.host(class) else {
            self.abort("free of a pointer into a class that has no regions");
        };
        objects.sort_unstable();
        let mut pending: Option<(RegionId, u32, u64)> = None;
        let mut carved: Option<(RegionId, u32)> = None;
        for &object in objects.iter() {
            let (region, slot) = self.validate(&host, object, class, &mut carved);
            let (word, bit) = position(slot);
            let bit = 1u64 << bit;
            match pending {
                Some((r, w, ref mut bits)) if r == region && w == word => {
                    if *bits & bit != 0 {
                        self.abort("double free: an object was freed twice in one batch");
                    }
                    *bits |= bit;
                }
                _ => {
                    if let Some((r, w, bits)) = pending {
                        tree::release(&host, r, w, bits);
                    }
                    pending = Some((region, word, bit));
                }
            }
        }
        if let Some((r, w, bits)) = pending {
            tree::release(&host, r, w, bits);
        }
        self.retain(class, objects.len().saturating_mul(class.size()));
    }

    /// The region and slot of a pointer the caller says is a handed-out slot of `class`, or an
    /// abort if it is not. `carved` caches one region's watermark across a sorted batch.
    fn validate(
        &self,
        host: &ClassHost<'a, O>,
        object: *const u8,
        class: Class,
        carved: &mut Option<(RegionId, u32)>,
    ) -> (RegionId, u32) {
        let Some(location) = self.space.locate(object) else {
            self.abort("free of a pointer outside the heap");
        };
        if self.state.table.get(location.region) != RegionTag::Class(class) {
            self.abort("free of a pointer with the wrong class");
        }
        let Some(slot) = self.space.plan().slot_at(location, class) else {
            self.abort("free of a pointer that is not the start of a slot");
        };
        let limit = match *carved {
            Some((region, limit)) if region == location.region => limit,
            _ => {
                let grow = host.meta(location.region, host.layout().grow_word());
                let limit = Grow::unpack(grow.load()).carved;
                *carved = Some((location.region, limit));
                limit
            }
        };
        if slot >= limit {
            self.abort("free of a slot that was never handed out");
        }
        (location.region, slot)
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
                let (region, slot) = self.validate(&host, ptr.as_ptr(), class, &mut None);
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
    /// A free of a large slot that leaves the large classes' retained count over its cap
    /// returns that slot's pages to the OS before it returns (`purge`).
    pub fn free(&self, ptr: NonNull<u8>, class: Class) {
        self.release_all(class, &mut [ptr.as_ptr()]);
        if class.is_small() {
            return;
        }
        if self.large_over_cap()
            && let Some(location) = self.space.locate(ptr.as_ptr())
            && let Some(slot) = self.space.plan().slot_at(location, class)
        {
            self.purge_slot(class, location.region, slot);
        }
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
