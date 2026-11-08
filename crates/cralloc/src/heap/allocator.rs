//! The `Allocator` trait's operations on a heap (cargo feature `allocator-api`).
//!
//! `allocate` returns the usable size: the class size for a small block, and for a large one
//! the size asked for carried to the end of the page it ends in (`Heap::usable_size`), never the
//! whole slot or run, which may be far bigger. `deallocate` may then be called with any size from
//! the one requested to that length, which maps to the same class (the fit rule, proved in
//! `classes::verify`), or, for a large block, is not consulted at all. A zero-size layout gets a
//! dangling pointer aligned to it and no block, and a zero-size block is never freed. The
//! ledger for the unsafe blocks here is `docs/safety/cralloc/heap/allocator-safety.md`.

use super::{Bound, Zero};
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

impl<O: Os, R: Routines + Copy> Bound<'_, O, R> {
    /// `Allocator::allocate` and `allocate_zeroed`.
    #[inline]
    pub(crate) fn allocate(self, layout: Layout, zero: bool) -> Result<NonNull<[u8]>, AllocError> {
        if layout.size() == 0 {
            return Ok(dangling(layout));
        }
        let fill = if zero { Zero::Whole } else { Zero::No };
        let grant = self.alloc_grant(layout, fill).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(grant.ptr, grant.len))
    }

    /// `Allocator::deallocate`.
    ///
    /// # Safety
    ///
    /// `ptr` and `layout` meet `Allocator::deallocate`'s contract for a block this heap's
    /// `allocate` returned.
    #[inline]
    pub(crate) unsafe fn deallocate(self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            // SAFETY: ALLOCATOR-01. a non-zero layout that fits a block describes the block it
            // was allocated as, by the fit rule.
            unsafe { self.dealloc(ptr.as_ptr(), layout) }
        }
    }

    /// `Allocator::grow` and `grow_zeroed`.
    ///
    /// # Safety
    ///
    /// The arguments meet `Allocator::grow`'s contract for a block of this heap.
    pub(crate) unsafe fn grow(
        self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
        zero: bool,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // A zero-size block is no block. Otherwise `new` is at least as large, so not empty.
        if old.size() == 0 {
            return self.allocate(new, zero);
        }
        // SAFETY: ALLOCATOR-02. `ptr` is a live block that `old` fits, which the caller gives
        // up unless the grow fails, and `old` is not empty.
        let grant = unsafe { self.grow_block(ptr, old, new, zero) }.ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(grant.ptr, grant.len))
    }

    /// `Allocator::shrink`.
    ///
    /// # Safety
    ///
    /// The arguments meet `Allocator::shrink`'s contract for a block of this heap.
    pub(crate) unsafe fn shrink(
        self,
        ptr: NonNull<u8>,
        old: Layout,
        new: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        if new.size() == 0 {
            // SAFETY: ALLOCATOR-03. `ptr` is a live block that `old` fits, which a shrink to
            // nothing gives up.
            unsafe { self.deallocate(ptr, old) };
            return Ok(dangling(new));
        }
        // SAFETY: ALLOCATOR-03. as in `grow`, and `new` is not empty.
        let grant = unsafe { self.shrink_block(ptr, old, new) }.ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(grant.ptr, grant.len))
    }
}
