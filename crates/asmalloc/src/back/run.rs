//! Objects bigger than a region: runs of whole regions.
//!
//! A run is tagged in the region table: its first region `RunHead`, the rest `RunTail`. The
//! object starts at the head's base, so a run's length is the head plus the tails that follow
//! it; the next run's head, or any other tag, ends it.
//!
//! **Allocation** looks from the top of the heap down for enough consecutive unassigned
//! regions, so runs stay away from class regions, which are taken from the bottom. It claims
//! the head first and then each tail, every one with a compare-and-swap from unassigned; if one
//! fails it gives back what it took, last first. Claiming the head first means tails always
//! follow their own head, which is what makes the length well defined. Then it commits the
//! run's memory. Unassigned regions always read as zero (never touched, or purged when their
//! last run was freed), so a run is always zeroed.
//!
//! **Free** purges the run's memory, which restores the zero promise, and then gives the
//! regions back, tails last first and the head last. If the OS refuses the purge, the run stays
//! assigned and is never reused: a leak of address space, not corruption.

use super::{Back, Kind};
use crate::os::{HugePages, Os};
use crate::space::{RegionId, RegionTag};
use core::ptr::NonNull;

impl<O: Os> Back<'_, O> {
    /// Allocates a run of whole regions holding at least `size` bytes, aligned to `align` (a
    /// power of two). The memory reads as zero. `None` if there is no such run free, or the OS
    /// refused memory.
    pub fn alloc_run(&self, size: usize, align: usize) -> Option<NonNull<u8>> {
        let plan = self.space.plan();
        let shift = plan.region_shift();
        let regions = size.div_ceil(1 << shift).max(1);
        if regions > plan.objects() || !align.is_power_of_two() {
            return None;
        }
        for first in (0..=plan.objects() - regions).rev() {
            let Some(head) = plan.region(first) else {
                continue;
            };
            let start = self.space.region(head);
            // Regions are aligned to their size, so only a larger alignment can rule one out.
            if start.addr().get() & (align - 1) != 0 {
                continue;
            }
            if !self.claim_run(first, regions) {
                continue;
            }
            let len = regions << shift;
            if self.space.commit(start, len).is_err() {
                self.unclaim_run(first, regions, regions);
                return None;
            }
            let _ = self.space.advise_huge(start, len, HugePages::Prefer);
            return Some(start);
        }
        None
    }

    /// Frees the run starting at `ptr`. Aborts if `ptr` is not the start of a run.
    pub fn free_run(&self, ptr: NonNull<u8>) {
        let Some(Kind::Run(regions)) = self.lookup(ptr.as_ptr()) else {
            self.abort("free of a pointer that is not the start of a run");
        };
        let shift = self.space.plan().region_shift();
        let first = (ptr.addr().get() - self.space.base().addr().get()) >> shift;
        // SAFETY: BACK-RUN-01 in docs/safety/asmalloc/back/run-safety.md. The caller owns the
        // run and is freeing it, so nothing uses its memory, and the range is whole regions of
        // the reservation.
        if unsafe { self.space.purge(ptr, regions << shift) }.is_err() {
            return;
        }
        self.unclaim_run(first, regions, regions);
    }

    /// The number of regions in the run whose head is `head`.
    pub(super) fn run_length(&self, head: RegionId) -> usize {
        let plan = self.space.plan();
        let mut length = 1;
        while let Some(next) = plan.region(head.index() + length) {
            if self.state.table.get(next) != RegionTag::RunTail {
                break;
            }
            length += 1;
        }
        length
    }

    /// Claims regions `first..first + count`, the head first. Gives back what it took and
    /// returns `false` if any was taken already.
    fn claim_run(&self, first: usize, count: usize) -> bool {
        let plan = self.space.plan();
        let table = &self.state.table;
        let free = (first..first + count).all(|i| {
            plan.region(i)
                .is_some_and(|r| table.get(r) == RegionTag::Unassigned)
        });
        if !free {
            return false;
        }
        for i in 0..count {
            let tag = if i == 0 {
                RegionTag::RunHead
            } else {
                RegionTag::RunTail
            };
            let taken = plan
                .region(first + i)
                .is_some_and(|r| table.replace(r, RegionTag::Unassigned, tag).is_ok());
            if !taken {
                self.unclaim_run(first, count, i);
                return false;
            }
        }
        true
    }

    /// Gives back the first `taken` regions of the run `first..first + count`, last first.
    fn unclaim_run(&self, first: usize, count: usize, taken: usize) {
        let plan = self.space.plan();
        for i in (0..taken.min(count)).rev() {
            let tag = if i == 0 {
                RegionTag::RunHead
            } else {
                RegionTag::RunTail
            };
            if let Some(region) = plan.region(first + i) {
                let _ = self.state.table.replace(region, tag, RegionTag::Unassigned);
            }
        }
    }
}
