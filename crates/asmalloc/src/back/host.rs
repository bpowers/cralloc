//! The protocols' `Host` over the real heap: one class's tree words in the back end's state, its
//! regions' metadata in the reservation, and the OS calls through `Space`.

use super::layout::{Layout, WORD_BYTES};
use super::tree::Host;
use super::word::Atomic;
use crate::os::{HugePages, Os};
use crate::space::{MAX_REGIONS, RegionId, Space};
use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;

/// Words in a class's per-region summary: one bit per region.
pub(crate) const CLASS_WORDS: usize = MAX_REGIONS / 64;

/// What the back end keeps per class outside the regions: the two class levels of the summary
/// tree, and the region being carved. All zero means no regions.
pub(crate) struct ClassState {
    /// A bit per class word that may be non-zero.
    pub(crate) top: Atomic,
    /// A bit per region whose top word may be non-zero.
    pub(crate) words: [Atomic; CLASS_WORDS],
    /// The region carves come from, plus one; zero for none yet.
    pub(crate) carving: AtomicU32,
}

impl ClassState {
    pub(crate) const fn new() -> ClassState {
        ClassState {
            top: Atomic::zero(),
            words: [const { Atomic::zero() }; CLASS_WORDS],
            carving: AtomicU32::new(0),
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
pub(crate) struct ClassHost<'a, O: Os> {
    pub(crate) space: Space<'a, O>,
    pub(crate) class: &'a ClassState,
    pub(crate) layout: Layout,
}

impl<O: Os> ClassHost<'_, O> {
    fn region_at(&self, region: RegionId, offset: usize) -> NonNull<u8> {
        self.space
            .region(region)
            .map_addr(|addr| addr.saturating_add(offset))
    }

    fn meta_at(&self, region: RegionId, offset: usize) -> NonNull<u8> {
        self.space
            .metadata(region)
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
    fn class_word(&self, index: u32) -> &Atomic {
        &self.class.words[index as usize % CLASS_WORDS]
    }

    #[inline(always)]
    fn class_top(&self) -> &Atomic {
        &self.class.top
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

    fn populate(&self, region: RegionId, offset: usize, len: usize) {
        let _ = self.space.populate(self.region_at(region, offset), len);
    }

    unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool {
        // SAFETY: the caller promises what `Space::purge` asks: nothing touches the range during
        // the call and nothing in it is needed.
        unsafe { self.space.purge(self.region_at(region, offset), len) }.is_ok()
    }

    fn abort(&self, message: &str) -> ! {
        self.space.os().abort(message)
    }
}
