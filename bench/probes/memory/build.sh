#!/usr/bin/env bash
# Build the workload binaries into bench/target/<label> with the given bench-alloc features.
# usage: build.sh <bench dir> <label>[=feat+feat...] ...   (a bare label is its own feature)
# RPC=1 also builds the RPC server and client.
set -euo pipefail
bench="$1"; shift
cd "$bench"
pkgs=(-p bench-workloads)
if [ -n "${RPC:-}" ]; then pkgs+=(-p bench-rpc); fi
for spec in "$@"; do
  label="${spec%%=*}"
  feats="${spec#*=}"
  list=""
  for f in ${feats//+/ }; do list="$list,bench-alloc/$f"; done
  echo "== $label (${list#,})"
  taskset -c "${BENCH_BUILD_CPUS:-4-7,20-23}" cargo build --release "${pkgs[@]}" \
    --target-dir "$bench/target/$label" --features "${list#,}" 2>&1 | tail -3
done
