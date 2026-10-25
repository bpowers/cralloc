#!/usr/bin/env bash
# Builds the tree at COMMIT, edited by this probe's scripts, as the bench build NAME of this
# checkout (docs/research/large-churn.md). As grown-slots/build.sh: the tree is extracted
# under target/ of this checkout (/tmp is memory), edited, built and removed.
#
#   scripts/heavy bench/probes/large-churn/build.sh NAME COMMIT FEATURE [EDIT.py...] [-- PART...]
#
# FEATURE is the bench-alloc feature (asmalloc, asmalloc-memstat). PARTs (default: workload):
#   workload  the drift workload binary, into bench/target/NAME/release/workload
#   bench     the bench workspace's workloads and RPC servers, into bench/target/NAME/release
#   PROGRAM   an app of bench/apps (datafusion, meilisearch, orders), as `benchrun build-apps`
#             builds it, sharing the main checkout's build directory for the program's crates,
#             into bench/target/NAME/apps/PROGRAM/release/PROGRAM
set -euo pipefail
name="$1" commit="$2" feature="$3"
shift 3
edits=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do
  edits+=("$1")
  shift
done
[ "${1:-}" = "--" ] && shift
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(workload)
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/large-churn-tree-$name"
out="$root/bench/target/$name"
rm -rf "$tree"
mkdir -p "$tree" "$out"
git -C "$root" archive "$commit" | tar -x -C "$tree"
# The probe's own files come from this checkout, whatever the commit.
mkdir -p "$tree/bench/probes/large-churn"
cp "$here"/*.py "$here"/*.rs "$tree/bench/probes/large-churn/"
built="$(git -C "$root" rev-parse "$commit") $feature"
for e in "${edits[@]}"; do
  python3 -I "$tree/bench/probes/large-churn/$(basename "$e")" "$tree"
  built="$built, $(basename "$e") $(sha256sum "$here/$(basename "$e")" | cut -c1-12)"
done
built="$built, lc.rs $(sha256sum "$here/lc.rs" | cut -c1-12)"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
for part in "${parts[@]}"; do
  case "$part" in
    workload)
      (cd "$tree/bench" && cargo build --release -p bench-workloads --features "bench-alloc/$feature" --target-dir "$out")
      ;;
    bench)
      (cd "$tree/bench" && cargo build --release -p bench-workloads -p bench-rpc --features "bench-alloc/$feature" \
        --target-dir "$out")
      ;;
    preload)
      (cd "$tree" && cargo build --release -p asmalloc-preload --target-dir "$out/preload")
      cp "$out/preload/release/libasmalloc_preload.so" "$out/libasmalloc_preload.so"
      ;;
    *)
      target="$out/apps/$part"
      mkdir -p "$target"
      (cd "$tree/bench/apps/$part" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/$part" \
        cargo build --release --locked --features "$feature" --target-dir "$target")
      ;;
  esac
done
echo "$built [${parts[*]}]" > "$out/BUILT-FROM"
rm -rf "$tree"
echo "$name: $(cat "$out/BUILT-FROM")"
