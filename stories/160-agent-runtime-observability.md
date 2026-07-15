# Agent runtime observability — see gen, logs, routes, live HTTP

**Goal:** The in-IDE agent can **observe the dual-loop runtime** the same way a
human does: gen/check/smoke output, generated harness routes, and live HTTP
probes — so it stops guessing paths and bouncing on 404s.

**Status:** Done · P1  
**Depends on:** AGT tools surface ([100](100-ide-agent.md)), dual-loop / devloop
([70](70-runtime-harness.md), `devloop_api`), smoke gate after writes (in-tree)  
**Mission impact:** Agents are primary authors; without system-state tools they
cannot close the build loop. This epic makes vibe-coding against `wear_test` +
backend dual-loop productive.

**Related**

- Agent tools / ACP+MCP: [100-ide-agent.md](100-ide-agent.md)  
- Local harness routes: [70-runtime-harness.md](70-runtime-harness.md) · [docs/HARNESS.md](../docs/HARNESS.md)  
- Codegen targets: [60-codegen-targets.md](60-codegen-targets.md)

**Non-goals**

- Browser network capture / frontend CDP  
- Unscoped shell or SSRF-capable HTTP  
- Replacing human review of topology  

---

## Problem (observed)

The IDE agent fails to ship working stack changes because:

1. **Knowledge gap** — does not know how local REST harness derives routes; often
   invents `@route` paths that codegen ignores; assumes `@main` is required.
2. **No system visibility** — cannot read dual-loop logs, generated
   `veil_bin/src/main.rs`, or curl the running backend.
3. **Circular confusion** — writes VEIL → cannot tell if gen/smoke/server picked
   it up → flails across files.

**Already landed (reference, not this epic’s todo):**

- Write smoke: after agent `write_source`, gen + `cargo check` (package-scoped);
  reject + restore previous source/gen on failure (`VEIL_AGENT_SMOKE`, default on).

---

## Architecture (target)

```
┌─────────────────────────────────────────────────────────────┐
│  Agent (ACP MCP or Rig tools)                               │
│  edit → smoke → dev_logs / read_generated / list_routes     │
│       → http_request → (optional) dev_restart               │
└─────────────┬───────────────────────────────────────────────┘
              │ tools
              ▼
┌─────────────────────────────────────────────────────────────┐
│  veil-server                                                │
│  • Source tools (existing)                                  │
│  • DevLoop logs / status / restart                          │
│  • Generated tree read (allowlisted under target outputs)   │
│  • Scoped HTTP to veil.toml dev_port hosts                  │
└─────────────┬───────────────────────────────────────────────┘
              │
     ┌────────┼────────────┐
     ▼        ▼            ▼
  .veil    generated/    cargo run :dev_port
  sources  veil_bin …    live API
```

**Tool surface rule:** every new capability ships on **MCP + Rig** and is listed
in Tier-0 agent preamble (`agent_context`).

---

## Epic outcomes

1. Agent can read dual-loop **status + logs** without the UI.
2. Agent can read **generated** Rust/TS under target outputs (and route extract).
3. Agent can **HTTP-probe** only configured local `dev_port`s.
4. Teaching materials match **actual** harness route rules (and/or codegen
   honors `@route` — see AGT-026).
5. After successful smoke, running backend can be **restarted** so new routes
   are live (no stale `cargo run` binary).
6. Docs + stories board updated; acceptance tests for tools + safety.

---

## Stories

### AGT-020: `dev_status` tool — Done · P1

**As an** IDE agent  
**I want** to query dual-loop target status  
**So that** I know which backends are running, their ports, and last errors

**Acceptance criteria:**

- [ ] Tool `dev_status` on MCP + Rig (optional `name` filter)
- [ ] Returns per-target: `name`, `status`, `package`, `target`, `output`,
      `dev_port`, `attached`, `last_gen`, `last_error`
- [ ] Uses existing DevLoop map (`get_or_create_dev_loop` / global loops)
- [ ] Read-only; no side effects
- [ ] Listed in Tier-0 / TIER0_ACP tool list with one-line “when to use”
- [ ] Unit or integration test: project with `veil.toml` returns ≥1 target

**Depends:** devloop + `veil.toml` targets  
**Mission impact:** Agent stops assuming server state  

**Touch:** `mcp.rs`, `rig_tools.rs`, `agent_context.rs`, optional thin helper
in `devloop.rs` / `devloop_api.rs`

---

### AGT-021: `dev_logs` tool — Done · P1

**As an** IDE agent  
**I want** to read dual-loop log lines (gen / check / smoke)  
**So that** I can diagnose compile failures without the human pasting logs

**Acceptance criteria:**

- [ ] Tool `dev_logs` on MCP + Rig: args `name?` (target), `tail?` (default 40,
      max 200)
- [ ] Returns ring-buffer lines already stored on `TargetState.logs`
- [ ] Surfaces `[gen]`, `[check]`, `[smoke]`, `[dev]` lines when present
- [ ] Empty logs → clear message (“no logs yet; start target or make a write”)
- [ ] Tier-0: “After WRITE REJECTED or 404, call `dev_logs` then fix”
- [ ] Test: after simulated log push, tool returns those lines

**Depends:** AGT-020 (can ship same PR)  
**Mission impact:** Makes smoke rejections actionable  

**Touch:** `mcp.rs`, `rig_tools.rs`, `devloop_api` patterns, `agent_context.rs`

---

### AGT-022: `read_generated` tool — Done · P1

**As an** IDE agent  
**I want** to read files under the project’s codegen output dirs  
**So that** I can see what routes/adapters the harness actually contains

**Acceptance criteria:**

- [ ] Tool `read_generated` on MCP + Rig
- [ ] Args: `path` (relative) **or** `what` preset (`harness` | `routes` | optional
      later `crate_lib`)
- [ ] Optional `max_chars` (default ~12k) with truncation marker
- [ ] Optional `list` / list-only mode under a prefix
- [ ] **Allowlist:** only paths under each `[[targets]].output` from `veil.toml`
      (and/or `generated/` under project root); reject `..` and escapes
- [ ] Preset `what=harness` → each Rust target’s
      `crates/veil_bin/src/main.rs` (concat or labeled sections)
- [ ] Preset `what=routes` → lines matching `.route("` (or structured extract)
- [ ] Default deny outside outputs; error text tells agent the allowlist roots
- [ ] Tier-0: “After adding HTTP surface, `read_generated(what=routes)` before
      claiming a path works”
- [ ] Tests: allow in-output path; deny path outside; harness preset finds main
      when fixture gen tree exists

**Depends:** project_root + parse_project_config  
**Mission impact:** Ends route guessing from VEIL alone  

**Touch:** new helper module or `file_ops`/`devloop`, `mcp.rs`, `rig_tools.rs`,
`agent_context.rs`, tests

---

### AGT-023: `http_request` tool (scoped) — Done · P1

**As an** IDE agent  
**I want** to call the local product HTTP API  
**So that** I can verify routes and status codes against the running server

**Acceptance criteria:**

- [ ] Tool `http_request` on MCP + Rig
- [ ] Args: `method` (default GET), `path` (e.g. `/health`, `/api/…`),
      `target?` (dual-loop name → use that target’s `dev_port`), `body?`,
      `headers?` (limited), `timeout_ms?` (default 3000)
- [ ] **SSRF safety:** only `127.0.0.1` / `localhost`; port must be a
      configured `dev_port` from `veil.toml` (or env allowlist
      `VEIL_AGENT_HTTP_PORTS`); reject other hosts/ports
- [ ] Response: status, truncated body (e.g. 8–16KB), selected headers
- [ ] Connection refused → clear error (“is backend started? dev_status”)
- [ ] Tier-0: “After route change + restart, `http_request` `/health` then API”
- [ ] Tests: deny external host; allow mock/local port in unit test with
      httptest**Depends:** AGT-020 (for ports); running server optional for unit deny tests  
**Mission impact:** Closes edit → live verify loop  

**Touch:** `mcp.rs`, `rig_tools.rs`, small HTTP client helper, `agent_context.rs`

---

### AGT-024: Teach local harness pipeline (Tier-0 + layers + docs) — Done · P1

**As an** IDE agent (and human author)  
**I want** accurate teaching about how local HTTP harness works  
**So that** I do not invent `@main` / `@route` myths that break the stack

**Acceptance criteria:**

- [ ] Tier-0 / TIER0_ACP includes a short **Local HTTP harness** section:
  - Context modules get `veil_bin` REST harness even **without** `@main`
  - **Current** route rule: document real behavior (name-derived
    `List`/`Get`/`Create`/`Update`/`Delete` → `/api/…` via `derive_rest_route`,
    **until** AGT-026 ships)
  - Smoke after write; use `dev_logs` / `read_generated` / `http_request`
  - Frontend uses relative `/api` + Vite `@proxy`; Bus ≠ browser transport
- [ ] `layers/harness.layer` and/or `layers/ddd.layer` prompt aligned (no
      contradictory “@main required for HTTP”)
- [ ] `docs/HARNESS.md` updated: module-driven harness + route table + link to
      agent tools
- [ ] Explicit note: `@route` on handlers may be **UI/IR only** until AGT-026

**Depends:** AGT-020–023 for tool names in prompts (can draft tools section as
“planned” then fill)  
**Mission impact:** Fixes knowledge gap without waiting for codegen  

**Touch:** `agent_context.rs`, `layers/*.layer`, `docs/HARNESS.md`

---

### AGT-025: `dev_restart` after successful smoke (or auto-restart) — Done · P1

**As an** IDE agent  
**I want** the running Rust backend to load newly generated code  
**So that** route changes are visible on the next HTTP probe

**Acceptance criteria:**

- [ ] Either:
  - **(A)** Tool `dev_restart(name?)` that stops + starts owned dual-loop
    process for a target, **or**
  - **(B)** Auto-restart owned Rust target after **successful** smoke when
    that target was Running
- [ ] Attached (external) processes: do not kill; return message to restart
      manually or use A only when owned
- [ ] Logs record restart reason (`[dev] restart after smoke` / tool)
- [ ] Tier-0: when to restart vs when smoke rejection means “do not restart”
- [ ] Manual test: change path → smoke OK → restart → `http_request` sees new
      route

**Depends:** AGT-020; smoke gate; process ownership in DevLoop  
**Mission impact:** Fixes silent stale-binary 404s  

**Touch:** `devloop.rs`, `devloop_api.rs`, tools, `agent_context.rs`

---

### AGT-026: Honor `@route` in local Rust harness (codegen) — Done · P1

**As a** package author  
**I want** `@route("GET /api/…")` on services/handlers to control harness paths  
**So that** annotations match the running server (and agent teaching)

**Acceptance criteria:**

- [ ] When a fn/service/handler has `@route` with method+path (or path-only),
      harness uses that path/method instead of `derive_rest_route` name rules
- [ ] Missing `@route` → keep existing name-derived fallback
- [ ] Multi-package harness (`gen-harness`) uses the same rule
- [ ] Codegen tests: annotated route appears in `veil_bin` main; unannotated
      still List/Get/Create…
- [ ] Update AGT-024 teaching to prefer `@route` once this lands
- [ ] `docs/HARNESS.md` route table documents annotation-first policy

**Depends:** GEN harness in `veil-codegen` (`derive_rest_route` call sites)  
**Mission impact:** Aligns author intent, agent mental model, and runtime  

**Touch:** `crates/veil-codegen/src/rust.rs`, tests, docs/layers (with AGT-024)

---

### AGT-027: `list_routes` structured dump — Done · P2

**As an** IDE agent  
**I want** a compact JSON list of harness routes  
**So that** I do not parse full `main.rs` for every turn

**Acceptance criteria:**

- [ ] Tool `list_routes` (or `read_generated(what=routes)` returning structured
      JSON): `[{ method, path, service? }]`
- [ ] Source of truth: generated harness parse **or** IR re-derive matching
      codegen (must match AGT-026 policy)
- [ ] Works for multi-package harness (all context crates)
- [ ] Tier-0 prefers `list_routes` before inventing paths
- [ ] Test against fixture gen output

**Depends:** AGT-022; better after AGT-026  
**Mission impact:** Token-efficient route awareness  

---

### AGT-028: Smoke excerpt + docs for agent loop — Done · P2

**As an** IDE agent  
**I want** rejected writes to point me at the next tool  
**So that** I recover without human help

**Acceptance criteria:**

- [ ] WRITE REJECTED / smoke failure messages mention `dev_logs` and
      `read_generated` by name
- [ ] `docs/AGENT.md` (or HARNESS) section: closed loop
      `edit → smoke → logs/gen → http → restart`
- [ ] `VEIL_AGENT_SMOKE=0` documented as escape hatch only
- [ ] Optional: `smoke_status` tool returning last ok/fail + crates checked

**Depends:** AGT-021–023  
**Mission impact:** Operational SOP for agents  

---

## Delivery order (PR stack)

| PR | Stories | Outcome |
|----|---------|---------|
| **PR1** | AGT-020, AGT-021 | Agent sees status + logs |
| **PR2** | AGT-022 | Agent sees generated harness/routes text |
| **PR3** | AGT-023 | Agent probes live HTTP safely |
| **PR4** | AGT-024 | Teaching matches system (name-derived until 026) |
| **PR5** | AGT-025 | Running server picks up new gen |
| **PR6** | AGT-026 | `@route` drives harness |
| **PR7** | AGT-027, AGT-028 | Sugar + docs polish |

**Suggested first slice:** PR1 + PR2 (observability without HTTP).  
**Highest leverage full slice:** PR1–PR4 in one sprint.

---

## Success criteria (epic DoD)

An agent, unattended by a human pasting logs, can:

1. Edit a package with a new list/create service (or `@route` after AGT-026).
2. Either get **WRITE REJECTED** with cargo errors, or smoke OK.
3. Call `dev_logs` and see `[check] ✓` or the failure.
4. Call `read_generated` / `list_routes` and see the **real** path.
5. After restart (AGT-025), `http_request` that path is non-404 (or clear
   connection error if server down).
6. Not claim a route works without having observed gen or HTTP.

---

## Status board

| ID | Story | Status | P |
|----|-------|--------|---|
| AGT-020 | `dev_status` tool | Done | P1 |
| AGT-021 | `dev_logs` tool | Done | P1 |
| AGT-022 | `read_generated` tool | Done | P1 |
| AGT-023 | `http_request` scoped | Done | P1 |
| AGT-024 | Teach harness pipeline | Done | P1 |
| AGT-025 | `dev_restart` / auto-restart | Done | P1 |
| AGT-026 | `@route` in Rust harness | Done | P1 |
| AGT-027 | `list_routes` structured | Done | P2 |
| AGT-028 | Smoke SOP docs | Done | P2 |
