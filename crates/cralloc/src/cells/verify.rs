//! Kani: a table's lookup is in bounds for every index.

use super::Cells;

/// For every index, `get` gives the entry at the index modulo the length: in bounds, and the
/// one an array would give.
#[kani::proof]
fn every_index_names_an_entry() {
    let entries: [u16; 8] = kani::any();
    let cells = Cells::new(entries);
    let index: usize = kani::any();
    let entry = cells.get(index);
    let base = cells.0.get().cast::<u16>();
    let offset = (core::ptr::from_ref(entry).addr() - base.addr()) / size_of::<u16>();
    assert!(offset < 8);
    assert_eq!(offset, index % 8);
    assert_eq!(*entry, entries[index % 8]);
}
