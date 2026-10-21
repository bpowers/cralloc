//! What the allocator asks of the operating system.
//!
//! The allocator is generic over [`Os`]. Programs use [`linux::LinuxOs`], which makes raw
//! system calls; tests, fuzzing, Kani and Miri use [`sim::SimOs`], which simulates the same
//! calls over ordinary memory and can fail any of them on demand. Nothing else in the crate
//! talks to the OS.
//!
//! The interface is what the design needs and no more: reserve address space, make parts of it
//! writable, return pages, steer transparent huge pages, mark a page to be wiped in forked
//! children, unmap, fence the restartable sequences of one CPU, draw a random number, read a
//! clock, and abort. Each implementation also fixes the [`Geometry`] the heap is laid out with,
//! so the simulated OS can use a scaled-down heap.
//!
//! Ranges passed to the memory calls are page-aligned, a whole number of pages long, not empty,
//! and inside one live reservation made by the same `Os` value. [`crate::space::Space`] checks
//! this and wraps the calls, so most code never calls them directly.

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(kani)))]
pub mod linux;
pub mod machine;
#[cfg(any(test, kani, feature = "sim"))]
pub mod sim;

pub use machine::{Aborted, Machine, Pinned, Step};

use crate::space::Geometry;
use core::fmt;
use core::num::NonZeroI32;
use core::ptr::NonNull;
use core::sync::atomic::AtomicU64;

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

/// A list of CPUs the kernel publishes, which [`Os::topology`] reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Topology {
    /// The CPUs that are online (`/sys/devices/system/cpu/online`).
    Online,
    /// The CPUs that share CPU `n`'s L3 cache
    /// (`/sys/devices/system/cpu/cpuN/cache/index3/shared_cpu_list`).
    SharedL3(u32),
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
/// - After `wipe_on_fork` succeeds, a child forked later reads the range as zero; the calling
///   process's contents and permissions are unchanged.
/// - A failed call may have taken effect on part of its range, but never beyond it.
/// - A reservation's memory stays valid until it is unmapped or the value that made it is
///   dropped, whichever is first. Moving the value moves none of its reservations.
/// - `fence(cpu)` returns `Ok` only after every restartable sequence of the process that was in
///   flight on CPU `cpu` when it was called has completed or been aborted.
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

    /// How many pages of a range are resident (`mincore`). No promise of this trait's safety
    /// contract depends on the answer: a wrong count only makes the allocator return a page it
    /// could have kept, or keep one it could have returned. The simulator counts a committed
    /// page that holds a nonzero byte, a lower bound, since it cannot see a write of zeros.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation, and no thread writes it during
    /// the call (the simulator reads it).
    unsafe fn resident(&self, start: NonNull<u8>, len: usize) -> Result<usize, OsError>;

    /// Marks a range so that a child forked afterwards reads it as zero (`MADV_WIPEONFORK`).
    /// The process generation lives on such a page.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation.
    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError>;

    /// Unmaps a range. A later `reserve` may return it again.
    ///
    /// # Safety
    ///
    /// The range follows the rules in the module documentation, and nothing reads or writes it
    /// after the call begins.
    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError>;

    /// Registers the process for [`Os::fence`]. Call once, before the first fence.
    fn register_fence(&self) -> Result<(), OsError>;

    /// Restarts every restartable sequence of the process in flight on CPU `cpu`
    /// (`MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ` with `MEMBARRIER_CMD_FLAG_CPU`). A CPU that is
    /// offline or does not exist has no sequence in flight, so fencing it succeeds at once.
    /// Fails unless [`Os::register_fence`] succeeded, and may fail for want of memory.
    fn fence(&self, cpu: u32) -> Result<(), OsError>;

    /// A full memory barrier on every CPU that runs one of the process's threads, before the
    /// call returns (`MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ` without a CPU): whatever another
    /// thread stored before its own barrier the caller sees, and whatever that thread loads
    /// after it sees what the caller stored before the call. Fails unless
    /// [`Os::register_fence`] succeeded, and may fail for want of memory.
    fn fence_all(&self) -> Result<(), OsError>;

    /// Waits while the low 32 bits of `word` hold the low 32 bits of `expected`, for at most
    /// `timeout_ns` nanoseconds if given (`FUTEX_WAIT_PRIVATE` on the word's first four bytes,
    /// its low half on x86-64). Returns at once if they differ, and otherwise on a
    /// [`Os::wake`], at the timeout, on a signal, or for no reason: the caller looks at the word
    /// again. Never fails visibly. Every change the caller waits for must change the low half.
    fn wait(&self, word: &AtomicU64, expected: u64, timeout_ns: Option<u64>);

    /// Wakes a thread waiting on `word` in [`Os::wait`], if there is one (`FUTEX_WAKE_PRIVATE`).
    fn wake(&self, word: &AtomicU64);

    /// The evidence that the program has created a thread of its own, which the background
    /// thread's default start waits for. In a process that is not a fork child, the C
    /// library's own record (glibc's `__libc_single_threaded`). In a fork child, whose copy of
    /// that record still says what the parent did, and where the library keeps none (musl),
    /// the weaker test: the calling thread is not the process's first. Without the background
    /// thread's feature, `false`: nothing then wants a thread.
    fn more_than_one_thread(&self, forked: bool) -> bool;

    /// A random number. It names a process generation, so it should differ from the parent's
    /// with overwhelming probability; it need not be fit for cryptography. Never fails: if the
    /// OS has no randomness to give, it returns something that varies with time and address.
    fn random(&self) -> u64;

    /// A count that rises with wall-clock time, read without a system call: the CPU's
    /// timestamp counter on Linux, a clock the test advances in the simulators. Its rate is not
    /// known (on x86-64 it is somewhere between 1 and 6 GHz), and nothing promises that two
    /// CPUs agree or that it never goes back. It paces what happens once an interval: the idle
    /// check's drains and the back end's clocked purge, both read on slow paths. No promise of
    /// this trait's safety contract depends on it: a wrong count only makes a drain or a purge
    /// pass come sooner or later, and both are safe at any time.
    fn ticks(&self) -> u64;

    /// Reads the CPU list `which` into `buf` as the kernel prints it (`0-7,16-23` and a
    /// newline) and returns its length, or `None` if it cannot be read whole: no such list, a
    /// refused call, or more than `buf` holds. Called only from the first slow path of the
    /// process (`depot::domain`). No promise of this trait's safety contract depends on what
    /// it returns: a wrong list only puts magazines in the wrong cache domain's stacks.
    fn topology(&self, which: Topology, buf: &mut [u8]) -> Option<usize>;

    /// Writes `message` to standard error and ends the process abnormally.
    fn abort(&self, message: &str) -> !;
}
