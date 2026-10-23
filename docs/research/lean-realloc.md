# A lean path for realloc of small blocks

2026-10-22, branch `lean-realloc`. Measured three times against a moving main: `55848b5`
(main `2754021` plus the `regrow` workload) against `77055c7` (the lean path) for the tables
below; again with main `1eea9af` merged into both ("Against later mains"); and the per-call
counts once more with main `2d208d4`, which has the shorter class lookup (`fast-lookup.md`),
merged in. Why: `server-profile.md` found
`realloc` of small blocks the largest allocator cost left on the servers, 2.5% of the RPC jobs'
cycles, every call a `Vec`, `String` or buffer moving one class up.

## Summary

- **About 230 fewer instructions a call.** A `Vec<u8>` growing from 128 to 256 bytes costs
  130 instructions a `realloc` (the call, the copy and the `Vec`'s growth code) against 360,
  and 26 cycles against 62. The allocator's own part of a moving call is now 85 instructions,
  26 of them the pop and push routines; `realloc` was 12 KiB of code and is 388 bytes.
- **With the shorter class lookup merged, 126 instructions against 354.** The lean path
  takes the new lookup as it is, one call a size.
- **The servers gain 1.2 to 3.1%, at every placement** (1.9 to 2.1% on the RPC jobs against
  `1eea9af`, below). Averaged over five code placements:
  gRPC h2 −2.5%, Connect h2 −3.1%, rpc log c64 −2.1%, the order service −1.2%. Lean is
  faster than main at every pad of every one of these jobs. Instructions a request fall by
  12,200 on the RPC jobs (4.1%, about 222 per `realloc` at 55 a request) and by 1,560 on the
  order service (8 a request).
- **Nothing else moved.** `json 32Mi` and the large `vecgrow` jobs are level within their
  placement noise; their instructions change by 0.2% or less, except the grow-and-free jobs,
  whose small early steps now take the lean path.
- **C's `realloc` gains little**: 345 instructions a call against 375. Its old class comes from
  the address, and the validated lookup (`process::find`, about 140 instructions) is most of
  what is left. A cheaper one is a behaviour question, below.

## What changed

`heap/resize.rs`. Every resize tries the lean path first: the old class from the caller's
layout (for C, from the address) and the new one from the new size, each with the inlined
lookup the fast path uses, once. Same class: the block stays. Otherwise: pop a block of the new
class, copy, push the old block. A pop that misses has changed nothing and falls to the
general resize (`large.rs`, now `#[cold]` and out of line), which decides exactly as before; a
push that misses ends in the free's slow path, as any free's does. `GlobalAlloc::realloc`,
`Allocator::grow`, `grow_zeroed` and `shrink`, and C's `realloc` share it.

Before, `realloc` was the general resize inlined whole: `Class::of_size` five times (the old
class three times, the new one twice), the large-path decisions, then `alloc_grant` and
`dealloc`, which each looked their class up again.

## Instructions and cycles a call

`workload regrow` (`bench/workloads/src/single.rs`) runs a growing `Vec` or `String` and a
control that reserves the final capacity, in alternating phases, and reports the difference per
`realloc`: the call, the copy and the collection's growth code, which are the same in both
builds. `bench/probes/lean-realloc/regrow.py 3 ...` runs the builds interleaved, three times,
under `scripts/measure`, on CPU 2; medians. C is the glibc build of the workload with each
build's preload library, so Rust's `System` allocator calls C's `realloc`.

| pattern | main instr | lean instr | main cycles | lean cycles | C main instr | C lean instr | glibc instr |
|---|---:|---:|---:|---:|---:|---:|---:|
| `Vec` 128 to 256 bytes | 360 | 130 | 62 | 26 | 375 | 345 | 409 |
| `Vec` 32 to 64 | 311 | 130 | 48 | 23 | 340 | 318 | 641 |
| `Vec` 1 KiB to 2 KiB | 422 | 192 | 70 to 76 | 43 | 440 | 410 | 193 |
| `String` 8 bytes to 1 KiB, per step | 367 | 168 | 67 | 45 | 395 | 368 | 324 |

The disassembly of the lean `__rust_realloc` (388 bytes, the general path a tail jump away):
a moving call runs 85 instructions besides `memcpy`: 16 saving and restoring registers, 15
for the two lookups and the compare, 26 in the pop and push routines, about 20 moving values
between registers around the routines' fixed operands and the call, and the rest branches.
One more cut is in the count: the new `Layout` is built unchecked, since `realloc`'s contract
already bounds its size (HEAP-06); checking it cost 8 instructions.

## The servers, over placement

These jobs move by 2.5 to 4% with code placement alone (`grpc-placement.md`), so each build was
made at bench-alloc's pads 0, 16, 32, 48 and 64 (`bench/probes/lean-realloc/build.sh`), and
every session ran main and lean at the same pads, interleaved, as local `allocators.toml`
entries (`entries.sh`), under `scripts/measure --both-l3` (`sweep.sh`: two sessions a round,
pads 00 to 32 and 48 to 64, a minute apart; that froze teammates' builds for too long, and
`sweep.sh` now waits as long as each session ran, five minutes at least). Medians per pad over the rounds, then the mean
over pads (`probes/spawn-gap/padsweep.py`). Server cycles a request at the 70% rate (k), for
the kernel jobs the job's own metric.

| job | rounds | main mean (range over pads) | lean mean (range) | lean, mean over pads | lean, pad00 alone |
|---|---:|---|---|---:|---:|
| connect log grpc h2 | 3 | 130.1 (127.4 to 131.9) | 126.9 (124.3 to 129.9) | −2.5% | −2.4% |
| connect log h2 | 3 | 128.8 (126.4 to 131.6) | 124.8 (123.7 to 127.5) | −3.1% | −2.6% |
| orders axum | 2 | 68.7 (68.2 to 69.8) | 67.9 (67.2 to 68.4) | −1.2% | −1.4% |
| rpc log c64 | 2 | 123.7 (123.0 to 124.2) | 121.1 (120.3 to 121.8) | −2.1% | −2.5% |
| json 32Mi, cycles/byte | 2 | 16.34 | 16.37 | +0.2% | +3.0% |
| vecgrow 1x256Mi, cycles/element | 2 | 2.221 | 2.194 | −1.2% | −0.2% |
| vecgrow 64x4Mi | 2 | 4.888 | 4.891 | +0.1% | −1.0% |
| vecgrow 1x4Mi grow-and-free | 2 | 0.4750 | 0.4736 | −0.3% | 0.0% |
| vecgrow 1x1Mi grow-and-free | 2 | 0.2967 | 0.2915 | −1.8% | −0.7% |

Instructions a request at 70% (medians over all runs): gRPC h2 299.1k to 287.0k, Connect h2
295.7k to 283.1k, orders 107.6k to 106.0k. Instructions over the run, lean against main: rpc
log c64 −4.0%, json −0.2%, vecgrow 1x256Mi and 64x4Mi −0.2%, 1x4Mi grow-and-free −1.1%,
1x1Mi grow-and-free −3.0%.

The profile estimated the RPC jobs would take back about 1.7%; they took back 2.5 to 3.1%.
The instructions saved per call (about 222) are close to all of what the profile's 240
"besides the copy" counted, not two thirds of it, and the cycles follow. On json, whose pad00
alone reads +3.0%, the mean over pads is level and the range over pads (3.5 to 4%) is wider
than the difference: a single-build comparison there would have been a placement reading.

## Against later mains

The same sweep with main `1eea9af` (the purge moved to the background thread) and the branch
merged with it: gRPC h2 three rounds, Connect h2 two, the order service two to three per
pad, the kernel jobs one. Mean over pads, lean against main; instructions a request (or a
run) in brackets.

| job | lean, mean over pads | pad00 alone | instructions |
|---|---:|---:|---:|
| connect log grpc h2 | −2.1% | −2.2% | −4.1% |
| connect log h2 | −1.9% | −2.5% | −4.1% |
| orders axum | −0.7% | +0.8% | −1.4% |
| rpc log c64 | −0.7% | −1.4% | −3.1% |
| json 32Mi | −0.6% | +1.2% | −0.2% |
| vecgrow 1x256Mi | +1.9% | 0.0% | 0.0% |
| vecgrow 64x4Mi | +0.5% | +0.8% | +0.1% |

The RPC and order-service gains hold, smaller than in the first sweep but in the same
direction at nearly every pad, with the instructions saved unchanged. `vecgrow 1x256Mi` runs
in two modes in both builds (2.24 or 2.38 to 2.40 cycles an element, the grow loop's
placement mode, `memory.md`); in this one round lean landed in the slow mode at two pads
where main did not, with instructions level, and the first sweep had lean 1.2% ahead. It
makes no small `realloc` worth counting.

Per call with main `2d208d4` merged (the shorter class lookup), as the first table: `Vec`
128 to 256 bytes 354 instructions and 66 cycles on main, 126 and 22 on the branch; 32 to 64
305 and 126; 1 KiB to 2 KiB 416 and 188; `String` per step 362 and 164. C: 375 and 342.

## C's realloc

C's lean path is the same pop, copy and push, with the new class from the size by C's
16-byte rule through the same lookup. What it keeps is `process::find`, which validates that
the pointer starts a slot (`Back::lookup`: the region table, the class's layout, a division)
and aborts on anything else, as C's `realloc` always has. That lookup is about 140
instructions, most of the 345. The generated C `free` instead reads the class byte from the
region table's copy beside the slabs (`c::mirror`) without validating the slot; doing the same
in `realloc` would bring the call to an estimated 215 instructions, but a pointer into the
middle of a small block would then be moved and pushed, as `free` pushes one, instead of
aborting. That is a change of behaviour on invalid input, left to a decision. A cheaper
validated small-slot lookup in the back end is the other way.

## Reproducing

```
git archive 55848b5 | tar -x -C target/src-main     # and 77055c7 into target/src-lean
scripts/heavy bench/probes/lean-realloc/build.sh main=target/src-main lean=target/src-lean
bench/probes/lean-realloc/entries.sh main lean >> bench/allocators.toml   # take out after
bench/probes/lean-realloc/sweep.sh OUT 3 servers "=connect log grpc h2"   # waits between sessions
bench/probes/spawn-gap/padsweep.py main OUT/connect-log-grpc-h2-*.jsonl
scripts/measure bench/probes/lean-realloc/regrow.py 3 main=BIN lean=BIN c-main=GLIBC_BIN+LIB ...
```

`benchrun` also needs the load generator (`bench/target/loadgen`) and, for `rpc log c64`, the
glibc build of `rpc-client` (`bench/target/glibc`).
