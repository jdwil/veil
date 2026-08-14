#!/usr/bin/env bash
# One-shot cleanup of leftover S3/DDB objects in the *dev* ProductHost store.
#
# Deletes:
#   - named leftover prefixes repos/relay, repos/test (canonical is repos/{uuid}/)
#   - old change_management git/{slug}/ facades (origin is git/{uuid}/)
#   - S3 repos/{uuid}/ with no DDB REPO# META
#   - DDB REPO# rows that are BRANCH/COMMIT-only (no META)
#   - leftover SOURCE# disk pointers
#
# Does NOT delete catalog META repos, stubs/, layers/, or git/{uuid}/ origins.
#
# Usage (from repo root, with .env):
#   ./scripts/ops-cleanup-dev-store.sh          # dry-run
#   ./scripts/ops-cleanup-dev-store.sh --apply

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi
: "${BUCKET:?set BUCKET or VEIL_S3_BUCKET}"
: "${VEIL_DDB_TABLE:?set VEIL_DDB_TABLE}"
export AWS_REGION="${AWS_REGION:-us-west-2}"
APPLY=0
if [[ "${1:-}" == "--apply" ]]; then
  APPLY=1
fi

echo "bucket=$BUCKET table=$VEIL_DDB_TABLE apply=$APPLY"

META_JSON=$(mktemp)
aws dynamodb scan --table-name "$VEIL_DDB_TABLE" --region "$AWS_REGION" \
  --filter-expression 'SK = :sk AND begins_with(PK, :p)' \
  --expression-attribute-values '{":sk":{"S":"META"},":p":{"S":"REPO#"}}' \
  --projection-expression 'PK' --output json >"$META_JSON"

python3 - "$META_JSON" "$BUCKET" "$VEIL_DDB_TABLE" "$AWS_REGION" "$APPLY" <<'PY'
import json, os, subprocess, sys
meta_path, bucket, table, region, apply_s = sys.argv[1:]
apply = apply_s == "1"

def run(args, check=True):
    print("+", " ".join(args))
    if not apply and args[0] == "aws" and args[1] in ("s3", "dynamodb") and args[2] in ("rm", "delete-item"):
        print("  (dry-run)")
        return subprocess.CompletedProcess(args, 0, b"", b"")
    return subprocess.run(args, check=check)

meta = json.load(open(meta_path))
keep = set()
for it in meta.get("Items", []):
    pk = it["PK"]["S"]
    if pk.startswith("REPO#"):
        keep.add(pk[5:])
print("catalog META ids:", sorted(keep))

# named leftovers + slug git facades
s3_rm = [
    f"s3://{bucket}/repos/relay/",
    f"s3://{bucket}/repos/test/",
    f"s3://{bucket}/git/agent-registry/",
    f"s3://{bucket}/git/relay/",
    f"s3://{bucket}/git/dlx-bus/",
]
# list s3 repos/
ls = subprocess.check_output(["aws", "s3", "ls", f"s3://{bucket}/repos/", "--region", region], text=True)
for line in ls.splitlines():
    line = line.strip()
    if "PRE " not in line:
        continue
    name = line.split("PRE ", 1)[1].strip().rstrip("/")
    if name in ("relay", "test"):
        continue
    # uuid?
    if name not in keep:
        s3_rm.append(f"s3://{bucket}/repos/{name}/")
        print("orphan s3 repo:", name)

for uri in s3_rm:
    run(["aws", "s3", "rm", uri, "--recursive", "--region", region], check=False)

# DDB REPO# without META
all_repo = subprocess.check_output([
    "aws", "dynamodb", "scan", "--table-name", table, "--region", region,
    "--filter-expression", "begins_with(PK, :p)",
    "--expression-attribute-values", json.dumps({":p": {"S": "REPO#"}}),
    "--projection-expression", "PK,SK", "--output", "json",
], text=True)
items = json.loads(all_repo).get("Items", [])
by = {}
for it in items:
    by.setdefault(it["PK"]["S"], []).append(it["SK"]["S"])
for pk, sks in sorted(by.items()):
    rid = pk[5:]
    if "META" in sks:
        continue
    print("ghost DDB repo (no META):", rid, sks)
    for sk in sks:
        run([
            "aws", "dynamodb", "delete-item", "--table-name", table, "--region", region,
            "--key", json.dumps({"PK": {"S": pk}, "SK": {"S": sk}}),
        ], check=False)

# leftover SOURCE# disk pointer
run([
    "aws", "dynamodb", "delete-item", "--table-name", table, "--region", region,
    "--key", json.dumps({"PK": {"S": "SOURCE#dlx-auth"}, "SK": {"S": "MAIN"}}),
], check=False)

print("done. apply=", apply)
PY
rm -f "$META_JSON"
