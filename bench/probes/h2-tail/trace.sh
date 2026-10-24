#!/usr/bin/env bash
# usage: trace.sh OUT ALLOCATOR [AT [DUR [SUITE ONLY]]]
# One benchrun run of the Connect h2 tail job (suite h2-tail) with every request's record, and
# from AT seconds into the load plan (default 18: inside the one-second pieces) DUR seconds
# (default 4) of perf on both sides' CPUs (0-3 the server, 8-11 the load generator) into
# OUT.srv.data, with timestamps on CLOCK_MONOTONIC so they line up with the records (the
# generator's start_mono_ns plus each due time). Results in OUT.jsonl and OUT.raw/. Run from
# bench/ under scripts/measure --both-l3.
set -euo pipefail
out="$1" alloc="$2" at="${3:-18}" dur="${4:-4}" suite="${5:-h2-tail}" only="${6:-=connect log h2 tail}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out.raw"
export BENCH_RAW_DIR="$out.raw"
before=$(pgrep -f "$bench/target/loadgen/release/loadgen" || true)
"$bench/target/release/benchrun" run --suite "$suite" --only "$only" --allocators "$alloc" \
  --rounds 1 --out "$out.jsonl" > "$out.log" 2>&1 &
runner=$!
gen=""
for _ in $(seq 1 6000); do
  gen=$(pgrep -n -f "$bench/target/loadgen/release/loadgen" || true)
  [ -n "$gen" ] && [ "$gen" != "$before" ] && break
  gen=""
  sleep 0.01
done
[ -n "$gen" ] || { echo "trace: no load generator seen" >&2; wait $runner; exit 1; }
t0=$(date +%s.%N)
# The idle-state counters of both sides' CPUs every 5 ms over the whole plan (cstate.py).
taskset -c 14 python3 -I "$here/cstate.py" "$out.cstate.jsonl" "${TRACE_PLAN_S:-45}" &
cs=$!
# The server's system-call and scheduling counters (procs.py); its pid names the generator's
# record file, `--raw DIR/<pid>.raw`.
spid=$(tr '\0' '\n' < "/proc/$gen/cmdline" | grep -A1 -x -- --raw | tail -1 | xargs -r basename | sed 's/\.raw$//')
pr=""
if [ -n "$spid" ]; then
  taskset -c 14 python3 -I "$here/procs.py" "$out.procs.jsonl" "${TRACE_PLAN_S:-45}" "$spid" &
  pr=$!
fi
# TRACE_SPIN=1: a SCHED_IDLE spinner on each server CPU for the plan (spin.py), started only now
# so that the runner's quiet gate has passed. The runner then marks the run invalid for other
# work on the server's CPUs, which is this.
spins=()
if [ "${TRACE_SPIN:-0}" = 1 ]; then
  for c in 0 1 2 3; do
    python3 -I "$here/spin.py" "$c" "${TRACE_PLAN_S:-45}" &
    spins+=($!)
  done
fi
sleep "$(python3 -c "import sys; print(max(0, float(sys.argv[1]) + float(sys.argv[2]) - float(sys.argv[3])))" "$t0" "$at" "$(date +%s.%N)")"
# perf itself runs on CPU 14, off both sides' CPUs. TRACE_MODE=switch (the default) records
# context switches only, with the dummy event, which takes no hardware counter: a sampling
# event beside the runner's own counters is multiplexed every millisecond, and that rotation
# on every tick slowed every allocator's requests (docs/research/h2-tail.md). TRACE_MODE=cycles
# samples as well.
if [ "${TRACE_MODE:-switch}" = cycles ]; then
  ev=(-e cycles -c 50000 -g)
else
  ev=(-e dummy)
fi
taskset -c 14 perf record -q -o "$out.srv.data" -a -C 0-3,8-11 "${ev[@]}" --switch-events -k mono \
  -m 8M -- sleep "$dur" > /dev/null 2>&1 &
p1=$!
wait $p1 || true
wait $cs || true
[ -z "$pr" ] || wait "$pr" || true
for p in "${spins[@]}"; do kill "$p" 2>/dev/null || true; wait "$p" 2>/dev/null || true; done
wait $runner
echo "gen $gen at $at dur $dur" > "$out.meta"
