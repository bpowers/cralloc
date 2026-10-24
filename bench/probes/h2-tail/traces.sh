#!/usr/bin/env bash
# usage: traces.sh DIR ALLOCATOR[:spin]... : trace.sh once for each, in turn, into
# DIR/NN-ALLOCATOR[-spin]; `:spin` runs it with TRACE_SPIN=1 (idle spinners on the server's
# CPUs). One measurement session: run under scripts/measure --both-l3 from bench/.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
dir="$1"
shift
mkdir -p "$dir"
i=0
for e in "$@"; do
  i=$((i + 1))
  a="${e%:spin}"
  spin=0
  tag="$a"
  if [ "$a" != "$e" ]; then spin=1; tag="$a-spin"; fi
  TRACE_SPIN=$spin "$here/trace.sh" "$dir/$(printf %02d $i)-$tag" "$a" "${TRACE_AT:-18}" "${TRACE_DUR:-4}" \
    "${TRACE_SUITE:-h2-tail}" "${TRACE_ONLY:-=connect log h2 tail}"
done
