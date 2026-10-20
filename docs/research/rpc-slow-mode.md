# The connect-rust server's slow mode: magazines locked at their ends

Measured 2026-10-20 at `db31497` (main with 1,024-slot magazines for classes up to 64 bytes),
on the reference host, with `bench/probes/loadgen` driving `rpc-server log` (connect-rust's
log-ingest server, asmalloc as `#[global_allocator]`, four workers on CPUs 0-3, 64
connections, closed loop for saturation and 63k req/s for steady load), under
`scripts/measure --both-l3`. Follows `server-latency.md`, which found the mode.

## Summary

- **The slow mode is still there on main, and it is not a flow between workers.** In about
  60% of server processes some (CPU, class) pairs enter a slow path on every request, in
  both directions: one allocation slow path and one free slow path per request per locked
  pair, on every CPU alike, for classes of 8 to 48 bytes and a few larger (640, 5,120).
  A process with 20 or more locked pairs runs at 228k to 236k req/s; one with none at 246k
  to 262k. jemalloc runs at 245k.
- **The mechanism is a swing that straddles a magazine's end.** Each swap installs a
  magazine at one end: an allocation slow path a full one (1,022 of 1,022 objects, every
  time), a free slow path an empty one. A request whose allocations and frees of a class go
  first below where they started and then above it (allocate, free, free, allocate) then
  crosses the far end of the new magazine before it ends, every request, and each crossing
  puts the next magazine at an end again. The swaps are with the CPU's own spares, so they
  are cheap (about 150 ns each, from the difference in throughput) but there are ten or more per request. A pair whose loaded
  magazine starts in the middle (after a partial fill) never reaches an end and never takes
  a slow path; a locked pair never leaves, because nothing ever installs a magazine in the
  middle and the age limit never sees a magazine an interval old. So the mode is decided per
  pair, early, by where the first magazines land, and lasts the life of the process.
- **Remedies.** Centring the magazine on a swap that reverses the last swap's direction
  removed the lock in all 16 processes tried: 246k req/s median against main's 238k, and at
  63k req/s 102k cycles per request against main's 108k and jemalloc's 102k. More spares
  (eight times the budget) changed nothing: 5 of 8 processes still locked, 233k median.
  Larger depot batches cannot help: almost no magazine goes to the depot.
- **The design's sizing rule would fire on some CPUs and would not end the mode.** The
  evidence it acts on, a magazine sent to the depot and fetched back, appears only on a
  locked CPU that has no spare room; one magazine of room then turns its depot trips into
  swaps with spares, as on the other CPUs. Its batch rule never fires, since the trips
  alternate in direction. Room makes a crossing cheaper; it cannot stop the crossing.
- **Unwind information**: the generated routines now carry `.cfi_startproc` and
  `.cfi_endproc` (commit `4124509`): a few lines in the Lean printer, no instruction
  changed, `lean/scripts/check.sh` passing.

## Method

A probe build (`probe-counts`, `crates/asmalloc/src/probe.rs`, on this branch only) counts,
per CPU and class, every slow-path entry and what it did: a magazine taken from the CPU's
spares, from the depot, or filled; where the displaced magazine went; stock admissions;
launders; and the levels of the magazines installed and displaced. The counters are a
static array that `serve.py` reads out of the server through `/proc/PID/mem` at the start
and end of each phase, so the server needs no code to report them. Saturation: 2 s of
warm-up, 4 to 5 s measured, a fresh process per run, variants interleaved in random order,
eight to ten rounds per comparison.

## The counts

A locked pair, per 1,000 requests (process at 233k req/s, class 24 bytes, CPU 0):

| | slow paths | magazine taken | displaced magazine | levels |
|---|---|---|---|---|
| allocation | 247 | full spare 238, depot 4, fill 5 | empty, to spares | installed 1,022 on average |
| free | 247 | empty spare 252 | full, to spares (stock) | displaced 1,022 on average |

Every CPU of a slow process looks like this for the locked classes, with allocation and free
slow paths equal on each CPU. A CPU with no spare room for a class trips to the depot both
ways instead (233 pops and 244 pushes per 1,000 requests on one), which is the only depot
traffic of any size. In a fast process every class takes fewer than one slow path per 10,000
requests.

| locked pairs (CPU, class), probe build | processes | req/s |
|---|---|---|
| 0 | 9 | 246k to 262k |
| 2 to 15 | 9 | 230k to 251k |
| 20 to 29 | 6 | 228k to 236k |

The split does not depend on where things are mapped (`server-latency.md`: the same with
address-space randomization off) or on magazine size (it was there with 254-slot magazines
before `d9f9295`), as the mechanism predicts: only where in its magazine each pair's swing
sits matters.

## Remedies, as experiments

Each is a build of this branch, interleaved with main's and jemalloc's, eight processes each.

| | req/s at saturation, median [range] | slow processes | kcycles per request at 63k req/s |
|---|---|---|---|
| main | 238k [229k to 253k] | 7 of 8 under 245k (no counters) | 108 [99 to 111] |
| eight times the spare budget (`probe-spares`) | 233k [229k to 255k] | 5 of 8 locked | 110 [98 to 113] |
| centre on a reversal (`probe-center`) | 246k [245k to 248k] | 0 of 8 locked (0 of 16 over two sessions) | 102 [101 to 103] |
| jemalloc | 245k [241k to 245k] | | 102 [102 to 103] |

**Centring** remembers, per CPU and class, the direction of the last swap at a magazine's
end. A swap that goes the other way is evidence of a swing that crosses both ends, and then
moves half of the magazine's objects so that the loaded one is left half full: an
allocation pops half of the newly installed full magazine into the empty one it displaced
and keeps that as a full spare; a free pushes half of the displaced full magazine into the
newly installed empty one. It costs one move of half a magazine (511 pointers for the
smallest classes, through the push and pop routines in the experiment), once per lock, and
parks nothing extra: the spare it keeps holds half a magazine instead of a whole one. A
one-way flow never reverses and is never centred. It recovers most of the gap but not all
of it: processes that happened to start with no locked pair reach 246k to 262k (most over 250k),
centred ones 244k to 250k. The rest is not explained; the counters show no slow paths left in
centred processes, so it lies outside the exchange (where the centred magazines' objects
sit, or code placement).

**More spares** cannot help, and did not: the locked pairs already swap with spares.

**Larger depot batches**, or larger magazines standing in for them, cannot help either:
a few percent of the locked pairs' slow paths reach the depot on CPUs that have spare room,
and a larger magazine still has ends. The smallest classes already have 1,024 slots.

## What the sizing rule would do here

The rule in `docs/design.md` ("Sizing from evidence") grows a class's room on a CPU when a
magazine it sent to the depot is fetched back within an interval, and its depot batch when
trips run in one direction.

- On a CPU with spare room (most locked pairs), nothing reaches the depot, so neither
  signal appears and nothing changes.
- On a CPU without room, a magazine goes to the depot and comes back within the same
  request, so the class earns one magazine of room at once, after which that CPU swaps with
  spares like the others. One magazine of room is all it needs; it recovers the depot trips
  (two atomic instructions each way) and not the slow paths.
- The batch rule never fires: the trips alternate.

So the rule, as written, would not end the slow mode at any amount of room. What the counts
show is a third kind of evidence, distinct from the two the design names: a swing that
crosses both ends of a magazine. Its remedy is not more room or larger batches but a
different starting level, which a slow path can choose without touching the fast path.

## Unwind information for the generated routines

Rust gives a naked function no unwind information, so `perf --call-graph dwarf` and
debuggers could not get from a sample inside a routine to its caller. Every generated
routine is a leaf that never moves the stack pointer, so the default frame of the System V
x86-64 CIE (return address at the stack pointer) is right throughout, and `.cfi_startproc`
and `.cfi_endproc` say all that is needed. The Lean printer now has `unwindStart` and
`unwindEnd` directives (x86-64: those two), and the emitter adds them to a naked function
whose printed code names no stack pointer, push, pop or call; anything else gets none. The
code a routine places in another section (an rseq abort handler and its exits) stays
without, which costs only call chains from samples there. No instruction changed,
`lean/scripts/check.sh` passes with the same sizes, and `readelf --debug-dump=frames` shows
an FDE for each routine where there was none.

In this server the samples in the routines themselves are rare; the call chains that were
missing in `server-latency.md` stop elsewhere (the `dwarf` unwinder gave user-space chains
for few samples of this binary even outside the allocator), which this change does not
address.

## Options

- **Centre on reversal**, as tested, in the slow path: a word of state per CPU and class (the
  last swap's direction), and a move of half a magazine when a swap reverses it. The move is
  better done as a copy between two magazines the thread holds (take the spare, copy half,
  install) than through the routines, as here.
- **Say in the design** that the evidence for a different starting level is a swap that
  reverses the last one, beside the two kinds of evidence it names.
- **Find the remaining 2 to 3%** between centred processes and those that never locked,
  before calling the slow mode closed.

## Reproducing

```
cargo build ... -p bench-rpc --features bench-alloc/asmalloc-probe    # target/asm-probe
python3 bench/probes/loadgen/serve.py run rpc --plan sat --rounds 6 --allocs build:asm-probe --out DIR
```

`serve.py` takes `build:NAME` for a native server built into `bench/target/NAME` and records
the probe's counters per phase when the server has them. `bench-alloc` has the features
`asmalloc-probe`, `asmalloc-center` and `asmalloc-spares` on this branch.
