# Safety ledger: `crates/asmalloc/src/os/linux/kernel.rs`

`LinuxOs`, the `Os` implementation over system calls. Every unsafe block here but one calls
`syscall()` (SYSCALL-01) or a thin helper over it, so each entry's obligation is that the
kernel's contract for the call is met and that the call changes only what the caller may change.
The exception is the timestamp-counter read in `ticks` (KERNEL-09).
Neither Kani nor Miri can run this file; the unit tests in it run every call on the real kernel.

References are to the Linux man pages (mmap(2), mprotect(2), madvise(2), munmap(2),
membarrier(2), getrandom(2), rt_sigaction(2), sigprocmask(2), tgkill(2), nanosleep(2),
write(2)) and to
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

## KERNEL-06: `abort`, `claim`, and the calls they make

**Blocks.** rt_sigprocmask in `unblock_sigabrt`; getpid and gettid in `Aborter::current`;
tgkill with signal 0 in `Aborter::is_alive`; tgkill with SIGABRT in `Aborter::raise_sigabrt`;
nanosleep in `sleep_step`; rt_sigaction and exit_group in `abort_with`.

**Order.** The thread that claims `ABORTING` (below) unblocks SIGABRT and raises it with the
program's disposition in place, as glibc's `abort` does, so a crash reporter's handler runs.
The heap is valid at that point: the allocator refused to go on, it did not break. If the
handler returns, or ignores the signal, or calls back into `abort` on the same thread, `abort`
restores the default action, unblocks and raises again, and ends with exit_group if even that
returns. A thread that does not claim goes straight to that second step, after waiting if it
must.

**The entry.** `ABORTING` is one `AtomicU64`: zero, or the process ID (high half) and thread
ID (low half) of the thread that claimed it, as getpid and gettid report them. Both IDs are
positive and at most 2^22 (`PID_MAX_LIMIT`), so the casts to `u32` keep them and zero names no
thread. It changes only by one compare-and-swap from the value the thread judged, and is never
cleared, since the process is ending. A fork copies it, which is why it names the process.

**What an aborting thread does** (`judge`, then `claim`), for every value it can find:

1. *Zero.* No one is aborting. It claims the entry and runs the handler.
2. *Its own process and thread IDs.* Its own handler called back in (by allocating into the
   failure that brought it here, say). It goes to the default action.
3. *Another process's IDs.* Stale. A fork copied the entry: the child of a handler that forks
   (crash reporters do), or of any other thread that forked while the handler ran. Or a
   process that shares this memory without being a thread of it (a vfork child) wrote it. No
   thread of this process will finish that abort, so it claims the entry and runs the handler.
   This is the finding abort-parks-forked-child: before, the child kept the parent's thread ID
   and parked for ever.
4. *This process, a thread it no longer has.* Stale: tgkill(pid, tid, 0) fails with ESRCH (the
   first aborter's handler left and its thread exited). It claims the entry and runs the
   handler. A thread that is exiting is still found for a moment, so it may first count as
   alive (case 5) for a step.
5. *This process, a live thread.* It waits, in steps of 10 ms, judging the entry again after
   each, for at most `PATIENCE` steps (about ten seconds). Usually the first aborter ends the
   process meanwhile. If the entry turns stale (case 4), it claims it. If its patience runs out,
   it goes to the default action, ending the process under the first thread's handler. That
   happens when the handler is blocked on something the waiting thread holds (a lock on a
   stream, if the waiting thread aborted from inside a C library call holding one), when the
   handler is slower than ten seconds, or when the handler escaped (longjmp) and its thread
   lives on.

**Why no abort parks for ever, by construction.** Every case ends in the handler (cases 1, 3,
4, then the default action), the default action (case 2), or a wait of at most `PATIENCE`
steps (case 5). No case depends on another thread making progress. `sleep_step` resumes an
interrupted nanosleep with the time remaining, which the kernel computes from the original
expiry, so a step lasts its 10 ms of wall-clock time and no more, and a burst of signals
cannot shorten the wait. A compare-and-swap in `claim` fails only when another aborting thread
changed the entry; each thread's swap succeeds at most once and then it returns, so a thread
retries at most once per other aborting thread. The one thing without a bound is the
program's own handler on the claiming thread; a handler that never returns hangs glibc's
`abort` too.

**What it can misread, and what then happens.** Each still ends the process.

- A descendant can get the process ID of an aborting process after that process is reaped, and
  inherit its entry through a fork chain. The entry then looks like this process's, and the
  liveness check decides: no such thread, claim (case 4); a thread with that ID, wait (case 5).
- A thread ID can be reused within the process after the first aborter's thread exits. The new
  thread's abort reads as nested (case 2) and skips the handler.
- An exited main thread stays a zombie while other threads live, so tgkill finds it and the
  waiter waits out its patience.
- tgkill failing for any reason but ESRCH (a seccomp filter, say) counts as alive: a bounded
  wait, not a wrong claim.

**What glibc 2.43 does**, from its source at tag `glibc-2.43` (`stdlib/abort.c`,
`signal/sigaction.c`, `sysdeps/nptl/_Fork.c`, `posix/fork.c`,
`sysdeps/unix/sysv/linux/spawni.c`, read 2026-10-17). `abort` raises SIGABRT first, with no
lock, so any number of threads can run the handler at once. Only when a raise returns does
that thread take the abort lock, a reader-writer lock, on the write side with all signals
blocked; it never releases it, resets SIGABRT to the default and raises again.
`sigaction(SIGABRT)` takes the write side, and `_Fork` and `posix_spawn` the read side, so no
fork starts once a thread is in that second phase, and a multithreaded fork reinitializes the
lock in the child (`__abort_fork_reset_child`). The lock orders the reset against sigaction,
fork and spawn. It does not protect a handler that is running: the first thread whose handler
returns ends the process under every other thread's handler. The earlier text here said the
parked threads waited "as they would on glibc's abort lock"; that was wrong. A handler that
longjmps away leaves no glibc thread waiting, since nothing takes the lock before the raise.
This design differs from glibc on purpose: a second aborter does not run the handler alongside
the first, and waits, for a bounded time, rather than cutting the first one short. A child
forked mid-abort starts with a usable `abort` in both.

**Rules.** No lock: a waiting thread holds nothing and the first aborter waits for nothing.
No thread-local state: the entry is one static word, all-zero valid, and changes by one
compare-and-swap. Nothing allocates. The wait is the only one in the crate. It happens only in
a process that is ending, and it is bounded.

**Proof of the blocks.**

- rt_sigprocmask(SIG_UNBLOCK, `&set`, null, 8) reads one signal set from a live local.
- rt_sigaction(SIGABRT, `&default`, null, 8) reads one `KernelSigaction` from a live local.
  The layout is the kernel's x86-64 `struct sigaction`: handler, flags, restorer, mask, which
  differs from glibc's. The size argument 8 is the kernel's `sigset_t`. The old-action and
  old-set pointers are null, so nothing is written back.
- nanosleep(`&request`, `&remaining`) reads one `struct timespec` (two 64-bit words on x86-64,
  `tv_sec` then `tv_nsec`, here 0 and 10^7) from a live local and, only when interrupted,
  writes one to `remaining`, a live, exclusively borrowed local of the same size.
- The other pointers come from shared references and the kernel only reads them.
- getpid and gettid take nothing. tgkill(pid, tid, 0) sends nothing: signal 0 only checks that
  thread `tid` is in thread group `pid` and may be signalled. tgkill(pid, tid, SIGABRT) runs
  the disposition in force: the program's handler the first time, the default (end the process
  with a core dump) the second. exit_group(127) ends the process if both raises returned, so
  the function does not return, as `-> !` requires.
- Changing the process's SIGABRT disposition and mask is this function's to do, because the
  process is ending. A handler that allocates re-enters a heap that is still consistent; at
  worst it aborts again, which case 2 turns into the default action.
- `ABORTING` holds no allocator state.

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
  `marked_pages_read_as_zero_in_a_forked_child`.
- `abort_ends_the_process_with_sigabrt` and
  `abort_runs_the_programs_handler_then_ends_the_process_anyway`: child processes, checking
  the signal, the message and that a returning handler ran.
- KERNEL-06's cases, each in a child process that the test kills and fails if it outlives a
  deadline: `a_second_aborting_thread_lets_the_first_handler_finish` (case 5, the handler
  finishes only after it has seen the second thread in nanosleep),
  `a_second_aborting_thread_ends_the_process_if_the_first_handler_blocks` (case 5, patience
  spent), `an_abort_in_a_child_forked_during_an_abort_ends_the_child` (case 3, from a fork),
  `an_abort_claims_an_entry_from_another_process` (case 3, planted) and
  `an_abort_claims_an_entry_whose_thread_is_gone` (case 4). In process:
  `the_rule_for_an_entry_covers_every_case` (`judge`), `liveness_is_asked_of_this_process`
  and `a_claim_replaces_a_stale_entry_and_waits_for_a_live_one`.

**Review.** 2026-10-17, fresh adversarial reviewer: KERNEL-01 to KERNEL-07 accepted, after
the fixes of its first round. Its second round found that a child forked mid-abort parked for
ever, that a waiting thread could hang the process, and that the glibc comparison and
KERNEL-09 were wrong or stale; KERNEL-06 and KERNEL-09 were rewritten for those and need the
same reviewer's check.
