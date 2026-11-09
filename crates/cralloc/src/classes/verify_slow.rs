//! Kani proofs too slow for the merge gate, run on demand with `scripts/kani-all` or one at a time
//! with `scripts/kani`.

/// The fast paths' table lookup gives the class `Class::of_layout` computes for every layout of
/// at least one byte, and turns away exactly the zero sizes and the large requests.
#[kani::proof]
fn the_table_lookup_is_the_computed_class() {
    use super::{Class, SmallClass};
    use core::alloc::Layout;
    let size: usize = kani::any();
    let shift: u32 = kani::any();
    kani::assume(shift < usize::BITS);
    let layout = Layout::from_size_align(size, 1 << shift);
    kani::assume(layout.is_ok());
    let layout = layout.unwrap();
    let looked_up = SmallClass::of_nonzero_layout(layout);
    if size == 0 {
        assert!(looked_up.is_none());
    } else {
        assert!(looked_up == Class::of_layout(layout).and_then(Class::small));
    }
}
