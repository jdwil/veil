# ADR: Unified Runtime Agent with ACP Tunnel

**Status:** Proposed  
**Date:** 2026-08-03  
**Relates to:** `docs/AGENT.md`, `docs/ACP_SPIKE.md`, `runtime/bootstrap/src/agent_chat.rs`

---

## Context

The VEIL runtime runs on ECS in AWS and serves the platform dashboard (project management, SDLC, deploy, registry). A separate `veil serve` process provides the IDE experience (code editing, check, gen, agent).

Currently these are conceptually separate agents with different capabilities:
- **Platform agent** — navigates dashboard, manages change requests, triggers deploys
- **IDE agent** — edits `.veil` source, runs check/gen/smoke, manages code structure

Users interact with a dashboard that embeds an agent chat pane. They expect to talk to **one agent** that can do everything — not two agents they must mentally route between.

Additionally, we want to support multiple LLM backends:
1. **Managed** — Bedrock/OpenAI running alongside veil-runtime on ECS (default)
2. **Bring Your Own Key** — User provides an API key, runtime calls the LLM directly
3. **Bring Your Own Agent (ACP)** — User connects their own local ACP-compatible agent (e.g., Kiro CLI with an existing subscription) as the LLM reasoning engine

---

## Decision

### 1. Single Unified Agent

One agent session per user. All tools — platform, SDLC, and IDE — are registered in a single tool set. The LLM decides which tools to invoke based on user intent.

```
User: "Show me open change requests"     → platform tool
User: "Add a guard to CreateCustomer"    → IDE tool (proxied to veil-serve)
User: "Deploy relay to staging"          → deploy tool
User: "Open the relay project in the IDE" → navigation tool (client-side)
```

The runtime is the **orchestrator**. It owns the conversation, manages tool execution, and streams results to the browser. The LLM is a pluggable reasoning backend.

### 2. LLM Provider Modes

| Mode | Config | How it works |
|------|--------|--------------|
| **Managed (Rig + Bedrock)** | `VEIL_AGENT_PROVIDER=bedrock` | Runtime uses [Rig](https://rig.rs) with Bedrock provider. Same `rig-core` agent framework as `veil serve` IDE agent. Tools registered as `rig_core::tool::Tool` impls. Zero user config. |
| **BYOK** | `VEIL_AGENT_PROVIDER=openai` + user-supplied key | Runtime uses Rig with OpenAI/Anthropic provider, keyed per-user. |
| **ACP Tunnel** | User connects local ACP agent | Runtime delegates reasoning to user's machine. |

All three modes use the same Rig tool definitions. The Managed and BYOK modes execute Rig agent turns directly on ECS. The ACP Tunnel mode serializes the same tool schemas into the ACP protocol and delegates reasoning externally — tool *execution* still happens via the same Rig tool impls on ECS.

### 3. ACP Tunnel Architecture

The ACP tunnel allows a user to connect their own local agent (Kiro, custom agent, local Ollama wrapper — anything that speaks ACP) as the LLM backend for their session.

#### Connection Flow

```
┌─────────────────────────────┐       ┌──────────────────────────────┐
│  User's Machine             │       │  ECS (veil-runtime)          │
│                             │       │                              │
│  ┌───────────────────────┐  │       │  ┌────────────────────────┐  │
│  │ Browser (dashboard)   │──┼──wss──┼──│ Agent WebSocket        │  │
│  └───────────────────────┘  │       │  │ /api/agent/chat        │  │
│                             │       │  └───────────┬────────────┘  │
│  ┌───────────────────────┐  │       │              │               │
│  │ Local ACP Agent       │──┼──wss──┼──│ ACP Tunnel Endpoint    │  │
│  │ (kiro-cli acp)        │  │       │  │ /api/agent/acp         │  │
│  └───────────────────────┘  │       │  └───────────┬────────────┘  │
│                             │       │              │               │
│                             │       │  ┌───────────▼────────────┐  │
│                             │       │  │ Unified Agent           │  │
│                             │       │  │ • Conversation state    │  │
│                             │       │  │ • Tool registry         │  │
│                             │       │  │ • Tool execution        │  │
│                             │       │  └───────────┬────────────┘  │
│                             │       │              │               │
│                             │       │  ┌───────────▼────────────┐  │
│                             │       │  │ Tool Backends           │  │
│                             │       │  │ • DDB / S3 (platform)  │  │
│                             │       │  │ • veil-serve (IDE)      │  │
│                             │       │  │ • Deploy (AWS APIs)     │  │
│                             │       │  └────────────────────────┘  │
│                             │       │                              │
└─────────────────────────────┘       └──────────────────────────────┘
```

#### Key Principles

1. **Outbound-only connections** — The local ACP agent connects *outbound* to the runtime. No inbound ports, no firewall issues, no NAT traversal.

2. **LLM-only delegation** — The local agent is purely the reasoning engine. It receives messages + tool definitions, returns tool_use decisions. All tool execution happens on ECS.

3. **Token streaming** — The WebSocket connection supports streaming tokens from the ACP agent back through the runtime to the browser in real-time.

4. **Idle between turns** — The connection persists for the session but is idle between user messages. No polling, no keepalive traffic beyond standard WebSocket pings.

5. **Graceful fallback** — If the ACP tunnel disconnects mid-session, the runtime can fall back to managed Bedrock or surface an error. Session history is preserved.

#### Protocol (ACP-over-WebSocket)

**Runtime → ACP Agent (turn request):**
```json
{
  "type": "turn_request",
  "turn_id": "turn_abc123",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "Add a guard to CreateCustomer" },
    { "role": "assistant", "content": "...", "tool_use": [...] },
    { "role": "tool_result", "content": "..." }
  ],
  "tools": [
    { "name": "navigate_to", "description": "...", "parameters": {...} },
    { "name": "edit_source", "description": "...", "parameters": {...} },
    { "name": "create_change_request", "description": "...", "parameters": {...} }
  ]
}
```

**ACP Agent → Runtime (streaming response):**
```json
{ "type": "content_delta", "turn_id": "turn_abc123", "delta": "I'll add a" }
{ "type": "content_delta", "turn_id": "turn_abc123", "delta": " validation guard..." }
{ "type": "tool_use", "turn_id": "turn_abc123", "tool_call": {
    "id": "call_xyz",
    "name": "edit_source",
    "arguments": { "project": "relay", "span_start": 1420, "body": ["guard customer.email.valid?()"] }
  }
}
{ "type": "turn_complete", "turn_id": "turn_abc123" }
```

**Runtime → ACP Agent (tool result, for multi-turn):**
```json
{
  "type": "tool_result",
  "turn_id": "turn_abc123",
  "call_id": "call_xyz",
  "output": { "ok": true, "diagnostics": [] }
}
```

#### Session Binding

- The ACP tunnel connection is bound to a **user session** (authenticated via token in the WebSocket handshake)
- One ACP tunnel per user session (multiple browser tabs share the same agent state)
- If both managed LLM and ACP tunnel are available, user preference (stored in config) determines which is used
- Dashboard UI shows connection status: "Agent: Bedrock" vs "Agent: Connected (Kiro)"

#### Local Agent Setup (User Experience)

```bash
# One-time: login to the runtime
veil login https://runtime.example.com

# Connect local Kiro as the agent backend
veil agent connect
# → Spawns: kiro-cli acp --trust-all-tools
# → Connects outbound to wss://runtime.example.com/api/agent/acp?token=...
# → Dashboard shows "Agent: Connected (Kiro)"

# Or with a custom agent
veil agent connect --command "my-agent --acp-mode"
```

The `veil agent connect` command:
1. Reads the runtime URL and auth token from `~/.veil/config.json`
2. Spawns the local ACP agent process
3. Opens a WebSocket to the runtime's `/api/agent/acp` endpoint
4. Bridges ACP protocol frames between the local process and the WebSocket
5. Keeps running until Ctrl+C

---

## Tool Execution Model

All tools execute on ECS regardless of LLM provider mode:

| Tool Category | Execution Location | Backend |
|---|---|---|
| Navigation (`navigate_to`, `open_ide`) | ECS → push to browser via WS | Client-side routing |
| Platform (`create_change_request`, `list_repos`) | ECS | DDB / S3 |
| IDE (`edit_source`, `veil_check`, `veil_gen`) | ECS → veil-serve instance | Per-project veil-serve on ECS |
| Deploy (`plan_provision`, `deploy`) | ECS | AWS APIs (Lambda, API Gateway, etc.) |
| Observability (`dev_logs`, `smoke_status`) | ECS → veil-serve | Per-project veil-serve |

IDE tools proxy to a `veil-serve` instance running on ECS (one per active project). Source files live in S3/DDB (via the `SourceResolver`), not on a local filesystem. The user never needs local file access for the unified agent to edit code.

---

## Consequences

### Positive
- Users get a single conversational interface for all VEIL operations
- "Bring your own agent" requires zero backend infrastructure from the user — just a CLI command
- Streaming works naturally (WebSocket is bidirectional)
- No firewall/NAT issues (all connections are outbound from user's machine)
- Vendor-agnostic on the LLM side — Bedrock, OpenAI, local Ollama via Kiro, anything ACP-compatible
- Session state and tool execution are centralized (reliable, auditable)

### Negative
- Latency: ACP tunnel adds a network hop (user machine → ECS → user machine for reasoning). For users far from the DC, token streaming may feel slower than a local agent.
- Complexity: Three provider modes to maintain and test
- Dependency: ACP tunnel mode depends on the user's machine staying connected. Laptop sleep / network blips interrupt the session.

### Mitigations
- Managed Bedrock is the default — ACP tunnel is opt-in for power users
- Reconnection logic with session preservation (conversation history is server-side)
- Consider a "hybrid" future mode: managed LLM for platform tools, ACP for code reasoning (but this adds routing complexity — defer unless needed)

---

## Implementation Phases

### Phase 1: Unified Agent (Managed LLM)
- Wire `ws_agent_chat` into the runtime router
- Register all tool categories in a single tool set
- Implement Bedrock provider for reasoning
- IDE tools proxy to ECS-hosted veil-serve instances

### Phase 2: BYOK
- User configures API key via dashboard Config page
- Runtime uses user's key for OpenAI/Anthropic calls
- Key stored encrypted in DDB per-user

### Phase 3: ACP Tunnel
- `/api/agent/acp` WebSocket endpoint on runtime
- `veil agent connect` CLI command
- Protocol bridge (ACP frames ↔ WebSocket messages)
- Session binding and fallback logic
- Dashboard connection status indicator

---

## Open Questions

| Question | Proposed Answer |
|----------|----------------|
| Multi-turn tool use: who manages the loop? | Runtime always. It sends tool results back to the ACP agent for the next reasoning step. |
| Token limit / context window management? | Runtime truncates/summarizes history before sending to ACP agent. Agent doesn't need to manage this. |
| Can multiple ACP agents connect (team)? | Defer. V1 is one agent per user session. |
| What if ACP agent misbehaves (infinite tool calls)? | Runtime enforces a max tool calls per turn (e.g., 25). |
| Auth for ACP tunnel? | Bearer token in WebSocket URL query param, same as dashboard auth. |
