#!/usr/bin/env bash
# usage: guard.sh QUEUEDIR
# Prints the server CPUs' idle setting (`scripts/deep-idle status`) into the session's log and
# fails, stopping the queue after this line (QUEUEDIR/stop), unless every deep state is disabled
# on them: these sessions measure the tails with deep idle off, and a session that ran with it
# on would mix two settings.
set -euo pipefail
W=$(cd "$(dirname "$0")/../.." && pwd)
s=$("$W/scripts/deep-idle" status)
echo "$s"
if ! grep -q '^every deep state .* is disabled on these CPUs$' <<<"$s"; then
  echo "guard: deep idle states are enabled on the server CPUs; stopping the queue" >&2
  touch "$1/stop"
  exit 3
fi
