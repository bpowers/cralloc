#!/usr/bin/env bash
# The log store as the first note measured it (servers-thread), three rounds of the switch
# binary's arms in rotating order, and two more sessions of the 256 MiB growth. A gap as long
# as each session after it. From the checkout:
#   bench/probes/grown-slots/sessions/chain3.sh OUTDIR
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
only="=connect store 1M thread"
run tstore1 scripts/measure --both-l3 $s/servers.sh "$out/tstore1.jsonl" gs-main,gs-fown,gs-fown64 servers-thread "$only"
run v256d env ROUNDS=5 scripts/measure $s/pk.sh "$out/v256d.jsonl" "vecgrow vecs=1 max=256Mi reps=4;vecgrow vecs=1 max=64Mi reps=8"
run tstore2 scripts/measure --both-l3 $s/servers.sh "$out/tstore2.jsonl" gs-fown64,gs-main,gs-fown servers-thread "$only"
run v256e env ROUNDS=5 scripts/measure $s/pk.sh "$out/v256e.jsonl" "vecgrow vecs=1 max=256Mi reps=4;vecgrow vecs=1 max=64Mi reps=8"
run tstore3 scripts/measure --both-l3 $s/servers.sh "$out/tstore3.jsonl" gs-fown,gs-fown64,gs-main servers-thread "$only"
echo "$(date +%T) chain done"
