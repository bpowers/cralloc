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
- **A large allocation and free pay 1.5 to 2 cycles a pair more than main** (under 1%), and 7
  in a loop that alternates two large classes (2.8%); they paid 8 to 12 before the reviews'
  costs were removed.

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
| the main thread frees what a worker allocated | yes | 18 | 0 | 35 |
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
between calls, so it wakes twice a second. Measured with the `steady` mode of
`tests/default_thread.rs` (a worker frees 128 MiB, then two threads allocate and free 64 KiB
every 100 ms for 20 s): 20 wake-ups in 10 s of that work, and parked 14 s into the silence after
it. The axum order service was not run again (the shared measurement lock was held by server
runs throughout); its numbers below are from before the review.

## Large blocks on the slow path (after the review)

The first review found that a large call that did not beat built a `Cache` to load the
thread's word, 15 calls in 16. `workload largepair ops=400000`, an allocation and a free of a
large block a pair, pinned to CPU 2, six interleaved rounds, user cycles a pair (medians,
ranges), against main (`f9ee162`):

| sizes | main | first fix (`147bdc8`) | branch (`ea67e41`) | branch without the crossing test |
|---|---|---|---|---|
| 64 KiB | 256.3 (255.7 to 258.8) | 261.8 | 257.8 (257.0 to 258.6) | 262.3 (262.0 to 264.3) |
| 1 MiB | 275.0 (274.2 to 275.7) | 280.2 | 277.1 (276.7 to 277.6) | 278.5 (278.3 to 280.5) |
| 64 KiB and 1 MiB in turn | 263.6 (262.9 to 268.3) | 274.5 | 270.9 (270.3 to 276.1) | 269.8 (269.4 to 270.4) |

The first fix loaded the word alone on a large call that did not beat. The callers still
looked at the snapshot it returned for `WANTED`, so `alloc_large_slow_using` and the large
free kept a frame around the heartbeat (the second review's disassembly). Now `WANTED` has a
bit of its own in the one mask the slow path tests, the cold look creates the thread itself on
a Rust call, and the callers look at nothing after the heartbeat. What is left, in instructions
a pair: 23 (64 KiB), 29 (1 MiB) and 26 a size in turn. Of those, the test that decides whether
an add to the retained tally crosses an eighth of the floor is 9 to 16 (the last column is a
build without it), and the rest is the load and test of the word.

The cycles do not follow the instructions. The build without the crossing test runs fewer
instructions and is slower on single sizes, and the same on the alternating loop. So the
second review's guess for the alternating row, that every fourth 1 MiB free crosses a 4 MiB
step and reads the tallies in `over_floor`, is not it: taking the test out changes that row by
a cycle. The 7 cycles that row pays, against 2 on single sizes, move with code placement and
not with what the code does; I did not pin them down further.

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
- A child that shares its parent's memory (`vfork`, `CLONE_VM`) shares its generation; a Rust
  slow path there that finds the word wanted creates the thread in the child. Allocating in
  such a child is a non-goal.
- A program that runs both `start_background` and the tokio task can have the task take every
  round of `maintain`; the thread then never sees a round, and never parks. The task wakes
  once an interval anyway, so the cost is the thread's two wake-ups a second besides.
- The schedule exploration of the park protocol is sequentially consistent: it checks the
  order of the store, the second look and a slow path's install and load, not the fence. That
  the fence gives that order on x86-64 rests on the argument in `depot/thread.rs`.
