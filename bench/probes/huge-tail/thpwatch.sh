#!/usr/bin/env bash
# Samples, every 2 s until killed, the resident and huge-page memory of every running process
# whose executable lies under bench/target/BUILD, with its ASMALLOC_HT setting: one line
# "ms setting rss_kib anonhuge_kib" a sample. For the server sessions of
# docs/research/huge-tail.md, whose records do not carry huge pages.
#
#   bench/probes/huge-tail/thpwatch.sh BUILD OUTFILE &
set -u
build="$1" out="$2"
while :; do
  for pid in $(pgrep -f "bench/target/$build/"); do
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || continue
    case "$exe" in */bench/target/"$build"/*) ;; *) continue ;; esac
    ht=$(tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | sed -n 's/^ASMALLOC_HT=//p')
    read -r rss huge < <(awk '/^Rss:/{r=$2} /^AnonHugePages:/{h=$2} END{print r, h}' "/proc/$pid/smaps_rollup" 2>/dev/null)
    echo "$(date +%s%3N) ${ht:-unset} $(basename "$exe") ${rss:-0} ${huge:-0}"
  done >> "$out"
  sleep 2
done
