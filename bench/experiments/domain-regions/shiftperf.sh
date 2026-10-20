#!/usr/bin/env bash
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074
for b in asm-base asm-dom asm-base asm-dom; do
  BENCH_CPUS=0,1,2,3,8,9,10,11 MEMLIMIT_QUIET=1 "$W/scripts/memlimit" -m 4G -t 5m -- \
    perf stat -x, -e cycles:u,instructions:u,ls_dmnd_fills_from_sys.near_cache,ls_dmnd_fills_from_sys.dram_io_near \
    -- taskset -c 0-3,8-11 "$W/bench/target/$b/release/workload" shift threads=4 live=500k keep=10 secs=4 sizes=small 2>&1 >/tmp/shift.$$.out \
    | awk -F, -v b=$b '/^[0-9]/ {printf "%s=%.0fM ", $3, $1/1e6} END {printf " %s\n", b}'
  grep SHIFT /tmp/shift.$$.out | awk '$3!="round=0" {split($5,a,"="); s+=a[2]; n++} END {printf "   B later ns/object %.1f\n", s/n}'
done
rm -f /tmp/shift.$$.out
