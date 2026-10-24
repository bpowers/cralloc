#!/usr/bin/env bash
# The HashMap job: the switch's arms in one binary, and mimalloc, tcmalloc and jemalloc, with
# dTLB misses and walks counted. From the checkout:
#   scripts/measure bench/probes/whole-block-huge/sessions/kern.sh OUT.jsonl
set -euo pipefail
B="$(pwd)/bench/target"
exec python3 -I bench/probes/whole-block-huge/kern.py "$1" --rounds "${ROUNDS:-5}" \
  --jobs "${JOBS:-map kind=hash n=1M reps=5}" \
  --arms "main=wbh:m0f002d0,all2=wbh:m1f002d0,nz2=wbh:m2f002d0,all16=wbh:m1f016d0,mi=$B/mi/release/workload,tc=$B/tc/release/workload,je=$B/je/release/workload"
