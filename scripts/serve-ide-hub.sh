#!/usr/bin/env bash
# Multi-project IDE hub with embedded viewer at /viewer.
# Usage: scripts/serve-ide-hub.sh [port]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${1:-8080}"
export VEIL_PROJECTS_DIR="${VEIL_PROJECTS_DIR:-$HOME/dev/veil-projects}"
export VEIL_LAYERS_DIR="${VEIL_LAYERS_DIR:-$ROOT/layers}"
export VEIL_VIEWER_STATIC="${VEIL_VIEWER_STATIC:-$ROOT/runtime/bootstrap/static/viewer}"

VEIL="${VEIL_BIN:-$ROOT/target/release/veil}"
if [[ ! -x "$VEIL" ]]; then
  echo "building veil…"
  (cd "$ROOT" && cargo build -p veil-cli --release)
fi

if [[ ! -f "$VEIL_VIEWER_STATIC/index.html" ]]; then
  echo "building viewer embed (VEIL_VIEWER_BASE=/viewer)…"
  (cd "$ROOT/veil-viewer" && VEIL_VIEWER_BASE=/viewer npm run build)
  mkdir -p "$VEIL_VIEWER_STATIC"
  rsync -a --delete "$ROOT/veil-viewer/build/" "$VEIL_VIEWER_STATIC/"
fi

# Verify build is not adapter-auto stub (must reference base /viewer)
if ! grep -q 'base: "/viewer"' "$VEIL_VIEWER_STATIC/index.html" 2>/dev/null; then
  echo "rebuilding viewer — static index missing base /viewer"
  (cd "$ROOT/veil-viewer" && VEIL_VIEWER_BASE=/viewer npm run build)
  rsync -a --delete "$ROOT/veil-viewer/build/" "$VEIL_VIEWER_STATIC/"
fi

fuser -k "${PORT}/tcp" 2>/dev/null || true
sleep 1
LOG="${LOG:-/tmp/veil-multi-${PORT}.log}"
nohup "$VEIL" serve --multi -p "$PORT" >"$LOG" 2>&1 </dev/null &
echo $! > "/tmp/veil-multi-${PORT}.pid"
echo "✓ IDE hub pid=$(cat /tmp/veil-multi-${PORT}.pid)  log=$LOG"
echo "  http://127.0.0.1:${PORT}/viewer/?project=reaction&mode=reaction"
echo "  http://127.0.0.1:${PORT}/api/projects"
sleep 1
curl -sS -o /dev/null -w "viewer %{http_code}  hub %{http_code}\n" \
  "http://127.0.0.1:${PORT}/viewer/" \
  "http://127.0.0.1:${PORT}/api/projects" || true
