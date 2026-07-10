# In-IDE agent & model providers

## Built-in agent (AGT-001 / AGT-006)

Toolbar **Agent** panel → `POST /api/agent/turn` with `{ "prompt": "…" }`.

Heuristic tools (always available):

| Prompt | Behavior |
|--------|----------|
| `check` | Full check pipeline |
| `outline` | Construct outline |
| `rename Old to New` | `EditOp::Rename` + write + check |

Other prompts call the configured **ModelProvider**, then show tool guidance.

## ModelProvider (AGT-003)

Env configuration (no engine/domain changes to add adapters):

| Variable | Meaning |
|----------|---------|
| `VEIL_MODEL_PROVIDER` | `echo` (default), `openai`, `bedrock` |
| `VEIL_MODEL_NAME` | Model id |
| `VEIL_MODEL_API_KEY` / `OPENAI_API_KEY` | Credentials |
| `VEIL_MODEL_BASE_URL` | OpenAI-compatible base (default `https://api.openai.com/v1`) |
| `VEIL_MODEL_REGION` / `AWS_REGION` | Bedrock region |

- **echo** — offline; returns guidance text  
- **openai** — OpenAI-compatible port (HTTP body prepared; wire reqwest next)  
- **bedrock** — port registered; honest error until AWS SDK linked  

List config: `GET /api/models`.

## Source port (AGT-004 / AGT-005)

Agent tools use `SourceProvider` (`FilesystemProvider` for `veil serve`).
Writes go through the same path as the IDE.

## Live sync (AGT-002)

`GET /api/events` — SSE revision heartbeat. Agent turns with `source_changed`
trigger client `fetchIr()`.

## Safety (AGT-009)

| Mode | Env | Behavior |
|------|-----|----------|
| Auto-apply (default local) | unset | Renames write immediately; tool log in panel |
| Confirm writes | `VEIL_AGENT_CONFIRM_WRITES=1` | Rename requires `confirm rename A to B` |

All tool calls are returned in the turn response for human review.
