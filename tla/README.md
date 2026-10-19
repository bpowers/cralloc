# TLA+ models of the lock-free protocols

Each protocol in `docs/design.md` that involves more than one thread has a small TLA+ model
here, checked exhaustively with TLC. The OS is an explicit actor in each: it stops, moves
and restarts threads, runs signal handlers, fences, forks, and hands out address space.

The models exist to find flaws in the design before it is implemented. A configuration that
is expected to fail is kept as a test: it records a flaw that was found, or shows that a
rule in the design is needed.

## Getting the tools

TLC needs Java 11 or later and `tla2tools.jar`. Neither is installed system-wide.

```
cd tla
curl -sL -o tla2tools.jar https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
```

`v1.8.0` is a rolling pre-release tag, so the file changes over time. The models were
checked with the build that reports `TLC2 Version 2026.10.15.145915 (rev: 932d35e)`,
SHA-256 `15d103b7e5617780bcc904c0af12f1ad242e2b345c0301136c5be4588f17edac`. The jar is
gitignored.

`./check` uses `java` from `PATH` if there is one, and otherwise runs
`nix shell nixpkgs#jdk_headless -c java`, which needs only `nix` and no root. Set
`TLA2TOOLS` to use a jar somewhere else.

`nix shell nixpkgs#tlaplus` also provides `tlc`, but it is the 1.7 line, which does not
understand the `ALIAS` line in the drain configurations.

## Running

```
tla/check                 # everything
tla/check Drain           # lines whose spec or configuration name matches
tla/check -v Drain_v1     # also print TLC's output, to read a counterexample
```

On a shared machine pin TLC and cap its workers:

```
TLC_PREFIX="taskset -c 11,26,27" TLC_WORKERS=3 tla/check
```

`models.sh` lists every check as `run SPEC CONFIG EXPECTATION`. `check` fails if any result
differs from its expectation, and prints the number of distinct states and the time for
each. The whole run takes about a quarter of an hour on three cores, most of it in
`Exchange` and the four largest drain configurations. `SLOW=1 tla/check` adds
`Drain_gen_any_full`, which takes about ten minutes more, and `Summary_run_wide`, which
takes about six (81 million distinct states, on four workers, 2026-10-18).

## The models

Every model is safety-checked over all interleavings within its bounds. "Never permanently
invisible" and "no thread ever waits" are stated as invariants (an obligation some thread
still holds, a step that is always enabled) rather than as temporal properties, which keeps
them cheap.

| Spec | Covers | Abstracts away |
|---|---|---|
| `Rseq.tla` | Pop, push and install as multi-step sequences that commit with the last step; preemption, migration and restart; cids, including an out-of-range cid. The slab invariant holds in every state, and no object is duplicated or lost. | The rest of the slow path is one step on a pool. |
| `Exchange.tla` | The slow path between routines: obtain, install, retire or put back; migration at every step; two threads on one cid; a signal handler as a nested call at any step; the marker branch; a call with no valid cid. Everything has exactly one owner and a call in progress always has a step it can take. | Routines, depot and back end are single steps. The drainer is an environment. |
| `Drain.tla` | The drain: claim, exchange of slab word and spare head, one fence, re-read, release; back-out when `membarrier` fails; concurrent drainers; drain against install, pop, push and spare push and pop in flight; fork during a drain and the child's repair. | One class and one spare stack per cid. Magazines carry a fill level, not objects. The depot is a set. |
| `Depot.tla` | Push and pop by compare-and-swap on a head with a version counter, with stale link reads and owners overwriting the link. | One stack. The depot has many: per class (and per magazine size, for empty magazines) one stack in each cache domain and one shared by all of them. A slow path pushes on its domain's stack while that is under a cap, else on the shared one, and pops its domain's, else the shared one. The stacks share no head: each has its own head and version counter, and a magazine is on at most one at a time, which the stack's own ownership argument gives (a push needs a magazine its pusher holds, a pop hands it to one thread). Which stack a slow path picks is a choice of heads, not a step of the protocol, so the one-stack model covers every stack. A capped push that finds its stack at the cap gives up after reading the head and the top's link, having written at most the link of the magazine it still holds (which the model lets an owner do), so the magazine stays its pusher's: in the model, a thread that does not push. The words beside the heads are shared across stacks and outside every model: each stack's water word, the sets of classes and domains that owe (`depot/shed.rs`) and the hint of which domains' stacks hold magazines (`depot/held.rs`). They only steer launders and looks, which are safe at any time; the interleaving checker covers their races (`depot/interleave.rs`), and no model does. |
| `Summary.tla` | Claim by compare-and-swap, release by atomic OR, set-on-first-free, clear-then-reread, at every level of a summary tree. With `Run` (`Summary_run*`), a fill's run of claims: searches that start at any word, empty words left to the thread that emptied them, and the clears of several emptied words made with one AND and a re-read of each. With `Keep` (`Summary_keep*`), a large class's claim: one that empties a word never clears its summary bit, and the next search to find the word empty does. With `Look` (`Summary_look*`), each free whose word was empty chooses whether to read the summary bit before setting it, as a scattered launder does while grouped launders and single frees on the same tree do not. | One region; two bits to a word (three in `Summary_run_wide`); batches only at the leaf level. |
| `Purge.tla` | Purge as a claim against allocation and laundering, with objects that do not line up with pages, and the zero bitmap of large classes. | Summary bits; the watermark. |
| `Startup.tla` | Racing initializers, one compare-and-swap, the loser unmapping, callers arriving throughout; a limited address-space budget; fork. | The heap's contents. |
| `Fork.tla` | One small instance of every shared structure, a fork at any step, and what the child can and cannot reach afterwards. | Routines and depot operations are single steps. |

### Bounds, and why they are enough

The flaws these protocols can have need few actors. A lost or duplicated magazine needs one
thread per conflicting view of a word: a sequence in flight, a drainer, and a second
drainer. An ABA on a stack needs one victim and one thread doing the pops and pushes. So
every model has two or three threads, two cids, two or three magazines of one or two
objects, and three or four objects. Each recorded counterexample is found within these
bounds, most of them within twenty steps.

What small bounds cannot show is a flaw that needs four or more distinct views of one word
at once, or a counter overflow; `Depot_wrap` covers the latter by shrinking the counter.

The drain configurations with a fork split the roles between threads to stay small.
`Drain_gen_any` has two threads; a three-thread version was explored to 22 million states
without a violation and not run to completion.

With two bits to a word, a batch of held clears in `Summary_run` is at most two words, and
they are every word under their summary word. `Summary_run_wide` has three bits to a word
and two threads, so a batch can leave a sibling's bit set in the summary word.

`Drain.tla` keeps no cid for a thread outside a sequence. A sequence starts on any cid that
no sequence in flight is using, and the OS may abort any sequence at any time. This is the
same set of behaviours as explicit preemption and resumption, with far fewer states;
`Rseq.tla` models cids without the shortcut.

## Findings

Each has a configuration that reproduces it. Run `tla/check -v NAME` for the trace.

1. **Two drainers, one marker value** (`Drain_v1`; fixed in the design by the drain claim).
   A sequence in flight commits over the first drainer's marker, a second drainer exchanges
   the new word, and the first then finds "its" marker and takes a stale word.
   `Drain_claim` and `Drain_backout` check the claim-based protocol, including back-out.

2. **The child's sweep races new drains** (`Drain_sweep`). The sweeping thread has not
   reached a slab yet; another thread claims that slab and exchanges its word; the sweep
   then clears the live marker and the flag; the drainer re-reads zero and drops the
   magazine it displaced. A longer trace clears a live flag and reproduces finding 1
   (`Drain_sweep_dup`).
   Smallest change: no drain starts until the sweep has finished (`Drain_sweep_hold`).
   Superseded by the fix to finding 3, which has no sweep.

3. **Fork from a signal handler that interrupted a drain** (`Drain_sweep_hold_any`). The
   forking thread is the drainer, so its claim and markers are live in the child. The sweep
   cannot tell them from a dead thread's and clears the flag; a second drainer claims the
   slab, and finding 1 follows: a magazine is taken at a stale fill level. Holding drains
   back does not help.
   The fix, now built (`Drain_gen`, `Drain_gen_any`): the flag and the marker carry the
   process's fork generation. A claim or marker of an older generation counts as absent: a
   drain may claim over it, and an exchange that returns it has displaced nothing. A
   drainer accepts only a marker of its own generation at step 4, and releases its claim
   with a compare-and-swap. There is no sweep. The generation comes from the wiped page:
   a slow path that reads it as zero adds one to a counter kept in ordinary memory and
   installs the result in the wiped page with a compare-and-swap, so every process has a
   generation greater than any ancestor's. Nothing is ever duplicated. (As built, the
   generation is a random number the child draws instead, which differs from every
   ancestor's with overwhelming probability; see "Fork" in the design.) In the case of this
   finding the forking thread may still lose the magazines it had displaced
   (`Drain_gen_any_loss`), which is a bounded leak in a case that needs `fork` inside a
   handler and a second thread draining the same slab in the child.
   A slab left with stale markers stays on the slowest path until some drain visits it, so
   a slow path that meets a stale marker should drain that slab. The model lets a drain
   start on any slab at any time and does not model that trigger, nor the two-word
   generation update.

4. **A summary bit left clear in a forked child** (`Summary_crash`, `Fork_summary`). A
   thread sets a word's bits and the fork takes it before it sets the summary bit (or
   between clearing the bit and reading the word again). In the child the word is not
   empty, so no later free into it is a "first free", and nothing sets the bit: the word's
   objects, and under a higher-level bit a whole subtree, stay invisible to every search.
   The design's claim that the child's shared structures are valid does not hold for this
   hint. The fix, now built (`Summary_heal*`): after its OR, a free reads the summary bit at
   each level on the way up and sets any it finds clear, not only when its word was empty.
   That costs a load per level and repairs the path the next time anything is freed into
   the word. It does not make the objects visible before then.

5. **Start-up with too little address space for every racer** (`Startup_tight_size`,
   `Startup_tight_null`). "As much as the process is allowed" depends on what other
   initializers hold at that instant. One racer reserves everything, a second gets the
   scraps and wins the compare-and-swap, and the first unmaps: the heap is small for the
   life of the process. Or the second gets nothing and fails its first allocation while a
   heap is about to be published. This needs `RLIMIT_AS`, or eight threads making their
   first call at once. No fix is proposed or checked here. Reading the heap word again
   after a short or failed reservation would narrow the window without closing it.

`Fork.tla` checks the rest of what a child inherits. With the forking thread outside the
allocator, what the child cannot reach is exactly what the other threads held privately:
per thread, at most one magazine with its objects, plus one object being freed or the bits
of one bitmap word being claimed, laundered or purged. A reservation that an initializer
had not yet published or unmapped stays mapped (`Startup_fork`).

## Readings of the design

Where the prose was open, the models take the reading below. Each is a suggestion for the
design text.

- **Step 4 of the drain writes zero whenever the word is still the marker**, whether or
  not the old value named a magazine. Writing zero only when it did leaves a marker in
  every word that held none (`Drain_zerorule`).
- **Back-out restores a displaced value only by compare-and-swap from the drainer's own
  marker**, and never restores a marker. Sequences in flight may then still commit, which
  is ordinary operation (`Drain_backout`).
- **A routine's commit stores a value computed from the word it read.** Adjusting the word
  in place corrupts it when a drainer has exchanged it (`Drain_inplace`).
- **Where an unloaded magazine keeps its fill level** is not stated. The slab word carries
  it for a loaded magazine; the models keep it in the magazine while it is in a spare
  stack or the depot. The depot's "non-empty" stack holds partly full magazines, because
  the marker branch sends one there after serving a single request, and a magazine emptied
  that way belongs on the empty stack.
- **Summary bits at higher levels** follow the one-level rule applied recursively. A
  search that finds an empty word under a set bit also clears and re-reads.
- **A purge claims every object that touches the pages it returns**, not only those inside
  them, since class sizes need not divide the page size (`Purge_claim_inside`). It sets the
  zero bit only of objects wholly inside, and before it releases their free bits
  (`Purge_zero_claimed`, `Purge_zero_after`). An allocation tests and clears the zero bit
  after it has claimed the object. The rule behind all of this: a zero bit is written only
  by the holder of the object's claim.
- **Everything a caller needs from the heap is in place before the publishing
  compare-and-swap**, including whether rseq is usable.
- **A slow path that holds a magazine and finds its cid out of range** treats that as the
  marker case: it serves the request from the magazine and sends it to the depot.
- **With no magazine to be had** (the magazine array is exhausted), the slow path serves
  the call from the back end.

## Not modelled

The heartbeat's pacing and the activity counter (a drain may target any cid at any time),
the retained-memory cap, huge-page policy, large-object resize, the choice between
`MADV_DONTNEED` units, and `membarrier` registration. A 32-bit depot counter leaves open
only the case in which exactly a multiple of 2^32 pushes and pops land between one
thread's read of the head and its compare-and-swap, and the head names the same magazine
at both ends; `Depot_wrap` shows the failure with a counter of four values.
