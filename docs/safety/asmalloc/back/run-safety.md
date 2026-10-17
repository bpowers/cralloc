# Safety ledger: `crates/asmalloc/src/back/run.rs`

## BACK-RUN-01: `self.space.purge(ptr, regions << shift)` in `Back::free_run`

**Preconditions** (`Space::purge`). No thread reads or writes the range during the call, and
nothing in it is needed afterwards. (`Space::purge` itself aborts on a range outside the
reservation or not whole pages.)

**The range.** `lookup(ptr)` returned `Kind::Run(regions)`, so `ptr` is the base of a region
tagged `RunHead`, and the next `regions - 1` regions are tagged `RunTail`. The range is those
regions: whole regions of the reservation, so whole pages inside it.

**Why the tails are this run's.** `claim_run` takes a run's head before any of its tails, each
with a compare-and-swap from `Unassigned`, and gives back tails before the head. So a `RunTail`
region always has its own head, unassigned or not, below it with only its own tails between,
and the count of consecutive tails after a head stops at the next run's head or at a region
that is not a tail. The regions counted are exactly the ones this run's allocation claimed.

**Proof.** `free_run` is called by the owner of the run to free it: `Back::free_run`'s contract,
and `free_by_address`'s, is that `ptr` is a live run the caller is giving up. By that contract
the program no longer reads or writes it. The allocator keeps nothing in object memory, and no
other thread can have been handed these regions, because they stay tagged until after the
purge. So nothing touches the range during the call, and nothing in it is needed.

**What the contract does not cover.** A double free of a run is not detected: a second
`free_run` of the same address, after another thread has allocated a new run over the same
regions, would purge that run's memory. Slots in class regions catch a double free with their
bitmap; runs have no per-object bit to test. This is the same exposure as any allocator that
returns memory on free.

If the purge fails, `free_run` returns without giving the regions back: they stay tagged and
are never reused, so the promise that unassigned regions read as zero still holds.

**Machine checks.** `back::tests::runs_hold_objects_bigger_than_a_region`,
`freeing_a_run_tail_aborts` and the shadow-model runs (`random_operations_*`), natively and
under Miri; fault injection makes purges fail in `random_operations_survive_os_failures`.

**Review.** Awaiting a fresh adversarial reviewer.
