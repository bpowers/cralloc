# Safety ledger: `crates/cralloc/src/back/run.rs`

## BACK-RUN-01: `self.space.purge(ptr, regions << shift)` in `Back::free_run`

**Preconditions** (`Space::purge`). No thread reads or writes the range during the call, and
nothing in it is needed afterwards. (`Space::purge` itself aborts on a range outside the
reservation or not whole pages.)

**The range.** `lookup(ptr)` returned `Kind::Run(regions)`, so `ptr` is the base of a region
tagged `RunHead`, and the next `regions - 1` regions are tagged `RunTail`. The range is those
regions: whole regions of the reservation, so whole pages inside it.

**Why the tails are this run's.** `claim_run` takes a run's head and then its tails in address
order, each with a compare-and-swap from `Unassigned`, and `unclaim_run` gives them back in the
reverse order, the head last. A fork can stop a thread anywhere in either, which only leaves such a
prefix for good. So a `RunTail` region always has its own head below it with only its own tails
between, and the count of consecutive tails after a head stops at the next run's head or at a
region that is not a tail. The regions counted are exactly the ones this run's allocation claimed.

**Proof.** `free_run` is called by the owner of the run to free it: `Back::free_run` and
`free_by_address` are crate-private, and their callers (`Heap::free_large`, the C `free`,
`move_large_with`) pass a live run the program, or the move, is giving up. By that contract
the program no longer reads or writes it. The allocator keeps nothing in object memory, and no
other thread can have been handed these regions, because they stay tagged until after the
purge. So nothing touches the range during the call, and nothing in it is needed.

**What the contract does not cover.** A second free of a run aborts while its head is unassigned,
since `lookup` finds no run there. It is not detected once another thread has allocated a new run
with the same head, or during the first free's purge: it then purges memory that may be the new
run's, and its `unclaim_run` can take back that run's tags. Slots in class regions catch a double
free with their bitmap; runs have no per-object bit to test. This is the same exposure as any
allocator that returns memory on free.

If the purge fails, `free_run` returns without giving the regions back: they stay tagged and
are never reused, so the promise that unassigned regions read as zero, which a move into a new run
relies on (HEAP-04), still holds. The only other way a region comes back unassigned is an assignment
a class lost (`Back::assign`), before anything was carved there.

**Machine checks.** `back::tests::runs_hold_objects_bigger_than_a_region`,
`freeing_a_run_tail_aborts` and the shadow-model runs (`random_operations_*`), natively and
under Miri; fault injection makes purges fail in `random_operations_survive_os_failures`.

Reviewed: 5a8ade1
