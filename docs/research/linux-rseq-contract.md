# The Linux and glibc contract behind the rseq fast path

Date: 2026-10-16. Host: Ryzen 9950X (32 CPUs), Linux 7.2.9-201.fc44, glibc 2.43, Rust 1.99.1.

**Status, 2026-10-17.** This note audits an earlier design that indexed the caches by
concurrency ID. The design now indexes them by CPU
number (`cpu_id`, rseq offset 4, bounds-checked against the slab count) and drains a cache
with a membarrier fence aimed at one CPU (`MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ` with
`MEMBARRIER_CMD_FLAG_CPU`). The concurrency-ID findings (items 1a to 1c, 6, and section 1 of
the evidence) are kept as a record of that design and do not describe the current one. The
findings on aborts, membarrier, glibc's registration and fork (items 2 to 5) still apply,
except that the per-CPU form of the membarrier command was not part of this audit.

The audited design's fast path pops and pushes pointers in a cache chosen by the thread's
concurrency ID (`mm_cid`, "cid"), inside a restartable sequence, with no atomics. `docs/design.md` lists
what that trusts under "What is trusted". This note checks each item against the source of
the kernel and C library the host runs, and then tries to break it on the host.

Method. Sources are the release tarballs `linux-7.2.9.tar.xz` (cdn.kernel.org) and
`glibc-2.43.tar.xz` (ftp.gnu.org); every `file:line` below refers to those. Short names:
`core.c` and `sched.h` are in `kernel/sched/`, `rseq.c` is `kernel/rseq.c`, `rseq.h` and
`rseq_entry.h` are in `include/linux/`, `membarrier.c` is `kernel/sched/membarrier.c`;
anything else is written out. Older kernels
were read from the tags `v5.9` to `v7.2` and `master` (7.3-rc7) of torvalds/linux. The mm_cid
and rseq files of 7.2.9 are identical to those of v7.2. Probes are in `bench/probes/`.

## Verdicts

| # | Assumption | Verdict |
|---|---|---|
| 1a | Two threads of a process never run user code with the same cid at once | Holds |
| 1b | A thread's cid changes only at an event that aborts its sequence | Holds |
| 1c | Cids are below 8192 | **Does not hold.** After an `execve` that returns, the caller's cid can read `0x80000000` |
| 2 | A thread preempted, migrated or signalled inside a sequence resumes at the abort handler | Holds, given a well-formed descriptor and no system call inside the sequence. A uprobe placed inside a sequence breaks it (root only; from source, not tested) |
| 3 | `membarrier` with the rseq command returns only after every sequence in flight has completed or been aborted | Holds. Needs a one-time registration; a call can fail with ENOMEM, which the drain must handle |
| 4 | glibc registers a thread's rseq area before the thread can call the allocator | Holds for dynamically linked programs, from the first interposed call to the last free at thread exit. In a static program, IFUNC resolvers run before the thread pointer exists |
| 5 | A forked child is safe; a `vfork`/`CLONE_VM` child is not, and glibc never allocates in one | Holds |
| 6 | Kernels below 7.2 | The cid code is the same from 7.0 (one kernel-internal fix apart). 6.19 differs and was not audited; 6.18 and older use a different scheme. See the table in section 6 |

One assumption fails, and it fails in a way that corrupts memory, so it comes first.

## The failure: an out-of-range cid after `execve` returns

**What happens.** In a process that has more threads than 1.25 times the CPUs it is allowed
to run on, a thread whose `execve` returns to it reads `mm_cid == 0x80000000` from then
until its next context switch. Three ways to get there were reproduced on the host:

- the file is executable but no binary format accepts it (ENOEXEC);
- the file is a script whose interpreter does not exist (ENOENT);
- `execveat(..., AT_EXECVE_CHECK)`, which checks that a file could be executed and returns
  0 without executing it.

A denial by a security module inside the exec path takes the same route. A missing file or
a permission error on the file itself does not: those fail before the cid is touched.

**Why.** `bprm_execve` gives up the caller's cid before trying the exec
(`sched_mm_cid_before_execve`, `fs/exec.c:1791`). When the exec does not go through it adds
the caller back and forces an rseq update (`fs/exec.c:1797-1798`, `1823-1824`).
`sched_mm_cid_after_execve` is `sched_mm_cid_fork` (`kernel/sched/core.c:11191-11195`), which
assigns a cid only in per-task mode: `if (!cid_on_cpu(mm->mm_cid.mode)) t->mm_cid.cid =
mm_get_cid(mm);` (`core.c:11071-11075`). In per-CPU mode a new task gets its cid when it is
first scheduled in, but this task is already running and holds `MM_CID_UNSET`, which is bit
31 (`include/linux/rseq_types.h:136`). `task_mm_cid` masks off only bits 29 and 30
(`include/linux/sched.h:2375-2378`), and the forced update writes the result to user space
(`include/linux/rseq_entry.h:639-646`, `503-506`). The kernel's own comment at
`core.c:10979-10989` acknowledges the window ("the fork()/execve() window leaves the task
active with no owned CID") but treats it only as an internal hazard.

**Measured** (`bench/probes/cid_exec_fail.rs`, pinned to 4 CPUs, 20,000 attempts each):

| Threads | Trigger | Reads of `0x80000000` straight after the call |
|---|---|---|
| 2 | ENOEXEC | 0 |
| 5 | ENOEXEC | 0 |
| 6 | ENOEXEC | 19,956 |
| 40 | ENOEXEC | 19,956 |
| 40 | missing interpreter | 19,945 |
| 40 | `AT_EXECVE_CHECK` | 19,951 |
| 2 | `AT_EXECVE_CHECK` | 0 |

The step between 5 and 6 threads on 4 CPUs is the kernel's threshold for per-CPU mode
(users above `min(1.25 * allowed CPUs, possible CPUs)`, `core.c:10842-10845`, `10872`). The
value lasted 6 to 14 ms in the sampled cases; on an otherwise idle CPU nothing bounds it.
With all 24 CPUs allowed, 40 and 96 threads reproduce it at the same rate.

**Which kernels.** The code is the same in v6.19, v7.0, v7.1, v7.2, 7.2.9 and 7.3-rc7. v6.18
and earlier assigned a cid immediately in `sched_mm_cid_after_execve` and are not affected.
It should be reported upstream; the fix is to give the task a cid in that path.

**Effect on asmalloc.** The fast path as designed shifts the cid and adds it to the address
of a static array "with nothing to bounds-check". With `0x80000000` it reads a slab word
about a terabyte or more past the array. Usually that faults. If something is mapped there
and the word passes the mask test, the routine stores through a wild pointer. A realistic
trigger is a server in a container pinned to four CPUs, with a blocking-thread pool, that
tries to run a script with a bad `#!` line and then frees the `Command` it built.

A start-up check cannot see this defect without provoking it.

## Proposed design changes

1. **Bound-check the cid in every routine (required).** After loading the cid, compare it
   with 8192 and leave by the slow exit if it is not below: one compare-and-branch pair that
   fuses into a single micro-op and is never taken in normal running. The slow path treats
   an out-of-range cid as "no rseq for this call" and serves the request from the back end,
   as it already does when there is no registration. In "What is trusted", replace "Cids are
   below 8192" with "a thread whose cid is below 8192 shares it with no running thread".
   The Lean rseq model should let the cid be any 32-bit value.

   If the extra micro-op proves unaffordable in the two hot routines, there is a zero-cost
   variant, to be measured before it is believed: load the cid sign-extended (`movsxd`), so
   that bit 31 indexes below the array, and at start-up map a read-only zero page where
   that index lands. Both hot routines read a zero word as "no magazine" before they store
   anything. It depends on that address being free, so it needs a fallback, and the
   single-store install routines still need the explicit check. The plain check is the
   recommendation.

2. **Say what the drain does when `membarrier` fails.** The call can return ENOMEM
   (`membarrier.c:401-402`). The drainer has then swapped the marker in and
   cannot claim the old value. It should put the old value back with a compare-and-swap
   against the marker and stop. If the swap fails, a sequence in flight has committed and
   there is nothing to restore; if it succeeds, a sequence in flight may still commit over
   it, which is the same as if the drain had never started.

3. **Start-up check.** Use the fast path only if `__rseq_size >= 28` (the area includes
   `mm_cid`) and the calling thread's `cpu_id` is not negative. Enable draining only if
   `MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ` returns 0. If the team wants a version
   floor as well, 7.0 is the first release whose cid code matches what was audited here.

Nothing else in the design needs to change. The requirements on generated code that the
audit turned up are listed in section 2.

## Evidence

### 1. Concurrency IDs

**Where a cid lives.** Each address space has a bitmap of cids with one bit per possible CPU
(`include/linux/mm_types.h:1592-1596`). A cid is owned by a task (`task_struct::mm_cid.cid`)
or by a CPU (`mm::mm_cid.pcpu->cid`, marked `MM_CID_ONCPU`); ownership mode is per process
(`core.c:10723-10744`). The value user space sees is the owned number with the two flag bits
removed (`include/linux/sched.h:2375-2378`).

**Uniqueness.** A number is handed out only by `test_and_set_bit` on the bitmap
(`sched.h:3920-3929`) and returned by `clear_bit` (`sched.h:3897-3900`). In
per-task mode each thread holds its own bit. In per-CPU mode the bit belongs to a CPU and
the thread running there borrows it at schedule-in (`mm_cid_from_cpu`, `sched.h:3974-4007`);
one CPU runs one thread. All of this happens under the CPU's runqueue lock.

**When the number changes.** The user-visible number of a thread changes in exactly these
places: at its schedule-in (`mm_cid_schedin`, `sched.h:4042-4056`); at its schedule-out when
a transition is under way, which only releases it (`mm_cid_schedout`, `sched.h:4058-4085`);
and in the fork, exec and exit paths, which run inside system calls. Each schedule-in that
changes it sets `ids_changed` (`mm_cid_update_task_cid`, `sched.h:3961-3967`), and
`rseq_sched_switch_event` then forces an update before the thread returns to user space
(`include/linux/rseq.h:53-85`, called at `core.c:5523-5529`).

**Mode transitions.** These are the dangerous part: a thread calling `clone`, exiting, or a
worker rewrites state that belongs to threads running on other CPUs. The rewrite never
changes a running thread's number. `mm_cid_fixup_tasks_to_cpus` and
`mm_cid_fixup_cpus_to_tasks` take each runqueue lock and then, for a thread that is running,
only set the `MM_CID_TRANSIT` flag on the number it already has (`core.c:10944-10952`,
`10966-10996`, `11001-11020`); for a thread that is not running they release its cid. A
thread with a transit cid releases or converts it when it next schedules out, and gets its
next number at schedule-in like any other. A stale per-CPU value left from the other mode is
never trusted: schedule-in takes the fast path only when both the task's and the CPU's
values carry the flag of the current mode (`sched.h:3982`, `4017`), and otherwise allocates
from the bitmap.

So a thread can lose or change its number only while it is switched out, and a thread
switched out in the middle of a sequence is aborted when it comes back (section 2). That
is assumption 1b, and with bitmap ownership it gives 1a.

**Affinity.** `sched_setaffinity` only widens the per-process count of allowed CPUs
(`core.c:10895-10932`); it can lead to a switch back to per-task mode through the same
fixup. Migration sets `ids_changed` (`sched.h:2377-2390`).

**Hotplug.** An offlined CPU's tasks are migrated, which is a context switch for each. The
CPU keeps the cid it owned, which costs a number but breaks nothing. Not tested: it needs
root.

**Threads created and exiting.** A new thread is added by `sched_mm_cid_fork` before it
first runs (`core.c:4935-4940`, `11048-11091`) and an exiting thread is removed by
`sched_mm_cid_exit` on its own way out (`kernel/exit.c:959`). Either can trigger a mode
switch, handled as above.

**Bound.** Every allocation searches at most `num_possible_cpus()` bits
(`sched.h:3931-3940`, `core.c:10845`), so a valid cid is at most `num_possible_cpus() - 1`.
x86-64 caps `NR_CPUS` at 8192 (`arch/x86/Kconfig:1016-1021`; the host's kernel is built with
`CONFIG_NR_CPUS=8192`, and has 32 possible CPUs). An array of 8192 slabs is therefore large
enough for every valid cid on any mainline x86-64 kernel. The one invalid value is the
subject of the section above. With `CONFIG_SCHED_MM_CID` off, the field holds the CPU number
(`include/linux/sched.h:2383-2391`), which has the same bound and the same exclusivity.

### 2. Aborts

**When the kernel looks at `rseq_cs`.** glibc registers 33 bytes on this kernel (section 4),
and a registration longer than 32 bytes selects "V2" (`kernel/rseq.c:429-430`). V2 does not
check `rseq_cs` on every return to user space. It checks when both hold: the thread entered
the kernel from user mode by an interrupt or exception (`user_irq`, set in
`irqentry_enter_from_user_mode`, `include/linux/irq-entry-common.h:291-295`), and since then
it was scheduled out, was hit by a `membarrier` IPI, or is about to run a signal handler
(`rseq.h:35-45`, `62-77`; `rseq_entry.h:545-554`, `599-649`). The 32-byte legacy mode
writes the ID fields on every switch and checks `rseq_cs` before every signal, but after a
context switch on x86-64 it applies the same user-interrupt condition
(`rseq_entry.h:545-550`). So the registration size changes when IDs are
written and whether user-space writes to them are fatal, not when sequences abort.

That condition is enough, because a thread inside a sequence can leave user mode only by an
interrupt or exception, unless it makes a system call:

| Event inside a sequence | Path | Result |
|---|---|---|
| Timer or IPI, then preemption or migration | interrupt entry sets `user_irq`; schedule-in raises `TIF_RSEQ`; exit-to-user loop checks (`kernel/entry/common.c:97-107`) | abort |
| Page fault that sleeps or is preempted | `exc_page_fault` uses `irqentry_enter` (`arch/x86/mm/fault.c:1533`), same as above | abort |
| Page fault handled without a context switch | nothing checks | resumes in place; correct, nothing else ran on this cid |
| Asynchronous signal | interrupt entry, then `rseq_signal_deliver` before the frame is built (`arch/x86/kernel/signal.c:236-239`) | abort; the handler later returns to the abort address |
| Synchronous signal (the sequence itself faults) | exception entry, then as above | abort |
| ptrace stop, breakpoint, single-step | exception entry, the tracee sleeps in the stop | abort; a sequence can never be single-stepped to its end |
| `membarrier` IPI | section 3 | abort |
| System call inside the sequence | `user_irq` is clear | **no abort**; with `rseq_debug` on, SIGSEGV (`rseq.c:363-380`) |
| uprobe on an instruction inside the sequence | the probed instruction runs out of line, so the saved instruction pointer is outside the sequence when the check runs (`kernel/entry/common.c:58-59` runs before the check at `103`) | **no abort** and `rseq_cs` is cleared. From source; not tested, needs root |

The design's routines make no system call. The uprobe case is a hazard for whoever traces
the allocator: probes on a routine's entry are fine, probes inside a sequence are not.

**When `rseq_cs` is cleared.** By the kernel at registration (`rseq.c:448`), when it aborts
a sequence (`rseq_entry.h:437`), and when a check finds the instruction pointer outside the
range (`rseq_entry.h:443`). Never otherwise: after a commit the field keeps pointing at the
last descriptor until the thread is next interrupted and rescheduled. Two consequences. The
descriptor must stay mapped for as long as any thread might still point at it, so a library
containing routines must not be unloaded. And a signal handler that runs its own sequences
and returns leaves the interrupted code with a different `rseq_cs`, which is harmless only
because every routine arms as its first step.

**Malformed descriptors.** On a production kernel the check reads `start_ip`,
`post_commit_offset` and `abort_ip`; if the instruction pointer is in range it requires
`abort_ip` to be a user address and the four bytes before it to equal the registered
signature; otherwise the thread gets SIGSEGV (`rseq_entry.h:379-455`, `rseq.c:303-310`). It
does not check version, flags, alignment, or that the abort address is outside the sequence
("If user space screwed it up, it can keep the pieces", `rseq_entry.h:403-405`). With
`rseq_debug=1` it checks all of those and kills on any failure (`rseq_entry.h:292-369`). A
descriptor in unmapped memory is SIGSEGV. Writing the ID fields is SIGSEGV at the next
update under V2 (`rseq_entry.h:485-501`).

**What a routine emitted through `asm!` must satisfy.**

- The descriptor is 32 bytes: `u32 version = 0`, `u32 flags = 0`, `u64 start_ip`,
  `u64 post_commit_offset`, `u64 abort_ip` (`include/uapi/linux/rseq.h:57-66`); align it
  to 32.
- `start_ip` is the first instruction after the arming store. `start_ip +
  post_commit_offset` is the address just past the committing store.
- `abort_ip` is outside that range, and the four bytes before it are `0x53053053`, the
  signature glibc registers on x86. Execution must not fall into those bytes.
- The descriptor holds absolute addresses, so in a shared object or PIE it needs dynamic
  relocations: put it in a writable section (`"aw"`), as the existing probes do.
- Each inlined copy of a routine needs its own descriptor; local labels and
  `.pushsection` give that.
- No system call, and nothing that can trap by design, between `start_ip` and the commit.
- Arm first, every time. Do not rely on `rseq_cs` being zero or on its previous value.

### 3. membarrier

`MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ` sends an IPI to every online CPU whose current task
has the caller's address space, except the caller's own CPU, and waits for all handlers to
finish (`membarrier.c:398-438`). The handler raises the rseq event on whatever is running
there (`membarrier.c:210-230`). A thread interrupted in user mode has `user_irq` set by the
IPI's own entry, so its sequence is checked and aborted before it returns.

Threads that are not running need no IPI. One that was preempted inside a sequence was
preempted by an interrupt, so the abort is already pending and is applied when it next
runs. One that was scheduled out anywhere else is not in a sequence, and anything it starts
later begins after the caller's stores: the call does a full memory barrier on entry and
exit, paired with the scheduler's when it switches tasks (`membarrier.c:171`, `373`;
`core.c:5505-5513`). A thread that is switched in on a CPU after the scan read that CPU is
one of those two cases.

- **Registration.** `MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ` once per address
  space; without it the command returns EPERM (`membarrier.c:344-348`; observed). A forked
  child inherits the registration (observed); `execve` clears it (`membarrier.c:249-257`).
- **Failure.** EPERM unregistered; EINVAL on a kernel before 5.10 or without rseq, always
  the same answer for the life of the boot; ENOMEM if the CPU mask cannot be allocated.
  A seccomp filter may deny the call.
- **Shortcut.** With one thread in the address space or one online CPU it returns at once,
  correctly.
- **Cost.** 3.5 to 26 microseconds per call on the host with 8 to 24 CPUs busy in the
  process, depending on what else is running (below). Each call interrupts every CPU running
  a thread of the process, and whole-process calls are serialized system-wide by one mutex
  (`membarrier.c:172-175`), so drains should stay rare.

### 4. glibc

**The area.** It sits in an extra block at the end of static TLS
(`elf/dl-tls.c:337-345`), sized from the kernel's `AT_RSEQ_FEATURE_SIZE`
(`sysdeps/unix/sysv/linux/dl-parse_auxv.h:63-67`). `__rseq_offset` is its offset from the
thread pointer and is always set; `__rseq_size` is the feature size, or 0 if the main
thread's registration failed or was disabled (`sysdeps/nptl/dl-tls_init_tp.c:105-114`). On
the host both read 33 and the registration is V2.

**Main thread.** `dl_main` calls `init_tls`, which sets the thread pointer and registers
rseq (`elf/rtld.c:733-784`, `2016`; `dl-tls_init_tp.c:111-112`), before it relocates any
loaded object (`rtld.c:2256-2292`). IFUNC resolvers run during relocation. The loader uses
its private allocator until relocation is complete and only then switches to the
interposed `malloc` (`rtld.c:2340`). Constructors run later still. So no interposed call
can come from an unregistered main thread, and the symbols are valid whenever one arrives.

**New threads.** `pthread_create` blocks all signals before `clone`
(`nptl/pthread_create.c:805`). The new thread registers near the top of `start_thread`
(`pthread_create.c:382-387`) and restores the signal mask after that (`438`). Nothing
before the registration allocates. If the creating thread is registered and the new
thread's registration fails, glibc aborts the process, so a process is registered in all
of its glibc threads or in none.

**Thread exit.** glibc never unregisters. C++ and Rust thread-local destructors, pthread
key destructors, glibc's own per-thread frees and the release of the thread's control block
all run registered (`pthread_create.c:459-465`, `598`, comment at `612-619`); the kernel
drops the registration when the thread dies.

**What the fields read.**

| State | `cpu_id` | `mm_cid` | `__rseq_size` |
|---|---|---|---|
| Registered | the CPU | the cid | 33 on this kernel |
| Disabled by `GLIBC_TUNABLES=glibc.pthread.rseq=0`, or registration refused (valgrind, seccomp) | -2 | 0 | 0 |
| New thread, before it registers | 0 on a fresh stack; on a reused stack possibly a dead thread's values (not checked) | same | unchanged |
| After explicit unregistration (glibc never does this; `rseq.c:390-410`) | -1 | 0 | unchanged |

The third row is why the start-up check must not be "does this look like a CPU number" on
an arbitrary thread; `__rseq_size` and the main thread's `cpu_id` are the reliable signals.
The design's claim that an unregistered process reads cid 0 is confirmed for the first two
rows.

**Static programs.** `libc.a` contains the same symbols and registers the main thread in
`__libc_setup_tls` (`csu/libc-tls.c:161`), so `__rseq_offset` and `__rseq_size` can be
relied on once `main` or a constructor runs. But static IFUNC relocations are applied
before TLS is set up (`csu/libc-start.c:277`, then `280`): an IFUNC resolver that allocates
would run the fast path with no thread pointer. Not tested; the host has no static glibc.

**Outside glibc's control.** A thread made with raw `clone` and its own TLS has no area
where `__rseq_offset` points. Under `LD_PRELOAD`, an IFUNC resolver in another library that
calls `malloc` can run before the preload library itself is relocated; that is true of any
preloaded allocator and is not specific to rseq.

### 5. fork, vfork and clone

**fork.** The child's task copies the parent's rseq state, registration included
(`rseq.h:155-161`, `kernel/fork.c:2463`). It is the first user of a new address space, so
its cid is 0 (`core.c:11063-11068`), and `wake_up_new_task` marks its IDs changed
(`core.c:4982`, `sched.h:2389`), so the area is rewritten before the child's first user
instruction. The child inherits `rseq_cs` as the forking thread left it, which points at
code that is mapped identically. Observed: every forked child read cid 0 at its first
instruction and ran sequences correctly.

**vfork and `CLONE_VM`.** The kernel clears the child's registration (`rseq.h:157-158`).
The child runs on the parent's thread pointer, so it reads the parent's area: the parent's
`cpu_id` and cid, which are not the child's, and nothing aborts its sequences. Observed.
The design's non-goal is stated correctly.

**Can glibc allocate in such a child?** `posix_spawn` clones with `CLONE_VM | CLONE_VFORK`
onto a stack the parent mapped, with all signals blocked and handlers reset, and the child
only makes system calls and then `execve`
(`sysdeps/unix/sysv/linux/spawni.c:104-141`, `384-427`, `493-496`). `system` is
`posix_spawn` (`sysdeps/posix/system.c:147`). `execvp`'s path search uses the stack
(`posix/execvpe.c:96-121`). Running Python's `subprocess`, `os.posix_spawn` and `os.system`,
and shell pipelines, under a preload library that checks every call found no allocation
from an unregistered thread.

### 6. Kernels below 7.2

| What the design needs | First release | Checked how |
|---|---|---|
| rseq aborts on preemption, migration, signal | 4.18 | not re-read; stated by the kernel's history |
| `membarrier` rseq command | 5.10 | absent in v5.9, present in v5.10 |
| `mm_cid` in the rseq area, `AT_RSEQ_FEATURE_SIZE` | 6.3 | absent in v6.2, present in v6.3 |
| A cid exclusive among running threads, below the CPU count | 6.3 | v6.3 allocates at schedule-in and frees at schedule-out. 6.4 to 6.18 use a lazier scheme that was not audited |
| A cid that stays with its thread while threads do not exceed CPUs | 6.19 | ownership modes absent in v6.18, present in v6.19 |
| The cid code audited here | 7.0 | v7.0 and v7.1 differ from v7.2 by one kernel-internal fix; v6.19 differs by about 100 lines and was not audited |
| A cid in range after `execve` returns | up to 6.18 only | broken from v6.19 to 7.3-rc7 |
| `rseq_cs` checked only after a user-mode interrupt | 6.19 | present in v6.19 `kernel/rseq.c` |
| V2 mode: IDs written only on change, read-only fields enforced | 7.1 | v7.0 has neither `rseq_v2` nor the check outside debug mode |

**What a start-up check can observe.** `__rseq_size` (is there a registration, does it
include `mm_cid`), the calling thread's `cpu_id`, `getauxval(AT_RSEQ_FEATURE_SIZE)`, the
`membarrier` query mask and the result of registering, the kernel release string, and
`/sys/devices/system/cpu/possible`. It cannot observe the ownership mode, whether the
registration is V2, or any of the behavioural guarantees; those can only be tied to a
version. None of them needs testing at start-up once the cid is bound-checked: the fast
path is safe on every kernel from 6.3 as far as this audit goes, and the thread-cache
locality that arrives in 6.19 is a performance property.

## Probes and results

All runs on the host, pinned with `taskset` to CPUs 4-7 and 12-31 (24 CPUs) unless noted.
Build lines are in `bench/probes/README.md`.

### `cid_stress.rs`: exclusivity and aborts

Each thread loops over a critical section that takes the slot for its cid, writes its
thread id there, loads the slot's counter, idles for 0 to 63 loop iterations, checks the
thread id is still its own, and commits the counter plus one. A foreign thread id, a cid of
8192 or more, or a final counter sum different from the number of commits is a violation.

| Run | Time | Commits | Aborts | Mode flips | Lost updates | Foreign owner | Bad cid |
|---|---|---|---|---|---|---|---|
| `flip phase_ms=5` (8 to 72 threads) | 420 s | 4.36e11 | 936,501 | 35,366 | 0 | 0 | 0 |
| `flip affinity signals membarrier fork pagefault` | 600 s | 1.58e11 | 6.48e8 | 8,173 | 0 | 0 | 0 |
| `affinity signals pagefault` (72 threads) | 300 s | 3.64e11 | 6.47e7 | none | 0 | 0 | 0 |
| CPUs 12-15 only, `flip affinity signals membarrier fork low=2 high=40 phase_ms=10` | 300 s | 3.67e10 | 2.43e8 | 16,457 | 0 | 0 | 0 |

A flip is one resize of the thread pool across both thresholds, so one change of ownership
mode. The probe shows the mode really changes: in the small phase 0.0000% of commits found
another thread's id in the slot, in the large phase 0.0005% did (one hand-over per time
slice). Cids never exceeded 29 with 32 possible CPUs, and never exceeded 3 in the 4-CPU run.
The second and fourth runs also delivered 1.9e8 and 1.0e8 signals whose handlers ran the
section, issued 2.5e8 and 2.1e8 `membarrier` calls, and forked 64,594 and 59,388 children,
every one of which read cid 0 at its first instruction and counted correctly.

Controls, 4 s each. `noarm` (the section never arms `rseq_cs`) with 72 threads: 14,200
foreign-owner detections and 1.6e9 lost updates. `noarm` with 16 threads: none, because in
per-task mode each thread has a cid to itself even without rseq.

### `rseq_drain.rs`: membarrier

The drain protocol from the design, against workers that commit unless they see the marker.

| Run | Drains owned | Drains lost to an in-flight commit | Commits after ownership | Lost updates |
|---|---|---|---|---|
| 8 CPUs, 24 threads, 300 s | 2,749,288 | 1,793,048 | 0 | 0 |
| 24 CPUs, 96 threads, 300 s | 3,081,647 | 1,965,169 | 0 | 0 |
| control `nobarrier`, 8 CPUs, 24 threads, 20 s | 352,441 | 3,175 | 137,660 | n/a |

"Lost to an in-flight commit" is the case the protocol allows for: a sequence that had
loaded the word before the marker went in committed before or during the call. With the
barrier, nothing committed after it returned in 5.8 million drains over 2.8e11 commits.
A call took 18 and 26 microseconds in these two runs, which shared the machine with the
stress runs above, and 3.5 to 5.7 when run alone.

### `cid_exec_fail.rs`

The table in the first section.

### `rseq_lifecycle.rs`

```
__rseq_size=33 __rseq_offset=-320 __rseq_flags=0 AT_RSEQ_FEATURE_SIZE=33
ctor        registered=true  cpu_id=30  mm_cid=0
main        registered=true  cpu_id=30  mm_cid=0
thread      registered=true  cpu_id=30  mm_cid=1
tls-dtor    registered=true  cpu_id=30  mm_cid=1
key-dtor    registered=true  cpu_id=30  mm_cid=1
fork-child  registered=true  cpu_id=30  mm_cid=0
membarrier: register in parent ret=0; rseq command in forked child errno=0
vm-child    registered=false cpu_id=30  mm_cid=0
main again  registered=true  cpu_id=30  mm_cid=0
atexit      registered=true  cpu_id=30  mm_cid=0
```

With `GLIBC_TUNABLES=glibc.pthread.rseq=0` every line reads `registered=false cpu_id=-2
mm_cid=0`, and `__rseq_size=0`.

### `rseq_preload.rs`

Preloaded into `ls`, `sh -c 'ls / | wc -l'`, `perl -e 'system(...)'`, `cargo --version`, a
Python script (16 threads, `subprocess.run`, `os.system`, `os.posix_spawn`, `os.fork`),
`cid_stress` with thread churn, forks and signals, and `rseq_lifecycle`: about 55,000
allocator calls, none from a thread without a registration.

## Not verified

- CPU hotplug, uprobes inside a sequence, and a kernel booted with `rseq_debug=1`: all need
  root. The first two are argued from source above.
- Static linking: no static glibc on the host.
- valgrind and seccomp: the "registration refused" row is from glibc's source and the
  tunable, not from running under either.
- Kernels other than 7.2.9: read, not run. 6.4 to 6.19 cid code was not audited.
- The mode switch triggered by widening affinity (`core.c:10916-10931`). From the
  arithmetic at `core.c:10849-10856` the user count can never be below the threshold it is
  compared with there, so the path looks unreachable; the fork- and exit-driven switches,
  which use the same fixup functions, were exercised tens of thousands of times.
- Stress testing shows absence of observed failures, not absence of failures. The source
  argument in section 1 is the stronger evidence; it is one reader's analysis of lock-free
  scheduler code and would benefit from a second.

## Corrections to earlier notes

`docs/research/prior-art.md` section 6 and `docs/research/verification-and-testing.md` date
the two rseq modes to v6.19. v6.19 and v7.0 apply the user-interrupt optimization to every
registration and enforce read-only fields only in debug mode; the split into a 32-byte
legacy mode and V2, and the enforcement, arrive in v7.1.
