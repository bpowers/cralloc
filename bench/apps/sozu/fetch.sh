#!/usr/bin/env bash
# Sōzu's command library compiles its protobuf types with protoc at build time, which the
# machine may not have: protobuf's own release of it, checked against a digest, into
# DEST/bin, which build-apps puts first on PATH for this program's builds.
set -euo pipefail
dest="$1"
"$(dirname "$0")/../protoc.sh" "$dest"
