#!/usr/bin/env bash
# usage: rpc-headers.sh NAME...
# For each traced build bench/target/trc-NAME (ab-build.sh --trace): starts rpc-server (the
# log service) on CPUs 12-15, stops it once it listens, and prints where tokio's run-queue
# headers landed (the 40-byte block allocated after each worker's 2048-byte queue buffer) and
# how many pairs of them share a cache line. Placement only, not timed. Run it under
# scripts/memlimit (scripts/memlimit -m 2G -- bench/probes/spawn-gap/rpc-headers.sh main side).
set -u
W=$(cd "$(dirname "$0")/../../.." && pwd)
for name in "$@"; do
  out="$W/bench/target/perf/rpc-headers-$name"
  mkdir -p "$(dirname "$out")"
  rm -f "$out.trace" "$out.log"
  BENCH_ALLOC_TRACE="$out.trace" BENCH_CPUS=12-15 taskset -c 12-15 \
    "$W/bench/target/trc-$name/release/rpc-server" log > "$out.log" 2>&1 &
  pid=$!
  for _ in $(seq 100); do grep -q BENCH_LISTEN "$out.log" && break; sleep 0.1; done
  sleep 0.5
  kill -INT "$pid"
  wait "$pid"
  echo "== $name"
  python3 -I - "$W/bench/probes/spawn-gap" "$out.trace" <<'EOF'
import sys
sys.path.insert(0, sys.argv[1])
import trace
recs = [r for r in trace.load(sys.argv[2]) if r[1] in (0, 2)]
heads = [recs[i + 1][5] for i in range(len(recs) - 1) if recs[i][4] == 2048 and recs[i + 1][4] == 40]
lines = [{p // 64, (p + 39) // 64} for p in heads]
print("headers:", " ".join(hex(p) for p in heads))
print("pairs sharing a line:", sum(1 for i in range(len(lines)) for j in range(i) if lines[i] & lines[j]))
EOF
done
