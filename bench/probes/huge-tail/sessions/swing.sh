#!/usr/bin/env bash
# The lifetime job with phases of a chosen length, so that a class's tail may be returned in
# each low phase and faulted back in each high one, with each arm, interleaved. From the
# checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/swing.sh OUTDIR "MOPS:PHASES ..." "ARMS"
# MOPS is millions of operations a phase: about 34M a second on the suite's eight measurement
# CPUs, so 100 is about 3 s. Two samples a phase.
set -euo pipefail
out="$1" periods="$2" arms="$3"
for p in $periods; do
  mops="${p%%:*}" phases="${p#*:}"
  bench/probes/huge-tail/sessions/kern.sh "$out/p$mops" "${ROUNDS:-1}" "$arms" \
    drift shape=lifetime ops="$((mops * phases))M" phases="$phases" samples="$((phases * 2))" \
    long_permille=5 tables=phase quiet=0
done
