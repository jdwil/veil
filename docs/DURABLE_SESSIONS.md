# Durable coding sessions

Production-like ProductHost keeps **efficient local checkouts** and **never loses accepted work**.

## Git-shaped workflow (operator model)

Think **branch → commit → merge**, even though the store is S3 + DDB (not a local git forge):

| Familiar | Runtime meaning |
|----------|-----------------|
| **Branch** | Isolated session (`draft` prefix). Create with `POST /api/sessions` `{ slug, branch_name }` |
| **Working tree** | Session workdir under `VEIL_WS_ROOT` |
| **Autosave / push of dirty files** | Every successful write → S3 (fail-closed). Chip: Synced / Saved |
| **Commit** | Named checkpoint: `POST /api/sessions/{id}/commits` `{ message }` → S3 snapshot + DDB `COMMIT#` |
| **Commit log** | `GET /api/sessions/{id}/commits` |
| **Merge to main** | `POST /api/sessions/{id}/merge` → sync workdir to product base branch prefix |
| **IDE Changes** | **Uncommitted** (working tree) + **History** (named commits) — not structural IR "vs baseline" |

Mainline sticky session (`POST /api/sessions` with only `slug`) still exists for quick open on **main**.

## Invariant

| Layer | Store | Disposable? |
|-------|--------|-------------|
| Browser / tab | view + typing buffer | yes |
| Session workdir | `{VEIL_WS_ROOT}/{user}/{session_id}/{slug}/` | yes (rebuildable) |
| **DDB `SESSION#…/META`** | session metadata, revision, etags, head_commit | **no** |
| **S3 objects** | file bytes (branch / drafts / commit snapshots) | **no** |
| **DDB `SESSION#…/TURN#…`** | agent transcript | **no** |
| **DDB `SESSION#…/COMMIT#…`** | named commits | **no** |

Success responses (IDE write, MCP `write_source` / `ws_*`, autosave) are returned **only after** durable S3 put succeeds.

## Efficiency

| Op | S3? |
|----|-----|
| Session open / attach (missing workdir) | Sync once |
| `ws_read` / `ws_grep` / `veil_check` | **No** |
| Single-file write | **1× PutObject** |
| `ws_pull` | Incremental sync (no `--delete`) |
| `ws_reset` | Full sync with `--delete` |

## API

```
POST   /api/sessions              { slug, branch?, draft?, branch_name? }
GET    /api/sessions
GET    /api/sessions/{id}
POST   /api/sessions/{id}/attach
POST   /api/sessions/{id}/pull
POST   /api/sessions/{id}/reset
POST   /api/sessions/{id}/flush
POST   /api/sessions/{id}/commits { message }     # git-shaped commit
GET    /api/sessions/{id}/commits
POST   /api/sessions/{id}/merge                   # promote branch → base (main)
GET    /api/sessions/{id}/turns
POST   /api/sessions/{id}/turns
POST   /api/sessions/{id}/ws/{list,read,write,str_replace,grep,rm}
POST   /api/p/{project}/autosave  + X-Veil-Session-Id
```

IDE routes also accept `X-Veil-Session-Id`. If omitted, the hub **auto-creates** a sticky default session for the user+slug.

## MCP workspace tools

`ws_list`, `ws_read`, `ws_write`, `ws_str_replace`, `ws_grep`, `ws_rm`, `ws_pull`, `ws_reset`

All path-jailed to the session workdir. Prefer structured VEIL tools for packages; use `ws_*` for full-tree edits.

## MCP git-shaped session tools

| Tool | Role |
|------|------|
| `session_status` | Branch, uncommitted, revision, head_commit |
| `create_branch` | Isolated feature branch (`branch_name`); becomes active work line |
| `session_commit` | Named checkpoint with message |
| `list_commits` | Commit log for the session |
| `merge_branch` | **Blocked by default** — use PR Wizard. Requires `force:true` + `VEIL_ALLOW_SESSION_MERGE=1` |
| `switch_main` | Return active work line to sticky mainline |
| `POST /api/sessions/{id}/publish-branch` | Sync worktree → `repos/{repo}/{branch}/` for CR structural diff |
| `POST /api/sessions/{id}/active-change` | Bind open PR id for agent reply writeback |

Agent loop: status → branch (if multi-step) → check → edit → check (fix new diags same turn) → commit → … → **create_pr + submit_pr** when done (host publishes session to PR branch).  
**Never session-merge to main.** Humans review in the IDE **PR Wizard** then Merge. Agent replies append to PR history when `active_pr_id` is set.  
The process remembers the **active work line** per project so subsequent MCP calls without a session header still hit the feature branch.

## Env

| Var | Meaning |
|-----|---------|
| `VEIL_SESSIONS` | `1` / `0` / `auto` (auto = on when source mode ≠ disk) |
| `VEIL_WS_ROOT` | Workdir root (default `$TMP/veil-ws`) |
| `VEIL_DEV_USER` | Session owner id (default `$USER`) |
| `VEIL_SOURCE_MODE` | `s3` / `prefer_s3` / `disk` |
| `VEIL_DDB_TABLE` / `BUCKET` | Durable store |

## Draft / branch isolation

`POST /api/sessions` with `"draft": true` **or** `"branch_name": "fix-foo"` writes under  
`repos/{repo_id}/drafts/{session_id}/` instead of the shared product branch tree.

Commits snapshot to `repos/{repo_id}/commits/{session_id}/{short_id}/`.  
Merge syncs the workdir to `repos/{repo_id}/{base_branch}/` (default `main`).

## Client

- `localStorage['veil.coding.sessionId']`
- Runtime agent: `ensureCodingSession`, `hydrateFromServer`
- IDE: `ideRequestHeaders` injects `X-Veil-Session-Id`; `scheduleAutosave` for free-text
- IDE top bar **SessionStatus** chip: Synced / Saving / Saved / Conflict
- Agent dock shows session slug · revision when status API reports open handles
- Sticky default session per user+slug (server `.sticky/` + DDB list) avoids creating a new session every page load
- **Identity:** product slug and repo UUID are the same project. Sticky is dual-written (`agent-registry.session` + `{uuid}.session` → one session_id) so `/projects/{uuid}/ide` and agent scope on the slug share one workdir. `write_source` bumps `revision` / `dirty` (Uncommitted) and rematerializes peer mainline workdirs for that repo.
- Response headers on durable writes: `X-Veil-Session-Id`, `X-Veil-Revision`, `X-Veil-Etag`
- Idle in-memory handle reaper every 5m (`VEIL_SESSION_TTL_SECS`, default 86400)

## Failure matrix

| Failure | Result |
|---------|--------|
| Browser crash | Restore session id → GET turns + attach workdir from S3 |
| ProductHost restart | Attach rematerializes if workdir missing |
| S3 put fails | Error to client; no false “saved” |
| Etag conflict | HTTP 412; client re-reads and retries |
