#!/usr/bin/env bash
# Builds the tree at COMMIT, edited by this probe's scripts, as the bench build NAME of this
# checkout (docs/research/large-heap-constants.md). The tree is extracted under target/ of this
# checkout (/tmp is memory), edited, built and removed.
#
#   scripts/heavy bench/probes/large-heap-constants/build.sh NAME COMMIT FEATURE [EDIT.py...] [-- PART...]
#
# FEATURE is the bench-alloc feature (asmalloc, asmalloc-memstat, live-bytes, je, ...). PARTs
# (default: workload):
#   workload  the drift workload binary, into bench/target/NAME/release/workload
#   PROGRAM   an app of bench/apps (orders, miniredis), as `benchrun build-apps` builds it,
#             sharing the main checkout's build directory for the program's crates, into
#             bench/target/NAME/apps/PROGRAM/release/PROGRAM
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
tree="$root/target/lhc-tree-$name"
out="$root/bench/target/$name"
rm -rf "$tree"
mkdir -p "$tree" "$out"
git -C "$root" archive "$commit" | tar -x -C "$tree"
built="$(git -C "$root" rev-parse "$commit") $feature"
for e in "${edits[@]}"; do
  python3 -I "$here/$(basename "$e")" "$tree"
  built="$built, $(basename "$e") $(sha256sum "$here/$(basename "$e")" | cut -c1-12)"
done
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
features=()
[ "$feature" = glibc ] || features=(--features "$feature")
for part in "${parts[@]}"; do
  case "$part" in
    workload)
      wf=()
      [ "$feature" = glibc ] || wf=(--features "bench-alloc/$feature")
      (cd "$tree/bench" && cargo build --release -p bench-workloads "${wf[@]}" --target-dir "$out")
      ;;
    *)
      target="$out/apps/$part"
      mkdir -p "$target"
      (cd "$tree/bench/apps/$part" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/$part" \
        cargo build --release --locked "${features[@]}" --target-dir "$target")
      ;;
  esac
done
echo "$built [${parts[*]}]" > "$out/BUILT-FROM"
rm -rf "$tree"
echo "$name: $(cat "$out/BUILT-FROM")"
