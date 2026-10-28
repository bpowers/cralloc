#!/usr/bin/env bash
# Runs batches of runs.sh one after another (docs/research/trough-gap.md). Each line of LIST:
#
#   heavy OUTDIR JOB BUILD...      memory runs, under scripts/heavy
#   timed OUTDIR JOB BUILD...      timed runs, one session under scripts/measure --both-l3
#   server OUTDIR ROUND [ALLOCS]   one session of servers.sh
#
# Run from the repository root; it waits first until no other queue of this probe runs.
#
#   bench/probes/trough-gap/queue.sh LIST
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
exec 9>"$root/target/tg-queue.lock"
flock 9
while read -r kind out job rest; do
  [ -z "$kind" ] && continue
  case "$kind" in \#*) continue ;; esac
  echo "$(date -Is) batch: $kind $out $job $rest"
  case "$kind" in
    heavy) "$root/scripts/heavy" "$here/runs.sh" "$out" "$job" "$rest" ;;
    timed) TG_TIMED=1 "$root/scripts/measure" --both-l3 "$here/runs.sh" "$out" "$job" "$rest" ;;
    server) "$root/scripts/measure" --both-l3 "$here/servers.sh" "$out" "$job" $rest ;;
    *) echo "unknown batch kind $kind" ;;
  esac
  echo "$(date -Is) batch ended: $?"
done < "$1"
echo "$(date -Is) QUEUE DONE $1"
