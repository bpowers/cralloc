//! The system calls that register a thread's rseq area, for a C library that registers none
//! (musl): `rseq` to register and unregister, `arch_prctl` to find the thread pointer, and
//! `rt_sigprocmask` to keep signal handlers out while a thread retires its area.
//!
//! What to register, when, and what the area's `cpu_id` then means is `front::area`'s; this
//! module only makes the calls. The ledger entry for the unsafe blocks here is
//! `docs/safety/cralloc/os/linux/rseq-safety.md`.

use super::syscall::{Reg, int, result, syscall};
use crate::os::OsError;
use core::ptr::{self, NonNull};

/// `rseq` and `arch_prctl` on x86-64 (`arch/x86/entry/syscalls/syscall_64.tbl`).
const RSEQ: usize = 334;
const ARCH_PRCTL: usize = 158;
const RT_SIGPROCMASK: usize = 14;
/// `arch_prctl` code: read the FS base, the thread pointer (`asm/prctl.h`).
const ARCH_GET_FS: usize = 0x1003;
/// `rseq` flag: unregister (`RSEQ_FLAG_UNREGISTER`, `include/uapi/linux/rseq.h`).
const RSEQ_FLAG_UNREGISTER: usize = 1;
const SIG_SETMASK: usize = 2;
/// The kernel's `sigset_t` is one 64-bit word on x86-64.
const SIGSET_SIZE: usize = 8;

/// `EBUSY`: `rseq` was asked to register what the thread has registered already.
pub(crate) const EBUSY: OsError = OsError::from_errno(16);

/// The calling thread's thread pointer: the FS base, at which its static thread-local data
/// ends (System V AMD64 psABI, "Thread-Local Storage").
pub(crate) fn thread_pointer() -> Option<NonNull<u8>> {
    let mut base: *mut u8 = ptr::null_mut();
    let out: Reg = ptr::from_mut(&mut base).cast();
    // SAFETY: RSEQ-01.
    result(unsafe {
        syscall(
            ARCH_PRCTL,
            [int(ARCH_GET_FS), out, int(0), int(0), int(0), int(0)],
        )
    })
    .ok()?;
    NonNull::new(base)
}

/// Registers `area`, `len` bytes, as the calling thread's rseq area, with `signature`.
///
/// # Safety
///
/// `area` is the calling thread's own, aligned to 32 bytes, at least `len` bytes, laid out as
/// `struct rseq`, and stays valid until the thread unregisters it or ends: the kernel writes it
/// whenever the thread returns to user space. Code that the area's `rseq_cs` may name stays
/// mapped as long.
pub(crate) unsafe fn register(
    area: NonNull<u8>,
    len: usize,
    signature: u32,
) -> Result<(), OsError> {
    // SAFETY: RSEQ-02; the caller keeps the area valid.
    result(unsafe { rseq(area, len, 0, signature) }).map(|_| ())
}

/// Unregisters `area`, which the calling thread registered with `len` and `signature`.
pub(crate) fn unregister(area: NonNull<u8>, len: usize, signature: u32) -> Result<(), OsError> {
    // SAFETY: RSEQ-03.
    result(unsafe { rseq(area, len, RSEQ_FLAG_UNREGISTER, signature) }).map(|_| ())
}

/// The `rseq` call.
///
/// # Safety
///
/// As [`register`] for a registration; an unregistration has no requirement the kernel does
/// not check.
unsafe fn rseq(area: NonNull<u8>, len: usize, flags: usize, signature: u32) -> Reg {
    // SAFETY: passed through from the callers (RSEQ-02, RSEQ-03).
    unsafe {
        syscall(
            RSEQ,
            [
                area.as_ptr(),
                int(len),
                int(flags),
                int(signature as usize),
                int(0),
                int(0),
            ],
        )
    }
}

/// Blocks every signal the kernel lets a thread block for the calling thread, and returns the
/// mask it had, for [`restore_signals`]; `None` if the call failed and nothing changed.
pub(crate) fn block_signals() -> Option<u64> {
    set_mask(u64::MAX)
}

/// Restores a mask that [`block_signals`] returned.
pub(crate) fn restore_signals(mask: u64) {
    set_mask(mask);
}

fn set_mask(mask: u64) -> Option<u64> {
    let mut old: u64 = 0;
    let new: Reg = ptr::from_ref(&mask).cast_mut().cast();
    let out: Reg = ptr::from_mut(&mut old).cast();
    // SAFETY: RSEQ-04.
    result(unsafe {
        syscall(
            RT_SIGPROCMASK,
            [int(SIG_SETMASK), new, out, int(SIGSET_SIZE), int(0), int(0)],
        )
    })
    .ok()?;
    Some(old)
}
