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

## KERNEL-06: `abort`, `unblock_sigabrt` and `raise_sigabrt`

**Blocks.** rt_sigprocmask in `unblock_sigabrt`; gettid in `thread_id`; getpid and tgkill in
`raise_sigabrt`; pause in `park`; rt_sigaction and exit_group in `abort`.

**Order.** The first thread to abort stores its thread ID in `ABORTING` by compare-and-swap from
zero, unblocks SIGABRT and raises it with the program's disposition in place, as glibc's
`abort` does, so a crash reporter's handler runs. The heap is valid at that point: the
allocator refused to go on, it did not break. If the handler returns, or ignores the signal, or
calls back into `abort` on the same thread (the stored ID is its own, so the nested call skips
straight on), `abort` restores the default action, unblocks and raises again, and ends with
exit_group if even that returns.

Any other thread that aborts meanwhile finds another ID stored and parks in a pause loop. It
must not go on to reset the disposition: that would end the process while the first thread's
handler is still writing its report. Parking takes no lock; the process ends when the first
thread finishes. If a handler never returns to `abort` (it longjmps away), the parked threads
wait forever, as they would on glibc's abort lock.

**Proof.**

- rt_sigprocmask(SIG_UNBLOCK, `&set`, null, 8) reads one signal set from a live local.
- rt_sigaction(SIGABRT, `&default`, null, 8) reads one `KernelSigaction` from a live local.
  The layout is the kernel's x86-64 `struct sigaction`: handler, flags, restorer, mask, which
  differs from glibc's. The size argument 8 is the kernel's `sigset_t`. The old-action and
  old-set pointers are null, so nothing is written back.
- The pointers come from shared references and the kernel only reads them.
- pause takes nothing and only sleeps until a signal is handled. Thread IDs are positive and
  below 2^22, so the cast to `u32` keeps them and zero never names a thread.
- getpid and gettid take nothing. tgkill(pid, tid, SIGABRT) runs the disposition in force: the
  program's handler the first time, the default (end the process with a core dump) the second.
  exit_group(127) ends the process if both raises returned, so the function does not return, as
  `-> !` requires.
- Changing the process's SIGABRT disposition and mask is this function's to do, because the
  process is ending. A handler that allocates re-enters a heap that is still consistent; at
  worst it aborts again, which the flag turns into the default action.
- `ABORTING` is one static atomic, all-zero valid; it holds no allocator state.

**Review.** The thread-ID handoff and `park` are new and need a fresh reviewer's check.

**`int_call` itself.** It forwards to `syscall` with three integer arguments. Its callers are
getpid, gettid, tgkill and exit_group here and getpid in `random`, none of which takes a
pointer.

## KERNEL-08: `protect_none`

**Block.** `range_call(MPROTECT, start, len, PROT_NONE)`, built only for tests and the `sim`
feature; the allocator never calls it.

**Preconditions** (its `# Safety`). Whole pages of a live `LinuxOs` reservation, which nothing
reads or writes afterwards until it is committed again.

**Proof.** As KERNEL-01, with the protection lowered instead of raised: mprotect keeps the
contents and changes only access, and since nothing touches the range until it is committed
again, no Rust access can fault on it except a bug's, which is what the simulator's guarded
memory exists to expose (SIM-MEMORY-06). Its one caller is `Memory::close`.

## KERNEL-09: `ticks`

**Block.** `core::arch::x86_64::_rdtsc()`.

**Preconditions.** None that the caller can break. The intrinsic is `unsafe` only because all of
`core::arch` is. It compiles to the `rdtsc` instruction, which every x86-64 processor has, so no
target feature has to be checked first.

**Proof.** `rdtsc` reads no memory, writes no memory, and writes only `edx` and `eax`, which the
intrinsic returns as one `u64`; the compiler knows this, as it does for any intrinsic. In user
mode it faults only if the kernel set `CR4.TSD`, which Linux does only for a thread that asked
with `prctl(PR_SET_TSC, PR_TSC_SIGSEGV)`. Such a thread gets SIGSEGV on the instruction: the
process stops, which is no memory-safety violation, and a program that forbids itself the
counter has opted out of anything that reads it.

**What the value is for.** The idle check's pacing (`depot::heartbeat`), and nothing else. The
`Os` trait makes no promise about it, so no safety argument rests on it: a counter that runs at
an unexpected rate, differs between CPUs, or goes back makes a drain come earlier or later, and
a drain is safe at any time (the drain protocol, `depot/drain.rs`).

## KERNEL-07: `unsafe impl Os for LinuxOs`

The comment above the impl maps each promise of the `Os` trait to the kernel behaviour that
provides it, and KERNEL-01 to KERNEL-06 and KERNEL-09 cover the individual calls. The promises not covered by a
single call:

- *Valid until unmapped or dropped.* Reservations are mappings of the process. Dropping a
  `LinuxOs`, which is zero-sized, changes nothing.
- *Reserved memory reads as zero.* Fresh private anonymous memory is zero-filled on first touch.
- *`rseq` reports glibc's registration.* It describes the process; whether the calling thread
  is registered is for the caller to check through its own `cpu_id` (GLIBC-01).
- *Any thread, signal handlers, no allocation.* Every method is a fixed sequence of system calls
  and arithmetic on locals, all async-signal-safe.

**Machine checks.** The tests in `kernel.rs`:

- `reserve_commit_purge_and_unmap`: alignment, zero-fill, purge zeroes, unmap.
- `bad_arguments_are_errors_not_calls`.
- `the_design_reservation_is_available`: a 16 TiB reservation.
- `glibc_registers_rseq`.
- `rseq_fences_aim_at_one_cpu_after_registering`, `ticks_rise`, `random_numbers_differ` and
  `a_page_can_be_marked_to_be_wiped_on_fork`.
- `abort_ends_the_process_with_sigabrt` and
  `abort_runs_the_programs_handler_then_ends_the_process_anyway`: child processes, checking
  the signal, the message and that a returning handler ran.

**Review.** 2026-10-17, fresh adversarial reviewer: KERNEL-01 to KERNEL-07 accepted, after
the fixes of its first round. KERNEL-06 changed since (the handler now runs first) and needs
the reviewer's check again. KERNEL-09 is new and needs a fresh reviewer's check.
