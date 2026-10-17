//! Kani proofs of the address-space arithmetic.
//!
//! Each property is a function of the geometry. The harnesses here run it on the geometries in
//! use (the design's and the simulator's two), which are constants and so verify in seconds;
//! `verify_slow.rs` runs the same properties on every geometry `Geometry::try_new` accepts.

use super::reservation::{pack, unpack};
use super::*;
use crate::classes::{COUNT, Class};
use crate::os::sim::{Shape, Standard, Tiny};

/// One of the geometries in use.
fn any_used_geometry() -> Geometry {
    match kani::any::<u8>() {
        0 => Geometry::DESIGN,
        1 => Standard::GEOMETRY,
        _ => Tiny::GEOMETRY,
    }
}

fn any_plan(geometry: Geometry) -> Plan {
    let objects: usize = kani::any();
    let plan = geometry.plan(objects);
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

/// The largest plan fits the geometry's maximum, one more object region would not, and plans
/// exist for exactly `1..=max_objects`. The count is below the region size, so it can share a
/// word with a region-aligned base.
pub(super) fn largest_plan_is_the_largest_that_fits(geometry: Geometry) {
    let max = geometry.max_objects();
    let plan = geometry.largest_plan();
    assert!(plan.objects() == max);
    assert!(geometry.plan(max) == Some(plan));
    assert!(geometry.plan(0).is_none() && geometry.plan(max + 1).is_none());
    assert!(plan.size() <= geometry.max_reservation());
    assert!(geometry.reservation_len(max + 1) > geometry.max_reservation());
    assert!(max >= 1 && max < geometry.region_size() && max <= MAX_REGIONS);
}

/// In every plan the object regions, the metadata blocks and the magazine area are disjoint and
/// in that order; each starts on a page and is whole pages; and the reservation spans at most
/// `max_regions` regions, so every address in it has a table index below `MAX_REGIONS`.
pub(super) fn plans_keep_objects_metadata_and_magazines_apart(geometry: Geometry) {
    let plan = any_plan(geometry);
    let region = any_object_region(plan);
    let page_mask = geometry.page_size() - 1;

    let region_start = plan.region_offset(region);
    assert!(region_start + geometry.region_size() <= plan.objects_end());

    let metadata = plan.metadata_offset(region);
    assert!(plan.objects_end() <= metadata);
    assert!(metadata + geometry.metadata_size() <= plan.metadata_end());
    assert!(metadata & page_mask == 0 && geometry.metadata_size() & page_mask == 0);
    if let Some(next) = plan.region(region.index() + 1) {
        assert!(metadata + geometry.metadata_size() == plan.metadata_offset(next));
    }

    let magazines = plan.magazines_offset();
    assert!(plan.metadata_end() <= magazines);
    assert!(magazines & (geometry.magazine_area_align() - 1) == 0);
    assert!(magazines & page_mask == 0 && geometry.magazine_area_size() & page_mask == 0);
    assert!(magazines + geometry.magazine_area_size() == plan.size());

    assert!(plan.size() <= geometry.max_reservation());
    assert!((plan.size() - 1) >> geometry.region_shift() < MAX_REGIONS);
    assert!(geometry.huge_page_size() & page_mask == 0);
    assert!(geometry.region_size() & (geometry.huge_page_size() - 1) == 0);
}

/// `locate` inverts region offsets: an offset is located exactly when it is inside an object
/// region, and the region and offset it gives add back up to it.
pub(super) fn locate_inverts_region_offsets(geometry: Geometry) {
    let plan = any_plan(geometry);
    let delta: usize = kani::any();
    match plan.locate(delta) {
        None => assert!(delta >= plan.objects_end()),
        Some(location) => {
            assert!(delta < plan.objects_end());
            assert!(location.region.index() < plan.objects());
            assert!((location.offset as usize) < geometry.region_size());
            assert!(plan.region_offset(location.region) + location.offset as usize == delta);
            // Whatever `slot_at` accepts is the start of a whole slot inside the region.
            let class = any_class();
            if let Some(index) = plan.slot_at(location, class) {
                assert!(class.slot_offset(index) == location.offset as usize);
                assert!(location.offset as usize + class.size() <= geometry.region_size());
            }
        }
    }
}

/// Every slot that fits in a region lies inside it, is found again by `locate` at the offset
/// `index * size`, and is aligned to its class's alignment or the region size, whichever is
/// smaller, given a region-aligned base. With the division proofs in `classes`, `slot` then
/// recovers the index.
pub(super) fn slots_lie_inside_their_region_and_are_aligned(geometry: Geometry) {
    let plan = any_plan(geometry);
    let region = any_object_region(plan);
    let class = any_class();
    let index: u32 = kani::any();
    let (odd, shift) = class.factors();
    // Slot `index` ends within the region: `(index + 1) * odd << shift <= region_size`.
    kani::assume(shift <= geometry.region_shift());
    kani::assume(index as usize * odd + odd <= geometry.region_size() >> shift);

    let offset = plan.slot_offset(region, class, index);
    assert!(offset + class.size() <= plan.region_offset(region) + geometry.region_size());
    let location = plan.locate(offset).unwrap();
    assert!(location.region == region);
    assert!(location.offset as usize == class.slot_offset(index));

    let align = if class.align() < geometry.region_size() {
        class.align()
    } else {
        geometry.region_size()
    };
    let base: usize = kani::any();
    kani::assume(base & (geometry.region_size() - 1) == 0);
    kani::assume(base <= usize::MAX - plan.size());
    assert!((base + offset) & (align - 1) == 0);
}

/// The published word gives back the base and count it was made from, and it is never zero, so
/// "nothing published" cannot be confused with a space.
pub(super) fn published_words_round_trip(geometry: Geometry) {
    let plan = any_plan(geometry);
    let base: usize = kani::any();
    kani::assume(base != 0 && base & (geometry.region_size() - 1) == 0);
    let word = pack(base, plan.objects());
    assert!(word != 0);
    assert!(unpack(geometry, word) == (base, plan.objects()));
}

#[kani::proof]
fn the_largest_used_plans_are_the_largest_that_fit() {
    largest_plan_is_the_largest_that_fits(any_used_geometry());
}

#[kani::proof]
fn used_plans_keep_objects_metadata_and_magazines_apart() {
    plans_keep_objects_metadata_and_magazines_apart(any_used_geometry());
}

#[kani::proof]
fn locate_inverts_region_offsets_in_used_geometries() {
    locate_inverts_region_offsets(any_used_geometry());
}

#[kani::proof]
fn slots_lie_inside_their_region_and_are_aligned_in_used_geometries() {
    slots_lie_inside_their_region_and_are_aligned(any_used_geometry());
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
