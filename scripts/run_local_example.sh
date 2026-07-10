#!/usr/bin/env bash
# RT-003: generate + run the local harness demo (real handler path).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-/tmp/veil_local_run}"
EXAMPLE="${ROOT}/examples/local_run.veil"

echo "==> veil gen ${EXAMPLE} → ${OUT}"
cargo run -q -p veil-cli --manifest-path "${ROOT}/Cargo.toml" -- \
  gen "${EXAMPLE}" -o "${OUT}" -t rust

echo "==> cargo run -p veil_bin"
cd "${OUT}"
cargo run -q -p veil_bin
