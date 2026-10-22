#!/usr/bin/env bash
# Runs where.py (cycles recordings), misses.py (ibs) or counts.py (counting runs) on each run
# directory given that has not been analyzed yet, leaving where.txt/.json, misses.txt/.json or
# counts.txt/.json beside its data. Run under scripts/heavy.
set -u
H=$(cd "$(dirname "$0")" && pwd)
dirs=()
for d in "$@"; do
  if [ -f "$d/meta.json" ]; then dirs+=("$d"); else for s in "$d"/*/; do dirs+=("${s%/}"); done; fi
done
for d in "${dirs[@]}"; do
  [ -f "$d/meta.json" ] || continue
  if [ -f "$d/counters.json" ]; then
    [ -f "$d/counts.json" ] || python3 "$H/counts.py" "$d" --json "$d/counts.json" > "$d/counts.txt" 2>&1
    continue
  fi
  mode=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['mode'])" "$d/meta.json")
  case "$mode" in
    cycles) [ -f "$d/where.json" ] || python3 "$H/where.py" "$d" --json "$d/where.json" > "$d/where.txt" 2>&1 ;;
    ibs) [ -f "$d/misses.json" ] || python3 "$H/misses.py" "$d" --json "$d/misses.json" > "$d/misses.txt" 2>&1 ;;
  esac
  echo "analyzed $d"
done
