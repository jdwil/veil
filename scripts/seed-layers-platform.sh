#!/usr/bin/env bash
# Seed platform .layer catalog: S3 body + DDB META pointer (read-only language packs).
#
#   s3://$BUCKET/layers/platform/{name}/{version}.layer
#   DDB PK=LAYER#{name} SK=META  data={ name, version, s3_key, bytes, fingerprint, visibility }
#
# Usage:
#   AWS_PROFILE=dashlx_dev VEIL_DDB_TABLE=veil-runtime-dev BUCKET=veil-runtime-dev \
#     ./scripts/seed-layers-platform.sh [layers_dir]
#
# Default layers_dir: monorepo layers/
#
# Products must not edit these. Customize by forking to e.g. layers/acme-ddd.layer
# and `use acme-ddd`.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LAYERS_DIR="${1:-$ROOT/layers}"
TABLE="${VEIL_DDB_TABLE:-veil-runtime-dev}"
BUCKET="${BUCKET:-${VEIL_S3_BUCKET:-veil-runtime-dev}}"
export AWS_PROFILE="${AWS_PROFILE:-dashlx_dev}"
export AWS_REGION="${AWS_REGION:-us-west-2}"
VERSION="${VEIL_PLATFORM_LAYER_VERSION:-1.0.0}"

if [[ ! -d "$LAYERS_DIR" ]]; then
  echo "missing layers dir: $LAYERS_DIR" >&2
  exit 1
fi

echo "Seeding platform layers from $LAYERS_DIR"
echo "  S3: s3://$BUCKET/layers/platform/{name}/{ver}.layer"
echo "  DDB META: $TABLE LAYER#*/META"
echo "  version: $VERSION"
TMPDIR_SEED=$(mktemp -d)
trap 'rm -rf "$TMPDIR_SEED"' EXIT
n=0
fail=0

for f in "$LAYERS_DIR"/*.layer; do
  [[ -f "$f" ]] || continue
  base=$(basename "$f" .layer)

  read -r name ver s3_key bytes fingerprint < <(python3 -c '
import sys, re, hashlib
path = sys.argv[1]
fallback = sys.argv[2]
default_ver = sys.argv[3]
text = open(path, "rb").read()
text_s = text.decode("utf-8", errors="replace")
# pkg ddd v1  or  pkg ddd
m = re.search(r"^pkg\s+(\S+)(?:\s+v?(\S+))?", text_s, re.M)
name = m.group(1) if m else fallback
raw_ver = (m.group(2) if m and m.group(2) else "").strip()
if raw_ver and re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z._+-]*", raw_ver):
    ver = raw_ver
else:
    ver = default_ver
# Prefer stem for catalog name (ddd.layer → ddd) over pkg display name
name = fallback
s3_key = f"layers/platform/{name}/{ver}.layer"
# FNV-1a 64 (matches veil_ir::content_fingerprint)
h = 0xcbf29ce484222325
for b in text:
    h ^= b
    h = (h * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
fp = f"{h:016x}"
print(name, ver, s3_key, len(text), fp)
' "$f" "$base" "$VERSION")

  uri="s3://$BUCKET/$s3_key"
  if ! aws s3 cp "$f" "$uri" >/dev/null; then
    echo "  ✗ $name (s3 cp failed)"
    fail=$((fail + 1))
    continue
  fi

  meta_file="$TMPDIR_SEED/${base}.meta.json"
  python3 -c '
import json, sys
name, ver, s3_key, bytes_, fp = sys.argv[1:6]
meta = {
  "name": name,
  "version": ver,
  "s3_key": s3_key,
  "bytes": int(bytes_),
  "fingerprint": fp,
  "visibility": "platform",
  "readonly": True,
}
item = {
  "PK": {"S": f"LAYER#{name}"},
  "SK": {"S": "META"},
  "data": {"S": json.dumps(meta)},
}
open(sys.argv[6], "w").write(json.dumps(item))
' "$name" "$ver" "$s3_key" "$bytes" "$fingerprint" "$meta_file"

  if aws dynamodb put-item --table-name "$TABLE" --item "file://$meta_file" >/dev/null; then
    echo "  ✓ $name @ $ver → $uri"
    n=$((n + 1))
  else
    echo "  ✗ $name (META put failed)"
    fail=$((fail + 1))
  fi
done

echo "Done: $n layer(s) seeded; failures=$fail"
echo "Materialize: host uses \$TMP/veil-platform-layers via layer_ops::materialize_platform_layers"
echo "Local dev: export VEIL_LAYERS_DIR=$LAYERS_DIR"
