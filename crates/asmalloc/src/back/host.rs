//! The protocols' `Host` over the real heap: one class's tree words in the back end's state, its
//! regions' metadata in the reservation, and the OS calls through `Space`.

use super::layout::{Layout, Memo, WORD_BYTES};
use super::tree::Host;
use super::word::Atomic;
use crate::lined::{LINE, Lined, lines};
use crate::os::{HugePages, Os};
use crate::space::{MAX_REGIONS, RegionId, Space};
use core::cell::Cell;
use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;

/// Words in a class's per-region summary: one bit per region.
pub(crate) const CLASS_WORDS: usize = MAX_REGIONS / 64;

/// What the back end keeps per class outside the regions: the two class levels of the summary
/// tree, and the region being carved. All zero means no regions.
///
/// The layout, which every operation on the class reads and nothing writes once it is worked
/// out, is on lines of its own (`crate::lined`): claims and releases write the summary words,
/// and a layout that shared a line with them, this class's or the next one's, cost a miss on
/// every call. The carving word, written when a region is assigned, is apart from the summary
/// words, which claims and releases write.
pub(crate) struct ClassState {
    /// The class's layout, once worked out.
    pub(crate) layout: Lined<Memo>,
    /// The class levels of the summary tree: a bit per class word that may be non-zero, then
    /// a bit per region whose top word may be non-zero. One protocol writes them all.
    pub(crate) summary: Lined<Summary>,
    /// The region carves come from, plus one; zero for none yet.
    pub(crate) carving: Lined<AtomicU32>,
}

/// A region's header: its `Grow` and `Hints` words (`layout`). All zero means nothing carved,
/// nothing committed, no huge-page part and no purge under way.
///
/// The headers of all regions are in the back end's state, side by side, not in the regions'
/// metadata blocks: every class carves and so writes its header, but a small class needs its
/// block only once its slots reach the bitmaps (a large class writes its slots' size words
/// there on every allocation), and a block's first touched page costs a page of memory and,
/// 128 MiB from the next class's, often a page of page tables (`docs/research/memory.md`,
/// "Small heaps").
pub(crate) struct RegionHeader {
    pub(crate) grow: Atomic,
    pub(crate) hints: Atomic,
}

impl RegionHeader {
    pub(crate) const fn new() -> RegionHeader {
        RegionHeader {
            grow: Atomic::zero(),
            hints: Atomic::zero(),
        }
    }
}

/// The headers of every region, by region number, four to a line: an exception to
/// `crate::lined`'s rule.
///
/// - `Grow` is written by the compare-and-swap in `region::carve`, which runs once per carving
///   fill (twice when it also publishes a commit), once per first use of a large slot and per
///   small allocation by a thread with no per-CPU cache (`Back::alloc`), and by `top_up` when
///   the purge walk reaches a region's top.
/// - `Hints` is written by `switch_to_huge` once per region, by `prepare` and `unprepare` at
///   assignment, and by `set_cursor` once per unit the purge walk passes, from the clocked pass
///   and from `Back::grow` when a carve pays a growth debt.
///
/// Sharing a line is acceptable because neither word is written in steady state: a carve is
/// followed by faults on the memory it hands out and a purge walk by a system call per unit,
/// and a carving fill already writes the shared growth word. Fills, launders and frees only
/// read `Grow`. A line each would make the state 256 KiB, which the simulated heaps the tests
/// build on their stacks cannot hold.
pub(crate) type Headers = [RegionHeader; MAX_REGIONS];

/// The class levels of a class's summary tree.
pub(crate) struct Summary {
    pub(crate) top: Atomic,
    pub(crate) words: [Atomic; CLASS_WORDS],
}

const _: () = assert!(
    size_of::<ClassState>() == LINE * (lines(size_of::<Memo>()) + lines(size_of::<Summary>()) + 1)
);

impl ClassState {
    pub(crate) const fn new() -> ClassState {
        ClassState {
            layout: Lined::new(Memo::new()),
            summary: Lined::new(Summary {
                top: Atomic::zero(),
                words: [const { Atomic::zero() }; CLASS_WORDS],
            }),
            carving: Lined::new(AtomicU32::new(0)),
        }
    }
}

/// One class of the heap, as the protocols see it.
///
/// Invariant: `layout` is this class's layout in the space's geometry, and every region a caller names
/// is assigned to this class in the region table and has had its fixed metadata committed. The
/// back end makes a `ClassHost` only for a class with a layout (`Back::host`), and names only
/// regions it found through this class's summary tree, its carving word, or a region-table
/// lookup that returned this class.
///
/// A `ClassHost` lives for one operation, on the stack of the thread doing it. It remembers the
/// metadata block of the last region it was asked about (`block`), since an operation reaches
/// many words of one region and working out a block's address costs more than using it.
pub(crate) struct ClassHost<'a, O: Os> {
    pub(crate) space: Space<'a, O>,
    pub(crate) class: &'a ClassState,
    pub(crate) headers: &'a Headers,
    pub(crate) layout: Layout,
    pub(crate) block: Cell<Option<(RegionId, NonNull<u8>)>>,
}

impl<'a, O: Os> ClassHost<'a, O> {
    #[inline(always)]
    pub(crate) fn new(
        space: Space<'a, O>,
        class: &'a ClassState,
        headers: &'a Headers,
        layout: Layout,
    ) -> Self {
        ClassHost {
            space,
            class,
            headers,
            layout,
            block: Cell::new(None),
        }
    }

    fn region_at(&self, region: RegionId, offset: usize) -> NonNull<u8> {
        self.space
            .region(region)
            .map_addr(|addr| addr.saturating_add(offset))
    }

    /// The metadata block of `region`, as [`Space::metadata`] gives it: from the last one
    /// worked out if it is this region's, so that the region is checked once per operation, not
    /// once per word.
    #[inline(always)]
    fn block(&self, region: RegionId) -> NonNull<u8> {
        if let Some((seen, block)) = self.block.get()
            && seen == region
        {
            return block;
        }
        let block = self.space.metadata(region);
        self.block.set(Some((region, block)));
        block
    }

    fn meta_at(&self, region: RegionId, offset: usize) -> NonNull<u8> {
        self.block(region)
            .map_addr(|addr| addr.saturating_add(offset))
    }
}

impl<O: Os> Host for ClassHost<'_, O> {
    type W = Atomic;

    #[inline(always)]
    fn layout(&self) -> &Layout {
        &self.layout
    }

    #[inline(always)]
    fn region(&self, index: usize) -> Option<RegionId> {
        self.space.plan().region(index)
    }

    #[inline(always)]
    fn meta(&self, region: RegionId, word: u32) -> &Atomic {
        let at = self
            .meta_at(region, word as usize * WORD_BYTES)
            .cast::<Atomic>();
        // SAFETY: BACK-HOST-01 in docs/safety/asmalloc/back/host-safety.md. The word lies in the
        // region's metadata block, is 8-aligned, and is committed; it is only ever accessed
        // atomically; the borrow of the OS in `space` keeps the memory alive.
        unsafe { at.as_ref() }
    }

    #[inline(always)]
    fn leaves(&self, region: RegionId, count: u32) -> &[Atomic] {
        if count > self.layout.level_words(0) {
            self.abort("more leaf words than a region has");
        }
        let first = self
            .meta_at(region, self.layout.tree_word(0, 0) as usize * WORD_BYTES)
            .cast::<Atomic>();
        // SAFETY: BACK-HOST-03 in docs/safety/asmalloc/back/host-safety.md. The words are the
        // first `count` of the region's leaf bitmap, which lies in its metadata block, and the
        // caller knows each to be committed; they are only ever accessed atomically.
        unsafe { core::slice::from_raw_parts(first.as_ptr(), count as usize) }
    }

    #[inline(always)]
    fn grow(&self, region: RegionId) -> &Atomic {
        &self.headers[region.index() % MAX_REGIONS].grow
    }

    #[inline(always)]
    fn hints(&self, region: RegionId) -> &Atomic {
        &self.headers[region.index() % MAX_REGIONS].hints
    }

    #[inline(always)]
    fn class_word(&self, index: u32) -> &Atomic {
        &self.class.summary.words[index as usize % CLASS_WORDS]
    }

    #[inline(always)]
    fn class_top(&self) -> &Atomic {
        &self.class.summary.top
    }

    fn commit(&self, region: RegionId, offset: usize, len: usize) -> bool {
        self.space
            .commit(self.region_at(region, offset), len)
            .is_ok()
    }

    fn commit_meta(&self, region: RegionId, offset: usize, len: usize) -> bool {
        self.space.commit(self.meta_at(region, offset), len).is_ok()
    }

    fn advise(&self, region: RegionId, offset: usize, len: usize, advice: HugePages) -> bool {
        self.space
            .advise_huge(self.region_at(region, offset), len, advice)
            .is_ok()
    }

    unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool {
        // SAFETY: BACK-HOST-02. the caller promises what `Space::purge` asks: nothing touches the range during
        // the call and nothing in it is needed.
        unsafe { self.space.purge(self.region_at(region, offset), len) }.is_ok()
    }

    fn abort(&self, message: &str) -> ! {
        self.space.os().abort(message)
    }
}
