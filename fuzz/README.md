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

cargo-fuzz builds with AddressSanitizer by default. That checks the testkit's own accesses
(it found the model writing into a zero-size block), but it also replaces the C library's
allocation functions with ASan's. ASan's `aligned_alloc` enforces C11's size rule, so under
ASan `model_c` rounds `aligned_alloc` sizes up (see `src/lib.rs`). A custom allocator that
exports its own `malloc` does not mix with ASan; build those with `-s none`.

## Running

Pin to idle cores, bound the time and memory, and run from the repository root:

```sh
cargo +nightly fuzz build
taskset -c 8,9,24,25 fuzz/target/x86_64-unknown-linux-gnu/release/model \
    -max_total_time=60 -rss_limit_mb=2048 fuzz/corpus/model
CRALLOC_FUZZ_MUTANT=scribbler taskset -c 8,9,24,25 \
    fuzz/target/x86_64-unknown-linux-gnu/release/model_c -max_total_time=60 -rss_limit_mb=2048
```

`cargo +nightly fuzz run <target> -- -max_total_time=60` works too. Never leave a fuzzer
running without a time limit on a shared machine.

A failing input panics with the model's report: the violated property, the operation, and,
for a short stream, the whole script. Crashing inputs land in `fuzz/artifacts/<target>/`.
Reproduce and shrink with:

```sh
cargo +nightly fuzz run model fuzz/artifacts/model/crash-<hash>
cargo +nightly fuzz tmin model fuzz/artifacts/model/crash-<hash>
```

The corpus is not checked in; the input decoding is dense, so a fresh fuzzer finds the
interesting paths within seconds. About 5 000 runs a second per core with ASan (2026-10-16).
