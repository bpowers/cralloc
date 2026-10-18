# Safety ledger: `crates/asmalloc/src/os/linux/syscall.rs`

The one hand-written `asm!` block in the allocator. CLAUDE.md allows it as the single
exception to "all assembly comes from Lean". Neither Kani nor Miri can execute it; the unit tests
in the same file and in `kernel.rs` run it on the real kernel.

## SYSCALL-01: the `syscall` instruction in `syscall()`

**Block.** `asm!("syscall", inlateout("rax") number => ret, in("rdi") a0, in("rsi") a1,
in("rdx") a2, in("r10") a3, in("r8") a4, in("r9") a5, lateout("rcx") _, lateout("r11") _,
options(nostack, preserves_flags))`.

**Preconditions** (the function's `# Safety`). The arguments meet the kernel's contract for
`number`. Any memory the call reads or writes is valid for that access. Whatever the call changes
(mappings, protections, signal state, the process's existence) is the caller's to change, and no
Rust code relies on it staying as it was.

**What the block itself must get right**, given those preconditions, is the interface to the
instruction, and the compiler's model of what it does.

1. *Register convention.* x86-64 Linux takes the system-call number in `rax` and arguments in
   `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9`, and returns the result in `rax` (kernel
   `Documentation/arch/x86/entry_64.rst`; `arch/x86/entry/entry_64.S`, `entry_SYSCALL_64`). The
   operands name exactly those registers. `r10` stands in for `rcx`, which the instruction
   overwrites.
2. *Clobbers.* The `syscall` instruction writes the return address to `rcx` and `RFLAGS` to
   `r11` (Intel SDM vol. 2B, SYSCALL). The kernel restores every other general-purpose register
   except `rax`, and the user's vector and x87 state, before returning. Both `rcx` and `r11`
   are declared `lateout(...) _`. No other register changes.
3. *Flags.* On return the kernel restores `RFLAGS` from the value saved at entry. On the
   `sysret` path that is `r11`; on the `iret` path it is the saved `pt_regs->flags`, which is
   the same value. So the status flags and `DF` are as before, which is what `preserves_flags`
   promises. A debugger may change them through ptrace; that is outside the program's semantics.
4. *Stack.* The instruction neither reads nor writes the user stack; the kernel switches to its
   own. A signal delivered on return builds its frame below the 128-byte red zone. So `nostack`
   holds.
5. *Memory.* Neither `nomem` nor `readonly` is given. The compiler therefore assumes the block
   may read and write any memory reachable from the pointer arguments, as the kernel does for
   `write`, `rt_sigaction` and `prctl`. No such memory is held in registers across the block.
6. *Provenance.* Arguments and the result are typed `*mut u8`. A pointer argument is passed with
   its provenance, which inline assembly may use as the caller's preconditions allow. The result
   of `mmap` is the address of a new mapping, and inline assembly, like a foreign function, may
   return a pointer to memory it created. Other results are integers in pointer form, read only
   through `addr()`. No integer-to-pointer cast is involved.
7. *Result decoding* (`result()`, safe code). Linux returns `-errno`, in `-4095..=-1`, on
   failure, and the test `value > usize::MAX - 4095` selects exactly that range. No successful
   call used here returns a value in it: `mmap` cannot return an address in the top page, and
   the others return small counts or zero.

**Machine checks.** `os::linux::syscall::tests::a_call_reaches_the_kernel` (getpid agrees with
`std::process::id`) and `results_decode_errors`. Every `LinuxOs` test in `kernel.rs` goes
through this block.

**Review.** 2026-10-17, fresh adversarial reviewer: accepted (register convention, clobbers,
`preserves_flags`, `nostack` and the red zone, memory, pointer-typed registers).

Reviewed: f41dbf9
