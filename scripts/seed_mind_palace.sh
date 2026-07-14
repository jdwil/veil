#!/usr/bin/env bash
# Trigger Mind Palace seeding via the VEIL IDE agent (ACP or Rig).
#
# Requires: veil serve running with MIND_PALACE=1 + AWS_PROFILE=dashlx_dev
#
# Usage:
#   # single-project serve (make serve PROJECT=/path/to/wear_test):
#   ./scripts/seed_mind_palace.sh
#
#   # multi-project hub:
#   VEIL_SEED_PROJECT=wear_test ./scripts/seed_mind_palace.sh
#
# Progress: SSE stream — status / tool lines print live.
set -euo pipefail

PORT="${VEIL_PORT:-3001}"
HOST="${VEIL_API_HOST:-http://127.0.0.1:${PORT}}"
PROJECT="${VEIL_SEED_PROJECT:-}"
TIMEOUT="${VEIL_SEED_TIMEOUT:-600}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PARSER="${SCRIPT_DIR}/seed_mind_palace_parse.py"

if [[ ! -f "$PARSER" ]]; then
  echo "error: missing $PARSER" >&2
  exit 1
fi

resolve_url() {
  local multi_stream base_stream code
  base_stream="${HOST}/api/agent/turn/stream"
  if [[ -n "$PROJECT" ]]; then
    multi_stream="${HOST}/api/p/${PROJECT}/agent/turn/stream"
    code=$(curl -sS -o /dev/null -w "%{http_code}" -m 3 \
      -X POST "$multi_stream" \
      -H 'Content-Type: application/json' \
      -H 'Accept: text/event-stream' \
      -d '{"prompt":"ping"}' 2>/dev/null || echo "000")
    if [[ "$code" != "404" && "$code" != "000" ]]; then
      echo "$multi_stream"
      return
    fi
    echo "note: multi-project route not available (HTTP $code) — using single-project /api" >&2
  fi
  echo "$base_stream"
}

if ! curl -sf -m 3 "${HOST}/api/files" >/dev/null 2>&1 \
   && ! curl -sf -m 3 "${HOST}/api/models" >/dev/null 2>&1; then
  echo "error: API not reachable at ${HOST}" >&2
  echo "  Start with: make serve PROJECT=/path/to/project" >&2
  exit 1
fi

echo "=== provider ==="
curl -sS -m 5 "${HOST}/api/models" 2>/dev/null | python3 -m json.tool 2>/dev/null || true
echo

URL="$(resolve_url)"
echo "Seeding Mind Palace via agent stream → ${URL}"
echo "(prompt: seed mind palace; timeout ${TIMEOUT}s)"
echo "You should see status / tool / chunk lines below. Ctrl-C aborts the client only."
echo "────────────────────────────────────────────────────────"
echo

set +e
# IMPORTANT: parser must read stdin from the pipe (not a heredoc).
curl -sS -N --max-time "$TIMEOUT" -X POST "$URL" \
  -H 'Content-Type: application/json' \
  -H 'Accept: text/event-stream' \
  -d '{"prompt":"seed mind palace"}' \
  | python3 -u "$PARSER"
rc=$?
set -e

echo
if [[ $rc -eq 28 ]]; then
  echo "error: timed out after ${TIMEOUT}s (raise VEIL_SEED_TIMEOUT or check ACP / Mind Palace)"
  exit 1
elif [[ $rc -ne 0 ]]; then
  echo "error: stream failed (exit $rc)"
  exit "$rc"
fi

echo "────────────────────────────────────────────────────────"
echo "If wiki tools never appeared: check MIND_PALACE=1, AWS_PROFILE, MIND_PALACE_* buckets,"
echo "and server log for 'Mind Palace tools enabled'."
echo "Verify later: agent prompt  list wiki  /  wiki_search veil"
