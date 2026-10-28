#!/usr/bin/env bash
# Builds the tree at COMMIT, edited by probe scripts, as the bench build NAME of this checkout
# (docs/research/trough-gap.md). The tree is extracted under target/ of this checkout, edited,
# built and removed. Run under scripts/heavy:
#
#   scripts/heavy bench/probes/trough-gap/build.sh NAME COMMIT FEATURE [EDIT.py...] [-- PART...]
#
# FEATURE is the bench-alloc feature (asmalloc, asmalloc-memstat, je, je-stats, ...). An EDIT is
# a path from the repository root or a file of this directory; each runs as
# `python3 -I EDIT TREE` in the order given, with the environment passed on (variant edits read
# it). PARTs (default: workload):
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
tree="$root/target/tg-tree-$name"
out="$root/bench/target/$name"
rm -rf "$tree"
mkdir -p "$tree" "$out"
git -C "$root" archive "$commit" | tar -x -C "$tree"
built="$(git -C "$root" rev-parse "$commit") $feature"
for e in "${edits[@]}"; do
  if [ -f "$root/$e" ]; then path="$root/$e"; else path="$here/$e"; fi
  python3 -I "$path" "$tree"
  built="$built, $e $(sha256sum "$path" | cut -c1-12)"
  if [ "$(basename "$path")" = tg_edit.py ]; then
    built="$built (memstat.rs $(sha256sum "$here/memstat.rs" | cut -c1-12), je.rs $(sha256sum "$here/je.rs" | cut -c1-12))"
  fi
done
for v in $(env | grep -o '^TG_[A-Z_]*' || true); do built="$built, $v=${!v}"; done
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
