//! Retained memory, the cap, and the purge driver.
//!
//! **Retained memory** is the back end's estimate of the bytes of free slots in its bitmaps that
//! are still resident. A launder or a free adds what it returned; a claim subtracts what it took,
//! less what the clean bits say was not resident; a purge subtracts the pages it returned that
//! were not already clean. The count saturates at zero rather than going negative.
//!
//! **The cap** (`policy::retained_cap`). The free that pushes the count past it purges until the
//! count is back under, within `policy::PURGE_CAP_WORDS` of bitmap reads; while the count stays
//! over, each heartbeat purges one step of `policy::PURGE_STEP_WORDS`. Below the cap nothing is
//! purged: that is the "purge late" rule.
//!
//! **Order.** Purging walks the regions from the highest address down, first those of large
//! classes, then those of small ones, skipping regions whose class summary bit says nothing in
//! them is free. Within a region it walks units of pages from the watermark down
//! (`Layout::unit_below`): whole pages in the small-page part, whole huge pages in the huge-page
//! part, so a huge page is never split. Each region keeps its own cursor in its `Hints`, and
//! the walk over regions is one word in the state; both are hints, and two purgers on the same
//! unit are safe because each purge is a claim (`region::purge_unit`).
//!
//! **Fragmentation is outside the cap.** Free slots on pages that also hold live ones cannot be
//! purged, yet they are counted. When a whole pass over every region purges nothing and has no
//! purge refused by the OS, what is left in the count is of that kind, and the pass subtracts
//! it, so a fragmented heap does not purge on every call.
//!
//! **Trim** purges every unit of every region, ignoring the cap, the budgets and the cursors,
//! and then subtracts what the count held when it began.

use super::Back;
use super::layout::{Hints, Step};
use super::policy::{PURGE_CAP_WORDS, PURGE_STEP_WORDS, retained_cap};
use super::region::{self, Purge};
use super::tree::Host;
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use core::sync::atomic::Ordering::Relaxed;

/// The purge walk's position, one word: bit 31 set means the pass over small classes, clear the
/// pass over large ones; the low bits are the region to look at next plus one, zero meaning the
/// top of the heap.
const SMALL_PASS: u32 = 1 << 31;

/// The regions a pass covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pass {
    Large,
    Small,
}

impl Pass {
    fn covers(self, class: Class) -> bool {
        class.is_small() == (self == Pass::Small)
    }
}

impl<O: Os> Back<'_, O> {
    /// Counts `bytes` of freed slots as retained, and purges if that pushed the count past the
    /// cap.
    pub(super) fn retain(&self, bytes: usize) {
        let cap = retained_cap(self.space.plan().geometry());
        let old = self.state.retained.fetch_add(bytes, Relaxed);
        if old <= cap && old.saturating_add(bytes) > cap {
            self.purge_under(cap, PURGE_CAP_WORDS);
        }
    }

    /// Subtracts `bytes` of claimed resident slots from the count, stopping at zero.
    pub(super) fn unretain(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let retained = &self.state.retained;
        let mut seen = retained.load(Relaxed);
        while let Err(found) =
            retained.compare_exchange_weak(seen, seen.saturating_sub(bytes), Relaxed, Relaxed)
        {
            seen = found;
        }
    }

    /// The maintenance a slow path does on its way out: one step of purging, if retained memory
    /// is over the cap.
    pub fn heartbeat(&self) {
        let cap = retained_cap(self.space.plan().geometry());
        if self.retained() > cap {
            self.purge_under(cap, PURGE_STEP_WORDS);
        }
    }

    /// Returns every wholly free page the bitmaps hold to the OS, for `malloc_trim`. Slots
    /// cached in magazines are not in the bitmaps; launder them first. Returns the bytes
    /// purged that had been resident.
    pub fn trim(&self) -> usize {
        let counted = self.retained();
        let mut purged = 0;
        for pass in [Pass::Large, Pass::Small] {
            let plan = self.space.plan();
            for region in (0..plan.objects()).rev().filter_map(|i| plan.region(i)) {
                if let Some(class) = self.purgeable(pass, region) {
                    purged += self.purge_region(class, region, usize::MAX, false).0;
                }
            }
        }
        // What was counted when the trim began and is still resident is fragmentation; frees
        // counted since stay counted.
        self.unretain(counted.saturating_sub(purged));
        purged
    }

    /// Purges until retained memory is at most `target` or `budget` words of bitmap have been
    /// read.
    fn purge_under(&self, target: usize, budget: usize) {
        let mut spent = 0;
        while self.retained() > target && spent < budget {
            spent += self.purge_step(budget - spent);
        }
    }

    /// One step of the walk: works on the region at the walk's position, and moves the position
    /// on if that region's pass is done. Returns the words of bitmap read, at least one.
    fn purge_step(&self, budget: usize) -> usize {
        let plan = self.space.plan();
        let seen = self.state.purge_at.load(Relaxed);
        let pass = if seen & SMALL_PASS != 0 {
            Pass::Small
        } else {
            Pass::Large
        };
        let next = (seen & !SMALL_PASS) as usize;
        let index = if next == 0 || next > plan.objects() {
            plan.objects() - 1
        } else {
            next - 1
        };
        let Some(region) = plan.region(index) else {
            return 1;
        };
        let (purged, read, done) = match self.purgeable(pass, region) {
            Some(class) => self.purge_region(class, region, budget, true),
            None => (0, 1, true),
        };
        if purged > 0 {
            self.state.pass_progress.fetch_add(purged, Relaxed);
        }
        if done {
            let moved = if index > 0 {
                index as u32 | if pass == Pass::Small { SMALL_PASS } else { 0 }
            } else if pass == Pass::Large {
                SMALL_PASS
            } else {
                0
            };
            if self
                .state
                .purge_at
                .compare_exchange(seen, moved, Relaxed, Relaxed)
                .is_ok()
                && moved == 0
                && self.state.pass_progress.swap(0, Relaxed) == 0
            {
                // A whole pass purged nothing, and the OS refused nothing: what the count held
                // is fragmentation. Frees counted since this load stay counted.
                self.unretain(self.retained());
            }
        }
        read.max(1)
    }

    /// The class of a region this pass should purge: one of the pass's classes, with its summary
    /// bit set.
    fn purgeable(&self, pass: Pass, region: RegionId) -> Option<Class> {
        let RegionTag::Class(class) = self.state.table.get(region) else {
            return None;
        };
        if !pass.covers(class) {
            return None;
        }
        let host = self.host(class)?;
        let index = region.index() as u32;
        let bit = 1u64 << (index % BITS);
        if host.class_word(index / BITS).load() & bit == 0 {
            return None;
        }
        Some(class)
    }

    /// Purges units of a region from the top down until the region is done or `budget` words
    /// have been read. With `resume`, starts at the region's cursor and keeps it up to date;
    /// without, walks the whole region and leaves the cursor alone. Returns the bytes purged,
    /// the words read, and whether the walk reached the bottom.
    fn purge_region(
        &self,
        class: Class,
        region: RegionId,
        budget: usize,
        resume: bool,
    ) -> (usize, usize, bool) {
        let Some(host) = self.host(class) else {
            return (0, 1, true);
        };
        let layout = *host.layout();
        let hints = host.meta(region, layout.hints_word());
        let grow = host.meta(region, layout.grow_word());
        let mut cursor = if resume {
            Hints::unpack(hints.load()).cursor
        } else {
            0
        };
        let (mut purged, mut read) = (0, 0);
        let done = loop {
            if read >= budget {
                break false;
            }
            let carved = super::layout::Grow::unpack(grow.load()).carved;
            let huge_from = Hints::unpack(hints.load()).huge_from;
            let next = match layout.unit_below(carved, huge_from, cursor) {
                Step::Done => 0,
                Step::Skip(page) => {
                    read += 1;
                    page
                }
                Step::Unit(unit) => {
                    let (outcome, words) = region::purge_unit(&host, region, unit);
                    read += words;
                    match outcome {
                        Purge::Done(bytes) => purged += bytes,
                        // A refusal is not fragmentation: keep this pass from resetting the
                        // count, so the cap still applies once the OS relents.
                        Purge::Failed => {
                            self.state.pass_progress.fetch_add(1, Relaxed);
                        }
                        Purge::Clean | Purge::Busy => {}
                    }
                    unit.start
                }
            };
            if resume {
                set_cursor(hints, cursor, next);
            }
            if next == 0 {
                break true;
            }
            cursor = next;
        };
        self.unretain(purged);
        (purged, read, done)
    }
}

/// Moves a region's purge cursor from `from` to `to`, unless another purger has moved it.
fn set_cursor<W: Word>(hints: &W, from: u32, to: u32) {
    let mut seen = hints.load();
    loop {
        let now = Hints::unpack(seen);
        if now.cursor != from {
            return;
        }
        let next = Hints { cursor: to, ..now };
        match hints.compare_exchange(seen, next.pack()) {
            Ok(_) => return,
            Err(found) => seen = found,
        }
    }
}
