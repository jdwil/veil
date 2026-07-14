# IDE agent platform — handoff for agents

Operational map of the **dual-loop IDE agent stack** as implemented in this
tree. Read this before changing agent dock, ACP, dev-loop, or Aether wiring.

Related: [AGENT.md](./AGENT.md), [MIND_PALACE.md](./MIND_PALACE.md),
[HARNESS.md](./HARNESS.md), [CODEGEN_TEMPLATES.md](./CODEGEN_TEMPLATES.md),
[MISSION.md](../MISSION.md).

## Architecture (one sentence)

**veil-viewer** (SvelteKit) talks to **veil-server** over REST + SSE + WebSocket;
the Agent dock uses **@aether-ui/core** streaming events over `/api/chat`; optional
**Mind Palace** wiki tools and **Kiro via ACP** run inside the serve process.

```
┌──────────────── veil-viewer (:5173) ────────────────┐
│  ReviewDock: Source | Agent | Split                 │
│  AetherAgentPanel + agentSession.ts (in-memory)     │
│  DevToolbar → /api/dev/*  (veil.toml [[targets]])   │
└───────────────┬─────────────────────────────────────┘
                │ HTTP / WS  (ideApiBase → :3001)
┌───────────────▼──────── veil-server ────────────────┐
│  /api/*  single-project   or  /api/p/{name}/* multi │
│  /chat  → aether_chat → agent_stream / ACP / Rig    │
│  /dev/* → devloop (gen + spawn dev_command)         │
│  /mcp   → tools for Kiro ACP workspace mcp.json     │
│  wiki_* → mind-palace (git dep) when MIND_PALACE=1  │
└─────────────────────────────────────────────────────┘
```

## Serve modes

| Mode | How | Routes |
|------|-----|--------|
| **Single-project** | `make serve PROJECT=/path/to/product` or `veil serve <path>` | `/api/ir`, `/api/chat`, `/api/dev/…` |
| **Multi-project** | `veil serve --multi` / runtime hub | `/api/p/{project}/…` + hub `/api/projects` |

Single-project **must** inject DevLoop state (`build_router` + `Extension(SharedDevLoops)`).
Without it `/api/dev/targets` 500s and the DevToolbar hides.

`CURRENT_PROJECT` is task-local under multi-project only. Dev handlers fall back to
`project_display_name(project_root)` in single-project (`devloop_api::resolve_project_key`).

## Agent dock (viewer)

| File | Role |
|------|------|
| `veil-viewer/src/lib/AetherAgentPanel.svelte` | UI: MessageList + ChatInput from Aether |
| `veil-viewer/src/lib/agentSession.ts` | **Shared** messages + `StreamService`; survives Agent↔Split remounts |
| `veil-viewer/src/lib/ReviewDock.svelte` | Tabs; keeps Source+Agent **mounted**, hides inactive pane (`hidden` + CSS) |
| `veil-viewer/src/app.css` | Tailwind v4 `@source` for Aether; theme tokens; typography plugin |
| `veil-viewer/vite.config.ts` | `optimizeDeps.exclude: ['@aether-ui/core']` (source `.svelte.ts`) |

### Chat history — do not assume persistence

| Event | UI transcript | ACP/Kiro memory |
|-------|---------------|-----------------|
| Agent ↔ Split / dock resize | Kept (`agentSession`) | Unchanged |
| Full page reload / HMR hard refresh | **Lost** (in-memory only) | Survives **if** `veil serve` process still running |
| `make serve` restart | Lost | **Lost** (new process, new `session/new`) |

**Frontend every turn:** sends full `messages[]` in Aether `ChatRequest`.

**Backend ACP path:** `aether_chat::extract_prompt` uses only the **last user**
message. Continuity across turns (same serve process) is primarily **Kiro’s
long-lived ACP session** (`static ACP` in `acp.rs`), not the FE history payload.

**Not implemented:** `sessionStorage` rehydrate of the dock; multi-turn transcript
re-feed into ACP after server restart.

### Aether UI dependency

```bash
cd veil-viewer && npm install github:jdwil/aether-ui
# or: ./scripts/sync-git-deps.sh
```

- Package: `@aether-ui/core` → monorepo root with **source exports** under
  `packages/aether-ui/src/lib`.
- Tailwind: `@source '../node_modules/@aether-ui/core/packages/aether-ui/src/lib'`
  (v4 does not scan `node_modules` by default — missing this → unstyled bubbles +
  visible native file picker instead of `sr-only`).
- Host dark mode: `class="dark"` on `<html>` when theme is dark (Aether `dark:` utils).
- **Do not** hardcode Vite/Tailwind UI in the engine; Aether is a separate product.

Repo: https://github.com/jdwil/aether-ui

## Dual-loop product targets (`veil.toml`)

Example (`wear_test`):

```toml
[[targets]]
name = "backend"
package = "wear_test.veil"
target = "rust"
output = "generated/backend"
dev_command = "… cargo run …"
dev_port = 3000

[[targets]]
name = "frontend"
package = "wear_test_ui.veil"
target = "typescript"
output = "generated/frontend"
dev_command = "npm install --silent && npx vite dev --port 5174"
dev_port = 5174
```

- DevToolbar: per-target ▶/■ or **All targets** (POST `/dev/start` with no `name` → `start_all`).
- On start: `veil gen <package> -t <target> -o <output>` then spawn `dev_command` in output dir.
- **Binary for gen:** `VEIL_BIN` env, else `std::env::current_exe()` (the running `veil serve`),
  else PATH `veil`. Makefile sets `VEIL_BIN` when launching serve.
- Empty `[[targets]]` → toolbar hidden (not an error).

### Frontend API proxy (`@proxy`)

UI packages call **relative** `/api/*`. Vite must proxy to the backend `dev_port`.

**Authoring (annotation must lead the construct):**

```veil
pkg MyUi
  use sveltekit5
  @proxy("/api", "http://127.0.0.1:3000")
  app MyApp
    …
```

Annotations **inside** the `app` block before `group` attach to the **next child**
(e.g. `group pages`), not the app — place `@proxy` **before** `app`.

**Codegen:** `layers/sveltekit5.layer` match `* where has_annotation("proxy")`
emits `vite.config.ts` via template (not hardcoded Vite in `typescript.rs`).

```
{{annotation_arg:proxy:0}}   # path
{{annotation_arg:proxy:1}}   # target URL
```

Engine invariant: **framework opinions live in layers**; template engine only
provides generic annotation interpolation (`template.rs`).

## ACP / Kiro

| Piece | Detail |
|-------|--------|
| Provider | `VEIL_MODEL_PROVIDER=acp` (Makefile default for local serve often ACP) |
| Process | One process-wide child (`static ACP`); `session/new` once per process |
| MCP | Workspace `.kiro/settings/mcp.json` with **url only**; `session/new` gets `mcpServers: []` (non-empty array crashes Kiro 2.12) |
| After turn | Reload project from disk; IDE SSE refresh |

See [AGENT.md](./AGENT.md) env table and [ACP_SPIKE.md](./ACP_SPIKE.md).

## Mind Palace

Optional wiki tools when `MIND_PALACE=1` + AWS. Cargo git deps:

```toml
mind-palace = { git = "https://github.com/jdwil/mind-palace" }
mind-palace-rig = { git = "https://github.com/jdwil/mind-palace" }
```

**Region must match stack** (dashlx_dev: `us-west-2`). Seed via agent:
`seed mind palace` or `./scripts/seed_mind_palace.sh`.

Full env: [MIND_PALACE.md](./MIND_PALACE.md).

## MISSION invariants (do not regress)

1. **Zero domain knowledge in the engine** — no Vite/Svelte/AWS hardcoding in
   codegen backends; layers own emission policy.
2. **Bus ≠ browser transport** — product UI uses HTTPS + Vite proxy; Bus is
   backend IPC only ([HARNESS.md](./HARNESS.md)).
3. **Stubs, not hardcoded SDKs** — cloud crates via `.stub` (`cargo_deps`, etc.).
4. **Aether/Mind Palace stay separate repos** — integrate by git/npm dep, not monorepo vendoring of their source trees into VEIL (vendor clone was transitional; prefer GitHub install).

## Key source map

| Concern | Path |
|---------|------|
| Aether WS bridge | `crates/veil-server/src/aether_chat.rs` |
| Agent turn / ACP / Rig | `crates/veil-server/src/agent.rs`, `acp.rs`, `model.rs` |
| Dev loop | `crates/veil-server/src/devloop.rs`, `devloop_api.rs` |
| Router single vs multi | `crates/veil-server/src/api.rs` (`build_router` / `build_multi_router`) |
| MCP tools | `crates/veil-server/src/mcp.rs` |
| Wiki tools | `crates/veil-server/src/mind_palace_tools.rs` |
| TS + layer templates | `crates/veil-codegen/src/typescript.rs`, `template.rs` |
| SvelteKit5 layer | `layers/sveltekit5.layer` |
| Svelte5 + `@proxy` ann | `layers/svelte5.layer` |

## Smoke checklist

```bash
# API + viewer
make serve PROJECT=$VEIL_PROJECTS_DIR/wear_test

# Dev toolbar should list backend/frontend; ▶ All runs both
curl -s http://127.0.0.1:3001/api/dev/targets | jq .

# Gen frontend must emit proxy when @proxy is on app
veil gen path/to/*_ui.veil -t typescript -o /tmp/fe && cat /tmp/fe/vite.config.ts

# Aether styles
# open Agent tab — no native "Browse… No files selected"; bubbles laid out
```
