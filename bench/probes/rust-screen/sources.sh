#!/usr/bin/env bash
# Fetches the candidates that are not built from crates.io, as release tarballs from GitHub,
# into ROOT/src. Their builds (and the patches that take out a built-in allocator) are in
# build-src.sh.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="${RUST_SCREEN_ROOT:-$here/../../../target/rust-screen}"
mkdir -p "$root/src/dl"
cd "$root/src"

get() { # NAME OWNER/REPO TAG
  local name="$1" repo="$2" tag="$3"
  local tgz="dl/$name-$tag.tar.gz"
  [ -s "$tgz" ] || curl -sSfL -o "$tgz" "https://codeload.github.com/$repo/tar.gz/refs/tags/$tag"
  if [ ! -d "$name" ]; then
    mkdir -p "$name"
    tar xzf "$tgz" -C "$name" --strip-components=1
  fi
  echo "$name $repo $tag $(sha256sum "$tgz" | cut -d' ' -f1)"
}

get ruff astral-sh/ruff 0.17.1
get rust-analyzer rust-lang/rust-analyzer 2026-10-12
get oxc oxc-project/oxc oxlint_v1.88.0
get uv astral-sh/uv 0.13.1

# Crates whose binary sets a #[global_allocator] unconditionally, from their crates.io
# tarballs, since build-src.sh patches them.
crate() { # NAME CRATE VERSION
  local f="dl/$2-$3.crate"
  [ -s "$f" ] || curl -sSfL -o "$f" "https://static.crates.io/crates/$2/$2-$3.crate"
  [ -d "$1" ] || { mkdir -p "$1"; tar xzf "$f" -C "$1" --strip-components=1; }
  echo "$1 crates.io $2 $3 $(sha256sum "$f" | cut -d' ' -f1)"
}
crate egglog egglog 3.0.0
crate datafusion datafusion-cli 54.1.0
crate difftastic difftastic 0.71.0
