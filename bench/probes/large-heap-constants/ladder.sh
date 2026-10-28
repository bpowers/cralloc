#!/usr/bin/env bash
# The memory ladder of docs/research/large-heap-constants.md: each drift job at each live size
# with each build, one run at a time, the builds in a new random order for each job and size.
# Each run writes OUTDIR/JOB-RUNG.BUILD.txt and its watch file (drift.py); a run whose file
# already ends cleanly is skipped, so a ladder that was stopped resumes. Run under
# scripts/heavy (memory runs; not timed):
#
#   scripts/heavy bench/probes/large-heap-constants/ladder.sh OUTDIR "JOB..." "RUNG..." "BUILD..."
#
# JOB is steady, lifetime or large; RUNG the live size in GiB (0.5, 2, 5, 10); BUILD a
# directory of bench/target, or asmalloc-diag:off for the diagnostic build with its
# background thread off (ASMALLOC_BACKGROUND=0). Before each run it waits until no other
# process on the machine holds more than 4 GiB resident (compilers aside), so that two
# memory-heavy jobs do not run at once.
set -euo pipefail
out="$1" jobs="$2" rungs="$3" builds="$4"
here="$(cd "$(dirname "$0")" && pwd)"

source "$here/args.sh"

# Waits while another process holds more than LIMIT GiB resident, so that two memory-heavy jobs
# do not run at once. A compiler or linker is not counted: it runs no allocator, and a
# teammate's link of several GiB would otherwise hold the ladder for hours.
wait_for_memory() {
  local limit="$1"
  while :; do
    big=$(ps -eo rss=,pid=,comm= | awk -v l="$limit" '$1 > l*1024*1024 && $3 !~ /^(rustc|ld|ld\.lld|cc1plus|cc1)$/ { print $2 " " $3 " " int($1/1024) "MiB" }')
    [ -z "$big" ] && return
    echo "$(date -Is) waiting for: $big"
    sleep 30
  done
}

mkdir -p "$out"
for job in $jobs; do
  for g in $rungs; do
    read -r -a job_args <<< "$(args_of "$job" "$g")"
    for spec in $(printf '%s\n' $builds | shuf); do
      build="${spec%%:*}"
      env=()
      name="$build"
      if [ "$spec" != "$build" ]; then
        name="$build-${spec#*:}"
        case "${spec#*:}" in
          off) env=(--env ASMALLOC_BACKGROUND=0) ;;
          *) echo "unknown variant $spec" >&2; exit 2 ;;
        esac
      fi
      file="$out/$job-$g.$name.txt"
      if [ -f "$file" ] && tail -1 "$file" | grep -q '^# exit 0'; then
        continue
      fi
      wait_for_memory 4
      echo "$(date -Is) $job $g $name"
      python3 -I "$here/drift.py" "$build" "$file" --mem 16G "${env[@]}" -- \
        "${job_args[@]}" quiet=60 quiet_step=1 || echo "$(date -Is) $job $g $name failed: $?"
    done
  done
done
