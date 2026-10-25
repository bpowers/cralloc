#!/usr/bin/env bash
# Qdrant's inputs:
#
# bin/protoc: Qdrant's API crate compiles protobuf with protoc at build time, which the machine
# may not have: protobuf's own release of it, checked against a digest, into DEST/bin, which
# build-apps puts first on PATH for this program's builds (as Sōzu's).
#
# collection/: the collection every run starts from, a copy of which `qdrant serve` loads:
# 1,000,000 vectors of 384 floats around 256 centres, with payloads (src/driver.rs, `build`),
# indexed by Qdrant 1.20.0 itself: this script builds the driver with glibc, as build-apps
# does and into the same place, and runs its `build` job on the cores scripts/heavy uses. The
# points come from a fixed seed, but Qdrant builds its HNSW graphs on several threads, so the
# collection's files differ from one fetch to the next; the build checks the point count and
# that every optimisation finished, and this script checks the vectors' digest through the
# request files, which are drawn from the same model.
#
# search.http, mixed.http: what the load generator sends (loadgen --requests), 20,000 whole
# HTTP requests each: nearest-neighbour queries, and the same with every 20th request an
# upsert of 20 points (src/driver.rs, `requests`). Checked against digests.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
heavy="${BENCH_APPS_HEAVY:-$here/../../../scripts/heavy}"
memlimit="$here/../../../scripts/memlimit"
data="${BENCH_APPS_DATA:?run through bench/apps/fetch}"

version=31.1
sha256=96553041f1a91ea0efee963cb16f462f5985b4d65365f3907414c360044d8065
mkdir -p "$dest"
zip="$dest/protoc-$version.zip"
curl -sfL -o "$zip" "https://github.com/protocolbuffers/protobuf/releases/download/v$version/protoc-$version-linux-x86_64.zip"
echo "$sha256  $zip" | sha256sum --check --quiet
rm -rf "$dest/bin" "$dest/include"
unzip -q -o "$zip" -d "$dest"
rm "$zip"
"$dest/bin/protoc" --version

target="$here/../../target/glibc/apps/qdrant"
(
  cd "$here"
  # As build-apps sets them, so that its later build of the same binary finds this one fresh.
  common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)/.."
  export PATH="$dest/bin:$PATH" CARGO_BUILD_BUILD_DIR="$data/build/qdrant"
  export BENCH_BASELINES="${BENCH_BASELINES:-$common/bench/baselines/out}"
  unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
  "$heavy" cargo build --release --locked --target-dir "$target"
)
qdrant="$target/release/qdrant"
"$heavy" "$memlimit" -m 12G -t 4h -- "$qdrant" build "out=$dest/collection"
"$qdrant" requests "out=$dest/search.http" kind=search > /dev/null
"$qdrant" requests "out=$dest/mixed.http" kind=mixed > /dev/null
(cd "$dest" && sha256sum --check --quiet "$here/inputs.sha256")
