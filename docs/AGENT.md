# In-IDE agent & Rig SDK

Agentic work in VEIL is built on **[Rig](https://rig.rs)** (`rig-core`).

## Built-in agent (AGT-001 / AGT-006)

Toolbar **Agent** → `POST /api/agent/turn` with `{ "prompt": "…" }`.

### Backends

| `VEIL_MODEL_PROVIDER` | Behavior |
|----------------------|----------|
| `echo` (default) | Offline heuristic: `check` · `outline` · `rename A to B` |
| `openai` | **Rig** OpenAI (or compatible) agent **with tools** |
| `ollama` | **Rig** Ollama agent **with tools** (local default) |
| `bedrock` | Use OpenAI-compatible Bedrock gateway via `openai` + `VEIL_MODEL_BASE_URL` |

### Rig tools (typed `rig_core::tool::Tool`)

| Tool | Purpose |
|------|---------|
| `veil_check` | Dual-loop check pipeline |
| `veil_outline` | Compact IR topology |
| `read_source` | Active `.veil` text (truncated) |
| `rename_construct` | Structured `EditOp::Rename` |

Tools mutate an in-memory workspace; the host persists via `SourceProvider` when
`source_changed` is true.

### Env

| Variable | Meaning |
|----------|---------|
| `VEIL_MODEL_PROVIDER` | `echo` \| `openai` \| `ollama` |
| `VEIL_MODEL_NAME` | Model id (defaults: `gpt-4o-mini`, `llama3.2`; **make serve** defaults to `qwen3.5:9b`) |

### Local make serve (Ollama)

```bash
# defaults: VEIL_MODEL_PROVIDER=ollama  VEIL_MODEL_NAME=qwen3.5:9b
make serve-examples

# offline heuristic instead
make serve VEIL_MODEL_PROVIDER=echo

# different Ollama model
make serve VEIL_MODEL_NAME=llama3.2
```

Requires `ollama serve` and the model pulled (`ollama pull qwen3.5:9b`).
| `VEIL_MODEL_API_KEY` / `OPENAI_API_KEY` | OpenAI credentials |
| `VEIL_MODEL_BASE_URL` / `OPENAI_BASE_URL` | Compatible base URL |
| `VEIL_AGENT_CONFIRM_WRITES=1` | Require `confirmed` on renames |
| `VEIL_AGENT_ALLOWLIST` | Comma-separated write paths/prefixes/globs (default: loaded `.veil` files) |
| `VEIL_AGENT_PLAN_ONLY=1` | Propose edits; never persist (`plan` field on response) |
| `VEIL_CONTEXT_MAX_TOKENS` | Default budget for `GET /api/context` (also `?max_tokens=`) |
| `VEIL_AUTH_TOKEN` | When set, require `Authorization: Bearer <token>` (or raw token) on all API routes (AGT-016) |

`GET /api/models` — provider + config (+ `"rig": true`).

## Source port (AGT-004 / AGT-005)

Agent tools use `SourceProvider` (`FilesystemProvider` for `veil serve`).

## Remote sessions (AGT-010)

Set `VEIL_REMOTE_URL` so local `veil serve` uses `RemoteHttpProvider` — same
agent tools and edit path, package source on the remote host. See
`docs/SERVER.md` (Remote SourceStore).

## Context pack (AGT-011 / PAR-009)

- Server: `GET /api/context`
- CLI: `veil prompt path/to.veil [--max-tokens N]` — layer prompts + construct
  outline + vocabulary for agent assembly

## Live sync (AGT-002)

`GET /api/events` — SSE revision heartbeat. Agent turns with `source_changed`
trigger client `fetchIr()`.

## Safety (AGT-009)

| Mode | Env | Behavior |
|------|-----|----------|
| Auto-apply (default) | unset | Renames apply when tools run |
| Confirm writes | `VEIL_AGENT_CONFIRM_WRITES=1` | Rename needs `confirmed=true` / `confirm rename …` |

Tool calls are returned in the turn response for review. Use **Review changes**
(UX-021) for structural diff.

## VEIL `rig` layer

`layers/rig.layer` defines `tool` / `agent` / `tool_set` constructs for
authoring agent apps *in VEIL*. The IDE agent itself is host-side Rig Rust.

## MCP / ACP

- Tool discovery: `GET /api/agent/tools` (veil-tools-v1 JSON schemas)
- ACP research & go/no-go: `docs/ACP_SPIKE.md` (Rig-first; ACP host later)
