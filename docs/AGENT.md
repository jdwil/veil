# In-IDE agent & Rig SDK

Agentic work in VEIL is built on **[Rig](https://rig.rs)** (`rig-core`) and
optional **ACP (Kiro)**. Platform wiring (dock, Aether, dev-loop, chat memory)
is summarized for implementers in **[IDE_AGENT_PLATFORM.md](./IDE_AGENT_PLATFORM.md)**.

## Built-in agent (AGT-001 / AGT-006)

IDE **Agent** dock uses **AetherUI** over WebSocket (`/api/chat` or
`/api/p/{project}/chat`). Legacy: `POST /api/agent/turn` / `…/turn/stream` (SSE).

Viewer: `veil-viewer/src/lib/AetherAgentPanel.svelte` + `agentSession.ts`
(in-memory transcript — **lost on full page reload**; survives Agent↔Split only).
Each turn the FE sends full `messages[]`, but the ACP path uses only the **last
user** message; multi-turn continuity while serve stays up is mostly Kiro’s
process-wide ACP session. Details: [IDE_AGENT_PLATFORM.md](./IDE_AGENT_PLATFORM.md).

### Mind Palace (optional knowledge tools)

See [MIND_PALACE.md](./MIND_PALACE.md). Set `MIND_PALACE=1` + AWS profile/resources
to attach `wiki_*` Rig tools for long-term VEIL platform knowledge.

### Backends

| `VEIL_MODEL_PROVIDER` | Behavior |
|----------------------|----------|
| `echo` (default) | Offline heuristic: `check` · `outline` · `rename A to B` |
| `openai` | **Rig** OpenAI (or compatible) agent **with tools** |
| `ollama` | **Rig** Ollama agent **with tools** (local default) |
| `bedrock` | Use OpenAI-compatible Bedrock gateway via `openai` + `VEIL_MODEL_BASE_URL` |

### Rig tools (typed `rig_core::tool::Tool`)

| Tool | Purpose | IDE equivalent |
|------|---------|----------------|
| `veil_check` | Dual-loop check pipeline | Check / diagnostics |
| `veil_outline` | Compact IR topology | Graph outline |
| `read_source` | Active file text (truncated) | Source dock |
| `rename_construct` | Structured rename | Property / rename |
| `list_files` | Packages/layers in project | File picker |
| `select_file` | Switch active file | File picker |
| `create_file` | New `.veil` / `.layer` | **+** in breadcrumb |
| `write_source` | Replace active file body (smoke-gated) | `POST /api/source` |
| `dev_status` | Dual-loop targets / ports / errors | Dev toolbar |
| `dev_logs` | Gen/check/smoke log ring | Dev logs |
| `smoke_status` | Last smoke/check excerpt | — |
| `read_generated` | Read under `[[targets]].output` | Generated tree |
| `list_routes` | JSON routes from `veil_bin` | — |
| `http_request` | Probe `127.0.0.1:dev_port` only | curl |
| `dev_restart` | Restart owned dual-loop process | Dev restart |

**Parity rule:** anything the dual-loop front-end can do should have a matching
agent tool (or structured edit). New IDE actions → add a Rig tool in
`rig_tools.rs` and register it in `model::prompt_with_tools`.

Tools mutate an in-memory workspace; each successful edit is **flushed live** to
`SourceProvider` (disk) mid-turn when possible, and `GET /api/events` streams
revision SSE so the viewer badge updates without waiting for the turn HTTP
response. `rename_construct` is **format-preserving** (identifier token patch —
does not re-serialize the whole package). `create_file` writes under the project
root and selects the new file (same as the UI).

### Closed loop: edit → smoke → observe → verify (AGT-020–028 · ACS-002)

```text
write_source → gen + cargo check (smoke)
    ├─ fail → WRITE REJECTED, previous file + gen restored → dev_logs → fix
    └─ ok   → list_routes / read_generated → dev_restart → http_request
```

**Mandatory after WRITE REJECTED:** call `dev_logs` / `smoke_status` before rewriting
the whole file again.

- **Smoke** is on by default (`VEIL_AGENT_SMOKE=0` disables — escape hatch only).
- **Auto-restart** after successful smoke when backend is owned Running:
  default on (`VEIL_AGENT_AUTO_RESTART=0` to disable) — ACS-004.
- **Bang / Opt / Res:** [BANG_CONTRACT.md](./BANG_CONTRACT.md) — never `.unwrap()` after `find!`.
- **Routes:** prefer `@route("GET /api/…")`; name-derived List/Get/Create = fallback only.
  See [HARNESS.md](./HARNESS.md).
- Stories: [160](../stories/160-agent-runtime-observability.md), [170](../stories/170-agent-complexity-shoreup.md).

### Structured check diagnostics (ACS-008)

`veil_check` (MCP / Rig / ACP) returns a one-line summary **plus** JSON:

```json
{
  "ok": false,
  "error_count": 1,
  "warning_count": 0,
  "diagnostics": [
    {
      "code": "type_mismatch",
      "severity": "error",
      "message": "…",
      "span": { "start": 120, "end": 145 },
      "hint": "…",
      "node_name": "CreateItem"
    }
  ]
}
```

| Field | Use |
|-------|-----|
| `code` | Stable rule id (`type_mismatch`, `parse_error`, `must_have`, …) |
| `severity` | `error` \| `warning` |
| `message` | Human text |
| `span` | Byte offsets into the active source when known — **edit that region** |
| `hint` | Optional remediation |

**CLI:** `veil check path.veil --json` includes the same structured `diagnostics` array.

**Do:** fix the reported span / code. **Don't:** rewrite the whole package for one type error.

### Env

| Variable | Meaning |
|----------|---------|
| `VEIL_MODEL_PROVIDER` | `echo` \| `openai` \| `ollama` \| **`acp`** / `kiro` |
| `VEIL_MODEL_NAME` | Model id (defaults: `gpt-4o-mini`, `llama3.2`; **make serve** defaults to `qwen3.5:9b`) |
| `VEIL_AGENT_SMOKE` | `0`/`false`/`off` disables post-write gen+check (default on) |
| `VEIL_AGENT_AUTO_RESTART` | `0`/`false`/`off` disables restart after successful smoke (default on) |
| `VEIL_AGENT_HTTP_PORTS` | Extra ports allowed for `http_request` (comma-separated) |

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

### Kiro via ACP (recommended for strong models)

VEIL acts as an **ACP client** and spawns Kiro CLI:

```bash
# once
kiro-cli login   # Builder ID / Pro developer deal

# serve with ACP backend
make serve VEIL_MODEL_PROVIDER=acp

# optional overrides
export VEIL_ACP_COMMAND=kiro-cli
export VEIL_ACP_ARGS="acp --trust-all-tools"
export VEIL_ACP_CWD=$PWD          # workspace root for Kiro
export VEIL_ACP_AGENT=personal    # optional agent profile
export VEIL_ACP_MODEL=…          # if your Kiro plan exposes model ids
export VEIL_ACP_TIMEOUT_SECS=300
```

Kiro edits files on disk; after each ACP turn the server **reloads from disk**
and the viewer refreshes via SSE (`GET /api/events`).

### Agent context (Tier 0 + Tier 1 — not vector RAG)

Each turn builds a **deterministic teaching pack** for the **active file**:

| Tier | Content |
|------|---------|
| 0 | Host rules + tools |
| 1 | Layer `prompt` sections (in `use` order), construct vocabulary, diagnostics, IR outline |

Budget: `VEIL_AGENT_PREAMBLE_MAX_TOKENS` (default **12000** ≈ 48k chars).

| Env | Meaning |
|-----|---------|
| `VEIL_AGENT_PREAMBLE_MAX_TOKENS` | Max approx tokens for preamble (`0` = unlimited) |
| `VEIL_AGENT_ALLOW_TRUNCATED=1` | **Force** model turn even if curriculum was cut (not recommended) |

**If truncated:** response sets `context_truncated: true`, fills `context_warning`, and
**refuses the Rig model turn** by default (`ok: false`, backend `rig-*-refused`).
The UI shows a red banner. Switch to a larger-context model, OpenAI flagship, or ACP
— do not trust a 9B with a cut layer curriculum.

| Variable | Meaning |
|----------|---------|
| `VEIL_MODEL_API_KEY` / `OPENAI_API_KEY` | OpenAI credentials |
| `VEIL_MODEL_BASE_URL` / `OPENAI_BASE_URL` | Compatible base URL |
| `VEIL_AGENT_CONFIRM_WRITES=1` | Require `confirmed` on renames |
| `VEIL_AGENT_ALLOWLIST` | Comma-separated write paths/prefixes/globs (default: loaded `.veil` files) |
| `VEIL_AGENT_PLAN_ONLY=1` | Propose edits; never persist (`plan` field on response) |
| `VEIL_CONTEXT_MAX_TOKENS` | Default budget for `GET /api/context` (also `?max_tokens=`) |
| `VEIL_AUTH_TOKEN` | When set, require `Authorization: Bearer <token>` (or raw token) on all API routes (AGT-016) |
| `VEIL_BIN` | Path to `veil` for dev-loop `veil gen` (default: running serve binary) |

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
