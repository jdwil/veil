# Live-like source store (S3 + DDB)

Local dual-loop against the **dev** account should mirror production:

| Data | Store |
|------|--------|
| Repo metadata (id, slug, default branch) | DynamoDB `VEIL_DDB_TABLE` |
| **Git origin** (truth) | S3 `git/{repo_id}/` — bundle engine (see [`ADR_GIT_ORIGIN_S3.md`](./ADR_GIT_ORIGIN_S3.md)) |
| Checkout cache (compile / HTTP) | S3 `repos/{repo_id}/{branch}/{path}` — updated on push/merge |
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
| `s3` (dev-stack default) | **IDE + deploy compile**: DDB META + S3 only. Materialize to `$TMP/veil-s3-ws/{slug}/`, write-through on edit. **No `VEIL_PROJECTS_DIR` product writes.** Closest to ECS. |
| `prefer_s3` | Try S3 open first; fall back to disk hub on miss |
| `disk` | Always `{projects_dir}/{slug}` (legacy local-only hub) |

### Create project (agent / API)

| Mode | `create_project` / create path |
|------|--------------------------------|
| `s3` | `POST /api/repos` (DDB META) + **S3 scaffold** (`seed_new_repo_scaffold`). Disk hub create is **hard-forbidden**. |
| `prefer_s3` | Prefer remote create + S3 scaffold; disk hub only if remote fails |
| `disk` | `{projects_dir}/{name}` via `init_project` |

Agent **must not** shell-write or `mkdir` under `~/dev/veil-projects` when remote — use MCP `create_project` → `open_ide` → `write_source` / `create_file` / `ws_*`.

### IDE kernel (ProductHost / `veil-server`)

- `ProjectsHub` honors `VEIL_SOURCE_MODE` via `provider/s3_workspace.rs`.
- `GET /api/projects` returns remote slugs when mode is `s3` / `prefer_s3`.
- Writes: `write_source` → session working tree (real git checkout). Durable history is `session_commit` (git commit + push bundle to `git/{id}/`). Checkout cache `repos/{id}/{branch}/` is refreshed on push.
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

## Platform stubs catalog (S3 body + DDB pointer)

Shared SDK stubs (not per-repo source). Bodies can be large (full rustdoc) so
they live in **S3**; DDB only stores META.

| Item | Location |
|------|----------|
| Body | `s3://$BUCKET/stubs/platform/{name}/{version}.stub` |
| Meta | `PK=STUB#<name>` `SK=META` — `{ name, version, s3_key, bytes, fingerprint, surface, generated }` |

```bash
# Seed monorepo runtime/src/stubs → S3 + DDB META
AWS_PROFILE=dashlx_dev VEIL_DDB_TABLE=veil-runtime-dev BUCKET=veil-runtime-dev \
  ./scripts/seed-stubs-platform.sh

# IDE / agent
GET  /api/p/{project}/stubs/catalog
GET  /api/p/{project}/stubs/{name}
POST /api/p/{project}/stubs/generate  {"crate_name":"reqwest","write":true}
POST /api/p/{project}/stubs/install   {"name":"reqwest"}
```

Resolve: project `stubs/` → platform (`VEIL_STUBS_DIR` / monorepo / materialize
META+S3 into `$TMP/veil-platform-stubs`). ProductHost warms the cache on startup.

## Platform layers catalog (S3 body + DDB pointer)

VEIL-owned language packs (`ddd`, `di`, `base`, …). **Read-only** for product
coders — customize by forking under a new name (`acme-ddd.layer` + `use acme-ddd`).

| Item | Location |
|------|----------|
| Body | `s3://$BUCKET/layers/platform/{name}/{version}.layer` |
| Meta | `PK=LAYER#<name>` `SK=META` — `{ name, version, s3_key, bytes, fingerprint, visibility=platform, readonly }` |

```bash
# Seed monorepo layers/ → S3 + DDB META
AWS_PROFILE=dashlx_dev VEIL_DDB_TABLE=veil-runtime-dev BUCKET=veil-runtime-dev \
  ./scripts/seed-layers-platform.sh

# Local / ProductHost
export VEIL_LAYERS_DIR=/path/to/veil/layers   # dev-stack sets this
# Materialize cache: $TMP/veil-platform-layers (layer_ops::ensure_platform_layer_cache)
```

Resolve for platform names: **catalog only** — never session workdirs under
`/tmp/veil-ws` or ambient sibling products. ProductHost warms the cache on
startup alongside stubs.

## What reads S3 now

- `GetProjectInfra` → `DeployExec.read_project_deploy(repo_id, branch, slug)`
- Plan / provision (UI passes `repo_id` + `branch`) → S3 checkout for **compile**
- Compile step materializes `s3://$BUCKET/repos/…` to a temp dir, then `veil gen` + `cargo build`

Disk hub remains useful for authoring; **seed after edits** so live-like paths see them.
