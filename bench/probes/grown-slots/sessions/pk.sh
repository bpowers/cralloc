#!/usr/bin/env bash
# Kernel jobs, the product build (bench/target/oc, build-plain.sh) against main (ocm).
# From the checkout: scripts/measure bench/probes/grown-slots/sessions/pk.sh OUT.jsonl [JOBS]
set -euo pipefail
jobs="${2:-vecgrow vecs=64 max=4Mi reps=4;vecgrow vecs=1 max=256Mi reps=4;map kind=hash n=1M reps=5;json size=32Mi reps=5}"
exec python3 -I bench/probes/grown-slots/kern.py "$1" --rounds "${ROUNDS:-3}" \
  --jobs "$jobs" --arms "main=ocm:-,rule=oc:-"
