#!/usr/bin/env bash
# PVR-031 smoke: build product host, hit health + projects + config (CAP-007).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PORT="${RUNTIME_PORT:-18080}"
export CI=1 VEIL_NONINTERACTIVE=1 VEIL_PORT="$PORT"
export VEIL_BIN="${VEIL_BIN:-$ROOT/target/release/veil}"

echo "==> pure-runtime-build"
make pure-runtime-build

BIN="$ROOT/runtime/bootstrap/target/release/veil-runtime"
if [[ ! -x "$BIN" ]]; then
  echo "missing $BIN" >&2
  exit 1
fi

# Ensure SPA dist exists (CAP-005)
if [[ ! -f runtime/bootstrap/static/dist/index.html ]]; then
  echo "missing generated SPA: runtime/bootstrap/static/dist/index.html" >&2
  exit 1
fi
if [[ ! -f runtime/bootstrap/static/dist/spa.js ]]; then
  echo "missing generated SPA: runtime/bootstrap/static/dist/spa.js" >&2
  exit 1
fi

# PVR-032: no handwritten product shell at static root; host must not reference ide.html
if [[ -f runtime/bootstrap/static/ide.html ]]; then
  echo "PVR-032: static/ide.html must live under static/legacy/ only" >&2
  exit 1
fi
# Fail if product host code still *loads* the old iframe shell path (string literal).
if grep -R --include='*.rs' -nE '"[^"]*ide\.html"|'"'"'[^'"'"']*ide\.html'"'"'' \
    crates/veil-server/src runtime/bootstrap/src 2>/dev/null; then
  echo "PVR-032: host sources must not reference ide.html path literals" >&2
  exit 1
fi

echo "==> start veil-runtime on :$PORT"
"$BIN" &
PID=$!
cleanup() { kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; }
trap cleanup EXIT

# Wait for listen
for i in $(seq 1 40); do
  if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 \
    || curl -sf "http://127.0.0.1:$PORT/api/projects" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

echo "==> GET /"
curl -sf "http://127.0.0.1:$PORT/" | head -c 200 | grep -q "VEIL\|veil\|app\|html\|Dashboard\|script" \
  || { echo "shell / did not look like HTML/SPA" >&2; exit 1; }

echo "==> GET /api/projects"
curl -sf "http://127.0.0.1:$PORT/api/projects" | grep -q "projects\|repos\|projects_dir\|\\[" \
  || { echo "projects API unexpected" >&2; exit 1; }

echo "==> GET /api/config"
curl -sf "http://127.0.0.1:$PORT/api/config" | grep -q "projects_dir" \
  || { echo "config GET failed" >&2; exit 1; }

echo "==> GET /static/dist/spa.js"
curl -sf "http://127.0.0.1:$PORT/static/dist/spa.js" | head -c 80 | grep -q "api\|NAV\|fetch" \
  || { echo "SPA asset missing" >&2; exit 1; }

echo "✓ pure-runtime smoke OK (port $PORT)"
