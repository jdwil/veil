#!/usr/bin/env bash
# Seed a projects-hub tree into the runtime ObjectStorage (S3), mimicking live.
#
# Fast path: `aws s3 sync` with directory prunes (never walks target/generated/.git).
#
# Usage:
#   BUCKET=veil-runtime-dev AWS_PROFILE=dashlx_dev \
#     ./scripts/seed-repo-s3.sh <repo_id> <slug> [branch]
#
# Example (Relay):
#   ./scripts/seed-repo-s3.sh cfb3bc05-0436-47b8-9fd1-9b54b75f6d44 relay main
#
# API (server with BUCKET set):
#   curl -sS -X POST http://127.0.0.1:3000/api/sync-repo-to-object-store \
#     -H 'Content-Type: application/json' \
#     -d '{"id":"<repo_id>","branch":"main"}'

set -euo pipefail
REPO_ID="${1:?repo_id required}"
SLUG="${2:?slug required}"
BRANCH="${3:-main}"
BUCKET="${BUCKET:-${VEIL_S3_BUCKET:-veil-runtime-dev}}"
PROJECTS_DIR="${VEIL_PROJECTS_DIR:-}"
AWS_PROFILE="${AWS_PROFILE:-${AWS_PROFILE:-dashlx_dev}}"
export AWS_PROFILE AWS_REGION="${AWS_REGION:-us-west-2}"

if [[ -z "$PROJECTS_DIR" && -f "$HOME/.veil/config.json" ]]; then
  PROJECTS_DIR=$(python3 -c "import json;print(json.load(open('$HOME/.veil/config.json')).get('projects_dir',''))")
fi
PROJECTS_DIR="${PROJECTS_DIR:-$HOME/dev/veil-projects}"
ROOT="$PROJECTS_DIR/$SLUG"

if [[ ! -d "$ROOT" ]]; then
  echo "missing project dir: $ROOT" >&2
  exit 1
fi

PREFIX="repos/$REPO_ID/$BRANCH"
DEST="s3://$BUCKET/$PREFIX/"

echo "Seeding $DEST"
echo "  from $ROOT"
echo "  (excludes: .git, generated, target, node_modules, .veil, dist)"

# Count sources first (prune — do not descend into build trees)
mapfile -t FILES < <(
  find "$ROOT" \
    \( -name .git -o -name generated -o -name target -o -name node_modules -o -name .veil -o -name dist \) -prune -o \
    -type f -print
)
echo "  source files: ${#FILES[@]}"

if [[ ${#FILES[@]} -eq 0 ]]; then
  echo "nothing to upload" >&2
  exit 1
fi
if [[ ${#FILES[@]} -gt 500 ]]; then
  echo "refusing to upload ${#FILES[@]} files — exclusion list may be wrong" >&2
  exit 1
fi

# Bulk sync (one process; much faster than per-file aws s3 cp)
aws s3 sync "$ROOT" "$DEST" \
  --exclude ".git/*" \
  --exclude "generated/*" \
  --exclude "**/generated/*" \
  --exclude "target/*" \
  --exclude "**/target/*" \
  --exclude "node_modules/*" \
  --exclude "**/node_modules/*" \
  --exclude ".veil/*" \
  --exclude "dist/*" \
  --exclude "**/dist/*" \
  --exclude "*.rlib" \
  --exclude "*.rmeta" \
  --delete

# Show what landed (top level)
echo "Remote top-level:"
aws s3 ls "$DEST" || true
COUNT=$(aws s3 ls "$DEST" --recursive | wc -l | tr -d ' ')
echo "Done: ~$COUNT object(s) under s3://$BUCKET/$PREFIX/"
echo "Server env: BUCKET=$BUCKET VEIL_SOURCE_MODE=prefer_s3|s3"
