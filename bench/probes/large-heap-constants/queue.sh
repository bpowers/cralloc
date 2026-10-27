#!/usr/bin/env bash
# Runs timed sessions one after another, each through scripts/measure --both-l3 (which waits
# for the lock and gives heavy work its turn between sessions). Each line of LIST is a session:
#
#   drift OUTDIR JOB RUNG BUILD...     timed.sh: one run of each build
#   orders OUTDIR RUNG ROUND           servers.sh
#
#   bench/probes/large-heap-constants/queue.sh LIST
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
while read -r kind rest; do
  [ -z "$kind" ] && continue
  case "$kind" in \#*) continue ;; esac
  read -r -a a <<< "$rest"
  echo "$(date -Is) session: $kind $rest"
  case "$kind" in
    drift) "$root/scripts/measure" --both-l3 "$here/timed.sh" "${a[0]}" "${a[1]}" "${a[2]}" "${a[*]:3}" ;;
    orders) "$root/scripts/measure" --both-l3 "$here/servers.sh" "${a[@]}" ;;
    *) echo "unknown session $kind" ;;
  esac
  echo "$(date -Is) session ended: $?"
done < "$1"
