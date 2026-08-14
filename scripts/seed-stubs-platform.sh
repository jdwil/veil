#!/usr/bin/env bash
# Seed platform .stub catalog: S3 body + DDB META pointer (not DDB CONTENT).
#
#   s3://$BUCKET/stubs/platform/{name}/{version}.stub
#   DDB PK=STUB#{name} SK=META  data={ name, version, s3_key, bytes, fingerprint, … }
#
# Usage:
#   AWS_PROFILE=dashlx_dev VEIL_DDB_TABLE=veil-runtime-dev BUCKET=veil-runtime-dev \
#     ./scripts/seed-stubs-platform.sh [stubs_dir]
#
# Default stubs_dir: stubs/

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STUBS_DIR="${1:-$ROOT/stubs}"
TABLE="${VEIL_DDB_TABLE:-veil-runtime-dev}"
BUCKET="${BUCKET:-${VEIL_S3_BUCKET:-veil-runtime-dev}}"
export AWS_PROFILE="${AWS_PROFILE:-dashlx_dev}"
export AWS_REGION="${AWS_REGION:-us-west-2}"

if [[ ! -d "$STUBS_DIR" ]]; then
  echo "missing stubs dir: $STUBS_DIR" >&2
  exit 1
fi

echo "Seeding platform stubs from $STUBS_DIR"
echo "  S3: s3://$BUCKET/stubs/platform/{name}/{ver}.stub"
echo "  DDB META: $TABLE STUB#*/META"
TMPDIR_SEED=$(mktemp -d)
trap 'rm -rf "$TMPDIR_SEED"' EXIT
n=0
fail=0

for f in "$STUBS_DIR"/*.stub; do
  [[ -f "$f" ]] || continue
  base=$(basename "$f" .stub)

  # Parse name/version + write META json; upload body via s3 cp
  read -r name ver s3_key bytes fingerprint gen surface < <(python3 -c '
import sys, re, hashlib, json
path = sys.argv[1]
fallback = sys.argv[2]
text = open(path, "rb").read()
text_s = text.decode("utf-8", errors="replace")
m = re.search(r"^stub\s+(\S+)(?:\s+(\S+))?", text_s, re.M)
name = m.group(1) if m else fallback
raw_ver = (m.group(2) if m and m.group(2) else "").strip()
# S3 key segment: only safe semver-ish tokens
import re as _re
if raw_ver and _re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z._+-]*", raw_ver) and not raw_ver.startswith("path:"):
    ver = raw_ver
else:
    ver = "latest"
s3_key = f"stubs/platform/{name}/{ver}.stub"
# FNV-1a 64 (matches veil_ir::content_fingerprint)
h = 0xcbf29ce484222325
for b in text:
    h ^= b
    h = (h * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
fp = f"{h:016x}"
gen = "1" if ("@generated" in text_s or "veil stub-gen" in text_s) else "0"
surf = "full" if gen == "1" else "curated"
if "surface sparse" in text_s:
    surf = "sparse"
elif "surface curated" in text_s:
    surf = "curated"
print(name, ver, s3_key, len(text), fp, gen, surf)
' "$f" "$base")

  uri="s3://$BUCKET/$s3_key"
  if ! aws s3 cp "$f" "$uri" >/dev/null; then
    echo "  ✗ $name (s3 cp failed)"
    fail=$((fail + 1))
    continue
  fi

  meta_file="$TMPDIR_SEED/${base}.meta.json"
  python3 -c '
import json, sys
name, ver, s3_key, bytes_, fp, gen, surface = sys.argv[1:8]
meta = {
  "name": name,
  "version": ver if ver != "latest" else "*",
  "s3_key": s3_key,
  "bytes": int(bytes_),
  "fingerprint": fp,
  "generated": gen == "1",
  "surface": surface,
}
item = {
  "PK": {"S": f"STUB#{name}"},
  "SK": {"S": "META"},
  "data": {"S": json.dumps(meta)},
}
open(sys.argv[8], "w").write(json.dumps(item))
' "$name" "$ver" "$s3_key" "$bytes" "$fingerprint" "$gen" "$surface" "$meta_file"

  if aws dynamodb put-item --table-name "$TABLE" --item "file://$meta_file" >/dev/null; then
    # Drop legacy CONTENT row if any
    aws dynamodb delete-item --table-name "$TABLE" \
      --key "$(python3 -c 'import json,sys; print(json.dumps({"PK":{"S":"STUB#"+sys.argv[1]},"SK":{"S":"CONTENT"}}))' "$name")" \
      >/dev/null 2>&1 || true
    echo "  ✓ $name @ $ver → $uri"
    n=$((n + 1))
  else
    echo "  ✗ $name (META put failed)"
    fail=$((fail + 1))
  fi
done

echo "Done: $n stub(s) seeded; failures=$fail"
echo "Materialize: host uses \$TMP/veil-platform-stubs via stub_ops::materialize_platform_stubs"
