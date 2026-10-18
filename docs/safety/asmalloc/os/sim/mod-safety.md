# Safety ledger: `crates/asmalloc/src/os/sim/mod.rs`

The simulated OS's `Os` implementation. Machine checks are as for `memory-safety.md`: the slow
Kani harness on the `Tiny` shape, and Miri over the unit tests on the `Standard` shape
(`scripts/miri`).

## SIM-OS-01: `unsafe impl<S: Shape> Os for SimOs<S>`

The comment above the impl lists how each promise of the trait holds. The load-bearing
invariant is the zero promise:

**Invariant Z.** Every byte of simulated memory that is not in a reserved page is zero.

- Initially, `Memory::new` zero-fills the allocation.
- `reserve` changes no bytes. It moves pages from unreserved to reserved by compare-and-swap
  from state 0 (`Pages::claim`), and rolls back its own claims on a conflict, so two reservations
  never share a page.
- `unmap` zeroes its whole range, whether or not it was committed, before releasing the pages
  with release stores. A later `claim` uses acquire compare-and-swaps that read those stores, so
  the next owner sees the zeroes.
- No other call releases pages.

So a reservation starts out zero, which is the trait's promise that reserved memory reads as
zero once committed. That holds even if a buggy caller wrote to pages it never committed. The
other promises:

- Reservations are aligned: candidate offsets step through aligned addresses.
- Reservations are disjoint from each other: per-page claims. They are disjoint from every
  other allocation: they lie inside `memory`, which this value owns until it is dropped.
- *Valid until unmapped or dropped*: the memory is freed only in `Drop`.
- *A range in one live reservation of this value.* `checked_range` panics before touching
  anything otherwise:
  - The range is outside this value's memory.
  - It contains a page that is not reserved.
  - It spans two reservations. The first page of each reservation carries `START`. `unmap`
    moves `START` to the remaining tail when it removes a reservation's head, as munmap does.
    It decides this before releasing its range, with a compare-and-swap that requires the page
    still to be reserved, so a concurrent reserve or unmap cannot be confused with the tail.
    This is a correctness matter for tests, not memory safety: a wrong flag can only make a
    later check panic, or leave a page unclaimable.
- Commit keeps contents (it only sets flags). Purge zeroes exactly the pages it acts on. An
  injected failure acts on no pages, or with `Faults::partially` on the first half of its
  range (`admit`), never beyond it. A partial unmap releases only the pages it acts on and marks
  the remainder as a reservation of its own, before releasing, as for a head.
- `fence` aborts every twin sequence in flight on its CPU and waits out a committing one
  (`cpus.rs`), so it returns only after each has completed or been aborted. It also waits out
  a magazine access in progress (from a passed `Access` step to `Machine::accessed`) before it
  aborts that sequence, so an aborted sequence makes no access after the fence returns, as a
  real one executes no further instruction. Tests: `os::sim::cpus::tests::
  a_fence_waits_for_an_access_in_progress_and_then_aborts_the_sequence` and `front::twin::tests::
  an_aborted_push_does_not_write_the_magazine_after_the_fence_returns`.
- That holds whoever moves the thread, and whenever. A thread's CPU and sequence state are one
  atomic word. A fence decides whether a thread is on its CPU from the same word it replaces,
  and a move (`SimThread::set_cpu`) changes the CPU and aborts the sequence in one
  compare-and-swap, after waiting out a commit or an access in progress, as a fence does. So a
  fence on the old CPU either finds the thread there and handles it, or finds it moved, and
  then its sequence was aborted and its access had ended before the move. When the CPU was a
  word of its own, stored before the interruption, a fence could read the new CPU, pass over
  a thread still accessing the old CPU's slab and return. Interrupting first would not do
  either: a free-running thread could start a sequence on the old CPU between the
  interruption and the store. `ReadCpu` takes its CPU from the word it swaps, for the same
  reason. Test: `os::sim::cpus::tests::
  a_fence_on_the_old_cpu_waits_for_an_access_that_a_move_interrupts`, which failed on the old
  order in each of five runs.
- `abort` panics, so it does not return.
- `ticks` reads a clock that only a test moves; the trait asks nothing of it.

## SIM-OS-02: `self.memory.zero(...)` in `purge` and `unmap`

**Preconditions of `Memory::zero`.** The range is inside the memory, and no other thread
touches it during the call.

**Proof.** `checked_range` returned, so `first` and `count` describe whole reserved pages inside
the memory; otherwise it panicked. The zeroed range is the first `applied <= count` of them. The
`purge` contract says no thread reads or writes the range during the call. The `unmap` contract
says nothing touches it after the call begins. So the write races with nothing.

## SIM-OS-03: `self.memory.zero(...)` in `wipe_for_fork`

**Proof.** The page index comes from `Pages::with`, which yields only indexes below the number
of pages, and `Pages::new` was given `memory.len() / page_size` pages, so
`index << page_shift` plus one page lies inside the memory. `wipe_for_fork`'s own `# Safety`
says no thread touches a marked page during the call, which is `Memory::zero`'s condition.
Zeroing a reserved page keeps invariant Z, which concerns unreserved pages only. `fork` calls
`wipe_for_fork` under the same condition, which its own `# Safety` states.

## SIM-OS-04: `self.memory.is_zero(...)` in `check_untouched`, `is_purged` and `resident_bytes`

**Proof.** Each page index is below the number of pages (it comes from a checked range or from
iterating the page states), so the page lies inside the memory. The no-writer condition is
discussed under SIM-MEMORY-05: uncommitted pages have no correct writer while checked
(`commit` holds them in `CHECKING`), and the two queries are documented as "call while nothing
writes".

**Review.** 2026-10-17, fresh adversarial reviewer: SIM-OS-01 and SIM-OS-02 accepted after the
fixes of its first round. Since then the simulator gained partial faults, fences that abort
twins, a whole fork, and the checks of SIM-OS-04; those parts need the reviewer's check. So
does the fence's wait for an access in progress, added after a free-running aborted push was
found storing into a magazine a drain had already passed on, and the one-word move that the
reviewer's second round asked for.
