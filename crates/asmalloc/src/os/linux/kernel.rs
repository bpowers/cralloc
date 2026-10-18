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
use core::sync::atomic::{AtomicU64, Ordering};

/// The thread that is running the program's SIGABRT handler for an allocator abort, as an
/// [`Aborter`] word, or zero (no process or thread has ID zero) before any thread has aborted.
/// See [`claim`] for who may replace it. A fork copies it, which is why it names the process.
static ABORTING: AtomicU64 = AtomicU64::new(0);

/// How long a later aborter sleeps between looks at the first one.
const WAIT_STEP_NS: u64 = 10_000_000;
/// How many steps a later aborter waits for the first one's handler before it ends the process
/// itself: about ten seconds, long enough for a crash reporter to write its report, and a bound
/// on the hang if that handler is blocked on something the waiting thread holds.
const PATIENCE: u32 = 1000;
const ESRCH: i32 = 3;
const EINTR: i32 = 4;

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

/// A thread, named by its process and thread IDs as this process's PID namespace sees them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Aborter {
    pid: u32,
    tid: u32,
}

impl Aborter {
    fn current() -> Aborter {
        // SAFETY: getpid and gettid take nothing and change nothing.
        let (pid, tid) = unsafe { (int_call(nr::GETPID, 0, 0, 0), int_call(nr::GETTID, 0, 0, 0)) };
        // Process and thread IDs are positive and at most 2^22 (the kernel's PID_MAX_LIMIT).
        Aborter {
            pid: pid as u32,
            tid: tid as u32,
        }
    }

    fn word(self) -> u64 {
        u64::from(self.pid) << 32 | u64::from(self.tid)
    }

    fn from_word(word: u64) -> Option<Aborter> {
        (word != 0).then_some(Aborter {
            pid: (word >> 32) as u32,
            tid: word as u32,
        })
    }

    /// Whether this process still has the thread (tgkill with signal 0 only checks). Only
    /// ESRCH counts as gone; any other failure counts as alive, which costs a bounded wait.
    /// An exited main thread whose process has other threads left is still there as a zombie.
    fn is_alive(self) -> bool {
        // SAFETY: tgkill with signal 0 sends nothing; it checks the target and changes nothing.
        let ret = unsafe {
            syscall(
                nr::TGKILL,
                [
                    int(self.pid as usize),
                    int(self.tid as usize),
                    int(0),
                    int(0),
                    int(0),
                    int(0),
                ],
            )
        };
        !matches!(result(ret), Err(error) if error.errno() == ESRCH)
    }

    fn raise_sigabrt(self) {
        // SAFETY: tgkill sends SIGABRT to this thread, which runs the disposition the process
        // chose or ends it. Raising it is `abort`'s purpose.
        unsafe { int_call(nr::TGKILL, self.pid as usize, self.tid as usize, SIGABRT) };
    }
}

/// What a thread that is aborting does about the entry it finds in [`ABORTING`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// No live aborter in this process: replace the entry and run the program's handler.
    Claim,
    /// The entry is this thread: its own handler called back in. Go to the default action.
    Nested,
    /// Another live thread of this process is aborting: wait for it to end the process.
    Wait,
}

/// The rule for an entry, given who is asking and whether a thread of this process is alive.
/// An entry from another process (a fork copied it, or a parent sharing memory wrote it) and
/// an entry naming a thread this process no longer has are stale: nothing will act on them.
fn judge(entry: u64, me: Aborter, alive: impl Fn(Aborter) -> bool) -> Verdict {
    match Aborter::from_word(entry) {
        None => Verdict::Claim,
        Some(first) if first == me => Verdict::Nested,
        Some(first) if first.pid != me.pid || !alive(first) => Verdict::Claim,
        Some(_) => Verdict::Wait,
    }
}

/// Decides whether this thread runs the program's SIGABRT handler (true) or goes straight to
/// the default action (false), after waiting at most `patience` steps for a live first aborter.
///
/// Each change to `ABORTING` is one compare-and-swap from the entry the caller judged, so a
/// stale entry is replaced once and a live one never. A failed swap means another aborting
/// thread changed the entry; each thread succeeds at most once, so the retries are bounded by
/// the number of threads aborting.
fn claim(me: Aborter, patience: u32) -> bool {
    let mut entry = ABORTING.load(Ordering::Acquire);
    let mut waited = 0;
    loop {
        match judge(entry, me, Aborter::is_alive) {
            Verdict::Nested => return false,
            Verdict::Claim => {
                match ABORTING.compare_exchange(
                    entry,
                    me.word(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return true,
                    Err(now) => entry = now,
                }
            }
            Verdict::Wait => {
                if waited >= patience {
                    return false;
                }
                sleep_step();
                waited += 1;
                entry = ABORTING.load(Ordering::Acquire);
            }
        }
    }
}

/// Sleeps for `WAIT_STEP_NS`, resuming the remainder after a signal handler interrupts it so
/// that signals cannot shorten the wait.
fn sleep_step() {
    let mut request = [0i64, WAIT_STEP_NS as i64];
    loop {
        let mut remaining = [0i64; 2];
        let req: Reg = ptr::from_ref(&request).cast_mut().cast();
        let rem: Reg = ptr::from_mut(&mut remaining).cast();
        // SAFETY: nanosleep reads one `struct timespec` (two 64-bit words on x86-64) from
        // `req`, a live local, and on EINTR writes one to `rem`, a live, exclusively borrowed
        // local of the same size. It changes nothing else.
        let ret = unsafe { syscall(nr::NANOSLEEP, [req, rem, int(0), int(0), int(0), int(0)]) };
        match result(ret) {
            Err(error) if error.errno() == EINTR => request = remaining,
            _ => return,
        }
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

    fn ticks(&self) -> u64 {
        // SAFETY: KERNEL-09.
        unsafe { core::arch::x86_64::_rdtsc() }
    }

    fn abort(&self, message: &str) -> ! {
        abort_with(message, PATIENCE)
    }
}

/// [`Os::abort`], with the number of steps a later aborter waits for the first one.
fn abort_with(message: &str, patience: u32) -> ! {
    write_stderr(b"asmalloc: ");
    write_stderr(message.as_bytes());
    write_stderr(b"\n");
    // First as glibc's abort does: raise SIGABRT with the program's handler in place, so a
    // crash reporter sees it. The heap is still valid; it refused to go on, it is not broken.
    // If the handler returns, or calls back in here (by allocating into the failure that
    // brought us here), restore the default action and raise again, which ends the process
    // with a core dump. Another thread that aborts meanwhile waits a while first: resetting the
    // disposition under a handler still running would end the process before the handler
    // could finish its report.
    let me = Aborter::current();
    if claim(me, patience) {
        unblock_sigabrt();
        me.raise_sigabrt();
    }
    let default = KernelSigaction {
        handler: SIG_DFL,
        flags: 0,
        restorer: 0,
        mask: 0,
    };
    let action: Reg = ptr::from_ref(&default).cast_mut().cast();
    // SAFETY: rt_sigaction reads one `KernelSigaction` from `action`, a live local, and writes
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
    unblock_sigabrt();
    me.raise_sigabrt();
    loop {
        // SAFETY: exit_group takes an integer and ends the process, which is this function's
        // purpose; it is reached only if the signal did not end it.
        unsafe { int_call(nr::EXIT_GROUP, 127, 0, 0) };
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

    #[test]
    fn the_rule_for_an_entry_covers_every_case() {
        let me = Aborter { pid: 10, tid: 11 };
        let unasked = |_: Aborter| -> bool { panic!("no liveness check is needed") };
        assert_eq!(judge(0, me, unasked), Verdict::Claim, "no one is aborting");
        assert_eq!(judge(me.word(), me, unasked), Verdict::Nested);
        let parent = Aborter { pid: 9, tid: 12 };
        assert_eq!(
            judge(parent.word(), me, unasked),
            Verdict::Claim,
            "an entry a fork copied from another process is stale"
        );
        let sibling = Aborter { pid: 10, tid: 12 };
        assert_eq!(judge(sibling.word(), me, |_| false), Verdict::Claim);
        assert_eq!(judge(sibling.word(), me, |_| true), Verdict::Wait);
        assert_eq!(Aborter::from_word(sibling.word()), Some(sibling));
    }

    #[test]
    fn liveness_is_asked_of_this_process() {
        let me = Aborter::current();
        assert!(me.is_alive());
        let gone = std::thread::spawn(Aborter::current).join().unwrap();
        assert_eq!(gone.pid, me.pid);
        // Join returns when the kernel clears the thread's ID word, a little before it releases
        // the thread; `claim` judges again after every step for the same reason.
        let started = std::time::Instant::now();
        while gone.is_alive() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "a thread that exited is gone"
            );
            std::thread::yield_now();
        }
        let parent = std::os::unix::process::parent_id();
        let elsewhere = Aborter {
            pid: me.pid,
            tid: parent,
        };
        assert!(
            !elsewhere.is_alive(),
            "another process's thread is not ours"
        );
    }

    /// The claim in this process, without aborting: a stale entry is replaced, the claimant's
    /// own entry reads as nested, and another thread waits out its patience while the claimant
    /// lives and leaves the entry alone. Only abort tests use `ABORTING`, and they run in
    /// child processes.
    #[test]
    fn a_claim_replaces_a_stale_entry_and_waits_for_a_live_one() {
        let me = Aborter::current();
        let parent = std::os::unix::process::parent_id();
        let stale = Aborter {
            pid: parent,
            tid: parent,
        };
        ABORTING.store(stale.word(), Ordering::Release);
        assert!(claim(me, 0), "a stale entry is claimed");
        assert_eq!(ABORTING.load(Ordering::Acquire), me.word());
        assert!(!claim(me, 0), "the claimant calling again is nested");
        let other = std::thread::spawn(|| {
            let started = std::time::Instant::now();
            (claim(Aborter::current(), 3), started.elapsed())
        });
        let (claimed, waited) = other.join().unwrap();
        assert!(!claimed, "a second thread does not claim a live entry");
        assert!(
            waited >= std::time::Duration::from_nanos(3 * WAIT_STEP_NS),
            "it waited out its patience: {waited:?}"
        );
        assert_eq!(ABORTING.load(Ordering::Acquire), me.word());
        ABORTING.store(0, Ordering::Release);
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

    /// Set by the handlers below once they have started, for the second aborting thread.
    static HANDLING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    /// The second aborting thread's ID, for the handler that watches it.
    static SECOND: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

    /// Starts a thread that aborts with `patience` once a handler is running. It waits by
    /// yielding, not sleeping, so that a thread of this process in nanosleep is in `claim`.
    fn abort_from_a_second_thread(patience: u32) {
        std::thread::spawn(move || {
            while !HANDLING.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            SECOND.store(Aborter::current().tid, Ordering::Release);
            abort_with("second thread", patience);
        });
    }

    /// Whether thread `tid` of this process is in nanosleep, from `/proc/self/task/<tid>/syscall`,
    /// read without allocating since this runs in a signal handler.
    fn sleeping(tid: u32) -> bool {
        unsafe extern "C" {
            fn open(path: *const core::ffi::c_char, flags: i32, ...) -> i32;
            fn read(fd: i32, buffer: *mut core::ffi::c_void, len: usize) -> isize;
            fn close(fd: i32) -> i32;
        }
        let mut path = [0u8; 48];
        let mut len = 0;
        let mut push = |bytes: &[u8]| {
            path[len..len + bytes.len()].copy_from_slice(bytes);
            len += bytes.len();
        };
        push(b"/proc/self/task/");
        let mut digits = [0u8; 10];
        let mut n = tid;
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        push(&digits[at..]);
        push(b"/syscall\0");
        let mut contents = [0u8; 16];
        // SAFETY: the path is NUL-terminated and the buffer is a live local of its length.
        let got = unsafe {
            let fd = open(path.as_ptr().cast(), 0);
            if fd < 0 {
                return false;
            }
            let got = read(fd, contents.as_mut_ptr().cast(), contents.len());
            close(fd);
            got
        };
        // The file starts with the number of the system call the thread is blocked in.
        const _: () = assert!(nr::NANOSLEEP == 35);
        got > 0 && contents.starts_with(b"35 ")
    }

    /// Lets the second thread reach its wait, then finishes. If the second thread did not wait
    /// but reset the disposition, the process ends before "handler ran".
    extern "C" fn watching_handler(_: i32) {
        write_stderr(b"handler start\n");
        HANDLING.store(true, Ordering::Release);
        for _ in 0..1000 {
            let second = SECOND.load(Ordering::Acquire);
            if second != 0 && sleeping(second) {
                write_stderr(b"second waited\n");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        write_stderr(b"handler ran\n");
    }

    /// A handler that never finishes, as one blocked on a lock the second thread holds.
    extern "C" fn blocked_handler(_: i32) {
        write_stderr(b"handler start\n");
        HANDLING.store(true, Ordering::Release);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    static FORKED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

    /// A crash handler that forks, as reporters do, and whose child then meets an allocator
    /// abort of its own. The child's copy of the handler returns at once, so a child that can
    /// abort ends with SIGABRT.
    extern "C" fn forking_handler(_: i32) {
        unsafe extern "C" {
            fn fork() -> i32;
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
            fn kill(pid: i32, signal: i32) -> i32;
        }
        const WNOHANG: i32 = 1;
        if FORKED.swap(true, Ordering::AcqRel) {
            return;
        }
        // SAFETY: fork, waitpid and kill are async-signal-safe; the child only aborts.
        unsafe {
            let pid = fork();
            if pid == 0 {
                LinuxOs.abort("child abort");
            }
            let mut status = 0;
            for _ in 0..100 {
                if waitpid(pid, &mut status, WNOHANG) == pid {
                    write_stderr(b"child ended\n");
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            write_stderr(b"child parked\n");
            kill(pid, 9);
            waitpid(pid, &mut status, 0);
        }
    }

    /// Finding abort-parks-forked-child (KERNEL-06). `ABORTING` is a static that a fork
    /// copies. A child forked while a thread of the parent is aborting inherits the parent's
    /// entry there. Here the child is forked by the SIGABRT handler itself; a child forked by
    /// any other thread during the handler has the same static, and outlives the parent with
    /// it. The child must see the entry as another process's and abort normally.
    #[test]
    fn an_abort_in_a_child_forked_during_an_abort_ends_the_child() {
        let output = run_abort_child("fork");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: child abort\n"), "{stderr}");
        assert!(
            stderr.contains("child ended\n"),
            "the child never ended: {stderr}"
        );
    }

    /// Run on its own by the abort tests below; does nothing otherwise. The variable names the
    /// case: which handler is installed and what `ABORTING` holds before the abort.
    #[test]
    fn abort_child() {
        let Some(mode) = std::env::var_os(CHILD) else {
            return;
        };
        let handler: Option<extern "C" fn(i32)> = match mode.to_str() {
            Some("plain") => None,
            Some("handler") => Some(returning_handler),
            Some("fork") => Some(forking_handler),
            Some("race") => {
                abort_from_a_second_thread(PATIENCE);
                Some(watching_handler)
            }
            Some("blocked") => {
                abort_from_a_second_thread(20);
                Some(blocked_handler)
            }
            Some("stale-thread") => {
                let gone = std::thread::spawn(Aborter::current).join().unwrap();
                ABORTING.store(gone.word(), Ordering::Release);
                Some(returning_handler)
            }
            Some("stale-process") => {
                let parent = std::os::unix::process::parent_id();
                let elsewhere = Aborter {
                    pid: parent,
                    tid: parent,
                };
                ABORTING.store(elsewhere.word(), Ordering::Release);
                Some(returning_handler)
            }
            _ => panic!("unknown abort case {mode:?}"),
        };
        if let Some(handler) = handler {
            // SAFETY: each handler only writes to standard error, sleeps, reads a file in
            // /proc, or forks and waits, all through system calls; none allocates.
            unsafe { signal(SIGABRT as i32, handler) };
        }
        LinuxOs.abort("test abort");
    }

    /// Runs `abort_child` in a child process with `mode`, killing it if it outlives
    /// `CHILD_DEADLINE`, which is how a regression to an unbounded park shows.
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

    /// Review R9: a second thread that aborts while the first one's handler runs must not end
    /// the process under that handler. The handler finishes only once it has seen the second
    /// thread waiting.
    #[test]
    fn a_second_aborting_thread_lets_the_first_handler_finish() {
        let output = run_abort_child("race");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("asmalloc: second thread\n"), "{stderr}");
        assert!(
            stderr.contains("second waited\n"),
            "the second thread never waited: {stderr}"
        );
        assert!(
            stderr.contains("handler ran\n"),
            "the handler was cut short: {stderr}"
        );
    }

    /// If the first aborter's handler never finishes (blocked, say, on a lock the second
    /// thread holds), the second thread ends the process itself once its patience is spent.
    #[test]
    fn a_second_aborting_thread_ends_the_process_if_the_first_handler_blocks() {
        let output = run_abort_child("blocked");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("handler start\n"), "{stderr}");
        assert!(stderr.contains("asmalloc: second thread\n"), "{stderr}");
        assert!(!stderr.contains("handler ran\n"), "{stderr}");
    }

    /// An entry naming a thread this process no longer has is claimed: the program's handler
    /// runs and the process ends.
    #[test]
    fn an_abort_claims_an_entry_whose_thread_is_gone() {
        let output = run_abort_child("stale-thread");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("handler ran\n"), "{stderr}");
    }

    /// An entry from another process is claimed, whatever that process's threads are doing.
    #[test]
    fn an_abort_claims_an_entry_from_another_process() {
        let output = run_abort_child("stale-process");
        assert_eq!(output.status.signal(), Some(SIGABRT as i32), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("handler ran\n"), "{stderr}");
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
