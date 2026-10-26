#!/usr/bin/env bash
# One timed round of a shortened large mix for every arm of the switch, with BUILD (default lsr),
# and once with OTHER if given (another build dir, run with arm -), on the suite's measurement
# CPUs (0-3 and 8-11), in a random order:
#
#   scripts/measure --both-l3 bench/probes/large-slack/timed.sh OUTDIR ROUND "ARM..." [BUILD [OTHER]]
#
# Writes OUTDIR/rROUND/large.ARM.txt (and large.OTHER.txt); cycles and faults
# are in each run's BENCH_RESULT.
set -euo pipefail
out="$1/r$2" arms="$3"
here="$(cd "$(dirname "$0")" && pwd)"
export BENCH_CPUS=0-3,8-11
short=(ops=240k phases=4 samples=32 quiet=0)
runs=()
build="${4:-lsr}"
for arm in $arms; do runs+=("$build:$arm"); done
[ -n "${5:-}" ] && runs+=("$5:-")
for r in $(printf '%s\n' "${runs[@]}" | shuf); do
  "$here/arms.sh" "$out" "${r%%:*}" large "${r#*:}" "${short[@]}"
done
