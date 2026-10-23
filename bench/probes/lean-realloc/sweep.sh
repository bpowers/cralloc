#!/usr/bin/env bash
# usage: sweep.sh OUTDIR ROUNDS SUITE JOB [NAME...]
# ROUNDS rounds of one job over the pad builds of build.sh for each NAME (default main lean),
# each round as two measurement sessions (pads 00-32, then 48-64), the builds interleaved within
# each. After each session it waits as long as the session ran, and at least GAP seconds
# (default 300), so that heavy work, which a session on both L3s freezes, gets its turn.
# Results in OUTDIR/<tag>-<half>-<round>.jsonl.
# Needs the entries.sh entries in bench/allocators.toml. Combine with
# `probes/spawn-gap/padsweep.py main OUTDIR/<tag>-*.jsonl`. Run from anywhere.
set -u
outdir="$1" rounds="$2" suite="$3" job="$4"
shift 4
names=("$@")
[ ${#names[@]} -gt 0 ] || names=(main lean)
gap="${GAP:-300}"
W=$(cd "$(dirname "$0")/../../.." && pwd)
mkdir -p "$outdir"
outdir=$(cd "$outdir" && pwd)
tag=$(echo "$job" | tr -c 'a-zA-Z0-9\n' '-' | tr -s '-' | sed 's/^-//;s/-$//')
for r in $(seq "$rounds"); do
  for half in "00 16 32" "48 64"; do
    allocs=""
    for p in $half; do
      for n in "${names[@]}"; do allocs="$allocs,$n@pad$p"; done
    done
    h=$(echo "$half" | tr ' ' '-')
    start=$SECONDS
    (cd "$W/bench" && timeout 1800 "$W/scripts/measure" --both-l3 ./target/release/benchrun run \
       --suite "$suite" --only "$job" --allocators "${allocs#,}" --rounds 1 \
       --out "$outdir/$tag-$h-$r.jsonl") > "$outdir/$tag-$h-$r.log" 2>&1
    echo "$tag $h round $r: exit $?"
    # A session on both L3s freezes everyone's heavy work, so leave it at least as long again
    # before the next.
    took=$((SECONDS - start))
    sleep $((took > gap ? took : gap))
  done
done
