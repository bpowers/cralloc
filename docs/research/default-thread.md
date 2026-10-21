# The default background thread: wake-ups, servers and the slow path's cost

Measured 2026-10-21 on branch `default-thread` at `22f4b79` (main at `cab8409` merged), on
the reference host (Ryzen 9950X, Linux 7.2.9). The design is "Heartbeat", from "So there is a
background thread".

## Summary

- **A silent process is not woken by its allocator.** Under the preload library a program
  that creates a thread, frees a 160 MiB burst and then does nothing gets the default thread,
  which returns the burst down to the floor in about five seconds and then parks: no futex
  call, `madvise` or `membarrier` at all for the remaining fourteen seconds. The process ends
  at 33 MiB, the floor.
- **A running thread wakes twice a second while the program works, and not at all once it
  has gone quiet.** Started explicitly on the axum order service: 1.95 voluntary context
  switches a second under steady load (its sleep is `MAINTAIN_EVERY`, 500 ms, under the
  shortest interval), and none in the last five seconds after the clients left.
- **The order service gets no default thread**: its free memory never reaches the floor (its
  204 MiB resident is live data), so default behaves as nothing does, burst starts included.
- **The slow path's extra load costs nothing measurable**: `burst` at 1.1 magazines of the
  8-byte class, 4.027 cycles an operation at `ec47910` against 4.011 here, medians of five
  interleaved rounds, ranges overlapping.

## Silent process

`crates/asmalloc-preload/tests/programs/background.c silent 20` under the preload library,
traced with `strace -f -e futex,madvise,membarrier`; futex calls by threads other than the
first (the program's own thread has been joined, and the main thread sleeps in `nanosleep`):

| seconds after start | 0 | 1 | 2 | 3 | 4 | 5 | 6 to 19 |
|---|---|---|---|---|---|---|---|
| allocator thread's futex calls | 3 | 2 | 2 | 2 | 2 | 1 | 0 |
| `madvise` and `membarrier` | 21 | 0 | 359 | 47 | 53 | 35 | 0 |

The thread starts dormant at the program's first `pthread_create`, the free that takes the
retained count over the floor wakes it, its rounds drain the idle slab and purge to the floor,
and the round after that parks it.

## Servers: nothing, default, thread, task

The axum order service with asmalloc linked in, at 90k and 215k requests a second, six bursts
after 5 s silences, then idle; two interleaved rounds. `none` sets `ASMALLOC_BACKGROUND=0`,
`default` leaves it unset, `thread` calls `start_background`, `task` runs the tokio task.

| | burst start, extra request-ms | first 0.5 ms p50 us | kcycles/request at 90k / 215k | resident 10 s after the clients leave, MiB |
|---|---|---|---|---|
| main `0483439` | 2.9 | 30 | 68.3 / 63.5 | 198.8 |
| none | 4.3 | 33 | 68.8 / 64.0 | 204.1 |
| default | 3.2 | 36 | 68.6 / 64.2 | 204.1 |
| thread | 6.5 | 132 | 69.0 / 64.0 | 204.3 |
| task | 7.0 | 125 | 69.6 / 64.3 | 204.3 |
| jemalloc | 3.6 | 32 | 69.1 / 64.2 | 200.1 |

Default and none are the same program here: no allocator thread was ever seen in the default
runs. The thread and the task cost the next burst about 3 request-ms for nothing they can
return, since the heap is under the floor; their drains empty the caches the burst refills
(`burst-start.md`, "The floor"). The thread's wake-ups, from its voluntary context switches
sampled every 250 ms: 1.95 a second at 90k, 0 in the idle tail.

## Slow path

`workload burst size=8 burst=1124 reps=20000` (1.1 magazines of the 8-byte class, two slow
paths a repetition), five interleaved rounds:

| | median cycles/op | range |
|---|---|---|
| `ec47910` | 4.027 | 4.012 to 4.037 |
| `22f4b79` | 4.011 | 4.007 to 4.068 |

The program has one thread, so it never wants the thread; what it pays is the load of the
word on each slow path and, on each launder, the shift and compare that looks for a crossing.
