#!/usr/bin/env bash
# The product's memory runs, one at a time, not timed: the long lifetime run (wandering and
# fixed tables) and the size shift with main and the product, the incumbents on the shift, and
# mini-redis with all of them. From the checkout, after build.sh made `main` and `prod`
# (HT_EDIT=0 DRIFT=none) and heap-drift's build-miniredis.sh made je, glibc and live-bytes:
#   scripts/heavy bench/probes/huge-tail/sessions/product-memory.sh OUTDIR
set -uo pipefail
out="$1"
mkdir -p "$out"
long="shape=lifetime ops=24G phases=24 samples=144 long_permille=1 quiet=60 quiet_step=5"
shift_args="shape=shift live=2Gi ops=12G phases=12 samples=288 quiet=60 quiet_step=5"
for build in $(printf '%s\n' main prod | shuf); do
  bench/probes/huge-tail/drift.sh "$build" "$out/lifetime.$build.txt" -- $long tables=phase
  bench/probes/huge-tail/drift.sh "$build" "$out/lifetime-shared.$build.txt" -- $long tables=shared
done
for build in $(printf '%s\n' main prod je glibc | shuf); do
  bench/probes/huge-tail/drift.sh "$build" "$out/shift.$build.txt" -- $shift_args
done
bench/probes/heap-drift/redis-matrix.sh "$out/redis" "live-bytes main prod je glibc"
