# The default background thread: wake-ups, starts, servers and the slow path's cost

First measured 2026-10-21 on branch `default-thread` at `22f4b79`; the sections marked
"after the review" were measured the same day on `default-thread-fixes`, after the adversarial
review of `e84ccc6`, on the reference host (Ryzen 9950X, Linux 7.2.9, glibc 2.43). The design
is "Heartbeat", from "So there is a background thread".

## Summary

- **A silent process is not woken by its allocator, fragmented or not.** The thread parks
  after nine rounds in a row that did no work while no slow path ran anywhere, and then makes
  no system call at all. A burst freed whole goes back to the floor in about five seconds and
  the thread parks at about fourteen; a heap that keeps 200 MiB of free slots on pages with live
  neighbours parks too, where before the review its thread woke twice a second for ever.
- **The thread starts wherever the library can say the program has a thread of its own**: a
  worker's frees, the main thread's frees, a freeing thread that had a signal blocked at the
  crossing, a burst freed before the second thread existed, a program with 1 MiB of static
  thread-local data. Not in a process that never created a thread, nor in a fork child whose
  first thread frees (by design), nor with `ASMALLOC_BACKGROUND=0`, nor once creation failed,
  nor in a process that calls `maintain` itself.
- **A large allocation and free pay 4 to 5 cycles a pair more than main** (1.6 to 1.9%), and 11
  in a loop that alternates two large classes (4%); they paid 8 to 12 before the review's cost
  was removed.

## Silent process (after the review)

`crates/asmalloc-preload/tests/programs/background.c silent 25` under the release preload
library, traced with `strace -f -tt -e trace=futex,madvise,membarrier`; calls by the
allocator's thread, by second since the program started (the program's own thread has been
joined, and the main thread sleeps in `nanosleep`):

| seconds after start | 0 | 1 | 2 | 3 | 4 | 5 | 6 to 13 | 14 | 15 to 24 |
|---|---|---|---|---|---|---|---|---|---|
| futex calls | 3 | 2 | 2 | 2 | 2 | 2 | 2 each | 1 | 0 |
| `madvise` and `membarrier` | 1 | 0 | 347 | 50 | 53 | 35 | 0 | 1 | 0 |

The process ends at 32 MiB anonymous, the floor. Before the review the thread parked at
second 6, on the first round that found nothing; it now waits out the nine quiet rounds
(`depot::thread::QUIET_ROUNDS`: the purge pass that takes runs of any length comes one pass in
eight, after the frontier's two intervals), sleeping half a second between calls, so a silent
process costs about 16 wake-ups more, once, and then none.

## Process cases (after the review)

`crates/asmalloc/tests/default_thread.rs`, one process per case, release build; anonymous
resident memory at the end, the thread's voluntary context switches in five seconds once it
had gone three without one, and the seconds of silence that took (a typical run):

| case | thread | parked after, s | wake-ups once parked | RssAnon, MiB |
|---|---|---|---|---|
| a worker frees 128 MiB | yes | 17 | 0 | 31 |
| the main thread frees what a worker allocated | yes | 18 | 0 | 31 |
| a worker allocates 384 MiB of 64-byte objects and frees every other one | yes | 20 | 0 | 410 |
| fork child of a parked parent, its first thread frees 128 MiB | no | | | |
| the same child, then a thread of its own frees 128 MiB | yes | 18 | 0 | 32 |
| a thread creation refused (`RLIMIT_NPROC`) | no | | | 51 |

The fragmented heap is the review's finding 1: before, the thread made 10 wake-ups in those
five seconds, and every round walked the bitmaps for a purge pass that returned nothing.

Against the branch before the review (`4942996`), the same binary fails `mainfrees` (no
thread, 145 MiB), `frag` (did not park, 10 wake-ups), `sigmask` (no thread, 90 MiB), `late`
(no thread, 88 MiB) and `tls` (no thread, 148 MiB). Those last two passed before only when the
program's later frees crossed another 4 MiB step, which the cases now avoid.

On a static musl build the cases pass except `mainfrees`, which gets no thread: musl keeps no
record of threads created, and the evidence there is a free by a thread other than the first.

**What glibc's record says in a fork child.** `__libc_single_threaded` reads 1 at start and 0
from the first `pthread_create` on, and stays 0: in a fork child of a threaded parent, in that
child after its own `pthread_create`, in the parent after joining its only thread, and in a
child of that parent (a C probe, glibc 2.43, dynamic). So a fork child, which the allocator
knows from drawing a generation where one had been drawn before, takes a free by a thread other
than its first as the evidence.

## Steady work (after the review)

The thread does not park while slow paths run: each round compares the slabs' activity
counters, and any change starts its count again. Awake, it sleeps `MAINTAIN_EVERY` (500 ms)
between calls, so it wakes twice a second. The axum order service was not run again; its
measurement below is from before the review, and the rule that keeps a working program's thread
awake did not change.

## Large blocks on the slow path (after the review)

The review found that a large call that did not beat built a `Cache` to load the thread's
word, 15 calls in 16. `workload largepair ops=400000`, an allocation and a free of a large
block a pair, pinned to CPU 2, main (`f9ee162`) and the branch interleaved, six rounds,
user cycles a pair (medians, ranges):

| sizes | main | branch as reviewed, with the review's cost removed only in part | branch |
|---|---|---|---|
| 64 KiB | 256.7 (255.9 to 257.4) | 264.2 (263.9 to 265.3) | 261.8 (261.1 to 269.1) |
| 1 MiB | 274.2 (273.8 to 278.8) | 284.2 (283.0 to 290.8) | 280.2 (279.4 to 282.1) |
| 64 KiB and 1 MiB in turn | 264.2 (263.0 to 267.9) | 276.3 (276.0 to 277.6) | 274.5 (273.6 to 280.2) |

The middle column is this branch before the last change: a large call that does not beat
loaded the word and then the heap's start, and the start hook was loaded before the snapshot
was tested. The change loads the word alone, tests the parked and level bits in one mask, and
keeps everything else in a cold function; the step that decides whether an add to the retained
count looks at the whole count is a constant. What is left is about 32 instructions a pair:
the add to the retained tally now returns its old value and tests a step (removing the test
saves 12 a pair in a build made for the purpose), the load and test of the word, and the test
of the snapshot for `WANTED` on the Rust side.

## Before the review: servers and the 8-byte burst

Measured at `22f4b79`. The axum order service with asmalloc linked in, at 90k and 215k
requests a second, six bursts after 5 s silences, then idle; two interleaved rounds. `none`
sets `ASMALLOC_BACKGROUND=0`, `default` leaves it unset, `thread` calls `start_background`,
`task` runs the tokio task.

| | burst start, extra request-ms | first 0.5 ms p50 us | kcycles/request at 90k / 215k | resident 10 s after the clients leave, MiB |
|---|---|---|---|---|
| main `0483439` | 2.9 | 30 | 68.3 / 63.5 | 198.8 |
| none | 4.3 | 33 | 68.8 / 64.0 | 204.1 |
| default | 3.2 | 36 | 68.6 / 64.2 | 204.1 |
| thread | 6.5 | 132 | 69.0 / 64.0 | 204.3 |
| task | 7.0 | 125 | 69.6 / 64.3 | 204.3 |
| jemalloc | 3.6 | 32 | 69.1 / 64.2 | 200.1 |

Default and none are the same program here: the service's free memory never reaches the
floor (its 204 MiB resident is live data), so no thread starts. The thread's wake-ups, from
its voluntary context switches sampled every 250 ms: 1.95 a second at 90k, 0 in the idle tail.

`workload burst size=8 burst=1124 reps=20000` (1.1 magazines of the 8-byte class, two slow
paths a repetition), five interleaved rounds: 4.027 cycles an operation at `ec47910` (4.012 to
4.037) against 4.011 (4.007 to 4.068) at `22f4b79`.

## Limits

- A Rust program that keeps a signal blocked in every thread never passes the test for a
  handler, and gets no thread by default; it calls `Asmalloc::start_background`.
- A handler installed with `SA_NODEFER` and no alternate stack has neither mark of a handler,
  and passes that test.
- A thread with no rseq registration has no activity counter, so the thread does not see its
  work; such a thread takes a slow path on every call, and wakes a parked thread on its next.
- In a fork child, and on musl, a process whose first thread frees the memory gets no thread.
- A program that frees a burst on its only thread, creates a thread and goes silent within the
  same interval gets no thread until a slow path in a later interval looks at the level again.
- Where the generation's page cannot be wiped on fork (drains are off there too), a fork child
  keeps its parent's generation and reads the parent's word as its own.
