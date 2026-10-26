#!/usr/bin/env bash
# One timed round of a shortened large mix for every arm of the switch and for jemalloc and
# glibc, on the suite's measurement CPUs (0-3 and 8-11), in a random order. One round is one
# session of about ten minutes:
#
#   scripts/measure --both-l3 bench/probes/large-churn/timed.sh OUTDIR ROUND "ARM..."
#
# Writes OUTDIR/rROUND/large.ARM.txt (and large.je.txt, large.glibc.txt); cycles and faults
# are in each run's BENCH_RESULT.
set -euo pipefail
out="$1/r$2" arms="$3"
here="$(cd "$(dirname "$0")" && pwd)"
export BENCH_CPUS=0-3,8-11
short=(ops=240k phases=4 samples=32 quiet=0)
runs=()
for arm in $arms; do runs+=("lcs:$arm"); done
runs+=("je:-" "glibc:-")
for r in $(printf '%s\n' "${runs[@]}" | shuf); do
  "$here/arms.sh" "$out" "${r%%:*}" large "${r#*:}" "${short[@]}"
done
