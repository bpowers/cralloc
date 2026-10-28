//! What each region holds, as two readers may learn it.
//!
//! The region table (`space::RegionTable`) gives a region's tag: a class, a run, or nothing.
//! A class takes a region by tagging it and then naming it in its carving word
//! (`Back::assign`); if another thread named a region for the class first, the tag goes back
//! to unassigned, and the region can then go to another class or to a run. So a tag read from
//! the table is the region's for good only if the reader knows the region was named: it holds
//! a pointer into the region, which exists only once the region was named and carved.
//!
//! A walk that finds regions by their number (the purge, the hold, the tail's lowerings and
//! raises, the inspection) knows no such thing. Had it read the table, it could read a tag in
//! the window before it goes back, and act on the region with that class's layout after
//! another class has it: return its live slots, or read leaf words it never committed
//! (`tla/Assign.tla`, `Assign_table`; `back::tests::ledger_review`). So the walks read a
//! second byte, the published tag, which the thread whose carving swing named the region
//! writes after the swing, once: a class's region never goes back, so the class is the
//! region's for good. A region named but not yet published is skipped by the walks for that
//! moment, which costs nothing they cannot do on their next pass.
//!
//! [`Tags`] keeps both bytes private and offers each reader only what it may read: the walks
//! a region's published class ([`Tags::published`]); a holder of a pointer the table's tag
//! where it falls ([`Tags::at`]); and the assigners and runs the compare-and-swaps that move
//! regions between unassigned and a tag, and the run tags they follow. No method gives a
//! class from a region number but the published one.
//!
//! All zero is valid: every region unassigned and unpublished.

use super::Back;
use super::tree::Host;
use super::word::Word;
use crate::cells::Cells;
use crate::classes::Class;
use crate::lined::Lined;
use crate::os::Os;
use crate::space::{Location, MAX_REGIONS, RegionId, RegionTable, RegionTag};
use core::sync::atomic::{AtomicU8, Ordering};

/// The region table and the published tags.
pub(super) struct Tags {
    table: Lined<RegionTable>,
    published: Lined<Cells<AtomicU8, MAX_REGIONS>>,
}

/// A published byte that names no class.
const UNPUBLISHED: u8 = 0;

impl Tags {
    pub(super) const fn new() -> Tags {
        Tags {
            table: Lined::new(RegionTable::new()),
            published: Lined::new(Cells::new(
                [const { AtomicU8::new(UNPUBLISHED) }; MAX_REGIONS],
            )),
        }
    }

    fn published_byte(&self, region: RegionId) -> &AtomicU8 {
        // `RegionId` is below MAX_REGIONS: the table's reduction changes nothing.
        self.published.get(region.index())
    }

    /// The class a region was named for, if its naming has been published: for the walks.
    /// Acquire, so that the walk sees what the naming thread wrote before it published (the
    /// region's prepared metadata).
    #[inline]
    pub(super) fn published(&self, region: RegionId) -> Option<Class> {
        Class::from_index(self.published_byte(region).load(Ordering::Acquire))
    }

    /// Publishes `class` as the class of `region`, which a carving swing of that class has
    /// named. Written once: a second write, by a fork child's repair or by the namer after it,
    /// writes the same class and changes nothing.
    pub(super) fn publish(&self, region: RegionId, class: Class) {
        let _ = self.published_byte(region).compare_exchange(
            UNPUBLISHED,
            class.index(),
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    /// The table's tag at `location`, which must be where a pointer the caller holds falls
    /// (`Space::locate`): a pointer the program or this crate got from the region. A region is
    /// named before anything in it is handed out, so the tag is the region's for good. A caller
    /// with no such pointer reads [`Tags::published`] instead. (A location, not the pointer:
    /// the callers have located the pointer already, and returning both from here changed the
    /// code of `Back::lookup`.)
    #[inline(always)]
    pub(super) fn at(&self, location: Location) -> RegionTag {
        self.table.get(location.region)
    }

    /// Whether the table has `region` unassigned: for the assigners and runs, which look for
    /// one to take.
    #[inline]
    pub(super) fn is_unassigned(&self, region: RegionId) -> bool {
        self.table.get(region) == RegionTag::Unassigned
    }

    /// Takes an unassigned region for `tag`, one compare-and-swap: for `Back::assign` and the
    /// runs. `false` if it was not unassigned.
    #[inline]
    pub(super) fn take(&self, region: RegionId, tag: RegionTag) -> bool {
        self.table
            .replace(region, RegionTag::Unassigned, tag)
            .is_ok()
    }

    /// Gives back a region this thread took for `tag` and has not named (a class) or has
    /// emptied (a run).
    #[inline]
    pub(super) fn give_back(&self, region: RegionId, tag: RegionTag) {
        let _ = self.table.replace(region, tag, RegionTag::Unassigned);
    }

    /// Whether `region` is a run's head: for the inspection, which reports runs. A run's
    /// regions are read only for their sizes, kept out of the regions (`run_sizes`).
    #[inline]
    pub(super) fn is_run_head(&self, region: RegionId) -> bool {
        self.table.get(region) == RegionTag::RunHead
    }

    /// Whether `region` is a later region of a run: for the length of a run whose head the
    /// caller holds or reports.
    #[inline]
    pub(super) fn is_run_tail(&self, region: RegionId) -> bool {
        self.table.get(region) == RegionTag::RunTail
    }

    /// Clears a region's published byte, for tests that play a fork that cut a namer off
    /// before it published.
    #[cfg(test)]
    pub(super) fn unpublish(&self, region: RegionId) {
        self.published_byte(region)
            .store(UNPUBLISHED, Ordering::Release);
    }

    /// The table's tag, for tests.
    #[cfg(test)]
    pub(super) fn tag(&self, region: RegionId) -> RegionTag {
        self.table.get(region)
    }
}

impl<O: Os> Back<'_, O> {
    /// In a fork's child, which inherits the tags but not the threads: publishes each class
    /// region whose namer the fork may have cut off between its swing and its publication, so
    /// that the walks do not pass it by for good. A region the table gives a class and that is
    /// not published is one of three: named and not yet published, and then it is its class's
    /// carving region or, if the carving word has moved on, one carved from; tagged by a thread
    /// that has not yet swung the carving word, which no carving word names and nothing has
    /// carved from; or tagged by a thread the fork cut off before its swing, the same. Only
    /// the first is published. A thread of the child that names a region meanwhile publishes
    /// the same class, which changes nothing.
    pub fn publish_named(&self) {
        let plan = self.space.plan();
        let tags = &self.state.tags;
        for region in plan.regions().take(self.walk_regions()) {
            if tags.published(region).is_some() {
                continue;
            }
            if let RegionTag::Class(class) = tags.table.get(region) {
                self.publish_if_named(region, class);
            }
        }
    }

    /// [`Back::publish_named`] for one region, whose tag it read as `class`. Threads of the
    /// child may run meanwhile, so that read may be stale: a thread of the child's own that
    /// loses an assignment holds the tag, gives the region back, and another class names it.
    ///
    /// - The class's carving word naming the region settles it: only a swing of the class
    ///   that won for the region puts it there, and a named region is the class's for good.
    /// - Otherwise the region must have been carved from, which is read through `class`'s
    ///   host, perhaps not the region's. A nonzero `carved` there still means some class has
    ///   carved from the region: what a host's grow word aliases holds nothing nonzero in the
    ///   carved field until a class has carved. Two small classes share the region's header
    ///   in `State::headers`, written only by carves (a small class's assignment writes no
    ///   header). A large class's header is the first word of the region's block, which a
    ///   large class's loser writes with nothing carved, and its own carves after; a small
    ///   class's top summary word sits there, nonzero only once one of its slots, so one
    ///   carved, has been released. A region carved from was named and keeps its tag for
    ///   good, so the tag read again names that carver: it is published only if that is
    ///   still `class`.
    fn publish_if_named(&self, region: RegionId, class: Class) {
        let tags = &self.state.tags;
        let Some(host) = self.host(class) else {
            return;
        };
        let carving = host
            .class
            .carving
            .load(core::sync::atomic::Ordering::SeqCst);
        if carving == region.index() as u32 + 1 {
            tags.publish(region, class);
            return;
        }
        let carved = super::layout::Grow::unpack(host.grow(region).load()).carved;
        if carved != 0 && tags.table.get(region) == RegionTag::Class(class) {
            tags.publish(region, class);
        }
    }

    /// [`Back::publish_if_named`], for a test that plays a stale read of the tag.
    #[cfg(test)]
    pub(super) fn publish_if_read(&self, region: RegionId, class: Class) {
        self.publish_if_named(region, class);
    }
}
