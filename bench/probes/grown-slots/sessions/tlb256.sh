#!/usr/bin/env bash
# One buffer grown to 256 MiB, four times, with dTLB misses and walks counted: main against
# the rules that keep a grown block's slot at its free. From the checkout:
#   scripts/measure bench/probes/grown-slots/sessions/tlb256.sh OUT.jsonl
set -euo pipefail
export BENCH_EVENTS=dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045
exec python3 -I bench/probes/grown-slots/kern.py "$1" --rounds "${ROUNDS:-3}" --diag \
  --jobs 'vecgrow vecs=1 max=256Mi reps=4' \
  --arms "main=gs:m0b0016s00r0d1,fown64=gs:m4b0064s00r0d1,keep=gs:m2b0016s00r0d1"
