//! The static shape of the heap: page and huge-page sizes, the range of region sizes, how many
//! regions, and the largest magazine.

use super::Plan;
use crate::classes::{MagazineSize, SmallClass};

/// The length of the class-of-region table, and so the most regions any geometry may have.
pub const MAX_REGIONS: usize = 4096;

/// A region's metadata block is `region_size >> METADATA_SHIFT` bytes: 128 MiB for a 4 GiB
/// region.
///
/// The back end's largest structure is the free bitmap of the smallest class, one bit per
/// 8-byte slot, which is `region_size / 64` bytes. The summary tree above it adds less than a
/// sixty-third of that. Twice the bitmap leaves the rest of the block for the summary, the zero
/// bitmap of large classes and per-region counters.
pub const METADATA_SHIFT: u32 = 5;

/// The shape of a heap.
///
/// The design's shape is [`Geometry::DESIGN`]: 4 KiB pages, 2 MiB huge pages, at most 4096
/// regions of 4 GiB. The region size is not fixed: when the process cannot have that much
/// address space, [`crate::space::Space::reserve`] halves the region size, keeping the number of
/// regions, down to the geometry's floor. The size a reservation got is part of its [`Plan`].
/// The simulated OS uses smaller shapes so that a heap fits in ordinary memory, and in a Kani
/// harness. Every quantity is a power of two, so the arithmetic on addresses is shifts and
/// masks.
///
/// A plan whose regions are smaller than a class hosts that class only as runs of regions
/// ([`Plan::fits_region`]); a plan whose regions are smaller than the largest small class hosts
/// only the small classes that fit ([`Plan::hosts_every_small_class`]). The second is allowed for
/// proofs, which need heaps of a few kilobytes.
///
/// The constructor checks what the rest of the crate relies on:
///
/// - regions are at most 4 GiB, so an offset within one is a `u32`;
/// - pages are at least 64 bytes; pages divide huge pages, huge pages divide regions at every
///   size the geometry allows, and pages divide metadata blocks, so every one of them can be
///   committed and purged on its own;
/// - a region at the smallest region size is at least as large as the largest magazine, so the
///   magazine area, a whole number of regions aligned to the region size, holds one and aligns
///   every magazine to its own size, which the fast paths' slot tests need;
/// - at least one object region fits next to the magazines and metadata, and the whole
///   reservation spans at most `max_regions` regions, so every address in it has an entry in the
///   class-of-region table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    page_shift: u8,
    huge_page_shift: u8,
    max_region_shift: u8,
    min_region_shift: u8,
    max_regions: u16,
    magazine_regions: u16,
    max_objects: u16,
    max_magazine: MagazineSize,
}

impl Geometry {
    /// The design's geometry: 4 KiB pages, 2 MiB huge pages, regions of 4 GiB down to 2 MiB,
    /// at most 4096 regions, 32 regions of magazines, and magazines up to the largest size.
    ///
    /// The floor is one huge page per region: there the 3940 object regions and their metadata
    /// span about 8 GiB.
    ///
    /// The magazine area shrinks with the regions, so it is sized for the floor: 32 regions are
    /// 64 MiB there (128 GiB at the top, more than any process will use). When it is used up,
    /// the slow path finds no magazine to make and serves the request from the back end
    /// directly, so running out costs speed, never an allocation.
    pub const DESIGN: Geometry = Geometry::new(12, 21, 32, 21, MAX_REGIONS, 32, MagazineSize::MAX);

    /// A geometry with pages of `2^page_shift` bytes, huge pages of `2^huge_page_shift`, regions
    /// of `2^max_region_shift` down to `2^min_region_shift`, at most `max_regions` regions in
    /// all, `magazine_regions` of them for magazines, and magazines of at most `max_magazine`.
    /// Panics, at compile time in a constant, if the shape breaks a rule in the type's
    /// documentation.
    pub const fn new(
        page_shift: u32,
        huge_page_shift: u32,
        max_region_shift: u32,
        min_region_shift: u32,
        max_regions: usize,
        magazine_regions: usize,
        max_magazine: MagazineSize,
    ) -> Geometry {
        match Geometry::try_new(
            page_shift,
            huge_page_shift,
            max_region_shift,
            min_region_shift,
            max_regions,
            magazine_regions,
            max_magazine,
        ) {
            Some(geometry) => geometry,
            None => panic!("invalid heap geometry"),
        }
    }

    /// As [`Geometry::new`], but `None` for a shape that breaks a rule.
    pub const fn try_new(
        page_shift: u32,
        huge_page_shift: u32,
        max_region_shift: u32,
        min_region_shift: u32,
        max_regions: usize,
        magazine_regions: usize,
        max_magazine: MagazineSize,
    ) -> Option<Geometry> {
        // The order matters: the first tests bound the shifts so the later ones cannot overflow.
        let valid = max_region_shift <= 32
            && page_shift >= 6
            && page_shift <= huge_page_shift
            && huge_page_shift <= min_region_shift
            && min_region_shift <= max_region_shift
            && page_shift + METADATA_SHIFT <= min_region_shift
            && magazine_regions >= 1
            && max_regions <= MAX_REGIONS
            && max_regions > magazine_regions
            && (1usize << min_region_shift) >= max_magazine.bytes();
        if !valid {
            return None;
        }
        // Object regions `k`, magazine regions `m`, `k` metadata blocks of 1/32 region and one
        // page, which is at most another 1/32 region, must span at most `max_regions` regions:
        // `32 (k + m) + k + 1 <= 32 max_regions`.
        let objects = (32 * (max_regions - magazine_regions) - 1) / 33;
        if objects == 0 {
            return None;
        }
        Some(Geometry {
            page_shift: page_shift as u8,
            huge_page_shift: huge_page_shift as u8,
            max_region_shift: max_region_shift as u8,
            min_region_shift: min_region_shift as u8,
            max_regions: max_regions as u16,
            magazine_regions: magazine_regions as u16,
            max_objects: objects as u16,
            max_magazine,
        })
    }

    /// Bytes per page: the unit of commit and purge.
    #[inline(always)]
    pub const fn page_size(self) -> usize {
        1 << self.page_shift
    }

    /// Bytes per transparent huge page: the unit of purge where huge pages are wanted.
    #[inline(always)]
    pub const fn huge_page_size(self) -> usize {
        1 << self.huge_page_shift
    }

    /// `log2` of the largest region size, which a reservation gets when the OS allows it.
    #[inline(always)]
    pub const fn max_region_shift(self) -> u32 {
        self.max_region_shift as u32
    }

    /// `log2` of the smallest region size. Below it the heap cannot start.
    #[inline(always)]
    pub const fn min_region_shift(self) -> u32 {
        self.min_region_shift as u32
    }

    /// The most regions a reservation may span, metadata and magazines included.
    #[inline(always)]
    pub const fn max_regions(self) -> usize {
        self.max_regions as usize
    }

    /// The number of object regions every plan has, whatever its region size.
    #[inline(always)]
    pub const fn max_objects(self) -> usize {
        self.max_objects as usize
    }

    /// The number of regions the magazine area takes.
    #[inline(always)]
    pub const fn magazine_regions(self) -> usize {
        self.magazine_regions as usize
    }

    /// The largest magazine this heap uses. The design's allows every size; the simulator's tiny
    /// shape caps it so that many magazines fit in its small magazine area.
    #[inline(always)]
    pub const fn max_magazine(self) -> MagazineSize {
        self.max_magazine
    }

    /// The magazine size of a small class in this heap: the class's own, capped by
    /// [`Geometry::max_magazine`].
    #[inline(always)]
    pub const fn magazine(self, class: SmallClass) -> MagazineSize {
        let own = class.magazine();
        if own.slots() > self.max_magazine.slots() {
            self.max_magazine
        } else {
            own
        }
    }

    /// The layout with regions of `2^region_shift` bytes, if the geometry allows that size.
    #[inline]
    pub const fn plan(self, region_shift: u32) -> Option<Plan> {
        if region_shift >= self.min_region_shift() && region_shift <= self.max_region_shift() {
            Some(Plan::new(self, region_shift as u8))
        } else {
            None
        }
    }

    /// The layout with the largest regions.
    #[inline]
    pub const fn largest_plan(self) -> Plan {
        Plan::new(self, self.max_region_shift)
    }

    /// The layout with the smallest regions.
    #[inline]
    pub const fn smallest_plan(self) -> Plan {
        Plan::new(self, self.min_region_shift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::Class;

    #[test]
    fn the_design_geometry_matches_the_design() {
        let g = Geometry::DESIGN;
        assert_eq!(g.page_size(), 4096);
        assert_eq!(g.huge_page_size(), 2 << 20);
        assert_eq!(g.max_objects(), 3940);
        let plan = g.largest_plan();
        assert_eq!(plan.region_size(), 4 << 30);
        assert_eq!(plan.metadata_size(), 128 << 20);
        assert_eq!(plan.magazine_area_size(), 128 << 30);
        assert!(plan.size() <= 16 << 40);
        assert_eq!(
            plan.slots_per_region(Class::SMALLEST) as usize,
            ((4 << 30) - plan.slot_origin(Class::SMALLEST)) / 8
        );
        assert!(Class::all().all(|c| plan.fits_region(c)));
        assert!(plan.hosts_every_small_class());
        let floor = g.smallest_plan();
        assert_eq!(floor.region_size(), 2 << 20);
        assert!(floor.size() < 9 << 30);
        assert!(
            floor.magazine_area_size() >= 64 << 20,
            "magazines at the floor"
        );
        assert!(floor.hosts_every_small_class());
    }

    #[test]
    fn the_cap_limits_magazines() {
        let capped = Geometry::new(6, 8, 11, 11, 4, 1, MagazineSize::MIN);
        for small in SmallClass::all() {
            assert_eq!(capped.magazine(small), MagazineSize::MIN);
            assert_eq!(Geometry::DESIGN.magazine(small), small.magazine());
        }
    }

    #[test]
    fn bad_shapes_are_refused() {
        let max = MagazineSize::MAX;
        assert!(Geometry::try_new(12, 21, 32, 21, 4096, 1, max).is_some());
        // Huge pages bigger than the smallest region.
        assert!(Geometry::try_new(12, 22, 32, 21, 4096, 1, max).is_none());
        // Regions bigger than 4 GiB.
        assert!(Geometry::try_new(12, 21, 33, 21, 2048, 1, max).is_none());
        // A floor above the largest size.
        assert!(Geometry::try_new(12, 21, 30, 31, 4096, 1, max).is_none());
        // Pages smaller than 64 bytes.
        assert!(Geometry::try_new(5, 5, 11, 11, 16, 1, max).is_none());
        // Metadata blocks smaller than a page at the floor.
        assert!(Geometry::try_new(12, 14, 20, 16, 16, 1, max).is_none());
        // More regions than the class table has entries.
        assert!(Geometry::try_new(12, 21, 32, 21, 4097, 1, max).is_none());
        // No room for an object region next to the magazines.
        assert!(Geometry::try_new(12, 21, 32, 21, 1, 1, max).is_none());
        // Regions smaller than the largest magazine (8 KiB): the magazine area is aligned only
        // to the region size, so a magazine in it would not be aligned to its own, however many
        // regions the area has.
        assert!(Geometry::try_new(6, 8, 12, 12, 32, 8, max).is_none());
        assert!(Geometry::try_new(6, 8, 13, 13, 16, 4, max).is_some());
        let quarter = MagazineSize::from_slots(256).unwrap();
        assert!(Geometry::try_new(6, 8, 11, 11, 4, 1, quarter).is_some());
    }

    #[test]
    fn every_valid_small_shape_spans_at_most_its_regions_at_every_size() {
        for page in 6..=12 {
            for floor in page + METADATA_SHIFT..=18 {
                for regions in [2, 3, 4, 7, 64, 256] {
                    for magazines in [1, 2, 16] {
                        let Some(g) = Geometry::try_new(
                            page,
                            page,
                            floor + 2,
                            floor,
                            regions,
                            magazines,
                            MagazineSize::MIN,
                        ) else {
                            continue;
                        };
                        for shift in floor..=floor + 2 {
                            let plan = g.plan(shift).unwrap();
                            assert!(plan.size() <= regions << shift, "{g:?} {shift}");
                        }
                    }
                }
            }
        }
    }
}
