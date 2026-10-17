//! The one system-call instruction in the allocator.
//!
//! Every system call goes through [`syscall`]. Arguments and results travel as pointers, as in
//! rustix's raw Linux backend, so that a pointer argument keeps its provenance into the kernel
//! and an address the kernel returns, from `mmap`, comes back as a pointer usable for the memory
//! it names, with no integer-to-pointer cast. The `asm!` block is the single exception to the
//! rule that all assembly comes from Lean; its ledger entry is
//! `docs/safety/asmalloc/os/linux/syscall-safety.md`.

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
pub(super) mod nr {
    pub const WRITE: usize = 1;
    pub const MMAP: usize = 9;
    pub const MPROTECT: usize = 10;
    pub const MUNMAP: usize = 11;
    pub const RT_SIGACTION: usize = 13;
    pub const RT_SIGPROCMASK: usize = 14;
    pub const MADVISE: usize = 28;
    pub const GETPID: usize = 39;
    pub const GETTID: usize = 186;
    pub const EXIT_GROUP: usize = 231;
    pub const TGKILL: usize = 234;
    pub const GETRANDOM: usize = 318;
    pub const MEMBARRIER: usize = 324;
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
#[inline(always)]
pub(super) unsafe fn syscall(number: usize, args: [Reg; 6]) -> Reg {
    let ret: Reg;
    // SAFETY: this block is the system-call instruction and nothing else. The register
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
