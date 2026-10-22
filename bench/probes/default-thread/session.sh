#!/usr/bin/env bash
# One session of the servers-thread suite with the watcher beside it
# (docs/research/default-thread-servers.md). Run from bench/ of this checkout, under
# scripts/measure --both-l3:
#
#   ../scripts/measure --both-l3 probes/default-thread/session.sh OUTDIR TAG JOB MEMCAP
#
# JOB is a job id of the suite (`orders axum cache 2G thread`), TAG names the files:
# OUTDIR/TAG.jsonl (benchrun's results) and OUTDIR/TAG.watch.jsonl (the watcher's samples).
# `asmalloc-off` is a local entry of bench/allocators.toml, not committed: the asmalloc build
# (`feature = "asmalloc"`, `build = "asmalloc"`) with `env = { ASMALLOC_BACKGROUND = "0" }`.
set -euo pipefail
out="$1" tag="$2" job="$3" cap="$4"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out"
taskset -c 15 python3 -I "$here/watch.py" --out "$out/$tag.watch.jsonl" \
  --exe "$bench/target/asmalloc/apps/orders/release/orders" \
  --exe "$bench/target/je/apps/orders/release/orders" \
  --exe "$bench/target/asmalloc/apps/meilisearch/release/meilisearch" \
  --exe "$bench/target/je/apps/meilisearch/release/meilisearch" \
  --exe "$bench/target/asmalloc/release/rpc-server" \
  --exe "$bench/target/je/release/rpc-server" \
  --loadgen "$bench/target/loadgen/release/loadgen" &
watcher=$!
trap 'kill -TERM $watcher; wait $watcher' EXIT
"$bench/target/release/benchrun" run --suite servers-thread --only "=$job" \
  --allocators asmalloc,asmalloc-off,jemalloc --rounds 1 --mem-cap "$cap" --out "$out/$tag.jsonl"
