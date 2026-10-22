# Where code lands moves the gRPC server by 4%

2026-10-22, branch `sibling-stride` (main `7a82ea4` and the branch's builds). Why
`connect log grpc h2` ran from 3.9% below to 1.5% above main across builds of one allocator
rule that differ only in code, with instructions per request level (`spawn-gap.md`, "The rule,
built").

## Summary

- **The cause is the service's protobuf decoder, not the allocator's code.** The slow builds
  starve the front end: the dispatch slots the front end leaves empty account for the cycles,
  and the extra op-cache misses, or at other placements the extra ops delivered by the
  decoder, fall in buffa's generated view decoder (`LogRequestView::decode_view_with_ctx`,
  `borrow_str`, `decode_varint`): over 90% of the difference. The allocator's code comes
  earlier in the binary, so any change to its size moves the decoder.
- **Forced, not correlated.** Builds of main identical except for a padding function of N
  bytes ahead of everything else move all later code by exactly N. N = 0, 16, 32 and 48 ran
  126.9, 127.1, 130.5 and 132.2k cycles a request, op-cache misses rising with them; N = 64 ran
  132.4k, with misses back down and 5% more ops from the decoder instead. The property is the
  decoder's address, with a period longer than a line: no single alignment decides it.
- **Nothing to pin in our code.** The sensitive code is not ours, and 64-byte function
  alignment would only hide the sub-line shifts (the 64-byte shift is slow too).
- **How to read the job.** One build's number stands for any point in a range of about 4%
  (126.9 to 132.4k here), and the same binary drifts about 2% between sessions. A comparison
  of two builds on this job needs several placements of each (below), or one binary that
  switches the behaviour at run time.
- **The headers.** With the stride switched at run time in one binary, separating tokio's
  run-queue headers does not move this job: 129.3k off, 129.0 with 16 siblings, 130.1 with 64,
  129.3 with whole magazines (`spawn-gap.md`). The 3.6% earlier attributed to them was
  measured across builds, through this noise.

## Which counters move

`benchrun run` with raw events per server segment (`BENCH_PROC_EVENTS`, the runner hunks of
`bench/probes/spawn-gap/diagnostics.patch`), s70, three rounds, medians, per request. Two
builds of one rule: `fwrd` (slow) and `ivpf` (fast), and main.

| Counter | fwrd | ivpf | main |
|---|---:|---:|---:|
| cycles (session 1 / 2) | 133.8k / 134.1k | 129.2k / 128.3k | 130.9k / 132.5k |
| dispatch slots the front end left empty (0x1a0, umask 1) | 398k | 360k | 370k |
| dispatch slots stalled by the back end (umask 0x1e) | 76k | 81k | 82k |
| branches mispredicted | 60 | 40 | 40 |
| op-cache misses (0x28f, umask 4) | 9.92k | 8.15k | 8.29k |
| ops from the decoder (0xaa, umask 1) | 29.9k | 29.8k | 30.2k |
| instruction-cache misses (0x18e, umask 0x18) | 14.0k | 14.7k | 15.1k |
| instructions | 299k | 298k | 299k |

38k empty slots at eight a cycle are 4.8k cycles, the whole difference between fwrd and ivpf.
Mispredicts, instruction-cache misses and back-end stalls do not account for it. Main is
slower than ivpf with the same op-cache misses: other front-end losses than the op cache's
also move with placement (the 64-byte shift below shows one).

A by-hand driver (`grpc-pstat.py`, `perf stat -p` on the server at the same rate) counted the
same instructions but 28% fewer cycles than benchrun, and did not separate the builds. Its
conditions differ in a way not found; the counts here are benchrun's own.

## Where

`perf record -a -C 0-3` over the s70 segment of one benchrun run each (`grpc-record.sh`),
op-cache misses sampled every 20,000, server threads only (`symdiff.py`): 221k samples in fwrd
against 195k in ivpf.

| Function | fwrd | ivpf | difference |
|---|---:|---:|---:|
| `LogRequestView::decode_view_with_ctx` | 36,004 | 23,053 | +12,951 |
| `buffa::types::borrow_str` | 15,017 | 8,249 | +6,768 |
| `buffa::encoding::decode_varint` | 10,090 | 5,226 | +4,864 |
| `LogSourceView::merge_into_view` | 7,611 | 5,406 | +2,205 |
| asmalloc's `GlobalAlloc::realloc` | 4,558 | 2,480 | +2,078 |
| `LogIngestServiceServer` dispatch | 2,181 | 631 | +1,550 |
| h2's connection | 6,962 | 8,205 | -1,243 |

Inside the decoder the misses spread over about 2 KiB of its 6 KiB body (`hotlines.py`), not
one small loop that straddles a line. The decoder starts at line offset 0 in ivpf and main, 16
in the commit, 48 in fwrd; functions are 16-byte aligned. (asmalloc's out-of-line `realloc` is
12,428 bytes, which is why its size moves everything after it.)

## Forcing the placement

A copy of main whose `bench-alloc` holds a naked function of N + 16 bytes that never runs, kept
by a used pointer (`BENCH_PAD`, a diagnostic not committed); the linker places it ahead of the
allocator and the service, so every later function moves by exactly N (checked with `nm`,
`symaddr.py`). Same session, three rounds, medians, per request:

| Pad | cycles | op-cache misses | empty front-end slots | ops from the decoder |
|---:|---:|---:|---:|---:|
| 0 | 126.9k | 8.25k | 353k | 29.7k |
| 16 | 127.1k | 8.82k | 355k | 29.6k |
| 32 | 130.5k | 9.18k | 383k | 29.9k |
| 48 | 132.2k | 9.67k | 389k | 29.9k |

A second session: pad 0 129.2k, pad 48 131.1k (misses 8.29k and 9.74k), pad 64 132.4k, with
misses at 8.48k but ops from the decoder up from 29.9k to 31.4k and 402k empty slots; the
decoder-delivered ops it adds fall in the same functions (`borrow_str` +8.4k samples,
`decode_view_with_ctx` +6.2k, `decode_varint` +1.6k). So a whole-line shift is as bad as the
worst sub-line one by another route: the property is where the decoder's hot footprint lands
in the front end's structures, with a period longer than 64 bytes, not a 64-byte phase.

`benchrun layout` (`bench/runner/src/layout.rs`) moves code with `-align-loops=64` or by
shuffling the linker's section order; both move every function at once, the allocator's
included, so they show that a job is placement-sensitive but cannot hold everything else
still. The pad does.

## How to read connect log grpc h2

- A single build's figure stands for any point in a range of about 4% (126.9 to 132.4k across
  five placements of one source), and the same binary drifted 2.3k between two sessions.
- To compare two builds, sweep placement on each: pads 0, 16, 32, 48 and 64 (or more), three
  rounds each, and compare the means over placements; or, for a change of behaviour, put both
  behaviours in one binary and switch at run time, as the stride study did.
- Proposal for `benchrun layout`: a `padN` variant family that links a padding function of N
  bytes ahead of the program, so that placement can be moved by a known amount with nothing
  else changed. Not built; the diagnostic here is the recipe.
- The RPC log jobs run the same server binary and moved 1 to 3% across builds in
  `spawn-gap.md` while a run-time switch showed nothing; treat them the same way until they are
  swept.

## Method

Builds from `git archive` (`bench/probes/spawn-gap/ab-build.sh`) and the padded ones as above,
all with `bench/` sources identical, listed as local `build = ...` entries in
`bench/allocators.toml`; `benchrun run --suite servers --only "=connect log grpc h2"` under
`scripts/measure --both-l3`, builds interleaved. Event encodings from `perf list --details` on
Zen 5; five counters at most per session (the NMI watchdog holds one). Raw results:
`docs/research/data/grpc-placement-2026-10-22.jsonl.gz`.
