# ADR: Single ProductHost — IDE native in veil-runtime

## Status

Accepted (2026-08-04). Replaces operational dual-process setup
(`veil_bin` / platform API on one port + `veil serve --multi` on another).

## Context

We had:

| Process | Port (typical) | Role |
|---------|----------------|------|
| Generated `veil_bin` or trampoline bus | 3000 | Platform API (repos, changes, deploy) |
| `veil serve --multi` | 3001 | IDE dual-loop + agent WS + MCP |
| Vite `runtime/ui` | 5180 | Shell SPA, proxies split to 3000/3001 |
| Vite `veil-viewer` | 8080 | IDE frontend (or static `/viewer`) |

That split forced the agent onto “IDE multi-serve” while the dashboard hit
another backend, and made “Open changes / control the UX” brittle.

## Decision

1. **One host process for product UX**: `veil_server::ProductHost` (runtime
   bootstrap / pure-runtime). It mounts:
   - Shell SPA (platform dashboard)
   - **IDE kernel** (same crate: `crates/veil-server`) as multi-project API
   - **IDE UI** static at `/viewer` (same origin)
   - Platform bus / change-management / deploy routes as needed

2. **IDE backend is already its own crate**: `crates/veil-server`  
   Runtime **links** it (does not reimplement IR/edit/agent/MCP).  
   CLI `veil serve <path>` remains a **thin single-project** convenience for
   package authors — not required for the product runtime dashboard.

3. **IDE frontend lives under runtime**: sources move from monorepo root
   `veil-viewer/` → **`runtime/ide-ui/`**. Build output still lands in
   `runtime/bootstrap/static/viewer` for same-origin serve. No separate
   “embedded multi-project veil serve” dependency for day-to-day runtime UX.

4. **Agent is same-origin**: shell agent WebSocket is
   `/api/agent/chat` (and MCP `/api/mcp` or project-scoped `/api/p/{name}/mcp`)
   on the ProductHost — not a second process.

## Non-goals

- Deleting `veil serve` for language/package dual-loop development.
- Rewriting IDE dual-loop in VEIL (stays Rust kernel + viewer UI).

## Consequences

- Dev: one backend port for `runtime/ui` Vite proxy (all `/api` + `/api/agent`).
- Build: `make pure-runtime-build` builds `runtime/ide-ui` into `static/viewer`.
- Ops: stop running `veil serve --multi` just to feed the dashboard agent.
- Platform domain HTTP (repos, change_requests, deploy, registry) is mounted on
  ProductHost via `runtime/bootstrap/src/platform_http.rs` (generated crates +
  AWS/local ports). No separate `veil_bin` on :3000 for the dashboard.

## Verification

```bash
# ProductHost only
VEIL_PORT=8080 AWS_PROFILE=dashlx_dev VEIL_DDB_TABLE=veil-runtime-dev BUCKET=veil-runtime-dev \
  ./runtime/bootstrap/target/release/veil-runtime

curl -s localhost:8080/api/projects | head
curl -s localhost:8080/api/repos | head
curl -s localhost:8080/api/change_requests | head
curl -s localhost:8080/api/deploy_environments | head
# Agent MCP on same origin
curl -s -X POST localhost:8080/api/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | head
```
