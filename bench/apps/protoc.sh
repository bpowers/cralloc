#!/usr/bin/env bash
# protobuf's own release of protoc, for the programs that compile protobuf at build time, which the
# machine may not have: checked against a digest, into DEST/bin, which build-apps puts first on PATH
# for that program's builds.
set -euo pipefail
dest="$1"
version=31.1
case "$(uname -m)" in
  aarch64) name=aarch_64 sha256=6c554de11cea04c56ebf8e45b54434019b1cd85223d4bbd25c282425e306ecc2 ;;
  *) name=x86_64 sha256=96553041f1a91ea0efee963cb16f462f5985b4d65365f3907414c360044d8065 ;;
esac
mkdir -p "$dest"
zip="$dest/protoc-$version.zip"
curl -sfL -o "$zip" "https://github.com/protocolbuffers/protobuf/releases/download/v$version/protoc-$version-linux-$name.zip"
echo "$sha256  $zip" | sha256sum --check --quiet
rm -rf "$dest/bin" "$dest/include"
unzip -q -o "$zip" -d "$dest"
rm "$zip"
"$dest/bin/protoc" --version
