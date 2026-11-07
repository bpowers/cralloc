#!/usr/bin/env bash
# Runs the kernels under the measurement lock; the program pins itself (RF_CPU).
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../../../scripts/measure" "$here/target/release/roofline-aarch64" "$@"
