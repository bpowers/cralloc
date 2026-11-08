#!/usr/bin/env bash
# The suite's jobs under variants build.sh made, in COPY, through the scripts' lock:
#
#   jobs.sh COPY OUT 'JOB,JOB' ROUNDS VARIANT...     writes the runner's results to OUT
#
# The variants of one size (plain alone, chainK with freeK) are one placement, so they compare
# directly; plain and `kept` do not, and need build.sh's pads.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
scripts=$(cd "$here/../../../../scripts" && pwd)
root=$(cd "$here/../../../.." && pwd)
copy=$1
out=$2
only=$3
rounds=$4
shift 4
common=$(cd "$root" && git rev-parse --path-format=absolute --git-common-dir)
export BENCH_BASELINES=${BENCH_BASELINES:-$(dirname "$common")/bench/baselines/out}
export BENCH_APPS_DATA=${BENCH_APPS_DATA:-$(dirname "$common")/bench/apps/target}
allocs=
for v in "$@"; do
  # =NAME is an allocator of the suite, which `benchrun build` built in COPY.
  case $v in =*) allocs+="${v#=}," ;; *) allocs+="cralloc-$v," ;; esac
done
cd "$copy/bench"
"$scripts/measure" --both-l3 target/runner/release/benchrun run --suite "${BENCH_SUITE:-full}" --only "$only" \
  --allocators "${allocs%,}" --rounds "$rounds" --out "$out"
