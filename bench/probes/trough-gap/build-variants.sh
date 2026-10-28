#!/usr/bin/env bash
# The variants of docs/research/trough-gap.md, each two builds: NAME (the diagnostic counters,
# for time and memory) and NAME-stat (also the probe's accounts, tg_edit.py). Run under
# scripts/heavy from the repository root:
#
#   scripts/heavy bench/probes/trough-gap/build-variants.sh NAME=FILE:CONST=VALUE[,...] ...
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
for spec in "$@"; do
  name="${spec%%=*}"
  consts="${spec#*=}"
  TG_CONST="$consts" "$here/build.sh" "$name" HEAD asmalloc \
    bench/probes/large-heap-constants/diag_edit.py const_edit.py || echo "FAILED $name"
  TG_CONST="$consts" "$here/build.sh" "$name-stat" HEAD asmalloc-memstat \
    bench/probes/large-heap-constants/diag_edit.py const_edit.py tg_edit.py || echo "FAILED $name-stat"
done
