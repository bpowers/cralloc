#!/usr/bin/env bash
# The product's timed drift runs against main (both built with build.sh HT_EDIT=0 DRIFT=none as
# `main` and `prod`): the fixed-mix control, the job without the background thread, and the
# fast ramp, each interleaved. From the checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/product-timed.sh OUTDIR
set -euo pipefail
out="$1"
k=bench/probes/huge-tail/sessions/kern.sh
four="shape=lifetime ops=2400M phases=4 samples=24 long_permille=5 quiet=0"
$k "$out/fixed" 1 "main=main:- prod=prod:-" drift $four tables=shared
$k "$out/nobg" 1 "main=main:-:ASMALLOC_BACKGROUND=0 prod=prod:-:ASMALLOC_BACKGROUND=0" drift $four tables=phase
$k "$out/ramp" 2 "main=main:- prod=prod:-" tree depth=25
