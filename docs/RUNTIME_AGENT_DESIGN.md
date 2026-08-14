# Runtime-Level Omnipresent Agent — Architecture Design

> **2026-08-14:** Paths below that say `runtime/ui` or `veil-viewer` mean `ui/`
> today. ProductHost is handwritten; AgentDock lives in `ui/src/lib/agent/`.

## Overview

Lift the `@aether-ui/core` agent from its current IDE-only scope to the
**veil-runtime shell level**, making an AI agent available on every page of the
runtime. The agent can control the entire veil UX — IDE editing, SDLC/VCS
operations, deployment, project switching, bus inspection — regardless of which
page the user is currently viewing.

The mental model: the agent is to the runtime what Spotlight/Alfred is to macOS —
always one keystroke away, context-aware of the current page, and able to
navigate to and operate any surface in the system.

---

## Current State

### IDE Agent (veil-viewer)
- `AetherAgentPanel.svelte` renders `MessageList` + `ChatInput` from `@aether-ui/core`
- `agentSession.ts` holds in-memory conversation state, survives panel remounts
- WebSocket connects to `veil-server` at `/api/chat` (or `/api/p/{name}/chat` in multi-project)
- Tools: workspace editing, mind-palace wiki, dev-loop (gen/build/run), MCP
- System prompt scoped to "VEIL IDE agent"

### Runtime UI (runtime/ui)
- SvelteKit app with Sidebar nav + page routes: dashboard, projects, changes, deploy, registry, bus, agents, config
- `AgentSurface.svelte` already collects `data-veil-agent` DOM contracts into `window.__veilAgentSurface`
- No `@aether-ui/core` dependency today (runtime `package.json` has no aether)
- Backend: generated Axum server from `runtime.veil` + `runtime-ui.veil`

---

## Design

### 1. Agent Dock — Shell-Level Panel

The agent lives in the **root layout** (`+layout.svelte`) as a persistent
slide-out panel anchored to the right edge of the viewport. It is **not** inside
any route component, so it survives navigation.

```
┌──────────────────────────────────────────────────────────────────┐
│ [Sidebar]  │              Page Content              │  [Agent]   │
│            │                                        │  (slide)   │
│  Dashboard │    /projects, /changes, /deploy, …    │  ┌──────┐  │
│  Projects  │                                        │  │Chat  │  │
│  Changes   │    IDE iframe lives at /projects/[id]  │  │Input │  │
│  Deploy    │                                        │  │      │  │
│  …         │                                        │  └──────┘  │
└──────────────────────────────────────────────────────────────────┘
```

**Behaviors:**
- **Toggle**: `Cmd+K` / floating button in status bar → slide panel open/close
- **Persists across navigation**: conversation and stream survive route changes
- **Resizable**: drag handle on left edge (like VS Code sidebar)
- **Collapsed state**: thin strip with badge showing unread/active streaming
- **Context injection**: on open or per-message, inject current page context

### 2. Agent Session — Runtime-Scoped

Port the `agentSession.ts` pattern from veil-viewer into a new
`src/lib/agent/runtimeAgentSession.ts`:

| Concern | IDE Version | Runtime Version |
|---------|-------------|-----------------|
| Scope | Single project IDE | Entire runtime (all projects) |
| WebSocket URL | `/api/chat` (veil-server) | `/api/agent/chat` (runtime backend) |
| Persistence | In-memory only | `sessionStorage` + localStorage handoff |
| System prompt | "VEIL IDE agent" | "VEIL Runtime agent" — full capabilities |
| Context | Selected node, current layer/view | Current page, project, selection |
| Tools | Edit, gen, dev-loop, wiki | All of the above + SDLC, deploy, nav, project-switch |

**Session state structure:**

```typescript
interface RuntimeAgentState {
  messages: Message[];
  isStreaming: boolean;
  isThinking: boolean;
  error: string | null;
  statusLine: string;
  activeProject: string | null;      // which project has focus
  currentPage: string;               // e.g. "/changes", "/projects/relay"
  pendingSeed: string;
}
```

### 3. Context Protocol — Session Focus + Intent + Present

**Law (2026-08):** See [ADR_FOCUS_INTENT_PRESENT.md](./ADR_FOCUS_INTENT_PRESENT.md).

| Primitive | Role |
|-----------|------|
| **SessionFocus** | Continuous: route, project, construct, file, form, diagnostics. UX publishes; every turn injects `ChatRequest.focus`. |
| **Intent** | Discrete command (`CreateProject`, `Navigate`, …) with actor + optional Present. |
| **Present** | UX choreography (goto → fill → pulse) so the operator *sees* agent product ops. |

Each page still contributes `data-veil-agent` / AgentSurface contracts. Focus is
authoritative for deictic language ("this component"). `get_current_context`
returns the structured snapshot.

```
## Session focus
- Route: /projects/relay/ide
- Project: relay
- Construct: RelayAuth (aggregate)
```

### 4. Tool Registry — Superset

The runtime agent has a **superset** of tools compared to the IDE agent:

| Category | Tools | Notes |
|----------|-------|-------|
| **IDE/Editing** | `write_source`, `read_source`, `create_file`, `list_files`, `veil_check`, dual-loop `dev_*` | Active project via MCP / Rig |
| **Projects** | `create_project`, `list_projects`, `get_project`, `delete_project`, `open_project`, `open_ide` | `POST/GET /api/repos` + disk scaffold; **not** wiki-only |
| **SDLC** | `create_pr`, `list_prs`, `get_pr`, `submit_pr`, `approve_pr`, `request_pr_changes`, `merge_pr`, `add_comment`, `get_pr_diff` | Real CM APIs + SPA navigation |
| **Deploy** | `provision_project`, `plan_provision`, `deploy_status`, `list_deploy_environments`, `get_provision_job` | ProductHost deploy routes |
| **Navigation** | `navigate_to`, `switch_project`, `open_*` | SPA `navigation` in tool result |
| **Registry** | `search_registry`, `list_registry_layers`, `list_registry_stubs` | Lightweight listing |
| **Wiki/Memory** | `wiki_search`, `wiki_read`, `wiki_create`, `wiki_update` | Mind-palace (when enabled) |
| **Config** | `get_config`, `get_mission`, `update_mission` | Runtime settings + product intent |
| **Meta** | `get_current_context` | Situational awareness |

Implementation: `crates/veil-server/src/platform_tools.rs` (MCP + Rig + host short-circuit).

### 5. Navigation & Cross-Project Control

The agent can **navigate the UI** programmatically. When the user asks "open the
relay project and add an authorize step", the agent:

1. Calls `navigate_to({ path: '/projects/relay' })` for project detail, or
2. Calls `open_ide({ project: 'relay' })` → SPA route `/projects/relay/ide`
   (iframe to `/viewer/?project=relay&showAgentRail=0`; **AgentDock stays in shell**)
3. Calls `edit_file(...)` via the project API — the IDE graph refreshes live (SSE)

**Implementation**: Navigation tools emit SPA navigation handled by root layout
(`onAgentNavigation` in `runtime/ui`). `open_ide` never full-page redirects to
`/viewer` (that would drop the runtime agent). Standalone IDE (own agent chrome)
remains at `/viewer/?project=…` or `/ide/{name}`.

For **cross-project operations**, the runtime backend maintains connections (or
spawns on-demand) to per-project veil-server instances:

```
Runtime Backend (port 3003)
  ├── /api/agent/chat          ← Agent WebSocket (runtime-level)
  ├── /api/p/{project}/…       ← Hub proxy to per-project veil-serve
  ├── /api/pull_requests/…   ← SDLC services
  └── /api/deploy/…            ← Deploy services
```

### 6. IDE Embed — Bidirectional Communication

The IDE is embedded at `/projects/{name}/ide` as an iframe to
`/viewer/?project={name}&showAgentRail=0` and talks to the runtime agent via
postMessage (`registerIdeFrame` / IDE bridge):

```
Runtime AgentDock ←→ native in-shell IDE (`ui/src/lib/ide`)
```

| Direction | Message | Purpose |
|-----------|---------|---------|
| Agent → IDE | `{ type: 'agent:edit', payload: {...} }` | Agent instructs IDE to edit |
| Agent → IDE | `{ type: 'agent:navigate', node: '...' }` | Focus a specific construct |
| IDE → Agent | `{ type: 'ide:selection', construct: '...' }` | User selected something in IDE |
| IDE → Agent | `{ type: 'ide:error', message: '...' }` | IDE reports a problem |
| Agent → IDE | `{ type: 'agent:refresh' }` | Reload after external edit |

This allows the agent to orchestrate visual changes: "I'm editing the relay
package now" → user sees the IDE update in real-time in the embedded viewer.

### 7. Backend Architecture

```
┌──────────── Runtime SvelteKit UI (:5173 dev / embedded prod) ──────┐
│  +layout.svelte                                                    │
│    ├── Sidebar                                                     │
│    ├── AgentDock (persistent, @aether-ui/core)                     │
│    │     └── runtimeAgentSession.ts → ws://runtime:3003/api/agent  │
│    ├── AgentSurface (context collector)                            │
│    └── {page content}                                              │
│           └── (optional) IDE iframe → veil-viewer at :8080         │
└────────────────────────┬───────────────────────────────────────────┘
                         │ WebSocket
┌────────────────────────▼───────────────────────────────────────────┐
│  Runtime Backend (Axum, :3003)                                     │
│                                                                    │
│  /api/agent/chat  →  AgentRouter                                   │
│    ├── LLM Provider (ACP / Bedrock / Ollama)                       │
│    ├── Tool Registry (runtime tools)                               │
│    │     ├── SDLC tools → PullRequestManagement services                │
│    │     ├── Deploy tools → LocalDeployExec services               │
│    │     ├── IDE tools → proxy to per-project veil-server          │
│    │     ├── Nav tools → return navigation commands to frontend    │
│    │     ├── Wiki tools → mind-palace                              │
│    │     └── Config/Meta tools → runtime state                     │
│    └── Context injection (page, project, surfaces)                 │
│                                                                    │
│  /api/p/{project}/…  →  Hub proxy (existing multi-project)         │
│  /api/pull_requests/…  →  PullRequestManagement aggregate             │
│  /api/deploy/…  →  DeployExec                                      │
└────────────────────────────────────────────────────────────────────┘
```

### 8. Streaming Protocol

Reuse the Aether streaming protocol (WebSocket, same event schema):

```
→ Client: {"messages":[...],"context":{page, project, surfaces},"systemPrompt":"..."}
← Server: {"event":"message_start","data":{...}}
← Server: {"event":"content_delta","data":{...}}
← Server: {"event":"tool_call_start","data":{...}}  // agent using a tool
← Server: {"event":"tool_result","data":{...}}       // tool output
← Server: {"event":"content_delta","data":{...}}     // more response
← Server: {"event":"done","data":{...}}
```

**Extension**: add a `navigation` event type for client-side actions:

```
← Server: {"event":"navigation","data":{"action":"goto","path":"/projects/relay"}}
← Server: {"event":"navigation","data":{"action":"open-ide","project":"relay"}}
```

The frontend handles `navigation` events by executing the action (SvelteKit
`goto()`, opening panels, switching tabs) rather than displaying them.

### 9. Keyboard UX

| Shortcut | Action |
|----------|--------|
| `Cmd+K` | Toggle agent panel |
| `Cmd+Shift+K` | Focus agent input (open panel if closed) |
| `Escape` | Close agent panel (when focused) |
| `Cmd+Shift+P` | Agent command palette (structured actions) |

### 10. Agent Identity & System Prompt

The runtime agent uses a broader system prompt than the IDE agent:

```
You are the VEIL Runtime agent. You have full control over the veil platform:
- Edit code in any project via the IDE
- Manage the SDLC: create/review/merge changes
- Deploy projects to environments
- Navigate the UI to show the user what you're doing
- Inspect the bus, registry, and configuration
- Remember knowledge in the wiki

When the user asks you to do something:
1. If it requires navigating to a different page, do so (they see the transition)
2. If it requires editing code, open the IDE for that project and make changes
3. If it spans multiple projects, switch between them seamlessly
4. Show your work — use navigation so the user sees what's happening

Current context will be injected per turn with page, project, and available actions.
```

---

## Implementation Plan

### Phase 1: Agent Dock in Layout (frontend only, stub backend)
1. Add `@aether-ui/core` to `runtime/ui/package.json`
2. Create `src/lib/agent/runtimeAgentSession.ts` (port from veil-viewer)
3. Create `src/lib/agent/AgentDock.svelte` — slide panel with toggle
4. Wire into `+layout.svelte` — persistent across routes
5. Add `Cmd+K` keyboard shortcut
6. Stub WebSocket endpoint returning echo responses

### Phase 2: Backend Agent Router
7. Add `/api/agent/chat` WebSocket route to runtime backend
8. Wire LLM provider (start with ACP, fall back to Bedrock/Ollama)
9. Implement tool registry skeleton — `get_current_context`, `navigate_to`
10. Context injection from client's `context` field in `ChatRequest`

### Phase 3: Navigation & IDE Integration
11. Implement `navigate_to` tool — returns `navigation` events
12. `open_ide` tool — activates IDE embed on project detail page
13. `postMessage` bridge between agent panel and IDE iframe
14. IDE tools proxy: agent → runtime backend → per-project veil-server

### Phase 4: SDLC & Deploy Tools
15. Wire `create_pr`, `approve_pr`, `merge_pr` tools to change-management services
16. Wire `deploy_project` tool to LocalDeployExec
17. Wire `list_prs`, `deploy_status` read tools

### Phase 5: Polish
18. `sessionStorage` persistence (survive page reload)
19. Command palette (`Cmd+Shift+P`)
20. Streaming tool-call visualization (user sees "Deploying relay…" with progress)
21. Notification badge on collapsed panel
22. Dark mode parity with existing designkit theme

---

## Dependencies & Constraints

| Dependency | Status | Action |
|------------|--------|--------|
| `@aether-ui/core` | Used in veil-viewer | Add to runtime/ui package.json |
| Tailwind v4 | Runtime UI uses designkit theme (not Tailwind) | Add Tailwind + @source for Aether |
| Runtime backend LLM | Not implemented | Add rig/aether_chat equivalent |
| Mind-palace | Available (git dep) | Wire into runtime backend |
| Per-project veil-server proxy | Hub routing exists in veil-server | Expose from runtime backend |
| IDE iframe postMessage | Not implemented | New protocol, both sides |

### Invariants (do not violate)

1. **Aether stays a separate repo** — install via git dep, no vendoring
2. **Agent dock does not block page rendering** — lazy-load, off-screen until toggled
3. **Conversation survives navigation** — session state lives above route components
4. **IDE agent continues to work standalone** — the veil-viewer's agent dock is unchanged; the runtime agent is additive
5. **Tools are backend-only** — no secret/credential exposure to the frontend; tools execute server-side

---

## Open Questions

1. **~~Dual agent coordination~~**: RESOLVED — Single shared session. The runtime owns the session (`runtimeAgentSession.ts`). The IDE viewer receives session state via postMessage (`agent:session-state` events forwarded on every message update). There is no separate IDE agent — one agent controls everything. When working within a project's IDE embed, the runtime agent proxies tool calls to that project's veil-server. The veil-viewer's standalone `agentSession.ts` remains for direct `veil serve` development without the runtime, but when the viewer is embedded in the runtime, the runtime's session takes precedence.

2. **Model routing**: Should the runtime agent use a different/larger model than the IDE agent? The runtime agent has broader scope (SDLC, deploy, multi-project) which may benefit from a more capable model. Proposal: configurable per-tool model routing (cheap model for navigation, capable model for code edits).

3. **Concurrency**: Can the agent execute tool calls in parallel across projects? The backend should support parallel tool execution for operations that don't conflict.

4. **Auth boundary**: Runtime agent tools include deploy and merge — these are privileged operations. Should the agent require re-authentication for destructive actions? Proposal: human-in-the-loop approval (Aether's `tool_approval` event) for deploy/merge tools.
