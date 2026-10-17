//! Rust's `Allocator` trait (stable from Rust 1.100).

use core::alloc::{Allocator, Layout};
use core::ptr::NonNull;

use super::{Grant, Held, Refused, Request, Resized, Subject, ZeroSize};

/// A size in `lo..=hi` chosen by `variant`, with the two ends as likely as everything between.
fn pick_between(lo: usize, hi: usize, variant: u32) -> usize {
    debug_assert!(lo <= hi);
    match variant % 4 {
        0 => lo,
        1 => hi,
        _ => lo + (variant as usize / 4) % (hi - lo + 1),
    }
}

/// Drives an `Allocator`. The trait lets a block be described by any size from the one
/// requested to the length returned, so `deallocate`, `grow` and `shrink` get a size chosen
/// from that interval; resizes may change the alignment; zero sizes are allowed.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViaAllocator<A>(pub A);

fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).expect("the model only builds valid layouts")
}

fn grant(slice: NonNull<[u8]>, align: usize, all_zero: bool) -> Grant {
    let usable = slice.len();
    Grant {
        ptr: slice.cast(),
        usable,
        align,
        zeroed: if all_zero { usable } else { 0 },
        tag: 0,
    }
}

impl<A: Allocator + Sync> Subject for ViaAllocator<A> {
    fn name(&self) -> String {
        format!("Allocator for {}", core::any::type_name::<A>())
    }

    fn zero_size(&self) -> ZeroSize {
        ZeroSize {
            alloc: true,
            resize: true,
            distinct: false,
        }
    }

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        let l = layout(req.size, req.align);
        let slice = if req.zeroed {
            self.0.allocate_zeroed(l)
        } else {
            self.0.allocate(l)
        }
        .map_err(|_| Refused)?;
        Ok(Some(grant(slice, req.align, req.zeroed)))
    }

    unsafe fn free(&self, block: Held, variant: u32) {
        let size = pick_between(block.size, block.usable, variant);
        // SAFETY: the block is live, allocated with this alignment, and `size` lies between
        // the size requested and the length returned, so the layout fits the block.
        unsafe { self.0.deallocate(block.ptr, layout(size, block.align)) }
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        let new = layout(req.size, req.align);
        let (lo, hi) = (block.size, block.usable);
        let can_grow = lo <= req.size;
        let can_shrink = hi >= req.size;
        let grow = can_grow && (!can_shrink || req.variant % 2 == 0);
        let pick = req.variant / 2;
        if grow {
            let old_size = pick_between(lo, hi.min(req.size), pick);
            let old = layout(old_size, block.align);
            // SAFETY: the block is live and `old` fits it; `new` is no smaller than `old`.
            let slice = unsafe {
                if req.zeroed {
                    self.0.grow_zeroed(block.ptr, old, new)
                } else {
                    self.0.grow(block.ptr, old, new)
                }
            }
            .map_err(|_| Refused)?;
            // Bytes from `old_size` to the old block's length may be kept or zeroed; only those
            // past the old length must be zero.
            let zeroed_from = req.zeroed.then_some(block.usable);
            Ok(Resized {
                grant: grant(slice, req.align, false),
                preserved: old_size,
                zeroed_from,
            })
        } else {
            let old_size = pick_between(lo.max(req.size), hi, pick);
            let old = layout(old_size, block.align);
            // SAFETY: the block is live and `old` fits it; `new` is no larger than `old`.
            let slice = unsafe { self.0.shrink(block.ptr, old, new) }.map_err(|_| Refused)?;
            Ok(Resized {
                grant: grant(slice, req.align, false),
                preserved: req.size,
                zeroed_from: None,
            })
        }
    }
}
