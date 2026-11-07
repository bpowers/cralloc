# What the kernel and glibc promise about rseq

cralloc's fast path trusts five things about restartable sequences. Each held, in the source of
Linux 7.2.9 and glibc 2.43 and when attacked on the reference host. An earlier design also trusted
the range of the concurrency ID, and that failed: after an `execve` that returns, a thread can read
`mm_cid == 0x80000000`, on every kernel from 6.19 on. cralloc therefore indexes its caches by CPU
number and compares that number with the slab count on every call, so memory safety does not rest on
what the kernel writes.

| What we rely on | Verdict | Shown by |
|---|---|---|
| A thread preempted, migrated or signalled inside a sequence resumes at its abort handler | Holds, given a well-formed descriptor and no system call inside. A uprobe inside one breaks it (root only, from source) | source; `cid_stress.rs`; `rseq_abort.rs`; the soak's sweep |
| `cpu_id` is the CPU the thread runs on, changes only at an event that aborts its sequence, and is negative with no registration | Holds | source; `rseq_lifecycle.rs` |
| `membarrier` with the rseq command returns only after every sequence in flight there has completed or been aborted | Holds. Needs a one-time registration | source; `rseq_drain.rs` (process-wide form) |
| glibc registers a thread's area before the thread can call the allocator | Holds, from the first interposed call to the last free at thread exit | source; `rseq_lifecycle.rs`; `rseq_preload.rs` |
| A forked child is safe. A `vfork` or `CLONE_VM` child is not, and glibc never allocates in one | Holds | source; `rseq_lifecycle.rs`; `cid_stress.rs` |

**What we refuse to rely on:**

- **That `cpu_id` is in range.** Every routine compares it with 8192, unsigned, in one fused
  micro-op. A sign test saves 4 bytes and no cycles, and would let a number of 8192 or more index
  past the slabs.
- **Writing the ID fields, or their being rewritten at every switch.** tcmalloc's cached slab
  pointer needs both. Under a full-size registration the top bit of `cpu_id_start` survives 1000
  `sched_yield`s and a 300 ms spin, and the first migration kills the process (`rseq_readonly.rs`).
- **What `rseq_cs` held before.** Every routine arms first, every time, so a handler's own sequences
  are harmless.
- **Anything seen at start-up.** The code reads neither `__rseq_size` nor the kernel version. A
  negative `cpu_id` is the whole test, and the start-up check first proposed was never built.
- **Which registration mode a thread is in.** glibc registers 33 bytes, V2. Our own area on musl
  registers 32, the legacy mode. They differ in when IDs are written and whether writing them is
  fatal, not in when sequences abort.
- **`membarrier` succeeding.** A failed fence backs a drain out or keeps the background thread from
  parking, and a refused registration turns draining off.

Method, 2026-10-16: the release tarballs `linux-7.2.9` and `glibc-2.43` read at the lines cited,
older kernels from tags, then probes on a Ryzen 9950X, Linux 7.2.9-201.fc44, Rust 1.99.1. `core.c`
is in `kernel/sched/`, `rseq.c` is `kernel/rseq.c`, `rseq.h` and `rseq_entry.h` are in
`include/linux/`. The probes were written for the concurrency-ID design and read `mm_cid`. Their
abort and fence mechanics do not depend on which ID a section reads, and the soak puts the
CPU-indexed routines through the same events.

## Aborts

A registration longer than 32 bytes selects V2 (`rseq.c:429-430`). V2 checks `rseq_cs` only when the
thread entered the kernel from user mode by an interrupt or exception, and since then was scheduled
out, was hit by a `membarrier` interrupt, or is about to run a signal handler (`rseq.h:35-45`,
`62-77`; `rseq_entry.h:545-554`, `599-649`). The 32-byte legacy mode writes IDs on every switch and
checks before every signal, and after a context switch on x86-64 applies the same user-interrupt
condition. That is enough, because a thread inside a sequence can leave user mode only by an
interrupt or exception, unless it makes a system call.

| Event inside a sequence | Result |
|---|---|
| Timer or interrupt, then preemption or migration | abort |
| Page fault that sleeps or is preempted | abort |
| Page fault handled without a context switch | resumes in place; correct, since nothing else ran on this CPU |
| Asynchronous signal | abort before the frame is built; the handler later returns to the abort address |
| Synchronous signal, ptrace stop, breakpoint, single-step | abort; a sequence can never be single-stepped to its end |
| `membarrier` interrupt | abort |
| System call inside the sequence | **no abort**; with `rseq_debug` on, SIGSEGV (`rseq.c:363-380`) |
| uprobe inside the sequence | **no abort**, and `rseq_cs` is cleared: the probed instruction runs out of line, so the saved instruction pointer is outside when the check runs (`kernel/entry/common.c:58-59`, `103`). From source |

**What an abort changes.** The kernel sets the saved instruction pointer to the abort handler and
clears `rseq_cs` (`rseq_entry.h:437`). It changes nothing else the thread saved, so the thread
resumes with the registers, flags and FS base it was interrupted with. If a signal handler ran
first, `sigreturn` restores them from the frame. Memory may differ. The Lean model's `Target.rely`
is this statement.

**When `rseq_cs` is cleared.** At registration (`rseq.c:448`), at an abort, and when a check finds
the instruction pointer outside the range (`rseq_entry.h:443`). After a commit it keeps naming the
last descriptor until the thread is next interrupted and rescheduled. So a descriptor must stay
mapped for as long as a thread might name it, and a shared object that holds the routines must not
be unloaded.

**A malformed descriptor** gets SIGSEGV only if the instruction pointer is in range and `abort_ip`
or the signature before it is wrong (`rseq_entry.h:379-455`). `rseq_debug=1` also checks version,
flags, alignment, and that the abort address lies outside.

**What the generator emits.** A 32-byte descriptor, aligned to 32, in a writable section
`__rseq_cs`, since its absolute addresses need dynamic relocations in a shared object or PIE.
`start_ip` is the first instruction after the arming store, and the range ends just past the commit.
`abort_ip` is in `.text.unlikely`, with the signature `0x53053053` before it inside a `ud1`, and the
handler jumps back to the arming store. Each inlined copy has its own descriptor, by numeric local
labels. The commit is a plain store of a computed value (`Rust.checkRseq`), because a
read-modify-write would corrupt a word a drainer exchanged meanwhile.

## membarrier

cralloc uses both forms of `MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ`. **The drain fences one CPU**
(`MEMBARRIER_CMD_FLAG_CPU`). **The background thread fences every CPU** before it parks, as a
barrier against a slow path that loaded the thread's word with no fence of its own
(`depot/thread.rs`).

The process-wide form interrupts every online CPU whose current task has the caller's address space,
except the caller's own, and waits for all handlers (`kernel/sched/membarrier.c:398-438`). The
per-CPU form, read at tag v7.2 while writing this note, does the same for one CPU (`:374-396`),
running the handler locally if that CPU is the caller's. The handler takes a full barrier and raises
the rseq event on whatever is running, so a thread interrupted in user mode is aborted before it
returns.

Threads not running need no interrupt. One preempted inside a sequence was preempted by an
interrupt, so its abort is pending. One scheduled out elsewhere is not in a sequence, and anything
it starts later begins after the caller's stores: the call has a full barrier at entry and exit,
paired with the scheduler's.

- **Registration.** `MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ`, once per address space, or
  `EPERM`. A forked child inherits it (observed); `execve` clears it.
- **Failure.** `EPERM` unregistered; `EINVAL` before 5.10 or without rseq; `ENOMEM` only from the
  process-wide form, which allocates a CPU mask. So a drain's back-out answers `EPERM`, `EINVAL` or
  a seccomp filter, not memory pressure.
- **Shortcuts, all correct.** One user of the address space or one online CPU returns at once. A CPU
  number past `nr_cpu_ids`, not online, or running another address space returns 0 with no
  interrupt.
- **Cost.** 3.5 to 5.7 microseconds a process-wide call run alone, 18 and 26 beside stress runs,
  with 8 to 24 CPUs busy in the process. Process-wide calls share one system-wide mutex
  (`:172-175`), per-CPU calls a mutex per CPU (`:176`). The per-CPU form's cost was not measured.

## glibc

**The area** sits at the end of static thread-local storage, sized from `AT_RSEQ_FEATURE_SIZE`.
`__rseq_offset` is always set. `__rseq_size` is the feature size, 33 on the host, or 0 if the main
thread's registration failed or was disabled (`sysdeps/nptl/dl-tls_init_tp.c:105-114`). The offset
follows the binary's static TLS layout, so it is read at run time.

**Main thread.** `dl_main` calls `init_tls`, which sets the thread pointer and registers rseq,
before it relocates any loaded object (`elf/rtld.c:733-784`, `2016`, `2256-2292`). The loader uses
its private allocator until relocation is complete and only then switches to the interposed `malloc`
(`rtld.c:2340`). So no interposed call comes from an unregistered main thread, and the symbols are
valid whenever one arrives.

**New threads and exit.** `pthread_create` blocks all signals before `clone`
(`nptl/pthread_create.c:805`). The new thread registers near the top of `start_thread` (`:382-387`)
and restores the mask after that (`:438`), and nothing before the registration allocates. glibc
never unregisters: thread-local destructors, key destructors and the release of the control block
all run registered (`:459-465`, `598`).

### What the fields read

| State | `cpu_id` | `__rseq_size` |
|---|---|---|
| Registered | the CPU | 33 on this kernel |
| Disabled by `GLIBC_TUNABLES=glibc.pthread.rseq=0`, or refused (valgrind, seccomp) | -2 | 0 |
| New thread, before it registers | 0 on a fresh stack; on a reused stack perhaps a dead thread's value (not checked) | unchanged |
| After explicit unregistration (glibc never does this) | -1 | unchanged |
| The crate's own area on musl, until its first slow path | -1 | not present |

The unsigned compare sends both negative values to the slow path. The third row does no harm,
because nothing allocates in that window, but it is why "does this look like a CPU number" is no
test on an arbitrary thread.

**Static programs.** `libc.a` has the same symbols and registers the main thread in
`__libc_setup_tls`, and static builds pass the allocator's tests ([portability.md](portability.md)).
IFUNC relocations apply before TLS is set up (`csu/libc-start.c:277`, then `280`), so a resolver
that allocates would run with no thread pointer.

**Outside glibc's control.** A thread made with raw `clone` and its own TLS has no area where
`__rseq_offset` points. An IFUNC resolver in another library that calls `malloc` can run before a
preload library is relocated.

## fork, vfork and clone

**fork.** The child copies the parent's rseq state, registration included (`rseq.h:155-161`), and
its IDs are marked changed (`core.c:4982`), so the area is rewritten before its first user
instruction. 64,594 and 59,388 children forked under stress all ran sequences correctly.

**vfork and `CLONE_VM`.** The kernel clears the child's registration (`rseq.h:157-158`). It runs on
the parent's thread pointer, so it reads the parent's `cpu_id` and nothing aborts its sequences
(observed). So such a child must not allocate, a non-goal in the design.

**glibc does not allocate in one.** `posix_spawn` clones with `CLONE_VM | CLONE_VFORK` onto a stack
the parent mapped, with all signals blocked, and the child only makes system calls and then `execve`
(`sysdeps/unix/sysv/linux/spawni.c:104-141`, `384-427`, `493-496`). `system` is `posix_spawn`. A
preload library that asks the kernel on every call found no call from an unregistered thread in
about 55,000, across `ls`, shell pipelines, `perl`, `cargo` and threaded Python with `subprocess`
and `os.posix_spawn` (`rseq_preload.rs`).

## Why we do not use `mm_cid`

The area also carries a concurrency ID, a compact numbering of a process's running threads. Since
the November 2025 rework a task owns its ID until threads pass 1.25 times its allowed CPUs, and then
the kernel moves to per-CPU ownership. A cache indexed by it would act as a thread cache when
threads are few and a bounded CPU cache when they are many, for the same instructions at another
offset. We evaluated it, built it first, and replaced it on 2026-10-17, for three reasons.

1. **A thread without a registration reads 0, which is a valid ID.** It would share another thread's
   cache unprotected, and telling the two apart costs instructions on every call. It reads a
   negative CPU number, which the bound check already rejects.
2. **The ID is in fewer kernels:** 6.3 on, with ownership modes from 6.19. `cpu_id` is in every
   kernel with rseq.
3. **Current kernels can show a garbage ID.**

**The defect.** `bprm_execve` gives up the caller's ID before trying the exec (`fs/exec.c:1791`).
When the exec does not go through it adds the caller back and forces an rseq update (`:1797-1798`,
`1823-1824`). `sched_mm_cid_after_execve` is `sched_mm_cid_fork`, which assigns an ID only in
per-task mode (`core.c:11071-11075`). In per-CPU mode a new task gets its ID when first scheduled
in, but this task is already running and holds `MM_CID_UNSET`, bit 31. `task_mm_cid` masks off only
bits 29 and 30, and the forced update writes the result to user space. The triggers: a file no
binary format accepts (`ENOEXEC`), a script with a missing interpreter, and
`execveat(AT_EXECVE_CHECK)`, which returns 0 without executing. A denial inside the exec path takes
the same route. A missing file or a permission error fails earlier and does not.

| Threads, pinned to 4 CPUs | Trigger | Reads of `0x80000000` in 20,000 attempts (`cid_exec_fail.rs`) |
|---|---|---:|
| 2, 5 | `ENOEXEC` | 0 |
| 6, 40 | `ENOEXEC` | 19,956 |
| 40 | missing interpreter | 19,945 |
| 40 | `AT_EXECVE_CHECK` | 19,951 |
| 2 | `AT_EXECVE_CHECK` | 0 |

The step from 5 to 6 threads is the threshold for per-CPU mode. The value lasted 6 to 14 ms where
sampled, and on an idle CPU nothing bounds it. v6.18 and earlier assigned an ID immediately and are
not affected. v6.19 to 7.3-rc7 have this code, and `sched_mm_cid_fork` and `task_mm_cid` read the
same at 7.4-rc1 (read 2026-11-06, not run). A fast path that adds the ID to a static array unchecked
reads a word a terabyte or more past it. A realistic trigger is a server pinned to four CPUs, with a
blocking pool, that tries to run a script with a bad `#!` line and frees the `Command`.

**What did hold.** An ID comes from a per-process bitmap under the runqueue lock and changes only
while the thread is switched out, so two running threads never share one and a change always aborts.
`cid_stress.rs` tried to break that for 1.0e12 commits, 9.6e8 aborts and 60,000 ownership-mode
flips, over four runs of 300 to 600 s, under affinity, signal, `membarrier`, fork and page-fault
storms: no lost update, no foreign owner, no ID of 8192 or more. Its control, never arming with 72
threads, gave 14,200 foreign owners and 1.6e9 lost updates in 4 s.

## Kernels below 7.2

Developed and run on 7.2.9; the others were read, not run.

| What the design needs | First release | How checked |
|---|---|---|
| rseq aborts on preemption, migration, signal | 4.18 | the kernel's history, not re-read |
| `membarrier` rseq command, and its per-CPU flag | 5.10 | absent in v5.9, present in v5.10 |
| `rseq_cs` checked only after a user-mode interrupt | 6.19 | present in v6.19 |
| V2: IDs written only on change, read-only fields enforced | 7.1 | v7.0 has neither `rseq_v2` nor the check outside debug mode |

## Which probe shows what

In `bench/probes/`, whose `README.md` has the build lines. None runs the allocator.

| Probe | Result |
|---|---|
| `rseq_abort.rs` | an abort forced before each of six instructions by a perf hardware breakpoint with `sigtrap`, unprivileged; nothing committed, and the retry succeeds |
| `rseq_auxv.rs` | `AT_RSEQ_FEATURE_SIZE=33`, `AT_RSEQ_ALIGN=64`, `__rseq_size=33` |
| `rseq_readonly.rs` | the write to `cpu_id_start` survives yields and spinning; migration kills the process |
| `rseq_lifecycle.rs` | registered in a constructor, `main`, a new thread, thread-local and key destructors, a forked child, `atexit`; not in a `CLONE_VM` child. With the tunable, `cpu_id=-2` everywhere |
| `rseq_preload.rs` | no allocator call from an unregistered thread in about 55,000 |
| `rseq_drain.rs` | 5.8 million owned drains over 2.8e11 commits, at 8 and 24 CPUs, 300 s each: none committed after the call returned. Without the barrier, 137,660 late commits in 20 s |
| `cid_stress.rs`, `cid_exec_fail.rs` | "Why we do not use `mm_cid`" |

## Open questions

- **The defect has not been reported upstream, as far as the repository records.** The fix is to
  give the task an ID in that path. Sending it is the project owner's decision.
- **No probe isolates the per-CPU fence.** `rseq_drain.rs` uses the process-wide form. The per-CPU
  form rests on the source, a test that it returns success, and the soak's modes that drain.
- **Hotplug, uprobes inside a sequence, and `rseq_debug=1`** need root and were not run. The first
  two are argued from source.
- **Static IFUNC resolvers** that allocate were not tested.
- **Stress shows absence of observed failures.** The source argument is stronger, and it is one
  reader's.
