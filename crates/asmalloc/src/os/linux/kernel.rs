//! [`LinuxOs`]: the `Os` interface as Linux system calls.
//!
//! Each method is one or a few system calls; the arguments come from the `Os` contract and the
//! kernel's documentation. The ledger entry for the unsafe blocks here is
//! `docs/safety/asmalloc/os/linux/kernel-safety.md`.

use super::syscall::{Reg, int, nr, result, syscall};
use crate::os::{HugePages, Os, OsError, Topology};
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
/// `openat`'s directory argument for a path relative to the working directory, and its flags
/// for reading a file that no `exec` should inherit.
const AT_FDCWD: usize = -100isize as usize;
const O_RDONLY: usize = 0;
const O_CLOEXEC: usize = 0o2_000_000;
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
    // SAFETY: KERNEL-01. passed through from the caller. The calls read and write no user memory through
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
    // SAFETY: KERNEL-02. membarrier takes no pointers and changes no memory; the rseq commands only
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
    // SAFETY: KERNEL-05. getrandom writes at most 8 bytes to `buffer`, a live, exclusively borrowed local
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

/// Reads the whole file at `path`, a NUL-terminated string, into `buf`. The bytes read, or
/// `None` if the file cannot be opened or read or does not fit in `buf`.
fn read_file(path: &[u8], buf: &mut [u8]) -> Option<usize> {
    if path.last() != Some(&0) {
        return None;
    }
    let name: Reg = path.as_ptr().cast_mut();
    // SAFETY: KERNEL-10. openat reads the NUL-terminated string at `name`, a live slice whose
    // last byte is the NUL, and opens a new descriptor that only this function uses.
    let fd = result(unsafe {
        syscall(
            nr::OPENAT,
            [
                int(AT_FDCWD),
                name,
                int(O_RDONLY | O_CLOEXEC),
                int(0),
                int(0),
                int(0),
            ],
        )
    })
    .ok()?
    .addr();
    let mut len = 0;
    let whole = loop {
        let Some(rest) = buf.get_mut(len..).filter(|rest| !rest.is_empty()) else {
            break false;
        };
        let into: Reg = rest.as_mut_ptr();
        // SAFETY: KERNEL-10. read writes at most `rest.len()` bytes to `into`, the start of a
        // live, exclusively borrowed slice of that length, from the descriptor opened above.
        let read = result(unsafe {
            syscall(
                nr::READ,
                [int(fd), into, int(rest.len()), int(0), int(0), int(0)],
            )
        });
        match read {
            Ok(n) if n.addr() == 0 => break true,
            Ok(n) => len += n.addr().min(rest.len()),
            Err(_) => break false,
        }
    };
    // SAFETY: KERNEL-10. close releases the descriptor opened above, which nothing else holds.
    let _ = unsafe { int_call(nr::CLOSE, fd, 0, 0) };
    whole.then_some(len)
}

/// Appends `bytes` to `path` at `at`; `false` if they do not fit.
fn append(path: &mut [u8], at: &mut usize, bytes: &[u8]) -> bool {
    match path.get_mut(*at..*at + bytes.len()) {
        Some(room) => {
            room.copy_from_slice(bytes);
            *at += bytes.len();
            true
        }
        None => false,
    }
}

/// Appends the decimal digits of `n` to `path` at `at`; `false` if they do not fit.
fn append_number(path: &mut [u8], at: &mut usize, n: u32) -> bool {
    let mut digits = [0u8; 10];
    let mut count = 0;
    let mut rest = n;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    digits[..count].reverse();
    append(path, at, &digits[..count])
}

/// Writes all of `bytes` to standard error, giving up on any error.
fn write_stderr(mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let buffer: Reg = bytes.as_ptr().cast_mut();
        // SAFETY: KERNEL-04. write reads `bytes.len()` bytes from `buffer`, which is a live slice.
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
    // SAFETY: KERNEL-06. passed through from the caller.
    unsafe { syscall(number, [int(a), int(b), int(c), int(0), int(0), int(0)]) }.addr()
}

/// Sends SIGABRT to the calling thread, which runs the disposition in force.
fn raise_sigabrt() {
    // SAFETY: KERNEL-06. getpid and gettid take nothing and change nothing. tgkill sends SIGABRT to this
    // thread, which runs the disposition the process chose or ends it; raising it is `abort`'s
    // purpose.
    unsafe {
        let pid = int_call(nr::GETPID, 0, 0, 0);
        let tid = int_call(nr::GETTID, 0, 0, 0);
        int_call(nr::TGKILL, pid, tid, SIGABRT);
    }
}

/// Resets SIGABRT to its default action, which ends the process with a core dump.
fn default_sigabrt() {
    let default = KernelSigaction {
        handler: SIG_DFL,
        flags: 0,
        restorer: 0,
        mask: 0,
    };
    let action: Reg = ptr::from_ref(&default).cast_mut().cast();
    // SAFETY: KERNEL-06. rt_sigaction reads one `KernelSigaction` from `action`, a live local, and writes
    // nothing back (the old-action pointer is null). The process is ending, so changing its
    // SIGABRT disposition is ours to do.
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
}

fn unblock_sigabrt() {
    let set: u64 = 1 << (SIGABRT - 1);
    let set: Reg = ptr::from_ref(&set).cast_mut().cast();
    // SAFETY: KERNEL-06. rt_sigprocmask reads one signal set from `set`, a live local, and writes nothing
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

// SAFETY: KERNEL-07. each promise of `Os`, against the kernel's documented behaviour (mmap(2), mprotect(2),
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
// - `ticks` is the timestamp counter, which the trait asks nothing of.
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
        // SAFETY: KERNEL-03. an anonymous mapping at an address of the kernel's choosing replaces nothing
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
            // SAFETY: KERNEL-03. `mapped..start` is the unused head of the fresh mapping.
            let _ = unsafe { range_call(nr::MUNMAP, mapped, lead, 0) };
        }
        if tail != 0 {
            let end = start.map_addr(|a| a.saturating_add(len));
            // SAFETY: KERNEL-03. `end..end + tail` is the unused tail of the fresh mapping.
            let _ = unsafe { range_call(nr::MUNMAP, end, tail, 0) };
        }
        Ok(start)
    }

    unsafe fn commit(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: KERNEL-03. the caller passes whole pages of a live reservation; making them writable
        // changes no contents and nothing else relies on them being inaccessible.
        unsafe { range_call(nr::MPROTECT, start, len, PROT_READ | PROT_WRITE) }
    }

    unsafe fn purge(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: KERNEL-03. the caller passes whole pages of a live reservation that nothing uses during
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
        // SAFETY: KERNEL-03. the caller passes whole pages of a live reservation; the advice changes no
        // contents or permissions.
        unsafe { range_call(nr::MADVISE, start, len, advice) }
    }

    unsafe fn wipe_on_fork(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: KERNEL-03. the caller passes whole pages of a live reservation; the advice changes
        // nothing in this process.
        unsafe { range_call(nr::MADVISE, start, len, MADV_WIPEONFORK) }
    }

    unsafe fn unmap(&self, start: NonNull<u8>, len: usize) -> Result<(), OsError> {
        // SAFETY: KERNEL-03. the caller passes whole pages of a live reservation that nothing uses again.
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

    fn random(&self) -> u64 {
        if let Some(value) = getrandom() {
            return value;
        }
        // No randomness to be had (a seccomp filter, say). A generation only has to differ from
        // the parent's, and the stack address and the process id both change across a fork in
        // practice; a collision costs one CPU's cache its drains, not correctness.
        let local = 0u8;
        // SAFETY: KERNEL-05. getpid takes nothing and changes nothing.
        let pid = unsafe { int_call(nr::GETPID, 0, 0, 0) };
        let mixed = ptr::from_ref(&local).addr() as u64 ^ (pid as u64).rotate_left(32);
        mixed.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }

    fn ticks(&self) -> u64 {
        // SAFETY: KERNEL-09.
        unsafe { core::arch::x86_64::_rdtsc() }
    }

    fn topology(&self, which: Topology, buf: &mut [u8]) -> Option<usize> {
        let mut path = [0u8; 96];
        let mut at = 0;
        let built = match which {
            Topology::Online => append(&mut path, &mut at, b"/sys/devices/system/cpu/online\0"),
            Topology::SharedL3(cpu) => {
                append(&mut path, &mut at, b"/sys/devices/system/cpu/cpu")
                    && append_number(&mut path, &mut at, cpu)
                    && append(&mut path, &mut at, b"/cache/index3/shared_cpu_list\0")
            }
        };
        if !built {
            return None;
        }
        read_file(&path[..at], buf)
    }

    fn abort(&self, message: &str) -> ! {
        write_stderr(b"asmalloc: ");
        write_stderr(message.as_bytes());
        write_stderr(b"\n");
        // First as glibc's abort does: raise SIGABRT with the program's handler in place, so a
        // crash reporter sees it. The heap is still valid; it refused to go on, it is not broken.
        // If the handler returns, restore the default action and raise again, which ends the
        // process with a core dump. SIGABRT is left blocked for the first raise if it is
        // blocked, as it is inside the program's own SIGABRT handler: an abort from there stays
        // pending until the default action is back, instead of running the handler again.
        // Another thread that aborts meanwhile may end the process under this thread's handler.
        raise_sigabrt();
        default_sigabrt();
        unblock_sigabrt();
        raise_sigabrt();
        loop {
            // SAFETY: KERNEL-06. exit_group takes an integer and ends the process, which is this
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
    fn rseq_fences_aim_at_one_cpu_after_registering() {
        LinuxOs.register_fence().unwrap();
        LinuxOs.fence(0).unwrap();
        // A CPU number no machine has: nothing can be in flight there.
        LinuxOs.fence(8191).unwrap();
    }

    #[test]
    fn ticks_rise() {
        let first = LinuxOs.ticks();
        std::thread::sleep(std::time::Duration::from_millis(1));
        assert!(LinuxOs.ticks() > first);
    }

    #[test]
    fn random_numbers_differ() {
        let a = LinuxOs.random();
        let b = LinuxOs.random();
        assert_ne!(a, b);
    }

    /// The behaviour the simulator models (review R2): a mark survives a second commit, and a
    /// page can be marked before it is first committed.
    #[test]
    fn marked_pages_read_as_zero_in_a_forked_child() {
        unsafe extern "C" {
            fn fork() -> i32;
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
            fn _exit(code: i32) -> !;
        }
        let os = LinuxOs;
        let base = os.reserve(2 * PAGE, PAGE).unwrap();
        let second = base.map_addr(|a| a.saturating_add(PAGE));
        // SAFETY: whole pages of a fresh reservation that nothing else uses. The child only
        // reads two bytes and exits, which is safe after fork in a threaded process.
        unsafe {
            os.commit(base, PAGE).unwrap();
            os.wipe_on_fork(base, PAGE).unwrap();
            os.commit(base, PAGE).unwrap();
            os.wipe_on_fork(second, PAGE).unwrap();
            os.commit(second, PAGE).unwrap();
            base.as_ptr().write(7);
            second.as_ptr().write(9);
            let pid = fork();
            if pid == 0 {
                _exit(i32::from(base.as_ptr().read()) | i32::from(second.as_ptr().read()) << 4);
            }
            let mut status = 0;
            assert_eq!(waitpid(pid, &mut status, 0), pid);
            assert_eq!(
                (status >> 8) & 0xff,
                0,
                "both pages read as zero in the child"
            );
            assert_eq!(base.as_ptr().read(), 7, "the parent keeps its contents");
            os.unmap(base, 2 * PAGE).unwrap();
        }
    }

    const CHILD: &str = "ASMALLOC_ABORT_CHILD";
    /// How long a child process may run before the test calls it hung.
    const CHILD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

    // The test's own way to install a handler: glibc's `signal`, which the test binary links.
    unsafe extern "C" {
        fn signal(signal: i32, handler: extern "C" fn(i32)) -> usize;
    }

    extern "C" fn returning_handler(_: i32) {
        write_stderr(b"handler ran\n");
    }

    /// A handler that aborts again, as one that allocates into the failure that brought it
    /// here would.
    extern "C" fn aborting_handler(_: i32) {
        write_stderr(b"handler start\n");
        LinuxOs.abort("nested abort");
    }

    /// Run on its own by the abort tests below; does nothing otherwise. The variable names the
    /// handler that is installed.
    #[test]
    fn abort_child() {
        let Some(mode) = std::env::var_os(CHILD) else {
            return;
        };
        let handler: Option<extern "C" fn(i32)> = match mode.to_str() {
            Some("plain") => None,
            Some("handler") => Some(returning_handler),
            Some("nested") => Some(aborting_handler),
            _ => panic!("unknown abort case {mode:?}"),
        };
        if let Some(handler) = handler {
            // SAFETY: each handler only writes to standard error through system calls, or
            // aborts; none allocates.
            unsafe { signal(SIGABRT as i32, handler) };
        }
        LinuxOs.abort("test abort");
    }

    /// Runs `abort_child` in a child process with `mode`, killing it if it outlives
    /// `CHILD_DEADLINE`, so that a hang fails the test.
    fn run_abort_child(mode: &str) -> std::process::Output {
        use std::process::Stdio;
        let exe = std::env::current_exe().unwrap();
        // `ulimit -c 0` keeps the expected crash from leaving a core dump behind.
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("ulimit -c 0; exec \"$0\" \"$@\"")
            .arg(exe)
            .args([
                "--exact",
                "os::linux::kernel::tests::abort_child",
                "--nocapture",
            ])
            .env(CHILD, mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > CHILD_DEADLINE {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("abort case {mode} hung: {output:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    }

    #[test]
    fn abort_ends_the_process_with_sigabrt() {
        let output = run_abort_child("plain");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: test abort\n"), "{stderr}");
    }

    /// An abort from inside the program's SIGABRT handler ends the process instead of running
    /// the handler again.
    #[test]
    fn an_abort_inside_the_handler_ends_the_process() {
        let output = run_abort_child("nested");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: nested abort\n"), "{stderr}");
        assert_eq!(stderr.matches("handler start\n").count(), 1, "{stderr}");
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
