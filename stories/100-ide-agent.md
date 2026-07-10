# IDE Coding Agent Stories

Mission: agents are the **primary authors**; the IDE is where humans prompt,
review topology/critical bodies, and approve. This file covers **in-IDE agent
interaction** — flexible models, pluggable agent platforms, and storage-agnostic
edit tools.

**Depends on (strongly recommended before / with implementation):**

- Edit + serialize integrity (**SER-***), especially SER-001/002/005  
- Honest check (**CHK-***) so the agent has a machine loop  
- Project-root serve as daily driver (**RT-020**, **UX-010/011**)  
- Source storage ports for local/cloud runtime (**RT-010/011**, see AGT-004)

**Priority band:** P1 for thin vertical slice; P2 for full provider/ACP/MCP
matrix. Do **not** block dual-loop P0s on full agent versatility.

---

## Architecture (target)

```
┌─────────────────────────────────────────────────────────────┐
│  veil-viewer (IDE)                                          │
│  • Prompt / thread UI                                       │
│  • Live IR refresh on SourceStore change events             │
│  • Review: topology + critical bodies + optional preview    │
└─────────────┬───────────────────────────────▲───────────────┘
              │ prompt / session              │ source-changed
              ▼                               │ events / poll
┌─────────────────────────────────────────────────────────────┐
│  Agent session host (veil-server or platform daemon)        │
│  • Session lifecycle, streaming tokens, tool call UX hooks  │
│  • ModelProvider port  → Bedrock / OpenAI / … adapters      │
│  • AgentPlatform port  → built-in agent | ACP external      │
│  • Tool surface        → native tools and/or MCP server     │
└─────────────┬───────────────────────────────────────────────┘
              │ tool calls (read/edit/check/…)
              ▼
┌─────────────────────────────────────────────────────────────┐
│  VEIL tool ports (domain)                                   │
│  • SourceStore   — list/read/write/watch package files      │
│  • EditApply     — structured EditOp and/or text patch      │
│  • Check         — veil check / diagnostics                 │
│  • Context       — palette, layer prompts, IR summary       │
│  Adapters bind these to the active environment (below)      │
└─────────────┬───────────────────────────────────────────────┘
              │
     ┌────────┼────────────────────────┐
     ▼        ▼                        ▼
 Local serve   Local runtime            Deployed runtime (AWS…)
 (project FS)  (FS workspace and/or     (S3/object + metadata
               sqlite metadata)          store / DB)
```

### ACP vs MCP (research note for implementers)

| Protocol | Role |
|----------|------|
| **ACP** ([agentclientprotocol.com](https://agentclientprotocol.com)) | Editor ↔ **coding agent** session (prompt, stream, permissions) — “LSP for agents.” External agents (Claude Code, Gemini CLI, …) often **own** their runtime, models, and tools. |
| **MCP** | Agent ↔ **tools/resources** (filesystem, DB, VEIL APIs). Complementary to ACP. |

**Zed-shaped pattern we should follow:**

1. IDE hosts threads and talks **ACP** to external agents when the user picks one.  
2. VEIL exposes edit/check/context capabilities as an **MCP server** (and as
   native tools for a built-in agent).  
3. Prefer **forwarding MCP tools into ACP sessions** when the external agent
   supports it (Zed documents this path; support varies by agent — see AGT-007).  
4. If an ACP agent cannot consume our MCP tools, fall back to: (a) agent’s own
   filesystem tools against a synced workspace, or (b) built-in agent that
   calls our ports directly.

We do **not** assume every ACP agent accepts host-injected tools. Design for
degradation.

### Source location matrix (must support all)

| Environment | Where `.veil` / `.layer` / `.stub` live | SourceStore adapter |
|-------------|----------------------------------------|---------------------|
| **Project-root `veil serve`** | User’s git working tree on **disk** | `LocalFsStore` (workspace root) |
| **Local veil-runtime** | **Hybrid (decision):** package *workspace files* on **disk** under a runtime data root; **metadata** (repos, branches, artifacts, registry) in **sqlite**. Optional later: blob-in-sqlite for tiny snippets only — not default for full trees. | `LocalFsStore` + `SqliteMeta` (metadata ≠ source blobs) |
| **Deployed runtime (AWS)** | Content-addressed **object storage** (S3) + **metadata DB** (Dynamo/etc.) | `ObjectStore` + `MetadataStore` adapters |
| **Tests** | Temp dir or in-memory fake | `MemoryStore` |

**Answer to “local runtime: disk or sqlite?”:**  
**Source files on disk; sqlite for platform metadata.** That keeps editors,
`git`, and agents honest, matches project-root mental model, and avoids stuffing
trees into a row store. Object storage remains the cloud analog of “disk.”

---

## AGT-001: In-IDE agent panel (MVP vertical slice)

**Status:** Done · **Priority:** P1  
**As a** human in the VEIL IDE  
**I want** to send a natural-language prompt that requests code changes  
**So that** the agent edits my package and the IDE updates without a manual reload

**Acceptance criteria:**

- Agent panel/thread UI in `veil-viewer` (prompt box, streaming response, basic
  history for the session)
- Backend session endpoint(s) on the active server (`veil serve` first)
- Agent may call tools to read/edit sources and run check
- After successful edits, IDE **automatically** refreshes IR, diagnostics, and
  VEIL body/source views (push event preferred; short poll acceptable for MVP)
- User can cancel an in-flight turn
- Errors (model, tool, validation) surface in the thread without crashing serve
- MVP may use a **single** built-in agent + one model provider adapter

**Mission impact:** Closes the human↔agent loop inside the product surface.

**Depends:** SER-001/002 (safe writes), UX-010 (editable pkg), CHK-001/002

**Done notes:** `AgentPanel.svelte` + `POST /api/agent/turn`; built-in heuristic
agent tools: check, outline, rename; cancel via AbortController; refresh on
`source_changed`. Real model adapters remain AGT-003.

---

## AGT-002: Live IDE sync on backend source changes

**Status:** Done · **Priority:** P1  
**As a** user watching the agent work  
**I want** the graph and panels to track source mutations from any writer  
**So that** agent tools, manual edits, and multi-tab/server paths stay consistent

**Acceptance criteria:**

- `SourceStore` emits change notifications (paths + revision/etag)
- Server pushes to IDE (WebSocket/SSE) or IDE watches with low latency
- Refresh coalesces rapid tool calls (debounce) to avoid UI thrash
- Conflict policy documented: if user edited same file mid-turn → warn / merge
  strategy (MVP: block agent write or prompt user)
- Works for LocalFs adapter first; same events for remote adapters later

**Done notes:** `GET /api/events` SSE revision heartbeat; agent turn sets
`source_changed` → client `fetchIr()`. Full shared event bus deferred.

---

## AGT-003: ModelProvider port + flexible adapters (Zed-like)

**Status:** Done · **Priority:** P1  
**As a** deployer / power user  
**I want** pluggable model backends (e.g. Amazon Bedrock, OpenAI-compatible, …)  
**So that** we are not locked to one vendor API

**Acceptance criteria:**

- `ModelProvider` port: list models, chat/completions stream, optional tools
  binding for built-in agent
- Config surface (env + config file): provider kind, region, credentials,
  default model, endpoint overrides
- **Bedrock adapter** as first-class non-OpenAI proof of flexibility
- At least one second adapter (OpenAI-compatible or Anthropic) to prove the port
- No provider-specific types leak into viewer or tool ports
- Document how to add a provider adapter without engine/domain changes

**Mission impact:** “Extremely flexible model support” — ports/adapters, not
hardcoded clients in the UI.

**Done notes:** `ModelProvider` + `EchoProvider`, `OpenAiCompatibleProvider`,
`BedrockProvider` (port registered; AWS SDK follow-up). Env:
`VEIL_MODEL_PROVIDER`, `VEIL_MODEL_NAME`, `VEIL_MODEL_API_KEY`,
`VEIL_MODEL_BASE_URL`, `VEIL_MODEL_REGION`. `GET /api/models`.

---

## AGT-004: SourceStore port + adapters (all environments)

**Status:** Done · **Priority:** P1  
**As the** edit toolchain  
**I want** one source abstraction for all deploy modes  
**So that** agent tools do not care whether files are on disk, in S3, or staged locally

**Acceptance criteria:**

- Port methods (MVP): `list`, `read`, `write`, `delete?`, `watch`/`subscribe`
- Adapters:
  - **LocalFs** — project root / runtime data root  
  - **Object+Meta** — S3 (or generic object) + metadata store for deployed  
  - **Memory** — tests  
- Local runtime default: **files on disk**, metadata in **sqlite** (document;
  do not put full source trees in sqlite by default)
- Write path runs through validation hooks optional flag: write → check
- Adapter selected by server mode / config, not by tool name

**Ties to:** RT-010/011 (storage), SER (serialization of VEIL text)

**Done notes:** `SourceProvider` port + `FilesystemProvider` (LocalFs) is the
agent tool surface. Object+Meta / Memory adapters deferred with RT-010.

---

## AGT-005: Agent edit tools as ports (not filesystem-only hacks)

**Status:** Done · **Priority:** P1  
**As an** agent  
**I want** first-class tools for VEIL editing  
**So that** changes go through the same integrity path as the IDE

**Acceptance criteria:**

Tool ports (names illustrative):

| Tool | Behavior |
|------|----------|
| `list_sources` | List package files via SourceStore |
| `read_source` | Read file text |
| `write_source` | Write full file (with optional check) |
| `apply_edit` | Structured `EditOp` (preferred when possible) |
| `apply_patch` | Unified diff / search-replace for agent convenience |
| `veil_check` | Run check pipeline; return diagnostics |
| `get_ir_summary` / `get_palette` | Topology + layer vocabulary for the agent |
| `get_context` | **LAY-010** — outline + presentation views + optional host projection (`GET /api/context?host_id=&view_id=`); prefer speaking in view terms |
| `get_layer_prompts` | Concatenated layer `prompt` sections (PAR-009); also included in context pack |

- Tools call ports; adapters handle storage
- Writes that fail check can be rejected or returned as diagnostics (configurable)
- Audit log of tool calls in the session (for human review)
- Built-in agent uses these natively

**Mission impact:** Agents edit VEIL structure safely; storage stays swappable.

**Done notes:** Built-in tools via `SourceProvider`: `read_source`,
`apply_edit` (Rename EditOp), `run_check`, `get_context` outline. Tool calls
logged in turn response. Full tool matrix / apply_patch later.

---

## AGT-006: Built-in agent (host-owned loop)

**Status:** Done · **Priority:** P1  
**As a** user without an external ACP agent  
**I want** a built-in coding agent in the IDE  
**So that** VEIL works out of the box with only model credentials

**Acceptance criteria:**

- Host owns: prompt assembly (layer prompts + open file + diagnostics + user text),
  tool loop, max steps, cancellation
- Uses ModelProvider + AGT-005 tools
- Streams assistant text and tool-call status to the panel
- Respects a simple permission mode: auto-apply vs confirm-each-write (MVP:
  auto-apply in local dev with clear activity log)
- Configurable system/instructions path (project `AGENTS.md` / layer prompts)

**Done notes:** `veil_server::agent::run_turn` host-owned heuristic loop;
tool status in panel; auto-apply rename in local serve. ModelProvider
pluggability = AGT-003.

---

## AGT-007: ACP client — pluggable external agent platforms

**Status:** Open · **Priority:** P2  
**As a** user  
**I want** to attach external ACP-compatible agents (Zed-style)  
**So that** I can use Claude Code / Gemini / in-house agents from the VEIL IDE

**Acceptance criteria:**

- VEIL IDE (or server) acts as **ACP client/host** for sessions
- User can configure agent command/endpoint (local stdio agent and/or remote)
- Session: prompt, stream, cancel; display agent messages in the same panel UX
- Capability negotiation documented; graceful error if agent missing
- Spike report: which popular agents we tested and what they support

**Research gate (must answer in spike before full build):**

1. Can we inject/forward **VEIL MCP tools** into the ACP session?  
2. If not, what is the best degradation (workspace FS mount, bridge process)?  
3. Do we need a thin **ACP↔MCP bridge** process?

Deliverable of the spike: short `docs/ACP_SPIKE.md` with go/no-go and chosen
pattern. Implementation stories may split after the spike.

---

## AGT-008: VEIL MCP server for tools

**Status:** Open · **Priority:** P2  
**As an** external agent (ACP or any MCP client)  
**I want** VEIL edit/check tools over MCP  
**So that** agentic platforms can modify packages without bespoke integrations

**Acceptance criteria:**

- MCP server exposes AGT-005 tools (stable names + JSON schemas)
- Auth/binding to a workspace / SourceStore session (local path or remote token)
- Works with at least one MCP client in tests (and documented with Claude/Cursor/etc.)
- Same port implementations as built-in agent (no duplicate edit logic)
- If ACP tool injection works (AGT-007), wire MCP forwarding; if not, document
  “run agent with VEIL MCP configured natively”

**Mission impact:** Decouples “our tools” from “which agent product you like.”

---

## AGT-009: Permissions, safety, and review hooks

**Status:** Open · **Priority:** P2  
**As a** human reviewer  
**I want** control over what the agent may change  
**So that** dual-loop trust holds when the agent is fast

**Acceptance criteria:**

- Modes: auto-apply local / confirm writes / plan-only (propose diff, no write)
- Optional path allowlist (e.g. only `**/*.veil`, never `.env`)
- After a turn: link to **structural diff** (UX-021) of agent changes
- Escape-hatch and check failures visible in-thread (CHK-006)
- No silent write on check error when `strict` is on

---

## AGT-010: Remote / multi-user agent sessions (platform)

**Status:** Open · **Priority:** P3  
**As a** user of deployed veil-runtime  
**I want** agent sessions against remote SourceStore  
**So that** cloud-hosted packages are editable via the same IDE UX

**Acceptance criteria:**

- Authn to runtime; SourceStore uses object+meta adapters
- IDE connects to remote agent host or tunnels tool calls to runtime
- Live sync (AGT-002) works over the network
- Does not require LocalFs on the browser machine for package files

**Depends:** RT harness + storage adapters, AGT-001–005

---

## AGT-011: Context packaging for agents (token-efficient)

**Status:** Open · **Priority:** P2  
**As an** agent  
**I want** compact, high-signal context  
**So that** I edit VEIL efficiently (mission token efficiency)

**Acceptance criteria:**

- Default context pack: layer prompts, construct outline (topology), open
  selection, current diagnostics, expose contracts
- Prefer IR/summary over dumping entire generated Rust
- Budget controls (max tokens / max files)
- Aligns with PAR-009 (`veil prompt` / API)

---

## AGT-012: Config UX for providers and agents

**Status:** Open · **Priority:** P2  
**As a** user  
**I want** IDE/settings UI or documented config for models and ACP agents  
**So that** versatility is usable, not only theoretical

**Acceptance criteria:**

- Configure default ModelProvider + model id
- Configure optional ACP agent launchers
- Configure MCP server enablement
- Secrets via env / OS keychain / cloud IAM — never commit keys
- Example configs for: local Bedrock, local OpenAI-compatible, ACP+MCP bridge

---

## Suggested implementation order

1. **AGT-004** SourceStore (LocalFs) + **AGT-005** tools (in-process)  
2. **AGT-003** one ModelProvider + **AGT-006** built-in agent loop  
3. **AGT-001** panel + **AGT-002** live refresh  
4. **AGT-009** safety basics  
5. **AGT-008** MCP server (reuse tools)  
6. **AGT-007** ACP spike → client integration  
7. Object-store adapter + **AGT-010** when platform storage lands  
8. **AGT-011/012** polish  

This delivers “prompt → edit → IDE updates” early, then expands versatility
(Bedrock, ACP, MCP, remote) without blocking the vertical slice.
