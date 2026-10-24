#!/usr/bin/env bash
# Where a server's dTLB walks land, by asmalloc region: one run of a server job under the
# `wbh-map` entry (main's behaviour, the region map written to MAP as regions are assigned),
# with IBS op samples taken for SECS seconds, DELAY seconds after the load generator starts.
# From the checkout, under scripts/measure --both-l3:
#   scripts/measure --both-l3 bench/probes/whole-block-huge/sessions/attrib-server.sh OUTDIR NAME SUITE ONLY EXE DELAY SECS [MEMCAP]
# EXE is the server's binary under bench/target/wbh (release/rpc-server, or
# apps/PROGRAM/release/PROGRAM); MAP is the `WBH_MAP` path of the `wbh-map` entry, which must
# be OUTDIR/map.txt.
set -euo pipefail
out="$(cd "$1" && pwd)" name="$2" suite="$3" only="$4" exe="$(pwd)/bench/target/wbh/$5" delay="$6" secs="$7" cap="${8:-4G}"
here="$(pwd)/bench/probes/whole-block-huge"
loadgen="$(pwd)/bench/target/loadgen/release/loadgen"
rm -f "$out/map.txt" "$out/diag.txt"
"$here/sessions/run.sh" "${out#$(pwd)/}/$name.jsonl" "$suite" "$only" wbh-map 1 "$cap" > "$out/$name.log" 2>&1 &
run=$!
find_exe() {
  for p in /proc/[0-9]*; do
    if [ "$(readlink "$p/exe" 2>/dev/null)" = "$1" ]; then echo "${p#/proc/}"; return 0; fi
  done
  return 1
}
srv=""
until srv=$(find_exe "$exe"); do sleep 0.5; kill -0 "$run" 2>/dev/null || exit 1; done
# A batch job (NOLOADGEN=1, with NOPROC=1 as run.sh takes it) is sampled from its start.
if [ -z "${NOLOADGEN:-}" ]; then
  until find_exe "$loadgen" > /dev/null; do sleep 0.2; kill -0 "$run" 2>/dev/null || exit 1; done
fi
sleep "$delay"
python3 -I "$here/ibs.py" attach "$out/$name.data" 10007 "$srv" "$secs" || true
wait "$run" || true
python3 -I "$here/ibs.py" classify "$out/$name.data" "$out/map.txt" | tee "$out/$name.class"
mv "$out/map.txt" "$out/$name.map"
mv "$out/diag.txt" "$out/$name.diag" 2>/dev/null || true
rm -f "$out/$name.data"
