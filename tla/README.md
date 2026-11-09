# TLA+ models of the protocols

These models check cralloc's lock-free protocols as designed (`docs/design.md` defines the terms).
The OS is an actor in each: it stops, moves and restarts threads, runs signal handlers, fences,
forks and hands out address space. TLC explores every interleaving of a few threads over a few
magazines and objects.

A pass means no interleaving within those bounds breaks the model's properties. It does not cover
weak memory, since every step is atomic and sequentially consistent, or the Rust, whose schedule
explorers follow these models by review, not proof (`back/models.rs` and `depot/interleave.rs` in
`crates/cralloc/src`). `docs/research/memory-ordering.md` says which words those steps are true of
and argues the rest; `armcheck.py` here checks by litmus test what that argument takes from Arm's
memory model.

A check expected to fail is kept on purpose: it shows the model can see a flaw, and that a rule is
needed or a limit is real.

*Cid* in the models means the number that picks a slab: the CPU number, not the kernel's concurrency
ID (`docs/design.md`, "What Linux provides"). The models need only that two running threads never
share it.

## Running them

TLC needs Java 11 or later and `tla2tools.jar`, which is gitignored. From the top of the repository:

```
curl -sL -o tla/tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
```

`v1.8.0` is a rolling tag, so this can fetch a newer build than the one behind the recorded results:
`TLC2 Version 2026.10.24.013619 (rev: 2ef2af8)`, SHA-256
`d15655eeaff4b3f7b3d65c46b838bc0f35d131b2ef4b1e473689b62c98d4726f`.

`tla/check` uses `java` from `PATH`, or else `nix shell nixpkgs#jdk_headless -c java`. In a git
worktree, set `TLA2TOOLS` to the jar's path. Run it through `scripts/heavy`, like all heavy work:

```
scripts/heavy tla/check               # every check
scripts/heavy tla/check Purge         # the checks whose spec or configuration name contains this
scripts/heavy tla/check -v Drain_v1   # also print TLC's output, to read a counterexample
SLOW=1 scripts/heavy tla/check        # four more checks as well
```

`check` fails if a result differs from `models.sh`. A full run takes a little over an hour on the
default three workers (`TLC_WORKERS`), and `SLOW=1` adds most of another. State files can reach
gigabytes, under `target/tla` or `TMPDIR`.

## The models

| Spec | Covers | Abstracts away |
|---|---|---|
| `Rseq` | Pop, push and install as steps that commit with the last; preemption, migration, restart; an out-of-range cid. | The rest of the slow path. |
| `Exchange` | The slow path between routines, with the half-full swap; migration at every step; a handler as a nested call; a live marker; no valid cid. | Routines, depot and back end are single steps. The drainer is an environment. |
| `Drain` | Claim, exchange, fence, re-read, release, back-out; concurrent drainers; sequences in flight; a fork at any step. | One class and spare stack per cid; the depot is a set; magazines carry a fill level, not objects; the generation is a counter. |
| `Depot` | Push and pop by compare-and-swap on a versioned head; stale link reads; owners overwriting the link. | One stack, since each has its own head. A capped push that gives up did not push. |
| `Summary` | Claim, release and clear-then-re-read at every level of the tree, with the healing free (`Heal`), the batched clears of a fill and of a purge's claims (`Run`), a claim that keeps its bit (`Keep`), and a free that reads a bit first (`Look`). | One region; two bits to a word (three in `Summary_run_wide`); batches only at the leaf level; how far a search walks, so whether a fill waits. |
| `SummaryLookUp` | `Summary` with a free that may read every level of its walk (`tree::release_one`), and claims that keep their bits. | As `Summary`. |
| `Purge` | A purge as a claim against allocation and laundering; objects that straddle pages; the zero bitmap. | Summary bits, the watermark, clean-page bits; which pages a purge picks. |
| `Startup` | Racing initializers, one compare-and-swap, the loser unmapping; a limited address-space budget; a fork. | The heap's contents; the allocator's retry and halving. |
| `Fork` | Slab words, drain flags, magazines, the depot, a bitmap word and summary bit, the purge claim, and a fork at any step. At the fork a child loses only what other threads held privately: at most a magazine each, and an object being freed or one bitmap word's bits. | Routines and depot operations are single steps. No sequences in flight, no spare stacks. |
| `Tail` | The word saying where a region's huge-page advice is off: owner bit, advice call, give-back; an OS that refuses part-way; an owner taken over while alive; a fork; a claim's mark and its flag. | One region of two or three huge pages; when a change is due. |
| `TailStale` | A thread holding the previous interval against a fresh owner, with a stamp ahead of it read as fresh (`Tail::owned`). | A copy of `Tail` with no flag or looks. |
| `Assign` | How a class takes a region, where a walk learns its class, and a fork child's repair of what a cut-off thread named. | Two regions; each assigner and the run go once; a carve is one fact. |

`SummaryLookUp_deep_crash.cfg`, a fork at two summary levels, is not in `models.sh`: it did not
finish (stopped at 723 million states, no violation). `kept_bits_and_single_frees_lost_at_any_step`
in `back/models.rs` covers the case on the code.

## Why small bounds are enough

The flaws these protocols can have need few actors: a lost or duplicated magazine needs one thread
per conflicting view of a word, such as a sequence in flight, a drainer and a second drainer. So two
or three threads, two cids and a handful of magazines and objects are enough, and the
counterexamples below take about twenty states at most. Small bounds cannot show a flaw that needs
four or more views of one word, or a counter overflow, which `Depot_wrap` reaches by shrinking the
counter.

## The rules, and what fails without them

- **A stopped sequence restarts, and two running threads never share a cid:** the OS guarantees the
  design trusts (`Rseq_norestart`, `Rseq_sharedcid`).
- **A half-full swap installs only over the zero it left,** or it overwrites another call's magazine
  (`Exchange_centre_blind`).
- **A commit stores a value computed from the word read,** or it corrupts a word a drainer exchanged
  (`Drain_inplace`).
- **A drain claims the slab first,** or a second drainer re-marks a word a sequence committed to,
  and the first takes a stale word (`Drain_v1`).
- **A drain writes zero whenever the word is still its marker,** or markers stay in words that held
  no magazine (`Drain_zerorule`).
- **A claim or marker carries the process's generation, and another generation's counts as absent**
  (`Drain_gen`, `Drain_gen_any`). A cut-off drain needs some repair (`Fork_norepair`), but a sweep
  races later drains (`Drain_sweep`, `Drain_sweep_dup`) and, with those held back, cannot tell a
  dead thread's claim from a live drainer's after a fork from a handler (`Drain_sweep_hold_any`).
- **A stack head carries a version counter,** or a stale compare-and-swap succeeds (`Depot_nover`).
- **Whoever clears a summary bit then re-reads the word,** or a free made meanwhile stays invisible
  (`Summary_noreread`, `Summary_run_noreread`).
- **A free sets every summary bit it finds clear on its way up,** since a fork can leave one clear
  over a non-empty word (`Summary_crash`, `Fork_summary`). Until the next free into that word, its
  objects stay hidden.
- **A purge claims every object touching its pages** (`Purge_claim_inside`) **and sets zero bits
  only of objects wholly inside, before releasing them** (`Purge_zero_claimed`,
  `Purge_zero_after`).
- **An owner whose bit was taken over asks for a repair,** or its late call leaves the advice wrong
  (`Tail_norepair`).
- **A fork child asks for that repair on every tail word** (`Tail_orphan`), **clears every owner bit
  and flags any mark,** or the mark goes unanswered (`Tail_stuck`).
- **A thread that clears the flag and loses its take of the word tries again,** or nothing finds the
  mark (`Tail_lost`).
- **A walk takes a region's class from the published tag, not the table,** or it can act on another
  owner's region (`Assign_table`).
- **A child's repair re-reads the tag after finding the region carved from,** or it publishes
  another class's region (`Assign_repair`).

Three failures have no rule behind them. They are limits we accept:

- **A fork from a handler that interrupted a drain can lose what the drain displaced,** if a second
  thread drains that slab in the child: a bounded leak, nothing duplicated (`Drain_gen_any_loss`,
  `Fork_gen_any_loss`; `Fork_any_loss` under a sweep).
- **A depot counter can wrap,** given exactly a multiple of 2^32 pushes and pops during one
  compare-and-swap attempt, with the same magazine on top (`Depot_wrap`).
- **Racers can share too little address space,** under `RLIMIT_AS` or with eight or more first calls
  at once. A short reservation can win and leave the heap small for good (`Startup_tight_size`), or
  a first allocation can fail just before a heap is published (`Startup_tight_null`). The
  allocator's retry narrows this to a racer stalled through a bounded patience. It is never unsafe
  (`Startup_tight`).

## What is not modelled

The models leave out what decides when a protocol runs: a drain, purge or advice change may start at
any time. So the heartbeat's pacing, the activity counter, the retained-memory cap and where
huge-page advice should go are outside them. So are:

- installing over another generation's marker: the slow paths here treat every marker as live, and
  `a_fork_at_any_step_duplicates_nothing` (`depot/interleave.rs`) explores the real install;
- carving from a watermark (`carving_threads_get_disjoint_slots` in `back/models.rs`);
- large-object resize and `membarrier` registration;
- the other words more than one thread writes: `docs/design.md`, "Models of the protocols", says why
  a flaw in one cannot cost a block.
