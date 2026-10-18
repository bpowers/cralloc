//! Where everything lives in a reservation, as offsets from its base.
//!
//! A reservation of `k` object regions, `m` magazine regions and regions of `2^s` bytes is laid
//! out as
//!
//! ```text
//! offset 0                region 0 .. region k-1     objects, one class per region
//! k << s                  m regions                  the magazine area
//! (k + m) << s            k metadata blocks          block r belongs to region r
//! after them              one page                   the process page, wiped on fork
//! ```
//!
//! The magazines, metadata and process page sit above the object regions, so the
//! class-of-region table entries for them stay unassigned and a pointer into them is never taken
//! for an object. Every boundary is aligned by construction (regions to the region size,
//! metadata blocks and the process page to a page), and every offset is shifts and adds of `k`,
//! `s` and the region number.

use super::{Geometry, MAX_REGIONS, METADATA_SHIFT};
use crate::classes::{Class, MAX_SMALL_SIZE};

/// A region number: less than [`MAX_REGIONS`], so it indexes the class-of-region table.
///
/// Only [`Plan::region`], [`Plan::regions`] and [`Plan::locate`] make one, so outside this module
/// a `RegionId` is always an object region of some plan, and every plan of a geometry has the
/// same object regions.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegionId(u16);

impl RegionId {
    /// The region with this number, if it is below [`MAX_REGIONS`].
    #[inline(always)]
    pub(super) const fn new(index: usize) -> Option<RegionId> {
        if index < MAX_REGIONS {
            Some(RegionId(index as u16))
        } else {
            None
        }
    }

    /// The region number.
    #[inline(always)]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Where a heap address falls: a region and a byte offset within it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Location {
    /// The object region.
    pub region: RegionId,
    /// The offset within the region. Regions are at most 4 GiB, so it fits.
    pub offset: u32,
}

impl Location {
    /// The index of the slot-sized piece of the region that holds this byte, counting from the
    /// region's base in steps of `class`'s size. No divide instruction. The piece may be the
    /// unused tail of the region; [`Plan::slot_at`] checks for that.
    #[inline(always)]
    pub const fn slot(self, class: Class) -> u32 {
        class.divisor().divide(self.offset)
    }
}

/// The layout of one reservation: a geometry and the region size it got.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    geometry: Geometry,
    region_shift: u8,
}

impl Plan {
    /// Made by [`Geometry::plan`], which checks the shift.
    pub(super) const fn new(geometry: Geometry, region_shift: u8) -> Plan {
        Plan {
            geometry,
            region_shift,
        }
    }

    /// The geometry.
    #[inline(always)]
    pub const fn geometry(self) -> Geometry {
        self.geometry
    }

    /// `log2` of the region size: the shift that turns a heap offset into a region number.
    #[inline(always)]
    pub const fn region_shift(self) -> u32 {
        self.region_shift as u32
    }

    /// Bytes per region. Regions are aligned to their size.
    #[inline(always)]
    pub const fn region_size(self) -> usize {
        1 << self.region_shift
    }

    /// The number of object regions.
    #[inline(always)]
    pub const fn objects(self) -> usize {
        self.geometry.max_objects()
    }

    /// Object region `index`, if there is one.
    #[inline(always)]
    pub const fn region(self, index: usize) -> Option<RegionId> {
        if index < self.objects() {
            RegionId::new(index)
        } else {
            None
        }
    }

    /// Every object region, lowest first.
    pub fn regions(self) -> impl Iterator<Item = RegionId> {
        (0..self.objects()).filter_map(move |index| self.region(index))
    }

    /// Whether a slot of this class fits in a region. Large classes that do not are served as
    /// runs of whole regions; at the design's largest region size every class fits.
    ///
    /// Alignment by construction stops at the region size: a run of regions is only
    /// region-aligned by default. A request whose alignment exceeds the region size must have
    /// its run's first region chosen so that the run start is aligned.
    #[inline(always)]
    pub const fn fits_region(self, class: Class) -> bool {
        class.size() <= self.region_size()
    }

    /// Whether every small class has a slot in a region, as at every region size of the
    /// design's geometry and the simulator's standard and native shapes.
    #[inline(always)]
    pub const fn hosts_every_small_class(self) -> bool {
        self.region_size() >= MAX_SMALL_SIZE
    }

    /// The number of slots of a class in one region, zero if it does not fit. This divides; it
    /// is meant for setup, not per-object paths.
    #[inline]
    pub const fn slots_per_region(self, class: Class) -> u32 {
        (self.region_size() / class.size()) as u32
    }

    /// The offset of a region's first byte.
    #[inline(always)]
    pub const fn region_offset(self, region: RegionId) -> usize {
        region.index() << self.region_shift
    }

    /// The offset of a slot: `index` slots of `class` into `region`.
    #[inline(always)]
    pub const fn slot_offset(self, region: RegionId, class: Class, index: u32) -> usize {
        self.region_offset(region) + class.slot_offset(index)
    }

    /// Where the object regions end and the magazine area begins.
    #[inline(always)]
    pub const fn objects_end(self) -> usize {
        self.objects() << self.region_shift
    }

    /// The offset of the magazine area, [`Plan::magazine_area_size`] bytes long and aligned to
    /// the region size, so every magazine can be aligned to its own size.
    #[inline(always)]
    pub const fn magazines_offset(self) -> usize {
        self.objects_end()
    }

    /// Bytes in the magazine area: a whole number of regions.
    #[inline(always)]
    pub const fn magazine_area_size(self) -> usize {
        self.geometry.magazine_regions() << self.region_shift
    }

    /// `log2` of [`Plan::metadata_size`].
    #[inline(always)]
    pub const fn metadata_shift(self) -> u32 {
        self.region_shift() - METADATA_SHIFT
    }

    /// Bytes of metadata per object region: `region_size >> METADATA_SHIFT`, a multiple of the
    /// page size.
    #[inline(always)]
    pub const fn metadata_size(self) -> usize {
        1 << self.metadata_shift()
    }

    /// The offset of a region's metadata block, [`Plan::metadata_size`] bytes long.
    #[inline(always)]
    pub const fn metadata_offset(self, region: RegionId) -> usize {
        self.metadata_start() + (region.index() << self.metadata_shift())
    }

    /// The offset of the first region's metadata block, where the metadata blocks begin.
    #[inline(always)]
    pub const fn metadata_start(self) -> usize {
        self.magazines_offset() + self.magazine_area_size()
    }

    /// Where the last metadata block ends.
    #[inline(always)]
    pub const fn metadata_end(self) -> usize {
        self.magazines_offset()
            + self.magazine_area_size()
            + (self.objects() << self.metadata_shift())
    }

    /// The offset of the process page: one page whose contents a forked child reads as zero, for
    /// the process generation and anything else that must not survive a fork.
    #[inline(always)]
    pub const fn process_page_offset(self) -> usize {
        self.metadata_end()
    }

    /// The length of the reservation.
    #[inline(always)]
    pub const fn size(self) -> usize {
        self.process_page_offset() + self.geometry.page_size()
    }

    /// Where an offset from the base falls, if it is inside an object region. `delta` is
    /// `address.wrapping_sub(base)`, so an address below the base is a huge offset and falls
    /// nowhere.
    #[inline(always)]
    pub const fn locate(self, delta: usize) -> Option<Location> {
        let index = delta >> self.region_shift;
        if index < self.objects() {
            Some(Location {
                region: RegionId(index as u16),
                offset: (delta & (self.region_size() - 1)) as u32,
            })
        } else {
            None
        }
    }

    /// The index of the slot of `class` that starts exactly at `location`, if there is one: the
    /// offset is a multiple of the class size and the whole slot fits in the region. This is the
    /// check a pointer from outside (C `free`, a checked build) must pass before it is trusted.
    #[inline(always)]
    pub const fn slot_at(self, location: Location, class: Class) -> Option<u32> {
        let index = location.slot(class);
        let start = class.slot_offset(index);
        if start == location.offset as usize && start + class.size() <= self.region_size() {
            Some(index)
        } else {
            None
        }
    }

    /// Whether slot `index` of `class` lies wholly inside a region.
    #[inline(always)]
    pub const fn slot_fits(self, class: Class, index: u32) -> bool {
        // In units of `2^shift` bytes: slot `index` ends at `(index + 1) * odd`. Counting in those
        // units keeps every term below 2^36, where `slot_offset` of a huge index would shift
        // bits out of the word.
        let (odd, shift) = class.factors();
        shift <= self.region_shift() && index as usize * odd + odd <= self.region_size() >> shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_design_plan_fits_in_sixteen_tebibytes() {
        let plan = Geometry::DESIGN.largest_plan();
        assert_eq!(plan.objects(), 3940);
        assert_eq!(plan.objects_end(), 3940 << 32);
        assert_eq!(plan.magazines_offset(), 3940 << 32);
        assert_eq!(plan.metadata_offset(plan.region(0).unwrap()), 3972 << 32);
        assert_eq!(plan.metadata_end(), (3972 << 32) + 3940 * (128 << 20));
        assert_eq!(plan.process_page_offset(), plan.metadata_end());
        assert_eq!(plan.size(), plan.metadata_end() + 4096);
        assert!(plan.size() <= 16 << 40);
        assert_eq!(plan.regions().count(), 3940);
    }

    #[test]
    fn halving_the_region_size_halves_the_reservation_but_keeps_the_regions() {
        let g = Geometry::DESIGN;
        for shift in g.min_region_shift()..g.max_region_shift() {
            let small = g.plan(shift).unwrap();
            let large = g.plan(shift + 1).unwrap();
            assert_eq!(small.objects(), large.objects());
            assert_eq!(small.size() - 4096, (large.size() - 4096) / 2);
        }
        assert_eq!(g.plan(g.min_region_shift() - 1), None);
        assert_eq!(g.plan(33), None);
    }

    #[test]
    fn locate_finds_regions_and_offsets() {
        let plan = Geometry::DESIGN.plan(24).unwrap();
        let r2 = plan.region(2).unwrap();
        assert_eq!(plan.region(plan.objects()), None);
        assert_eq!(
            plan.locate(plan.region_offset(r2) + 40),
            Some(Location {
                region: r2,
                offset: 40
            })
        );
        assert_eq!(plan.locate(plan.objects_end()), None);
        assert_eq!(plan.locate(plan.metadata_offset(r2)), None);
        assert_eq!(plan.locate(plan.process_page_offset()), None);
        assert_eq!(plan.locate(0usize.wrapping_sub(8)), None);
    }

    #[test]
    fn slots_round_trip_through_locate() {
        for plan in [
            Geometry::DESIGN.largest_plan(),
            Geometry::DESIGN.smallest_plan(),
        ] {
            let region = plan.region(1).unwrap();
            for class in Class::all().filter(|&c| plan.fits_region(c)) {
                let slots = plan.slots_per_region(class);
                for index in [0, 1, slots / 2, slots - 1]
                    .into_iter()
                    .filter(|&i| i < slots)
                {
                    assert!(plan.slot_fits(class, index));
                    let location = plan.locate(plan.slot_offset(region, class, index)).unwrap();
                    assert_eq!(location.region, region);
                    assert_eq!(location.slot(class), index, "{class:?}");
                    assert_eq!(plan.slot_at(location, class), Some(index));
                }
                assert!(!plan.slot_fits(class, slots));
                let inside = plan.locate(plan.slot_offset(region, class, 0) + 1).unwrap();
                assert_eq!(plan.slot_at(inside, class), None);
            }
        }
    }

    #[test]
    fn the_unused_tail_of_a_region_is_no_slot() {
        // A 4 GiB region holds 178956970 slots of 24 bytes, ending 16 bytes short of the end; an
        // offset 16 bytes from the end is a multiple of 24 but not a slot.
        let plan = Geometry::DESIGN.largest_plan();
        let class = Class::of_size(24).unwrap();
        let tail = plan.locate((1 << 32) - 16).unwrap();
        assert_eq!(tail.slot(class), 178_956_970);
        assert_eq!(plan.slots_per_region(class), 178_956_970);
        assert_eq!(plan.slot_at(tail, class), None);
        let last = plan.locate((1 << 32) - 40).unwrap();
        assert_eq!(plan.slot_at(last, class), Some(178_956_969));
    }
}
