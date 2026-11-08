//! The one system-call instruction in the allocator, and on aarch64 the registers the OS
//! interface reads, which Rust has no stable way to name.
//!
//! Every system call goes through [`syscall`]. Arguments and results travel as pointers, as in
//! rustix's raw Linux backend, so that a pointer argument keeps its provenance into the kernel
//! and an address the kernel returns, from `mmap`, comes back as a pointer usable for the memory
//! it names, with no integer-to-pointer cast. The `asm!` blocks are the exception to the rule
//! that all assembly comes from Lean; their ledger entries are in
//! `docs/safety/cralloc/os/linux/syscall-safety.md`.

use crate::os::OsError;
use core::arch::asm;
use core::ptr;

/// A system-call argument or result register.
pub(super) type Reg = *mut u8;

/// An integer argument.
#[inline(always)]
pub(super) const fn int(value: usize) -> Reg {
    ptr::without_provenance_mut(value)
}

/// System-call numbers on x86-64 (`arch/x86/entry/syscalls/syscall_64.tbl`).
#[cfg(target_arch = "x86_64")]
pub(super) mod nr {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
    pub const CLOSE: usize = 3;
    pub const MMAP: usize = 9;
    pub const MPROTECT: usize = 10;
    pub const MUNMAP: usize = 11;
    pub const RT_SIGACTION: usize = 13;
    pub const RT_SIGPROCMASK: usize = 14;
    pub const MINCORE: usize = 27;
    pub const MADVISE: usize = 28;
    pub const GETPID: usize = 39;
    pub const PRCTL: usize = 157;
    pub const FUTEX: usize = 202;
    pub const GETTID: usize = 186;
    pub const EXIT_GROUP: usize = 231;
    pub const TGKILL: usize = 234;
    pub const OPENAT: usize = 257;
    pub const GETRANDOM: usize = 318;
    pub const MEMBARRIER: usize = 324;
    #[cfg(target_env = "musl")]
    pub const RSEQ: usize = 334;
}

/// System-call numbers on aarch64, the generic table (`include/uapi/asm-generic/unistd.h`).
#[cfg(target_arch = "aarch64")]
pub(super) mod nr {
    pub const OPENAT: usize = 56;
    pub const CLOSE: usize = 57;
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const EXIT_GROUP: usize = 94;
    pub const FUTEX: usize = 98;
    pub const TGKILL: usize = 131;
    pub const RT_SIGACTION: usize = 134;
    pub const RT_SIGPROCMASK: usize = 135;
    pub const PRCTL: usize = 167;
    pub const GETPID: usize = 172;
    pub const GETTID: usize = 178;
    pub const MUNMAP: usize = 215;
    pub const MMAP: usize = 222;
    pub const MPROTECT: usize = 226;
    pub const MINCORE: usize = 232;
    pub const MADVISE: usize = 233;
    pub const GETRANDOM: usize = 278;
    pub const MEMBARRIER: usize = 283;
    #[cfg(target_env = "musl")]
    pub const RSEQ: usize = 293;
}

/// Makes system call `number` with six argument registers; the kernel ignores the ones a call
/// does not take.
///
/// # Safety
///
/// The arguments meet the kernel's contract for `number`; any memory the call reads or writes
/// is valid for that access; and whatever the call changes (mappings, protections, signal
/// dispositions, the process's existence) is something the caller is entitled to change and
/// that no Rust code relies on staying as it was.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub(super) unsafe fn syscall(number: usize, args: [Reg; 6]) -> Reg {
    let ret: Reg;
    // SAFETY: SYSCALL-01. this block is the system-call instruction and nothing else. The register
    // assignment is the x86-64 Linux convention (number and result in rax, arguments in rdi,
    // rsi, rdx, r10, r8, r9). The instruction overwrites rcx (return address) and r11 (saved
    // flags), declared as clobbers; it does not touch the user stack (`nostack`) and the return
    // restores the flags from r11 (`preserves_flags`). Memory is not declared untouched, so the
    // compiler assumes the call may read or write anything the arguments reach. What the kernel
    // does with the arguments is the caller's obligation under this function's contract.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number => ret,
            in("rdi") args[0],
            in("rsi") args[1],
            in("rdx") args[2],
            in("r10") args[3],
            in("r8") args[4],
            in("r9") args[5],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack, preserves_flags),
        );
    }
    ret
}

/// Makes system call `number` with six argument registers; the kernel ignores the ones a call
/// does not take.
///
/// Never inlined: naming the vectors makes the function that holds this block save `d8` to `d15`,
/// a cost that belongs to a call about to enter the kernel and not to the slow path around it.
///
/// # Safety
///
/// As for the x86-64 form.
#[cfg(target_arch = "aarch64")]
#[inline(never)]
pub(super) unsafe fn syscall(number: usize, args: [Reg; 6]) -> Reg {
    let ret: Reg;
    // SAFETY: SYSCALL-02. this block is the system-call instruction and nothing else. The register
    // assignment is the aarch64 Linux convention (number in x8, arguments in x0 to x5, result in
    // x0). The kernel restores every other general-purpose register and the condition flags on the
    // way back, and does not touch the user stack (`nostack`, `preserves_flags`). In a thread that
    // has used SVE it zeroes the predicates and all of each vector but its low half, and only `v`
    // names a vector's top half. Memory is not declared untouched, as on x86-64.
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") args[0] => ret,
            in("x1") args[1],
            in("x2") args[2],
            in("x3") args[3],
            in("x4") args[4],
            in("x5") args[5],
            lateout("p0") _, lateout("p1") _, lateout("p2") _, lateout("p3") _,
            lateout("p4") _, lateout("p5") _, lateout("p6") _, lateout("p7") _,
            lateout("p8") _, lateout("p9") _, lateout("p10") _, lateout("p11") _,
            lateout("p12") _, lateout("p13") _, lateout("p14") _, lateout("p15") _,
            lateout("v0") _, lateout("v1") _, lateout("v2") _, lateout("v3") _,
            lateout("v4") _, lateout("v5") _, lateout("v6") _, lateout("v7") _,
            lateout("v8") _, lateout("v9") _, lateout("v10") _, lateout("v11") _,
            lateout("v12") _, lateout("v13") _, lateout("v14") _, lateout("v15") _,
            lateout("v16") _, lateout("v17") _, lateout("v18") _, lateout("v19") _,
            lateout("v20") _, lateout("v21") _, lateout("v22") _, lateout("v23") _,
            lateout("v24") _, lateout("v25") _, lateout("v26") _, lateout("v27") _,
            lateout("v28") _, lateout("v29") _, lateout("v30") _, lateout("v31") _,
            options(nostack, preserves_flags),
        );
    }
    ret
}

/// The generic timer's count and its rate in Hz (`cntvct_el0`, `cntfrq_el0`), which every
/// aarch64 Linux lets a program read, unless the program has asked otherwise (`PR_SET_TSC`).
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub(super) fn generic_timer() -> (u64, u64) {
    let (count, rate): (u64, u64);
    // SAFETY: SYSCALL-03. Two reads of registers, which touch no memory, the stack or the flags.
    unsafe {
        asm!(
            "mrs {count}, cntvct_el0",
            "mrs {rate}, cntfrq_el0",
            count = out(reg) count,
            rate = out(reg) rate,
            options(nomem, nostack, preserves_flags),
        );
    }
    (count, rate)
}

/// The calling thread's thread pointer (`tpidr_el0`).
#[cfg(all(target_arch = "aarch64", target_env = "musl"))]
#[inline(always)]
pub(super) fn thread_register() -> Reg {
    let pointer: Reg;
    // SAFETY: SYSCALL-03. A read of a register, which touches no memory, the stack or the flags.
    unsafe {
        asm!(
            "mrs {pointer}, tpidr_el0",
            pointer = out(reg) pointer,
            options(nomem, nostack, preserves_flags),
        );
    }
    pointer
}

/// Decodes a result: Linux returns `-errno`, between -4095 and -1, on failure.
#[inline(always)]
pub(super) fn result(ret: Reg) -> Result<Reg, OsError> {
    let value = ret.addr();
    if value > usize::MAX - 4095 {
        Err(OsError::from_errno(value.wrapping_neg() as i32))
    } else {
        Ok(ret)
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn results_decode_errors() {
        assert_eq!(result(int(0)).map(|r| r.addr()), Ok(0));
        assert_eq!(result(int(4096)).map(|r| r.addr()), Ok(4096));
        assert_eq!(result(int(12usize.wrapping_neg())), Err(OsError::ENOMEM));
        assert_eq!(
            result(int(4095usize.wrapping_neg())),
            Err(OsError::from_errno(4095))
        );
        assert!(result(int(4096usize.wrapping_neg())).is_ok());
    }

    #[test]
    fn a_call_reaches_the_kernel() {
        // SAFETY: getpid takes no arguments and changes nothing.
        let pid = unsafe { syscall(nr::GETPID, [int(0); 6]) };
        assert_eq!(result(pid).unwrap().addr(), std::process::id() as usize);
    }
}
