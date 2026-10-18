//! The back end as the depot's [`Source`].
//!
//! The depot hands over a magazine's object slots, which the back end fills and launders in
//! place (`back::Descending`, `back::SlotPointer`).

use crate::back::{Back, Descending};
use crate::classes::SmallClass;
use crate::depot::Source;
use crate::os::Os;
use core::ptr::NonNull;
use core::sync::atomic::AtomicPtr;

impl<O: Os> Source for Back<'_, O> {
    fn fill(&self, class: SmallClass, slots: &[AtomicPtr<u8>]) -> usize {
        Back::fill(self, class, &mut Descending(slots))
    }

    fn take_back(&self, class: SmallClass, objects: &[AtomicPtr<u8>]) {
        // SAFETY: HEAP-SOURCE-01.
        unsafe { self.launder(class, objects) };
    }

    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.alloc(class.class(), class.size())
            .map(|block| block.ptr)
    }

    fn free_one(&self, class: SmallClass, object: NonNull<u8>) {
        self.free(object, class.class());
    }

    fn purge_step(&self, now: &dyn Fn() -> u64) {
        self.heartbeat_with(now);
    }

    fn purge_pass(&self, now: &dyn Fn() -> u64) {
        self.decay_pass(now);
    }
}
