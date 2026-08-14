#!/usr/bin/env bash
# Smoke ProductHost (single process). Prefer scripts/dev-stack.sh smoke
# when the stack is already running.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then
  exec "$ROOT/scripts/dev-stack.sh" smoke
fi

PORT="${RUNTIME_PORT:-18080}"
export CI=1 VEIL_NONINTERACTIVE=1 VEIL_PORT="$PORT"
BIN="$ROOT/target/release/veil-runtime"
if [[ ! -x "$BIN" ]]; then
  echo "==> cargo build -p veil-runtime --release"
  cargo build --release -p veil-runtime
fi

echo "==> start veil-runtime on :$PORT"
"$BIN" &
PID=$!
cleanup() { kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; }
trap cleanup EXIT

for _ in $(seq 1 40); do
  if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

curl -sf "http://127.0.0.1:$PORT/health" >/dev/null
curl -sf "http://127.0.0.1:$PORT/api/projects" >/dev/null
curl -sf "http://127.0.0.1:$PORT/api/review/outstanding" >/dev/null
echo "✓ ProductHost smoke OK (port $PORT)"
