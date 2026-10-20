#!/usr/bin/env bash
# The V8 benchmark suite, version 7 (BSD), as boa's own benchmark job runs it: the copy in
# boa-dev/data, bench/bench-v8, at a pinned commit, checked against recorded digests.
set -euo pipefail
dest="$1"
here=$(cd "$(dirname "$0")" && pwd)
commit=2b011b5d2f4adc86d708d0c6295ab0c595b7fb88
base="https://raw.githubusercontent.com/boa-dev/data/$commit/bench/bench-v8"

mkdir -p "$dest/v8"
for f in base.js richards.js deltablue.js crypto.js raytrace.js earley-boyer.js regexp.js splay.js \
  navier-stokes.js; do
  curl -sSf -o "$dest/v8/$f" "$base/$f"
done
(cd "$dest/v8" && sha256sum --check --quiet "$here/v8.sha256")
