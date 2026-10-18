//! The `Allocator` trait's operations on a heap (cargo feature `allocator-api`).
//!
//! `allocate` returns the usable size: the class size for a small block, and for a large one
//! the size asked for rounded up to a page (`Heap::usable_size`), never the whole slot or run,
//! which may be far bigger. `deallocate` may then be called with any size from the one
//! requested to that length, which maps to the same class (the fit rule, proved in
//! `classes::verify`), or, for a large block, is not consulted at all. A zero-size layout gets a dangling pointer aligned to it and no
//! block, and a zero-size block is never freed.

use super::{Heap, Zero};
use crate::front::Routines;
use crate::os::Os;
use core::alloc::{AllocError, Layout};
use core::ptr::{self, NonNull};

/// The empty block for a zero-size layout: a dangling pointer with the layout's alignment.
fn dangling(layout: Layout) -> NonNull<[u8]> {
    let ptr = NonNull::new(ptr::without_provenance_mut::<u8>(layout.align()))
        .unwrap_or(NonNull::dangling());
    NonNull::slice_from_raw_parts(ptr, 0)
}

impl<O: Os> Heap<O> {
    /// `Allocator::allocate` and `allocate_zeroed`.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`].
    #[inline]
    pub(crate) unsafe fn allocate<R: Routines + Copy>(
        &self,
        routines: R,
        layout: Layout,
        zero: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        if layout.size() == 0 {
            return Ok(dangling(layout));
        }
        let fill = if zero { Zero::Whole } else { Zero::No };
        // SAFETY: passed through from the caller.
        let grant = unsafe { self.alloc_grant(routines, layout, fill) }.ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(grant.ptr, grant.len))
    }

    /// `Allocator::deallocate`.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`]; and `ptr` and `layout` meet `Allocator::deallocate`'s contract
    /// for a block this heap's `allocate` returned.
    #[inline]
    pub(crate) unsafe fn deallocate<R: Routines + Copy>(
        &self,
        routines: R,
        ptr: NonNull<u8>,
        layout: Layout,
    ) {
        if layout.size() != 0 {
            // SAFETY: a non-zero layout that fits a block describes the block it was allocated
            // as, by the fit rule.
            unsafe { self.dealloc(routines, ptr.as_ptr(), layout) }
        }
    }

    /// `Allocator::grow` and `grow_zeroed`.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`]; and the arguments meet `Allocator::grow`'s contract.
    pub(crate) unsafe fn grow<R: Routines + Copy>(
        &self,
        routines: R,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
        zero: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: passed through from the caller.
        unsafe {
            // A zero-size block is no block. Otherwise `new` is at least as large, so not empty.
            if old.size() == 0 {
                return self.allocate(routines, new, zero);
            }
            let grant = self
                .resize(routines, ptr, old, new, zero)
                .ok_or(AllocError)?;
            Ok(NonNull::slice_from_raw_parts(grant.ptr, grant.len))
        }
    }

    /// `Allocator::shrink`.
    ///
    /// # Safety
    ///
    /// As for [`Heap::alloc`]; and the arguments meet `Allocator::shrink`'s contract.
    pub(crate) unsafe fn shrink<R: Routines + Copy>(
        &self,
        routines: R,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: passed through from the caller.
        unsafe {
            if new.size() == 0 {
                self.deallocate(routines, ptr, old);
                return Ok(dangling(new));
            }
            let grant = self
                .resize(routines, ptr, old, new, false)
                .ok_or(AllocError)?;
            Ok(NonNull::slice_from_raw_parts(grant.ptr, grant.len))
        }
    }
}
