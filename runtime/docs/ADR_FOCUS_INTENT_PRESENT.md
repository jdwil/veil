# ADR: Session Focus + Intent + Present

**Status:** Accepted  
**Date:** 2026-08-06  
**Related:** [RUNTIME_AGENT_DESIGN.md](./RUNTIME_AGENT_DESIGN.md), palace `decision-session-focus-intent`

## Context

Backend, product UX (incl. IDE), and the agent must stay aware of each other.
Today sync is a star of ad-hoc bridges (tool→route maps, postMessage tokens,
host regex short-circuits, thin page/project context). That feels immature and buggy.

## Decision

Three roles with asymmetric authority — **not** a free-for-all star, **not**
universal Agent→UX→Server for all work:

| Entity | Authority |
|--------|-----------|
| **Server** | Domain source of truth |
| **UX** | Presentation, human I/O, **Focus** |
| **Agent** | Propose **Intents**; domain tools for host work |

### Primitives

1. **SessionFocus** (continuous) — route, project, file, construct, form, diagnostics.
   UX publishes; every agent turn injects it; `get_current_context` returns it.
2. **Intent** (discrete) — typed command any actor can emit (`CreateProject`, `Navigate`, …).
3. **Present** — optional choreography on an Intent (goto → fill → pulse → final route).

### Execution flows

| Class | Path |
|-------|------|
| Human product | User → UX → Server (+ Focus patch) |
| Agent product (visible) | User → Agent → **Present on UX** → (domain already on server *or* UX commit) |
| Agent domain (coding/host) | User → Agent → Server → SSE/Present projection on UX |

**Rule:** No dual silent writers for the same product change. Prefer Present over
ad-hoc `TOOL_NAV` when a tool returns `intent.present`.

### create_project (flagship)

| Path | When | Flow |
|------|------|------|
| **`via=ux`** | Browser pure create (host short-circuit + Focus) | Present form → pulse → `POST /api/ux/create_project` → IDE (true Agent→UX→Server) |
| **`via=server`** | Multi-step prefix, ACP mid-turn, headless | Domain first (DDB+S3) → Present illustrates form → goto (no second POST) |

### create_change

Same dual path; open form-only when no title.

### Intent log + ACK + wait

- Agent + human + UX commits recorded (FE sessionStorage + `POST /api/ux/intent_log` + session META `intent_log` in DDB).
- Present completion → `POST /api/ux/intent_ack`.
- **`wait_intent_ack({intent_id})`** blocks (oneshot) until ACK — call **after** create tool result is streamed (no deadlock). Host multi-step prefix auto-waits ~14s when browser Focus is present.
- Session META stores `last_focus` / `intent_log`; FE hydrates Focus from session GET.
- Pulse targets support `text:Approve` / `text:Merge` for change-detail buttons.

### Change / deploy Present

- `submit_change`, `approve_change`, `request_changes`, `merge_change`, `add_comment` → goto change + pulse + announce
- `plan_provision`, `provision_project` → goto `/deploy` + pulse + announce

## Consequences

- Modules: `focus.ts`, `intent.ts` (commit, human capture, ACK, local log restore)
- Server: `focus.rs`, `/api/ux/*`, SessionMeta.`last_focus` / `intent_log`
- Ban new ad-hoc sync paths unless they implement Focus/Intent/Present
