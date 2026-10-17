//! Kani proofs that a heap can start on the proof simulator cheaply enough for the merge gate,
//! which is what every bounded proof of the heap's logic builds on.

use super::{ProofOs, Tiny};
use crate::os::Os;
use crate::space::SpaceCell;

/// Start a `Tiny` heap the way the allocator does, through a `SpaceCell`, commit a metadata
/// page, write to any byte of it and read it back, purge it and see zero, then drop the cell,
/// which unmaps the reservation. Kani checks every memory access along the way.
#[kani::proof]
#[kani::unwind(3)]
fn a_tiny_heap_starts_and_its_memory_works() {
    let cell = SpaceCell::new(ProofOs::<Tiny>::new());
    let space = cell.get_or_reserve().unwrap();
    let page = <ProofOs<Tiny> as Os>::GEOMETRY.page_size();
    let meta = space.metadata(space.plan().region(0).unwrap());
    space.commit(meta, page).unwrap();
    let offset: usize = kani::any();
    kani::assume(offset < page);
    let byte = meta.map_addr(|a| a.saturating_add(offset)).as_ptr();
    // SAFETY: the page was just committed and nothing else uses it.
    unsafe {
        assert!(byte.read() == 0);
        byte.write(7);
        assert!(byte.read() == 7);
        space.purge(meta, page).unwrap();
        assert!(byte.read() == 0);
    }
}
