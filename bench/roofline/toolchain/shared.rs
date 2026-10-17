//! The direct load of glibc's `__rseq_offset` from src/main.rs, in a shared library. It needs a
//! PC-relative relocation against a symbol another object defines, which a shared library
//! cannot have, so this is expected not to link.
use std::arch::asm;

extern "C" {
    static __rseq_offset: isize;
}

#[no_mangle]
pub extern "C" fn rseq_offset_direct() -> isize {
    let v: isize;
    unsafe { asm!("mov {v}, qword ptr [rip + {s}]", v = out(reg) v, s = sym __rseq_offset, options(nostack, readonly)) };
    v
}
