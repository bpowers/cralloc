#!/usr/bin/env bash
# Runs record.py once for each line of PLAN, each under its own `scripts/measure --both-l3`
# with GAP seconds between them (default 60), so that no session holds the machine for more
# than a run. A line is: OUTNAME|JOB|ALLOCATOR|MODE[|extra arguments]; MODE `count` runs
# count.py (the counting build) instead of record.py. Lines whose
# OUTNAME already holds a meta.json with runner_status 0 are skipped, so a plan can be resumed.
#
#   bench/probes/server-profile/campaign.sh PLAN OUTDIR   (from the repository root)
set -u
plan="$1" outdir="$2" gap="${GAP:-60}"
W=$(cd "$(dirname "$0")/../../.." && pwd)
first=1
while IFS='|' read -r name job alloc mode extra; do
  [ -z "$name" ] && continue
  case "$name" in \#*) continue ;; esac
  out="$outdir/$name"
  if [ -f "$out/meta.json" ] && grep -q '"runner_status": 0' "$out/meta.json"; then
    echo "skip $name"; continue
  fi
  [ "$first" = 1 ] || sleep "$gap"
  first=0
  echo "== $(date +%T) $name: $job / $alloc / $mode $extra"
  if [ "$mode" = count ]; then
    # shellcheck disable=SC2086
    timeout 1800 "$W/scripts/measure" --both-l3 taskset -c 13 python3 "$W/bench/probes/server-profile/count.py" \
      --job "$job" --alloc "$alloc" --out "$out" $extra 2>&1 | tail -3
  else
    # shellcheck disable=SC2086
    timeout 1800 "$W/scripts/measure" --both-l3 taskset -c 13 python3 "$W/bench/probes/server-profile/record.py" \
      --job "$job" --alloc "$alloc" --mode "$mode" --out "$out" $extra 2>&1 | tail -3
  fi
done < "$plan"
echo "== $(date +%T) done"
