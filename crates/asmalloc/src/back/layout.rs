//! Where a region's metadata words are for one class, and the arithmetic between slots, bytes,
//! pages and purge units.
//!
//! A region's metadata block (`Space::metadata`) is an array of 64-bit words, all zero until
//! touched. A small class's block:
//!
//! ```text
//! word 0                      summary levels height down to 1, each a word per 64 words below
//! ...                         clean bitmap: one bit per page, set means purged and untouched
//! next page                   leaf bitmap: one bit per slot, set means free
//! ```
//!
//! Everything before the leaf bitmap is committed when the region is assigned; the leaf bitmap,
//! the one part that grows with the region, is committed a page at a time as the region grows.
//! All zero means nothing is free and no purge is under way.
//!
//! The order keeps what a class touches together, because each touched page of a block costs a
//! page of memory and, at 128 MiB from the next class's block, often a page of page tables too:
//! a region whose slots reach the bitmaps touches the first page of the block (the small top
//! levels, and for most classes the start of level 1), the start of level 1 for the smallest
//! classes, and the first page of the leaf bitmap, which for every small class lies in the same
//! 2 MiB as the rest, so one page of page tables serves them all (`docs/research/memory.md`,
//! "Small heaps"). A small class's header (`Grow`, `Hints`) is not in the block at all: it is
//! in the back end's state, beside every other region's (`host::RegionHeader`), so that a small
//! class whose slots never reach the bitmaps never touches its block.
//!
//! A large class touches its block on every allocation and free, so its block keeps everything
//! those read together, and is committed whole at assignment (its leaf bitmap is small):
//!
//! ```text
//! word 0                      header: Grow (watermark, committed pages), then Hints (start of
//!                             the huge-page part, purge cursor), on a line of its own
//! ...                         summary levels height down to 1
//! ...                         leaf bitmap
//! ...                         zero bitmap: one bit per slot, set means free and known to read
//!                             as zero
//! ...                         clean bitmap
//! ...                         sizes: one word per slot, the size the object in it was last
//!                             allocated or resized to, and whether the slot is advised for huge
//!                             pages (`Recorded`)
//! ```
//!
//! With the header in the back end's state and the leaf bitmap after the sizes, each large
//! allocation and free touched two more lines than this, 1.5% on a pipeline of large objects.

use super::policy::{STAGGER_SLOTS, STAGGER_STEP};
use super::word::BITS;
use crate::classes::{Class, Divisor};
use crate::space::Plan;
use core::sync::atomic::{AtomicU64, Ordering};

/// The most levels a region's bit tree has, leaf included: a 4 GiB region of 8-byte slots has
/// 2^23 leaf words, then 2^17, 2^11, 2^5 and 1.
pub(crate) const MAX_LEVELS: usize = 5;

/// Bytes per metadata word.
pub(crate) const WORD_BYTES: usize = 8;

/// A large class's header words at the start of its block: `Grow`, then `Hints`.
const HEADER_WORDS: u32 = 2;

/// Words in a cache line.
const LINE_WORDS: u32 = (crate::lined::LINE / WORD_BYTES) as u32;

/// The metadata layout and slot arithmetic of one class in one geometry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Layout {
    size: usize,
    divisor: Divisor,
    /// Where slot 0 lies in the region (`Plan::slot_origin`): whole pages, zero for a large
    /// class. The pages below it hold no slot and are never committed.
    origin: usize,
    slots: u32,
    /// The top level of the region's tree: 0 if the leaf bitmap is a single word.
    height: u8,
    /// Word offset of each level of the tree; level 0 is the leaf bitmap, last in the block.
    offset: [u32; MAX_LEVELS],
    /// Words in each level.
    count: [u32; MAX_LEVELS],
    pages: u32,
    clean: u32,
    /// Word offset of the zero bitmap, or 0 if the class has none (small classes).
    zero: u32,
    /// Word offset of the per-slot sizes, or 0 if the class has none (small classes).
    sizes: u32,
    page_shift: u8,
    huge_shift: u8,
}

/// Words a [`Layout`] packs into for [`Memo`]: everything but the size and its divisor, which
/// come from the class, as seventeen 32-bit fields (the total derivable and not read back) and
/// three bytes.
const PACKED_WORDS: usize = 9;

/// Set in the last packed word: the words hold a layout.
const PACKED: u64 = 1 << 63;

/// One class's layout, worked out once for a heap and kept, since a launder or a fill needs it
/// on every call and working it out costs more than the call's work on a word of slots.
///
/// All zero means nothing kept. The layout depends only on the class and the reservation's
/// plan, which never changes for the heap, so every thread that works it out writes the same
/// words: concurrent writers agree, and a reader that sees the last word (written last, with
/// release ordering, and holding [`PACKED`]) sees the rest. A child forked at any point sees
/// either the whole layout or no mark, and works it out again.
pub(crate) struct Memo {
    words: [AtomicU64; PACKED_WORDS],
}

impl Memo {
    pub(crate) const fn new() -> Memo {
        Memo {
            words: [const { AtomicU64::new(0) }; PACKED_WORDS],
        }
    }

    /// The layout of `class` in `plan`, from the memo if it holds one, else worked out and
    /// kept. `plan` must be the same for every call on one memo.
    #[inline]
    pub(crate) fn layout(&self, plan: Plan, class: Class) -> Option<Layout> {
        let mut last = self.words[PACKED_WORDS - 1].load(Ordering::Acquire);
        if last & PACKED == 0 {
            last = self.work_out(plan, class)?;
        }
        let mut words = [0; PACKED_WORDS];
        for (word, kept) in words.iter_mut().zip(&self.words) {
            *word = kept.load(Ordering::Relaxed);
        }
        words[PACKED_WORDS - 1] = last;
        Some(Layout::unpack(words, class))
    }

    /// Works the layout out and keeps it, once per class per heap, and returns the last packed
    /// word. Out of line and returning no layout of its own, so that the common path is small
    /// and makes its layout in one place: when the layout came back from here too, the caller
    /// built the class's host twice over and copied it (`memmove`, then 20 stores and loads),
    /// 4% on a pipeline of large objects (`docs/research/memory.md`, "Small heaps").
    #[cold]
    #[inline(never)]
    fn work_out(&self, plan: Plan, class: Class) -> Option<u64> {
        let layout = Layout::new(plan, class)?;
        let words = layout.pack();
        for (kept, &word) in self.words.iter().zip(&words).take(PACKED_WORDS - 1) {
            kept.store(word, Ordering::Relaxed);
        }
        self.words[PACKED_WORDS - 1].store(words[PACKED_WORDS - 1], Ordering::Release);
        Some(words[PACKED_WORDS - 1])
    }
}

/// Where a region is in its growth: how many slots are carved (the watermark) and how many pages
/// from the region's base are committed. One word, so a carve checks the commit and advances the
/// watermark with one compare-and-swap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Grow {
    /// Slots carved: those below have been handed out at least once.
    pub(crate) carved: u32,
    /// Pages from the region's base that are writable.
    pub(crate) committed: u32,
}

impl Grow {
    #[inline(always)]
    pub(crate) const fn unpack(word: u64) -> Grow {
        Grow {
            carved: word as u32,
            committed: (word >> 32) as u32,
        }
    }

    #[inline(always)]
    pub(crate) const fn pack(self) -> u64 {
        self.carved as u64 | (self.committed as u64) << 32
    }
}

/// Hints a region keeps for the huge-page policy and for purging. One word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Hints {
    /// The first page of the huge-page part, which runs to the end of the region, if the region
    /// has one. Stored as the page plus one, so zero means none.
    pub(crate) huge_from: Option<u32>,
    /// The page at which the next top-down purge step ends (exclusive); zero means start again
    /// at the top.
    pub(crate) cursor: u32,
}

impl Hints {
    #[inline(always)]
    pub(crate) const fn unpack(word: u64) -> Hints {
        let huge = word as u32;
        Hints {
            huge_from: if huge == 0 { None } else { Some(huge - 1) },
            cursor: (word >> 32) as u32,
        }
    }

    #[inline(always)]
    pub(crate) const fn pack(self) -> u64 {
        let huge = match self.huge_from {
            Some(page) => page + 1,
            None => 0,
        };
        huge as u64 | (self.cursor as u64) << 32
    }
}

/// What a large slot's size word holds: the size of the object in it, and whether the slot's
/// whole huge pages are advised for huge pages because the object grew there. One word,
/// written only by whoever holds the slot. A size is at most a region, 4 GiB, so the flag
/// takes the top bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Recorded {
    pub(crate) size: usize,
    pub(crate) huge: bool,
}

impl Recorded {
    const HUGE: u64 = 1 << 63;

    #[inline(always)]
    pub(crate) const fn unpack(word: u64) -> Recorded {
        Recorded {
            size: (word & !Recorded::HUGE) as usize,
            huge: word & Recorded::HUGE != 0,
        }
    }

    #[inline(always)]
    pub(crate) const fn pack(self) -> u64 {
        let flag = if self.huge { Recorded::HUGE } else { 0 };
        self.size as u64 | flag
    }
}

/// A run of pages a purge returns as one call: `start..end`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Unit {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

/// What a top-down purge pass over a region does next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    /// The pass has reached the bottom of the region.
    Done,
    /// Nothing to look at below the cursor until this page; continue from there. Zero means the
    /// pass is done.
    Skip(u32),
    /// Look at this unit; the pass continues below its start.
    Unit(Unit),
}

impl Layout {
    /// The layout of `class` in a reservation laid out as `plan`, or `None` if a slot does not
    /// fit in a region or the metadata does not fit in the region's block.
    pub(crate) const fn new(plan: Plan, class: Class) -> Option<Layout> {
        // The leaf bitmap starts a page, so that its commits are its own pages, if the block has
        // room for the padding (every geometry but the smallest simulated ones).
        match Layout::with_leaf_aligned(plan, class, true) {
            Some(layout) => Some(layout),
            None => Layout::with_leaf_aligned(plan, class, false),
        }
    }

    /// The layout, with the leaf bitmap at the next page boundary after the fixed part if
    /// `aligned`, else straight after it.
    const fn with_leaf_aligned(plan: Plan, class: Class, aligned: bool) -> Option<Layout> {
        let geometry = plan.geometry();
        let origin = plan.slot_origin(class);
        let slots = slots_in_region(plan, class);
        if slots == 0 {
            return None;
        }
        let mut offset = [0u32; MAX_LEVELS];
        let mut count = [0u32; MAX_LEVELS];
        count[0] = slots.div_ceil(BITS);
        let mut height = 0;
        while count[height] > 1 {
            height += 1;
            if height >= MAX_LEVELS {
                return None;
            }
            count[height] = count[height - 1].div_ceil(BITS);
        }
        // A large class's header first, on a line of its own where the block has room
        // (`header_word`); then the summary levels from the top down, the small ones sharing the
        // block's first page.
        let mut next = match (class.is_small(), aligned) {
            (true, _) => 0,
            (false, true) => LINE_WORDS,
            (false, false) => HEADER_WORDS,
        };
        let mut level = height;
        while level >= 1 {
            offset[level] = next;
            next += count[level];
            level -= 1;
        }
        let page_shift = geometry.page_size().trailing_zeros();
        let pages = (plan.region_size() >> page_shift) as u32;
        let clean_count = pages.div_ceil(BITS);
        let (clean, zero, sizes) = if class.is_small() {
            // The clean bitmap, then the leaf bitmap from a page boundary, last, so that it can
            // be committed as the region grows.
            let clean = next;
            next += clean_count;
            if aligned {
                let page_words = (geometry.page_size() / WORD_BYTES) as u32;
                next = next.next_multiple_of(page_words);
            }
            offset[0] = next;
            next += count[0];
            (clean, 0, 0)
        } else {
            // What every allocation and free reads, together after the summary: the leaf and
            // zero bitmaps; then the clean bitmap and the sizes. The block is committed whole at
            // assignment: a large class's leaf bitmap is small.
            offset[0] = next;
            let zero = offset[0] + count[0];
            let clean = zero + count[0];
            let sizes = clean + clean_count;
            next = sizes + slots;
            (clean, zero, sizes)
        };
        if next as usize * WORD_BYTES > plan.metadata_size() {
            return None;
        }
        let level = height;
        Some(Layout {
            size: class.size(),
            divisor: class.divisor(),
            origin,
            slots,
            height: level as u8,
            offset,
            count,
            pages,
            clean,
            zero,
            sizes,
            page_shift: page_shift as u8,
            huge_shift: geometry.huge_page_size().trailing_zeros() as u8,
        })
    }

    /// The layout as [`PACKED_WORDS`] words, the last marked [`PACKED`].
    fn pack(&self) -> [u64; PACKED_WORDS] {
        let f = [
            self.slots,
            self.offset[0],
            self.offset[1],
            self.offset[2],
            self.offset[3],
            self.offset[4],
            self.count[0],
            self.count[1],
            self.count[2],
            self.count[3],
            self.count[4],
            self.pages,
            self.clean,
            self.origin as u32,
            self.zero,
            self.sizes,
            self.total_words(),
        ];
        let pair = |a: u32, b: u32| a as u64 | (b as u64) << 32;
        [
            pair(f[0], f[1]),
            pair(f[2], f[3]),
            pair(f[4], f[5]),
            pair(f[6], f[7]),
            pair(f[8], f[9]),
            pair(f[10], f[11]),
            pair(f[12], f[13]),
            pair(f[14], f[15]),
            f[16] as u64
                | (self.height as u64) << 32
                | (self.page_shift as u64) << 40
                | (self.huge_shift as u64) << 48
                | PACKED,
        ]
    }

    /// The layout `pack` made for `class`.
    #[inline(always)]
    fn unpack(w: [u64; PACKED_WORDS], class: Class) -> Layout {
        let lo = |i: usize| w[i] as u32;
        let hi = |i: usize| (w[i] >> 32) as u32;
        Layout {
            size: class.size(),
            divisor: class.divisor(),
            origin: hi(6) as usize,
            slots: lo(0),
            height: (w[8] >> 32) as u8,
            offset: [hi(0), lo(1), hi(1), lo(2), hi(2)],
            count: [lo(3), hi(3), lo(4), hi(4), lo(5)],
            pages: hi(5),
            clean: lo(6),
            zero: lo(7),
            sizes: hi(7),
            page_shift: (w[8] >> 40) as u8,
            huge_shift: (w[8] >> 48) as u8,
        }
    }

    /// The slot size in bytes.
    #[inline(always)]
    pub(crate) const fn size(&self) -> usize {
        self.size
    }

    /// Slots in a region.
    #[inline(always)]
    pub(crate) const fn slots(&self) -> u32 {
        self.slots
    }

    /// The top level of the region's tree.
    #[inline(always)]
    pub(crate) const fn height(&self) -> u8 {
        self.height
    }

    /// Word offset of `index` in tree level `level`. Level 0 is the leaf bitmap.
    #[inline(always)]
    pub(crate) const fn tree_word(&self, level: u8, index: u32) -> u32 {
        self.offset[level as usize % MAX_LEVELS] + index
    }

    /// Words in tree level `level`.
    #[inline(always)]
    pub(crate) const fn level_words(&self, level: u8) -> u32 {
        self.count[level as usize % MAX_LEVELS]
    }

    /// Words in the leaf bitmap.
    #[cfg(any(test, kani))]
    #[inline(always)]
    pub(crate) const fn leaf_words(&self) -> u32 {
        self.count[0]
    }

    /// Word offset of clean-bitmap word `index`.
    #[inline(always)]
    pub(crate) const fn clean_word(&self, index: u32) -> u32 {
        self.clean + index
    }

    /// Words in the clean bitmap.
    #[cfg(any(test, kani))]
    #[inline(always)]
    pub(crate) const fn clean_words(&self) -> u32 {
        self.pages.div_ceil(BITS)
    }

    /// Word offset of the region's `Grow` word in its block, `Hints` the word after it, for a
    /// large class; `None` for a small class, whose header is in the back end's state
    /// (`host::RegionHeader`). Every large allocation and free touches its block anyway, and
    /// with the header outside it touched one more line.
    #[inline(always)]
    pub(crate) const fn header_word(&self) -> Option<u32> {
        if self.sizes == 0 { None } else { Some(0) }
    }

    /// Word offset of the size word of slot `slot`, if the class records sizes (large classes).
    #[inline(always)]
    pub(crate) const fn size_word(&self, slot: u32) -> Option<u32> {
        if self.sizes == 0 {
            None
        } else {
            Some(self.sizes + slot)
        }
    }

    /// Word offset of zero-bitmap word `index`, if the class has a zero bitmap.
    #[inline(always)]
    pub(crate) const fn zero_word(&self, index: u32) -> Option<u32> {
        if self.zero == 0 {
            None
        } else {
            Some(self.zero + index)
        }
    }

    /// Words of metadata the layout uses.
    #[inline(always)]
    pub(crate) const fn total_words(&self) -> u32 {
        if self.sizes == 0 {
            self.offset[0] + self.count[0]
        } else {
            self.sizes + self.slots
        }
    }

    /// Bytes per page.
    #[inline(always)]
    pub(crate) const fn page_size(&self) -> usize {
        1 << self.page_shift
    }

    /// `log2` of the page size.
    #[inline(always)]
    pub(crate) const fn page_shift(&self) -> u32 {
        self.page_shift as u32
    }

    /// Pages per huge page.
    #[inline(always)]
    pub(crate) const fn huge_pages(&self) -> u32 {
        1 << (self.huge_shift - self.page_shift)
    }

    /// Pages in a region.
    #[inline(always)]
    pub(crate) const fn region_pages(&self) -> u32 {
        self.pages
    }

    /// The offset of slot `index` from the region base, `origin + index * size`. For
    /// `index <= slots` this is at most the region size.
    #[inline(always)]
    pub(crate) const fn slot_bytes(&self, index: u32) -> usize {
        self.origin + self.bytes_of(index)
    }

    /// The bytes of `count` slots.
    #[inline(always)]
    pub(crate) const fn bytes_of(&self, count: u32) -> usize {
        count as usize * self.size
    }

    /// The page holding slot 0: no purge or commit goes below it.
    #[inline(always)]
    pub(crate) const fn origin_page(&self) -> u32 {
        (self.origin >> self.page_shift) as u32
    }

    /// The slot holding byte `offset` of the region, or slot 0 for a byte below the origin.
    /// `offset` must be below the region size, which is at most 4 GiB.
    #[inline(always)]
    pub(crate) const fn slot_at(&self, offset: usize) -> u32 {
        self.divisor
            .divide(offset.saturating_sub(self.origin) as u32)
    }

    /// Pages lying wholly below the watermark: a purge looks only at these, so it never touches
    /// a slot a carve could hand out.
    #[inline(always)]
    pub(crate) const fn carved_pages(&self, carved: u32) -> u32 {
        (self.slot_bytes(carved) >> self.page_shift) as u32
    }

    /// The first huge-page boundary at or above page `page`, in pages.
    #[inline(always)]
    pub(crate) const fn huge_ceil(&self, page: u32) -> u32 {
        let mask = self.huge_pages() - 1;
        (page + mask) & !mask
    }

    /// Where in the block the leaf bitmap must be writable up to, for a region committed up to
    /// page `committed`: past the words covering every slot that starts below that page,
    /// rounded up to a whole page. With nothing committed, the page the leaf bitmap starts in,
    /// so that the range from one commit to the next is always whole pages of the bitmap. For a
    /// large class, whose block is committed whole at assignment, the end of the block, so that
    /// no carve commits anything more.
    #[inline]
    pub(crate) const fn leaf_commit(&self, committed: u32) -> usize {
        if self.sizes != 0 {
            return self.block_bytes();
        }
        let bytes = (committed as usize) << self.page_shift;
        let covered = if bytes <= self.origin {
            0
        } else {
            let last = self.slot_at(bytes - 1) + 1;
            if last < self.slots { last } else { self.slots }
        };
        let start = self.offset[0] as usize * WORD_BYTES;
        let page = self.page_size();
        if covered == 0 {
            return start & !(page - 1);
        }
        let end = start + covered.div_ceil(BITS) as usize * WORD_BYTES;
        (end + page - 1) & !(page - 1)
    }

    /// The byte range of the metadata block that is committed when the region is assigned, as
    /// whole pages: for a small class everything before the leaf bitmap, for a large class the
    /// whole block.
    #[inline]
    pub(crate) const fn fixed_commit(&self) -> (usize, usize) {
        if self.sizes != 0 {
            return (0, self.block_bytes());
        }
        let page = self.page_size();
        let end = (self.offset[0] as usize * WORD_BYTES + page - 1) & !(page - 1);
        (0, end)
    }

    /// Bytes of the block the layout uses, as whole pages: the most of it a region can touch.
    #[inline]
    pub(crate) const fn block_bytes(&self) -> usize {
        let page = self.page_size();
        (self.total_words() as usize * WORD_BYTES + page - 1) & !(page - 1)
    }

    /// The next thing a top-down purge pass looks at below page `end` (zero for the top), in a
    /// region with `carved` slots and a huge-page part from `huge_from`.
    ///
    /// Only pages wholly below the watermark and at or above the origin count. In the
    /// huge-page part a unit is whole huge pages, but for one that the origin cuts; in the
    /// rest, whole pages. A unit ends at a unit boundary and starts at the
    /// boundary at or below the start of the slot holding its last byte, so a unit of slots of
    /// a page or more is exactly the pages of one slot when the slot is aligned to the unit,
    /// and the slots it touches are always consecutive.
    pub(crate) const fn unit_below(&self, carved: u32, huge_from: Option<u32>, end: u32) -> Step {
        let top = self.carved_pages(carved);
        let end = if end == 0 || end > top { top } else { end };
        if end == 0 {
            return Step::Done;
        }
        let (floor, end, granule) = match huge_from {
            Some(huge) if end > huge => {
                let granule = self.huge_pages();
                let aligned = end & !(granule - 1);
                if aligned <= huge {
                    // Part of a huge page below the cursor is not carved yet; nothing in the
                    // huge-page part can go, and the small part starts at `huge`.
                    return Step::Skip(huge);
                }
                (huge, aligned, granule)
            }
            _ => (0, end, 1),
        };
        // Nothing below the origin holds a slot or was ever committed.
        let floor = if floor < self.origin_page() {
            self.origin_page()
        } else {
            floor
        };
        if end <= floor {
            return Step::Done;
        }
        let last = self.slot_at(((end as usize) << self.page_shift) - 1);
        let first_page = (self.slot_bytes(last) >> self.page_shift) as u32;
        let start = first_page & !(granule - 1);
        let start = if start < floor { floor } else { start };
        Step::Unit(Unit { start, end })
    }

    /// The pages wholly inside slot `slot`, as a unit, if there are any. A slot of whole pages
    /// (every large class) is all of them. The only slot such a unit touches is `slot`, so a
    /// purge of it claims that slot alone.
    #[inline]
    pub(crate) const fn slot_unit(&self, slot: u32) -> Option<Unit> {
        let page = self.page_size();
        let start = self.slot_bytes(slot).div_ceil(page) as u32;
        let end = (self.slot_bytes(slot + 1) >> self.page_shift) as u32;
        if start < end {
            Some(Unit { start, end })
        } else {
            None
        }
    }

    /// How far past its slot's start a block in slot `slot` starts when it is staggered: a
    /// multiple of `policy::STAGGER_STEP` that depends only on the slot's index, so that it is
    /// never stored and a free recomputes it. Zero for small classes, which are never
    /// staggered. A block of a large class starts either here or at the slot's start; which
    /// one the address says.
    #[inline(always)]
    pub(crate) const fn stagger(&self, slot: u32) -> usize {
        if self.sizes == 0 {
            0
        } else {
            (slot as usize % STAGGER_SLOTS) * STAGGER_STEP
        }
    }

    /// The slot a block starting at byte `offset` of the region is in, and how far into the
    /// slot it starts, if `offset` is a block's start: the slot's start, or its stagger. The
    /// one place a pointer becomes a slot. The slot may be past the region's last one; callers
    /// check it against the watermark or the slot count.
    #[inline(always)]
    pub(crate) const fn block_at(&self, offset: usize) -> Option<(u32, usize)> {
        if offset < self.origin {
            return None;
        }
        let slot = self.slot_at(offset);
        let into = offset - self.slot_bytes(slot);
        if into == 0 || into == self.stagger(slot) {
            Some((slot, into))
        } else {
            None
        }
    }

    /// The whole huge pages inside slot `slot`, as a unit, if there are any: the part of a
    /// slot that huge-page advice may cover without making a huge page straddle into the
    /// slots beside it.
    #[inline]
    pub(crate) const fn huge_interior(&self, slot: u32) -> Option<Unit> {
        let page = self.page_size();
        let start = self.huge_ceil(self.slot_bytes(slot).div_ceil(page) as u32);
        let end =
            ((self.slot_bytes(slot + 1) >> self.page_shift) as u32) & !(self.huge_pages() - 1);
        if start < end {
            Some(Unit { start, end })
        } else {
            None
        }
    }

    /// The slots a unit touches: `first..=last`.
    #[inline]
    pub(crate) const fn touching(&self, unit: Unit) -> (u32, u32) {
        let first = self.slot_at((unit.start as usize) << self.page_shift);
        let last = self.slot_at(((unit.end as usize) << self.page_shift) - 1);
        (first, last)
    }

    /// The slots wholly inside a unit: `first..end`, empty if `first >= end`.
    #[inline]
    pub(crate) const fn inside(&self, unit: Unit) -> (u32, u32) {
        let (first, last) = self.touching(unit);
        let start = (unit.start as usize) << self.page_shift;
        let end = (unit.end as usize) << self.page_shift;
        let lo = if self.slot_bytes(first) < start {
            first + 1
        } else {
            first
        };
        let hi = if self.slot_bytes(last + 1) <= end {
            last + 1
        } else {
            last
        };
        (lo, hi)
    }
}

/// The number of slots of `class` in a region of `plan`: `(region_size - origin) / size`, zero
/// if a slot does not fit. Without a divide instruction, because the back end computes layouts
/// at run time from the reservation's plan: `Divisor` gives `m = (room - 1) / size`, and one
/// more slot fits exactly when the size divides the room.
pub(crate) const fn slots_in_region(plan: Plan, class: Class) -> u32 {
    let region = plan.region_size() - plan.slot_origin(class);
    let size = class.size();
    if size > region {
        return 0;
    }
    let below = class.divisor().divide((region - 1) as u32);
    if (below as usize + 1) * size == region {
        below + 1
    } else {
        below
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::MagazineSize;
    use crate::classes::{MAX_SMALL_SIZE, SmallClass};
    use crate::os::sim::{Native, Shape, Standard, Tiny};
    use crate::space::{Geometry, Plan};

    /// The plans the tests cover: the largest and smallest of the design, and the simulator's.
    fn plans() -> [Plan; 5] {
        [
            Geometry::DESIGN.largest_plan(),
            Geometry::DESIGN.smallest_plan(),
            Standard::GEOMETRY.largest_plan(),
            Native::GEOMETRY.largest_plan(),
            Tiny::GEOMETRY.largest_plan(),
        ]
    }

    #[test]
    fn every_class_that_fits_has_a_layout_in_the_used_geometries() {
        for plan in plans() {
            for class in Class::all() {
                let layout = Layout::new(plan, class);
                assert_eq!(layout.is_some(), plan.fits_region(class), "{class:?}");
                let Some(layout) = layout else { continue };
                assert!(layout.total_words() as usize * WORD_BYTES <= plan.metadata_size());
                assert_eq!(layout.level_words(layout.height()), 1);
                assert_eq!(layout.zero_word(0).is_some(), !class.is_small());
            }
        }
    }

    #[test]
    fn slot_counts_match_division() {
        for plan in plans() {
            for class in Class::all() {
                assert_eq!(
                    slots_in_region(plan, class),
                    plan.slots_per_region(class),
                    "{class:?}"
                );
            }
        }
    }

    #[test]
    fn the_design_tree_has_five_levels_for_eight_byte_slots() {
        let plan = Geometry::DESIGN.largest_plan();
        let layout = Layout::new(plan, Class::SMALLEST).unwrap();
        let origin = plan.slot_origin(Class::SMALLEST);
        assert_eq!(layout.slot_bytes(0), origin);
        assert_eq!(layout.slots() as usize, ((1 << 32) - origin) / 8);
        assert_eq!(layout.height(), 4);
        let leaf = layout.slots().div_ceil(64);
        assert_eq!(
            [0, 1, 2, 3, 4].map(|l| layout.level_words(l)),
            [leaf, leaf.div_ceil(64), 1 << 11, 1 << 5, 1]
        );
        assert_eq!(layout.clean_words(), (1 << 20) / 64);
    }

    /// In the design's geometry, at every region size, the first word of every level of a small
    /// class's block, which is what a region whose slots reach the bitmaps touches first, lies
    /// in the block's first 2 MiB, so that one page of page tables maps them all; the leaf
    /// bitmap starts a page; and the top levels share the block's first page.
    #[test]
    fn what_a_small_class_touches_first_is_in_one_huge_page() {
        let huge = Geometry::DESIGN.huge_page_size();
        let page = Geometry::DESIGN.page_size();
        for plan in [
            Geometry::DESIGN.largest_plan(),
            Geometry::DESIGN.smallest_plan(),
        ] {
            for class in Class::all().filter(|c| c.is_small()) {
                let layout = Layout::new(plan, class).unwrap();
                let leaf = layout.tree_word(0, 0) as usize * WORD_BYTES;
                assert_eq!(leaf % page, 0, "{class:?}");
                assert!(leaf < huge, "{class:?}");
                if layout.height() > 0 {
                    assert_eq!(layout.tree_word(layout.height(), 0), 0);
                }
                for level in 2..=layout.height() {
                    assert!(
                        (layout.tree_word(level, 0) as usize * WORD_BYTES) < page,
                        "{class:?}"
                    );
                }
                assert_eq!(layout.fixed_commit(), (0, leaf));
            }
        }
    }

    #[test]
    fn the_tiny_geometry_fits_eight_byte_slots_with_the_leaf_bitmap_last() {
        let layout = Layout::new(Tiny::GEOMETRY.largest_plan(), Class::SMALLEST).unwrap();
        assert!(layout.total_words() as usize * WORD_BYTES <= 64);
        assert_eq!(
            layout.tree_word(0, 0) + layout.leaf_words(),
            layout.total_words()
        );
    }

    #[test]
    fn every_small_geometry_fits_every_class_it_hosts() {
        for page in 6..=12 {
            for region in page + crate::space::METADATA_SHIFT..=20 {
                for huge in page..=region {
                    let Some(g) =
                        Geometry::try_new(page, huge, region, region, 64, 1, MagazineSize::MIN)
                    else {
                        continue;
                    };
                    let plan = g.largest_plan();
                    for class in Class::all().filter(|&c| plan.fits_region(c)) {
                        assert!(Layout::new(plan, class).is_some(), "{g:?} {class:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn layouts_pack_and_come_back_from_the_memo() {
        for plan in plans() {
            for class in Class::all() {
                let Some(layout) = Layout::new(plan, class) else {
                    continue;
                };
                assert_eq!(Layout::unpack(layout.pack(), class), layout, "{class:?}");
                let memo = Memo::new();
                assert_eq!(memo.layout(plan, class), Some(layout));
                assert_eq!(memo.layout(plan, class), Some(layout), "kept");
            }
        }
    }

    #[test]
    fn headers_round_trip() {
        let grow = Grow {
            carved: 12345,
            committed: 1 << 26,
        };
        assert_eq!(Grow::unpack(grow.pack()), grow);
        assert_eq!(
            Grow::unpack(0),
            Grow {
                carved: 0,
                committed: 0
            }
        );
        for huge_from in [None, Some(0), Some(77)] {
            let hints = Hints {
                huge_from,
                cursor: 9,
            };
            assert_eq!(Hints::unpack(hints.pack()), hints);
        }
        assert_eq!(
            Hints::unpack(0),
            Hints {
                huge_from: None,
                cursor: 0
            }
        );
        for huge in [false, true] {
            let recorded = Recorded {
                size: 1 << 32,
                huge,
            };
            assert_eq!(Recorded::unpack(recorded.pack()), recorded);
        }
        assert_eq!(Recorded::unpack(4096).size, 4096);
    }

    #[test]
    fn a_slots_huge_interior_is_its_whole_huge_pages() {
        let plan = Geometry::DESIGN.largest_plan();
        let huge = Geometry::DESIGN.huge_page_size();
        let page = Geometry::DESIGN.page_size();
        for class in Class::all().filter(|c| !c.is_small() && plan.fits_region(*c)) {
            let layout = Layout::new(plan, class).unwrap();
            for slot in [0, 1, 2, 3, layout.slots() - 1] {
                let (start, end) = (layout.slot_bytes(slot), layout.slot_bytes(slot + 1));
                match layout.huge_interior(slot) {
                    Some(unit) => {
                        let (a, b) = (unit.start as usize * page, unit.end as usize * page);
                        assert!(a >= start && b <= end && a % huge == 0 && b % huge == 0);
                        assert!(a - start < huge && end - b < huge, "{class:?} {slot}");
                    }
                    None => assert!(start.next_multiple_of(huge) + huge > end, "{class:?}"),
                }
            }
        }
    }

    /// Walks every top-down pass over a region of the standard geometry and checks that the
    /// units tile the carved pages from the top, stay in their part, and touch exactly the slots
    /// `touching` names.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn purge_passes_tile_the_carved_pages() {
        for g in [
            Standard::GEOMETRY.largest_plan(),
            Standard::GEOMETRY.smallest_plan(),
            Tiny::GEOMETRY.largest_plan(),
            Native::GEOMETRY.largest_plan(),
        ] {
            for class in Class::all().filter(|&c| g.fits_region(c)) {
                let layout = Layout::new(g, class).unwrap();
                let slots = layout.slots();
                for carved in [0, 1, 2, 3, slots / 3, slots / 2 + 1, slots - 1, slots] {
                    let huge = layout.huge_pages();
                    for huge_from in [None, Some(0), Some(huge), Some(layout.huge_ceil(5))] {
                        let huge_from = huge_from.filter(|&h| h < layout.region_pages());
                        check_pass(&layout, carved, huge_from);
                    }
                }
            }
        }
        let small = SmallClass::all().last().unwrap();
        assert_eq!(small.size(), MAX_SMALL_SIZE);
    }

    fn check_pass(layout: &Layout, carved: u32, huge_from: Option<u32>) {
        let top = layout.carved_pages(carved);
        let mut cursor = 0;
        let mut covered_to = top;
        loop {
            match layout.unit_below(carved, huge_from, cursor) {
                Step::Done => break,
                Step::Skip(next) => {
                    assert!(next < covered_to || (next == covered_to && cursor == 0));
                    covered_to = next;
                    if next == 0 {
                        break;
                    }
                    cursor = next;
                }
                Step::Unit(unit) => {
                    assert!(unit.start < unit.end && unit.end <= covered_to, "{unit:?}");
                    if let Some(h) = huge_from.filter(|&h| unit.end > h) {
                        assert!(unit.start >= h);
                        assert_eq!(unit.end % layout.huge_pages(), 0);
                        assert!(
                            unit.start == h
                                || unit.start == layout.origin_page()
                                || unit.start % layout.huge_pages() == 0
                        );
                    }
                    let (first, last) = layout.touching(unit);
                    assert!(last < carved);
                    let page = layout.page_size();
                    let (start, end) = (unit.start as usize * page, unit.end as usize * page);
                    assert!(layout.slot_bytes(first) <= start);
                    assert!(layout.slot_bytes(first + 1) > start);
                    assert!(layout.slot_bytes(last) < end);
                    assert!(layout.slot_bytes(last + 1) >= end);
                    let (lo, hi) = layout.inside(unit);
                    for slot in first..=last {
                        let wholly =
                            layout.slot_bytes(slot) >= start && layout.slot_bytes(slot + 1) <= end;
                        assert_eq!(wholly, slot >= lo && slot < hi);
                    }
                    assert!(unit.start >= layout.origin_page());
                    covered_to = unit.start;
                    if unit.start == 0 {
                        break;
                    }
                    cursor = unit.start;
                }
            }
        }
    }

    #[test]
    fn leaf_commits_cover_every_slot_below_the_commit() {
        let g = Standard::GEOMETRY.largest_plan();
        let layout = Layout::new(g, Class::SMALLEST).unwrap();
        let page = Standard::GEOMETRY.page_size();
        let leaf = layout.tree_word(0, 0) as usize * WORD_BYTES;
        assert_eq!(layout.leaf_commit(0), leaf);
        // Pages up to the origin hold no slots; one page of eight-byte slots past it needs a
        // few leaf words, rounded up to a page.
        let origin = layout.origin_page();
        assert_eq!(layout.leaf_commit(origin), leaf);
        assert_eq!(layout.leaf_commit(origin + 1), leaf + page);
        let all = layout.leaf_commit(layout.region_pages());
        assert!(all >= layout.total_words() as usize * WORD_BYTES);
        assert_eq!(all, layout.block_bytes());
        let (start, end) = layout.fixed_commit();
        assert_eq!((start, end), (0, leaf));
    }
}
