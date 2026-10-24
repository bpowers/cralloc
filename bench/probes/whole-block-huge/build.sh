#!/usr/bin/env bash
# Builds the allocator at COMMIT with the whole-block-huge switch (wbh_edit.py) for
# docs/research/whole-block-huge.md, as the bench build NAME of this checkout. As
# grown-slots/build.sh: the commit's tree is extracted under target/ of this checkout (/tmp is
# memory), edited, built, and removed.
#
#   scripts/heavy bench/probes/whole-block-huge/build.sh NAME COMMIT [PART...]
#
# PARTs (default: bench preload):
#   bench       the bench workspace's workloads and RPC servers with bench-alloc/asmalloc,
#               into bench/target/NAME/release (a `build = "NAME"` entry of allocators.toml)
#   preload     the preload library, into bench/target/NAME/libasmalloc_preload.so
#   sparse      the sparse-tenant probe (sparse/), into bench/target/NAME/sparse
#   runner      benchrun with runner_edit.py (raw events in the server counters), into
#               bench/target/NAME/benchrun; it reads this checkout's bench/ (WBH_BENCH_DIR)
#   PROGRAM     an app of bench/apps (datafusion, meilisearch, orders), as `benchrun
#               build-apps` builds it, sharing the main checkout's build directory for the
#               program's crates, into bench/target/NAME/apps/PROGRAM/release/PROGRAM
set -euo pipefail
name="$1" commit="$2"
shift 2
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(bench preload)
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/whole-block-huge-$name"
out="$root/bench/target/$name"
rm -rf "$tree"
mkdir -p "$tree" "$out"
git -C "$root" archive "$commit" | tar -x -C "$tree"
mkdir -p "$tree/bench/probes/whole-block-huge"
cp -r "$here"/* "$tree/bench/probes/whole-block-huge/"
python3 -I "$tree/bench/probes/whole-block-huge/wbh_edit.py" "$tree"
echo "$(git -C "$root" rev-parse "$commit"), wbh_edit.py $(sha256sum "$here/wbh_edit.py" | cut -c1-12), wbh.rs $(sha256sum "$here/wbh.rs" | cut -c1-12)" > "$out/BUILT-FROM"
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
      ;;
    sparse)
      (cd "$tree/bench/probes/whole-block-huge/sparse" && cargo build --release --target-dir "$out/sparse-build")
      cp "$out/sparse-build/release/sparse" "$out/sparse"
      ;;
    runner)
      python3 -I "$here/runner_edit.py" "$tree/bench"
      (cd "$tree/bench" && cargo build --release -p bench-runner --target-dir "$out/runner-build")
      cp "$out/runner-build/release/benchrun" "$out/benchrun"
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
