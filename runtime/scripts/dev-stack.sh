#!/usr/bin/env bash
# Manage the single ProductHost backend + runtime UI Vite for local dashlx_dev.
#
# Usage:
#   runtime/scripts/dev-stack.sh start|stop|restart|status|smoke
#
# Backend:  http://127.0.0.1:8080  (ProductHost — IDE + agent + platform APIs)
# Frontend: http://127.0.0.1:5180  (Vite → proxies /api to :8080)
#
# Env overrides: VEIL_PORT, VEIL_RUNTIME_PROXY, VEIL_DDB_TABLE, BUCKET, AWS_PROFILE

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BACKEND_BIN="${BACKEND_BIN:-$ROOT/runtime/bootstrap/target/release/veil-runtime}"
BACKEND_PORT="${VEIL_PORT:-8080}"
UI_PORT="${UI_PORT:-5180}"
BACKEND_LOG="${BACKEND_LOG:-/tmp/veil-product-host.log}"
UI_LOG="${UI_LOG:-/tmp/veil-ui.log}"

export AWS_PROFILE="${AWS_PROFILE:-dashlx_dev}"
export AWS_REGION="${AWS_REGION:-us-west-2}"
export VEIL_DDB_TABLE="${VEIL_DDB_TABLE:-veil-runtime-dev}"
export BUCKET="${BUCKET:-veil-runtime-dev}"
export VEIL_S3_BUCKET="${VEIL_S3_BUCKET:-$BUCKET}"
# Production-like: IDE source R/W via DDB META + S3 only (no veil-projects disk).
# Override with VEIL_SOURCE_MODE=prefer_s3|disk only when intentionally testing hybrid/local.
export VEIL_SOURCE_MODE="${VEIL_SOURCE_MODE:-s3}"
export VEIL_SOURCE_BRANCH="${VEIL_SOURCE_BRANCH:-main}"
# Platform language packs (ddd, di, …) — monorepo layers/ for local; also seedable to S3+DDB.
export VEIL_LAYERS_DIR="${VEIL_LAYERS_DIR:-$ROOT/layers}"
# Durable coding sessions (session workdirs + DDB META + agent turns).
export VEIL_SESSIONS="${VEIL_SESSIONS:-1}"
export VEIL_WS_ROOT="${VEIL_WS_ROOT:-${TMPDIR:-/tmp}/veil-ws}"
export VEIL_DEV_USER="${VEIL_DEV_USER:-${USER:-local-dev}}"
# Optional slug→repo_id map (DDB scan is primary). Known dev seeds:
export VEIL_REPO_MAP="${VEIL_REPO_MAP:-relay=cfb3bc05-0436-47b8-9fd1-9b54b75f6d44,agentic-workflows=b603a7dc-2d3d-4f0a-a405-fc61d81fa440,dlx-auth=a4184638-ccb3-47f9-a06c-af17ed778300,wear-test=7b4a20ee-b559-4706-9d57-ac9142d65289}"
export VEIL_DEV="${VEIL_DEV:-1}"
export VEIL_PORT="$BACKEND_PORT"
export VEIL_NONINTERACTIVE=1
export CI="${CI:-1}"
# Disk hub path kept for prefer_s3 fallback / tooling; ignored when VEIL_SOURCE_MODE=s3.
export VEIL_PROJECTS_DIR="${VEIL_PROJECTS_DIR:-$HOME/dev/veil-projects}"
export VEIL_VIEWER_STATIC="${VEIL_VIEWER_STATIC:-$ROOT/runtime/bootstrap/static/viewer}"
export VEIL_MODEL_PROVIDER="${VEIL_MODEL_PROVIDER:-acp}"
export VEIL_ACP_COMMAND="${VEIL_ACP_COMMAND:-kiro-cli}"
export VEIL_ACP_ARGS="${VEIL_ACP_ARGS:-acp --trust-all-tools}"
# Dedicated agent with mind-palace + jira + veil-ide-tools (see runtime/config/kiro-agent-veil.json).
# Does not modify hive.json — install as ~/.kiro/agents/veil.json
export VEIL_ACP_AGENT="${VEIL_ACP_AGENT:-veil}"
export VEIL_ACP_CWD="${VEIL_ACP_CWD:-$ROOT}"
export VEIL_RUNTIME_PROXY="${VEIL_RUNTIME_PROXY:-http://127.0.0.1:$BACKEND_PORT}"

kill_port() {
  local port="$1"
  fuser -k "${port}/tcp" 2>/dev/null || true
}

stop_stack() {
  echo "==> stop backend :$BACKEND_PORT and UI :$UI_PORT"
  kill_port "$BACKEND_PORT"
  kill_port "$UI_PORT"
  # Stale alternate ports from older dual-process setups
  kill_port 3000 2>/dev/null || true
  kill_port 3001 2>/dev/null || true
  kill_port 3210 2>/dev/null || true
  sleep 1
}

start_backend() {
  if [[ ! -x "$BACKEND_BIN" ]]; then
    echo "==> building veil-runtime (release)…"
    cargo build --release --manifest-path "$ROOT/runtime/bootstrap/Cargo.toml"
  fi
  echo "==> start ProductHost :$BACKEND_PORT  (log $BACKEND_LOG)"
  nohup "$BACKEND_BIN" >"$BACKEND_LOG" 2>&1 &
  echo "    pid $!"
  # wait for listen
  for _ in $(seq 1 30); do
    if ss -tln 2>/dev/null | grep -qE ":${BACKEND_PORT}\\b"; then
      break
    fi
    sleep 0.2
  done
}

start_ui() {
  echo "==> start Vite UI :$UI_PORT → $VEIL_RUNTIME_PROXY  (log $UI_LOG)"
  cd "$ROOT/runtime/ui"
  nohup env VEIL_RUNTIME_PROXY="$VEIL_RUNTIME_PROXY" npx vite dev --port "$UI_PORT" --host 127.0.0.1 \
    >"$UI_LOG" 2>&1 &
  echo "    pid $!"
  for _ in $(seq 1 40); do
    if ss -tln 2>/dev/null | grep -qE ":${UI_PORT}\\b"; then
      break
    fi
    sleep 0.25
  done
}

smoke() {
  echo "==> smoke"
  local ok=1
  for path in /health /api/projects /api/repos /api/change_requests /api/deploy_environments; do
    code=$(curl -sf -o /tmp/veil-smoke.json -w "%{http_code}" --max-time 8 \
      "http://127.0.0.1:${BACKEND_PORT}${path}" || echo err)
    echo "  backend $code  $path"
    [[ "$code" == "200" ]] || ok=0
  done
  code=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 8 \
    "http://127.0.0.1:${UI_PORT}/api/change_requests" || echo err)
  echo "  ui-proxy $code  /api/change_requests"
  [[ "$code" == "200" ]] || ok=0
  if [[ "$ok" -eq 1 ]]; then
    echo "✓ stack OK  UI http://127.0.0.1:${UI_PORT}/  API http://127.0.0.1:${BACKEND_PORT}/"
  else
    echo "✗ smoke failed — see $BACKEND_LOG $UI_LOG" >&2
    return 1
  fi
}

status() {
  echo "ports:"
  ss -tlnp 2>/dev/null | grep -E ":(${BACKEND_PORT}|${UI_PORT}|3000|3001|3210)\\b" || echo "  (none matching)"
  echo "env: AWS_PROFILE=$AWS_PROFILE TABLE=$VEIL_DDB_TABLE BUCKET=$BUCKET SOURCE_MODE=$VEIL_SOURCE_MODE BRANCH=${VEIL_SOURCE_BRANCH:-main} SESSIONS=${VEIL_SESSIONS:-} WS_ROOT=${VEIL_WS_ROOT:-} PROXY=$VEIL_RUNTIME_PROXY"
}

cmd="${1:-status}"
case "$cmd" in
  stop) stop_stack ;;
  start)
    stop_stack
    start_backend
    start_ui
    smoke
    ;;
  restart)
    stop_stack
    start_backend
    start_ui
    smoke
    ;;
  status) status ;;
  smoke) smoke ;;
  backend)
    kill_port "$BACKEND_PORT"
    sleep 1
    start_backend
    smoke
    ;;
  ui)
    kill_port "$UI_PORT"
    sleep 1
    start_ui
    smoke
    ;;
  *)
    echo "usage: $0 start|stop|restart|status|smoke|backend|ui" >&2
    exit 2
    ;;
esac
