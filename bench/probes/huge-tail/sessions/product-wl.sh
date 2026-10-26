#!/usr/bin/env bash
# The address-space trace (first-touch's wl_edit.py, builds wlmain and wlprod) on one server
# job, main and the product in a random order, ROUNDS each, to line long advice calls up with
# the slowest requests. From the checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/product-wl.sh OUTDIR "JOB" [ROUNDS]
# Needs local allocators.toml entries wl-main and wl-prod (build = "wlmain", "wlprod").
set -euo pipefail
out="$(realpath -m "$1")" job="$2" rounds="${3:-1}"
cd bench
for r in $(seq 1 "$rounds"); do
  for a in $(printf '%s\n' wl-main wl-prod | shuf); do
    probes/first-touch/session.sh "$out" "$a.r$r" servers-thread "$job" 12G "$a"
  done
done
