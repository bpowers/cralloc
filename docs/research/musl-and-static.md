# musl and static linking

Measured 2026-10-17 and 18 on the machine of `zen5-roofline.md` section 2 (Linux 7.2.9, glibc
2.43, musl 1.2.5 as Rust 1.99.1's `x86_64-unknown-linux-musl` target ships it and as Fedora's
`musl-libc-static` packages it). Benchmarks are `bench/roofline/rust-alloc`, fat LTO, pinned to
CPU 12 with its sibling 28 idle (checked from `/proc/stat` before each run; other teammates'
jobs kept the load average at 15 to 25 on other CPUs), cross-thread workloads on 12 to 15, four
interleaved rounds of every configuration. Cells give the range over rounds.

## What changed and why

- **Static glibc needs nothing.** `libc.a` defines `__rseq_offset` and registers every thread,
  so both forms link (the GOT form's load is relaxed to a `lea`) and run as fast as the
  dynamic ones. The allocator's unit tests, the test kit's system and fork tests, and a
  statically linked C program over `export_c_abi!` pass.
- **musl registers no rseq area and has no `__rseq_offset`** (no rseq symbol in its `libc.a`,
  none in its source; a registration of our own succeeds in every thread). So the crate brings
  its own: a `struct rseq` in initialized TLS reading `cpu_id` -1, and a word holding its
  `@tpoff` offset, which the routines' direct form reads. A thread registers at its first slow
  path. The kernel answers a second registration with `EBUSY`, another area with `EINVAL`, another
  signature with `EPERM`; a forked child keeps the forking thread's registration;
  unregistering resets `cpu_id` to -1.
- **A thread must unregister before musl frees its TLS.** musl unmaps a detached thread's stack
  and TLS (`__unmapself`: `munmap`, then `exit`), and the kernel writes a registered area on the
  way back to user space from that `munmap`. Without unregistering, a probe that ran detached
  threads beside four threads cycling 1 MiB buffers died with SIGSEGV within 1000 threads, every
  time (three of three runs at 50,000, one of one at 1000); libtest's own test binary died the
  same way. With a pthread key whose destructor unregisters, 150,000 threads ran clean, and so
  does the test `detached_threads_come_and_go_registered` (3000 threads), which fails with
  SIGSEGV in every run when the destructor is disabled. Joined threads are safe either way: the
  joiner unmaps only after the kernel has cleared the thread's tid.
- **A local-exec immediate (no load at all) is slower than the one load**, so there is no third
  form. Same binary otherwise, area at page offset 0x300:

  | | constant pair | `Box` pair | run-time size | batch |
  |---|---:|---:|---:|---:|
  | offset read from the word (adopted) | 4.96 | 5.95 | 7.45 | 5.64 |
  | offset as an immediate (`mov r10, OFFSET area@tpoff`) | 5.30 | 5.96 | 7.85 | 5.76 |

## The area's page offset

A load of `cpu_id` at the page offset of a slab word stored just before waits for that store as
if it were the same location. Where musl put the area by default (page offset 0x20, which
holds the 32-byte class's word) a pair cost 7.04 cycles against 4.96 with the area at 0x300, and
every other loop about 2 cycles more. musl sets the thread pointer to the TLS segment's
alignment and places each TLS symbol at a link-time offset from it, so aligning the area's
section to a page fixes its page offset in every thread: 0x300, with a compile-time assertion
that it is clear of the slab's hot words (0 to 512) and below the words the C routines read
from 1024. Padding the TLS segment without the alignment moved nothing, since musl moves the
thread pointer with the segment. At page offsets 0x300, 0x680, 0xa00 and 0xd00 the loops cost
the same.

### glibc's area: where it can land

glibc places the area itself. Method: a C program that prints the page offset of `fs:0 +
__rseq_offset` in its main thread and in threads of four stack sizes, built dynamically and
statically with 0 to 4000 bytes of extra `__thread` data; and `rust-alloc` with a
`thread_local!` array of chosen size and its loops run on a worker thread. `AT_RSEQ_ALIGN` is 64 here, and every offset seen was a multiple of 64.

- Main thread: 0x680 dynamically linked, 0x340 statically, whatever the program's static TLS
  (0 to 4000 bytes extra).
- Worker threads: the thread pointer sits at page offset 0x6c0 for every stack size tried
  (default, 64 KiB, 1 MiB, 8 MiB), and the area at the thread pointer minus the static TLS
  size, rounded to 64. So it moves with the program's static TLS: 0x600 with none, 0x580 for
  `rust-alloc`, 0x200 with 1000 bytes more, 0x40 and 0x0 near 1400.
- Where it lands on a hot word, it costs the same as on musl. `rust-alloc` on a worker thread,
  run-time-size pair of 64 bytes (class word at 0x40): 7.22 cycles with the area at 0x580 or
  0x80, 8.98 with the area at 0x40 (extra static TLS of 1344 bytes).

So on glibc, a program whose static TLS puts its workers' area at page offset 64k, for k from
1 to 5, has a hot class (index 8k, whose word holds `cpu_id`'s page offset) about 1.75 cycles
slower per pair in every worker thread: 5 of the 64 positions the area can take. Whether the
`rseq_cs` store at 64k + 8 costs class 8k + 1 the same was not measured.

**Ruled out by construction (2026-10-18).** The kernel requires a registered area to be 32-byte
aligned, so its `cpu_id` and `rseq_cs` always sit at bytes 4 to 15 of a 32-byte chunk. The slab
now gives each class a 32-byte chunk with its hot word at bytes 16 to 23 (its spare heads at 0
to 15), so no hot word can share a page offset with either field. Same method, before (main
f41dbf9), two hot words to a chunk at bytes 16 to 31 (tried first), and one to a chunk (adopted),
minimum cycles over three interleaved rounds:

| | before | two per chunk | one per chunk |
|---|---:|---:|---:|
| worker, area at 0x580, 64-byte run-time-size pair | 7.21 | 9.16 | 7.64 |
| worker, area at 0x40, the same pair | 8.98 | 9.14 | 7.65 |
| glibc, constant pair | 4.95 | 5.01 | 5.01 |
| glibc, `Box` pair | 5.94 | 5.91 | 5.89 |
| glibc, run-time-size pair | 7.43 | 9.76 | 7.78 |
| glibc, batch of 32 | 5.44 | 5.40 | 5.40 |
| glibc, churn | 29.8 | 33.1 | 29.5 |
| glibc static, run-time-size pair | 7.43 | 9.75 | 7.77 |
| musl static, run-time-size pair | 7.41 | 9.77 | 7.80 |
| musl static, churn | 29.6 | 33.0 | 29.3 |

Two to a chunk needs arithmetic to find a class's word on the run-time path (2.3 cycles a pair,
3.4 a churn step); one to a chunk needs a shift where a scaled `lea` did (0.35 to 0.45 cycles on
run-time-size pairs, nothing measurable elsewhere). The worker's pair now costs the same
wherever its area lands.

## Numbers

Cycles per allocate and free, single thread:

| | constant pair | `Box` pair | run-time size | batch of 32 | churn |
|---|---:|---:|---:|---:|---:|
| asmalloc, glibc, GOT form | 5.43 | 6.93 | 7.74 | 6.41-6.77 | 29.3-30.1 |
| asmalloc, glibc, direct form | 4.96 | 5.90-5.93 | 7.50 | 5.34 | 29.7-30.0 |
| asmalloc, glibc static | 4.95-4.96 | 5.90 | 7.43-7.48 | 5.34-5.50 | 29.9-30.5 |
| asmalloc, musl static | 4.95-4.96 | 5.93-5.95 | 7.38-7.50 | 5.65-5.67 | 29.5-30.0 |
| musl's malloc | 200 | 196 | 201 | 77 | 227 |
| glibc's malloc | 11.0 | 11.0 | 21.9 | 33-35 | 46.6 |

Nanoseconds per object, both sides working, cross-thread:

| | x_cross | x_cross_touch | x_swap2 | x_swap4 | x_cross_pairs |
|---|---:|---:|---:|---:|---:|
| asmalloc, glibc direct | 3.00-3.03 | 4.69-4.84 | 4.42-4.91 | 5.66-5.74 | 5.50-5.64 |
| asmalloc, glibc static | 2.96-3.02 | 4.66-4.69 | 4.63-4.79 | 5.66-5.72 | 5.44-5.61 |
| asmalloc, musl static | 2.98-3.03 | 4.62-4.69 | 4.37-4.77 | 5.70-5.76 | 5.54-5.61 |
| musl's malloc | 83-86 | 83-85 | 113-116 | 403-414 | 299-302 |

The musl batch loop is 0.3 cycles slower than glibc's. It is not the area: the same at four page
offsets. Not explained; the binaries differ in layout.

Thin LTO (16 codegen units) inlines the fast path on musl as on glibc: 40 instructions per
constant pair on both, though the crate has module-level assembly there (the generated area).

## Static musl C programs

`export_c_abi!` in a static library linked into a C program with `musl-gcc -static` replaces
musl's `malloc` (musl keeps its internal allocator for its own early allocations). The preload
suite's `threads`, `forks` and `signals` programs print what they print under musl's malloc;
`signals`, which allocates in a handler, deadlocks under musl's malloc and passes under asmalloc.
Under gdb, about 16% of `threads`' calls (2 waves, 4 threads) took the C slow path, the same as
the statically linked glibc build, so the C routines' fast path runs there too.

## A trap for tests: forking a threaded test process

A child forked from the test harness while other test threads run may inherit a lock of std's
held by one of them. Each thread the child then starts blocks for ever in std's thread start
(`stack_overflow::thread_info::set_current_info`, a global mutex). Seen once in a musl test
that forked, then spawned threads. The tests that need threads in a child process now run in a
fresh process (`front/area.rs`, `in_fresh_process`); the fork test's child starts no thread.
