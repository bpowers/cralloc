#!/usr/bin/env bash
# Runs the binaries build-incumbent.sh made, ROUNDS times each, interleaved, under the
# measurement lock and a memory cap; prints each run's line prefixed by its placement.
#
#   run-incumbent.sh OUT [ROUNDS]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out=$1
rounds=${2:-2}
bins=$(ls "$out" | grep -E '^incumbent-[0-9]+$' | sort -t- -k2 -n)
run() {
  for r in $(seq "$rounds"); do
    for b in $bins; do
      echo "${b#incumbent-} $("$out/$b")"
    done
  done
}
export -f run
export out rounds bins
flock /tmp/cralloc-measure.lock "$root/scripts/memlimit" -m 1G -t 20m -- bash -c run
