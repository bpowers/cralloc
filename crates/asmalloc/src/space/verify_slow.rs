//! The address-space properties of `verify.rs` for every geometry `Geometry::try_new` accepts,
//! the slot count, which divides, and the two properties that take several seconds even on the
//! geometries in use. Too slow for the merge gate; run on demand with
//! `scripts/kani-all verify_slow`.

use super::Geometry;
use super::verify::*;
use crate::classes::MagazineSize;

fn any_geometry() -> Geometry {
    let ordinal: usize = kani::any();
    kani::assume(ordinal < MagazineSize::COUNT);
    let geometry = Geometry::try_new(
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        MagazineSize::from_ordinal(ordinal).unwrap(),
    );
    kani::assume(geometry.is_some());
    geometry.unwrap()
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
fn every_plan_fits_its_regions_in_every_geometry() {
    every_plan_fits_its_regions(any_geometry());
}

#[kani::proof]
fn plans_keep_their_parts_apart_in_every_geometry() {
    plans_keep_their_parts_apart(any_geometry());
}

#[kani::proof]
fn locate_inverts_region_offsets_in_every_geometry() {
    locate_inverts_region_offsets(any_geometry());
}

#[kani::proof]
fn slots_lie_inside_their_region_and_are_aligned_in_every_geometry() {
    slots_lie_inside_their_region_and_are_aligned(any_geometry());
}

#[kani::proof]
fn published_words_round_trip_in_every_geometry() {
    published_words_round_trip(any_geometry());
}

/// `slots_per_region` counts exactly the slots that fit past the class's origin, and
/// `slot_fits` accepts exactly them. The origin is whole pages, a multiple of the class's
/// alignment, zero for a large class, and under a sixty-fourth of the region.
#[kani::proof]
fn slots_per_region_counts_the_slots_that_fit() {
    let geometry = any_geometry();
    let shift: u32 = kani::any();
    let plan = geometry.plan(shift);
    kani::assume(plan.is_some());
    let plan = plan.unwrap();
    let class = any_class();
    let slots = plan.slots_per_region(class);
    let size = class.size();
    let origin = plan.slot_origin(class);
    assert!(origin % geometry.page_size() == 0 && origin % class.align() == 0);
    assert!(class.is_small() || origin == 0);
    assert!(origin < plan.region_size() / 64 || origin == 0);
    assert!(origin + slots as usize * size <= plan.region_size());
    assert!(origin + (slots as usize + 1) * size > plan.region_size());
    assert!(plan.fits_region(class) == (slots >= 1));
    let index: u32 = kani::any();
    assert!(plan.slot_fits(class, index) == (index < slots));
}
