#!/usr/bin/env bash
# Run the C-entry driver under the repository's memory cap:
#   run.sh [--ibt] sizes | run PATTERN[,PATTERN...]
# The driver pins itself to RF_CPU (default 13).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
bin=$here/out/centry
if [ "${1:-}" = --ibt ]; then bin=$here/out/centry-ibt; shift; fi
MEMLIMIT_QUIET=1 exec "$here/../../../scripts/memlimit" -m 2G -t 60m -- "$bin" "$@"
