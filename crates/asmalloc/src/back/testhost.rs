//! A `Host` over a few words in ordinary memory, for the protocol tests.
//!
//! It runs the production protocols (`tree`, `region`) without a reservation: each region's
//! metadata is a vector of words laid out as in the heap, and the OS calls only record what they
//! were asked. It also keeps a ghost record of which pages of each slot hold data, so a test can
//! check that a purge never discards a live slot's data and that a slot said to be zero is.

use super::layout::Layout;
use super::tree::{self, Host};
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::HugePages;
use crate::space::{Plan, RegionId};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};

/// A word type the test host can make, with a hook the host calls when it purges.
pub(crate) trait TestWord: Word + Sized {
    /// A zero word. `leaf` is the heap-wide number of the first slot it covers, for a leaf
    /// bitmap word.
    fn new(leaf: Option<u64>) -> Self;

    /// Called by a purge for the heap-wide slots `first..=last` that touch the pages it
    /// returns, before it returns them.
    fn purging(_first: u64, _last: u64) {}
}

impl TestWord for super::word::Atomic {
    fn new(_: Option<u64>) -> Self {
        super::word::Atomic::zero()
    }
}

/// One class's regions and tree, in ordinary memory.
pub(crate) struct TestHost<W> {
    layout: Layout,
    plan: Plan,
    /// How many of the plan's regions the host has.
    regions: usize,
    meta: Vec<Box<[W]>>,
    /// Each region's `Grow` and `Hints` words, which the heap keeps outside the metadata block.
    headers: Box<[(W, W)]>,
    /// Each region's `Frontier` word, which the heap keeps outside the block for a small class.
    frontiers: Box<[W]>,
    class_words: Box<[W]>,
    top: W,
    /// For each heap-wide slot, which of its pages (counting from the slot's first page) hold
    /// data. A page bit is set by `write` and cleared by a purge of that page.
    dirty: Box<[AtomicU64]>,
    /// Pages committed per region.
    committed: Box<[AtomicUsize]>,
}

impl<W: TestWord> TestHost<W> {
    /// A host for `class` with the first `regions` regions of `plan`.
    pub(crate) fn new(plan: Plan, class: Class, regions: usize) -> TestHost<W> {
        let layout = Layout::new(plan, class).expect("the class fits the plan's regions");
        assert!(regions <= plan.objects(), "the plan has that many regions");
        let slots = layout.slots() as u64;
        let leaf = layout.tree_word(0, 0)..layout.tree_word(0, 0) + layout.leaf_words();
        let meta = (0..regions)
            .map(|r| {
                (0..layout.total_words())
                    .map(|w| {
                        let first =
                            |w: u32| r as u64 * slots + (w - leaf.start) as u64 * BITS as u64;
                        W::new(leaf.contains(&w).then(|| first(w)))
                    })
                    .collect()
            })
            .collect();
        let headers = (0..regions).map(|_| (W::new(None), W::new(None))).collect();
        TestHost {
            layout,
            plan,
            regions,
            meta,
            headers,
            frontiers: (0..regions).map(|_| W::new(None)).collect(),
            class_words: (0..64).map(|_| W::new(None)).collect(),
            top: W::new(None),
            dirty: (0..regions as u64 * slots)
                .map(|_| AtomicU64::new(0))
                .collect(),
            committed: (0..regions).map(|_| AtomicUsize::new(0)).collect(),
        }
    }

    /// Region `index`.
    pub(crate) fn region_at(&self, index: usize) -> RegionId {
        self.plan.region(index).expect("a test region")
    }

    /// The heap-wide number of a slot.
    pub(crate) fn global(&self, region: RegionId, slot: u32) -> u64 {
        region.index() as u64 * self.layout.slots() as u64 + slot as u64
    }

    fn pages_of(&self, slot: u32) -> (u32, u32) {
        let shift = self.layout.page_shift();
        let start = (self.layout.slot_bytes(slot) >> shift) as u32;
        let end = self
            .layout
            .slot_bytes(slot + 1)
            .div_ceil(self.layout.page_size()) as u32;
        (start, end)
    }

    /// Records that the program wrote to every page of a slot.
    pub(crate) fn write(&self, region: RegionId, slot: u32) {
        let (start, end) = self.pages_of(slot);
        let all = u64::MAX >> (64 - (end - start).min(64));
        self.dirty[self.global(region, slot) as usize].store(all, SeqCst);
    }

    /// Whether every byte of a slot reads as zero.
    pub(crate) fn is_zero(&self, region: RegionId, slot: u32) -> bool {
        self.dirty[self.global(region, slot) as usize].load(SeqCst) == 0
    }

    /// The free slots of a region, from its leaf bitmap.
    pub(crate) fn free_slots(&self, region: RegionId) -> Vec<u32> {
        let mut out = Vec::new();
        for w in 0..self.layout.leaf_words() {
            let mut bits = tree::word(self, tree::Node::leaf(region, w)).load();
            while bits != 0 {
                out.push(w * BITS + bits.trailing_zeros());
                bits &= bits - 1;
            }
        }
        out
    }

    /// Whether every leaf word with free bits can be reached from the class's top word through
    /// set summary bits: the property `Visible` of `tla/Summary.tla` once no thread is at work.
    pub(crate) fn all_visible(&self) -> bool {
        (0..self.regions).all(|r| {
            let region = self.region_at(r);
            (0..self.layout.leaf_words()).all(|w| {
                let node = tree::Node::leaf(region, w);
                tree::word(self, node).load() == 0 || self.path_set(node)
            })
        })
    }

    /// Whether every summary bit from `node` to the top is set.
    pub(crate) fn path_set(&self, mut node: tree::Node) -> bool {
        loop {
            let (above, bit) = match node {
                tree::Node::Region {
                    region,
                    level,
                    index,
                } if level < self.layout.height() => (
                    tree::Node::Region {
                        region,
                        level: level + 1,
                        index: index / BITS,
                    },
                    index % BITS,
                ),
                tree::Node::Region { region, .. } => {
                    let r = region.index() as u32;
                    (tree::Node::Class(r / BITS), r % BITS)
                }
                tree::Node::Class(index) => (tree::Node::Top, index),
                tree::Node::Top => return true,
            };
            if tree::word(self, above).load() & (1 << bit) == 0 {
                return false;
            }
            node = above;
        }
    }

    /// Marks slots `0..count` of a region carved, with their memory committed, as a carve
    /// would leave them.
    pub(crate) fn carve_all(&self, region: RegionId, count: u32) {
        let pages = self
            .layout
            .slot_bytes(count)
            .div_ceil(self.layout.page_size()) as u32;
        let grow = super::layout::Grow {
            carved: count,
            committed: pages,
        };
        self.grow(region).fetch_or(grow.pack());
        self.committed[region.index()].store(pages as usize, SeqCst);
    }
}

impl<W: TestWord> Host for TestHost<W> {
    type W = W;

    fn layout(&self) -> &Layout {
        &self.layout
    }

    fn tree_words(&self, region: RegionId, level: u8, count: u32) -> &[W] {
        assert!(count <= self.layout.level_words(level));
        let first = self.layout.tree_word(level, 0) as usize;
        &self.meta[region.index()][first..first + count as usize]
    }

    fn region(&self, index: usize) -> Option<RegionId> {
        if index < self.regions {
            self.plan.region(index)
        } else {
            None
        }
    }

    fn meta(&self, region: RegionId, word: u32) -> &W {
        &self.meta[region.index()][word as usize]
    }

    fn grow(&self, region: RegionId) -> &W {
        match self.layout.header_word() {
            Some(word) => self.meta(region, word),
            None => &self.headers[region.index()].0,
        }
    }

    fn hints(&self, region: RegionId) -> &W {
        match self.layout.header_word() {
            Some(word) => self.meta(region, word + 1),
            None => &self.headers[region.index()].1,
        }
    }

    fn frontier(&self, region: RegionId) -> &W {
        match self.layout.header_word() {
            Some(word) => self.meta(region, word + 2),
            None => &self.frontiers[region.index()],
        }
    }

    fn class_word(&self, index: u32) -> &W {
        &self.class_words[index as usize]
    }

    fn class_top(&self) -> &W {
        &self.top
    }

    fn commit(&self, region: RegionId, offset: usize, len: usize) -> bool {
        let page = self.layout.page_size();
        assert!(offset.is_multiple_of(page) && len.is_multiple_of(page) && len > 0);
        self.committed[region.index()].fetch_max((offset + len) / page, SeqCst);
        true
    }

    fn commit_meta(&self, _: RegionId, offset: usize, len: usize) -> bool {
        let page = self.layout.page_size();
        assert!(offset.is_multiple_of(page) && len.is_multiple_of(page) && len > 0);
        true
    }

    fn advise(&self, _: RegionId, offset: usize, len: usize, _: HugePages) -> bool {
        let page = self.layout.page_size();
        assert!(offset.is_multiple_of(page) && len.is_multiple_of(page) && len > 0);
        true
    }

    unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool {
        let page = self.layout.page_size();
        assert!(offset.is_multiple_of(page) && len.is_multiple_of(page) && len > 0);
        let start = (offset / page) as u32;
        let end = ((offset + len) / page) as u32;
        assert!(end <= self.committed[region.index()].load(SeqCst) as u32);
        let first = self.layout.slot_at(offset);
        let last = self.layout.slot_at(offset + len - 1);
        W::purging(self.global(region, first), self.global(region, last));
        for slot in first..=last {
            let (from, _) = self.pages_of(slot);
            let mut mask = 0u64;
            for p in start.max(from)..end {
                if p - from < 64 {
                    mask |= 1 << (p - from);
                }
            }
            self.dirty[self.global(region, slot) as usize].fetch_and(!mask, SeqCst);
        }
        true
    }

    fn abort(&self, message: &str) -> ! {
        panic!("asmalloc: {message}")
    }
}
