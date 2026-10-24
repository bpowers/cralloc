#!/usr/bin/env bash
# usage: rates.sh DIR RATE:ALLOCATOR... : trace.sh for Connect h2 at each rate variant of the
# h2-tail suite (`r68k`; `-` for the ranked 63k), into DIR/NN-RATE-ALLOCATOR. One measurement
# session: run under scripts/measure --both-l3 from bench/.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
dir="$1"
shift
mkdir -p "$dir"
i=0
for e in "$@"; do
  i=$((i + 1))
  r="${e%%:*}"
  a="${e#*:}"
  only="=connect log h2 tail"
  [ "$r" = - ] || only="$only $r"
  "$here/trace.sh" "$dir/$(printf %02d $i)-$r-$a" "$a" "${TRACE_AT:-20}" "${TRACE_DUR:-8}" h2-tail "$only"
done
