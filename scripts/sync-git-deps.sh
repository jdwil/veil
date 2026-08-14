#!/usr/bin/env bash
# Refresh git-based external deps.
# - aether-ui: npm github:jdwil/aether-ui (source exports — no vendor build)
# - mind-palace: Cargo git (cargo fetch)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> @aether-ui/core (ui/ uses vendor/aether-ui)"
cd "$ROOT/ui"
npm install --no-fund --no-audit

echo "==> mind-palace (Cargo git)"
cd "$ROOT"
cargo fetch

echo "OK"
