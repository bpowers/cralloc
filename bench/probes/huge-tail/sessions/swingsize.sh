#!/usr/bin/env bash
# Where the floor stops the rule: the lifetime job with its short-lived set swinging by SWING
# MiB above a trough of 128 MiB (beside the 384 MiB long-lived cache), in 10 s phases, each
# swing size with each arm, interleaved. From the checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/swingsize.sh OUTDIR "SWING..." "ARMS"
set -euo pipefail
out="$1" swings="$2" arms="$3"
for s in $swings; do
  bench/probes/huge-tail/sessions/kern.sh "$out/s$s" "${ROUNDS:-1}" "$arms" \
    drift shape=lifetime ops=2720M phases=8 samples=32 long_permille=5 tables=phase quiet=0 \
    short_lo=$((128 << 20)) short_hi=$(((128 + s) << 20))
done
