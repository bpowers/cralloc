#!/usr/bin/env bash
# The kernel's charges (populate.c), all three modes, with the machine's state before each.
# From the checkout, after building the probe into target/ft:
#   cc -O2 -pthread -o target/ft/populate bench/probes/first-touch/populate.c
#   scripts/measure --both-l3 bench/probes/first-touch/kernel.sh OUTDIR
set -euo pipefail
out="$1"
mkdir -p "$out"
bin="$(pwd)/target/ft/populate"
state() {
  # Free blocks of order 9 and up in the Normal zone, and the THP and compaction counters.
  awk '/Normal/ { s = 0; for (i = 14; i <= NF; i++) s += $i; print "free_order9plus", s }' /proc/buddyinfo
  grep -E '^(thp_fault_alloc|thp_fault_fallback|compact_stall) ' /proc/vmstat
}
run() {
  local name="$1"
  shift
  { echo "# $(date -Is) $*"; state; } > "$out/$name.txt"
  scripts/memlimit -m 2G -t 10m -- "$@" >> "$out/$name.txt" 2>> "$out/$name.err"
  state >> "$out/$name.txt"
}
run cost taskset -c 2 "$bin" cost 2 "${ROUNDS:-15}"
run cost512 taskset -c 2 "$bin" cost 2 "${ROUNDS:-15}" 512
run hold taskset -c 2 "$bin" hold 2
run neigh "$bin" neigh 2 3,4,5,6,7
