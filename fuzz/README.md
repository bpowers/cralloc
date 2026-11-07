# Fuzz targets

Coverage-guided fuzzing of the testkit's model (`crates/cralloc-testkit`) with cargo-fuzz and
libFuzzer. This crate is its own workspace because it needs nightly and `libfuzzer-sys`; the
merge gate never builds it.

Each target reads one byte to pick a profile and treats the rest of the input as the operation
stream, four bytes per decision, so every mutation the fuzzer makes is a mutation of the
sequence of allocations, frees, resizes and touches. A run ends when the input does, and frees
every block still live.

| Target | Interface |
|---|---|
| `model` | `GlobalAlloc` |
| `model_c` | the C functions, through `posix_memalign`, `aligned_alloc`, `calloc` splits and sized frees |
| `model_allocator` | the `Allocator` trait: zero sizes, frees and resizes with any size the block fits, alignment changes |

The allocator is the testkit's `target` module, so pointing the testkit at an allocator points
these targets at it too: cralloc's process heap. Setting `CRALLOC_FUZZ_MUTANT` to a mutant's
name (`overlapping`, `misaligned`, `short`, `no_zero_fill`, `loses_contents`, `double_handout`,
`scribbler`, `premature_reuse`) wraps the allocator in that bug, which shows the fuzzer finds it.

cargo-fuzz builds with AddressSanitizer by default, and all three targets run on cralloc under
it. The targets call cralloc by its Rust paths, `model_c` included, so ASan's `malloc` serves
only the kit's own bookkeeping. ASan knows nothing of cralloc's blocks: it checks the bookkeeping
and the stacks and globals of the instrumented code, and a stray write into a block is the
model's to catch, not ASan's.

## Running

Run from the repository root, through `scripts/heavy` and `scripts/memlimit`, and always with a
time limit:

```sh
scripts/heavy cargo +nightly fuzz build
scripts/heavy scripts/memlimit -m 4G -t 5m -- \
    cargo +nightly fuzz run model -- -max_total_time=60 -rss_limit_mb=2048 -len_control=0
CRALLOC_FUZZ_MUTANT=scribbler scripts/heavy scripts/memlimit -m 4G -t 5m -- \
    cargo +nightly fuzz run model_c -- -max_total_time=60 -rss_limit_mb=2048 -len_control=0
```

Keep `-len_control=0`. A run ends when its input does, and libFuzzer otherwise starts with short
inputs and lengthens them slowly, so a bug that takes many operations to show goes unreached:
without the flag a minute can miss `overlapping` or `scribbler`. With it, a fresh fuzzer finds
most mutants within a few seconds; `double_handout`, which needs the stream to move to another
thread, can take more than a minute.

A failing input panics with the model's report: the violated property, the operation, and,
for a short stream, the whole script. Crashing inputs land in `fuzz/artifacts/<target>/`.
Reproduce and shrink with:

```sh
scripts/heavy scripts/memlimit -m 4G -t 5m -- \
    cargo +nightly fuzz run model fuzz/artifacts/model/crash-<hash>
scripts/heavy scripts/memlimit -m 4G -t 5m -- \
    cargo +nightly fuzz tmin model fuzz/artifacts/model/crash-<hash>
```

An input that found a mutant reproduces only with the same `CRALLOC_FUZZ_MUTANT` set.

The corpus (`fuzz/corpus/<target>/`) is not checked in; later runs start from it.
