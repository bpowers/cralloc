#!/usr/bin/env bash
# Builds the allocator at COMMIT with the huge-tail switch (ht_edit.py) and heap-drift's
# workload and sampler, for docs/research/huge-tail.md, as the bench build NAME of this
# checkout. As grown-slots/build.sh: the commit's tree is extracted under target/ of this
# checkout (/tmp is memory), edited, built, and removed.
#
#   scripts/heavy bench/probes/huge-tail/build.sh NAME COMMIT [PART...]
#
# The drift workload, its probes and the memstat sampler come from heap-drift's commits
# (DRIFT_BASE..DRIFT, bench/ only), applied over COMMIT's tree, unless DRIFT=none (a commit
# that has them, as main since 47e88d4). HT_EDIT=0 builds COMMIT as it is, without the
# switch: the product and main, built the same way for comparison.
#
# PARTs (default: workload memstat):
#   workload    the bench workloads with bench-alloc/asmalloc, into bench/target/NAME/release
#   memstat     the same with bench-alloc/asmalloc-memstat, into bench/target/NAME-memstat/release
#   je, glibc   the workloads with jemalloc or glibc, into bench/target/PART
#   rpc         the bench RPC servers with bench-alloc/asmalloc, into bench/target/NAME/release
#   PROGRAM     an app of bench/apps (meilisearch, orders, ...), as `benchrun build-apps`
#               builds it, into bench/target/NAME/apps/PROGRAM/release/PROGRAM
# KEEP=1 keeps the edited tree under target/huge-tail-NAME.
set -euo pipefail
name="$1" commit="$2"
shift 2
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(workload memstat)
DRIFT_BASE="${DRIFT_BASE:-553661020a6d83e4e4c8b3c517c7ccee9d500e3b}"
DRIFT="${DRIFT:-4522abd484eee7a39d5df92c419db91ffc638afa}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/huge-tail-$name"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$root" archive "$commit" | tar -x -C "$tree"
stamp="$(git -C "$root" rev-parse "$commit")"
if [ "$DRIFT" != none ]; then
  git -C "$root" diff "$DRIFT_BASE" "$DRIFT" -- bench > "$tree/drift.patch"
  # Not `git apply`: run inside this checkout, it skips paths outside the current directory.
  (cd "$tree" && patch -p1 -s --no-backup-if-mismatch < drift.patch)
  stamp="$stamp + heap-drift $(git -C "$root" rev-parse --short "$DRIFT")"
fi
if [ "${HT_EDIT:-1}" = 1 ]; then
  mkdir -p "$tree/bench/probes/huge-tail"
  cp "$here"/*.py "$here"/*.rs "$tree/bench/probes/huge-tail/"
  python3 -I "$tree/bench/probes/huge-tail/ht_edit.py" "$tree"
  stamp="$stamp, ht_edit.py $(sha256sum "$here/ht_edit.py" | cut -c1-12), ht.rs $(sha256sum "$here/ht.rs" | cut -c1-12)"
fi
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
for part in "${parts[@]}"; do
  case "$part" in
    workload)
      out="$root/bench/target/$name"
      mkdir -p "$out"
      (cd "$tree/bench" && cargo build --release -p bench-workloads --features bench-alloc/asmalloc --target-dir "$out")
      echo "$stamp" > "$out/BUILT-FROM"
      ;;
    memstat)
      out="$root/bench/target/$name-memstat"
      mkdir -p "$out"
      (cd "$tree/bench" && cargo build --release -p bench-workloads --features bench-alloc/asmalloc-memstat --target-dir "$out")
      echo "$stamp" > "$out/BUILT-FROM"
      ;;
    je | glibc)
      # The same workload with an incumbent, for comparison, into bench/target/PART.
      out="$root/bench/target/$part"
      mkdir -p "$out"
      features=()
      [ "$part" = glibc ] || features=(--features bench-alloc/je)
      (cd "$tree/bench" && cargo build --release -p bench-workloads --target-dir "$out" "${features[@]}")
      echo "$stamp" > "$out/BUILT-FROM"
      ;;
    rpc)
      out="$root/bench/target/$name"
      mkdir -p "$out"
      (cd "$tree/bench" && cargo build --release -p bench-rpc --features bench-alloc/asmalloc --target-dir "$out")
      echo "$stamp" > "$out/BUILT-FROM"
      ;;
    *)
      out="$root/bench/target/$name"
      target="$out/apps/$part"
      mkdir -p "$target"
      (cd "$tree/bench/apps/$part" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/$part" \
        cargo build --release --locked --features asmalloc --target-dir "$target")
      echo "$stamp" > "$out/BUILT-FROM"
      ;;
  esac
done
# KEEP=1 keeps the edited tree, for another checkout to build a program against.
[ "${KEEP:-0}" = 1 ] || rm -rf "$tree"
echo "$name: $stamp [${parts[*]}]"
