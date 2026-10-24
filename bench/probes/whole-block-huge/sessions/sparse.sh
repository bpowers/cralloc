#!/usr/bin/env bash
# The sparse-tenant probe under the switch's arms. From the checkout:
#   scripts/measure bench/probes/whole-block-huge/sessions/sparse.sh OUT.jsonl
set -euo pipefail
exec python3 -I bench/probes/whole-block-huge/sparse.py "$1" --rounds "${ROUNDS:-3}" \
  --arms "main=m0f002d0,all2=m1f002d0,nz2=m2f002d0,all16=m1f016d0"
