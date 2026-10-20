#!/usr/bin/env bash
# csmix.sh PROG ROUNDS tag=lib...: interleaved csrec runs, one per library per round.
set -euo pipefail
R="$RUST_SCREEN_ROOT"; S="${APPF_OUT:-/tmp/app-faults}"; T="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prog="$1"; rounds="$2"; shift 2
mkdir -p "$S/cs"
for r in $(seq "$rounds"); do
  for spec in $(printf '%s\n' "$@" | shuf); do
    tag="${spec%%=*}"; lib="${spec#*=}"
    "$R/tools/spin" 0-3,8-11 300
    "$T/run.sh" "$prog" "$lib" perf record -q -e context-switches -c 1 -g -o "$S/cs/$tag-$r.data" \
      taskset -c 0-3,8-11 "$R/tools/walltime" "$S/cs/$tag-$r.wall"
    echo "$tag-$r $(cat "$S/cs/$tag-$r.wall")"
  done
done
