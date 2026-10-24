#!/usr/bin/env bash
# Kernel jobs under the ranked rules, one binary, against mimalloc.
# From the checkout: scripts/measure bench/probes/grown-slots/sessions/k3.sh OUT.jsonl
set -euo pipefail
B="$(pwd)/bench/target"
exec python3 -I bench/probes/grown-slots/kern.py "$1" --rounds "${ROUNDS:-3}" \
  --jobs 'vecgrow vecs=64 max=4Mi reps=4;vecgrow vecs=1 max=256Mi reps=4;vecgrow vecs=1 max=4Mi reps=1000;map kind=hash n=1M reps=5;json size=32Mi reps=5' \
  --arms "main=gs:m0b0016s00r0d0,own64=gs:m1b0064s00r0d0,fown=gs:m4b0016s00r0d0,fown64=gs:m4b0064s00r0d0,keep=gs:m2b0016s00r0d0,mi=$B/mi/release/workload"
