//! The protocols' `Host` over the real heap: one class's tree words in the back end's state, its
//! regions' metadata in the reservation, and the OS calls through `Space`.

use super::layout::{Layout, MAX_LEVELS, Memo, WORD_BYTES};
use super::tree::Host;
use super::word::{Atomic, Word};
use crate::lined::{LINE, Lined, lines};
use crate::os::{HugePages, Os};
use crate::space::{MAX_REGIONS, RegionId, Space};
use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;

/// Words in a class's per-region summary: one bit per region.
pub(crate) const CLASS_WORDS: usize = MAX_REGIONS / 64;

/// EXPERIMENT (domain-affine back end): the cache domains the back end keeps apart. A small
/// class has a carving region per domain, and a fill prefers free slots in its own domain's
/// regions. Domains fold modulo this; 16 keeps the eight CPUs of the suite (0-3, 8-11) apart
/// when every CPU is made a domain of its own.
pub(crate) const BACK_DOMAINS: usize = 16;

/// EXPERIMENT: which regions belong to each domain, one bit per region, by domain. Written when
/// a region is assigned (and given back), read by a fill's search for its own domain's slots.
pub(crate) type Owners = [[Atomic; CLASS_WORDS]; BACK_DOMAINS];

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
    /// The region carves come from, plus one; zero for none yet. EXPERIMENT: one per domain
    /// (`BACK_DOMAINS`); large classes and threads without a domain use domain 0's.
    pub(crate) carving: Lined<[AtomicU32; BACK_DOMAINS]>,
}

/// A region's header: its `Grow` and `Hints` words (`layout`). All zero means nothing carved,
/// nothing committed, no huge-page part and no purge under way.
///
/// A small class's headers are in the back end's state, side by side, not in the regions'
/// metadata blocks: every class carves and so writes its header, but a small class needs its
/// block only once its slots reach the bitmaps, and a block's first touched page costs a page
/// of memory and, 128 MiB from the next class's, often a page of page tables
/// (`docs/research/memory.md`, "Small heaps"). A large class touches its block on every
/// allocation and free anyway, so its header stays there (`Layout::header_word`), and its
/// entry here is unused.
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

/// The headers of every region of a small class, by region number, four to a line: an exception to
/// `crate::lined`'s rule.
///
/// - `Grow` is written by the compare-and-swap in `region::carve`, which runs once per carving
///   fill (twice when it also publishes a commit) and per small allocation by a thread with no
///   per-CPU cache (`Back::alloc`), and by `top_up` when the purge walk reaches a region's top.
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

/// The `Frontier` word of every region, by region number, eight to a line: an exception to
/// `crate::lined`'s rule, as [`Headers`] is. Large classes' are here too, not in their blocks'
/// headers: a word that only a thread holding a claim in the region writes needs no other
/// reader or writer to know the region's class.
///
/// Every fill and every claim of a single slot reads its region's word, and writes it only
/// when it reaches higher than any claim of the interval so far, or is the region's first
/// claim of an interval. In steady reuse that is the first fill of each interval and the few
/// after it that reach a little higher: a handful of writes a region a second. A line each
/// would make the state another 224 KiB, too much for the simulated heaps the tests build on
/// their stacks.
pub(crate) type Frontiers = [Atomic; MAX_REGIONS];

/// The class levels of a class's summary tree.
pub(crate) struct Summary {
    pub(crate) top: Atomic,
    pub(crate) words: [Atomic; CLASS_WORDS],
}

const _: () = assert!(
    size_of::<ClassState>()
        == LINE
            * (lines(size_of::<Memo>())
                + lines(size_of::<Summary>())
                + lines(size_of::<[AtomicU32; BACK_DOMAINS]>()))
);

impl ClassState {
    pub(crate) const fn new() -> ClassState {
        ClassState {
            layout: Lined::new(Memo::new()),
            summary: Lined::new(Summary {
                top: Atomic::zero(),
                words: [const { Atomic::zero() }; CLASS_WORDS],
            }),
            carving: Lined::new([const { AtomicU32::new(0) }; BACK_DOMAINS]),
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
/// A `ClassHost` lives for one operation, on the stack of the thread doing it.
pub(crate) struct ClassHost<'a, O: Os> {
    pub(crate) space: Space<'a, O>,
    pub(crate) class: &'a ClassState,
    pub(crate) headers: &'a Headers,
    pub(crate) frontiers: &'a Frontiers,
    pub(crate) layout: Layout,
    /// EXPERIMENT: the regions of each domain.
    pub(crate) owners: &'a Owners,
    /// EXPERIMENT: the domain whose regions a search prefers and whose carving word carves;
    /// `None` for no preference (carving in domain 0).
    pub(crate) domain: Option<usize>,
    /// EXPERIMENT: a search looks only in the domain's regions.
    pub(crate) own_only: bool,
}

impl<'a, O: Os> ClassHost<'a, O> {
    #[inline(always)]
    pub(crate) fn new(
        space: Space<'a, O>,
        class: &'a ClassState,
        headers: &'a Headers,
        frontiers: &'a Frontiers,
        layout: Layout,
        owners: &'a Owners,
        domain: Option<usize>,
    ) -> Self {
        ClassHost {
            space,
            class,
            headers,
            frontiers,
            layout,
            owners,
            domain,
            own_only: false,
        }
    }

    /// EXPERIMENT: whether `region` is in this operation's domain.
    #[inline(always)]
    pub(crate) fn owns(&self, region: usize) -> bool {
        let (word, bit) = self.owner_bit(self.carving_domain(), region);
        word.load() & bit != 0
    }

    /// EXPERIMENT: the domain this operation carves in, below `BACK_DOMAINS`.
    #[inline(always)]
    pub(crate) fn carving_domain(&self) -> usize {
        self.domain.unwrap_or(0) % BACK_DOMAINS
    }

    /// EXPERIMENT: the carving word of this operation's domain.
    #[inline(always)]
    pub(crate) fn carving(&self) -> &'a AtomicU32 {
        &self.class.carving[self.carving_domain()]
    }

    /// EXPERIMENT: the word of `owners` for `domain` that has `region`'s bit, and the bit.
    #[inline(always)]
    pub(crate) fn owner_bit(&self, domain: usize, region: usize) -> (&'a Atomic, u64) {
        (
            &self.owners[domain % BACK_DOMAINS][(region / 64) % CLASS_WORDS],
            1 << (region % 64),
        )
    }

    fn region_at(&self, region: RegionId, offset: usize) -> NonNull<u8> {
        self.space
            .region(region)
            .map_addr(|addr| addr.saturating_add(offset))
    }

    /// Byte `offset` of `region`'s metadata block, as [`Space::metadata`] gives the block.
    /// Plain pointer arithmetic, worked out afresh each time: an operation reaches a handful of
    /// words, and keeping the last block it found cost more than finding it.
    #[inline(always)]
    fn meta_at(&self, region: RegionId, offset: usize) -> *mut u8 {
        self.space.metadata_at(region, offset)
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
        unsafe { &*at }
    }

    #[inline(always)]
    fn tree_words(&self, region: RegionId, level: u8, count: u32) -> &[Atomic] {
        if level as usize >= MAX_LEVELS || count > self.layout.level_words(level) {
            self.abort("more words than a level of a region's tree has");
        }
        let first = self
            .meta_at(
                region,
                self.layout.tree_word(level, 0) as usize * WORD_BYTES,
            )
            .cast::<Atomic>();
        // SAFETY: BACK-HOST-03 in docs/safety/asmalloc/back/host-safety.md. The words are the
        // first `count` of one level of the region's tree, which lies in its metadata block,
        // and the caller knows each to be committed; they are only ever accessed atomically.
        unsafe { core::slice::from_raw_parts(first, count as usize) }
    }

    #[inline(always)]
    fn grow(&self, region: RegionId) -> &Atomic {
        match self.layout.header_word() {
            Some(word) => self.meta(region, word),
            None => &self.headers[region.index() % MAX_REGIONS].grow,
        }
    }

    #[inline(always)]
    fn hints(&self, region: RegionId) -> &Atomic {
        match self.layout.header_word() {
            Some(word) => self.meta(region, word + 1),
            None => &self.headers[region.index() % MAX_REGIONS].hints,
        }
    }

    #[inline(always)]
    fn frontier(&self, region: RegionId) -> &Atomic {
        &self.frontiers[region.index() % MAX_REGIONS]
    }

    #[inline(always)]
    fn class_word(&self, index: u32) -> &Atomic {
        &self.class.summary.words[index as usize % CLASS_WORDS]
    }

    #[inline(always)]
    fn class_top(&self) -> &Atomic {
        &self.class.summary.top
    }

    #[inline(always)]
    fn falls_back(&self) -> bool {
        !self.own_only
    }

    #[inline(always)]
    fn preferred(&self, index: u32) -> Option<u64> {
        let domain = self.domain?;
        Some(self.owners[domain % BACK_DOMAINS][index as usize % CLASS_WORDS].load())
    }

    fn commit(&self, region: RegionId, offset: usize, len: usize) -> bool {
        self.space
            .commit(self.region_at(region, offset), len)
            .is_ok()
    }

    fn commit_meta(&self, region: RegionId, offset: usize, len: usize) -> bool {
        let block = self.space.metadata(region);
        let start = block.map_addr(|addr| addr.saturating_add(offset));
        self.space.commit(start, len).is_ok()
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
