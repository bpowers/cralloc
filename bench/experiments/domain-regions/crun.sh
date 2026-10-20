#!/usr/bin/env bash
# Runs a preloaded C program under several libraries and placements, interleaved in a new
# random order each round, each run capped and in a cgroup of its own, with hardware counters.
# usage: crun.sh <rounds> <lib,lib,...> <one,two> <tag> <program> [args...]
# A lib is a name in libs/ (libs/<name>.so) or "mi" for mimalloc.
# Prints one line per run: tag lib placement round wall_s peak_MiB and the counters.
# Run it under scripts/measure --both-l3.
set -uo pipefail
S=/tmp/claude-1000/-home-bpowers-src-asmalloc/9350b66e-ff5f-4010-842a-f49ee9af0052/scratchpad
W=/home/bpowers/src/asmalloc/.claude/worktrees/agent-a25b3cca499ac1074
CB=/home/bpowers/src/asmalloc/bench/baselines/out/cbench
rounds=$1; libs=$2; places=$3; tag=$4; shift 4
prog="$CB/$1"; shift
out="$S/out"; mkdir -p "$out"
EV=${EV:-cycles,instructions,ls_dmnd_fills_from_sys.near_cache,ls_dmnd_fills_from_sys.dram_io_near,ls_dmnd_fills_from_sys.local_ccx}
for round in $(seq 1 "$rounds"); do
  combos=()
  for l in ${libs//,/ }; do for p in ${places//,/ }; do combos+=("$l:$p"); done; done
  for c in $(printf '%s\n' "${combos[@]}" | shuf); do
    lib=${c%%:*}; place=${c##*:}
    case "$lib" in
      mi) so=/home/bpowers/src/asmalloc/bench/baselines/out/mimalloc/libmimalloc.so ;;
      *) so="$S/libs/$lib.so" ;;
    esac
    case "$place" in one) cpus=0-7 ;; two) cpus=0-3,8-11 ;; esac
    base="$out/$tag.$lib.$place.$round"
    t0=$(date +%s%N)
    MEMLIMIT_QUIET=1 "$W/scripts/memlimit" -m 6G -t 10m -- \
      bash "$W/bench/probes/memory/incg.sh" "$base.peak" \
      perf stat -a -C "$cpus" -A -x, -o "$base.perf" -e "$EV" -- \
      taskset -c "$cpus" bash -c 's=$EPOCHREALTIME; LD_PRELOAD="$0" "$@"; r=$?; e=$EPOCHREALTIME; echo "WALL $s $e" >&2; exit $r' \
      "$so" "$prog" "$@" >"$base.out" 2>"$base.err"
    st=$?
    wall=$(awk '/^WALL/ {printf "%.3f", $3-$2}' "$base.err")
    peak=$(awk '{printf "%.1f", $1/1048576}' "$base.peak" 2>/dev/null)
    # Per-CPU lines: CPUn,value,unit,event,...; summed per event, and cycles per L3 (0-7, 8-15).
    ctr=$(awk -F, '$1 ~ /^CPU/ && $2 ~ /^[0-9]/ {
        cpu = substr($1, 4) + 0; ev = $4; sub(/^ls_dmnd_fills_from_sys\./, "", ev)
        sum[ev] += $2; if (ev == "cycles") l3[int(cpu / 8)] += $2 }
      END { for (e in sum) printf "%s=%d ", e, sum[e];
            printf "l3a=%.2f", (l3[0] + l3[1] > 0 ? l3[0] / (l3[0] + l3[1]) : 0) }' "$base.perf" 2>/dev/null)
    # The program's own figure: larson's throughput, xmalloc-test's frees a second.
    m=$(cat "$base.out" "$base.err" | awk '/Throughput =/ {for (i=1;i<=NF;i++) if ($i=="=") v=$(i+1)}
          /free\/sec:/ {for (i=1;i<=NF;i++) if ($i=="free/sec:") v=$(i+1)} END {if (v!="") print v}')
    echo "$tag $lib $place r$round st=$st wall=$wall peak=$peak ${m:+m=$m} $ctr"
  done
done
