//! [`LinuxOs`]: the `Os` interface as Linux system calls.
//!
//! Each method is one or a few system calls; the arguments come from the `Os` contract and the
//! kernel's documentation. The ledger entry for the unsafe blocks here is
//! `docs/safety/asmalloc/os/linux/kernel-safety.md`.

use super::glibc;
use super::syscall::{Reg, int, nr, result, syscall};
use crate::os::{HugePages, Os, OsError, RseqArea};
use crate::space::Geometry;
use core::ptr::{self, NonNull};

const PROT_NONE: usize = 0;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_NORESERVE: usize = 0x4000;
const MADV_DONTNEED: usize = 4;
const MADV_HUGEPAGE: usize = 14;
const MADV_NOHUGEPAGE: usize = 15;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 7;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 8;
const SIGABRT: usize = 6;
const SIG_DFL: usize = 0;
const SIG_UNBLOCK: usize = 1;
const STDERR: usize = 2;
/// The kernel's `sigset_t` is one 64-bit word on x86-64.
const SIGSET_SIZE: usize = 8;
const PR_GET_AUXV: usize = 0x4155_5856;
const AT_NULL: u64 = 0;
const AT_RSEQ_FEATURE_SIZE: u64 = 27;
/// The bytes of `struct rseq` the kernel maintained before `AT_RSEQ_FEATURE_SIZE` existed: up to
/// and including `flags`, but not `node_id` or `mm_cid` (Linux 6.3).
const ORIGINAL_RSEQ_FEATURE_SIZE: u32 = 20;

/// The kernel's `struct sigaction` on x86-64 (`include/linux/signal_types.h` with
/// `__ARCH_HAS_SA_RESTORER`), not glibc's, which is laid out differently.
#[repr(C)]
struct KernelSigaction {
    handler: usize,
    flags: u64,
    restorer: usize,
    mask: u64,
}

/// The operating system, through raw Linux system calls on x86-64.
///
/// A zero-sized handle: every value is the same process's OS.
#[derive(Clone, Copy, Default, Debug)]
pub struct LinuxOs;

/// A memory call on `start..start + len` with one more argument.
///
/// # Safety
///
/// `number` is `mprotect`, `madvise` or `munmap`, and the call is one the caller's `Os` method
/// contract permits on that range.
unsafe fn range_call(
    number: usize,
    start: NonNull<u8>,
    len: usize,
    arg: usize,
) -> Result<(), OsError> {
    // SAFETY: passed through from the caller; these calls read and write no user memory.
    result(unsafe {
        syscall(
            number,
            [start.as_ptr(), int(len), int(arg), int(0), int(0), int(0)],
        )
    })
    .map(|_| ())
}

fn membarrier(command: usize) -> Result<(), OsError> {
    // SAFETY: membarrier takes no pointers and changes no memory; the rseq commands only
    // restart sequences in flight, which every sequence must tolerate anyway.
    result(unsafe {
        syscall(
            nr::MEMBARRIER,
            [int(command), int(0), int(0), int(0), int(0), int(0)],
        )
    })
    .map(|_| ())
}

/// Writes all of `bytes` to standard error, giving up on any error.
fn write_stderr(mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let buffer: Reg = bytes.as_ptr().cast_mut();
        // SAFETY: write reads `bytes.len()` bytes from `buffer`, which is a live slice.
        let written = result(unsafe {
            syscall(
                nr::WRITE,
                [
                    int(STDERR),
                    buffer,
                    int(bytes.len()),
                    int(0),
                    int(0),
                    int(0),
                ],
            )
        });
        match written {
            Ok(n) if n.addr() > 0 => bytes = &bytes[n.addr().min(bytes.len())..],
            _ => return,
        }
    }
}

/// A system call that takes plain integers and changes nothing the allocator relies on.
///
/// # Safety
///
/// As for [`syscall`]; the callers below are getpid, gettid, tgkill and exit_group.
unsafe fn int_call(number: usize, a: usize, b: usize, c: usize) -> usize {
    // SAFETY: passed through from the caller.
    unsafe { syscall(number, [int(a), int(b), int(c), int(0), int(0), int(0)]) }.addr()
}

/// The number of bytes of an rseq area the kernel maintains, from `AT_RSEQ_FEATURE_SIZE` in the
/// auxiliary vector.
///
/// glibc 2.40 and later already report this as `__rseq_size`, but 2.35 to 2.39 report 32 on any
/// kernel. On a kernel older than 6.3 that would claim an `mm_cid` the kernel never writes; every
/// thread would read zero and share one slab. So the size is capped by what the kernel says.
/// `PR_GET_AUXV` exists from Linux 6.4; where it fails, or the entry is missing, this answers the
/// original size, which turns the fast paths off on 6.3 rather than risk them anywhere.
fn kernel_rseq_feature_size() -> u32 {
    let mut auxv = [[0u64; 2]; 64];
    let buffer: Reg = auxv.as_mut_ptr().cast();
    let len = size_of_val(&auxv);
    // SAFETY: prctl(PR_GET_AUXV) writes at most `len` bytes of the auxiliary vector into
    // `buffer`, a live, exclusively borrowed local array of that many bytes, and changes nothing
    // else. The last two arguments must be zero.
    let copied = result(unsafe {
        syscall(
            nr::PRCTL,
            [int(PR_GET_AUXV), buffer, int(len), int(0), int(0), int(0)],
        )
    });
    if copied.is_err() {
        return ORIGINAL_RSEQ_FEATURE_SIZE;
    }
    for [kind, value] in auxv {
        match kind {
            AT_NULL => break,
            AT_RSEQ_FEATURE_SIZE => {
                return u32::try_from(value).unwrap_or(ORIGINAL_RSEQ_FEATURE_SIZE);
            }
            _ => {}
        }
    }
    ORIGINAL_RSEQ_FEATURE_SIZE
}

// SAFETY: each promise of `Os`, against the kernel's documented behaviour (mmap(2), mprotect(2),
// madvise(2), munmap(2), membarrier(2)):
// - `reserve` maps a fresh private anonymous range, which overlaps nothing that exists; the
//   kernel returns it with the provenance of a new allocation through the `syscall` result
//   pointer. It is `PROT_NONE` until committed, and fresh anonymous memory reads as zero. The
//   over-reservation and trim make the start aligned, and only the unused ends are unmapped.
// - `commit` is mprotect to read and write, which keeps contents. `purge` is MADV_DONTNEED,
//   after which private anonymous memory reads as zero and stays mapped with its protection.
//   `advise_huge` is MADV_HUGEPAGE or MADV_NOHUGEPAGE, which change neither.
// - A failed call reports `-errno`; mprotect and madvise may have acted on part of the range,
//   never beyond it.
// - Every `LinuxOs` is the same process's OS, so values are interchangeable.
// - `fence` is MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ, which returns after every thread of the
//   process has been interrupted out of any rseq critical section; it fails with EPERM unless
//   the process registered.
// - `rseq` reads glibc's registration, which is per process and so the same for every thread,
//   and caps its size by the kernel's feature size, so it never reports a concurrency ID the
//   kernel does not maintain.
// - `abort` ends with exit_group if the signal did not end the process first, so it does not
//   return.
// - None of the calls allocates or depends on thread state, so any thread may make them,
//   in a signal handler too.
unsafe impl Os for LinuxOs {
    const GEOMETRY: Geometry = Geometry::DESIGN;

    fn reserve(&self, len: usize, align: usize) -> Result<NonNull<u8>, OsError> {
        let page = Self::GEOMETRY.page_size();
        if len == 0 || !len.is_multiple_of(page) || !align.is_power_of_two() || align < page {
            return Err(OsError::EINVAL);
        }
        // mmap returns page-aligned addresses, so `align - page` extra bytes always contain an
        // aligned run of `len`. The ends are unmapped again below.
        let span = len.checked_add(align - page).ok_or(OsError::ENOMEM)?;
        let flags = MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE;
        let args = [
            ptr::null_mut(),
            int(span),
            int(PROT_NONE),
            int(flags),
            int(usize::MAX),
            int(0),
        ];
        // SAFETY: an anonymous mapping at an address of the kernel's choosing replaces nothing
        // and reads no user memory (fd -1, offset 0).
        let mapped = result(unsafe { syscall(nr::MMAP, args) })?;
        let Some(mapped) = NonNull::new(mapped) else {
            return Err(OsError::ENOMEM);
        };
        let lead = mapped.addr().get().wrapping_neg() & (align - 1);
        let start = mapped.map_addr(|a| a.saturating_add(lead));
        let tail = span - lead - len;
        // A failed trim leaves an inaccessible range mapped, which costs address space only.
        if lead != 0 {
            // SAFETY: `mapped..start` is the unused head of the fresh mapping.
            let _ = unsafe { range_call(nr::MUNMAP, mapped, lead, 0) };
        }
        if tail != 0 {
            let end = start.map_addr(|a| a.saturating_add(len));
            // SAFETY: `end..end + tail` is the unused tail of the fresh mapping.
            let _ = unsafe { range_call(nr::MUNMAP, end, tail, 0) };
        }
        Ok(start)
    }

    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: the caller passes whole pages of a live reservation; making them writable
        // changes no contents and nothing else relies on them being inaccessible.
        unsafe { range_call(nr::MPROTECT, start, len, PROT_READ | PROT_WRITE) }
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: the caller passes whole pages of a live reservation that nothing uses during
        // the call and whose contents nothing needs, so zeroing them breaks nothing.
        unsafe { range_call(nr::MADVISE, start, len, MADV_DONTNEED) }
    }

    unsafe fn advise_huge(
        &self,
        start: NonNull<u8>,
        len: usize,
        advice: HugePages,
    ) -> Result<(), OsError> {
        let advice = match advice {
            HugePages::Prefer => MADV_HUGEPAGE,
            HugePages::Avoid => MADV_NOHUGEPAGE,
        };
        // SAFETY: the caller passes whole pages of a live reservation; the advice changes no
        // contents or permissions.
        unsafe { range_call(nr::MADVISE, start, len, advice) }
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: the caller passes whole pages of a live reservation that nothing uses again.
        unsafe { range_call(nr::MUNMAP, start, len, 0) }
    }

    fn register_fence(&self) -> Result<(), OsError> {
        membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ)
    }

    fn fence(&self) -> Result<(), OsError> {
        membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ)
    }

    fn rseq(&self) -> Option<RseqArea> {
        let area = glibc::registration()?;
        RseqArea::new(area.offset(), area.size().min(kernel_rseq_feature_size()))
    }

    fn abort(&self, message: &str) -> ! {
        write_stderr(b"asmalloc: ");
        write_stderr(message.as_bytes());
        write_stderr(b"\n");
        // Restore the default action before raising: a handler could allocate, and the heap is
        // in a state we refuse to continue from. The default action ends the process with a
        // core dump.
        let default = KernelSigaction {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        };
        let unblock: u64 = 1 << (SIGABRT - 1);
        let action: Reg = ptr::from_ref(&default).cast_mut().cast();
        let set: Reg = ptr::from_ref(&unblock).cast_mut().cast();
        // SAFETY: rt_sigaction reads one `KernelSigaction` from `action` and rt_sigprocmask one
        // signal set from `set`, both live locals; neither writes back (old pointers are null).
        // The process is ending, so changing its SIGABRT disposition and mask is ours to do.
        unsafe {
            let args = [
                int(SIGABRT),
                action,
                ptr::null_mut(),
                int(SIGSET_SIZE),
                int(0),
                int(0),
            ];
            syscall(nr::RT_SIGACTION, args);
            let args = [
                int(SIG_UNBLOCK),
                set,
                ptr::null_mut(),
                int(SIGSET_SIZE),
                int(0),
                int(0),
            ];
            syscall(nr::RT_SIGPROCMASK, args);
        }
        // SAFETY: getpid and gettid take nothing; tgkill sends SIGABRT to this thread, which
        // ends the process; exit_group ends it if anything above failed. Ending the process is
        // this function's purpose.
        unsafe {
            let pid = int_call(nr::GETPID, 0, 0, 0);
            let tid = int_call(nr::GETTID, 0, 0, 0);
            int_call(nr::TGKILL, pid, tid, SIGABRT);
            loop {
                int_call(nr::EXIT_GROUP, 127, 0, 0);
            }
        }
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::space::Space;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::string::String;

    const PAGE: usize = 4096;
    const REGION: usize = 1 << 32;

    #[test]
    fn reserve_commit_purge_and_unmap() {
        let os = LinuxOs;
        let base = os.reserve(4 * REGION, REGION).unwrap();
        assert_eq!(base.addr().get() % REGION, 0);
        let p = base.as_ptr();
        // SAFETY: each range is whole pages of the reservation, and nothing else uses it.
        unsafe {
            os.commit(base, 2 * PAGE).unwrap();
            assert_eq!(p.add(PAGE + 1).read(), 0);
            p.add(PAGE + 1).write(42);
            os.advise_huge(base, 2 * PAGE, HugePages::Avoid).unwrap();
            os.purge(base, 2 * PAGE).unwrap();
            assert_eq!(p.add(PAGE + 1).read(), 0);
            os.unmap(base, 4 * REGION).unwrap();
        }
    }

    #[test]
    fn bad_arguments_are_errors_not_calls() {
        assert_eq!(LinuxOs.reserve(0, PAGE), Err(OsError::EINVAL));
        assert_eq!(LinuxOs.reserve(PAGE + 1, PAGE), Err(OsError::EINVAL));
        assert_eq!(LinuxOs.reserve(PAGE, 3 * PAGE), Err(OsError::EINVAL));
        assert_eq!(
            LinuxOs.reserve(usize::MAX & !(PAGE - 1), REGION),
            Err(OsError::ENOMEM)
        );
    }

    #[test]
    fn the_design_reservation_is_available() {
        let space = Space::reserve(&LinuxOs).unwrap();
        assert_eq!(space.plan().objects(), Geometry::DESIGN.max_objects());
        // SAFETY: the space was never published and nothing uses it.
        unsafe { space.unmap(&LinuxOs).unwrap() };
    }

    #[test]
    fn glibc_registers_rseq_with_a_concurrency_id() {
        // The supported platform is glibc 2.43 on Linux 7.2, which always register.
        let area = LinuxOs.rseq().expect("glibc registered rseq");
        assert!(area.has_cid(), "{area:?}");
        assert_eq!(area.offset() % 32, 0, "the area is 32-byte aligned");
    }

    #[test]
    fn the_kernel_reports_a_feature_size_with_a_concurrency_id() {
        // Linux 7.2 reports 33 on the reference host.
        assert!(kernel_rseq_feature_size() >= 28);
    }

    #[test]
    fn rseq_fences_work_after_registering() {
        LinuxOs.register_fence().unwrap();
        LinuxOs.fence().unwrap();
    }

    const CHILD: &str = "ASMALLOC_ABORT_CHILD";

    /// Run on its own by `abort_ends_the_process_with_sigabrt`; does nothing otherwise.
    #[test]
    fn abort_child() {
        if std::env::var_os(CHILD).is_some() {
            LinuxOs.abort("test abort");
        }
    }

    #[test]
    fn abort_ends_the_process_with_sigabrt() {
        let exe = std::env::current_exe().unwrap();
        // `ulimit -c 0` keeps the expected crash from leaving a core dump behind.
        let output = Command::new("sh")
            .arg("-c")
            .arg("ulimit -c 0; exec \"$0\" \"$@\"")
            .arg(exe)
            .args([
                "--exact",
                "os::linux::kernel::tests::abort_child",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: test abort\n"), "{stderr}");
    }
}
