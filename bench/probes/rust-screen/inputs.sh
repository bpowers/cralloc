#!/usr/bin/env bash
# Downloads the screening inputs at pinned versions into ROOT/inputs and prints their hashes.
# Each input is fetched once; delete its directory to fetch it again.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="${RUST_SCREEN_ROOT:-$here/../../../target/rust-screen}"
mkdir -p "$root/inputs/dl"
cd "$root/inputs"

fetch() { # URL [NAME]
  local name="${2:-$(basename "$1")}"
  [ -s "dl/$name" ] || curl -sSfL -o "dl/$name" "$1"
}

# Source trees: ripgrep, tokei, fd, bat, difftastic, tantivy's corpus, ruff.
fetch https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.tar.xz
fetch https://www.python.org/ftp/python/3.13.0/Python-3.13.0.tgz
fetch https://www.python.org/ftp/python/3.12.7/Python-3.12.7.tgz
[ -d linux-6.12 ] || tar xf dl/linux-6.12.tar.xz
[ -d Python-3.13.0 ] || tar xzf dl/Python-3.13.0.tgz
[ -d Python-3.12.7 ] || tar xzf dl/Python-3.12.7.tgz

# JavaScript corpus (oxc, swc, boa): published npm packages.
mkdir -p js
for spec in typescript/-/typescript-5.6.3 three/-/three-0.170.0 lodash/-/lodash-4.17.21 \
            react-dom/-/react-dom-18.3.1 vue/-/vue-3.5.12 d3/-/d3-7.9.0 \
            esbuild-wasm/-/esbuild-wasm-0.24.0 moment/-/moment-2.30.1 jquery/-/jquery-3.7.1; do
  fetch "https://registry.npmjs.org/$spec.tgz"
  d="js/$(basename "$spec")"
  [ -d "$d" ] || { mkdir -p "$d"; tar xzf "dl/$(basename "$spec").tgz" -C "$d"; }
done

# Octane, for boa: the benchmark sources at the last commit of chromium/octane.
# The repository is archived, so master no longer moves; the hashes printed below pin it.
octane=master
mkdir -p octane
for f in base.js richards.js deltablue.js raytrace.js splay.js earley-boyer.js navier-stokes.js crypto.js regexp.js; do
  [ -s "octane/$f" ] || curl -sSfL -o "octane/$f" "https://raw.githubusercontent.com/chromium/octane/$octane/$f"
done

# Large JSON for jaq: San Francisco's city lots, 189 MB.
fetch https://raw.githubusercontent.com/zemirco/sf-city-lots-json/master/citylots.json

# Video for rav1e: Xiph's derf collection.
fetch https://media.xiph.org/video/derf/y4m/akiyo_cif.y4m

# std's sources for rust-analyzer (RUST_SRC_PATH), without changing the shared toolchain.
fetch https://static.rust-lang.org/dist/rust-src-1.99.1.tar.xz
[ -d rust-src-1.99.1 ] || tar xf dl/rust-src-1.99.1.tar.xz
# rust-analyzer's target: ripgrep's crates.io source, with a workspace of its own since it sits
# under this checkout's.
if [ ! -d ra/ripgrep-15.2.0 ]; then
  fetch https://static.crates.io/crates/ripgrep/ripgrep-15.2.0.crate
  mkdir -p ra && tar xzf dl/ripgrep-15.2.0.crate -C ra
  printf '\n[workspace]\n' >>ra/ripgrep-15.2.0/Cargo.toml
fi

# A repository with history for gitoxide: rust-lang/cargo, used up to tag 0.90.0.
[ -d repo/cargo.git ] || { mkdir -p repo; git clone -q --bare https://github.com/rust-lang/cargo.git repo/cargo.git; }

# PNGs for oxipng: every PNG in the two source trees.
if [ ! -d png ]; then
  mkdir -p png
  find linux-6.12 Python-3.13.0 -name '*.png' -exec cp {} png/ \;
fi

# The rest needs the candidates built (build-all.sh), so it runs once they are.
bin="$root/bin"
if [ -x "$bin/tpchgen/tpchgen-cli" ] && [ ! -d tpch/sf1 ]; then
  "$bin/tpchgen/tpchgen-cli" parquet -s 1 --output-dir=tpch/sf1
  "$bin/tpchgen/tpchgen-cli" csv -s 1 --tables=lineitem --output-dir=tpch/csv
fi
mkdir -p tpch/queries
for i in $(seq 1 22); do
  [ -s "tpch/queries/q$i.sql" ] || curl -sSfL -o "tpch/queries/q$i.sql" \
    "https://raw.githubusercontent.com/apache/datafusion/54.1.0/benchmarks/queries/q$i.sql"
done
if [ ! -s wasm/pulldown-cmark.wasm ]; then
  mkdir -p wasm
  CARGO_TARGET_DIR="$root/build/wasmprog" cargo install --locked --target wasm32-wasip1 \
    --root "$root/inst/wasmprog" pulldown-cmark@0.13.4
  cp "$root/inst/wasmprog/bin/pulldown-cmark.wasm" wasm/
  rm -rf "$root/build/wasmprog" "$root/inst/wasmprog"
fi
# uv's cache, filled online once; the screened runs resolve offline from it.
if [ -x "$bin/uv/uv" ] && [ ! -d uv/cache ]; then
  mkdir -p uv
  cp "$here/uv-requirements.in" uv/requirements.in
  UV_CACHE_DIR=uv/cache "$bin/uv/uv" pip compile -q --python-version 3.12 --exclude-newer 2026-10-01 \
    --python-platform x86_64-unknown-linux-gnu uv/requirements.in >/dev/null
fi

sha256sum dl/* octane/*
