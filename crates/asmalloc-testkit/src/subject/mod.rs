//! The interfaces under test, seen through the operations the model performs.
//!
//! The model speaks one small language: allocate a block for a size and alignment (optionally
//! zeroed), free a block, resize a block. A [`Subject`] translates that into one interface's
//! calls and reports back what the interface promised, so the same checks apply to every
//! interface:
//!
//! - [`ViaGlobalAlloc`]: Rust's `GlobalAlloc` (`alloc`, `alloc_zeroed`, `dealloc`, `realloc`);
//! - `ViaAllocator` (feature `allocator-api`): Rust's `Allocator` (`allocate`,
//!   `allocate_zeroed`, `deallocate` with any size the block fits, `grow`, `grow_zeroed`,
//!   `shrink`, alignment changes);
//! - [`ViaC`]: the C functions, through the [`CAbi`] trait.
//!
//! Each request carries a `variant`, a number drawn from the stream's entropy, that the adapter
//! uses to choose among equivalent entry points (`malloc` or `aligned_alloc`, `free` or
//! `free_sized`, `grow` or `shrink` for a size between the requested and the usable one), so
//! every entry point is exercised and a run still replays exactly.

mod c;
mod global;

#[cfg(feature = "allocator-api")]
mod allocator;

use core::ptr::NonNull;

#[cfg(feature = "allocator-api")]
pub use allocator::ViaAllocator;
pub use c::{CAbi, CFns, MAX_ALIGN, SizedFree, ViaC, c_default_align};
pub use global::ViaGlobalAlloc;

/// A request for a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// Bytes requested.
    pub size: usize,
    /// Alignment required: a power of two. An interface whose natural alignment for this size
    /// is at least this much may ignore it.
    pub align: usize,
    /// Whether the block should be zero-filled. An interface that cannot zero this request
    /// (C has no aligned `calloc`) says so in the [`Grant`].
    pub zeroed: bool,
    /// Chooses among equivalent entry points.
    pub variant: u32,
}

/// What an interface returned for a request, and what it promised about it.
#[derive(Clone, Copy, Debug)]
pub struct Grant {
    /// The block.
    pub ptr: NonNull<u8>,
    /// Bytes the caller may use: `Allocator`'s slice length, `malloc_usable_size`, or the
    /// request for `GlobalAlloc`. A correct allocator gives at least the requested size.
    pub usable: usize,
    /// The alignment the interface promises for this block, at least the requested one.
    pub align: usize,
    /// Bytes from the start that the interface promises are zero.
    pub zeroed: usize,
    /// Adapter-private: which entry point made the block, as it needs to know to free it.
    pub tag: u32,
}

/// A live block as the model hands it back to the subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    /// The block.
    pub ptr: NonNull<u8>,
    /// The size passed to the call that produced the block.
    pub size: usize,
    /// [`Grant::align`] of that call.
    pub align: usize,
    /// [`Grant::usable`] of that call.
    pub usable: usize,
    /// [`Grant::tag`] of that call.
    pub tag: u32,
}

impl Held {
    /// The record of a block granted for `size` bytes.
    pub fn new(grant: &Grant, size: usize) -> Held {
        Held {
            ptr: grant.ptr,
            size,
            align: grant.align,
            usable: grant.usable,
            tag: grant.tag,
        }
    }

    /// Address of the block.
    pub fn addr(&self) -> usize {
        self.ptr.addr().get()
    }
}

// SAFETY: a `Held` names a block of plain memory; the model moves that ownership between
// threads exactly as a program moves an allocation, and never shares one between threads.
unsafe impl Send for Held {}

/// What a resize returned and promised.
#[derive(Clone, Copy, Debug)]
pub struct Resized {
    /// The new block (possibly at the old address).
    pub grant: Grant,
    /// Bytes from the start that the interface promises to have carried over.
    pub preserved: usize,
    /// When the interface promises the bytes from here to [`Grant::usable`] are zero.
    pub zeroed_from: Option<usize>,
}

/// The interface reported that it could not serve a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// Which operations may ask for zero bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroSize {
    /// `alloc` with size zero is allowed.
    pub alloc: bool,
    /// `resize` to size zero is allowed.
    pub resize: bool,
}

/// An allocator interface the model can drive.
///
/// Implementations must be `Sync`: the multithreaded driver calls one subject from many threads
/// at once, as a program calls its allocator.
pub trait Subject: Sync {
    /// Name used in failure reports.
    fn name(&self) -> String;

    /// Which operations may ask for zero bytes. Asking for zero otherwise is a caller bug.
    fn zero_size(&self) -> ZeroSize;

    /// Allocate a block. `Ok(None)` means the interface legitimately returned nothing (`malloc(0)`
    /// may return null); `Err(Refused)` means it failed.
    ///
    /// # Safety
    ///
    /// `req.align` is a power of two, `req.size` rounded up to it does not overflow `isize`, and
    /// `req.size` is non-zero unless [`zero_size`](Self::zero_size) allows it.
    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused>;

    /// Free a block.
    ///
    /// # Safety
    ///
    /// `block` was produced by this subject's `alloc` or `resize`, exactly as recorded, and has
    /// not been freed or resized since.
    unsafe fn free(&self, block: Held, variant: u32);

    /// Resize a block. On `Ok` the old block is gone; on `Err` it is still live and unchanged.
    ///
    /// # Safety
    ///
    /// As for [`free`](Self::free) for `block`, and as for [`alloc`](Self::alloc) for `req`.
    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused>;

    /// The usable size the interface reports now for a live block, if it can report one. The
    /// model checks that it never changes while the block is live.
    ///
    /// # Safety
    ///
    /// `block` is live, as for [`free`](Self::free).
    unsafe fn usable_size(&self, block: Held) -> Option<usize> {
        let _ = block;
        None
    }
}

impl<T: Subject + ?Sized> Subject for &T {
    fn name(&self) -> String {
        (**self).name()
    }

    fn zero_size(&self) -> ZeroSize {
        (**self).zero_size()
    }

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        // SAFETY: same contract.
        unsafe { (**self).alloc(req) }
    }

    unsafe fn free(&self, block: Held, variant: u32) {
        // SAFETY: same contract.
        unsafe { (**self).free(block, variant) }
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        // SAFETY: same contract.
        unsafe { (**self).resize(block, req) }
    }

    unsafe fn usable_size(&self, block: Held) -> Option<usize> {
        // SAFETY: same contract.
        unsafe { (**self).usable_size(block) }
    }
}

impl<T: Subject + ?Sized> Subject for Box<T> {
    fn name(&self) -> String {
        (**self).name()
    }

    fn zero_size(&self) -> ZeroSize {
        (**self).zero_size()
    }

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        // SAFETY: same contract.
        unsafe { (**self).alloc(req) }
    }

    unsafe fn free(&self, block: Held, variant: u32) {
        // SAFETY: same contract.
        unsafe { (**self).free(block, variant) }
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        // SAFETY: same contract.
        unsafe { (**self).resize(block, req) }
    }

    unsafe fn usable_size(&self, block: Held) -> Option<usize> {
        // SAFETY: same contract.
        unsafe { (**self).usable_size(block) }
    }
}
