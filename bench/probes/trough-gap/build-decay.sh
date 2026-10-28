#!/usr/bin/env bash
# The three arms of the clocked pass's goal (decay_edit.py), each with the diagnostic counters
# (large-heap-constants/diag_edit.py), as bench builds decay-A, decay-B and decay-C. Run under
# scripts/heavy from the repository root:
#
#   scripts/heavy bench/probes/trough-gap/build-decay.sh [COMMIT]
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
commit="${1:-HEAD}"
for arm in A B C; do
  TG_DECAY=$arm "$here/build.sh" "decay-$arm" "$commit" asmalloc \
    bench/probes/large-heap-constants/diag_edit.py decay_edit.py || echo "FAILED decay-$arm"
done
