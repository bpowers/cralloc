# Safety ledger: `crates/asmalloc/src/os/linux/kernel.rs`

`LinuxOs`, the `Os` implementation over system calls. Every unsafe block here but one calls
`syscall()` (SYSCALL-01) or a thin helper over it, so each entry's obligation is that the
kernel's contract for the call is met and that the call changes only what the caller may change.
The exception is the timestamp-counter read in `ticks` (KERNEL-09).
Neither Kani nor Miri can run this file; the unit tests in it run every call on the real kernel.

References are to the Linux man pages (mmap(2), mprotect(2), madvise(2), munmap(2),
membarrier(2), getrandom(2), rt_sigaction(2), sigprocmask(2), tgkill(2), write(2)) and to
`include/uapi/linux/{mman,membarrier,random}.h`.

## KERNEL-01: `range_call`

**Block.** `syscall(number, [start, len, arg, 0, 0, 0])`.

**Preconditions** (the function's `# Safety`). `number` is mprotect, madvise or munmap, and the
call is one the calling `Os` method's contract permits on `start..start + len`.

**Proof.** The three calls take an address, a length and one integer, which is what is passed.
None reads or writes user memory through its arguments. What they do to the range itself is
covered by the caller's contract (KERNEL-03), which this function forwards unchanged:
mprotect changes protection, madvise may zero contents, munmap removes the mapping.

## KERNEL-02: `membarrier`

**Block.** `syscall(MEMBARRIER, [command, flags, cpu, 0, 0, 0])`.

**Proof.** membarrier takes a command, flags and a CPU id, and no pointers.
`REGISTER_PRIVATE_EXPEDITED_RSEQ` (flags 0) only records the registration. Afterwards
`PRIVATE_EXPEDITED_RSEQ` with `MEMBARRIER_CMD_FLAG_CPU` interrupts the thread of the process
running on CPU `cpu`, if there is one, so that any rseq critical section in flight there is
aborted, and returns when that is done; a CPU that is offline or out of range has nothing to
interrupt and the call returns 0 (`kernel/sched/membarrier.c`, `membarrier_private_expedited`).
A thread of the process preempted inside a sequence on that CPU was preempted by an interrupt,
so its abort is already pending. An abort is something every sequence must already tolerate,
since preemption causes one at any instruction. No memory changes.

## KERNEL-03: the memory calls in `reserve`, `commit`, `purge`, `advise_huge`, `unmap`

**mmap in `reserve`.** Arguments: address null (the kernel chooses, so no existing mapping is
replaced), `span` bytes, `PROT_NONE`, `MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE`, fd -1,
offset 0. It reads no user memory. The result is a fresh range disjoint from every existing
mapping. A null result is treated as failure, since the kernel never places an anonymous
mapping at address zero.

**Trimming munmaps in `reserve`.** `mapped` is page-aligned, so `lead`, the distance to the next
`align` boundary, is a multiple of the page size and at most `align - page`. Hence
`start + len <= mapped + span` and `tail = span - lead - len` does not underflow. The head
`mapped..start` and the tail `start + len..mapped + span` are parts of the fresh mapping that
nothing has seen, so unmapping them affects no one. If either munmap fails, an inaccessible range
stays mapped; that wastes address space and nothing else.

**`commit`: mprotect to `PROT_READ | PROT_WRITE`.** The caller guarantees whole pages of a live
reservation (the `Os` contract, discharged by `Space::commit`'s range check). Raising protection
keeps contents, and nothing relies on the pages being inaccessible.

**`purge`: madvise `MADV_DONTNEED`.** After it, a private anonymous range reads as zero and
keeps its protection. The caller guarantees no thread touches the range during the call and
nothing in it is needed, so zeroing it cannot break any Rust value or reference.

**`advise_huge`: madvise `MADV_HUGEPAGE` or `MADV_NOHUGEPAGE`.** These change only how the
kernel backs the range, never its contents or protection.

**`wipe_on_fork`: madvise `MADV_WIPEONFORK`** (Linux 4.14). It sets a flag on the private
anonymous mapping so that a child's copy reads as zero; the calling process's contents and
protection are unchanged. On an older kernel it fails with EINVAL and changes nothing.

**`unmap`: munmap.** The caller guarantees nothing uses the range afterwards.

**Partial failure.** mprotect and madvise can fail after acting on part of the range (ENOMEM
from a VMA split). The `Os` contract allows this, and no caller assumes either outcome.

## KERNEL-04: `write_stderr`

**Block.** `syscall(WRITE, [2, bytes.as_ptr(), bytes.len(), 0, 0, 0])`.

**Proof.** write(2) reads at most `bytes.len()` bytes from the buffer, which is a live slice for
the duration of the call. The pointer comes from a shared reference and the kernel only reads
it. A short write advances the slice by the count returned, capped at its length, and any error
or a zero count ends the loop, so the loop cannot run forever on a broken descriptor.

## KERNEL-05: `getrandom`, and getpid in `random`

**Blocks.** `syscall(GETRANDOM, [&mut value, 8, GRND_INSECURE, 0, 0, 0])` in `getrandom`, and
`int_call(GETPID, 0, 0, 0)` in `random`.

**Proof.** getrandom(2) writes at most its length argument, 8, to the buffer, which is a live,
exclusively borrowed local `u64` of 8 bytes, and changes nothing else. `GRND_INSECURE` (Linux
5.6) means it never blocks; on an older kernel it fails with EINVAL and writes nothing. The
value is used only if the call reports all 8 bytes written, and any bit pattern is a valid
`u64`. getpid takes nothing and changes nothing.

**What the value is for.** The process generation (`depot::generation`). Its quality is not a
memory-safety matter: a child that drew its parent's generation would treat the parent's
abandoned drain claim and markers as live, which leaves one CPU's cache undrained and on the
slow path, and duplicates nothing.

## KERNEL-06: `abort` and the calls it makes

**Blocks.** getpid, gettid and tgkill in `raise_sigabrt`; rt_sigaction in `default_sigabrt`;
rt_sigprocmask in `unblock_sigabrt`; exit_group in `abort`.

**Order.** `abort` writes its message, then raises SIGABRT on the calling thread with the
program's disposition in place, as glibc's `abort` does, so a crash reporter's handler runs.
The heap is valid at that point: the allocator refused to go on, it did not break. If the
raise returns (the handler returned, the signal is ignored, or it is blocked), `abort` resets
SIGABRT to the default action, unblocks it, and raises again, which ends the process with a
core dump. If even that returns, exit_group(127) ends the process, so the function does not
return, as `-> !` requires.

**The first raise does not unblock SIGABRT.** Inside the program's own SIGABRT handler the
signal is blocked (unless the handler was installed with `SA_NODEFER`), so an abort from there,
by a handler that allocates into the failure that brought it here, leaves its first raise
pending; the reset and the unblock then deliver it with the default action. The handler is not
run again, and nothing recurses. A thread that blocked SIGABRT for some other reason skips the
program's handler the same way.

**Two threads at once.** Each runs the handler on its own thread. The first whose handler
returns resets the disposition and ends the process, possibly before the other's handler has
finished its report. That is accepted: it costs at most part of a crash report, and it needs no
shared state and no waiting. glibc 2.43's `abort` behaves the same way in this respect: the
first thread to finish its handler ends the process under every other thread's
(`stdlib/abort.c`).

**Rules.** No shared state, no lock, no wait, no allocation.

**Proof of the blocks.**

- getpid and gettid take nothing. tgkill(pid, tid, SIGABRT) with this thread's IDs runs the
  disposition in force on this thread: the program's handler the first time, the default (end
  the process with a core dump) the second.
- rt_sigaction(SIGABRT, `&default`, null, 8) reads one `KernelSigaction` from a live local.
  The layout is the kernel's x86-64 `struct sigaction`: handler, flags, restorer, mask, which
  differs from glibc's. The size argument 8 is the kernel's `sigset_t`. The old-action pointer
  is null, so nothing is written back. Resetting a handled signal to its default does not
  discard a pending one.
- rt_sigprocmask(SIG_UNBLOCK, `&set`, null, 8) reads one signal set from a live local and
  writes nothing back.
- exit_group(127) takes an integer and ends the process.
- Changing the process's SIGABRT disposition and the thread's mask is this function's to do,
  because the process is ending. A handler that allocates re-enters a heap that is still
  consistent; at worst it aborts again, which the paragraph above turns into the default action.

**`int_call` itself.** It forwards to `syscall` with three integer arguments. Its callers are
getpid, gettid, tgkill and exit_group here and getpid in `random`, none of which takes a
pointer.

## KERNEL-09: `ticks`

**Block.** `core::arch::x86_64::_rdtsc()`.

**Preconditions.** None that the caller can break. The intrinsic is `unsafe` only because all of
`core::arch` is. It compiles to the `rdtsc` instruction, which every x86-64 processor has, so no
target feature has to be checked first.

**Proof.** `rdtsc` reads no memory, writes no memory, and writes only `edx` and `eax`, which the
intrinsic returns as one `u64`; the compiler knows this, as it does for any intrinsic.

**When it faults.** In user mode only if `CR4.TSD` is set. Linux sets it while a thread with
`TIF_NOTSC` runs (`disable_TSC` sets both, and the context switch toggles `CR4.TSD` when the
flag differs between threads; `arch/x86/kernel/process.c`). Two things set `TIF_NOTSC`:
`prctl(PR_SET_TSC, PR_TSC_SIGSEGV)` (`set_tsc_mode`), and entering strict seccomp
(`seccomp_set_mode_strict` in `kernel/seccomp.c` calls `disable_TSC`). Seccomp filter mode
does not: a filter sees system calls, not instructions. The thread gets SIGSEGV on the
instruction. Unhandled, the process stops, which is no memory-safety violation. A strict
seccomp thread is killed anyway by its first system call other than read, write, exit and
sigreturn, which the allocator's slow paths make.

**When it traps and is emulated.** Then it returns a value, slowly, and nothing else changes:

- Record-and-replay tools (rr) set `PR_TSC_SIGSEGV` on the traced program and emulate and
  record each read in the tracer, so every read costs a signal and a ptrace round trip.
- A hypervisor may intercept the instruction (RDTSC exiting), making each read a VM exit; KVM
  does not by default. In an AMD SEV-ES or SEV-SNP guest an intercepted read raises #VC, which
  the guest kernel serves through the hypervisor (`vc_handle_rdtsc` in
  `arch/x86/coco/sev/vc-handle.c`), unless the guest uses Secure TSC.

Under either, every slow path pays a trap or two (see below). That is a cost, not a safety
matter. Checked against the Linux source at 7.3-rc7 (`arch/x86/kernel/process.c`,
`kernel/seccomp.c`, `arch/x86/coco/sev/vc-handle.c`), read 2026-10-17. That rr traps the
counter this way comes from the kernel discussions of `PR_SET_TSC` and its arm64 port, not
from a measurement here.

**What the value is for.** Pacing what happens once an interval: the idle check's drains
(`depot::heartbeat`, when a heartbeat looks at a slab) and the back end's clocked purge
(`back::purge`, read by every back-end heartbeat). The `Os` trait makes no promise about it,
so no safety argument rests on it: a counter that runs at an unexpected rate, differs between
CPUs, or goes back makes a drain or a purge pass come earlier or later, and both are safe at
any time (the drain protocol, `depot/drain.rs`; a purge claims the bits of what it returns,
`back/region.rs`).

## KERNEL-07: `unsafe impl Os for LinuxOs`

The comment above the impl maps each promise of the `Os` trait to the kernel behaviour that
provides it, and KERNEL-01 to KERNEL-06 and KERNEL-09 cover the individual calls. The promises not covered by a
single call:

- *Valid until unmapped or dropped.* Reservations are mappings of the process. Dropping a
  `LinuxOs`, which is zero-sized, changes nothing.
- *Reserved memory reads as zero.* Fresh private anonymous memory is zero-filled on first touch.
- *Any thread, signal handlers, no allocation.* Every method is a fixed sequence of system calls
  and arithmetic on locals, all async-signal-safe.

**Machine checks.** The tests in `kernel.rs`:

- `reserve_commit_purge_and_unmap`: alignment, zero-fill, purge zeroes, unmap.
- `bad_arguments_are_errors_not_calls`.
- `the_design_reservation_is_available`: a 16 TiB reservation.
- `rseq_fences_aim_at_one_cpu_after_registering`, `ticks_rise`, `random_numbers_differ` and
  `marked_pages_read_as_zero_in_a_forked_child`.
- `abort_ends_the_process_with_sigabrt`,
  `abort_runs_the_programs_handler_then_ends_the_process_anyway` and
  `an_abort_inside_the_handler_ends_the_process`: child processes, checking the signal, the
  message, that a returning handler ran, and that a handler that aborts runs once.

**Review.** 2026-10-17, fresh adversarial reviewer: KERNEL-01 to KERNEL-07 accepted, after
the fixes of its first round. Its second round found that a child forked mid-abort parked for
ever, that a waiting thread could hang the process, and that the glibc comparison and
KERNEL-09 were wrong or stale; KERNEL-09 was rewritten for those and needs the same reviewer's
check. KERNEL-06 was since rewritten for a simpler `abort` with no shared state, and needs a
fresh review.

Reviewed: f41dbf9
