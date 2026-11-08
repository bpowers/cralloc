//! The static shape of the heap: page and huge-page sizes, the units its rules are stated in, the
//! range of region sizes, how many regions, and the largest magazine.

use super::Plan;
use crate::back::policy::retained_cap;
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
/// regions of 4 GiB, and [`Geometry::of_kernel`] is the same for a kernel with larger pages. The
/// region size is not fixed: when the process cannot have that much
/// address space, [`crate::space::Space::reserve`] halves the region size, keeping the number of
/// regions, down to the geometry's floor. The size a reservation got is part of its [`Plan`].
/// The simulated OS uses smaller shapes so that a heap fits in ordinary memory, and in a Kani
/// harness. Every quantity is a power of two, so the arithmetic on addresses is shifts and
/// masks.
///
/// A rule about memory means the same on every kernel, so `back::policy` states it in two units
/// that are not the kernel's: [`Geometry::page_unit`] and [`Geometry::huge_unit`], 4 KiB and 2 MiB
/// on every real kernel. A simulated heap's units are its own page and huge page, so that it keeps
/// its scale: its cap is 32 of its huge pages, whatever they are. A rule about what the kernel
/// does per page, or about the kernel's huge page itself, uses [`Geometry::page_size`] and
/// [`Geometry::huge_page_size`].
///
/// A plan whose regions are smaller than a class hosts that class only as runs of regions
/// ([`Plan::fits_region`]); a plan whose regions are smaller than the largest small class hosts
/// only the small classes that fit ([`Plan::hosts_every_small_class`]). The second is allowed for
/// proofs, which need heaps of a few kilobytes. A plan uses no huge pages
/// ([`Geometry::huge_pages_usable`]) if its regions are smaller than a huge page, since the
/// 32 MiB or 512 MiB huge page of a kernel with large pages must not set how small a heap can be,
/// or if a huge page is larger than the retained cap, which it could not then be held to.
///
/// The constructor checks what the rest of the crate relies on:
///
/// - regions are at most 4 GiB, so an offset within one is a `u32`;
/// - pages are at least 64 bytes; pages divide huge pages, huge pages divide the largest regions,
///   and pages divide metadata blocks, so every one of them can be committed and purged on its
///   own;
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
    /// `log2` of [`Geometry::page_unit`].
    page_unit_shift: u8,
    /// `log2` of [`Geometry::huge_unit`].
    huge_unit_shift: u8,
    max_region_shift: u8,
    min_region_shift: u8,
    max_regions: u16,
    magazine_regions: u16,
    max_objects: u16,
    max_magazine: MagazineSize,
    /// Whether the kernel's huge pages may be asked for: see [`Geometry::in_regions_of`].
    huge_pages_usable: bool,
    /// Whether large blocks stagger by whole pages as well as by lines: see
    /// [`Geometry::staggers_pages`].
    staggers_pages: bool,
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
    ///
    /// This is x86-64's geometry. Its blocks stagger by lines alone, which is all that has been
    /// measured there; [`Geometry::of_kernel`]'s stagger by pages too.
    pub const DESIGN: Geometry = Geometry {
        staggers_pages: false,
        ..Geometry::of_kernel(12)
    };

    /// The geometry for an aarch64 kernel whose pages are `2^page_shift` bytes: the design's, with
    /// that kernel's huge pages. A page table is a page of 8-byte entries, so a huge page, one
    /// table's worth of pages, is `page^2 / 8` bytes: 2 MiB, 32 MiB or 512 MiB for pages of
    /// 4 KiB, 16 KiB or 64 KiB. Regions keep the design's floor whatever the page size, so that a
    /// process's limit on address space means the same on every kernel; a page of 64 KiB is the
    /// largest whose metadata blocks still divide into pages there.
    pub const fn of_kernel(page_shift: u32) -> Geometry {
        Geometry::new(
            page_shift,
            2 * page_shift - 3,
            32,
            21,
            MAX_REGIONS,
            32,
            MagazineSize::MAX,
        )
        .with_kernel_rules()
    }

    /// This shape under a real kernel's rules, for a simulated kernel: units of 4 KiB and 2 MiB
    /// whatever its pages are, and blocks that stagger by pages.
    pub const fn with_kernel_rules(self) -> Geometry {
        Geometry {
            page_unit_shift: 12,
            huge_unit_shift: 21,
            staggers_pages: true,
            ..self
        }
    }

    /// This shape with blocks that stagger by pages as a real kernel's do, for a test's heap that
    /// keeps its own units.
    #[cfg(test)]
    pub const fn with_page_stagger(self) -> Geometry {
        Geometry {
            staggers_pages: true,
            ..self
        }
    }

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
            && huge_page_shift <= max_region_shift
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
            page_unit_shift: page_shift as u8,
            huge_unit_shift: huge_page_shift as u8,
            max_region_shift: max_region_shift as u8,
            min_region_shift: min_region_shift as u8,
            max_regions: max_regions as u16,
            magazine_regions: magazine_regions as u16,
            max_objects: objects as u16,
            max_magazine,
            huge_pages_usable: true,
            staggers_pages: false,
        })
    }

    /// This geometry as a plan with regions of `2^region_shift` bytes uses it: the one switch for
    /// huge pages. A huge page cannot be used if it does not fit in such a region, and is not used
    /// if it is larger than the retained cap (`policy::retained_cap`): a purge of one whole page
    /// could not be held to the cap, and a fault of one stalls for milliseconds. Then the heap asks
    /// the kernel for none, and the size it takes for a huge page is the region's, so that every
    /// rule stated in huge pages finds none in a region. Where no plan has such regions and the
    /// huge page is within the cap, this folds to `self`, whatever the shift.
    #[inline(always)]
    pub(super) const fn in_regions_of(self, region_shift: u8) -> Geometry {
        let fits =
            self.huge_page_shift <= self.min_region_shift || self.huge_page_shift <= region_shift;
        if fits && self.huge_page_size() <= retained_cap(self) {
            return self;
        }
        Geometry {
            huge_page_shift: region_shift,
            huge_pages_usable: false,
            ..self
        }
    }

    /// Whether the heap may ask the kernel for huge pages: not where they do not fit in a
    /// region, or are over the retained cap ([`Geometry::in_regions_of`]).
    #[inline(always)]
    pub const fn huge_pages_usable(self) -> bool {
        self.huge_pages_usable
    }

    /// Whether a large block's stagger has a term in whole pages, on top of the one in lines
    /// (`policy::stagger_pages`): on every real kernel's geometry but x86-64's, and on a simulated
    /// kernel's.
    #[inline(always)]
    pub const fn staggers_pages(self) -> bool {
        self.staggers_pages
    }

    /// Bytes per page: the unit of commit and purge.
    #[inline(always)]
    pub const fn page_size(self) -> usize {
        1 << self.page_shift
    }

    /// The unit of the rules about small amounts of memory: 4 KiB on every real kernel, whatever
    /// its pages are, and a simulated heap's own page.
    #[inline(always)]
    pub const fn page_unit(self) -> usize {
        1 << self.page_unit_shift
    }

    /// The unit of the rules about large amounts of memory: 2 MiB on every real kernel, whatever
    /// its huge pages are, and a simulated heap's own huge page.
    #[inline(always)]
    pub const fn huge_unit(self) -> usize {
        1 << self.huge_unit_shift
    }

    /// Bytes per transparent huge page: the unit of purge where huge pages are wanted. In a plan
    /// that uses none, a region's size ([`Geometry::in_regions_of`]).
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

    /// The kernels aarch64 has: each has the design's regions, floor and units, and its own huge
    /// page. x86-64's differs from aarch64's 4 KiB kernel only in the stagger.
    #[test]
    fn every_kernels_geometry_has_its_huge_page_and_the_designs_regions() {
        assert_eq!(
            Geometry {
                staggers_pages: true,
                ..Geometry::DESIGN
            },
            Geometry::of_kernel(12)
        );
        assert!(!Geometry::DESIGN.staggers_pages());
        for (shift, huge) in [(12, 2 << 20), (14, 32 << 20), (16, 512 << 20)] {
            let g = Geometry::of_kernel(shift);
            assert_eq!(g.page_size(), 1 << shift);
            assert_eq!(g.huge_page_size(), huge);
            assert_eq!(g.page_unit(), 4 << 10);
            assert_eq!(g.huge_unit(), 2 << 20);
            assert!(g.staggers_pages());
            assert_eq!(g.max_objects(), Geometry::DESIGN.max_objects());
            assert_eq!(g.largest_plan().region_size(), 4 << 30);
            let floor = g.smallest_plan();
            assert_eq!(floor.region_size(), 2 << 20);
            assert!(
                floor.size() < 9 << 30,
                "the floor is about 8 GiB on {shift}"
            );
            assert!(floor.hosts_every_small_class());
        }
    }

    /// A plan whose regions cannot hold a huge page uses none, and its huge page is the region;
    /// every plan that can hold one is untouched. Either way its rules' units do not change.
    #[test]
    fn a_plan_uses_huge_pages_only_where_they_fit_its_regions() {
        let g = Geometry::of_kernel(14);
        for shift in g.min_region_shift()..=g.max_region_shift() {
            let inside = g.plan(shift).unwrap().geometry();
            let fits = 1usize << shift >= g.huge_page_size();
            assert_eq!(inside.huge_pages_usable(), fits, "{shift}");
            assert_eq!(
                inside.huge_page_size(),
                g.huge_page_size().min(1 << shift),
                "{shift}"
            );
            assert_eq!(inside.page_size(), g.page_size());
            assert_eq!(inside.page_unit(), g.page_unit());
            assert_eq!(inside.huge_unit(), g.huge_unit());
        }
        for shift in Geometry::DESIGN.min_region_shift()..=Geometry::DESIGN.max_region_shift() {
            assert_eq!(
                Geometry::DESIGN.plan(shift).unwrap().geometry(),
                Geometry::DESIGN
            );
        }
    }

    /// A huge page over the retained cap is used in no plan, even one whose regions hold it, and
    /// a simulated heap's, being its unit, is always within its cap.
    #[test]
    fn a_huge_page_over_the_cap_is_used_in_no_plan() {
        let g = Geometry::of_kernel(16);
        assert!(g.huge_page_size() > retained_cap(g));
        for shift in g.min_region_shift()..=g.max_region_shift() {
            let inside = g.plan(shift).unwrap().geometry();
            assert!(!inside.huge_pages_usable(), "{shift}");
            assert_eq!(inside.huge_page_size(), 1 << shift);
        }
        let simulated = Geometry::new(16, 29, 32, 29, 16, 1, MagazineSize::MAX);
        assert!(simulated.largest_plan().geometry().huge_pages_usable());
    }

    /// A shape's units are its own unless it asks for a kernel's.
    #[test]
    fn a_simulated_heaps_units_are_its_own() {
        let g = Geometry::new(12, 16, 20, 17, 256, 1, MagazineSize::MAX);
        assert_eq!((g.page_unit(), g.huge_unit()), (4 << 10, 64 << 10));
        assert!(!g.staggers_pages());
        let g = g.with_kernel_rules();
        assert_eq!((g.page_unit(), g.huge_unit()), (4 << 10, 2 << 20));
        assert!(g.staggers_pages());
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
        // Huge pages bigger than the largest region.
        assert!(Geometry::try_new(12, 21, 20, 20, 4096, 1, max).is_none());
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
