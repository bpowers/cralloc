#!/usr/bin/env bash
# Interleaved rounds of one memory-shape workload under several native builds, with
# bench/probes/memory/memrun.sh (capped, a cgroup each, confined to the CPUs).
# usage: mrun.sh <rounds> <label,label,...> <cpus> <tag> <workload args...>
# Logs land in $S/mem/<tag>.r<round>.<label>.log; a summary line per run goes to stdout.
set -uo pipefail
S=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074
rounds=$1; labels=$2; cpus=$3; tag=$4; shift 4
mkdir -p "$S/mem"
for round in $(seq 1 "$rounds"); do
  for label in $(tr ',' '\n' <<<"$labels" | shuf); do
    OUT="$S/mem" CONFINE=1 EVERY="${EVERY:-4}" bash "$W/bench/probes/memory/memrun.sh" "$W" "$label" "$cpus" \
      "$tag.r$round" "$@" | head -1
  done
done
