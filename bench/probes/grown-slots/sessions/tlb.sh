#!/usr/bin/env bash
# The HashMap job with dTLB misses and page walks by page size counted, asmalloc against
# mimalloc and jemalloc. From the checkout:
#   scripts/measure bench/probes/grown-slots/sessions/tlb.sh OUT.jsonl
set -euo pipefail
B="$(pwd)/bench/target"
export BENCH_EVENTS=dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045
exec python3 -I bench/probes/grown-slots/kern.py "$1" --rounds "${ROUNDS:-3}" \
  --jobs 'map kind=hash n=1M reps=5' \
  --arms "asm=$B/asm/release/workload,mi=$B/mi/release/workload,je=$B/je/release/workload"
