#!/usr/bin/env bash
# The long drift run of the problem statement (lifetime, 24 phases, the wandering size mix
# and the fixed one) with each arm, one run at a time, memory only. From the checkout:
#   scripts/heavy bench/probes/huge-tail/sessions/long.sh OUTDIR TABLES "ARMS"
# ARMS: name=BUILD:ASMALLOC_HT, e.g. "off=ht2:t0 on=ht2:t1d1 je=je:- glibc=glibc:-".
set -euo pipefail
out="$1" tables="$2" arms="$3"
mkdir -p "$out"
for arm in $(printf '%s\n' $arms | shuf); do
  name="${arm%%=*}" rest="${arm#*=}"
  build="${rest%%:*}" ht="${rest#*:}"
  file="$out/$name.$tables.txt"
  if [ -f "$file" ] && tail -1 "$file" | grep -q '^# exit 0'; then
    continue
  fi
  bench/probes/huge-tail/drift.sh "$build" "$file" ASMALLOC_HT="$ht" -- \
    shape=lifetime ops=24G phases=24 samples=144 long_permille=1 tables="$tables" quiet=60 quiet_step=5 \
    || echo "$(date -Is) $arm failed: $?"
done
