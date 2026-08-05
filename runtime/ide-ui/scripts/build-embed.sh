#!/usr/bin/env bash
# Build dual-loop viewer for ProductHost / multi serve embed at /viewer.
set -euo pipefail
cd "$(dirname "$0")/.."
export VEIL_VIEWER_BASE="${VEIL_VIEWER_BASE:-/viewer}"
rm -rf build
npm run build
DEST="${1:-../runtime/bootstrap/static/viewer}"
mkdir -p "$DEST"
rsync -a --delete build/ "$DEST/"
echo "✓ Viewer embed build → $DEST (base=$VEIL_VIEWER_BASE)"
