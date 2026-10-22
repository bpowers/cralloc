#!/usr/bin/env bash
# The order service's sensitivity to code placement: ROUNDS rounds of `orders axum` over the
# pad builds of pad-orders.sh, each round as two measurement sessions (pads 00-32, then 48-64)
# with GAP seconds between, results in OUTDIR/padN-R.jsonl. Combine with
# `probes/spawn-gap/padsweep.py asmalloc OUTDIR/*.jsonl`. Run from the repository root.
set -u
outdir="$1" rounds="${2:-2}" gap="${GAP:-60}"
W=$(cd "$(dirname "$0")/../../.." && pwd)
mkdir -p "$outdir"
outdir=$(cd "$outdir" && pwd)
for r in $(seq "$rounds"); do
  for half in "00 16 32" "48 64"; do
    allocs=""
    for p in $half; do allocs="$allocs,asmalloc@pad$p,jemalloc@pad$p"; done
    tag=$(echo "$half" | tr ' ' '-')
    (cd "$W/bench" && timeout 1800 "$W/scripts/measure" --both-l3 ./target/release/benchrun run --suite servers \
       --only "=orders axum" --allocators "${allocs#,}" --rounds 1 --out "$outdir/pad$tag-$r.jsonl") 2>&1 \
      | grep -E "server cycles|invalid" | tail -12
    sleep "$gap"
  done
done
