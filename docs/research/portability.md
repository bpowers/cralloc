# What changes under musl, static linking, thread stacks and Miri

Static glibc needs no code of ours. Static musl needs an rseq area of our own, which costs nothing
measurable on the fast path and brings limits at the edges of a thread's life. A musl shared library
is not supported. The background thread takes the C library's default stack, because both ways of
asking for a size failed. Miri checks the allocator's tests on a simulated heap, one seed a test,
and has found no undefined behaviour, but its run is not green. `README.md`, "Limits worth knowing",
is the short list a user needs, including limits outside this note: address-space limits, strict
overcommit, `mlockall`.

| Configuration | State | What differs |
|---|---|---|
| glibc, dynamic | developed and measured here | one more load than the `executable` feature, 0.2 to 1.0 cycles a pair |
| glibc, static | works, tested | no code of ours; a static C link with lld needs `-lgcc` before our library |
| musl, static | works, tested; `front::` tests in the gate | our own area, registered lazily |
| musl, dynamic executable | expected to work, not tested | |
| musl shared or preload library | not supported | the area's offset is a link-time constant only in an executable |
| No registration (valgrind, seccomp, `glibc.pthread.rseq=0`) | correct, slow | every call takes the slow path; not measured, the design estimates tens of cycles |
| Miri | the allocator's handle does not exist | a program declares its allocator `#[cfg(not(miri))]` |

Unless noted: Linux 7.2.9, glibc 2.43, musl 1.2.5 as Rust 1.99.1's target ships it, 2026-10-17 to
18, fat LTO, pinned with the sibling idle, four interleaved rounds. Cells are ranges over rounds.

## What a musl build costs: nothing

Cycles per allocate and free, one thread, as of 2026-10-18. The default form then read a GOT slot
and now reads a word of its own, the same two loads, and later work cut churn to about 24
([roofline.md](roofline.md)):

| | Constant pair | `Box` pair | Run-time size | Batch of 32 | Churn |
|---|---:|---:|---:|---:|---:|
| cralloc, glibc, default form | 5.43 | 6.93 | 7.74 | 6.41 to 6.77 | 29.3 to 30.1 |
| cralloc, glibc, `executable` | 4.96 | 5.90 to 5.93 | 7.50 | 5.34 | 29.7 to 30.0 |
| cralloc, glibc static | 4.95 to 4.96 | 5.90 | 7.43 to 7.48 | 5.34 to 5.50 | 29.9 to 30.5 |
| cralloc, musl static | 4.95 to 4.96 | 5.93 to 5.95 | 7.38 to 7.50 | 5.65 to 5.67 | 29.5 to 30.0 |
| musl's malloc | 200 | 196 | 201 | 77 | 227 |

Across threads, on five shapes of job with both sides working, the last three cralloc rows take 3.0
to 5.8 ns an object, with overlapping ranges on every job; musl's malloc takes 83 to 414 ns. Thin
LTO inlines the fast path on musl as on glibc, 40 instructions a constant pair, though the crate has
module-level assembly there. An immediate in place of the load of the area's offset was slower than
the load (5.30 against 4.96 on a constant pair), so musl reads a word too. The musl batch loop's
extra 0.3 cycles are not the area and are not explained.

## musl has no rseq area, so the crate brings one

The crate defines a `struct rseq` in initialized thread-local data, generated from Lean, reading
`cpu_id` -1, and a read-only word holding its `@tpoff` offset, which the routines read as they read
glibc's `__rseq_offset`. A thread registers 32 bytes at its first slow path, the kernel's legacy
mode. From the source, sequences abort there as they do under V2; no probe forces an abort in legacy
mode ([linux-rseq-contract.md](linux-rseq-contract.md), "Aborts"). `cpu_id` is all the state a
thread keeps: -1 never tried, a CPU number registered, -2 refused or retired. The kernel answers a
second registration with `EBUSY`, another area with `EINVAL`, another signature with `EPERM`, and a
forked child keeps the forking thread's registration (measured).

**A thread must unregister before musl frees its thread-local data.** musl unmaps a detached
thread's stack and data, then exits, and the kernel writes a registered area on the way back to user
space from that `munmap`. Without unregistering, detached threads beside four threads cycling 1 MiB
buffers died with SIGSEGV within 1000 threads, every time. With a pthread key whose destructor
unregisters, 150,000 ran clean, and `detached_threads_come_and_go_registered` (3000 threads) fails
in every run with the destructor disabled. Joined threads are safe either way.

**Limits of registering lazily,** all documented, none fixed.

- **A handler that registers a thread inside a routine.** On a thread at -1 the kernel aborts
  nothing, so a signal between a routine's arming store and its read of `cpu_id` is delivered with
  the arm in place. If the handler makes the thread's first allocator call, the routine then runs to
  its commit unprotected, and a second event within about eight instructions does harm. No fix fits
  lazy registration. glibc registers before any call, so this cannot arise there. An ignored test,
  `limitation_a_handler_that_registers_a_thread_inside_its_first_sequence_unprotects_it`, shows it.
- **In `docs/design.md`, "Threads on musl":** a first allocator call after our key's destructor dies
  registered, and a detached thread can then crash the process; destructors after ours, Rust's in
  practice, run on the slow path; the process's first registration takes musl's key lock, so a
  handler making that call inside musl's key functions deadlocks.
- **Seccomp.** A filter that fails `arch_prctl(ARCH_GET_FS)` makes every slow path repeat the call.
  One that kills on `arch_prctl` or `rseq` kills at the thread's first slow path. One that fails
  `rseq` is handled, by retiring the thread.
- **The background thread.** musl keeps no record of threads created, so a process whose first
  thread frees the memory gets no thread (`docs/design.md`, "Heartbeat").

## The area's page offset

A load of `cpu_id` at the page offset of a slab word stored just before waits for that store as if
it were the same location ([roofline.md](roofline.md)). Where musl put the area by default, page
offset 0x20, which held the 32-byte class's word, a pair cost 7.04 cycles against 4.96 with the area
at 0x300, two cycles more on every other loop too. musl aligns the thread pointer to the
thread-local segment and places each symbol at a link-time offset from it, so aligning the area's
section to a page fixes its page offset in every thread: 0x300, with compile-time assertions that it
clears the hot words and the words the C routines read from 1024. Padding the segment without the
alignment moved nothing.

glibc places its own area at any multiple of 64, and a worker's follows the static thread-local
size. On a hot word it cost as on musl: 8.98 cycles at 0x40, the 64-byte class's word, against 7.22
elsewhere.

**Ruled out by construction.** The kernel requires a registered area to be 32-byte aligned, so
`cpu_id` and `rseq_cs` always sit at bytes 4 to 15 of a 32-byte chunk. Each class therefore has a
32-byte chunk with its hot word outside those bytes, today at 24 to 31, where no 16-aligned word can
overlap it either. Minimum cycles over three interleaved rounds, against main at `f41dbf9`; the
adopted word then sat at bytes 16 to 23:

| | Before | Two hot words a chunk | One a chunk (adopted) |
|---|---:|---:|---:|
| Worker, area at 0x580, 64-byte run-time pair | 7.21 | 9.16 | 7.64 |
| Worker, area at 0x40, the same pair | 8.98 | 9.14 | 7.65 |
| glibc, constant pair | 4.95 | 5.01 | 5.01 |

Two to a chunk needs arithmetic to find a word, 2.3 cycles a run-time pair. One to a chunk needs a
shift where a scaled `lea` did, 0.35 to 0.45 cycles on run-time pairs and nothing measurable
elsewhere.

## Static linking

- **Static glibc.** `libc.a` defines `__rseq_offset` and registers every thread, so the allocator
  needs no code of its own. The allocator's unit tests, the test kit's system and fork tests, and a
  static C program over `export_c_abi!` pass.
- **Static musl C programs.** `export_c_abi!` in a static library linked with `musl-gcc -static`
  replaces musl's `malloc`; musl keeps its internal allocator for its own early allocations. The
  preload suite's `threads`, `forks` and `signals` programs print what they print under musl's
  malloc. `signals`, which allocates in a handler, deadlocks under musl's malloc and passes under
  cralloc.
- **A static C link with lld needs `-lgcc` before our library.** lld takes static glibc's soft-float
  helpers from the first archive that defines them: ours, whose `compiler_builtins` names
  `rust_eh_personality`, undefined in `no_std`. GNU ld takes them from `libgcc.a`.
- **An object that holds the allocator must not be unloaded,** since a thread's `rseq_cs` goes on
  naming a sequence inside it. A shared object links `-z nodelete`, as the preload library does.
- **A trap for tests.** A child forked from a threaded harness may inherit a lock of std's, and each
  thread it starts then blocks for ever in std's thread start. It hung the test kit's fork test in 4
  of about 127 runs on a loaded machine, and a musl test once. The kit's child now starts its
  threads with `pthread_create`, after which none of 300 runs hung, and other such tests run in a
  fresh process (`crate::fresh`).

## The background thread's stack

The thread uses the C library's default stack, as the program's own threads do. Measured 2026-10-26
(glibc 2.43, musl at 1b76ff07) by `crates/cralloc/tests/thread_stack.rs`: a debug build with 252,000
bytes of static thread-local data, the thread run through its paths in the process and in a fork
child, with the same result in both.

- Of 8 MiB, 287,176 bytes are written and 290,816 resident (71 pages). `VmFlags` shows `nh`, so the
  stack cannot become 2 MiB resident whatever the huge-page mode.
- The thread's own frames reach about 8 KiB in a release build and 34 KiB in a debug one.

| C library, `ulimit -s` | TLS 104 B: default, room | TLS 252,000 B: default, room |
|---|---|---|
| glibc, 16 | 16 KiB, 11,631 B | 266,240 B, 9,583 B |
| glibc, 8192 | 8 MiB, 8,383,855 B | 8 MiB, 8,131,951 B |
| musl, 8192 | 128 KiB, 134,639 B | 128 KiB, 132,607 B |

glibc's rule, read in the 2.43 source: the soft `RLIMIT_STACK` fixed at start, 2 MiB if unlimited,
16 KiB if below that, then raised to at least a page plus the static thread-local block plus 2048
bytes. So frames get never less than about 6 to 10 KiB. Every default holds a release build's
rounds; a debug build's need more than glibc's floor.

**Limits.** A stack limit below the static thread-local data plus about 40 KiB, or a default lowered
with `pthread_setattr_default_np`, gives the thread the same few KiB the program's own threads get.
`mlockall(MCL_FUTURE)` makes the whole stack resident, as for its own threads.

**Why not a size of our own.**

- **A fixed 256 KiB.** glibc refuses a size only when the thread-local data, its block and 2 KiB do
  not fit, so a program just under that gets almost no stack. With `__thread char[N]`: N = 254,000
  left 3,495 bytes; 255,000 was accepted and crashed; 256,000 got `EINVAL`. A Rust program with a
  252,000-byte `thread_local!` got about 5 KiB and crashed on its first burst of frees.
- **The need plus the modules' thread-local data, summed with `dl_iterate_phdr`.** That call holds
  glibc's `dl_load_write_lock` during its callbacks, and `fork` does not reset it in the child. A
  child forked while a parent thread was inside it, as during a backtrace, waited for ever in the
  allocator's start (`crates/cralloc/tests/fork_loader_lock.rs`).

## Miri

Miri runs neither assembly nor system calls. Our tests run on a simulated heap with the twins, under
`-Zmiri-strict-provenance`, through `scripts/miri`: one process and one seed a test, with a CPU
limit. A small allocator over `mmap` showed what Miri rejects: inline assembly, `MADV_DONTNEED`,
partial `munmap`, `mprotect`, `PROT_NONE`, `MAP_NORESERVE` and integer-to-pointer casts.

**The cost was the aliasing model, not the tests' loops.** Tests that run in 2 to 20 ms natively
took over 15 CPU minutes, and a minute or two with Stacked Borrows off. A retag of a shared
reference visits every interior-mutable range of the pointee (`sb_reborrow`,
`visit_freeze_sensitive` in a profile of Miri), so a reference to a struct of thousands of atomics
costs thousands of visits each time it is passed. Tree Borrows halves it, no more. CPU seconds for
2,000 calls taking `&T`, Stacked Borrows, 2026-10-25 to 28:

| `T` | Before | After |
|---|---:|---:|
| `u64` | 1 | 1 |
| `back::State` | 217 | 13 |
| `front::Slabs<[Slab; 64]>` | 104 | (8 simulated CPUs under Miri) |

"After" holds each per-region table and each class's summary words in one `UnsafeCell`
(`cells::Cells`, the same compiled code but for one loop after a fork), with four cache domains and
eight simulated CPUs under Miri. Comparing simulated pages as byte slices compiles to `memcmp`,
which Miri runs natively. What remains slow is work: 25,000 to 300,000 basic blocks a second, on
tests that run hundreds of rounds of slow paths or fill whole heaps. They want their counts scaled
(`scale::under_miri`), test by test.

Last full run, 600 CPU seconds a test: 351 of 393 passed, 42 over the limit, none failed, in 11.7
CPU hours. A rerun of the slowest 47 after the last two changes: 17 passed, 26 over, 4 failed.

## Open questions

- **Four tests fail under Miri and pass natively, deterministically:**
  `a_fresh_heaps_first_objects_of_a_small_class_lie_on_lines_of_their_own`,
  `a_class_opens_one_run_at_start_whatever_the_clock_reads`,
  `large_allocations_and_frees_run_the_heartbeat` and
  `review_lean_realloc::a_simulated_heap_gives_back_the_blocks_a_resize_leaves`. Each asserts a heap
  policy, none is undefined behaviour, and none had finished under Miri before. Preemption,
  weak-memory emulation and isolation do not matter, and every Miri-only setting forced natively
  passes. Until they are explained and the 26 are scaled, `scripts/miri` exits non-zero.
- **The default form under static glibc has not been re-measured.** The table's run read a GOT slot,
  whose load the static linker relaxes to a `lea`. The form now reads a word of its own, which it
  does not.
- **A dynamically linked musl executable** has not been run.
