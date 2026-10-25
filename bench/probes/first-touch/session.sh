#!/usr/bin/env bash
# One server run with the address-space trace read beside it (wlwatch.py) and every request's
# record kept. Run from bench/ of this checkout, under scripts/measure --both-l3:
#
#   ../scripts/measure --both-l3 probes/first-touch/session.sh OUTDIR TAG SUITE JOB MEMCAP [ALLOCATORS]
#
# SUITE and JOB as benchrun takes them (`servers-thread` and `=connect store 1M thread`).
# Results: OUTDIR/TAG.jsonl (benchrun's), OUTDIR/TAG.raw/<server pid>.raw (the requests),
# OUTDIR/TAG.wl/ (the trace). ALLOCATORS defaults to asmalloc-wl, a local entry of
# bench/allocators.toml (not committed): `feature = "asmalloc"`, `build = "wl"`.
set -euo pipefail
out="$1" tag="$2" suite="$3" job="$4" cap="$5" allocs="${6:-asmalloc-wl}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out/$tag.raw"
exes=()
for f in "$bench"/target/*/release/rpc-server "$bench"/target/*/apps/{orders,meilisearch}/release/{orders,meilisearch}; do
  [ -x "$f" ] && exes+=(--exe "$f")
done
{
  echo "start $(date -Is)"
  awk '/Normal/ { s = 0; for (i = 14; i <= NF; i++) s += $i; print "free_order9plus", s }' /proc/buddyinfo
  grep -E '^(thp_fault_alloc|thp_fault_fallback|compact_stall) ' /proc/vmstat
} > "$out/$tag.machine"
taskset -c 15 python3 -I "$here/wlwatch.py" "$out/$tag.wl" "${exes[@]}" &
watcher=$!
trap 'kill -TERM $watcher; wait $watcher' EXIT
export BENCH_RAW_DIR="$out/$tag.raw"
"$bench/target/release/benchrun" run --suite "$suite" --only "$job" \
  --allocators "$allocs" --rounds 1 --mem-cap "$cap" --out "$out/$tag.jsonl"
{
  echo "end $(date -Is)"
  awk '/Normal/ { s = 0; for (i = 14; i <= NF; i++) s += $i; print "free_order9plus", s }' /proc/buddyinfo
  grep -E '^(thp_fault_alloc|thp_fault_fallback|compact_stall) ' /proc/vmstat
} >> "$out/$tag.machine"
