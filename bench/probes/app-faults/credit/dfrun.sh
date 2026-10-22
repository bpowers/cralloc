#!/usr/bin/env bash
# dfrun.sh BUILD TAG: one DataFusion (TPC-H SF3) run of bench/target/BUILD on CPUs 0-3, in a
# cgroup of its own whose memory.peak is the peak reported. Writes OUT/TAG.{out,err,peak}, and
# moves a series file the run's allocator wrote (creditdiag builds) to OUT/TAG.series.
set -uo pipefail
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-ac0a2195f8109ecfc
O=/home/bpowers/src/asmalloc/.claude/worktrees/agent-ac0a2195f8109ecfc/bench/target/hc-runs/df
build="$1"; tag="$2"
mkdir -p "$O"
before=$(date +%s.%N)
env -i PATH="$PATH" HOME="$HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}" BENCH_CPUS=0,1,2,3 \
  "$W/scripts/memlimit" -m 4G -t 5m -- bash "$W/bench/probes/memory/incg.sh" "$O/$tag.peak" taskset -c 0-3 \
  "$W/bench/target/$build/apps/datafusion/release/datafusion" tpch data=/home/bpowers/src/asmalloc/bench/apps/target/inputs/datafusion/sf3 \
  > "$O/$tag.out" 2> "$O/$tag.err"
s=$(find "$W/bench/target/hc-series" -name '*.series' -newermt "@$before" | head -1)
if [ -n "$s" ]; then mv "$s" "$O/$tag.series"; fi
