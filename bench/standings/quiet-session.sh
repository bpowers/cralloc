#!/usr/bin/env bash
# usage: quiet-session.sh [--rounds N] [--only "=JOB,=JOB"] [--allocators LIST] NAME
#
# Every server job under every allocator, with a log of every context switch on the machine, for a
# session whose tails are to be read. Tails on a shared machine follow short runs of other work that
# no limit on CPU time can see (docs/research/measuring.md, "On the M1 Max"); the log shows which
# runs had none.
#
# Before it: build at the commit to measure (`benchrun build`, `benchrun build-apps`, under
# scripts/heavy), turn off deep idle, and clear the machine: no team, no browser, no agent working,
# no large writes just before.
#
# It checks what it can and says so here, then detaches and returns at once, so that nothing need run
# or poll until it exits. It writes to memory while it runs, because a write to the disk wakes kernel
# threads on the server's CPUs, and at the end moves everything to bench/results/NAME of the main
# checkout: the results, the logs and session.log, whose last line is its exit status. While it runs,
# session.log is in /tmp/cralloc-quiet-NAME; `kill -- -$(cat session.pid)` from there stops it and moves
# what it has. Then `scripts/heavy bench/standings/whoran.py DIR` says what ran and which runs were
# quiet, and `standings/servers.py` reads the results.
#
# About 47 minutes a round. Each job and round is its own piece under `scripts/measure --both-l3`.
set -uo pipefail
W=$(cd "$(dirname "$0")/../.." && pwd)
args=("$@")
rounds=3
only=""
allocators="cralloc,jemalloc,tcmalloc-percpu,mimalloc,glibc,snmalloc"
while [ $# -gt 1 ]; do
  case "$1" in
    --rounds) rounds="$2"; shift 2 ;;
    --only) only="$2"; shift 2 ;;
    --allocators) allocators="$2"; shift 2 ;;
    *) echo "quiet-session: unknown option $1" >&2; exit 2 ;;
  esac
done
name="${1:?usage: quiet-session.sh [--rounds N] [--only LIST] [--allocators LIST] NAME}"
common=$(cd "$(git -C "$W" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)
final="$common/bench/results/$name"
memory="${CRALLOC_QUIET_DIR:-/tmp/cralloc-quiet-$name}"
benchrun="$W/bench/target/release/benchrun"
log="$memory/session.log"

if [ -z "${QUIET_DETACHED:-}" ]; then
  fail() { echo "quiet-session: $1" >&2; exit 3; }
  [ -x "$benchrun" ] || fail "no $benchrun: build first"
  [ ! -e "$final" ] || fail "$final exists: choose another name"
  [ ! -e "$memory" ] || fail "$memory exists: one is running, or a killed one left it"
  mkdir -p "$memory" || fail "cannot make $memory"
  [ "$(stat -f -c %T "$memory")" = tmpfs ] || { rmdir "$memory"; fail "$memory is not in memory: set CRALLOC_QUIET_DIR"; }
  # The logs take about 2.5 GiB a round.
  need=$((rounds * 3))
  have=$(($(stat -f -c '%a * %S' "$memory") / 1073741824))
  [ "$have" -ge "$need" ] || { rmdir "$memory"; fail "$have GiB free in $memory, and $rounds rounds need $need"; }
  rmdir "$memory"
  "$W/scripts/deep-idle" status | grep -q '^every deep state .* is disabled on these CPUs$' \
    || fail "deep idle is on: sudo scripts/deep-idle off"
  # A build older than the allocator's source would measure other code.
  built="$W/bench/target/cralloc/apps/orders/release/orders"
  [ -e "$built" ] || fail "no $built: build first"
  [ -z "$(find "$W/crates/cralloc" -type f -newer "$built" -print -quit)" ] \
    || fail "crates/cralloc is newer than the builds: build first"
  check=$(mktemp -p /tmp quiet-check.XXXXXX)
  perf record -q -o "$check" -a -k CLOCK_MONOTONIC --switch-events -e dummy -z -- sleep 0.1 >/dev/null 2>&1 \
    || { rm -f "$check"; fail "perf cannot log context switches here: see perf_event_paranoid"; }
  rm -f "$check"
  QUIET_DETACHED=1 setsid nohup "$0" "${args[@]}" >/dev/null 2>&1 </dev/null &
  echo "started: $rounds rounds, about $((rounds * 47)) minutes. Log $log; results $final"
  exit 0
fi

mkdir -p "$memory"
exec >"$log" 2>&1
echo $$ >"$memory/session.pid"
now() { date '+%F %T'; }
echo "$(now) start: commit $(git -C "$W" rev-parse --short HEAD), $rounds rounds, $allocators"
"$W/scripts/deep-idle" status

if [ -n "$only" ]; then
  IFS=, read -ra jobs <<<"$only"
else
  mapfile -t jobs < <("$benchrun" list --suite servers 2>/dev/null | sed -E 's/^servers +native +(.*[^ ]) +cpus .*$/=\1/')
fi

finish() {
  echo "$(now) moving to $final"
  mkdir -p "$(dirname "$final")"
  cp -r "$memory" "$final" && rm -rf "$memory"
  echo "$(now) exit $1" >>"$final/session.log"
  exit "$1"
}
trap 'finish 130' INT TERM HUP

# Whatever is still dirty would be written back by kernel threads on the server's CPUs, for tens of
# milliseconds at a time, in the middle of a run.
sync

# One piece, under the measurement lock, so that the log never runs beside someone else's measurement.
# The log runs on an E core, away from the job, and into memory. The gate's wait is short: nothing else
# should be running, and a run that goes ahead beside something shows it in its record and the log.
piece_script='
  taskset -c 0 perf record -q -o "$1.perf" -a -k CLOCK_MONOTONIC --switch-events -e dummy -z -m 4096 \
    -- sleep 100000 &
  log=$!
  trap "kill -INT $log; wait $log" EXIT
  trap "exit 130" INT TERM HUP
  sleep 1
  "$BENCHRUN" run --suite servers --only "$2" --allocators "$3" --rounds 1 --gate-wait 60 --out "$1.jsonl"
'
export BENCHRUN="$benchrun"

status=0
cd "$W/bench" || finish 3
for round in $(seq 1 "$rounds"); do
  for job in "${jobs[@]}"; do
    slug=$(echo "${job#=}" | tr ' ' '-')
    use="$allocators"
    # This job's rate is set by the others: glibc is half as fast on the Router (suite.rs).
    if [ "$job" = "=router graphql fast" ]; then
      use=$(echo ",$allocators," | sed 's/,glibc,/,/; s/^,//; s/,$//')
    fi
    echo "$(now) round $round: $job"
    "$W/scripts/measure" --both-l3 bash -c "$piece_script" piece "$memory/r$round-$slug" "$job" "$use" || status=1
  done
done
finish "$status"
