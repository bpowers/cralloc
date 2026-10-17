# Safety ledger: `crates/asmalloc/src/os/linux/kernel.rs`

`LinuxOs`, the `Os` implementation over system calls. Every unsafe block here calls
`syscall()` (SYSCALL-01) or a thin helper over it, so each entry's obligation is that the
kernel's contract for the call is met and that the call changes only what the caller may change.
Neither Kani nor Miri can run this file; the unit tests in it run every call on the real kernel.

References are to the Linux man pages (mmap(2), mprotect(2), madvise(2), munmap(2),
membarrier(2), prctl(2), rt_sigaction(2), sigprocmask(2), tgkill(2), write(2)) and to
`include/uapi/linux/{mman,membarrier,prctl,auxvec}.h`.

## KERNEL-01: `range_call`

**Block.** `syscall(number, [start, len, arg, 0, 0, 0])`.

**Preconditions** (the function's `# Safety`). `number` is mprotect, madvise or munmap, and the
call is one the calling `Os` method's contract permits on `start..start + len`.

**Proof.** The three calls take an address, a length and one integer, which is what is passed.
None reads or writes user memory through its arguments. What they change, protections, page
contents and mappings, is covered by the caller's contract (KERNEL-03), which this function
forwards unchanged.

## KERNEL-02: `membarrier`

**Block.** `syscall(MEMBARRIER, [command, 0, 0, 0, 0, 0])`.

**Proof.** membarrier takes a command, flags (0) and a CPU id (unused for these commands), and
no pointers. `REGISTER_PRIVATE_EXPEDITED_RSEQ` only records the registration. Afterwards
`PRIVATE_EXPEDITED_RSEQ` interrupts every running thread of the process, so that any rseq
critical section in flight is aborted, and returns when that is done. An abort is something
every sequence must already tolerate, since preemption causes one at any instruction. No memory
changes.

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

**`unmap`: munmap.** The caller guarantees nothing uses the range afterwards.

**Partial failure.** mprotect and madvise can fail after acting on part of the range (ENOMEM
from a VMA split). The `Os` contract allows this, and no caller assumes either outcome.

## KERNEL-04: `write_stderr`

**Block.** `syscall(WRITE, [2, bytes.as_ptr(), bytes.len(), 0, 0, 0])`.

**Proof.** write(2) reads at most `bytes.len()` bytes from the buffer, which is a live slice for
the duration of the call. The pointer comes from a shared reference and the kernel only reads
it. A short write advances the slice by the count returned, capped at its length, and any error
or a zero count ends the loop, so the loop cannot run forever on a broken descriptor.

## KERNEL-05: `kernel_rseq_feature_size`

**Block.** `syscall(PRCTL, [PR_GET_AUXV, buffer, len, 0, 0, 0])`.

**Proof.** `PR_GET_AUXV` (Linux 6.4) copies at most `len` bytes of the auxiliary vector to
`buffer`, returns the vector's full size, and requires the last two arguments to be zero. The
buffer is a local `[[u64; 2]; 64]` of exactly `len` bytes, exclusively borrowed, so the write is
in bounds and aliases nothing. On older kernels the call fails with EINVAL and touches nothing.

**Why it exists (a correctness obligation that memory safety depends on).** The fast paths are
safe only if no two threads run with the same concurrency ID. glibc 2.35 to 2.39 report
`__rseq_size = 32` on any kernel. On a kernel before 6.3 the `mm_cid` field is then never
written, every thread reads zero, and all would share one slab. Capping glibc's size by the
kernel's `AT_RSEQ_FEATURE_SIZE` makes `has_cid` true only when the kernel maintains the field.
Every failure mode (no `PR_GET_AUXV`, no entry, a truncated copy, an out-of-range value) yields
20, the size before `mm_cid` existed. So an error can only switch the fast paths off, never on.
The cost is that on Linux 6.3 exactly, the fast paths are off.

## KERNEL-06: `abort`

**Blocks.** rt_sigaction and rt_sigprocmask; then getpid, gettid, tgkill and exit_group through
`int_call`.

**Proof.**

- rt_sigaction(SIGABRT, `&default`, null, 8) reads one `KernelSigaction` from a live local.
  The layout is the kernel's x86-64 `struct sigaction`: handler, flags, restorer, mask, which
  differs from glibc's. The size argument 8 is the kernel's `sigset_t`. The old-action pointer
  is null, so nothing is written back.
- rt_sigprocmask(SIG_UNBLOCK, `&set`, null, 8) reads one signal set from a live local.
- The pointers come from shared references and the kernel only reads them.
- getpid and gettid take nothing. tgkill(pid, tid, SIGABRT) with the default disposition
  restored and the signal unblocked ends the process with a core dump. exit_group(127) ends it
  if anything before failed, so the function does not return, as `-> !` requires.
- Changing the process's SIGABRT disposition and mask is this function's to do, because the
  process is ending. Restoring the default action on purpose skips any user handler, since a
  handler that allocated would re-enter a heap the allocator has just refused to continue with.

**`int_call` itself.** It forwards to `syscall` with three integer arguments. Its callers are
exactly the four calls above, none of which takes a pointer.

## KERNEL-07: `unsafe impl Os for LinuxOs`

The comment above the impl maps each promise of the `Os` trait to the kernel behaviour that
provides it, and KERNEL-01 to KERNEL-06 cover the individual calls. The promises not covered by a
single call:

- *Interchangeable values.* `LinuxOs` is zero-sized, and every value is the same process's OS.
- *Reserved memory reads as zero.* Fresh private anonymous memory is zero-filled on first touch.
- *Any thread, signal handlers, no allocation.* Every method is a fixed sequence of system calls
  and arithmetic on locals, all async-signal-safe.

**Machine checks.** The tests in `kernel.rs`:

- `reserve_commit_purge_and_unmap`: alignment, zero-fill, purge zeroes, unmap.
- `bad_arguments_are_errors_not_calls`.
- `the_design_reservation_is_available`: a 16 TiB reservation.
- `glibc_registers_rseq_with_a_concurrency_id` and
  `the_kernel_reports_a_feature_size_with_a_concurrency_id`.
- `rseq_fences_work_after_registering`.
- `abort_ends_the_process_with_sigabrt`: a child process, checking the signal and the message.

**Review.** Pending: needs an adversarial review by a fresh agent.
