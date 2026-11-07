#!/usr/bin/env bash
# The long memory runs of docs/research/heap-drift.md: every job with every build, one run at
# a time, the builds in a new random order for each job. Each run writes OUTDIR/JOB.BUILD.txt;
# a run whose file already ends cleanly is skipped, so a matrix that was stopped resumes. Run
# under scripts/heavy (these runs are not timed and need no measurement lock):
#
#   scripts/heavy bench/standings/drift/matrix.sh OUTDIR "JOB..." "BUILD..." [key=value ...]
#
# Extra key=value arguments are passed to every run.
set -euo pipefail
out="$1" jobs="$2" builds="$3"
shift 3
here="$(cd "$(dirname "$0")" && pwd)"

# Each job about ten minutes of cralloc on the eight CPUs of scripts/heavy, with a table of
# sizes drawn for every phase, as the runs of 2026-10-24 were made (`tables=shared` after the
# job's name gives phases that ask for the same sizes the same table).
args_of() {
  case "$1" in
    steady) echo "shape=steady live=2Gi ops=15G samples=150" ;;
    shift) echo "shape=shift live=2Gi ops=12G phases=12 samples=288" ;;
    lifetime) echo "shape=lifetime ops=24G phases=24 samples=144 long_permille=1" ;;
    swing) echo "shape=swing live=1Gi ops=4800M phases=24 samples=144" ;;
    large) echo "shape=large live=1Gi ops=2400k phases=16 samples=128" ;;
    *) echo "unknown job $1" >&2; exit 2 ;;
  esac
}

mkdir -p "$out"
for job in $jobs; do
  # Assigned first so that an unknown job stops the matrix: a failure inside `<<<` would not,
  # and the run would go on with the workload's defaults.
  args=$(args_of "$job")
  read -r -a job_args <<< "$args"
  for build in $(printf '%s\n' $builds | shuf); do
    file="$out/$job.$build.txt"
    if [ -f "$file" ] && tail -1 "$file" | grep -q '^# exit 0'; then
      continue
    fi
    echo "$(date -Is) $job $build"
    "$here/run.sh" "$build" "$file" "${job_args[@]}" tables=phase quiet=60 quiet_step=5 "$@" || echo "$(date -Is) $job $build failed: $?"
  done
done
