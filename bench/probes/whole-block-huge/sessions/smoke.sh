#!/usr/bin/env bash
# A check, not a measurement, that the switch acts: the sparse probe and the HashMap job under
# main and the 2 MiB arm, with the counters. From the checkout, under scripts/heavy:
#   scripts/heavy bench/probes/whole-block-huge/sessions/smoke.sh OUTDIR
set -euo pipefail
out="$1"
mkdir -p "$out"
B="$(pwd)/bench/target/wbh"
for s in m0f002d1 m1f002d1 m2f002d1; do
  for z in 0 1; do
    ASMALLOC_WBH=$s WBH_DIAG="$out/sparse-$s-z$z.diag" scripts/memlimit -m 2G -t 2m -- \
      "$B/sparse" case=random size=64 blocks=2 touch=64 lives=2 zero=$z
  done
  ASMALLOC_WBH=$s WBH_DIAG="$out/map-$s.diag" BENCH_CPUS=12 BENCH_EVENTS=dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045 \
    scripts/memlimit -m 2G -t 2m -- "$B/release/workload" map kind=hash n=1M reps=1 | cut -c1-600
done
