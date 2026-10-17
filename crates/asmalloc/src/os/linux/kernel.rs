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
use core::sync::atomic::{AtomicBool, Ordering};

/// Whether a thread of this process has begun aborting. One process-wide flag, all-zero valid,
/// so that a SIGABRT handler that ends up in `abort` again goes straight to the default action
/// instead of raising into itself.
static ABORTING: AtomicBool = AtomicBool::new(false);

const PROT_NONE: usize = 0;
const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const MAP_PRIVATE: usize = 0x02;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_NORESERVE: usize = 0x4000;
const MADV_DONTNEED: usize = 4;
const MADV_HUGEPAGE: usize = 14;
const MADV_NOHUGEPAGE: usize = 15;
const MADV_WIPEONFORK: usize = 18;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 7;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: usize = 1 << 8;
const MEMBARRIER_CMD_FLAG_CPU: usize = 1;
/// `getrandom` flag: never block, even before the kernel's pool is initialized (Linux 5.6).
const GRND_INSECURE: usize = 0x0004;
const SIGABRT: usize = 6;
const SIG_DFL: usize = 0;
const SIG_UNBLOCK: usize = 1;
const STDERR: usize = 2;
/// The kernel's `sigset_t` is one 64-bit word on x86-64.
const SIGSET_SIZE: usize = 8;

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

/// Makes a range of a reservation inaccessible again (mprotect to `PROT_NONE`). The allocator
/// never needs this; the simulator uses it to turn memory it no longer counts as committed back
/// into memory that faults, as a real reservation's uncommitted pages do.
///
/// # Safety
///
/// The range is whole pages of a live reservation made by `LinuxOs`, and nothing reads or writes
/// it afterwards until it is committed again.
#[cfg(any(test, feature = "sim"))]
pub(crate) unsafe fn protect_none(start: NonNull<u8>, len: usize) -> Result<(), OsError> {
    // SAFETY: the caller passes whole pages of a live reservation that nothing touches until it
    // is committed again; removing access changes no contents.
    unsafe { range_call(nr::MPROTECT, start, len, PROT_NONE) }
}

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
    // SAFETY: passed through from the caller. The calls read and write no user memory through
    // their arguments; what they do to the range itself (madvise may zero it) is what the
    // caller's contract permits.
    result(unsafe {
        syscall(
            number,
            [start.as_ptr(), int(len), int(arg), int(0), int(0), int(0)],
        )
    })
    .map(|_| ())
}

fn membarrier(command: usize, flags: usize, cpu: u32) -> Result<(), OsError> {
    // SAFETY: membarrier takes no pointers and changes no memory; the rseq commands only
    // restart sequences in flight, which every sequence must tolerate anyway.
    result(unsafe {
        syscall(
            nr::MEMBARRIER,
            [
                int(command),
                int(flags),
                int(cpu as usize),
                int(0),
                int(0),
                int(0),
            ],
        )
    })
    .map(|_| ())
}

/// Eight bytes from the kernel's random pool, or `None` if the call fails.
fn getrandom() -> Option<u64> {
    let mut value = 0u64;
    let buffer: Reg = ptr::from_mut(&mut value).cast();
    // SAFETY: getrandom writes at most 8 bytes to `buffer`, a live, exclusively borrowed local
    // of 8 bytes, and changes nothing else. `GRND_INSECURE` keeps it from blocking.
    let written = result(unsafe {
        syscall(
            nr::GETRANDOM,
            [buffer, int(8), int(GRND_INSECURE), int(0), int(0), int(0)],
        )
    });
    match written {
        Ok(n) if n.addr() == 8 => Some(value),
        _ => None,
    }
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

fn unblock_sigabrt() {
    let set: u64 = 1 << (SIGABRT - 1);
    let set: Reg = ptr::from_ref(&set).cast_mut().cast();
    // SAFETY: rt_sigprocmask reads one signal set from `set`, a live local, and writes nothing
    // back (the old-set pointer is null). Unblocking SIGABRT while aborting is ours to do.
    unsafe {
        syscall(
            nr::RT_SIGPROCMASK,
            [
                int(SIG_UNBLOCK),
                set,
                ptr::null_mut(),
                int(SIGSET_SIZE),
                int(0),
                int(0),
            ],
        );
    }
}

fn raise_sigabrt() {
    // SAFETY: getpid and gettid take nothing; tgkill sends SIGABRT to this thread, which runs
    // the disposition the process chose or ends it. Raising it is `abort`'s purpose.
    unsafe {
        let pid = int_call(nr::GETPID, 0, 0, 0);
        let tid = int_call(nr::GETTID, 0, 0, 0);
        int_call(nr::TGKILL, pid, tid, SIGABRT);
    }
}

// SAFETY: each promise of `Os`, against the kernel's documented behaviour (mmap(2), mprotect(2),
// madvise(2), munmap(2), membarrier(2)):
// - `reserve` maps a fresh private anonymous range, which overlaps nothing that exists; the
//   kernel returns it with the provenance of a new allocation through the `syscall` result
//   pointer. It is `PROT_NONE` until committed, and fresh anonymous memory reads as zero. The
//   over-reservation and trim make the start aligned, and only the unused ends are unmapped.
// - `commit` is mprotect to read and write, which keeps contents. `purge` is MADV_DONTNEED,
//   after which private anonymous memory reads as zero and stays mapped with its protection.
//   `advise_huge` is MADV_HUGEPAGE or MADV_NOHUGEPAGE, which change neither. `wipe_on_fork` is
//   MADV_WIPEONFORK, which changes nothing in this process and makes a child's copy of the
//   range read as zero.
// - A failed call reports `-errno`; mprotect and madvise may have acted on part of the range,
//   never beyond it.
// - Reservations are mappings of the process, valid until unmapped; dropping a `LinuxOs`
//   changes nothing.
// - `fence(cpu)` is MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ with MEMBARRIER_CMD_FLAG_CPU, which
//   returns after the thread of the process running on that CPU, if any, has been interrupted
//   out of any rseq critical section; it fails with EPERM unless the process registered.
// - `rseq` reads glibc's process-wide registration facts.
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

    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: the caller passes whole pages of a live reservation; the advice changes
        // nothing in this process.
        unsafe { range_call(nr::MADVISE, start, len, MADV_WIPEONFORK) }
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: the caller passes whole pages of a live reservation that nothing uses again.
        unsafe { range_call(nr::MUNMAP, start, len, 0) }
    }

    fn register_fence(&self) -> Result<(), OsError> {
        membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ, 0, 0)
    }

    fn fence(&self, cpu: u32) -> Result<(), OsError> {
        membarrier(
            MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ,
            MEMBARRIER_CMD_FLAG_CPU,
            cpu,
        )
    }

    fn rseq(&self) -> Option<RseqArea> {
        glibc::registration()
    }

    fn random(&self) -> u64 {
        if let Some(value) = getrandom() {
            return value;
        }
        // No randomness to be had (a seccomp filter, say). A generation only has to differ from
        // the parent's, and the stack address and the process id both change across a fork in
        // practice; a collision costs one CPU's cache its drains, not correctness.
        let local = 0u8;
        // SAFETY: getpid takes nothing and changes nothing.
        let pid = unsafe { int_call(nr::GETPID, 0, 0, 0) };
        let mixed = ptr::from_ref(&local).addr() as u64 ^ (pid as u64).rotate_left(32);
        mixed.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }

    fn abort(&self, message: &str) -> ! {
        write_stderr(b"asmalloc: ");
        write_stderr(message.as_bytes());
        write_stderr(b"\n");
        // First as glibc's abort does: raise SIGABRT with the program's handler in place, so a
        // crash reporter sees it. The heap is still valid; it refused to go on, it is not
        // broken. If the handler returns, or calls back in here (by allocating into the failure
        // that brought us here), restore the default action and raise again, which ends the
        // process with a core dump.
        if !ABORTING.swap(true, Ordering::AcqRel) {
            unblock_sigabrt();
            raise_sigabrt();
        }
        let default = KernelSigaction {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        };
        let action: Reg = ptr::from_ref(&default).cast_mut().cast();
        // SAFETY: rt_sigaction reads one `KernelSigaction` from `action`, a live local, and
        // writes nothing back (the old-action pointer is null). The process is ending, so
        // changing its SIGABRT disposition is ours to do.
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
        }
        unblock_sigabrt();
        raise_sigabrt();
        loop {
            // SAFETY: exit_group takes an integer and ends the process, which is this
            // function's purpose; it is reached only if the signal did not end it.
            unsafe { int_call(nr::EXIT_GROUP, 127, 0, 0) };
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
        unsafe { space.unmap().unwrap() };
    }

    #[test]
    fn glibc_registers_rseq() {
        // The supported platform is glibc 2.43 on Linux 7.2, which always register.
        let area = LinuxOs.rseq().expect("glibc registered rseq");
        assert!(area.size() >= crate::os::rseq::ORIGINAL_SIZE, "{area:?}");
        assert_eq!(area.offset() % 32, 0, "the area is 32-byte aligned");
    }

    #[test]
    fn rseq_fences_aim_at_one_cpu_after_registering() {
        LinuxOs.register_fence().unwrap();
        LinuxOs.fence(0).unwrap();
        // A CPU number no machine has: nothing can be in flight there.
        LinuxOs.fence(8191).unwrap();
    }

    #[test]
    fn random_numbers_differ() {
        let a = LinuxOs.random();
        let b = LinuxOs.random();
        assert_ne!(a, b);
    }

    #[test]
    fn a_page_can_be_marked_to_be_wiped_on_fork() {
        let os = LinuxOs;
        let base = os.reserve(PAGE, PAGE).unwrap();
        // SAFETY: one whole page of a fresh reservation that nothing else uses.
        unsafe {
            os.commit(base, PAGE).unwrap();
            os.wipe_on_fork(base, PAGE).unwrap();
            base.as_ptr().write(7);
            assert_eq!(base.as_ptr().read(), 7, "the parent keeps its contents");
            os.unmap(base, PAGE).unwrap();
        }
    }

    const CHILD: &str = "ASMALLOC_ABORT_CHILD";

    // The test's own way to install a handler: glibc's `signal`, which the test binary links.
    unsafe extern "C" {
        fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
    }

    extern "C" fn returning_handler(_: i32) {
        write_stderr(b"handler ran\n");
    }

    /// Run on its own by `abort_ends_the_process_with_sigabrt`; does nothing otherwise. With
    /// the variable set to `handler`, it first installs a SIGABRT handler that returns.
    #[test]
    fn abort_child() {
        let Some(mode) = std::env::var_os(CHILD) else {
            return;
        };
        if mode == "handler" {
            // SAFETY: installs a handler that only writes to standard error, which is
            // async-signal-safe.
            unsafe { signal(SIGABRT as i32, returning_handler) };
        }
        LinuxOs.abort("test abort");
    }

    fn run_abort_child(mode: &str) -> std::process::Output {
        let exe = std::env::current_exe().unwrap();
        // `ulimit -c 0` keeps the expected crash from leaving a core dump behind.
        Command::new("sh")
            .arg("-c")
            .arg("ulimit -c 0; exec \"$0\" \"$@\"")
            .arg(exe)
            .args([
                "--exact",
                "os::linux::kernel::tests::abort_child",
                "--nocapture",
            ])
            .env(CHILD, mode)
            .output()
            .unwrap()
    }

    #[test]
    fn abort_ends_the_process_with_sigabrt() {
        let output = run_abort_child("plain");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: test abort\n"), "{stderr}");
    }

    #[test]
    fn abort_runs_the_programs_handler_then_ends_the_process_anyway() {
        let output = run_abort_child("handler");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: test abort\n"), "{stderr}");
        assert!(stderr.contains("handler ran\n"), "{stderr}");
    }
}
