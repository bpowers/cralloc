#!/usr/bin/env bash
# The mini-redis runs of docs/research/heap-drift.md: redis.py once per build, one at a time,
# in a random order, the live-bytes build (the denominator) among them. Run from the
# repository root under scripts/heavy, after requests.py has written the inputs:
#
#   scripts/heavy bench/probes/heap-drift/redis-matrix.sh OUTDIR "BUILD..." [redis.py options]
#
# A measurement on both L3s freezes heavy work, the server and the load generator with it,
# but the generator's clock runs on, so a phase it falls in is cut short and its requests
# fail. Such a run is kept aside as OUTDIR/frozen/redis.BUILD.N.jsonl and made again, up to
# four tries.
set -euo pipefail
out="$1" builds="$2"
shift 2
here="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$out/frozen"
for build in $(printf '%s\n' $builds | shuf); do
  file="$out/redis.$build.jsonl"
  for try in 1 2 3 4; do
    if [ -f "$file" ] && tail -1 "$file" | grep -q '"ok": true'; then
      break
    fi
    [ -f "$file" ] && mv "$file" "$out/frozen/redis.$build.$(date +%s).jsonl"
    echo "$(date -Is) redis $build try $try"
    python3 -I "$here/redis.py" "$build" "$file" "$@" || echo "$(date -Is) redis $build failed"
  done
done
