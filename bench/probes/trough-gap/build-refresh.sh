#!/usr/bin/env bash
# The builds of docs/research/trough-gap.md, "Refresh on evidence": the commit with the
# diagnostic counters and the run-time switch (refresh_switch.py: TG_REFRESH=off the base rate,
# =4 one in 4 always, unset the rule), as the drift workload and the order service (`rx`), and
# the accounted workload (`rx-stat`). Run under scripts/heavy from the repository root:
#
#   scripts/heavy bench/probes/trough-gap/build-refresh.sh [COMMIT]
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
commit="${1:-HEAD}"
"$here/build.sh" rx "$commit" asmalloc bench/probes/large-heap-constants/diag_edit.py \
  refresh_switch.py -- workload orders || echo "FAILED rx"
"$here/build.sh" rx-stat "$commit" asmalloc-memstat bench/probes/large-heap-constants/diag_edit.py \
  refresh_switch.py tg_edit.py || echo "FAILED rx-stat"
