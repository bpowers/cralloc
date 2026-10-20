//! Kani proofs of the address-space arithmetic.
//!
//! Each property is a function of the geometry and holds for every region size it allows. The
//! harnesses here run it on the geometries in use (the design's and the simulator's three), which
//! are constants and so verify in seconds; `verify_slow.rs` runs the same properties on every
//! geometry `Geometry::try_new` accepts, and the two that take several seconds even on the
//! geometries in use.

use super::reservation::{pack, unpack};
use super::*;
use crate::classes::{COUNT, Class};
use crate::os::sim::{Native, Shape, Standard, Tiny};

/// One of the geometries in use.
pub(super) fn any_used_geometry() -> Geometry {
    match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        2 => Native::GEOMETRY,
        _ => Tiny::GEOMETRY,
    }
}

/// A plan at any region size the geometry allows.
fn any_plan(geometry: Geometry) -> Plan {
    let shift: u32 = kani::any();
    let plan = geometry.plan(shift);
    kani::assume(plan.is_some());
    plan.unwrap()
}

fn any_object_region(plan: Plan) -> RegionId {
    let index: usize = kani::any();
    let region = plan.region(index);
    kani::assume(region.is_some());
    region.unwrap()
}

pub(super) fn any_class() -> Class {
    let index: u8 = kani::any();
    kani::assume(index >= 1 && index as usize <= COUNT);
    Class::from_index(index).unwrap()
}

/// Plans exist for exactly the region sizes from the floor to the largest, every one has the
/// same object regions, and every one spans at most `max_regions` regions of its own size, so
/// every address in a reservation has an entry in the class-of-region table.
pub(super) fn every_plan_fits_its_regions(geometry: Geometry) {
    let shift: u32 = kani::any();
    let plan = geometry.plan(shift);
    assert!(
        plan.is_some()
            == (geometry.min_region_shift() <= shift && shift <= geometry.max_region_shift())
    );
    if let Some(plan) = plan {
        assert!(plan.region_shift() == shift);
        assert!(plan.objects() == geometry.max_objects() && plan.objects() >= 1);
        assert!(plan.size() <= geometry.max_regions() << shift);
        assert!((plan.size() - 1) >> shift < MAX_REGIONS);
    }
    assert!(geometry.largest_plan().region_shift() == geometry.max_region_shift());
    assert!(geometry.smallest_plan().region_shift() == geometry.min_region_shift());
}

/// In every plan the object regions, the magazine area, the metadata blocks and the process page
/// are disjoint and in that order; each starts on a page and is whole pages; and the magazine
/// area is aligned to the region size, and a region is at least the largest magazine, so the
/// area aligns every magazine to its own size and holds one.
pub(super) fn plans_keep_their_parts_apart(geometry: Geometry) {
    let plan = any_plan(geometry);
    let region = any_object_region(plan);
    let page_mask = geometry.page_size() - 1;

    let region_start = plan.region_offset(region);
    assert!(region_start + plan.region_size() <= plan.objects_end());

    let magazines = plan.magazines_offset();
    assert!(plan.objects_end() <= magazines);
    assert!(magazines & (plan.region_size() - 1) == 0);
    assert!(plan.region_size() >= geometry.max_magazine().bytes());
    assert!(plan.magazine_area_size() >= geometry.max_magazine().bytes());

    let metadata = plan.metadata_offset(region);
    assert!(magazines + plan.magazine_area_size() <= metadata);
    assert!(metadata + plan.metadata_size() <= plan.metadata_end());
    assert!(metadata & page_mask == 0 && plan.metadata_size() & page_mask == 0);
    if let Some(next) = plan.region(region.index() + 1) {
        assert!(metadata + plan.metadata_size() == plan.metadata_offset(next));
    }

    let process = plan.process_page_offset();
    assert!(plan.metadata_end() <= process);
    assert!(process & page_mask == 0);
    assert!(process + geometry.page_size() == plan.size());

    assert!(geometry.huge_page_size() & page_mask == 0);
    assert!(plan.region_size() & (geometry.huge_page_size() - 1) == 0);
}

/// `locate` inverts region offsets: an offset is located exactly when it is inside an object
/// region, and the region and offset it gives add back up to it. Whatever `slot_at` accepts is
/// the start of a whole slot inside the region.
pub(super) fn locate_inverts_region_offsets(geometry: Geometry) {
    let plan = any_plan(geometry);
    let delta: usize = kani::any();
    match plan.locate(delta) {
        None => assert!(delta >= plan.objects_end()),
        Some(location) => {
            assert!(delta < plan.objects_end());
            assert!(location.region.index() < plan.objects());
            assert!((location.offset as usize) < plan.region_size());
            assert!(plan.region_offset(location.region) + location.offset as usize == delta);
            let class = any_class();
            if let Some(index) = plan.slot_at(location, class) {
                assert!(
                    plan.slot_origin(class) + class.slot_offset(index) == location.offset as usize
                );
                assert!(location.offset as usize + class.size() <= plan.region_size());
                assert!(plan.slot_fits(class, index));
            }
        }
    }
}

/// Every slot `slot_fits` accepts lies inside its region, is found again by `locate` at the
/// offset `origin + index * size`, and is aligned to its class's alignment or the region
/// size, whichever is smaller, given a region-aligned base. With the division proofs in
/// `classes`, `Plan::slot_at` then recovers the index: past the origin it is the search with
/// no origin (`plan::slot_from`).
pub(super) fn slots_lie_inside_their_region_and_are_aligned(geometry: Geometry) {
    let plan = any_plan(geometry);
    let region = any_object_region(plan);
    let class = any_class();
    let index: u32 = kani::any();
    kani::assume(plan.slot_fits(class, index));

    let offset = plan.slot_offset(region, class, index);
    assert!(offset + class.size() <= plan.region_offset(region) + plan.region_size());
    let location = plan.locate(offset).unwrap();
    assert!(location.region == region);
    assert!(location.offset as usize == plan.slot_origin(class) + class.slot_offset(index));

    let align = if class.align() < plan.region_size() {
        class.align()
    } else {
        plan.region_size()
    };
    let base: usize = kani::any();
    kani::assume(base & (plan.region_size() - 1) == 0);
    kani::assume(base <= usize::MAX - plan.size());
    assert!((base + offset) & (align - 1) == 0);
}

/// The published word gives back the base and region shift it was made from, and it is never
/// zero, so "nothing published" cannot be confused with a space.
pub(super) fn published_words_round_trip(geometry: Geometry) {
    let plan = any_plan(geometry);
    let base: usize = kani::any();
    kani::assume(base != 0 && base & (plan.region_size() - 1) == 0);
    let shift = plan.region_shift() as usize;
    let word = pack(base, shift);
    assert!(word != 0);
    assert!(unpack(word) == (base, shift));
}

#[kani::proof]
fn every_used_plan_fits_its_regions() {
    every_plan_fits_its_regions(any_used_geometry());
}

#[kani::proof]
fn used_plans_keep_their_parts_apart() {
    plans_keep_their_parts_apart(any_used_geometry());
}

#[kani::proof]
fn published_words_round_trip_in_used_geometries() {
    published_words_round_trip(any_used_geometry());
}

/// Region tags round-trip through their table byte, and a byte no tag makes reads as
/// unassigned.
#[kani::proof]
fn region_tags_round_trip() {
    let byte: u8 = kani::any();
    let tag = RegionTag::from_byte(byte);
    if tag != RegionTag::Unassigned {
        assert!(tag.to_byte() == byte);
    }
    let class = any_class();
    assert!(RegionTag::from_byte(RegionTag::Class(class).to_byte()) == RegionTag::Class(class));
    assert!(RegionTag::from_byte(RegionTag::Unassigned.to_byte()) == RegionTag::Unassigned);
}
