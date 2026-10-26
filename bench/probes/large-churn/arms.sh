#!/usr/bin/env bash
# Runs a drift job once per arm of the large-churn switch (ASMALLOC_LC, back/lcs.rs) with one
# build, in a random order, one at a time, under scripts/heavy (memory runs) or inside a
# scripts/measure session (timed runs, with BENCH_CPUS set). Each run writes
# OUTDIR/JOB.ARM.txt; a run that already ended cleanly is skipped.
#
#   scripts/heavy bench/probes/large-churn/arms.sh OUTDIR BUILD JOB "ARM..." [key=value ...]
#
# JOB is large (the heap-drift matrix's large mix) or big (the same with one new buffer in a
# hundred ten times larger). An ARM is the switch's setting, e.g. t0p0b0000a0 for main.
# BUILD may also be je or glibc, run once each, with ARM `-`.
set -euo pipefail
out="$1" build="$2" job="$3" arms="$4"
shift 4
here="$(cd "$(dirname "$0")" && pwd)"
run="$here/../heap-drift/run.sh"
case "$job" in
  large) args=(shape=large live=1Gi ops=2400k phases=16 samples=128 tables=phase) ;;
  big) args=(shape=large live=1Gi ops=2400k phases=16 samples=128 tables=phase big_permille=10) ;;
  *) echo "unknown job $job" >&2; exit 2 ;;
esac
mkdir -p "$out"
for arm in $(printf '%s\n' $arms | shuf); do
  name="$arm"
  [ "$arm" = - ] && name="$build"
  file="$out/$job.$name.txt"
  if [ -f "$file" ] && tail -1 "$file" | grep -q '^# exit 0'; then
    continue
  fi
  echo "$(date -Is) $job $name"
  ASMALLOC_LC="$arm" "$run" "$build" "$file" "${args[@]}" quiet=60 quiet_step=5 "$@" \
    || echo "$(date -Is) $job $name failed"
done
