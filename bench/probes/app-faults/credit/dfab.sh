#!/usr/bin/env bash
# dfab.sh SET ROUNDS BUILD...: interleaved DataFusion runs, the builds in a fresh random order
# each round, each after 300 ms of spinning the CPUs up. One line per run: round, build, cgroup
# peak, page faults, cycles. A measurement: run under scripts/measure --both-l3.
set -uo pipefail
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-ac0a2195f8109ecfc
A=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad/hc
O="$W/bench/target/hc-runs/df"
set_="$1"; rounds="$2"; shift 2
for r in $(seq "$rounds"); do
  for b in $(printf '%s\n' "$@" | shuf); do
    "$W/bench/target/hc-tools/spin" 0-3 300
    tag="$set_-$b-r$r"
    bash "$A/dfrun.sh" "$b" "$tag"
    o="$O/$tag"
    peak=$(awk '{printf "%.1f", $1/1048576}' "$o.peak")
    pf=$(grep -o '"page_faults":[0-9]*' "$o.out" | head -1 | cut -d: -f2)
    cyc=$(grep -o '"cycles":[0-9]*' "$o.out" | head -1 | cut -d: -f2)
    ok=$(grep -c '"primary"' "$o.out")
    echo "r$r $b peak=$peak faults=$pf cycles=$cyc ok=$ok"
  done
done
