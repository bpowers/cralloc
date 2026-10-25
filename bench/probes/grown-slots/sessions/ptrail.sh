#!/usr/bin/env bash
# The trail probe, the product build against main, each under a 6 GiB cap on CPUs 12-15.
# From the checkout:
#   scripts/heavy bench/probes/grown-slots/sessions/ptrail.sh OUTDIR 'trail args'
set -euo pipefail
out="$1" args="$2"
mkdir -p "$out"
for build in ocm oc; do
  # shellcheck disable=SC2086
  scripts/memlimit -m 6G -t 5m -- taskset -c 12-15 bench/target/$build/trail $args \
    > "$out/$build.txt" 2>&1 || echo "$build: exit $?"
  echo "== $build"; grep '^#' "$out/$build.txt"
done
