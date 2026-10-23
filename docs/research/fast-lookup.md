# The class lookup when the size is not a constant

2026-10-22. The change is `4b4f3f4` on branch `fast-lookup`, from main `2754021`, with the masks
moved after the slot sizes on review; it was merged with main `1eea9af` before it went to main.
Every measurement below is `4b4f3f4` against `2754021`, not against the main it was merged with.
Measured on the reference machine (`zen5-roofline.md`, section 2).

## Summary

- When a layout is not a compile-time constant, the inlined fast path finds the class byte, the
  slab word's address and the magazine mask before it calls the pop or push routine. At a call
  site outside a loop that took **10 instructions; it now takes 8** (6 inside a loop, from 8).
  Allocations and frees whose size is a constant compile to the same instructions as before.
- The lookup costs instructions, not latency. The word's address is ready about as soon as the
  routine has the CPU number, and the mask is not on the critical path at all: an immediate in
  its place makes the loop no faster.
- **A mask that the routine loads from memory makes constant-size calls slower**, by 0.4 to 1.0
  cycles an allocate and free, although it saves an instruction. So the routine keeps taking the
  mask in a register, and a constant class keeps it as an immediate. This rules out the two
  designs that change the Lean routines, and with them the server profile's suggestion of one
  table load giving both the word and the mask.
- Cycles per allocate and free, averaged over four code placements: in the executable form
  −0.21 to −0.55 on the run-time-size loops (3 to 5%); through the GOT, the crate's default,
  −0.32 to +0.08 (`batch_dyn` level, `pair_mix` within its spread); unchanged on constant sizes
  in both.
- On the log store, over five placements, the change is 1.6% lower at the mean and lower at
  every pad, within what two rounds can resolve: no regression. Of the kernel suite's 25
  small-object jobs, 19 are faster. `pipeline t8` is 16% faster, far more than the instructions
  saved; the gain follows the change, needs both of its halves, and its mechanism is not known
  (below).

## What the lookup was

`GlobalAlloc::alloc` and `dealloc`, and `alloc_zeroed` and the `Allocator` methods with them, take
the class from the layout with `SmallClass::of_nonzero_layout`, then hand the routine the
address of the class's word in slab 0 and the class's mask. With a run-time size, fat LTO,
executable form, one call outside a loop (`site_alloc_one` in `bench/roofline/rust-alloc`):

```
lea   rax,[size-1]                ; with a run-time or large alignment: lea, lea, or
cmp   rax,0x8000 ; jae cold
shr   rax,3
lea   rcx,[rip+BY_SIZE]
movzx edx,BYTE PTR [rax+rcx+0x800] ; the class byte: the word's position p
lea   eax,[rdx*8]
lea   rdi,[rip+SLABS]
or    rdi,rax                      ; SLABS | 8p: LLVM turned the sum into `or`
lea   rax,[rip+MASKS]
mov   esi,DWORD PTR [rax+rdx*4+0x800]
```

A constant layout folds all of it to `lea rdi,[rip+SLABS+8p]` and `mov esi,mask`. In
`rpc-server` 1,134 of the 1,820 inlined routines have a run-time size, and many of those a
run-time alignment too (a `Box<dyn Trait>` drop reads both from the vtable). Alignment costs
nothing more than its `or`: for a constant alignment of 8 or less LLVM drops it, and an
alignment above what the size's class gives is handled by the same rounding, with no test.
`Allocator::allocate` also computed the class size for the returned length, about seven
instructions and a branch. The C entry points are generated routines with tables of their own,
untouched here.

**The critical path.** The routine needs the word's address and the CPU's slab offset before
its slab-word load. From the size to the address is `lea`, `shr`, the class-byte load (the
five-cycle form, with an index and a displacement), `lea` and `or`: about 9 cycles. The CPU
number takes two dependent loads and a shift, about 9 to 10 cycles in the executable form and
13 to 14 through the GOT. So the class chain is hidden behind the CPU number through the GOT and
level with it in an executable. The mask comes from a second load that waits on the class byte,
about 12 cycles after the size, but nothing waits on it but a predicted branch: built with the
mask as an immediate (probe `v1c`, correct only for the size it ran), `pair_dyn` ran no faster
(+0.34) and `batch_dyn` the same (−0.05).

## What changed

1. The word's address is a pointer offset from the slabs (`Front::word`), which LLVM emits as
   one `lea` from the slabs' address. As an integer sum it became `lea` and `or`.
2. The class bytes, the masks and the slot sizes share one page-aligned static
   (`classes/lookup.rs`), so one base address serves every load. The masks are 16 bits (the
   largest is 8,176), by word position; the class bytes stay 2048 bytes into the first
   page; the slot sizes, by class index, follow them, and the masks follow the slot sizes, at
   page offsets 2176 to 2687 of the second page. All are clear of every field of a slab (`front` asserts it for each class, a test checks the linked
   addresses). `front::MASKS` is gone, and the static takes the two pages `BY_SIZE` did.
3. `Allocator::allocate` reads the length from the slot sizes' table: one shift and one load.

After, the same call site:

```
lea   rax,[size-1]
cmp   rax,0x8000 ; jae cold
shr   rax,3
lea   rcx,[rip+LOOKUP]
movzx edx,BYTE PTR [rax+rcx+0x800]
lea   rax,[rip+SLABS]
lea   rdi,[rax+rdx*8]
movzx esi,WORD PTR [rcx+rdx*2+0x1880]
```

`of_nonzero_layout` keeps its signature. Kani proves every mask entry equal to the class's
magazine mask and every size entry equal to `Class::size`, and zero for every other position
(`classes/verify.rs`); the existing proof that the class bytes give `Class::of_layout`'s class
stands.

**Lines touched.** For the server profile's mix (sizes 8 to 448 bytes, eleven classes), the
lookup reads one line of class bytes and, for the masks, five lines before and three after.
A constant-size call reads none, before and after.

## Measurements

**Method.** `bench/roofline/rust-alloc`, built with fat LTO against each version of the crate,
run on CPU 2 under `scripts/measure`. Each workload is 20 million allocate-and-free pairs,
repeated four times, the first a warm-up and the fastest of the rest reported, in cycles and
instructions per pair from user-space counters. Placement moves these loops by up to a cycle a
pair (`zen5-roofline.md`, section 15), so each version was built at four placements (`RA_PAD`
0, 16, 32 and 48: that many bytes of `nop` before each workload's loop), all builds run in a new
random order in each of three rounds, the median taken at each placement and then the mean over
placements. Rounds agreed to within 0.1 cycle in nearly every cell.

Workloads: `pair_const` and `pair_box`, a constant 32-byte layout, plain and through `Box`;
`batch_const`, 32 allocations then 32 frees; `pair_dyn` and `batch_dyn`, the same with the size
(32) hidden from the compiler; `pair_mix` and `batch_mix`, sizes cycled from a list in the
server profile's proportions (mostly 16 and 24 bytes, a tail to 448); `churn`, 10,000 live
objects of 16 to 512 bytes, one freed and one allocated per step at random.

**Main against the change**, cycles per pair, mean over placements (range over placements), and
instructions per pair:

| | main | change | difference |
|---|---|---|---|
| `pair_const` | 4.96 (4.95–5.00), 40 | 4.96 (4.95–5.00), 40 | 0.00 |
| `pair_box` | 5.89 (5.89–5.91), 44 | 5.90 (5.89–5.91), 44 | +0.01 |
| `batch_const` | 5.35 (5.35–5.37), 43.3 | 5.36 (5.34–5.38), 43.3 | +0.01 |
| `pair_dyn` | 7.47, 60 | 7.26, 56 | −0.21 |
| `batch_dyn` | 7.54, 59.3 | 7.29, 57.3 | −0.25 |
| `pair_mix` | 9.39, 55 | 8.93, 53 | −0.47 |
| `batch_mix` | 10.25, 64.5 | 9.71, 60.5 | −0.54 |
| `churn` | 24.46, 92.5 | 23.91, 90.5 | −0.55 |

The constant workloads' functions are the same instructions in both builds (compared after
taking out addresses).

Through the GOT, the crate's default form, the same way:

| | main | change | difference |
|---|---|---|---|
| `pair_const` | 5.47 (5.43–5.49) | 5.59 (5.43–5.96) | +0.12, from one placement (below) |
| `pair_box` | 6.93 | 6.95 | +0.02 |
| `batch_const` | 6.19 (5.98–6.39) | 6.21 (5.99–6.43) | +0.02 |
| `pair_dyn` | 7.76, 62 | 7.52, 58 | −0.24 |
| `batch_dyn` | 8.02, 61.3 | 8.02, 59.3 | 0.00 |
| `pair_mix` | 9.76 (9.65–9.89) | 9.84 (9.62–10.38) | +0.08, within the runs' spread |
| `batch_mix` | 10.93, 66.5 | 10.68, 62.5 | −0.25 |
| `churn` | 24.84, 94.5 | 24.52, 92.5 | −0.32 |

At one placement the change's `pair_const` ran 5.96 in all three rounds against 5.43 to 5.49 at
the other three; its instructions there are the same as main's at that placement, so that is
where the loop landed, not the change.

**What was tried and dropped.** Each a scratch build of the crate, measured the same way, as
a difference from main in cycles per pair. "Ops" counts instructions at the call site outside a
loop, run-time size and constant.

| | ops | `pair_dyn` | `batch_mix` | `churn` | `pair_box` | `batch_const` |
|---|---|---|---|---|---|---|
| the word as one `lea` only | 9 / 2 | −0.17 | −0.36 | −0.28 | −0.01 | 0.00 |
| that, and one static for class bytes and 32-bit masks | 8 / 2 | −0.19 | −0.40 | −0.47 | +0.02 | +0.02 |
| the routine reads the mask at `MASK_DISP` from the word, as the C routines do | 7 / 1 | −0.66 | −0.19 | −0.83 | +0.38 | +0.08 |
| the routine reads the mask from the slab, beside the hot word | 7 / 1 | −0.64 | −0.16 | −0.99 | +0.84 | +0.51 |
| main with the constant mask loaded instead of an immediate (probe) | 2 loads | 0.00 | +0.06 | +0.06 | +0.36 | +0.40 |

The last two designs would need new Lean routines, and both save the mask's instructions on
every call. They lose on constant sizes, through the GOT as well (the per-slab mask: +0.67 on
`pair_box`, +0.98 on `pair_const`), and the probe in the last row shows why: it changes nothing
but where the constant path's mask comes from, and costs as much. Counters on `pair_box` put the
extra cycles where nothing retires while a load is outstanding (1.04 cycles a pair on main, 1.57
with the mask at `MASK_DISP`, 1.84 with it in the slab), with no more blocked loads; the per-slab
mask, whose address waits on the CPU number, costs twice what the fixed one does. Why a
predicted branch's operand delays these loops was not found; the rule that follows is that the
routine's mask stays in a register.

Also not built:

- **One table load giving both the word's offset and the mask**, the server profile's
  suggestion. It shortens only the mask's chain, which the probe above shows does not matter, and
  an 8-byte entry per 8-byte size step is eight times the class bytes' footprint and leaves fewer
  sizes clear of the hot words' page offsets.
- **Computing the class for small sizes** (to 64 bytes, `(size + 7) & -8` gives the word's
  offset and every mask is the same): a branch on the size, which mispredicts at sites that see
  many sizes, or, without it, more instructions than the load it saves.
- **A smaller class table.** The byte table already touches one line for every size up to 512.

## Servers

`connect store 1M`, the job where the fast path is the largest share (10.6% of its cycles, about
5,200 calls a request, most of them run-time sizes). Main and the change were each built at
bench-alloc's pads 0 to 64 (`bench/probes/server-profile/pad-orders.sh`'s method, for the
workloads and `rpc-server`) and run interleaved in one `benchrun run` per half of the pads,
two rounds, under `scripts/measure --both-l3`; combined with `probes/spawn-gap/padsweep.py`.
Server cycles a request at 70% load, thousands, the two runs of each cell:

| | pad 0 | 16 | 32 | 48 | 64 | mean of medians | range |
|---|---|---|---|---|---|---:|---:|
| main | 363.5, 368.8 | 371.9, 365.8 | 371.2, 373.8 | 369.8, 367.0 | 369.5, 371.0 | 369.2 | 1.7% |
| change | 362.6, 357.1 | 364.2, 359.0 | 364.6, 358.4 | 365.4, 363.3 | 371.2, 368.5 | 363.4 | 2.8% |

The change is lower at every pad, 1.6% over the mean. Runs of one cell differ by up to 1.7%,
and this job's placement spread is of the same size, so the sweep shows no regression and is
consistent with a gain of a few tenths of a percent to the 1.6% it shows; it cannot say more
from two rounds. My estimate from the instructions saved is the low end: about two
instructions on each of 5,200 calls, a fraction of a cycle each.

**The kernel suite's small-object jobs** (`--suite full --only small`, 25 native jobs, sizes 16
to 512 bytes, so mostly run-time sizes), main and the change at pad 0, three sessions of one
interleaved round each. Median, the change against main:

- 19 jobs faster: the six `churn small` jobs by up to 5.7% (live=1M +0.3%), the exchanges by 0.6
  to 8.5%, the `mpmc` jobs with one side local by 1.1 to 2.8%, the local pipelines by 0.8 to
  5.9%.
- `pipeline t8 both all-alloc` 16.5% faster; `mpmc p4 c4 both` 2.5% slower, level over the five
  pads (−1.0% on the means; main's own range there is 18%).
- `pipeline t2 split all-alloc` and `free-only` 1.1% and 3.0% slower, and 1.8 and 1.9% over the
  pads. These jobs are bimodal for every build, at about 1.00e9 or 1.13 to 1.20e9 cycles, and
  these sessions fell wholly in the slow mode; in the fast mode the change is level or slightly
  ahead (the reviewer's runs). That is a mode of the job, not a cost of the change.

**`pipeline t8`.** 16.4% faster at every pad, on 5% fewer instructions. Over eight builds run
interleaved (the reviewer's), every build with the whole change is fast and both mains are slow,
at four different page offsets of `HEAP` each way, so it is not where a static landed. Main with
only the single `lea`, and the change with the integer sum put back, are slow too: the gain
needs both halves. About 0.28e9 of the 0.45e9 fewer instructions are the lookup itself; the rest
is probably less spinning in a shorter run. The mechanism is not known.

## Why the servers' fast path costs more than `pair_dyn`

The server profile (`server-profile.md`) puts the fast path at 4 to 12 cycles a call; `pair_dyn`
runs 3.7 a call. From what was measured here and there, without a new measurement, the
difference is:

- **Misses.** The allocator's own L1 misses are about 180 a request on the store, against about
  5,200 calls: one call in thirty misses on a slab word, a magazine slot or a table, mostly to L2
  (about 15 cycles), which alone adds about half a cycle a call on average. The profile's op
  samples inside the pop fall on the slab word's load (26%) and the slot's (37%), which is where
  misses show. In `pair_dyn` everything is in L1.
- **Mixed sizes.** Each class has its own slab word line and magazine, so calls of many classes
  touch more lines and lose the forwarding one class gets from the previous call: `pair_mix`
  costs 4.5 a call against `pair_dyn`'s 3.7, all in L1.
- **The CPU number's latency.** In a loop the next call's two loads for the CPU number overlap
  the last call; a lone call waits about 10 cycles (executable) or 14 (through the GOT) for its
  slab word whenever the caller needs the pointer soon, as an allocation followed by writes to
  the block does.
- **Front end.** 1,820 inlined copies of the routines in `rpc-server`, about 21 instructions each
  with a run-time size, spread over the program's code; a copy that misses the op cache or ends
  a line with a short tail pays a cycle or more (`zen5-roofline.md`, section 15).

What the Rust around the routines can still do is small: the lookup is now 6 to 8 instructions
and its latency is hidden. The CPU-number chain is inside the routines (a change to the Lean
routines, for example reading the rseq area's address once per function, would be needed to
share it between calls), and the misses are the layout's: hot words of different classes are on
different lines (one 32-byte chunk per class, two classes to a line). Those are where the
remaining cycles are, not in the lookup.

## Reproducing

```
cd bench/roofline/rust-alloc
for p in 0 16 32 48; do RA_PAD=$p ../../../scripts/heavy cargo build -q --profile lto \
  --features asm_exe --target-dir target/asm_exe-p$p; done
RF_SINGLE_ONLY=1 ../../../scripts/measure target/asm_exe-p0/lto/roofline-rust-alloc label   # each
```

`data/fast-lookup-2026-10-22.tar.gz` holds the raw outputs; the scratch variants as patches
against main's crate (`v1` the `lea` alone, `v1b` one static with 32-bit masks, `v1u` the
change as merged, `v2` and `v3` the two masks the routine loads, `v0m` and `v1c` the probes);
and the scripts that built and ran them: `build-pads.sh` and `ra-pads.sh` (the roofline sweep,
combined with `agg.py`), `samecode.sh` (functions compared instruction by instruction),
`build-servers.sh`, `store-sweep.sh` and `small-jobs.sh` (hand builds of main and the change
listed in a local `allocators.toml` as `main@padNN` and `br@padNN`, as `pad-orders.sh` does,
then `probes/spawn-gap/padsweep.py main` on the results).
