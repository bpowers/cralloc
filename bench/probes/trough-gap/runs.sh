#!/usr/bin/env bash
# Drift runs for docs/research/trough-gap.md, one at a time, the builds in a random order. Each
# writes OUTDIR/lifetime-RUNG.BUILD.txt (so that large-heap-constants/summary.py reads it) and its
# watch file; a run whose file already ends cleanly is skipped.
#
#   scripts/heavy bench/probes/trough-gap/runs.sh OUTDIR JOB "BUILD..."                (memory)
#   TG_TIMED=1 scripts/measure --both-l3 bench/probes/trough-gap/runs.sh OUTDIR JOB "BUILD..."
#
# JOB:
#   life2    the lifetime job of large-heap-constants at 2 GiB: six phases of 1.2G operations
#   short2   the same heap with 24 phases of 100M operations, about 4 s each: what one phase
#            frees the next takes again
#   small    a small heap: short-lived values swinging between 64 and 5 MiB beside 16 MiB of
#            long-lived ones, eight phases of about ten seconds (rung 0.08)
# BUILD is a directory of bench/target. `NAME-stat-acct` is the instrumented build NAME-stat
# (asmalloc's, from tg_edit.py) or `je-stat` (jemalloc) with its account taken at every phase
# end (ASMALLOC_MEMSTAT / JEMALLOC_ACCT, written beside the run's file).
# TG_TIMED=1 runs on the measurement CPUs (0-3 and 8-11); otherwise on the heavy slice.
set -euo pipefail
out="$1" job="$2" builds="$3"
here="$(cd "$(dirname "$0")" && pwd)"
lhc="$here/../large-heap-constants"
case "$job" in
  life5) rung=5; args="shape=lifetime short_hi=4045Mi short_lo=340Mi long=1025Mi phases=6 ops=7200M samples=156 quiet=60 quiet_step=1"; every=26 ;;
  life2) rung=2; args="shape=lifetime short_hi=1618Mi short_lo=136Mi long=410Mi phases=6 ops=7200M samples=156 quiet=60 quiet_step=1"; every=26 ;;
  short2) rung=2; args="shape=lifetime short_hi=1618Mi short_lo=136Mi long=410Mi phases=24 ops=2400M samples=96 quiet=20 quiet_step=1"; every=4 ;;
  # short2 and small as their phases last on the measurement CPUs, which run them three times
  # as fast as the heavy slice: about 4 s and 10 s.
  short2t) rung=2; args="shape=lifetime short_hi=1618Mi short_lo=136Mi long=410Mi phases=24 ops=7200M samples=96 quiet=20 quiet_step=1"; every=4 ;;
  smallt) rung=0.08; args="shape=lifetime short_hi=64Mi short_lo=5Mi long=16Mi phases=8 ops=8400M samples=96 quiet=30 quiet_step=1"; every=12 ;;
  # The steady cache of large-heap-constants at 2 GiB: a heap with little fragmentation.
  steady2) rung=2; args="shape=steady live=2048Mi ops=800M samples=60 quiet=20 quiet_step=1"; every=15 ;;
  small) rung=0.08; args="shape=lifetime short_hi=64Mi short_lo=5Mi long=16Mi phases=8 ops=2400M samples=96 quiet=30 quiet_step=1"; every=12 ;;
  *) echo "unknown job $job" >&2; exit 2 ;;
esac
[ -n "${TG_TIMED:-}" ] && export BENCH_CPUS=0-3,8-11
mkdir -p "$out"
for spec in $(printf '%s\n' $builds | shuf); do
  # SPEC is NAME, or NAME@VALUE: the build NAME with TG_REFRESH=VALUE (refresh_switch.py),
  # whose files are named NAME-VALUE.
  build="${spec%@*}" env=() extra=()
  name="$build"
  if [ "$spec" != "$build" ]; then
    env=(--env "TG_REFRESH=${spec#*@}")
    name="$build-${spec#*@}"
  fi
  file="$out/lifetime-$rung.$name.txt"
  if [ -f "$file" ] && tail -1 "$file" | grep -q '^# exit 0'; then
    continue
  fi
  case "$build" in
    je-stat-acct)
      build=je-stat
      rm -f "$file.je"
      env+=(--env "JEMALLOC_ACCT=$file.je")
      extra=("account_every=$every") ;;
    *-stat-acct)
      build="${build%-acct}"
      env+=(--env "ASMALLOC_MEMSTAT=$file.memstat" --env ASMALLOC_MEMSTAT_MS=0 --env ASMALLOC_MEMSTAT_CPU=15 --env ASMALLOC_MEMSTAT_THP=1)
      extra=("account_every=$every") ;;
  esac
  echo "$(date -Is) $job $name"
  python3 -I "$lhc/drift.py" "$build" "$file" --mem 16G "${env[@]}" -- $args "${extra[@]}" \
    || echo "$(date -Is) $job $name failed: $?"
done
