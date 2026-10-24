#!/usr/bin/env bash
# The trail probe under each setting given, one after another, each under a 6 GiB cap, CPUs
# 12-15. From the checkout:
#   scripts/heavy bench/probes/grown-slots/sessions/trail.sh OUTDIR 'trail args' SETTING...
# Writes OUTDIR/<setting>.txt.
set -euo pipefail
out="$1" args="$2"
shift 2
mkdir -p "$out"
for setting in "$@"; do
  # shellcheck disable=SC2086
  rm -f "$out/$setting.diag"
  GS_DIAG="$(pwd)/$out/$setting.diag" ASMALLOC_GROWN="$setting" scripts/memlimit -m 6G -t 5m -- taskset -c 12-15 \
    bench/target/gs/trail $args > "$out/$setting.txt" 2>&1 || echo "$setting: exit $?"
  echo "== $setting"; grep '^#' "$out/$setting.txt"
done
