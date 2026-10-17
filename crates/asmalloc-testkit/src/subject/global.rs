//! Rust's `GlobalAlloc`.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

use super::{Grant, Held, Refused, Request, Resized, Subject, ZeroSize};

/// Drives a `GlobalAlloc`. `dealloc` and `realloc` get exactly the layout the block was
/// allocated with, as the trait requires, and `realloc` keeps the alignment.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViaGlobalAlloc<G>(pub G);

fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).expect("the model only builds valid layouts")
}

impl<G: GlobalAlloc + Sync> Subject for ViaGlobalAlloc<G> {
    fn name(&self) -> String {
        format!("GlobalAlloc for {}", core::any::type_name::<G>())
    }

    fn zero_size(&self) -> ZeroSize {
        ZeroSize::NONE
    }

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let l = layout(req.size, req.align);
        // SAFETY: the caller guarantees a non-zero size, since `zero_size` forbids zero.
        let p = unsafe {
            if req.zeroed {
                self.0.alloc_zeroed(l)
            } else {
                self.0.alloc(l)
            }
        };
        let ptr = NonNull::new(p).ok_or(Refused)?;
        Ok(Some(Grant {
            ptr,
            usable: req.size,
            align: req.align,
            zeroed: if req.zeroed { req.size } else { 0 },
            tag: 0,
        }))
    }

    unsafe fn free(&self, block: Held, _variant: u32) {
        // SAFETY: the caller guarantees the block is live and was allocated with this layout.
        unsafe {
            self.0
                .dealloc(block.ptr.as_ptr(), layout(block.size, block.align))
        }
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        let old = layout(block.size, block.align);
        // SAFETY: the block is live with layout `old`; the new size is non-zero and does not
        // overflow `isize` when rounded up to the alignment (caller contract).
        let p = unsafe { self.0.realloc(block.ptr.as_ptr(), old, req.size) };
        let ptr = NonNull::new(p).ok_or(Refused)?;
        Ok(Resized {
            grant: Grant {
                ptr,
                usable: req.size,
                align: block.align,
                zeroed: 0,
                tag: 0,
            },
            preserved: block.size.min(req.size),
            zeroed_from: None,
        })
    }
}
