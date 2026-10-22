#!/usr/bin/env bash
# Builds the order service with bench-alloc's layout pad (BENCH_LAYOUT_PAD, as `benchrun layout
# --layouts pads` does for the workloads, which it does not do for the apps) for asmalloc and
# jemalloc at pads 0, 16, 32, 48 and 64, into bench/target/{asm,jem}-pNN/apps/orders. Run from
# bench/ under scripts/heavy. benchrun runs them through local allocators.toml entries named
# `asmalloc@padNN` and `jemalloc@padNN` with `build = "asm-pNN"` / `"jem-pNN"`, so that
# probes/spawn-gap/padsweep.py can combine the sessions.
set -euo pipefail
bench="$(cd "$(dirname "$0")/../.." && pwd)"
common="$(git -C "$bench" rev-parse --path-format=absolute --git-common-dir)/.."
data="${BENCH_APPS_DATA:-$common/bench/apps/target}"
export BENCH_BASELINES="${BENCH_BASELINES:-$common/bench/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
for pad in 00 16 32 48 64; do
  for pair in asm:asmalloc jem:je; do
    name="${pair%%:*}-p$pad" feature="${pair#*:}"
    (cd "$bench/apps/orders" && BENCH_LAYOUT_PAD=$((10#$pad)) CARGO_BUILD_BUILD_DIR="$data/build/orders" \
       cargo build --release --locked --features "$feature" --target-dir "$bench/target/$name/apps/orders")
  done
done
