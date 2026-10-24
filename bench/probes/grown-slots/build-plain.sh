#!/usr/bin/env bash
# Builds the allocator at COMMIT as it is, with no switch, for the product build's comparison
# against main in docs/research/grown-slots.md ("The rule built"). As build.sh, without the
# edit: the commit's tree is extracted under target/ of this checkout, built, and removed.
#
#   scripts/heavy bench/probes/grown-slots/build-plain.sh NAME COMMIT [PART...]
#
# PARTs as build.sh takes them: bench, preload, trail, or an app of bench/apps.
set -euo pipefail
name="$1" commit="$2"
shift 2
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(bench preload)
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/grown-slots-$name"
out="$root/bench/target/$name"
rm -rf "$tree"
mkdir -p "$tree" "$out"
git -C "$root" archive "$commit" | tar -x -C "$tree"
# The probe's own files come from this checkout, whatever the commit.
mkdir -p "$tree/bench/probes/grown-slots"
cp -r "$here"/trail "$tree/bench/probes/grown-slots/"
git -C "$root" rev-parse "$commit" > "$out/BUILT-FROM"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
for part in "${parts[@]}"; do
  case "$part" in
    bench)
      (cd "$tree/bench" && cargo build --release -p bench-workloads -p bench-rpc \
        --features bench-alloc/asmalloc --target-dir "$out")
      ;;
    preload)
      (cd "$tree" && cargo build --release -p asmalloc-preload --target-dir "$out/preload")
      cp "$out/preload/release/libasmalloc_preload.so" "$out/libasmalloc_preload.so"
      rm -rf "$out/preload"
      ;;
    trail)
      (cd "$tree/bench/probes/grown-slots/trail" && cargo build --release --target-dir "$out/trail-build")
      cp "$out/trail-build/release/trail" "$out/trail"
      rm -rf "$out/trail-build"
      ;;
    *)
      target="$out/apps/$part"
      mkdir -p "$target"
      (cd "$tree/bench/apps/$part" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/$part" \
        cargo build --release --locked --features asmalloc --target-dir "$target")
      ;;
  esac
done
rm -rf "$tree"
echo "$name: $(cat "$out/BUILT-FROM") [${parts[*]}]"
