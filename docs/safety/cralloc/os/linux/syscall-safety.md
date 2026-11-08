# Safety ledger: `crates/cralloc/src/os/linux/syscall.rs`

The system-call instruction, the lasting exception CLAUDE.md allows to "all assembly comes from
Lean", once for each architecture, and on aarch64 the reads of three registers Rust has no stable way
to name. Neither Kani nor Miri can execute them; the unit tests in the same file and in `kernel.rs`
run them on the real kernel.

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
   `write` and `openat` (which read a buffer and a path), `read` (which writes a buffer),
   `rt_sigaction` and `prctl`, `mincore` (which writes a vector) and `futex` (which reads the
   word waited on). No such memory is held in registers across the block.
6. *Provenance.* Arguments and the result are typed `*mut u8`. A pointer argument is passed with
   its provenance, which inline assembly may use as the caller's preconditions allow. The result
   of `mmap` is the address of a new mapping, and inline assembly, like a foreign function, may
   return a pointer to memory it created. Other results are integers in pointer form, read only
   through `addr()`. No integer-to-pointer cast is involved.
7. *Result decoding* (`result()`, safe code). Linux returns `-errno`, in `-4095..=-1`, on
   failure, and the test `value > usize::MAX - 4095` selects exactly that range. No successful
   call used here returns a value in it: `mmap` cannot return an address in the top page;
   `openat` returns a file descriptor, a small non-negative number; `read` and `write` return
   a count no larger than the buffer they were given; and the others return small counts or
   zero.

**Call numbers.** The constants in `nr` are the x86-64 numbers of
`arch/x86/entry/syscalls/syscall_64.tbl`: `read` 0, `write` 1, `close` 3, `mmap` 9, `mprotect`
10, `munmap` 11, `rt_sigaction` 13, `rt_sigprocmask` 14, `mincore` 27, `madvise` 28, `getpid`
39, `prctl` 157, `gettid` 186, `futex` 202, `exit_group` 231, `tgkill` 234, `openat` 257,
`getrandom` 318, `membarrier` 324, and on musl `rseq` 334. A wrong number would make a different
call with the same arguments, so each is checked against the table.

**Machine checks** (of this entry and SYSCALL-02). `os::linux::syscall::tests::a_call_reaches_the_kernel` (getpid agrees with
`std::process::id`) and `results_decode_errors`. Every `LinuxOs` test in `kernel.rs` goes
through this block.

## SYSCALL-02: the `svc #0` instruction in `syscall()`, on aarch64

**Block.** `asm!("svc #0", in("x8") number, inlateout("x0") a0 => ret, in("x1") a1, in("x2") a2,
in("x3") a3, in("x4") a4, in("x5") a5, lateout("p0") _, ... lateout("p15") _, lateout("v0") _, ...
lateout("v31") _, options(nostack, preserves_flags))`, in a function that is never inlined.

**Preconditions.** As SYSCALL-01.

**Proof.** Items 5, 6 and 7 of SYSCALL-01 hold as they stand. For the others:

1. *Register convention.* aarch64 Linux takes the number in `x8` and arguments in `x0` to `x5`, and
   returns the result in `x0` (`syscall(2)`, "Architecture calling conventions";
   `arch/arm64/kernel/syscall.c`, `el0_svc_common`). The operands name exactly those.
2. *General-purpose registers.* The kernel restores every one but `x0` from its saved frame on the
   way back (`kernel_exit` in `arch/arm64/kernel/entry.S`), so none other is declared.
3. *SVE.* If the thread has used SVE, the kernel zeroes `P0` to `P15`, `FFR` and every bit of `Z0` to
   `Z31` above the low 128 on the way in (`fpsimd_syscall_enter` in
   `arch/arm64/kernel/entry-common.c`; `Documentation/arch/arm64/sve.rst`, "System call behaviour").
   A build for Graviton or Grace has SVE, and a compiler with SVE keeps such state across a block
   that does not declare it: given `+sve`, `resident`'s counting loop would form `ptrue p0.d` before
   the call and load through it after, and read nothing. So the predicates are declared, and the
   vectors through `v`, which covers more than is lost but is the only name a vector's top half has.
   `FFR` is not: LLVM reserves it, keeps no value there and warns of a clobber that names it, and
   nothing in the crate reads it. The declarations are the same with SVE or without. A function
   that clobbers `v8` to `v15` saves their low halves, so this one is never inlined and that cost
   stays with the call. The kernel also leaves streaming mode, in which no Rust code runs.
4. *Flags.* The condition flags are in `SPSR_EL1`, saved at the exception and restored by `eret`, so
   `preserves_flags` holds.
5. *Stack.* `svc` neither reads nor writes the user stack; the kernel runs on its own, and a signal
   frame goes below `sp`, this architecture having no red zone. So `nostack` holds.

**Call numbers.** The constants in the aarch64 `nr` are those of `/usr/include/asm-generic/unistd.h`:
`openat` 56, `close` 57, `read` 63, `write` 64, `exit_group` 94, `futex` 98, `tgkill` 131,
`rt_sigaction` 134, `rt_sigprocmask` 135, `prctl` 167, `getpid` 172, `gettid` 178, `munmap` 215,
`mmap` 222 (`__NR3264_mmap`), `mprotect` 226, `mincore` 232, `madvise` 233, `getrandom` 278,
`membarrier` 283, `rseq` 293.

No test here has a thread with SVE state, since the M1 has none. Item 3 rests on the kernel's source
and on reading what the compiler emits for `+sve,+sve2`.

## SYSCALL-03: `mrs` of the timer's registers and the thread pointer, on aarch64

**Blocks.** `generic_timer`, of `cntvct_el0` and `cntfrq_el0`, and `thread_register`, of
`tpidr_el0`.

**Proof.** Each writes the one output register the compiler chose and nothing else, touches no memory
or stack and leaves the flags alone, as `options(nomem, nostack, preserves_flags)` says.

`tpidr_el0` is a register of the thread's own, always readable from user mode. Linux opens the
virtual counter by `CNTKCTL_EL1.EL0VCTEN` (`arch_counter_set_user_access` in
`drivers/clocksource/arm_arch_timer.c`, `update_cntkctl_el1` in `arch/arm64/kernel/process.c`), and
that bit opens `cntfrq_el0` too. It stays clear in two cases, and then both reads trap:

- Where an erratum needs the counter read another way, the kernel emulates each read
  (`cntvct_read_handler`, `cntfrq_read_handler` in `arch/arm64/kernel/traps.c`): the same one register
  written, slowly.
- A thread that asked for `PR_TSC_SIGSEGV`, as a record-and-replay tool does for its tracee, gets
  SIGSEGV on the instruction, as on x86-64 (KERNEL-09). Unhandled, the process stops, which is no
  memory-safety violation.

The timer's two values are integers, and no promise rests on `ticks` (KERNEL-09). The thread pointer
comes back typed as a pointer, as an `mmap` result does (SYSCALL-01, item 6): the address the C
library set, of memory Rust never allocated. `front::area` offsets it to the area and makes the
4-byte atomic access AREA-02 argues for.

Reviewed: 5a8ade1