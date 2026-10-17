//! The address-space properties of `verify.rs` for every geometry `Geometry::try_new` accepts,
//! and the slot count, which divides. Too slow for the merge gate (sizing the largest plan of a
//! symbolic geometry divides); run on demand with `scripts/kani-all verify_slow`.

use super::Geometry;
use super::verify::*;

fn any_geometry() -> Geometry {
    let geometry = Geometry::try_new(
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
    );
    kani::assume(geometry.is_some());
    geometry.unwrap()
}

#[kani::proof]
fn the_largest_plan_is_the_largest_that_fits() {
    largest_plan_is_the_largest_that_fits(any_geometry());
}

#[kani::proof]
fn plans_keep_objects_metadata_and_magazines_apart_in_every_geometry() {
    plans_keep_objects_metadata_and_magazines_apart(any_geometry());
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

/// `slots_per_region` counts exactly the slots that fit.
#[kani::proof]
fn slots_per_region_counts_the_slots_that_fit() {
    let geometry = any_geometry();
    let class = any_class();
    let slots = geometry.slots_per_region(class) as usize;
    let size = class.size();
    assert!(slots * size <= geometry.region_size());
    assert!((slots + 1) * size > geometry.region_size());
    assert!(geometry.fits_region(class) == (slots >= 1));
}
