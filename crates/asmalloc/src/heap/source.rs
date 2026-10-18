//! The depot's [`Source`] over the back end.
//!
//! The depot moves pointers in `Option<NonNull<u8>>` and `NonNull<u8>` slices, which the back
//! end fills and launders as they are (`back::SlotPointer`).

use crate::back::Back;
use crate::classes::SmallClass;
use crate::depot::Source;
use crate::os::Os;
use core::ptr::NonNull;

/// The back end as the depot's source of objects.
pub(crate) struct BackSource<'a, O: Os>(pub(crate) Back<'a, O>);

impl<O: Os> Source for BackSource<'_, O> {
    fn fill(&self, class: SmallClass, out: &mut [Option<NonNull<u8>>]) -> usize {
        self.0.fill(class, out)
    }

    fn take_back(&self, class: SmallClass, objects: &[NonNull<u8>]) {
        self.0.launder(class, objects);
    }

    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.0
            .alloc(class.class(), class.size())
            .map(|block| block.ptr)
    }

    fn free_one(&self, class: SmallClass, object: NonNull<u8>) {
        self.0.free(object, class.class());
    }

    fn purge_step(&self, now: &dyn Fn() -> u64) {
        self.0.heartbeat_with(now);
    }
}
