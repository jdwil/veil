#!/usr/bin/env bash
# Pin ChromeDriver to the installed Google Chrome version for the rustBrowser MCP.
#
# WHY: The rustBrowser MCP (browser automation: navigate/screenshot/get_console_logs)
# launches `chromedriver` by bare name from PATH and drives Google Chrome. If the two
# major versions drift, every session fails with
#   "session not created: This version of ChromeDriver only supports Chrome version N".
# The distro `chromium` package ships an old /usr/bin/chromedriver that does NOT track
# Google Chrome's fast release cadence, so we install a matching driver into a
# user-local, PATH-priority bin dir instead of touching system packages (no sudo).
#
# WHAT: Reads the installed Chrome version, downloads the exact-match ChromeDriver from
# Google's Chrome for Testing endpoint, and installs it to $DEST (default ~/.local/bin,
# which is ahead of /usr/bin on PATH). Re-run this after any Chrome auto-update.
#
# Usage:
#   scripts/pin-chromedriver.sh            # install/refresh to match current Chrome
#   scripts/pin-chromedriver.sh --check    # report drift only, non-zero exit if mismatch
#
# See palace: incident-inner-agent-stale-branch, sop-pin-chromedriver-for-mcp.

set -euo pipefail

DEST="${CHROMEDRIVER_DEST:-$HOME/.local/bin}"
CHROME_BIN="${CHROME_BIN:-}"
CFT_JSON="https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json"
MODE="install"
[[ "${1:-}" == "--check" ]] && MODE="check"

die() { echo "✗ $*" >&2; exit 1; }

find_chrome() {
  if [[ -n "$CHROME_BIN" ]]; then echo "$CHROME_BIN"; return; fi
  for c in google-chrome-stable google-chrome chromium-browser chrome; do
    if command -v "$c" >/dev/null 2>&1; then command -v "$c"; return; fi
  done
  die "no Google Chrome binary found (set CHROME_BIN=/path/to/chrome)"
}

# "Google Chrome 152.0.7977.64 " -> "152.0.7977.64"
chrome_version() {
  "$1" --version 2>/dev/null | grep -oE '[0-9]+(\.[0-9]+){3}' | head -1
}

driver_version() {
  local d="$1"
  [[ -x "$d" ]] || return 1
  "$d" --version 2>/dev/null | grep -oE '[0-9]+(\.[0-9]+){3}' | head -1
}

major() { echo "${1%%.*}"; }

CHROME="$(find_chrome)"
CVER="$(chrome_version "$CHROME")"
[[ -n "$CVER" ]] || die "could not read Chrome version from $CHROME"

# What driver does PATH currently resolve to?
RESOLVED_DRIVER="$(command -v chromedriver 2>/dev/null || true)"
RESOLVED_VER=""
[[ -n "$RESOLVED_DRIVER" ]] && RESOLVED_VER="$(driver_version "$RESOLVED_DRIVER" || true)"

echo "chrome:       $CVER  ($CHROME)"
echo "chromedriver: ${RESOLVED_VER:-<none>}  (${RESOLVED_DRIVER:-not on PATH})"

if [[ "$MODE" == "check" ]]; then
  if [[ -n "$RESOLVED_VER" && "$(major "$RESOLVED_VER")" == "$(major "$CVER")" ]]; then
    echo "✓ chromedriver major matches Chrome ($(major "$CVER"))"
    exit 0
  fi
  echo "✗ DRIFT: chromedriver $(major "${RESOLVED_VER:-none}") != Chrome $(major "$CVER")" >&2
  echo "  fix: scripts/pin-chromedriver.sh" >&2
  exit 1
fi

# Already matching (exact) and resolved from DEST? nothing to do.
if [[ "$RESOLVED_DRIVER" == "$DEST/chromedriver" && "$RESOLVED_VER" == "$CVER" ]]; then
  echo "✓ already pinned to exact match $CVER at $DEST/chromedriver"
  exit 0
fi

command -v curl >/dev/null 2>&1 || die "curl required"
command -v unzip >/dev/null 2>&1 || die "unzip required"
command -v jq >/dev/null 2>&1 || die "jq required"

echo "==> resolving ChromeDriver download for $CVER (linux64)…"
URL="$(curl -sfL "$CFT_JSON" \
  | jq -r --arg v "$CVER" \
      '.versions[] | select(.version==$v) | .downloads.chromedriver[]? | select(.platform=="linux64") | .url')"

if [[ -z "$URL" ]]; then
  # Fall back to the latest build in the same milestone (major) as Chrome.
  MJ="$(major "$CVER")"
  echo "==> exact $CVER not published; falling back to latest milestone $MJ"
  URL="$(curl -sfL "https://googlechromelabs.github.io/chrome-for-testing/latest-versions-per-milestone-with-downloads.json" \
    | jq -r --arg m "$MJ" '.milestones[$m].downloads.chromedriver[]? | select(.platform=="linux64") | .url')"
fi
[[ -n "$URL" ]] || die "no ChromeDriver download found for Chrome $CVER"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
echo "==> downloading $URL"
curl -sfL "$URL" -o "$TMP/chromedriver.zip"
unzip -q -o "$TMP/chromedriver.zip" -d "$TMP"
SRC="$(find "$TMP" -type f -name chromedriver | head -1)"
[[ -n "$SRC" ]] || die "chromedriver not found in archive"

mkdir -p "$DEST"
install -m 0755 "$SRC" "$DEST/chromedriver"
NEW_VER="$(driver_version "$DEST/chromedriver" || true)"
echo "✓ installed chromedriver $NEW_VER → $DEST/chromedriver"

# Sanity: is DEST ahead of any system chromedriver on PATH?
RESOLVED_AFTER="$(command -v chromedriver 2>/dev/null || true)"
if [[ "$RESOLVED_AFTER" != "$DEST/chromedriver" ]]; then
  echo "⚠ PATH resolves chromedriver to $RESOLVED_AFTER, not $DEST/chromedriver." >&2
  echo "  Ensure $DEST is earlier in PATH than the system bin dir." >&2
fi

if [[ "$(major "$NEW_VER")" != "$(major "$CVER")" ]]; then
  die "installed driver major $(major "$NEW_VER") still != Chrome $(major "$CVER")"
fi
echo "✓ chromedriver pinned to Chrome $CVER"
