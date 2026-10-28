#!/usr/bin/env bash
# One timed session of the server ladder of docs/research/large-heap-constants.md: the order
# service at one live size, every allocator once in a random order (benchrun interleaves them),
# on the large-heap day. Run from the repository root with the local entries of
# allocators.local.toml appended to bench/allocators.toml:
#
#   scripts/measure --both-l3 bench/probes/large-heap-constants/servers.sh OUTDIR RUNG ROUND [ALLOCATORS]
#
# RUNG is 0.5G, 2G, 4G or 10G; ALLOCATORS defaults to the four incumbents' peers below, the
# diagnostic build and the live-bytes build.
set -euo pipefail
out="$1" rung="$2" round="$3"
allocators="${4:-asmalloc,jemalloc,tcmalloc-percpu,mimalloc,asmalloc-diag,live-bytes}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
case "$rung" in
  0.5G) cap=3G ;;
  2G) cap=7G ;;
  4G) cap=12G ;;
  10G) cap=13G ;;
  *) echo "unknown rung $rung" >&2; exit 2 ;;
esac
mkdir -p "$out"
cd "$bench"
# The watcher samples every server, on the runner's CPU: page tables, mappings, the background
# thread, and the diagnostic build's counters.
exes=()
for b in asmalloc asmalloc-diag je tc mi; do exes+=("target/$b/apps/orders/release/orders"); done
taskset -c 15 python3 -I "$here/watch_servers.py" "$out/orders-$rung-r$round.watch.jsonl" 0.5 "${exes[@]}" &
watcher=$!
trap 'kill $watcher 2>/dev/null || true' EXIT
./target/release/benchrun run --suite servers-ladder --only "=orders axum cache $rung" \
  --allocators "$allocators" --rounds 1 --mem-cap "$cap" --out "$out/orders-$rung-r$round.jsonl"
