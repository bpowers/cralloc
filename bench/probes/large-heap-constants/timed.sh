#!/usr/bin/env bash
# Timed drift runs for docs/research/large-heap-constants.md: what the ladder's runs under
# scripts/heavy cannot give, because a measurement anywhere on the machine freezes them and
# their cores are shared: how long memory takes to come back in seconds, the lengths of the
# allocator's steps, cycles per operation. The runs go on the suite's measurement CPUs (0-3
# and 8-11), one after another in a random order, each writing OUTDIR/JOB-RUNG.BUILD.txt and
# its watch file as ladder.sh does. Keep a session to a run or two:
#
#   scripts/measure --both-l3 bench/probes/large-heap-constants/timed.sh OUTDIR JOB RUNG "BUILD..."
set -euo pipefail
out="$1" job="$2" rung="$3" builds="$4"
here="$(cd "$(dirname "$0")" && pwd)"
export BENCH_CPUS=0-3,8-11
source "$here/args.sh"
read -r -a job_args <<< "$(args_of "$job" "$rung")"
mkdir -p "$out"
for spec in $(printf '%s\n' $builds | shuf); do
  build="${spec%%:*}"
  env=()
  name="$build"
  if [ "$spec" != "$build" ]; then
    name="$build-${spec#*:}"
    env=(--env ASMALLOC_BACKGROUND=0)
  fi
  file="$out/$job-$rung.$name.txt"
  if [ -f "$file" ] && tail -1 "$file" | grep -q '^# exit 0'; then
    continue
  fi
  echo "$(date -Is) $job $rung $name"
  python3 -I "$here/drift.py" "$build" "$file" --mem 16G "${env[@]}" -- \
    "${job_args[@]}" quiet=60 quiet_step=1 || echo "$(date -Is) $job $rung $name failed: $?"
done
