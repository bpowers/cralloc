#!/usr/bin/env bash
# usage: build.sh NAME=SOURCE...
# Builds each source tree (a `git archive` of a commit, extracted) at bench-alloc's layout pads
# 0, 16, 32, 48 and 64 (BENCH_LAYOUT_PAD, as `benchrun layout --layouts pads` does): the
# workloads and rpc-server into bench/target/lr-NAME-pNN, and the order service into
# bench/target/lr-NAME-pNN/apps/orders, so that local allocators.toml entries
# `NAME@padNN` with `build = "lr-NAME-pNN"` (`entries.sh` prints them) run every build of every
# source in one benchrun session, interleaved, and `probes/spawn-gap/padsweep.py` combines the
# sessions. Run it from the repository root under scripts/heavy.
set -euo pipefail
W=$(cd "$(dirname "$0")/../../.." && pwd)
common="$(git -C "$W" rev-parse --path-format=absolute --git-common-dir)/.."
export BENCH_BASELINES="${BENCH_BASELINES:-$common/bench/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
for spec in "$@"; do
  name=${spec%%=*}
  src=$(cd "${spec#*=}" && pwd)
  for pad in 00 16 32 48 64; do
    out="$W/bench/target/lr-$name-p$pad"
    (cd "$src/bench" && BENCH_LAYOUT_PAD=$((10#$pad)) cargo build --release -p bench-workloads \
       -p bench-rpc --features bench-alloc/asmalloc --target-dir "$out")
    (cd "$src/bench/apps/orders" && BENCH_LAYOUT_PAD=$((10#$pad)) \
       CARGO_BUILD_BUILD_DIR="$W/target/lr-appbuild-$name" \
       cargo build --release --locked --features asmalloc --target-dir "$out/apps/orders")
  done
done
