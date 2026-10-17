//! Where everything lives in a reservation, as offsets from its base.
//!
//! A reservation of `k` object regions is laid out as
//!
//! ```text
//! offset 0                  region 0 .. region k-1      objects, one class per region
//! k * region_size           k metadata blocks           block r belongs to region r
//! (aligned up)              the magazine area
//! ```
//!
//! The metadata and the magazines sit above the object regions, so the class-of-region table
//! entries for them stay unassigned and a pointer into them is never taken for an object. All
//! offsets are shifts and adds of `k` and the region number.

use super::{Geometry, MAX_REGIONS};
use crate::classes::Class;

/// A region number: less than [`MAX_REGIONS`], so it indexes the class-of-region table.
///
/// A plan's object regions are `0..objects`; [`Plan::region`] and [`Plan::locate`] produce only
/// those.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegionId(u16);

impl RegionId {
    /// The region with this number, if it is below [`MAX_REGIONS`].
    #[inline(always)]
    pub const fn new(index: usize) -> Option<RegionId> {
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
    /// The index of the slot of `class` that holds this byte. No divide instruction.
    #[inline(always)]
    pub const fn slot(self, class: Class) -> u32 {
        class.divisor().divide(self.offset)
    }

    /// Whether this is the first byte of a slot of `class`, as every pointer the allocator hands
    /// out is.
    #[inline(always)]
    pub const fn is_slot_start(self, class: Class) -> bool {
        class.slot_offset(self.slot(class)) == self.offset as usize
    }
}

/// The layout of one reservation: a geometry and a number of object regions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Plan {
    geometry: Geometry,
    objects: u16,
}

impl Plan {
    /// Made by [`Geometry::plan`], which checks `objects`.
    pub(super) const fn new(geometry: Geometry, objects: u16) -> Plan {
        Plan { geometry, objects }
    }

    /// The geometry.
    #[inline(always)]
    pub const fn geometry(self) -> Geometry {
        self.geometry
    }

    /// The number of object regions.
    #[inline(always)]
    pub const fn objects(self) -> usize {
        self.objects as usize
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

    /// The offset of a region's first byte.
    #[inline(always)]
    pub const fn region_offset(self, region: RegionId) -> usize {
        region.index() << self.geometry.region_shift()
    }

    /// The offset of a slot: `index` slots of `class` into `region`.
    #[inline(always)]
    pub const fn slot_offset(self, region: RegionId, class: Class, index: u32) -> usize {
        self.region_offset(region) + class.slot_offset(index)
    }

    /// Where the object regions end and the metadata begins.
    #[inline(always)]
    pub const fn objects_end(self) -> usize {
        self.objects() << self.geometry.region_shift()
    }

    /// The offset of a region's metadata block, [`Geometry::metadata_size`] bytes long.
    #[inline(always)]
    pub const fn metadata_offset(self, region: RegionId) -> usize {
        self.objects_end() + (region.index() << self.geometry.metadata_shift())
    }

    /// Where the last metadata block ends.
    #[inline(always)]
    pub const fn metadata_end(self) -> usize {
        self.objects_end() + (self.objects() << self.geometry.metadata_shift())
    }

    /// The offset of the magazine area, [`Geometry::magazine_area_size`] bytes long and aligned
    /// to [`Geometry::magazine_area_align`].
    #[inline(always)]
    pub const fn magazines_offset(self) -> usize {
        let align = self.geometry.magazine_area_align();
        (self.metadata_end() + align - 1) & !(align - 1)
    }

    /// The length of the reservation.
    #[inline(always)]
    pub const fn size(self) -> usize {
        self.magazines_offset() + self.geometry.magazine_area_size()
    }

    /// Where an offset from the base falls, if it is inside an object region. `delta` is
    /// `address.wrapping_sub(base)`, so an address below the base is a huge offset and falls
    /// nowhere.
    #[inline(always)]
    pub const fn locate(self, delta: usize) -> Option<Location> {
        let index = delta >> self.geometry.region_shift();
        if index < self.objects() {
            Some(Location {
                region: RegionId(index as u16),
                offset: (delta & (self.geometry.region_size() - 1)) as u32,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_design_plan_fills_sixteen_tebibytes() {
        let plan = Geometry::DESIGN.largest_plan();
        assert_eq!(plan.objects(), 3970);
        assert_eq!(plan.objects_end(), 3970 << 32);
        assert_eq!(plan.metadata_end(), (3970 << 32) + 3970 * (128 << 20));
        assert_eq!(plan.magazines_offset(), plan.metadata_end());
        assert_eq!(plan.size(), plan.magazines_offset() + (4 << 30));
        assert!(plan.size() <= 16 << 40);
        assert_eq!(plan.regions().count(), 3970);
    }

    #[test]
    fn locate_finds_regions_and_offsets() {
        let plan = Geometry::DESIGN.plan(3).unwrap();
        let r2 = plan.region(2).unwrap();
        assert_eq!(plan.region(3), None);
        assert_eq!(
            plan.locate(plan.region_offset(r2) + 40),
            Some(Location {
                region: r2,
                offset: 40
            })
        );
        assert_eq!(plan.locate(plan.objects_end()), None);
        assert_eq!(plan.locate(plan.metadata_offset(r2)), None);
        assert_eq!(plan.locate(0usize.wrapping_sub(8)), None);
    }

    #[test]
    fn slots_round_trip_through_locate() {
        let plan = Geometry::DESIGN.plan(2).unwrap();
        let region = plan.region(1).unwrap();
        for class in Class::all() {
            let slots = Geometry::DESIGN.slots_per_region(class);
            for index in [0, 1, slots / 2, slots - 1]
                .into_iter()
                .filter(|&i| i < slots)
            {
                let location = plan.locate(plan.slot_offset(region, class, index)).unwrap();
                assert_eq!(location.region, region);
                assert_eq!(location.slot(class), index, "{class:?}");
                assert!(location.is_slot_start(class));
            }
            let inside = plan.locate(plan.slot_offset(region, class, 0) + 1).unwrap();
            assert!(!inside.is_slot_start(class));
        }
    }
}
