# veil-contract-dual-loop-smoke

**Type:** Concept  
**Summary:** Edit → smoke (gen+check) → list_routes → restart → http_request. On reject: logs first.

## Contract

- After backend HTTP edits: **smoke** runs (gen + `cargo check`).
- Smoke fail → **WRITE REJECTED**, file restored — call `dev_logs` / `smoke_status` before rewrite.
- Smoke ok → `list_routes` → `dev_restart` (or auto-restart ACS-004) → `http_request`.
- Do not claim success without a live `http_request` to the real route.
- Escape: `VEIL_AGENT_SMOKE=0`, `VEIL_AGENT_AUTO_RESTART=0` only when deliberate.

## Example

```text
write_source → smoke OK
  → list_routes
  → dev_restart (or auto)
  → http_request(path="/api/items", target=backend)
```

**Source of truth:** `docs/AGENT.md`, `docs/HARNESS.md`
