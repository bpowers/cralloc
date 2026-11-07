#!/usr/bin/env bash
# Builds the workload binary of this checkout once per allocator, as `benchrun build` does
# (into bench/target/NAME), plus asmalloc's memstat build, which breaks its heap down. Run
# under scripts/heavy:
#
#   scripts/heavy bench/probes/heap-drift/build.sh [NAME...]
#
# NAME is one of glibc je mi tc asmalloc asmalloc-memstat (default: all).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
shared="$(dirname "$(git -C "$bench" rev-parse --path-format=absolute --git-common-dir)")/bench"
export BENCH_BASELINES="${BENCH_BASELINES:-$shared/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
names=("$@")
[ ${#names[@]} -gt 0 ] || names=(glibc je mi tc asmalloc asmalloc-memstat)
cd "$bench"
for name in "${names[@]}"; do
  features=()
  [ "$name" = glibc ] || features=(--features "bench-alloc/$name")
  echo "== $name"
  cargo build --release -p bench-workloads --target-dir "target/$name" "${features[@]}"
done
