//! The static shape of the heap: page, huge-page and region sizes, and how many regions.

use super::Plan;
use crate::classes::{Class, MAX_SMALL_SIZE, MagazineSize};

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
/// The design's shape is [`Geometry::DESIGN`]: 4 KiB pages, 2 MiB huge pages, 4 GiB regions, at
/// most 4096 of them, and 4 GiB of magazines. The simulated OS uses smaller ones so that a heap
/// fits in ordinary memory, and in a Kani harness. Every quantity is a power of two, so the
/// arithmetic on addresses is shifts and masks, and a geometry that is a constant folds away.
///
/// A geometry whose regions are smaller than [`MAX_SMALL_SIZE`] hosts only the classes that fit
/// in a region ([`Geometry::fits_region`]). That is allowed for proofs, which need heaps of a
/// few kilobytes; [`Geometry::hosts_every_small_class`] says whether a geometry is of that kind.
///
/// The constructor checks what the rest of the crate relies on:
///
/// - regions are at most 4 GiB, so an offset within one is a `u32`;
/// - pages are at least 64 bytes; pages divide huge pages, huge pages divide regions, and pages
///   divide metadata blocks and the magazine area, so every one of them can be committed and
///   purged on its own;
/// - the magazine area holds at least one of the largest magazines;
/// - a reservation with one object region fits in the maximum, and the number of object regions
///   is less than the region size, so a region-aligned base and the count share one word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    page_shift: u8,
    huge_page_shift: u8,
    region_shift: u8,
    magazine_shift: u8,
    max_regions: u16,
    max_objects: u16,
}

impl Geometry {
    /// The design's geometry.
    pub const DESIGN: Geometry = Geometry::new(12, 21, 32, MAX_REGIONS, 32);

    /// A geometry with pages of `2^page_shift` bytes, huge pages of `2^huge_page_shift`, regions
    /// of `2^region_shift`, at most `max_regions` regions in all, and a magazine area of
    /// `2^magazine_shift` bytes. Panics, at compile time in a constant, if the shape breaks a
    /// rule in the type's documentation.
    pub const fn new(
        page_shift: u32,
        huge_page_shift: u32,
        region_shift: u32,
        max_regions: usize,
        magazine_shift: u32,
    ) -> Geometry {
        match Geometry::try_new(
            page_shift,
            huge_page_shift,
            region_shift,
            max_regions,
            magazine_shift,
        ) {
            Some(geometry) => geometry,
            None => panic!("invalid heap geometry"),
        }
    }

    /// As [`Geometry::new`], but `None` for a shape that breaks a rule.
    pub const fn try_new(
        page_shift: u32,
        huge_page_shift: u32,
        region_shift: u32,
        max_regions: usize,
        magazine_shift: u32,
    ) -> Option<Geometry> {
        // The order matters: the first tests bound the shifts so the later ones cannot overflow.
        let valid = region_shift <= 32
            && magazine_shift <= 40
            && page_shift <= huge_page_shift
            && huge_page_shift <= region_shift
            && page_shift >= 6
            && page_shift + METADATA_SHIFT <= region_shift
            && page_shift <= magazine_shift
            && magazine_shift >= MagazineSize::MAX.bytes().trailing_zeros()
            && max_regions >= 1
            && max_regions <= MAX_REGIONS;
        if !valid {
            return None;
        }
        let mut geometry = Geometry {
            page_shift: page_shift as u8,
            huge_page_shift: huge_page_shift as u8,
            region_shift: region_shift as u8,
            magazine_shift: magazine_shift as u8,
            max_regions: max_regions as u16,
            max_objects: 0,
        };
        // Ignoring the alignment of the magazine area, `k` object regions need
        // `k * (region + metadata) + magazines` bytes. The alignment adds less than one
        // metadata-plus-region, so the estimate is at most one too many.
        let per_region = geometry.region_size() + geometry.metadata_size();
        let limit = geometry.max_reservation();
        if limit < per_region + geometry.magazine_area_size() {
            return None;
        }
        let mut objects = (limit - geometry.magazine_area_size()) / per_region;
        if geometry.reservation_len(objects) > limit {
            objects -= 1;
        }
        if objects == 0 || objects >= geometry.region_size() {
            return None;
        }
        geometry.max_objects = objects as u16;
        Some(geometry)
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

    /// The most regions a reservation may span, metadata and magazines included.
    #[inline(always)]
    pub const fn max_regions(self) -> usize {
        self.max_regions as usize
    }

    /// The largest reservation: `max_regions` regions.
    #[inline(always)]
    pub const fn max_reservation(self) -> usize {
        self.max_regions() << self.region_shift
    }

    /// The most object regions a reservation can have.
    #[inline(always)]
    pub const fn max_objects(self) -> usize {
        self.max_objects as usize
    }

    /// Bytes of metadata per object region: `region_size >> METADATA_SHIFT`. A multiple of the
    /// page size.
    #[inline(always)]
    pub const fn metadata_size(self) -> usize {
        1 << self.metadata_shift()
    }

    /// `log2` of [`Geometry::metadata_size`].
    #[inline(always)]
    pub const fn metadata_shift(self) -> u32 {
        self.region_shift() - METADATA_SHIFT
    }

    /// Bytes in the magazine area.
    #[inline(always)]
    pub const fn magazine_area_size(self) -> usize {
        1 << self.magazine_shift
    }

    /// The alignment of the magazine area: a page, or the largest magazine if that is bigger, so
    /// that every magazine can be aligned to its size and the area committed by pages.
    #[inline(always)]
    pub const fn magazine_area_align(self) -> usize {
        let largest = MagazineSize::MAX.bytes();
        if largest > self.page_size() {
            largest
        } else {
            self.page_size()
        }
    }

    /// The layout of a reservation with `objects` object regions, if the geometry allows that
    /// many.
    #[inline]
    pub const fn plan(self, objects: usize) -> Option<Plan> {
        if objects >= 1 && objects <= self.max_objects() {
            Some(Plan::new(self, objects as u16))
        } else {
            None
        }
    }

    /// The plan with the most object regions.
    #[inline]
    pub const fn largest_plan(self) -> Plan {
        Plan::new(self, self.max_objects)
    }

    /// The bytes a reservation with `objects` object regions spans. Used before a `Plan` exists,
    /// to size the largest one.
    pub(super) const fn reservation_len(self, objects: usize) -> usize {
        let metadata_end = (objects << self.region_shift) + (objects << self.metadata_shift());
        let align = self.magazine_area_align();
        let magazines = (metadata_end + align - 1) & !(align - 1);
        magazines + self.magazine_area_size()
    }

    /// Whether every small class has a slot in a region, as in the design's geometry and the
    /// simulator's standard one.
    #[inline(always)]
    pub const fn hosts_every_small_class(self) -> bool {
        self.region_size() >= MAX_SMALL_SIZE
    }

    /// Whether a slot of this class fits in a region. Large classes that do not are served as
    /// runs of whole regions; in the design's geometry every class fits.
    ///
    /// Alignment by construction stops at the region size: a run of regions is only
    /// region-aligned by default. A request whose alignment exceeds the region size (above
    /// 4 GiB in the design's geometry, above 64 KiB in the simulator's standard one) must have
    /// its run's first region chosen so that the run start is aligned.
    #[inline(always)]
    pub const fn fits_region(self, class: Class) -> bool {
        class.size() <= self.region_size()
    }

    /// The number of slots of a class in one region, zero if it does not fit. This divides; it
    /// is meant for constants and setup, not per-object paths.
    #[inline]
    pub const fn slots_per_region(self, class: Class) -> u32 {
        (self.region_size() / class.size()) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_design_geometry_matches_the_design() {
        let g = Geometry::DESIGN;
        assert_eq!(g.page_size(), 4096);
        assert_eq!(g.huge_page_size(), 2 << 20);
        assert_eq!(g.region_size(), 4 << 30);
        assert_eq!(g.max_reservation(), 16 << 40);
        assert_eq!(g.metadata_size(), 128 << 20);
        assert_eq!(g.magazine_area_size(), 4 << 30);
        assert_eq!(g.max_objects(), 3970);
        assert!(g.reservation_len(3970) <= g.max_reservation());
        assert!(g.reservation_len(3971) > g.max_reservation());
        assert_eq!(g.slots_per_region(Class::SMALLEST), 1 << 29);
        assert!(Class::all().all(|c| g.fits_region(c)));
        assert!(g.hosts_every_small_class());
    }

    #[test]
    fn slots_per_region_counts_the_slots_that_fit_in_every_used_geometry() {
        use crate::os::sim::{Shape, Standard, Tiny};
        for g in [Geometry::DESIGN, Standard::GEOMETRY, Tiny::GEOMETRY] {
            for class in Class::all() {
                let slots = g.slots_per_region(class) as usize;
                assert!(slots * class.size() <= g.region_size());
                assert!((slots + 1) * class.size() > g.region_size());
                assert_eq!(g.fits_region(class), slots >= 1);
            }
        }
    }

    #[test]
    fn bad_shapes_are_refused() {
        assert!(Geometry::try_new(12, 21, 32, 4096, 32).is_some());
        // Huge pages bigger than regions.
        assert!(Geometry::try_new(12, 33, 32, 4096, 32).is_none());
        // Regions bigger than 4 GiB.
        assert!(Geometry::try_new(12, 21, 33, 2048, 32).is_none());
        // Pages smaller than 64 bytes.
        assert!(Geometry::try_new(5, 5, 11, 16, 11).is_none());
        // Metadata blocks smaller than a page.
        assert!(Geometry::try_new(12, 14, 16, 16, 20).is_none());
        // More regions than the class table has entries.
        assert!(Geometry::try_new(12, 21, 32, 4097, 32).is_none());
        // No room for one object region next to the magazines.
        assert!(Geometry::try_new(12, 21, 32, 1, 32).is_none());
        // A magazine area smaller than the largest magazine.
        assert!(Geometry::try_new(10, 12, 15, 4, 10).is_none());
    }

    #[test]
    fn every_valid_small_shape_sizes_its_largest_plan_exactly() {
        for page in 6..=12 {
            for region in page + METADATA_SHIFT..=20 {
                for regions in [1, 2, 3, 4, 7, 64, 256] {
                    for magazines in page.max(11)..=region + 2 {
                        let Some(g) = Geometry::try_new(page, page, region, regions, magazines)
                        else {
                            continue;
                        };
                        let max = g.max_objects();
                        assert!(g.reservation_len(max) <= g.max_reservation(), "{g:?}");
                        assert!(g.reservation_len(max + 1) > g.max_reservation(), "{g:?}");
                    }
                }
            }
        }
    }
}
