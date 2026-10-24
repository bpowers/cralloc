#!/usr/bin/env bash
# usage: session.sh OUT SUITE ONLY ALLOCATORS [ROUNDS]
# One measurement session of benchrun on both L3s with every request's record kept:
# results in OUT.jsonl, the load generator's records in OUT.raw/<server pid>.raw. Keep a session
# to about ten minutes and leave a gap as long after it (CLAUDE.md, "How we work").
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
out="$1" suite="$2" only="$3" allocs="$4" rounds="${5:-1}"
mkdir -p "$out.raw"
export BENCH_RAW_DIR="$out.raw"
cd "$bench"
exec "$bench/../scripts/measure" --both-l3 ./target/release/benchrun run --suite "$suite" --only "$only" \
  --allocators "$allocs" --rounds "$rounds" --out "$out.jsonl"
