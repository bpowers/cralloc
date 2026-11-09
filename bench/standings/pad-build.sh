#!/usr/bin/env bash
# usage: pad-build.sh PART [ALLOC...]
#   PART: workloads, or a program (orders, sozu, miniredis, router, qdrant, surrealdb, datafusion,
#   ruff)
#   ALLOC: cra jem tcm mim gli (default all five), or cro, cralloc out of line; PADS: the pads, default
#   "00 16 32 48 64", or
#   on aarch64, where a line can be 128 bytes, "000 032 064 096 128"
# Builds this checkout's servers at bench-alloc's layout pads
# (BENCH_LAYOUT_PAD, as `benchrun layout --layouts pads` does) for each allocator: the workloads
# and rpc-server into bench/target/ALLOC-pNN, a program into bench/target/ALLOC-pNN/apps/PROGRAM.
# Local allocators.toml entries `NAME@padNN` with `build = "ALLOC-pNN"` (`pad-entries.sh`
# prints them) then run every placement of every allocator in one benchrun session,
# interleaved, and `padsweep.py` or `servers.py` combines the sessions.
# `benchrun layout` does the same for rpc-server but builds no app, and one mechanism for both
# keeps the sessions alike. Run under scripts/heavy. The parts share no build directory, so
# they can build at once: a program's fat-LTO link runs on one core.
#
# The build names have one length, so every binary's path has one length: the path's length
# moved glibc's tokio jobs by up to 18% (`docs/research/measuring.md`, "glibc's spawn result moved
# with the length of the binary's path").
set -euo pipefail
W=$(cd "$(dirname "$0")/../.." && pwd)
common="$(git -C "$W" rev-parse --path-format=absolute --git-common-dir)/.."
data="${BENCH_APPS_DATA:-$common/bench/apps/target}"
export BENCH_BASELINES="${BENCH_BASELINES:-$common/bench/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
part="${1:?usage: pad-build.sh PART [ALLOC...]}"
shift
allocs=("$@")
[ ${#allocs[@]} -gt 0 ] || allocs=(cra jem tcm mim gli)
feature() {
  case "$1" in
    cra) echo cralloc ;; cro) echo cralloc-outline ;; jem) echo je ;; tcm) echo tc ;; mim) echo mi ;; gli) echo "" ;;
    *) echo "pad-build: unknown allocator $1" >&2; exit 2 ;;
  esac
}
case "$(uname -m)" in
  aarch64) default_pads="000 032 064 096 128" ;;
  *) default_pads="00 16 32 48 64" ;;
esac
for a in "${allocs[@]}"; do
  feat=$(feature "$a")
  for pad in ${PADS:-$default_pads}; do
    out="$W/bench/target/$a-p$pad"
    case "$part" in
      workloads)
        # One build directory for every variant, so the dependencies compile once per feature
        # and a pad recompiles only bench-alloc and what depends on it.
        (cd "$W/bench" && BENCH_LAYOUT_PAD=$((10#$pad)) CARGO_BUILD_BUILD_DIR="$W/bench/target/pad-build-ws" \
           cargo build --release -p bench-workloads -p bench-rpc ${feat:+--features bench-alloc/$feat} \
           --target-dir "$out") ;;
      orders | sozu | miniredis | router | qdrant | surrealdb | datafusion | ruff | meilisearch)
        (cd "$W/bench/apps/$part" && PATH="$data/inputs/$part/bin:$PATH" BENCH_LAYOUT_PAD=$((10#$pad)) \
           CARGO_BUILD_BUILD_DIR="$data/build/$part" \
           cargo build --release --locked ${feat:+--features $feat} --target-dir "$out/apps/$part") ;;
      *) echo "pad-build: unknown part $part" >&2; exit 2 ;;
    esac
  done
done
