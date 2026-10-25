#!/usr/bin/env bash
# Kernel jobs: main (ocm), the product build (oc), and the variant that does not keep an
# outgrown slot advised for huge pages (ocn, nohuge_edit.py). From the checkout:
#   scripts/measure bench/probes/grown-slots/sessions/pk3.sh OUT.jsonl
set -euo pipefail
exec python3 -I bench/probes/grown-slots/kern.py "$1" --rounds "${ROUNDS:-4}" \
  --jobs 'vecgrow vecs=64 max=4Mi reps=4;vecgrow vecs=1 max=256Mi reps=4;vecgrow vecs=1 max=64Mi reps=8' \
  --arms "main=ocm:-,rule=oc:-,nohuge=ocn:-"
