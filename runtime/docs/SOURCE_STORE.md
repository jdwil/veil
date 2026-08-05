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
export VEIL_SOURCE_MODE=s3              # strict remote (default for dev-stack) | `prefer_s3` | `disk`
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
| `s3` (dev-stack default) | **IDE + deploy compile**: DDB META + S3 only. Materialize to `$TMP/veil-s3-ws/{slug}/`, write-through on edit. No `VEIL_PROJECTS_DIR`. Closest to ECS. |
| `prefer_s3` | Try S3 open first; fall back to disk hub on miss |
| `disk` | Always `{projects_dir}/{slug}` (legacy local-only hub) |

### IDE kernel (ProductHost / `veil-server`)

- `ProjectsHub` honors `VEIL_SOURCE_MODE` via `provider/s3_workspace.rs`.
- `GET /api/projects` returns remote slugs when mode is `s3` / `prefer_s3`.
- Writes: `write_source` → local materialization **and** `aws s3 cp` to `repos/{id}/{branch}/{path}` (**fail closed** — no success without durable put).
- Optional `VEIL_REPO_MAP=slug=uuid,…` skips DDB for id resolve; DDB META scan is primary.

### Durable sessions

See [`DURABLE_SESSIONS.md`](./DURABLE_SESSIONS.md).

- Session workdirs: `{VEIL_WS_ROOT}/{user}/{session_id}/{slug}/` (not a shared slug-global tmp).
- DDB `SESSION#{id}/META` + `TURN#…` for resume after browser/host crash.
- Workspace tools: MCP `ws_*` (grep/read/write/str_replace) on the session workdir only.
- Header `X-Veil-Session-Id`; auto default session when omitted.

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
