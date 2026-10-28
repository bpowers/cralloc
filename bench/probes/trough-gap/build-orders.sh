#!/usr/bin/env bash
# The order service (bench/apps/orders) with the variants of docs/research/trough-gap.md, for
# benchrun's servers-large suite, and benchrun itself. Each build carries the diagnostic
# counters; `o-base` changes no constant (REFRESH_EVERY set to its own value), so the builds
# differ only in the variant. Run under scripts/heavy from the repository root:
#
#   scripts/heavy bench/probes/trough-gap/build-orders.sh
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
for spec in "o-base=depot/slow.rs:REFRESH_EVERY=32" "o-r4=depot/slow.rs:REFRESH_EVERY=4" \
            "o-m256=classes/magazine.rs:MAX_LOG2=8"; do
  name="${spec%%=*}"
  TG_CONST="${spec#*=}" "$here/build.sh" "$name" HEAD asmalloc \
    bench/probes/large-heap-constants/diag_edit.py const_edit.py -- orders || echo "FAILED $name"
done
"$here/build.sh" live-bytes HEAD live-bytes -- orders || echo "FAILED live-bytes"
(cd "$bench" && cargo build --release -p bench-runner) || echo "FAILED benchrun"
(cd "$bench/probes/loadgen" && cargo build --release --locked --target-dir "$bench/target/loadgen") || echo "FAILED loadgen"
