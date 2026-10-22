#!/usr/bin/env bash
# One session of the servers-thread suite for docs/research/purge-off-path.md: main's build
# (`asmalloc-main`, from build-at.sh and a local entry of bench/allocators.toml), this
# checkout's, and jemalloc, with default-thread's watcher beside them. Run from bench/ of this
# checkout, under scripts/measure --both-l3:
#
#   ../scripts/measure --both-l3 probes/purge-off-path/servers.sh OUTDIR TAG JOB MEMCAP
#
# JOB is a job id of the suite (`connect store 1M thread`); OUTDIR/TAG.jsonl holds benchrun's
# results and OUTDIR/TAG.watch.jsonl the watcher's; probes/default-thread/summarize.py reads
# them.
set -euo pipefail
out="$1" tag="$2" job="$3" cap="$4"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out"
exes=()
for build in asmalloc asmalloc-main je; do
  exes+=(--exe "$bench/target/$build/apps/meilisearch/release/meilisearch"
         --exe "$bench/target/$build/release/rpc-server"
         --exe "$bench/target/$build/apps/orders/release/orders")
done
taskset -c 15 python3 -I "$bench/probes/default-thread/watch.py" --out "$out/$tag.watch.jsonl" \
  "${exes[@]}" --loadgen "$bench/target/loadgen/release/loadgen" &
watcher=$!
trap 'kill -TERM $watcher; wait $watcher' EXIT
"$bench/target/release/benchrun" run --suite servers-thread --only "=$job" \
  --allocators asmalloc-main,asmalloc,jemalloc --rounds 1 --mem-cap "$cap" --out "$out/$tag.jsonl"
