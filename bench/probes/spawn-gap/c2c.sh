#!/usr/bin/env bash
# usage: c2c.sh PLACE TAG BUILD... ; records perf c2c (IBS) for each build's spawn run.
# ROUNDS (default 20000); TRACE=1 also writes the allocation trace (traced builds).
set -u
WT=$(cd "$(dirname "$0")/../../.." && pwd)
place=$1; tag=$2; shift 2
rounds=${ROUNDS:-20000}
case $place in
  t4) cpus=0-3; th=4 ;;
  t8) cpus=0-3,8-11; th=8 ;;
esac
for b in "$@"; do
  out="$WT/bench/target/perf/$b-$place$tag"
  if [ -n "${TRACE:-}" ]; then export BENCH_ALLOC_TRACE="$out.trace"; fi
  BENCH_CPUS=$cpus MEMLIMIT_QUIET=1 "$WT/scripts/memlimit" -m 4G -t 120 -- \
    perf c2c record -o "$out.c2c" -- -a -C "$cpus" -c ${PERIOD:-10000} \
    taskset -c "$cpus" "$WT/bench/target/run/$b/workload" spawn threads=$th rounds=$rounds sizes=small ${EXTRA:-} 2>&1 \
    | grep -E "BENCH_RESULT|samples" | cut -c1-160
done
