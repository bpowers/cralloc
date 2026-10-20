#!/usr/bin/env bash
# TPC-H at scale factor 3 as Parquet, made as DataFusion's own benchmarks make it
# (benchmarks/bench.sh at 55.2.0, `data_tpch`): tpchgen-cli, one file per table, ZSTD(1).
# That script predates tpchgen-cli 4's command line; these are the same options in it. The
# generator is deterministic, so the files are checked against their recorded digests.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
version=4.0.0
sf=3

# Built from crates.io with the lock file the crate was published with, on disk (cargo's
# default build directory for `install` is under /tmp, which is memory here), then dropped.
build="$BENCH_APPS_DATA/build/tpchgen-cli"
"$BENCH_APPS_HEAVY" cargo install --quiet --locked "tpchgen-cli@$version" \
  --root "$dest/tool" --target-dir "$build"
rm -rf "$build"

"$BENCH_APPS_HEAVY" "$dest/tool/bin/tpchgen-cli" parquet --scale-factor "$sf" \
  --compression 'ZSTD(1)' --output-dir "$dest/sf$sf"
(cd "$dest/sf$sf" && sha256sum --check --quiet "$here/sf$sf.sha256")
