#!/usr/bin/env bash
# usage: plan.sh QUEUEDIR OUTDIR ROUND SUITE JOB[,JOB] PADS ALLOCS [benchrun run's other options...]
#   PADS: pads joined by '-', such as 00-16; ALLOCS: cra jem tcm mim gli joined by ','.
# Prints one queue line (queue.sh) for one session: one round of the jobs at the pads
# for the allocators, interleaved, the results in OUTDIR/rROUND-JOB-PADS.jsonl. Each line first
# runs guard.sh, which logs the server CPUs' idle setting and stops the queue unless deep idle
# is off on them; benchrun records the setting per run as well.
set -euo pipefail
W=$(cd "$(dirname "$0")/../.." && pwd)
q="$1" out="$2" round="$3" suite="$4" jobs="$5" pads="$6" allocs="$7"
shift 7
name() {
  case "$1" in
    cra) echo cralloc ;; cro) echo cralloc-outline ;; jem) echo jemalloc ;; tcm) echo tcmalloc-percpu ;;
    mim) echo mimalloc ;; gli) echo glibc ;;
    *) echo "plan: unknown allocator $1" >&2; exit 2 ;;
  esac
}
list=()
for p in ${pads//-/ }; do
  for a in ${allocs//,/ }; do list+=("$(name "$a")@pad$p"); done
done
only=$(echo "$jobs" | sed 's/^/=/; s/,/,=/g')
tag=$(echo "$jobs" | tr ' ,' '-+')
file="$out/r$round-$tag-$pads.jsonl"
alist=$(IFS=,; echo "${list[*]}")
printf '%s/bench/standings/guard.sh %s && %s/bench/standings/session.sh %s %s %q %s %s\n' \
  "$W" "$q" "$W" "$file" "$suite" "$only" "$alist" "$*"
