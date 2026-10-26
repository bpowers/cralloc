# The background thread's stack

2026-10-26. glibc 2.43, musl at 1b76ff07 (static), Linux 7.2, x86-64. Each statement says
whether it was *measured* here or *read in source*.

## What the thread uses now

The C library's default stack, with a null attribute, as the program's own threads get.
Earlier versions asked for a size of their own; both ways of doing that failed (below).

Measured with `crates/asmalloc/tests/thread_stack.rs` (debug build, a program with 252,000
bytes of static thread-local data, the thread run through a burst freed, a fragmented heap,
growth over the cap, parking and a wake, in the process and again in a fork child):

| | Process | Fork child |
|---|---|---|
| Stack (glibc default, `ulimit -s 8192`) | 8 MiB | 8 MiB |
| Written below its top (TLS, glibc's block, frames) | 287,176 B | 287,176 B |
| Resident | 290,816 B (71 pages) | 290,816 B |
| `VmFlags` | `rd wr mr mw me gu ac sd nh` | the same |

- Only the touched pages are resident: 284 KiB of an 8 MiB stack, the thread-local data and
  about 35 KiB under it. Residency is read from `/proc/self/pagemap`, which faults nothing in
  (a read through `/proc/self/mem` would map the zero page into every page it reads).
- `nh`: the kernel keeps transparent huge pages off the thread's stack, so it cannot become
  2 MiB resident whatever the system's THP mode (here `madvise`). `gu`: glibc 2.43's guard
  page is a guard region inside the mapping (`MADV_GUARD_INSTALL`), not a separate one.
- The thread's own frames reach about 8 KiB in a release build and 34 KiB in a debug one
  (the earlier measurement, by the same method, with 104 bytes of TLS).

## The default under other stack limits

Measured with a C program that prints the default and the room below a new thread's frame
(`pthread_getattr_default_np`, `pthread_getattr_np`):

| C library, `ulimit -s` | TLS 104 B: default, room | TLS 252,000 B: default, room |
|---|---|---|
| glibc, 16 | 16 KiB, 11,631 B | 266,240 B, 9,583 B |
| glibc, 64 | 64 KiB, 60,783 B | 266,240 B, 9,583 B |
| glibc, 8192 | 8 MiB, 8,383,855 B | 8 MiB, 8,131,951 B |
| glibc, unlimited | 2 MiB, 2,092,399 B | 2 MiB, 1,840,495 B |
| musl, 8192 | 128 KiB, 134,639 B | 128 KiB, 132,607 B |

glibc's rule (read in the 2.43 source by the reviewer, and consistent with the table): the
soft `RLIMIT_STACK` fixed at start; 2 MiB if unlimited; 16 KiB if below that; then raised to
at least a page plus the static thread-local block plus 2048 bytes, rounded up to a page, with
the guard page on top. So the frames get the limit less the static block, never less than
about 6 to 10 KiB. The reviewer measured 9,776 bytes for 250,000 bytes of TLS under `ulimit -s
256` and `-s 128`. musl's default is 128 KiB of stack plus the thread-local data and its own
block (read in source: `size = guard + ROUND(stacksize + libc.tls_size + __pthread_tsd_size)`).

Against the thread's need, 8 KiB in release and 34 KiB in debug: every default above holds a
release build's rounds; a debug build's need more than glibc's floor.

## Limits

- A process started with a stack limit below its static thread-local data plus about 40 KiB,
  or a program that lowers the default with `pthread_setattr_default_np`, gives the thread
  the same few KiB its own default threads get.
- A program that locks all its memory (`mlockall(MCL_FUTURE)`) gets the whole default stack
  resident, 8 MiB under the usual limit, as it does for each of its own threads.

## Why not a size of our own

- **A fixed 256 KiB** (measured): glibc refuses a requested size only when the TLS, its own
  block and 2 KiB do not fit, so a program whose TLS lands just under that gets a thread with
  almost no stack. A C program with `__thread char[N]` and a 256 KiB thread: N = 254,000 left
  3,495 bytes and ran; 255,000 and 255,500 were accepted and crashed; 256,000 and above got
  `EINVAL`. A Rust program with asmalloc and a 252,000-byte `thread_local!` array got a
  thread with about 5 KiB of stack, which crashed on the first burst of frees.
- **The need plus the modules' TLS**, summed with `dl_iterate_phdr`: that call holds glibc's
  `dl_load_write_lock` while it runs callbacks, and glibc's `fork` does not reset that lock in
  the child (`posix/fork.c`, read by the reviewer), so a child forked while any thread of the
  parent was inside `dl_iterate_phdr` (a backtrace being symbolized, a profiler) waited for
  ever in the allocator's start, though the program's own `pthread_create` worked there
  (`tests/fork_loader_lock.rs`). It also put a lock that runs user callbacks under the
  allocator. The library's default needs neither.
