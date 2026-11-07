#!/usr/bin/env bash
# Runs one drift workload (bench/workloads/src/drift.rs) with one build of the workload binary,
# under a memory cap, and writes its output to a file. Run under scripts/heavy:
#
#   scripts/heavy bench/standings/drift/run.sh BUILD OUTFILE [key=value ...]
#
# BUILD is a directory under bench/target of this checkout (glibc, je, mi, tc, cralloc). The
# tcmalloc build gets the environment its entry in allocators.toml gives it (glibc's rseq
# registration off, so that tcmalloc can use per-CPU caches).
#   HEAP_DRIFT_MEM      the cap, default 8G (never above 16G)
#   HEAP_DRIFT_TIMEOUT  default 90m
set -euo pipefail
build="$1" out="$2"
shift 2
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
exe="$root/bench/target/$build/release/workload"
[ -x "$exe" ] || { echo "no workload binary at $exe" >&2; exit 2; }
env=()
[ "$build" = tc ] && env+=(GLIBC_TUNABLES=glibc.pthread.rseq=0)
mkdir -p "$(dirname "$out")"
{
  echo "# $(date -Is) build=$build exe=$(readlink -f "$exe") commit=$(git -C "$root" rev-parse --short HEAD)"
  echo "# args: $*"
} > "$out"
status=0
MEMLIMIT_QUIET=0 env "${env[@]}" "$root/scripts/memlimit" -m "${HEAP_DRIFT_MEM:-8G}" -t "${HEAP_DRIFT_TIMEOUT:-90m}" -- \
  "$exe" drift "$@" >> "$out" 2>&1 || status=$?
echo "# exit $status $(date -Is)" >> "$out"
exit "$status"
