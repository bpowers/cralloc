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
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};

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
    class_words: Box<[W]>,
    top: W,
    /// For each heap-wide slot, which of its pages (counting from the slot's first page) hold
    /// data. A page bit is set by `write` and cleared by a purge of that page.
    dirty: Box<[AtomicU64]>,
    /// Pages committed per region.
    committed: Box<[AtomicUsize]>,
    /// Per region, which pages of the metadata block are committed. `meta` fails on a word in
    /// any other page, which checks BACK-HOST-01's claim that the protocols touch only committed
    /// metadata.
    meta_committed: Vec<Box<[AtomicBool]>>,
    /// Make the next OS calls fail.
    pub(crate) fail_commit: AtomicBool,
    pub(crate) fail_purge: AtomicBool,
    pub(crate) purges: AtomicUsize,
}

impl<W: TestWord> TestHost<W> {
    /// A host for `class` with the first `regions` regions of `plan`.
    pub(crate) fn new(plan: Plan, class: Class, regions: usize) -> TestHost<W> {
        let layout = Layout::new(plan, class).expect("the class fits the plan's regions");
        assert!(regions <= plan.objects(), "the plan has that many regions");
        let slots = layout.slots() as u64;
        let meta = (0..regions)
            .map(|r| {
                (0..layout.total_words())
                    .map(|w| {
                        let leaf = w < layout.leaf_words();
                        W::new(leaf.then_some(r as u64 * slots + w as u64 * BITS as u64))
                    })
                    .collect()
            })
            .collect();
        let host = TestHost {
            layout,
            plan,
            regions,
            meta,
            class_words: (0..64).map(|_| W::new(None)).collect(),
            top: W::new(None),
            dirty: (0..regions as u64 * slots)
                .map(|_| AtomicU64::new(0))
                .collect(),
            committed: (0..regions).map(|_| AtomicUsize::new(0)).collect(),
            meta_committed: (0..regions)
                .map(|_| {
                    let pages = layout.fixed_commit().1.div_ceil(layout.page_size());
                    (0..pages).map(|_| AtomicBool::new(false)).collect()
                })
                .collect(),
            fail_commit: AtomicBool::new(false),
            fail_purge: AtomicBool::new(false),
            purges: AtomicUsize::new(0),
        };
        // As `assign` does before it tags a region.
        let (start, end) = layout.fixed_commit();
        for r in 0..regions {
            host.mark_meta(r, start, end - start);
        }
        host
    }

    fn mark_meta(&self, region: usize, offset: usize, len: usize) {
        let page = self.layout.page_size();
        for p in offset / page..(offset + len) / page {
            self.meta_committed[region][p].store(true, SeqCst);
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
        self.mark_meta(region.index(), 0, self.layout.leaf_commit(pages));
        self.meta(region, self.layout.grow_word())
            .fetch_or(grow.pack());
        self.committed[region.index()].store(pages as usize, SeqCst);
    }
}

impl<W: TestWord> Host for TestHost<W> {
    type W = W;

    fn layout(&self) -> &Layout {
        &self.layout
    }

    fn region(&self, index: usize) -> Option<RegionId> {
        if index < self.regions {
            self.plan.region(index)
        } else {
            None
        }
    }

    fn meta(&self, region: RegionId, word: u32) -> &W {
        let page = word as usize * super::layout::WORD_BYTES / self.layout.page_size();
        assert!(
            self.meta_committed[region.index()][page].load(SeqCst),
            "metadata word {word} of region {} is not committed",
            region.index()
        );
        &self.meta[region.index()][word as usize]
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
        if self.fail_commit.load(SeqCst) {
            return false;
        }
        self.committed[region.index()].fetch_max((offset + len) / page, SeqCst);
        true
    }

    fn commit_meta(&self, region: RegionId, offset: usize, len: usize) -> bool {
        let page = self.layout.page_size();
        assert!(offset.is_multiple_of(page) && len.is_multiple_of(page) && len > 0);
        if self.fail_commit.load(SeqCst) {
            return false;
        }
        self.mark_meta(region.index(), offset, len);
        true
    }

    fn advise(&self, _: RegionId, offset: usize, len: usize, _: HugePages) {
        let page = self.layout.page_size();
        assert!(offset.is_multiple_of(page) && len.is_multiple_of(page) && len > 0);
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
        if self.fail_purge.load(SeqCst) {
            return false;
        }
        self.purges.fetch_add(1, SeqCst);
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
