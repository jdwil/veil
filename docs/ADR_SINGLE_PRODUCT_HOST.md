# ADR: Single ProductHost — IDE native in veil-runtime

## Status

Accepted (2026-08-04). Updated 2026-08-14: host lives at `crates/veil-runtime`
+ `ui/` (the old `runtime/` dogfood tree is gone). Replaces operational
dual-process setup (`veil_bin` / platform API on one port + `veil serve --multi`
on another).

## Context

We had:

| Process | Port (typical) | Role |
|---------|----------------|------|
| Generated `veil_bin` or trampoline bus | 3000 | Platform API (repos, changes, deploy) |
| `veil serve --multi` | 3001 | IDE dual-loop + agent WS + MCP |
| Vite `ui/` | 5180 | Shell SPA, proxies split to 3000/3001 |
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

3. **IDE frontend is native in `ui/`** at `/projects/{slug}/ide`. The old
   standalone `ide-ui` / `/viewer` embed is retired. No separate
   “embedded multi-project veil serve” for day-to-day runtime UX.

4. **Agent is same-origin**: shell agent WebSocket is
   `/api/agent/chat` (and MCP `/api/mcp` or project-scoped `/api/p/{name}/mcp`)
   on the ProductHost — not a second process.

## Non-goals

- Deleting `veil serve` for language/package dual-loop development.
- Rewriting IDE dual-loop in VEIL (stays Rust kernel + viewer UI).

## Consequences

- Dev: one backend port for `ui/` Vite proxy (all `/api` + `/api/agent`).
- **Product IDE is native** at `/projects/{slug}/ide` inside `ui/` (no iframe,
  no second SPA navigation). Dual-loop UI lives under `ui/src/lib/ide/`.
- Ops: stop running `veil serve --multi` just to feed the dashboard agent.
- Platform domain HTTP (repos, pull_requests, deploy, registry) is mounted on
  ProductHost via `crates/veil-runtime` (`platform_http.rs` + AWS/local ports).
  No separate `veil_bin` on :3000 for the dashboard.

## Verification

```bash
# ProductHost only (see .env.example)
scripts/dev-stack.sh restart
scripts/dev-stack.sh smoke

curl -s localhost:8080/api/projects | head
curl -s localhost:8080/api/repos | head
curl -s localhost:8080/api/pull_requests | head
curl -s localhost:8080/api/deploy_environments | head
# Agent MCP on same origin
curl -s -X POST localhost:8080/api/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | head
```
