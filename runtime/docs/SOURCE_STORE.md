# Live-like source store (S3 + DDB)

Local dual-loop against the **dev** account should mirror production:

| Data | Store |
|------|--------|
| Repo metadata (id, slug, branches) | DynamoDB `VEIL_DDB_TABLE` |
| Source files + `veil.toml` | S3 `BUCKET` / `VEIL_S3_BUCKET` keys `repos/{repo_id}/{branch}/{path}` |
| Deploy state (CURRENT / versions) | DynamoDB `DEPLOY#…` rows |

## Env (local server)

**Required for jd@dashlx.com local development** — always use the **dashlx_dev**
profile and the dev account table/bucket. Without these, repos/changes look empty
or fail silently, and provision cannot talk to real AWS.

```bash
export AWS_PROFILE=dashlx_dev AWS_REGION=us-west-2
export VEIL_DDB_TABLE=veil-runtime-dev
export BUCKET=veil-runtime-dev          # or VEIL_S3_BUCKET (same bucket)
export VEIL_SOURCE_MODE=prefer_s3       # or `s3` (strict) | `disk` (legacy hub only)
export VEIL_DEPLOY_CONFIG=runtime/config/deploy.toml
export VEIL_DEV=1
export PORT=3000

# From monorepo root after `veil gen runtime/src/runtime.veil -o runtime/generated`:
cd runtime/generated && cargo build -p veil_bin
AWS_PROFILE=dashlx_dev AWS_REGION=us-west-2 \
  VEIL_DDB_TABLE=veil-runtime-dev BUCKET=veil-runtime-dev VEIL_DEV=1 PORT=3000 \
  ./target/debug/veil_bin
```

UI (Vite) proxies `/api` → `:3000` and `/api/agent` (WS) → IDE agent on `:3001`.
Confirm identity: `AWS_PROFILE=dashlx_dev aws sts get-caller-identity` → account `086261225885`.

| Mode | Behavior |
|------|----------|
| `prefer_s3` (default) | Use S3 if `veil.toml` exists for the repo; else projects hub disk |
| `s3` | S3 only — fail if not seeded (closest to live) |
| `disk` | Always `{projects_dir}/{slug}/veil.toml` |

## Seed hub → S3 (dev convenience)

```bash
# CLI
./scripts/seed-repo-s3.sh <repo_id> <slug> main

# or API (server running with BUCKET set)
curl -sS -X POST http://127.0.0.1:3000/api/sync-repo-to-object-store \
  -H 'Content-Type: application/json' \
  -d '{"id":"<repo_id>","branch":"main"}'
```

## What reads S3 now

- `GetProjectInfra` → `DeployExec.read_project_deploy(repo_id, branch, slug)`
- Plan / provision (UI passes `repo_id` + `branch`) → S3 checkout for **compile**
- Compile step materializes `s3://$BUCKET/repos/…` to a temp dir, then `veil gen` + `cargo build`

Disk hub remains useful for authoring; **seed after edits** so live-like paths see them.
