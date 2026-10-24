#!/usr/bin/env bash
# DataFusion (the apps suite's TPC-H SF3 job) under the given allocators, ROUNDS rounds.
# From the checkout: scripts/measure --both-l3 bench/probes/grown-slots/sessions/apps.sh OUT.jsonl ALLOCATORS
set -euo pipefail
cd bench
exec target/release/benchrun run --suite apps --only "datafusion tpch sf3 t4" \
  --allocators "$2" --rounds "${ROUNDS:-1}" --out "../$1"
