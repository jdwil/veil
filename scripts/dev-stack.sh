#!/usr/bin/env bash
# Manage the single ProductHost backend + Vite UI.
#
# Usage:
#   scripts/dev-stack.sh start|stop|restart|status|smoke
#
# Backend:  http://127.0.0.1:8080  (ProductHost — IDE + agent + platform APIs)
# Frontend: http://127.0.0.1:5180  (Vite → proxies /api to :8080)
#
# Env: copy .env.example → .env, or export AWS_PROFILE / VEIL_DDB_TABLE / BUCKET.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Runtime selection: VEIL_ENV=dlx (default, DashLX dev-account store, :8080)
#                    VEIL_ENV=foss (local FOSS dev, no AWS, :8090)
# Picks $ROOT/.env.$VEIL_ENV; falls back to plain $ROOT/.env for backward compat.
VEIL_ENV="${VEIL_ENV:-dlx}"
ENV_FILE="$ROOT/.env.$VEIL_ENV"
[[ -f "$ENV_FILE" ]] || ENV_FILE="$ROOT/.env"
if [[ -f "$ENV_FILE" ]]; then
  echo "==> env: $ENV_FILE"
  set -a
  # shellcheck disable=SC1091
  source "$ENV_FILE"
  set +a
fi
BACKEND_BIN="${BACKEND_BIN:-$ROOT/target/release/veil-runtime}"
BACKEND_PORT="${VEIL_PORT:-8080}"
UI_PORT="${UI_PORT:-5180}"
BACKEND_LOG="${BACKEND_LOG:-/tmp/veil-product-host.log}"
UI_LOG="${UI_LOG:-/tmp/veil-ui.log}"

export AWS_REGION="${AWS_REGION:-us-west-2}"
if [[ "${VEIL_PLATFORM_LOCAL:-}" == "1" || "${VEIL_PLATFORM_LOCAL:-}" == "true" || "${VEIL_PLATFORM_LOCAL:-}" == "local" ]]; then
  # Personal host: local SQLite/JSON catalog + GitHub origin. Do not default to DashLX DDB/S3.
  export VEIL_PLATFORM_LOCAL=1
  export VEIL_SOURCE_MODE="${VEIL_SOURCE_MODE:-local}"
  export VEIL_PROJECTS_DIR="${VEIL_PROJECTS_DIR:-$HOME/.veil/personal/projects}"
  unset AWS_PROFILE || true
  unset VEIL_DDB_TABLE || true
  unset BUCKET || true
  unset VEIL_S3_BUCKET || true
else
  export VEIL_DDB_TABLE="${VEIL_DDB_TABLE:-veil-runtime-dev}"
  export BUCKET="${BUCKET:-veil-runtime-dev}"
  export VEIL_S3_BUCKET="${VEIL_S3_BUCKET:-$BUCKET}"
  # Production-like: IDE source R/W via DDB META + S3 only (no veil-projects disk).
  export VEIL_SOURCE_MODE="${VEIL_SOURCE_MODE:-s3}"
fi
export VEIL_SOURCE_BRANCH="${VEIL_SOURCE_BRANCH:-main}"
# Platform language packs (ddd, di, …) — monorepo layers/ for local; also seedable to S3+DDB.
export VEIL_LAYERS_DIR="${VEIL_LAYERS_DIR:-$ROOT/layers}"
# Durable coding sessions (session workdirs + DDB META + agent turns).
export VEIL_SESSIONS="${VEIL_SESSIONS:-1}"
# Real git origin on S3 (see docs/ADR_GIT_ORIGIN_S3.md). auto = on with sessions.
export VEIL_GIT_ORIGIN="${VEIL_GIT_ORIGIN:-auto}"
export VEIL_WS_ROOT="${VEIL_WS_ROOT:-${TMPDIR:-/tmp}/veil-ws}"
export VEIL_DEV_USER="${VEIL_DEV_USER:-${USER:-local-dev}}"
# Optional slug→repo_id map. DDB META scan is primary; do not ship tenant UUIDs.
if [[ -n "${VEIL_REPO_MAP:-}" ]]; then
  export VEIL_REPO_MAP
fi
export VEIL_DEV="${VEIL_DEV:-1}"
export VEIL_PORT="$BACKEND_PORT"
export VEIL_NONINTERACTIVE=1
export CI="${CI:-1}"
# Disk hub path kept for prefer_s3 fallback / tooling; ignored when VEIL_SOURCE_MODE=s3.
export VEIL_PROJECTS_DIR="${VEIL_PROJECTS_DIR:-$HOME/dev/veil-projects}"
export VEIL_VIEWER_STATIC="${VEIL_VIEWER_STATIC:-$ROOT/crates/veil-runtime/static/viewer}"
export VEIL_MODEL_PROVIDER="${VEIL_MODEL_PROVIDER:-acp}"
export VEIL_ACP_COMMAND="${VEIL_ACP_COMMAND:-kiro-cli}"
export VEIL_ACP_ARGS="${VEIL_ACP_ARGS:-acp --trust-all-tools}"
# Dedicated agent with mind-palace + jira + veil-ide-tools (see config/kiro-agent-veil.json).
# Does not modify hive.json — install as ~/.kiro/agents/veil.json
export VEIL_ACP_AGENT="${VEIL_ACP_AGENT:-veil}"
# Empty sandbox — not the monorepo, not the session checkout. Agent uses MCP.
# Optional conversion source: VEIL_REFERENCE_DIRS (read-only MCP reference_*).
export VEIL_ACP_CWD="${VEIL_ACP_CWD:-${TMPDIR:-/tmp}/veil-acp-cwd}"
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

# Smoke shells out to sibling `target/release/veil` (not in-process codegen).
# A newer veil-runtime with a stale CLI reintroduces `unstubbed` / rustc fails.
ensure_host_bins() {
  local veil_cli="$ROOT/target/release/veil"
  local need=0
  if [[ ! -x "$BACKEND_BIN" ]]; then need=1; fi
  if [[ ! -x "$veil_cli" ]]; then need=1; fi
  if [[ -x "$BACKEND_BIN" && -x "$veil_cli" && "$BACKEND_BIN" -nt "$veil_cli" ]]; then
    echo "==> veil CLI older than veil-runtime — rebuilding both (write_source smoke uses the CLI)"
    need=1
  fi
  if [[ "$need" -eq 1 ]]; then
    echo "==> building veil-runtime + veil-cli (release)…"
    (cd "$ROOT" && cargo build --release -p veil-runtime -p veil-cli)
  fi
}

start_backend() {
  ensure_host_bins
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
  cd "$ROOT/ui"
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
  for path in /health /api/projects /api/repos /api/pull_requests /api/deploy_environments; do
    code=$(curl -sf -o /tmp/veil-smoke.json -w "%{http_code}" --max-time 8 \
      "http://127.0.0.1:${BACKEND_PORT}${path}" || echo err)
    echo "  backend $code  $path"
    [[ "$code" == "200" ]] || ok=0
  done
  code=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 8 \
    "http://127.0.0.1:${UI_PORT}/api/pull_requests" || echo err)
  echo "  ui-proxy $code  /api/pull_requests"
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
  echo "env: PLATFORM_LOCAL=${VEIL_PLATFORM_LOCAL:-} AWS_PROFILE=${AWS_PROFILE:-off} TABLE=${VEIL_DDB_TABLE:-off} BUCKET=${BUCKET:-off} SOURCE_MODE=$VEIL_SOURCE_MODE BRANCH=${VEIL_SOURCE_BRANCH:-main} SESSIONS=${VEIL_SESSIONS:-} WS_ROOT=${VEIL_WS_ROOT:-} PROXY=$VEIL_RUNTIME_PROXY"
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
