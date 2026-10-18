//! The back end as the depot's [`Source`].
//!
//! The depot moves pointers in `Option<NonNull<u8>>` and `NonNull<u8>` slices, which the back
//! end fills and launders as they are (`back::SlotPointer`).

use crate::back::Back;
use crate::classes::SmallClass;
use crate::depot::Source;
use crate::os::Os;
use core::ptr::NonNull;

impl<O: Os> Source for Back<'_, O> {
    fn fill(&self, class: SmallClass, out: &mut [Option<NonNull<u8>>]) -> usize {
        Back::fill(self, class, out)
    }

    fn take_back(&self, class: SmallClass, objects: &[NonNull<u8>]) {
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
