#!/usr/bin/env bash
# Timed runs for docs/research/heap-drift.md: a shortened version of one drift job with every
# build, on the suite's measurement CPUs (0-3 and 8-11: both L3 domains, one thread a core),
# ROUNDS rounds with the builds in a new random order each round, under a memory cap. Cycles
# per operation are in each run's BENCH_RESULT. One job is one session of a few minutes:
#
#   scripts/measure --both-l3 bench/probes/heap-drift/timed.sh OUTDIR JOB [ROUNDS] [BUILDS]
#
# and leave a gap as long as the session before the next.
set -euo pipefail
out="$1" job="$2" rounds="${3:-2}" builds="${4:-asmalloc je tc mi glibc}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
cpus="0-3,8-11"
case "$job" in
  steady) args="shape=steady live=2Gi ops=1500M samples=30" ;;
  shift) args="shape=shift live=2Gi ops=1500M phases=3 samples=36" ;;
  lifetime) args="shape=lifetime ops=2400M phases=4 samples=24 long_permille=5" ;;
  swing) args="shape=swing live=1Gi ops=480M phases=6 samples=36" ;;
  large) args="shape=large live=1Gi ops=240k phases=4 samples=32" ;;
  *) echo "unknown job $job" >&2; exit 2 ;;
esac
mkdir -p "$out"
for round in $(seq 1 "$rounds"); do
  for build in $(printf '%s\n' $builds | shuf); do
    file="$out/timed.$job.$build.r$round.txt"
    env=()
    [ "$build" = tc ] && env+=(GLIBC_TUNABLES=glibc.pthread.rseq=0)
    echo "# $(date -Is) build=$build cpus=$cpus args: $args" > "$file"
    # shellcheck disable=SC2086
    MEMLIMIT_QUIET=1 env "${env[@]}" BENCH_CPUS="$cpus" "$root/scripts/memlimit" -m 8G -t 30m -- \
      taskset -c "$cpus" "$root/bench/target/$build/release/workload" drift $args tables=phase quiet=0 >> "$file" 2>&1 \
      || echo "# exit $?" >> "$file"
  done
done
