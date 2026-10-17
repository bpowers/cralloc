//! The depot's [`Source`] over the back end.
//!
//! The depot moves pointers in `Option<NonNull<u8>>` slices; the back end fills and launders
//! `*mut u8` slices, which it sorts in place. A small stack buffer converts between them, a
//! chunk at a time.

use crate::back::Back;
use crate::classes::SmallClass;
use crate::depot::Source;
use crate::os::Os;
use core::ptr::{self, NonNull};

/// Pointers moved per call into the back end.
const CHUNK: usize = 64;

/// The back end as the depot's source of objects.
pub(crate) struct BackSource<'a, O: Os>(pub(crate) Back<'a, O>);

impl<O: Os> Source for BackSource<'_, O> {
    fn fill(&self, class: SmallClass, out: &mut [Option<NonNull<u8>>]) -> usize {
        let mut got = 0;
        for part in out.chunks_mut(CHUNK) {
            let mut buffer = [ptr::null_mut(); CHUNK];
            let want = part.len();
            let filled = self.0.fill(class, &mut buffer[..want]).min(want);
            for (slot, &object) in part.iter_mut().zip(&buffer[..filled]) {
                *slot = NonNull::new(object);
            }
            got += filled;
            if filled < want {
                break;
            }
        }
        got
    }

    fn take_back(&self, class: SmallClass, objects: &[NonNull<u8>]) {
        for part in objects.chunks(CHUNK) {
            let mut buffer = [ptr::null_mut(); CHUNK];
            for (slot, object) in buffer.iter_mut().zip(part) {
                *slot = object.as_ptr();
            }
            self.0.launder(class, &mut buffer[..part.len()]);
        }
    }

    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.0.alloc(class.class(), class.size()).map(|block| block.ptr)
    }

    fn free_one(&self, class: SmallClass, object: NonNull<u8>) {
        self.0.free(object, class.class());
    }

    fn purge_step(&self) {
        self.0.heartbeat();
    }
}
