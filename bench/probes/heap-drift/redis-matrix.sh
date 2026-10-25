#!/usr/bin/env bash
# The mini-redis runs of docs/research/heap-drift.md: redis.py once per build, one at a time,
# in a random order, the live-bytes build (the denominator) among them. Run from the
# repository root under scripts/heavy, after requests.py has written the inputs:
#
#   scripts/heavy bench/probes/heap-drift/redis-matrix.sh OUTDIR "BUILD..." [redis.py options]
set -euo pipefail
out="$1" builds="$2"
shift 2
here="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$out"
for build in $(printf '%s\n' $builds | shuf); do
  file="$out/redis.$build.jsonl"
  if [ -f "$file" ] && tail -1 "$file" | grep -q '"ok": true'; then
    continue
  fi
  echo "$(date -Is) redis $build"
  python3 -I "$here/redis.py" "$build" "$file" "$@" || echo "$(date -Is) redis $build failed: $?"
done
