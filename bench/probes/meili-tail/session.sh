#!/usr/bin/env bash
# One round of the meili-tail suite (Meilisearch, two minutes at 70%) with the allocators in a
# random order, for docs/research/meili-tail.md. Run from bench/ of this checkout:
#
#   ../scripts/measure --both-l3 probes/meili-tail/session.sh OUTDIR TAG [ALLOCATORS]
#
# Writes OUTDIR/TAG.jsonl (benchrun's results), OUTDIR/TAG.raw/<server pid>.raw (every
# request, from the load generator), OUTDIR/TAG.watch.jsonl (default-thread's watcher: the
# allocator thread's wake-ups and CPU time every 250 ms) and OUTDIR/TAG.irqs.jsonl (irqs.py:
# each CPU's TLB-shootdown and other interrupts every 100 ms). With SCHED=1 also
# OUTDIR/TAG.sched.jsonl (purge-off-path's schedtrace.py, every thread's run-queue wait every
# 2 ms).
#
# ALLOCATORS defaults to asmalloc,asmalloc-parent,sw-thread,sw-slow,jemalloc. All but asmalloc
# and jemalloc are local entries of bench/allocators.toml, not committed:
#
#   [[allocator]]
#   name = "asmalloc-parent"          # build-at.sh asmalloc-parent 2754021
#   feature = "asmalloc"
#   build = "asmalloc-parent"
#   expect = { allocator = "asmalloc", library = "executable" }
#
#   [[allocator]]
#   name = "sw-thread"                # build-at.sh asmalloc-switch 1eea9af switch_edit.py
#   feature = "asmalloc"
#   build = "asmalloc-switch"
#   env = { ASMALLOC_DIAG_SLOW_PURGE = "0" }
#   expect = { allocator = "asmalloc", library = "executable" }
#
# and `sw-slow`, the same with `ASMALLOC_DIAG_SLOW_PURGE = "1"`.
set -euo pipefail
out="$1" tag="$2" allocs="${3:-asmalloc,asmalloc-parent,sw-thread,sw-slow,jemalloc}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out/$tag.raw"
exes=()
for f in "$bench"/target/*/apps/meilisearch/release/meilisearch; do
  [ -x "$f" ] && exes+=(--exe "$f")
done
taskset -c 15 python3 -I "$bench/probes/default-thread/watch.py" --out "$out/$tag.watch.jsonl" \
  "${exes[@]}" --loadgen "$bench/target/loadgen/release/loadgen" &
pids=($!)
taskset -c 14 python3 -I "$here/irqs.py" --out "$out/$tag.irqs.jsonl" &
pids+=($!)
if [ "${SCHED:-0}" = 1 ]; then
  taskset -c 13 python3 -I "$bench/probes/purge-off-path/schedtrace.py" --out "$out/$tag.sched.jsonl" "${exes[@]}" &
  pids+=($!)
fi
trap 'kill -TERM "${pids[@]}"; wait "${pids[@]}"' EXIT
BENCH_RAW_DIR="$out/$tag.raw" "$bench/target/release/benchrun" run --suite meili-tail \
  --allocators "$allocs" --rounds 1 --mem-cap 12G --out "$out/$tag.jsonl"
