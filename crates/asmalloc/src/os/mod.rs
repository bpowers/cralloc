//! What the allocator asks of the operating system.
//!
//! The allocator is generic over [`Os`]. Programs use [`linux::LinuxOs`], which makes raw
//! system calls; tests, fuzzing, Kani and Miri use [`sim::SimOs`], which simulates the same
//! calls over ordinary memory and can fail any of them on demand. Nothing else in the crate
//! talks to the OS.
//!
//! The interface is what the design needs and no more: reserve address space, make parts of it
//! writable, return pages, steer transparent huge pages, unmap, fence restartable sequences,
//! report the rseq registration, and abort. Each implementation also fixes the [`Geometry`] the
//! heap is laid out with, so the simulated OS can use a scaled-down heap.
//!
//! Ranges passed to the memory calls are page-aligned, a whole number of pages long, not empty,
//! and inside one live reservation made by the same `Os` value. [`crate::space::Space`] checks
//! this and wraps the calls, so most code never calls them directly.

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(kani)))]
pub mod linux;
pub mod rseq;
#[cfg(any(test, kani, feature = "sim"))]
pub mod sim;

pub use rseq::RseqArea;

use crate::space::Geometry;
use core::fmt;
use core::num::NonZeroI32;
use core::ptr::NonNull;

/// A failed OS call, with the `errno` it reported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OsError(NonZeroI32);

impl OsError {
    /// Not enough memory or address space.
    pub const ENOMEM: OsError = OsError::from_errno(12);
    /// An invalid argument, or a feature the kernel lacks.
    pub const EINVAL: OsError = OsError::from_errno(22);
    /// Not permitted, for example a fence without registering first.
    pub const EPERM: OsError = OsError::from_errno(1);

    /// The error for an `errno` value. Zero is not an error and maps to `EINVAL`.
    pub const fn from_errno(errno: i32) -> OsError {
        match NonZeroI32::new(errno) {
            Some(errno) => OsError(errno),
            None => OsError::from_errno(22),
        }
    }

    /// The `errno` value.
    pub const fn errno(self) -> i32 {
        self.0.get()
    }
}

impl fmt::Debug for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OsError(errno {})", self.errno())
    }
}

/// Transparent-huge-page advice for a range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HugePages {
    /// Back the range with huge pages when possible (`MADV_HUGEPAGE`).
    Prefer,
    /// Never back the range with huge pages (`MADV_NOHUGEPAGE`).
    Avoid,
}

/// The operating system, as the allocator sees it.
///
/// # Safety
///
/// The allocator's memory safety rests on these promises, so implementing the trait is unsafe:
///
/// - `reserve(len, align)` returns the start of `len` bytes aligned to `align`, carrying
///   provenance for all of them, that no live allocation and no other live reservation
///   overlaps. The range stays reserved until unmapped. Its bytes read as zero once committed.
/// - After `commit` succeeds the range is readable and writable until it is unmapped, and its
///   contents are unchanged.
/// - After `purge` succeeds the range reads as zero and stays writable if it was.
/// - `advise_huge` changes neither contents nor permissions.
/// - A failed call may have taken effect on part of its range, but never beyond it.
/// - A reservation's memory stays valid until it is unmapped or the value that made it is
///   dropped, whichever is first.
/// - `fence` returns only after every restartable sequence that was in flight in the process
///   when it was called has completed or been aborted.
/// - `rseq` never overstates the process's registration: if it reports a concurrency ID, the
///   kernel maintains `mm_cid` in every registered area. It does not promise that the calling
///   thread is registered; see [`Os::rseq`].
/// - `abort` does not return.
/// - Every method may be called from any thread at any time, including from a signal handler,
///   and none of them allocates.
pub unsafe trait Os: Sync {
    /// The shape of the heap this OS hosts.
    const GEOMETRY: Geometry;

    /// Reserves `len` bytes of inaccessible address space aligned to `align`. `len` is a
    /// non-zero multiple of the page size and `align` a power of two no smaller than a page.
    fn reserve(&self, len: usize, align: usize) -> Result<NonNull<u8>, OsError>;

    /// Makes a range readable and writable.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation.
    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError>;

    /// Returns a range's physical memory to the OS (`MADV_DONTNEED`). Afterwards it reads as
    /// zero.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation, no thread reads or writes it
    /// during the call, and nothing in it is needed afterwards.
    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError>;

    /// Advises for or against transparent huge pages over a range.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation.
    unsafe fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError>;

    /// Unmaps a range. A later `reserve` may return it again.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation, and nothing reads or writes it
    /// after the call begins.
    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError>;

    /// Registers the process for [`Os::fence`]. Call once, before the first fence.
    fn register_fence(&self) -> Result<(), OsError>;

    /// Restarts every restartable sequence in flight in the process
    /// (`MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ`). Fails unless [`Os::register_fence`] succeeded.
    fn fence(&self) -> Result<(), OsError>;

    /// Where the rseq area is and how much of it the kernel maintains, or `None` if the process
    /// has no registration.
    ///
    /// This describes the process, not the calling thread. glibc registers each thread as it
    /// starts, and a registration that fails later (for example under a seccomp filter installed
    /// after start-up) leaves that thread's `cpu_id` field at -2 while these facts stay the
    /// same. Code that relies on a thread being registered must check the thread's own `cpu_id`
    /// (`rseq::CPU_ID`) is not negative. Meant for start-up: on Linux each call is a system call.
    fn rseq(&self) -> Option<RseqArea>;

    /// Writes `message` to standard error and ends the process abnormally.
    fn abort(&self, message: &str) -> !;
}
