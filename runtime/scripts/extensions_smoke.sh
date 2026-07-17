#!/usr/bin/env bash
# EXT-00 dual-loop smoke: File adapters + VEIL services (no AWS required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export VEIL_LAYERS_DIR="${VEIL_LAYERS_DIR:-$ROOT/../layers}"
export VEIL_EXTENSIONS_DIR="${VEIL_EXTENSIONS_DIR:-$(mktemp -d /tmp/veil-ext-XXXX)}"
cd "$ROOT/generated"
echo "== extensions unit tests =="
cargo test -p extensions --tests --quiet
echo "== smoke OK (dir=$VEIL_EXTENSIONS_DIR) =="
