//! Kani proofs of the class arithmetic for every input.
//!
//! Divisions by symbolic values are slow to verify, so where a property is about division it is
//! stated with multiplications and masks instead.

use super::*;

fn any_class() -> Class {
    let index: u8 = kani::any();
    kani::assume(index >= 1 && index as usize <= COUNT);
    Class::from_index(index).unwrap()
}

/// Any layout Rust can construct: a power-of-two alignment and a size that stays within
/// `isize::MAX` when rounded up to it.
fn any_layout() -> Layout {
    let size: usize = kani::any();
    let shift: u32 = kani::any();
    kani::assume(shift < usize::BITS);
    let layout = Layout::from_size_align(size, 1 << shift);
    kani::assume(layout.is_ok());
    layout.unwrap()
}

fn any_magazine() -> MagazineSize {
    let ordinal: usize = kani::any();
    kani::assume(ordinal < MagazineSize::COUNT);
    MagazineSize::from_ordinal(ordinal).unwrap()
}

/// The size a request is classed by: rounded up to the alignment, with zero counted as one.
fn rounded(size: usize, align: usize) -> usize {
    let size = if size == 0 { 1 } else { size };
    (size + align - 1) & !(align - 1)
}

/// The class of a size is the smallest class at least that large, and exists exactly up to
/// `MAX_SIZE`.
#[kani::proof]
fn every_size_has_the_smallest_class_that_fits() {
    let size: usize = kani::any();
    match Class::of_size(size) {
        None => assert!(size > MAX_SIZE),
        Some(class) => {
            assert!(size <= MAX_SIZE);
            assert!(class.size() >= size);
            if let Some(below) = Class::from_index(class.index() - 1) {
                assert!(below.size() < size);
            }
        }
    }
}

/// Every class size is `q << e` with `q` in {1, 3, 5, 7} and `e >= 3`, sizes strictly increase,
/// and a class size maps back to its class. The divisor and the alignment argument rest on the
/// first fact.
#[kani::proof]
fn class_sizes_are_odd_multiples_of_powers_of_two() {
    let class = any_class();
    let size = class.size();
    let e = size.trailing_zeros();
    let q = size >> e;
    assert!(e >= 3);
    assert!(q == 1 || q == 3 || q == 5 || q == 7);
    assert!(class.align() == 1 << e);
    assert!(Class::of_size(size) == Some(class));
    if let Some(next) = class.next() {
        assert!(next.size() > size);
    }
    assert!(size <= MAX_SIZE);
}

/// A larger request never gets a smaller class.
#[kani::proof]
fn the_class_function_is_monotone() {
    let small: usize = kani::any();
    let large: usize = kani::any();
    kani::assume(small <= large && large <= MAX_SIZE);
    assert!(Class::of_size(small).unwrap() <= Class::of_size(large).unwrap());
}

/// The `Allocator` fit rule: every size from a request to its class size has the same class.
#[kani::proof]
fn every_size_up_to_the_class_size_has_the_same_class() {
    let request: usize = kani::any();
    let other: usize = kani::any();
    kani::assume(request <= MAX_SIZE);
    let class = Class::of_size(request).unwrap();
    kani::assume(request <= other && other <= class.size());
    assert!(Class::of_size(other) == Some(class));
}

/// The fit rule for layouts: `deallocate` keeps the alignment and may pass any size from the
/// requested one to the class size.
#[kani::proof]
fn every_layout_up_to_the_class_size_has_the_same_class() {
    let layout = any_layout();
    let other: usize = kani::any();
    if let Some(class) = Class::of_layout(layout) {
        kani::assume(layout.size() <= other && other <= class.size());
        let wider = Layout::from_size_align(other, layout.align()).unwrap();
        assert!(Class::of_layout(wider) == Some(class));
    }
}

/// Alignment by construction: the class of a layout is a multiple of its alignment, so a slot is
/// aligned with no padding, for every alignment up to `MAX_SIZE` (4 GiB). Larger alignments, or
/// sizes that round past `MAX_SIZE`, have no class and take a run of regions.
#[kani::proof]
fn every_layout_gets_an_aligned_class_that_covers_it() {
    let layout = any_layout();
    let align = layout.align();
    let rounded = rounded(layout.size(), align);
    match Class::of_layout(layout) {
        None => assert!(rounded > MAX_SIZE),
        Some(class) => {
            assert!(class.size() >= layout.size());
            assert!(class.size() & (align - 1) == 0);
            assert!(class.align() >= align);
            assert!(class == Class::of_size(rounded).unwrap());
        }
    }
}

/// The C rule: as for layouts, and a block of 16 bytes or more is 16-byte aligned.
#[kani::proof]
fn every_c_request_gets_a_class_aligned_as_c_requires() {
    let layout = any_layout();
    let mut align = layout.align();
    if layout.size() >= C_ALIGN && align < C_ALIGN {
        align = C_ALIGN;
    }
    match Class::of_c(layout) {
        None => assert!(rounded(layout.size(), align) > MAX_SIZE),
        Some(class) => {
            assert!(class.size() >= layout.size());
            assert!(class.size() & (layout.align() - 1) == 0);
            if layout.size() >= C_ALIGN {
                assert!(class.size() & (C_ALIGN - 1) == 0);
            }
        }
    }
}

/// Every class size is `odd << shift` as `factors` says, its divisor is the multiplier for that
/// odd factor and that shift, which is what the division proofs take as given, and every slot
/// offset is a multiple of the class alignment.
#[kani::proof]
fn every_class_divides_by_its_odd_factor() {
    let class = any_class();
    let (odd, shift) = class.factors();
    assert!(odd << shift == class.size());
    let divisor = class.divisor();
    assert!(divisor.shift == shift && shift >= 3);
    assert!(odd == 1 || odd == 3 || odd == 5 || odd == 7);
    assert!(divisor.magic == divisor::MAGIC[odd >> 1]);
    let index: u32 = kani::any();
    assert!(class.slot_offset(index) & (class.align() - 1) == 0);
}

/// `divide` is `quotient` of the offset shifted right (by its definition), and the shifted
/// offset is below 2^29, the range over which `verify_slow.rs` proves `quotient` exact. For
/// `q = 1` the quotient is the shifted offset itself.
#[kani::proof]
fn the_shifted_offset_is_in_the_quotients_range() {
    let offset: u32 = kani::any();
    let shift: u32 = kani::any();
    kani::assume((3..=32).contains(&shift));
    let x = offset as u64 >> shift;
    assert!(x < 1 << 29);
    assert!(divisor::quotient(x as u32, divisor::MAGIC[0]) == x as u32);
}

/// The shift reduces division by a class size to division by its odd factor: any index that is
/// `x / q` for `x = offset >> shift` is `offset / (q << shift)`.
#[kani::proof]
#[kani::unwind(5)]
fn division_reduces_to_the_odd_factor() {
    let offset: u32 = kani::any();
    let shift: u32 = kani::any();
    kani::assume((3..=32).contains(&shift));
    let x = offset as u64 >> shift;
    for q in [1u64, 3, 5, 7] {
        let index: u64 = kani::any();
        if index <= x && index * q <= x && x < index * q + q {
            let below = (index * q) << shift;
            assert!(below <= offset as u64);
            assert!((offset as u64 - below) < q << shift);
        }
    }
}

/// The allocation test is "slot 0 or 1" and the free test is "slot 0 or the last", for every
/// magazine size, every magazine address and every slot.
#[kani::proof]
fn the_slot_tests_mean_empty_and_full() {
    let magazine = any_magazine();
    let base: usize = kani::any();
    let slot: usize = kani::any();
    kani::assume(base & (magazine.bytes() - 1) == 0);
    kani::assume(base <= usize::MAX - magazine.bytes());
    kani::assume(slot < magazine.slots());
    let word = base + slot * SLOT_BYTES;
    assert!(magazine.slot_of(word) == slot);
    assert!(magazine.alloc_slow(word) == (slot <= 1));
    assert!(magazine.free_slow(word) == (slot == 0 || slot == magazine.slots() - 1));
}

/// A never-written slab word and the drain marker send both operations to the slow path.
#[kani::proof]
fn zero_and_the_marker_fail_both_tests() {
    let magazine = any_magazine();
    assert!(magazine.alloc_slow(0) && magazine.free_slow(0));
    assert!(magazine.alloc_slow(MARKER) && magazine.free_slow(MARKER));
}

/// Small classes are exactly those up to `MAX_SMALL_SIZE`, and each one's magazine keeps within
/// the budget unless it is the smallest magazine.
#[kani::proof]
fn small_classes_have_magazines_within_budget() {
    let class = any_class();
    match class.small() {
        None => assert!(class.size() > MAX_SMALL_SIZE),
        Some(small) => {
            assert!(class.size() <= MAX_SMALL_SIZE);
            assert!(SmallClass::from_ordinal(small.ordinal()) == Some(small));
            let magazine = small.magazine();
            assert!(magazine.capacity() * small.size() <= MAGAZINE_BUDGET);
            if magazine < MagazineSize::MAX {
                let twice = MagazineSize::from_slots(2 * magazine.slots()).unwrap();
                assert!(twice.capacity() * small.size() > MAGAZINE_BUDGET);
            }
        }
    }
}
