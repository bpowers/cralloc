#!/usr/bin/env bash
# The product build's measurements, one session at a time with a gap as long as the session
# after each (at most ten minutes), as the machine's rules ask. From the checkout:
#   bench/probes/grown-slots/sessions/chain.sh OUTDIR
# Each line of the plan below is one session.
set -uo pipefail
out="$1"
mkdir -p "$out"
s=bench/probes/grown-slots/sessions
run() {
  local name="$1"
  shift
  local t0
  t0=$(date +%s)
  echo "$(date +%T) start $name"
  "$@" > "$out/$name.log" 2>&1
  echo "$(date +%T) end $name rc=$?"
  local ran=$(($(date +%s) - t0))
  [ "$ran" -gt 600 ] && ran=600
  sleep "$ran"
}
run store1 scripts/measure --both-l3 $s/servers.sh "$out/store1.jsonl" gs-main,gs-fown,gs-fown64 servers-large "=connect store 1M"
run kern scripts/measure $s/pk.sh "$out/kern.jsonl"
run tpch scripts/measure --both-l3 $s/ptpch.sh "$out/tpch.jsonl"
run df env ROUNDS=6 scripts/measure --both-l3 $s/apps.sh "$out/df.jsonl" ocm,oc
run meili scripts/measure --both-l3 $s/servers.sh "$out/meili.jsonl" ocm,oc meili-tail
run store2 scripts/measure --both-l3 $s/servers.sh "$out/store2.jsonl" gs-fown64,gs-main,gs-fown servers-large "=connect store 1M"
run v256a env ROUNDS=3 scripts/measure $s/pk.sh "$out/v256a.jsonl" "vecgrow vecs=1 max=256Mi reps=4"
run orders scripts/measure --both-l3 $s/servers.sh "$out/orders.jsonl" ocm,oc servers-thread "=orders axum cache 2G thread"
run store3 scripts/measure --both-l3 $s/servers.sh "$out/store3.jsonl" gs-fown,gs-fown64,gs-main servers-large "=connect store 1M"
run v256b env ROUNDS=3 scripts/measure $s/pk.sh "$out/v256b.jsonl" "vecgrow vecs=1 max=256Mi reps=4"
run meili2 scripts/measure --both-l3 $s/servers.sh "$out/meili2.jsonl" oc,ocm meili-tail
run v256c env ROUNDS=3 scripts/measure $s/pk.sh "$out/v256c.jsonl" "vecgrow vecs=1 max=256Mi reps=4"
echo "$(date +%T) chain done"
