#!/usr/bin/env bash
# The breakdowns of docs/research/heap-drift.md, one run at a time with the memstat build: the
# lifetime mix (half the matrix's length, a breakdown at the end of every phase, with the unit
# maps), one cycle of the size-mix shift and of the large mix (a breakdown at each phase end),
# then the mini-redis runs. Run from the repository root under scripts/heavy:
#
#   scripts/heavy bench/probes/heap-drift/accounts.sh OUTDIR
set -euo pipefail
out="$1"
here="$(cd "$(dirname "$0")" && pwd)"
"$here/matrix.sh" "$out/lifetime" lifetime asmalloc-memstat ops=12G phases=12 samples=72 account_every=6
"$here/matrix.sh" "$out/shift" shift asmalloc-memstat ops=3G phases=3 samples=72 account_every=24
"$here/matrix.sh" "$out/large" large asmalloc-memstat ops=600k phases=4 samples=32 account_every=8
"$here/redis-matrix.sh" "$out/redis" "live-bytes asmalloc je tc mi glibc asmalloc-memstat"
