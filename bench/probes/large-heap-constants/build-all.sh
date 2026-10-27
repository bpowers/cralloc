#!/usr/bin/env bash
# Every build docs/research/large-heap-constants.md runs, from COMMIT (default HEAD). Run under
# scripts/heavy from the repository root:
#
#   scripts/heavy bench/probes/large-heap-constants/build-all.sh [COMMIT]
#
# The plain workload builds (glibc je mi tc asmalloc asmalloc-memstat) come from
# bench/probes/heap-drift/build.sh; this adds the diagnostic build (diag_edit.py) of the drift
# workload and the order service, the order service for each allocator and its live-bytes
# build, and the load generator. The order service goes where benchrun looks for it,
# bench/target/BUILD/apps/orders.
set -uo pipefail
commit="${1:-HEAD}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
"$here/../heap-drift/build.sh" glibc je mi tc asmalloc asmalloc-memstat || echo "FAILED workloads"
build() {
  echo "== $*"
  "$here/build.sh" "$@" || echo "FAILED $1"
}
build asmalloc-diag "$commit" asmalloc diag_edit.py -- workload orders
build asmalloc "$commit" asmalloc -- orders
build je "$commit" je -- orders
build tc "$commit" tc -- orders
build mi "$commit" mi -- orders
build glibc "$commit" glibc -- orders
build live-bytes "$commit" live-bytes -- orders
echo "== loadgen"
(cd "$bench/probes/loadgen" && cargo build --release --locked --target-dir "$bench/target/loadgen") || echo "FAILED loadgen"
