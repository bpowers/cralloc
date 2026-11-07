#!/usr/bin/env bash
# usage: session.sh OUT SUITE ONLY ALLOCATORS [benchrun run's other options...]
# One measurement session: one round of the jobs ONLY of SUITE under ALLOCATORS (comma lists, as
# benchrun takes them), on both L3s, results appended to OUT. queue.sh runs these one at a time
# with a gap after each; keep each to about ten minutes.
set -euo pipefail
W=$(cd "$(dirname "$0")/../.." && pwd)
out="$1" suite="$2" only="$3" allocs="$4"
shift 4
mkdir -p "$(dirname "$out")"
cd "$W/bench"
exec "$W/scripts/measure" --both-l3 ./target/release/benchrun run --suite "$suite" --only "$only" \
  --allocators "$allocs" --rounds 1 --out "$out" "$@"
