#!/usr/bin/env bash
# Kernel jobs: plain main, the switch at main, keep, never, and three incumbents.
# From the checkout: scripts/measure bench/probes/grown-slots/sessions/k1.sh OUT.jsonl
set -euo pipefail
B="$(pwd)/bench/target"
exec python3 -I bench/probes/grown-slots/kern.py "$1" --rounds "${ROUNDS:-3}" \
  --jobs 'vecgrow vecs=64 max=4Mi reps=4;map kind=hash n=1M reps=5;json size=32Mi reps=5;vecgrow vecs=1 max=256Mi reps=4' \
  --arms "asm=$B/asm/release/workload,main=gs:m0b0016s00r0d0,keep=gs:m2b0016s00r0d0,never=gs:m3b0016s00r0d0,je=$B/je/release/workload,mi=$B/mi/release/workload,glibc=$B/glibc/release/workload"
