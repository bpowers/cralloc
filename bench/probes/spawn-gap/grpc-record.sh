#!/usr/bin/env bash
# usage: grpc-record.sh ALLOCATOR TAG [EVENTS]
# One benchrun run of connect log grpc h2 with ALLOCATOR, and `perf record -a -C 0-3` over the
# six seconds of its s70 segment, into bench/target/perf/rec-ALLOCATOR-TAG.data. The server is
# found by its binary path (the allocator's build directory) only to time the recording.
# Run it under scripts/measure --both-l3, from the repository root.
set -u
W=$(cd "$(dirname "$0")/../../.." && pwd)
alloc=$1; tag=$2
events=${3:-cycles/period=2000000/,op_cache_hit_miss.op_cache_miss/period=20000/}
build=$(awk -v n="$alloc" '$1=="name" && $3=="\""n"\"" {f=1} f && $1=="build" {gsub(/"/,"",$3); print $3; exit}' "$W/bench/allocators.toml")
out="$W/bench/target/perf/rec-$alloc-$tag"
mkdir -p "$(dirname "$out")"
(cd "$W/bench" && ./target/release/benchrun run --suite servers --only "=connect log grpc h2" \
   --allocators "$alloc" --rounds 1 --out "$out.jsonl" > "$out.log" 2>&1) &
runner=$!
bin="$W/bench/target/$build/release/rpc-server"
for _ in $(seq 1200); do pgrep -f "^$bin" > /dev/null && break; sleep 0.1; done
# The plan before s70: _fill 2 s, sat 4, _w30 1, s30 6, _w70 1 (suite.rs, `day`).
sleep 14.5
perf record -q -o "$out.data" -a -C 0-3 -e "$events" -- sleep 5 > /dev/null 2>&1
wait "$runner"
grep -E "server cycles" "$out.log" | tail -1
