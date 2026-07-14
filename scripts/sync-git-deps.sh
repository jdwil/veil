#!/usr/bin/env bash
# Refresh git-based external deps.
# - aether-ui: npm github:jdwil/aether-ui (source exports — no vendor build)
# - mind-palace: Cargo git (cargo fetch)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "==> @aether-ui/core (github:jdwil/aether-ui)"
cd "$ROOT/veil-viewer"
npm install "github:jdwil/aether-ui" --no-fund --no-audit

echo "==> mind-palace (Cargo git)"
cd "$ROOT"
cargo fetch

echo "OK"
node -e "
const p = require('./veil-viewer/node_modules/@aether-ui/core/package.json');
console.log('@aether-ui/core', p.version, p.svelte || p.exports?.['.']?.svelte);
"
