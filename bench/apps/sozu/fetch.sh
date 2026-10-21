#!/usr/bin/env bash
# Sōzu's command library compiles its protobuf types with protoc at build time, which the
# machine may not have: protobuf's own release of it, checked against a digest, into
# DEST/bin, which build-apps puts first on PATH for this program's builds.
set -euo pipefail
dest="$1"
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
