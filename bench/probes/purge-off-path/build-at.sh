#!/usr/bin/env bash
# Builds the allocator at a commit, optionally with one constant changed, as the bench build
# NAME beside this checkout's `asmalloc`, for the comparisons of
# docs/research/purge-off-path.md: the commit's tree is extracted under target/ (gitignored;
# /tmp is memory), its runner builds the workloads and the programs for asmalloc there,
# sharing the main checkout's baselines and apps data, and the builds are copied to
# bench/target/NAME of this checkout.
#
#   scripts/heavy bench/probes/purge-off-path/build-at.sh NAME COMMIT [FILE SED-EXPRESSION]
#
# For example `asmalloc-main main`, or `asmalloc-l4 purge-off-path
# crates/asmalloc/src/back/policy.rs 's/GROWTH_UNPAID_HUGE_PAGES: usize = 8/GROWTH_UNPAID_HUGE_PAGES: usize = 2/'`.
# Then add a local entry to bench/allocators.toml (not committed):
#
#   [[allocator]]
#   name = "NAME"
#   feature = "asmalloc"
#   build = "NAME"
#   expect = { allocator = "asmalloc", library = "executable" }
set -euo pipefail
name="$1" commit="$2"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/purge-off-path-$name"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$root" archive "$commit" | tar -x -C "$tree"
echo "$(git -C "$root" rev-parse "$commit") ${3:-} ${4:-}" > "$tree/BUILT-FROM"
if [ $# -ge 4 ]; then
  before="$(sha256sum "$tree/$3")"
  sed -i "$4" "$tree/$3"
  [ "$before" != "$(sha256sum "$tree/$3")" ] || { echo "build-at: $4 changed nothing" >&2; exit 1; }
fi
export BENCH_BASELINES="$shared/baselines/out" BENCH_APPS_DATA="$shared/apps/target" \
  BENCH_RESULTS="$shared/results"
cd "$tree/bench"
cargo build --release -p bench-runner
./target/release/benchrun build --allocators asmalloc
./target/release/benchrun build-apps --apps orders,datafusion,meilisearch --allocators asmalloc
rm -rf "$root/bench/target/$name"
cp -a "$tree/bench/target/asmalloc" "$root/bench/target/$name"
cp "$tree/BUILT-FROM" "$root/bench/target/$name/BUILT-FROM"
echo "$name: $(cat "$tree/BUILT-FROM")"
