# Durable coding sessions

Production-like ProductHost keeps **efficient local checkouts** and **never loses accepted work**.

## Invariant

| Layer | Store | Disposable? |
|-------|--------|-------------|
| Browser / tab | view + typing buffer | yes |
| Session workdir | `{VEIL_WS_ROOT}/{user}/{session_id}/{slug}/` | yes (rebuildable) |
| **DDB `SESSION#…/META`** | session metadata, revision, etags | **no** |
| **S3 objects** | file bytes | **no** |
| **DDB `SESSION#…/TURN#…`** | agent transcript | **no** |

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
POST   /api/sessions              { slug, branch?, draft? }
GET    /api/sessions
GET    /api/sessions/{id}
POST   /api/sessions/{id}/attach
POST   /api/sessions/{id}/pull
POST   /api/sessions/{id}/reset
POST   /api/sessions/{id}/flush
GET    /api/sessions/{id}/turns
POST   /api/sessions/{id}/turns
POST   /api/sessions/{id}/ws/{list,read,write,str_replace,grep,rm}
POST   /api/p/{project}/autosave  + X-Veil-Session-Id
```

IDE routes also accept `X-Veil-Session-Id`. If omitted, the hub **auto-creates** a sticky default session for the user+slug.

## MCP workspace tools

`ws_list`, `ws_read`, `ws_write`, `ws_str_replace`, `ws_grep`, `ws_rm`, `ws_pull`, `ws_reset`

All path-jailed to the session workdir. Prefer structured VEIL tools for packages; use `ws_*` for full-tree edits.

## Env

| Var | Meaning |
|-----|---------|
| `VEIL_SESSIONS` | `1` / `0` / `auto` (auto = on when source mode ≠ disk) |
| `VEIL_WS_ROOT` | Workdir root (default `$TMP/veil-ws`) |
| `VEIL_DEV_USER` | Session owner id (default `$USER`) |
| `VEIL_SOURCE_MODE` | `s3` / `prefer_s3` / `disk` |
| `VEIL_DDB_TABLE` / `BUCKET` | Durable store |

## Draft isolation

`POST /api/sessions` with `"draft": true` writes under  
`repos/{repo_id}/drafts/{session_id}/` instead of the shared branch tree.

## Client

- `localStorage['veil.coding.sessionId']`
- Runtime agent: `ensureCodingSession`, `hydrateFromServer`
- IDE: `ideRequestHeaders` injects `X-Veil-Session-Id`; `scheduleAutosave` for free-text
- IDE top bar **SessionStatus** chip: Synced / Saving / Saved / Conflict
- Agent dock shows session slug · revision when status API reports open handles
- Sticky default session per user+slug (server `.sticky/` + DDB list) avoids creating a new session every page load
- Response headers on durable writes: `X-Veil-Session-Id`, `X-Veil-Revision`, `X-Veil-Etag`
- Idle in-memory handle reaper every 5m (`VEIL_SESSION_TTL_SECS`, default 86400)

## Failure matrix

| Failure | Result |
|---------|--------|
| Browser crash | Restore session id → GET turns + attach workdir from S3 |
| ProductHost restart | Attach rematerializes if workdir missing |
| S3 put fails | Error to client; no false “saved” |
| Etag conflict | HTTP 412; client re-reads and retries |
