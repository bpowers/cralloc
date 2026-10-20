#!/usr/bin/env bash
# Instruction profiles of `workload shift` under two native builds.
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074
S=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad
for b in asm-base asm-dom; do
  BENCH_CPUS=0,1,2,3,8,9,10,11 MEMLIMIT_QUIET=1 "$W/scripts/memlimit" -m 4G -t 5m -- \
    perf record -q -e instructions:u -o "$S/shift.$b.perf" -- taskset -c 0-3,8-11 \
    "$W/bench/target/$b/release/workload" shift threads=4 live=500k keep=10 secs=4 sizes=small >/dev/null 2>&1
done
for b in asm-base asm-dom; do
  echo "== $b"
  perf report -q -i "$S/shift.$b.perf" --sort symbol 2>/dev/null | head -12
done
