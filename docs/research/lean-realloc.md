# A lean path for realloc of small blocks

2026-10-22, branch `lean-realloc`. Why: `server-profile.md` found `realloc` of small blocks
the largest allocator cost left on the servers, 2.5% of the RPC jobs' cycles, every call a
`Vec`, `String` or buffer moving one class up. Main moved while this was measured, so each
comparison names its main: the servers over placement against `1eea9af` (first) and
`2754021`, the per-call counts against `437adf1`, `2d208d4` and `2754021`.

## Summary

- **About 230 fewer instructions a call.** Against main `437adf1`, a `Vec<u8>` growing from
  128 to 256 bytes costs 126 instructions a `realloc` (the call, the copy and the `Vec`'s
  growth code) against 354, and 23 cycles against 63; a `String` grown to 1 KiB, 164 against
  362 a step. `realloc` was 12 KiB of code and is under 400 bytes.
- **About 4% fewer instructions a request on the RPC jobs**, 12,200 at 55 reallocs a
  request (about 222 a call), and 1.4% on the order service. This holds against both mains.
- **About 2% faster on the two HTTP/2 RPC jobs**, averaged over five code placements, against
  main `1eea9af`: gRPC h2 −2.1% (three runs per placement) and Connect h2 −1.9% (two). These
  are firm. The first sweep, against `2754021`, read −2.5% and −3.1%.
- **Not established**: the order service, −0.7% over placements (two to three runs; lean is
  slower at pad00); rpc log c64, −0.7% (one run per placement, level or slower at three of
  five). Against `2754021` they read −1.2% and −2.1%. That the other jobs did not move is not
  established either: `json 32Mi` is level, but `vecgrow 1x256Mi` runs in two modes in both
  builds and one round read +1.9% with its instructions level.
- **C's `realloc`: 273 instructions against 375.** The old class now comes from the region
  table and the slot check alone (`Back::small_slot`, about 70 instructions, most of them the
  proved slot arithmetic computing the class size three times), not from the general lookup;
  an interior pointer still aborts.
- **Calls the lean path does not take cost slightly more**, by the extra call to the general
  path: small to large (24 to 48 KiB) 1,866 to 1,924 instructions, large to large (40 to
  80 KiB) 5,751 to 5,786. A small block that grows to the largest small class (16 to
  32 KiB) gains: 1,137 to 908.

## What changed

`heap/resize.rs`. Every resize tries the lean path first: the old class from the caller's
layout (for C, from the address) and the new one from the new size, each with the inlined
lookup the fast path uses, once. Same class: the block stays. Otherwise: pop a block of the new
class, copy, push the old block. A pop that misses has changed nothing and falls to the
general resize (`large.rs`, now `#[cold]` and out of line), which decides exactly as before; a
push that misses ends in the free's slow path, as any free's does. `GlobalAlloc::realloc`,
`Allocator::grow`, `grow_zeroed` and `shrink`, and C's `realloc` share it.

C's `realloc` takes the old class from the address with `Back::small_slot`: the region table
and `Plan::slot_at`, the arithmetic `Back::lookup` uses for a small class, without the class's
host and layout. A pointer that starts no small slot goes to `find`, which finds a large block
or aborts, as before.

Before, `realloc` was the general resize inlined whole: `Class::of_size` five times (the old
class three times, the new one twice), the large-path decisions, then `alloc_grant` and
`dealloc`, which each looked their class up again.

## Instructions and cycles a call

`workload regrow` (`bench/workloads/src/single.rs`) runs a growing `Vec` or `String` and a
control that reserves the final capacity, in alternating phases, and reports the difference per
`realloc`: the call, the copy and the collection's growth code, which are the same in both
builds. `bench/probes/lean-realloc/regrow.py 3 ...` runs the builds interleaved, three times,
under `scripts/measure`, on CPU 2; medians. C is the glibc build of the workload with each
build's preload library, so Rust's `System` allocator calls C's `realloc`. This table is
against main `2754021` and the branch's first version, whose C path still used `find` (see
"C's realloc" for the counts now).

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

The profile estimated the RPC jobs would take back about 1.7%; in this first sweep they took
back 2.5 to 3.1%, and about 2% in the second (below).
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

The HTTP/2 RPC jobs keep about 2%, lean ahead at every pad, with three and two runs per
cell. The order service and rpc log c64 are within their noise here: lean is slower at
pad00 on orders, and rpc log c64, one run per cell, is level or slower at three pads. The
instructions saved are unchanged on all four. `vecgrow 1x256Mi` runs
in two modes in both builds (2.24 or 2.38 to 2.40 cycles an element, the grow loop's
placement mode, `memory.md`); in this one round lean landed in the slow mode at two pads
where main did not, with instructions level, and the first sweep had lean 1.2% ahead. It
makes no small `realloc` worth counting.

Per call with main `2d208d4` merged (the shorter class lookup), as the first table: `Vec`
128 to 256 bytes 354 instructions and 66 cycles on main, 126 and 22 on the branch; 32 to 64
305 and 126; 1 KiB to 2 KiB 416 and 188; `String` per step 362 and 164. C: 375 and 342,
before C took its class from `Back::small_slot`; the same with main `437adf1`.

## C's realloc

The first version kept `process::find` for the old class, a general lookup of about 140
instructions (the class's host and layout, which only large classes need), and saved only 30
instructions a call. The C `free` instead reads the class byte from the region table's copy
beside the slabs (`c::mirror`) without checking that the pointer starts a slot; `realloc`
must not, because it copies from the pointer: an interior pointer would copy up to a class
size from the neighbouring slot before being pushed. So `realloc` now reads the back end's
own region table, which never lags as the copy can, and checks the slot with the plan's
arithmetic. A test compares it with `find` at every 8-byte step around a block of every small
class; the preload test makes `realloc` of an interior pointer abort. Per call against
`437adf1`: `Vec` 128 to 256 bytes 375 to 273 instructions (66 to 54 cycles), 32 to 64 340 to
246, 1 KiB to 2 KiB 440 to 338, `String` per step 395 to 296.

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
