//! What rustc generates around an `asm!` block that calls a function. Without `nostack` the
//! Rust reference guarantees that the stack pointer is aligned for a call on entry to the block
//! and that the block may use the stack below it (the red zone). Compiled by `check.sh` with
//! `--emit asm`; read the output.
use std::arch::asm;

#[no_mangle]
pub extern "C" fn callee() -> usize {
    7
}

/// A leaf function whose only body is an asm block that calls, without `nostack`.
#[no_mangle]
pub unsafe fn leaf_call() -> usize {
    let r: usize;
    asm!("call {f}", f = sym callee, lateout("rax") r, clobber_abi("C"));
    r
}

/// The same, where the call is reached only by a branch inside the block.
#[no_mangle]
pub unsafe fn leaf_cold_call(x: usize) -> usize {
    let r: usize;
    asm!("mov rax, {x}", "test rax, rax", "jz 3f", "2:", ".pushsection .text.unlikely,\"ax\"", "3:", "call {f}", "jmp 2b", ".popsection",
         x = in(reg) x, f = sym callee, lateout("rax") r, clobber_abi("C"));
    r
}

/// Explicit clobbers only (what a register-preserving stub would allow).
#[no_mangle]
pub unsafe fn leaf_call_noabi() -> usize {
    let r: usize;
    asm!("call {f}", f = sym callee, lateout("rax") r, out("rcx") _, out("r8") _);
    r
}

/// A leaf that keeps a local in the red zone across an asm block, with and without nostack.
#[no_mangle]
pub unsafe fn redzone(x: usize) -> usize {
    let mut a = [x; 4];
    std::hint::black_box(&mut a);
    asm!("nop", out("rcx") _);
    std::hint::black_box(&mut a);
    a[0] + a[3]
}

#[no_mangle]
pub unsafe fn redzone_nostack(x: usize) -> usize {
    let mut a = [x; 4];
    std::hint::black_box(&mut a);
    asm!("nop", out("rcx") _, options(nostack));
    std::hint::black_box(&mut a);
    a[0] + a[3]
}

#[inline(never)]
#[no_mangle]
pub extern "C" fn other(x: usize) -> usize {
    std::hint::black_box(x)
}

/// An asm block that calls, in a function that also tail-calls on another path.
#[no_mangle]
pub unsafe fn with_tail(x: usize) -> usize {
    if x > 100 {
        return other(x);
    }
    let r: usize;
    asm!("call {f}", f = sym callee, lateout("rax") r, clobber_abi("C"));
    r
}

extern "C" {
    fn takes_ptr(p: *mut usize) -> i32;
}

/// One path makes an ordinary call that needs a frame (so shrink-wrapping can move the prologue
/// there), the other has the asm block that calls.
#[no_mangle]
pub unsafe fn shrinkwrap(x: usize) -> usize {
    if x > 100 {
        let mut v = 0usize;
        takes_ptr(&mut v);
        return v;
    }
    let r: usize;
    asm!("call {f}", f = sym callee, lateout("rax") r, clobber_abi("C"));
    r
}

/// The same, with a local live across the asm block on its path.
#[no_mangle]
pub unsafe fn shrinkwrap_local(x: usize) -> usize {
    if x > 100 {
        let mut v = 0usize;
        takes_ptr(&mut v);
        return v;
    }
    let mut a = [x; 4];
    std::hint::black_box(&mut a);
    let r: usize;
    asm!("call {f}", f = sym callee, lateout("rax") r, clobber_abi("C"));
    std::hint::black_box(&mut a);
    r + a[1]
}

/// Several asm blocks behind a jump table, as a match over size classes produces.
#[no_mangle]
pub unsafe fn with_table(x: usize) -> usize {
    let r: usize;
    match x {
        0 => asm!("mov eax, 0", "call {f}", f = sym callee, lateout("rax") r, clobber_abi("C")),
        1 => asm!("mov eax, 1", "call {f}", f = sym callee, lateout("rax") r, clobber_abi("C")),
        2 => asm!("mov eax, 2", "call {f}", f = sym callee, lateout("rax") r, clobber_abi("C")),
        3 => asm!("mov eax, 3", "call {f}", f = sym callee, lateout("rax") r, clobber_abi("C")),
        4 => asm!("mov eax, 4", "call {f}", f = sym callee, lateout("rax") r, clobber_abi("C")),
        _ => return other(x),
    }
    r
}
