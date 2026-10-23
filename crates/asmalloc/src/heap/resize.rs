//! The lean path of a resize: a small block to a small class.
//!
//! Nearly every resize a program makes is this case: a `Vec`, `String` or buffer that has
//! outgrown its block and moves one class up. So every resize tries it first, and it does the
//! least the case needs. It finds each class once, the old one from the caller's layout (for C,
//! from the address) and the new one from the new size, with the same inlined lookup the fast
//! path uses. When the two are the same class the block stays: a small block stays in place
//! only then, since the next free finds its class from its layout (design: "Large objects").
//! Otherwise it takes a block of the new class from this CPU's magazine with the pop routine,
//! copies, and gives the old block back with the push routine.
//!
//! Everything else goes to the general resize (`large.rs`), cold and out of line, which
//! decides as it always has: a large block on either side, a size or alignment with no small
//! class, a zero size, and a pop that misses, which has changed nothing by then. A push that
//! misses ends in the free's own slow path, as any free's does. So the lean path changes what
//! a resize costs and nothing that it does.
//!
//! The ledger for the unsafe blocks here is `docs/safety/asmalloc/heap/resize-safety.md`.

use super::{Bound, Grant};
use crate::classes::SmallClass;
use crate::front::Routines;
use crate::os::Os;
use core::alloc::Layout;
use core::ptr::{self, NonNull};

/// Which interface a resize came through, which decides how a push that misses completes: a
/// Rust call's slow path may start the background thread, a C call's never does
/// (`Bound::after_rust_slow_path`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Caller {
    Rust,
    C,
}

impl<O: Os, R: Routines + Copy> Bound<'_, O, R> {
    /// `GlobalAlloc::realloc`: the lean path, and the general resize for anything else. Null
    /// if a move found no memory, or for a new size no `Layout` can have; the old block is
    /// then untouched.
    ///
    /// # Safety
    ///
    /// As `GlobalAlloc::realloc`: `ptr` is a block this heap handed out that the caller owns,
    /// described by `old` as for [`Bound::dealloc`], and gives up unless the call returns
    /// null; `new_size` is not zero.
    #[inline(always)]
    pub(crate) unsafe fn realloc(self, ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        // The new size by itself, with no `Layout`: one is built only on the general path,
        // which checks it.
        if let (Some(from), Some(to)) = (
            SmallClass::of_nonzero_layout(old),
            SmallClass::of_nonzero(new_size, old.align()),
        ) {
            // SAFETY: HEAP-06.
            let block = unsafe { NonNull::new_unchecked(ptr) };
            let kept = old.size().min(new_size);
            // SAFETY: HEAP-07, whose preconditions this caller meets: the block is the
            // caller's, of `from` by the fit rule, as `dealloc` takes it; `kept` is at most the
            // old size and the new one, so at most either class.
            if let Some(moved) = unsafe { self.resize_small(block, from, to, kept, Caller::Rust) } {
                return moved.as_ptr();
            }
        }
        // SAFETY: the caller's contract, which the lean path left as it found it.
        unsafe { self.realloc_other(ptr, old, new_size) }
    }

    /// [`Bound::realloc`] for what the lean path does not take, out of line.
    ///
    /// # Safety
    ///
    /// As for [`Bound::realloc`].
    #[cold]
    #[inline(never)]
    unsafe fn realloc_other(self, ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        let (Some(block), Ok(new)) = (
            NonNull::new(ptr),
            Layout::from_size_align(new_size, old.align()),
        ) else {
            return ptr::null_mut();
        };
        // SAFETY: the caller's contract, and `new_size` is not zero.
        unsafe { self.resize(block, old, new, false) }
            .map_or(ptr::null_mut(), |grant| grant.ptr.as_ptr())
    }

    /// `Allocator::grow` and `grow_zeroed` of a block that is not empty: the lean path, with
    /// the bytes past the old block cleared when `zero` asks and the block moved, as the
    /// general resize clears them; and the general resize for anything else.
    ///
    /// # Safety
    ///
    /// `Allocator::grow`'s contract for a block of this heap, with `old` not empty.
    // Compiled without the feature too, for the tests' stable hooks into it.
    #[cfg_attr(not(any(test, feature = "allocator-api")), allow(dead_code))]
    #[inline(always)]
    pub(crate) unsafe fn grow_block(
        self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
        zero: bool,
    ) -> Option<Grant> {
        if let (Some(from), Some(to)) = (
            SmallClass::of_nonzero_layout(old),
            SmallClass::of_nonzero_layout(new),
        ) {
            let kept = old.size().min(new.size());
            // SAFETY: HEAP-07, met as in `realloc`.
            if let Some(moved) = unsafe { self.resize_small(ptr, from, to, kept, Caller::Rust) } {
                if zero && moved != ptr {
                    // SAFETY: HEAP-08.
                    unsafe { moved.as_ptr().add(kept).write_bytes(0, to.size() - kept) };
                }
                return Some(Grant {
                    ptr: moved,
                    len: to.size(),
                    zeroed: false,
                });
            }
        }
        // SAFETY: the caller's contract; `new` is at least `old`, so not empty.
        unsafe { self.resize(ptr, old, new, zero) }
    }

    /// `Allocator::shrink` to a size that is not zero: the lean path, and the general resize
    /// for anything else.
    ///
    /// # Safety
    ///
    /// `Allocator::shrink`'s contract for a block of this heap, with `new` not empty.
    // Compiled without the feature too, for the tests' stable hooks into it.
    #[cfg_attr(not(any(test, feature = "allocator-api")), allow(dead_code))]
    #[inline(always)]
    pub(crate) unsafe fn shrink_block(
        self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Option<Grant> {
        if let (Some(from), Some(to)) = (
            SmallClass::of_nonzero_layout(old),
            SmallClass::of_nonzero_layout(new),
        ) {
            let kept = old.size().min(new.size());
            // SAFETY: HEAP-07, met as in `realloc`.
            if let Some(moved) = unsafe { self.resize_small(ptr, from, to, kept, Caller::Rust) } {
                return Some(Grant {
                    ptr: moved,
                    len: to.size(),
                    zeroed: false,
                });
            }
        }
        // SAFETY: the caller's contract, and `new` is not empty.
        unsafe { self.resize(ptr, old, new, false) }
    }

    /// The lean path: the block at `ptr`, of class `from`, resized to class `to` with its first
    /// `kept` bytes kept. The same block when the classes are the same; else a block of `to`
    /// popped from this CPU's magazine, with `kept` bytes copied into it, and the old block
    /// pushed back, or freed by the free's slow path if the push misses. `None` if the pop
    /// misses: nothing has changed, and the caller takes the general path.
    ///
    /// # Safety
    ///
    /// `ptr` is a live block of `from` this heap handed out, which the caller owns and gives up
    /// if this returns another block; `kept` is at most the size of each class.
    #[inline(always)]
    pub(crate) unsafe fn resize_small(
        self,
        ptr: NonNull<u8>,
        from: SmallClass,
        to: SmallClass,
        kept: usize,
        caller: Caller,
    ) -> Option<NonNull<u8>> {
        if from == to {
            return Some(ptr);
        }
        let moved = self.routines.pop(to)?;
        // SAFETY: HEAP-07.
        unsafe { ptr.as_ptr().copy_to_nonoverlapping(moved.as_ptr(), kept) };
        if !self.routines.push(from, ptr) {
            // A push that misses stores nothing, so the block is still the caller's, who gives
            // it up: a block of `from` this heap handed out, as both slow paths take.
            match caller {
                // SAFETY: as just said.
                Caller::Rust => unsafe { self.rust_free_slow(from, ptr) },
                // SAFETY: as just said.
                Caller::C => drop(unsafe { self.free_slow(from, ptr) }),
            }
        }
        Some(moved)
    }
}
