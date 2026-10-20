#!/usr/bin/env bash
# `workload shift` under the System-allocator build with a counting library preloaded: the
# counters by domain, and the cgroup peak.
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074
S=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad
for lib in "$@"; do
  BENCH_CPUS=0,1,2,3,8,9,10,11 MEMLIMIT_QUIET=1 "$W/scripts/memlimit" -m 4G -t 5m -- \
    bash "$W/bench/probes/memory/incg.sh" "$S/shiftcount.$lib.peak" \
    taskset -c 0-3,8-11 env LD_PRELOAD="$S/libs/$lib.so" \
    "$W/bench/target/x-sys/release/workload" shift threads=4 live=500k keep=10 secs=4 sizes=small \
    >"$S/shiftcount.$lib.out" 2>"$S/shiftcount.$lib.err"
  echo "== $lib peak=$(awk '{printf "%.1f", $1/1048576}' "$S/shiftcount.$lib.peak")"
  grep XCOUNT "$S/shiftcount.$lib.err" | awk '{printf "  %s %s %s %s %s %s\n", $2, $4, $5, $6, $12, $13}'
done
