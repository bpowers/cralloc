#!/usr/bin/env bash
# usage: queue.sh DIR
# Runs measurement sessions one at a time from DIR/queue, one shell command a line, each in a
# process group of its own, then waits as long as the session ran (two minutes at least) before
# the next, so teammates' builds get their turn (CLAUDE.md, "How we work"). Lines can be added to
# the queue while it runs. Each session's output goes to DIR/log/NNN.log, and DIR/done lists
# what ran, with start, seconds and exit status. QUEUE_GAP_MAX (seconds) caps the gap; a line
# that starts with `NOGAP=1 ` (heavy work, not a measurement) is followed by none.
#
# Stopping: `touch DIR/stop` ends the loop after the running session; `kill PID` (the loop's PID
# is in DIR/pid) stops the running session's whole process group as well and leaves nothing
# started. The loop starts every session itself, so nothing it would have started outlives it.
set -uo pipefail
dir=$(cd "$1" && pwd)
mkdir -p "$dir/log"
echo $$ > "$dir/pid"
child=""
stop() {
  if [ -n "$child" ]; then
    kill -TERM -- "-$child" 2>/dev/null || kill -TERM "$child" 2>/dev/null
    wait "$child" 2>/dev/null
  fi
  rm -f "$dir/pid" "$dir/running"
  exit 130
}
trap stop TERM INT HUP
n=$(ls "$dir/log" | wc -l)
while :; do
  [ -e "$dir/stop" ] && break
  line=$(grep -m1 -v '^\s*\(#.*\)\?$' "$dir/queue" 2>/dev/null || true)
  [ -n "$line" ] || break
  # Take the line out before running it, so an edit of the queue meanwhile cannot run it twice.
  python3 -I -c 'import sys
p, l = sys.argv[1], sys.argv[2]
lines = open(p).read().split("\n")
lines.remove(l)
open(p, "w").write("\n".join(lines))' "$dir/queue" "$line"
  n=$((n + 1))
  log=$(printf '%s/log/%03d.log' "$dir" "$n")
  echo "$line" > "$dir/running"
  t0=$(date +%s)
  echo "# $line" > "$log"
  setsid bash -c "$line" >> "$log" 2>&1 &
  child=$!
  wait "$child"
  status=$?
  child=""
  t1=$(date +%s)
  printf '%03d %s %ds exit=%d %s\n' "$n" "$(date -d @"$t0" +%H:%M)" $((t1 - t0)) "$status" "$line" >> "$dir/done"
  rm -f "$dir/running"
  gap=$((t1 - t0))
  # A session's time includes its wait for the measurement lock while teammates measure, and
  # scripts/measure gives heavy work its turn itself; QUEUE_GAP_MAX caps the gap so that a busy
  # lock does not double it.
  [ -z "${QUEUE_GAP_MAX:-}" ] || [ "$gap" -le "$QUEUE_GAP_MAX" ] || gap=$QUEUE_GAP_MAX
  [ "$gap" -ge 120 ] || gap=120
  # A line that starts with NOGAP=1 is heavy work (scripts/heavy), not a measurement: it owes
  # nobody a turn, and is queued here only so that it never runs beside a memory-heavy session.
  case "$line" in NOGAP=1\ *) gap=5 ;; esac
  # Wait in short steps so a stop file is seen.
  end=$((t1 + gap))
  while [ "$(date +%s)" -lt "$end" ] && [ ! -e "$dir/stop" ]; do sleep 5 & child=$!; wait $child; child=""; done
done
rm -f "$dir/pid"
