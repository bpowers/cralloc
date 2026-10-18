//! Where a region's metadata words are for one class, and the arithmetic between slots, bytes,
//! pages and purge units.
//!
//! A region's metadata block (`Space::metadata`) is an array of 64-bit words, all zero until
//! touched:
//!
//! ```text
//! word 0                      leaf bitmap: one bit per slot, set means free
//! leaf_words                  header: Grow (watermark, committed pages)
//! leaf_words + 1              header: Hints (start of the huge-page part, purge cursor)
//! ...                         summary levels 1 to height, each a word per 64 words below
//! ...                         clean bitmap: one bit per page, set means purged and untouched
//! ...                         zero bitmap, large classes only: one bit per slot, set means
//!                             free and known to read as zero
//! ...                         sizes, large classes only: one word per slot, the size the
//!                             object in it was last allocated or resized to
//! ```
//!
//! The leaf bitmap comes first so that it can be committed a page at a time as the region grows,
//! starting at the block's first byte; everything after it is committed when the region is
//! assigned. All zero means nothing is free, nothing is carved, nothing is committed, the region
//! has no huge-page part and no purge is under way.

use super::word::BITS;
use crate::classes::{Class, Divisor};
use crate::space::Plan;

/// The most levels a region's bit tree has, leaf included: a 4 GiB region of 8-byte slots has
/// 2^23 leaf words, then 2^17, 2^11, 2^5 and 1.
pub(crate) const MAX_LEVELS: usize = 5;

/// Bytes per metadata word.
pub(crate) const WORD_BYTES: usize = 8;

/// Header words after the leaf bitmap.
const HEADER_WORDS: u32 = 2;

/// The metadata layout and slot arithmetic of one class in one geometry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Layout {
    size: usize,
    divisor: Divisor,
    slots: u32,
    /// The top level of the region's tree: 0 if the leaf bitmap is a single word.
    height: u8,
    /// Word offset of each level of the tree; level 0 is the leaf bitmap at offset 0.
    offset: [u32; MAX_LEVELS],
    /// Words in each level.
    count: [u32; MAX_LEVELS],
    pages: u32,
    clean: u32,
    clean_count: u32,
    /// Word offset of the zero bitmap, or 0 if the class has none (small classes).
    zero: u32,
    /// Word offset of the per-slot sizes, or 0 if the class has none (small classes).
    sizes: u32,
    total: u32,
    page_shift: u8,
    huge_shift: u8,
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
        let geometry = plan.geometry();
        let slots = slots_in_region(plan, class);
        if slots == 0 {
            return None;
        }
        let mut offset = [0u32; MAX_LEVELS];
        let mut count = [0u32; MAX_LEVELS];
        count[0] = slots.div_ceil(BITS);
        let mut next = count[0] + HEADER_WORDS;
        let mut level = 0;
        while count[level] > 1 {
            level += 1;
            if level >= MAX_LEVELS {
                return None;
            }
            count[level] = count[level - 1].div_ceil(BITS);
            offset[level] = next;
            next += count[level];
        }
        let page_shift = geometry.page_size().trailing_zeros();
        let pages = (plan.region_size() >> page_shift) as u32;
        let clean = next;
        let clean_count = pages.div_ceil(BITS);
        next += clean_count;
        let (zero, sizes) = if class.is_small() {
            (0, 0)
        } else {
            let zero = next;
            let sizes = zero + count[0];
            next = sizes + slots;
            (zero, sizes)
        };
        if next as usize * WORD_BYTES > plan.metadata_size() {
            return None;
        }
        Some(Layout {
            size: class.size(),
            divisor: class.divisor(),
            slots,
            height: level as u8,
            offset,
            count,
            pages,
            clean,
            clean_count,
            zero,
            sizes,
            total: next,
            page_shift: page_shift as u8,
            huge_shift: geometry.huge_page_size().trailing_zeros() as u8,
        })
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
    #[cfg(any(test, kani))]
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

    /// Word offset of the `Grow` header word.
    #[inline(always)]
    pub(crate) const fn grow_word(&self) -> u32 {
        self.count[0]
    }

    /// Word offset of the `Hints` header word.
    #[inline(always)]
    pub(crate) const fn hints_word(&self) -> u32 {
        self.count[0] + 1
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
        self.clean_count
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
    #[cfg(any(test, kani))]
    #[inline(always)]
    pub(crate) const fn total_words(&self) -> u32 {
        self.total
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

    /// The offset of slot `index` from the region base, `index * size`. For `index <= slots`
    /// this is at most the region size.
    #[inline(always)]
    pub(crate) const fn slot_bytes(&self, index: u32) -> usize {
        index as usize * self.size
    }

    /// The slot holding byte `offset` of the region. `offset` must be below the region size,
    /// which is at most 4 GiB.
    #[inline(always)]
    pub(crate) const fn slot_at(&self, offset: usize) -> u32 {
        self.divisor.divide(offset as u32)
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

    /// Bytes of leaf bitmap that must be writable for a region committed up to page
    /// `committed`: the words covering every slot that starts below that page, rounded up to a
    /// whole page.
    #[inline]
    pub(crate) const fn leaf_commit(&self, committed: u32) -> usize {
        let bytes = (committed as usize) << self.page_shift;
        let covered = if bytes == 0 {
            0
        } else {
            let last = self.slot_at(bytes - 1) + 1;
            if last < self.slots { last } else { self.slots }
        };
        let leaf = covered.div_ceil(BITS) as usize * WORD_BYTES;
        let page = self.page_size();
        (leaf + page - 1) & !(page - 1)
    }

    /// The byte range of the metadata block that is committed when the region is assigned:
    /// from the page holding the header to the end of the layout, as whole pages.
    #[inline]
    pub(crate) const fn fixed_commit(&self) -> (usize, usize) {
        let page = self.page_size();
        let start = (self.grow_word() as usize * WORD_BYTES) & !(page - 1);
        let end = (self.total as usize * WORD_BYTES + page - 1) & !(page - 1);
        (start, end)
    }

    /// The next thing a top-down purge pass looks at below page `end` (zero for the top), in a
    /// region with `carved` slots and a huge-page part from `huge_from`.
    ///
    /// Only pages wholly below the watermark count. In the huge-page part a unit is whole huge
    /// pages; in the rest, whole pages. A unit ends at a unit boundary and starts at the
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

/// The number of slots of `class` in a region of `plan`: `region_size / size`, zero if a slot
/// does not fit. Without a divide instruction, because the back end computes layouts at run time
/// from the reservation's plan: `Divisor` gives `m = (region_size - 1) / size`, and one more
/// slot fits exactly when the size divides the region size.
pub(crate) const fn slots_in_region(plan: Plan, class: Class) -> u32 {
    let region = plan.region_size();
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
        let layout = Layout::new(Geometry::DESIGN.largest_plan(), Class::SMALLEST).unwrap();
        assert_eq!(layout.slots(), 1 << 29);
        assert_eq!(layout.height(), 4);
        assert_eq!(
            [0, 1, 2, 3, 4].map(|l| layout.level_words(l)),
            [1 << 23, 1 << 17, 1 << 11, 1 << 5, 1]
        );
        assert_eq!(layout.clean_words(), (1 << 20) / 64);
    }

    #[test]
    fn the_tiny_geometry_fills_its_metadata_exactly_for_eight_byte_slots() {
        let layout = Layout::new(Tiny::GEOMETRY.largest_plan(), Class::SMALLEST).unwrap();
        assert_eq!(layout.total_words() as usize * WORD_BYTES, 64);
        assert_eq!(layout.fixed_commit(), (0, 64));
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
                        assert!(unit.start == h || unit.start % layout.huge_pages() == 0);
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
        assert_eq!(layout.leaf_commit(0), 0);
        // One page of eight-byte slots needs a few leaf words, rounded up to a page.
        assert_eq!(layout.leaf_commit(1), Standard::GEOMETRY.page_size());
        let all = layout.leaf_commit(layout.region_pages());
        assert!(all >= layout.leaf_words() as usize * WORD_BYTES);
        let (start, end) = layout.fixed_commit();
        assert!(start <= layout.leaf_words() as usize * WORD_BYTES);
        assert!(end >= layout.total_words() as usize * WORD_BYTES);
    }
}
