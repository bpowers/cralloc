#!/usr/bin/env bash
# One session of the servers-thread suite with the watcher beside it
# (docs/research/default-thread-servers.md). Run from bench/ of this checkout, under
# scripts/measure --both-l3:
#
#   ../scripts/measure --both-l3 probes/default-thread/session.sh OUTDIR TAG JOB MEMCAP \
#       [ALLOCATORS] [BURST_AT]
#
# JOB is a job id of the suite (`orders axum cache 2G thread`), TAG names the files:
# OUTDIR/TAG.jsonl (benchrun's results) and OUTDIR/TAG.watch.jsonl (the watcher's samples).
# ALLOCATORS defaults to asmalloc,asmalloc-off,jemalloc. With BURST_AT (seconds from the start
# of the plan to the burst: the fill, 2, 30 and 30) the watcher samples a diagnostic build's
# counters every millisecond around the burst, and every request's record is kept in
# OUTDIR/TAG.raw/<server pid>.raw.
#
# Entries other than asmalloc and jemalloc are local, in bench/allocators.toml, not committed:
# `asmalloc-off` is the asmalloc build (`feature = "asmalloc"`, `build = "asmalloc"`) with
# `env = { ASMALLOC_BACKGROUND = "0" }`; the note lists the others.
set -euo pipefail
out="$1" tag="$2" job="$3" cap="$4" allocs="${5:-asmalloc,asmalloc-off,jemalloc}" burst="${6:-}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out"
exes=()
for f in "$bench"/target/*/release/rpc-server "$bench"/target/*/apps/{orders,meilisearch}/release/{orders,meilisearch}; do
  [ -x "$f" ] && exes+=(--exe "$f")
done
fast=()
if [ -n "$burst" ]; then
  fast=(--fast-at "$burst")
  mkdir -p "$out/$tag.raw"
  export BENCH_RAW_DIR="$out/$tag.raw"
fi
taskset -c 15 python3 -I "$here/watch.py" --out "$out/$tag.watch.jsonl" "${exes[@]}" "${fast[@]}" \
  --loadgen "$bench/target/loadgen/release/loadgen" &
watcher=$!
trap 'kill -TERM $watcher; wait $watcher' EXIT
"$bench/target/release/benchrun" run --suite servers-thread --only "=$job" \
  --allocators "$allocs" --rounds 1 --mem-cap "$cap" --out "$out/$tag.jsonl"
