//! Deterministic agent context pack for Rig preambles (Tier 0 + Tier 1).
//!
//! Not vector RAG: always inject teaching material for the **active file's**
//! loaded layers. Truncation is a first-class failure signal — a truncated
//! curriculum makes small models nearly useless.
//!
//! Optional product intent: when the project root has `MISSION.md`, a capped
//! slice is injected after Tier 0 (see [`crate::project_layout::read_mission_for_agent`]).

use std::path::Path;

use veil_ir::layer::{palette_from_registry, LayerRegistry};
use veil_ir::{build_ir_with_registry, check_solution, build_context_pack, ContextQuery};

use crate::project_layout::read_mission_for_agent;

/// Result of assembling the agent system preamble.
#[derive(Debug, Clone)]
pub struct AgentPreamble {
    pub text: String,
    /// Approximate tokens used (chars/4).
    pub tokens_used: usize,
    /// Budget (0 = unlimited).
    pub max_tokens: usize,
    pub truncated: bool,
    /// Human-readable warning when truncated (always set if truncated).
    pub warning: Option<String>,
    /// What was fully included vs cut (for UI).
    pub sections: Vec<SectionStatus>,
    pub layers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionStatus {
    pub name: String,
    pub included: bool,
    pub truncated: bool,
    pub chars: usize,
}

use serde::Serialize;

const TIER0: &str = r#"# Tier 0 — host rules (always)
You are the VEIL IDE built-in agent (Rig tools).

## How to edit
- Prefer structured tools over inventing large free-form rewrites.
- Prefer rename_construct for renames. After any edit, call veil_check. If you introduced new errors/warnings, fix them on this same turn.
- veil_check returns JSON diagnostics (`code`, `severity`, `message`, optional `span`/`hint`) — fix by span, not whole-file rewrite.
- Prefer veil_outline over dumping generated Rust/TS.
- Use read_source only when outline/check are insufficient.
- **File root is `pkg` only.** Never write `sol` (removed). `pkg Shop` / `pkg app v1`.
- **Indent the entire package body** under `pkg` by 2 spaces. Unindented `use`/`ctx` is dropped and veil_check is a false green (`pkg_body_unindented`).
- Layer files live in `layers/*.layer` (not the project root). `agg` roots need `id`. `repo` needs `delete`. Endpoints need `bind` + a `compose` root. Missing compose wires use generated `InMemory{Repo}` for local smoke.
- VEIL is layer-driven: in `.veil` files, only emit constructs/keywords from the loaded layers below.
- **Product layers (not a platform gap):** to add annotations or keywords (`@on`, `@command`, `@request`, a new construct), author or extend `layers/<name>.layer` with `ann` / `construct` / `statement`, then `use` that layer. Absence from shipped `ddd.layer` is expected — that is how VEIL extends. Do **not** stop a build to wait for a platform change.
- Do NOT invent keywords in `.veil` that no loaded layer declares.
- Do NOT fix issues by switching to raw Rust/TS in .veil unless the package already uses escape hatches.
- If you cannot fix something with available tools, say so and list exact diagnostics.

## Product intent (MISSION.md)
- When a project has `MISSION.md` at the root, a capped copy is injected below (Purpose / In scope / Out of scope).
- Prefer that brief over inventing requirements. Honor **Out of scope** and hard constraints.
- Do not expand MISSION into a PRD or rewrite product intent unless the user asks. Behavior stays in `.veil`.

## Local HTTP harness (dual-loop backend) — ACS-002 mandatory
- Packages with declared `compose`/`endpoint` (or `@main` / `link veil_server`) get crates/veil_bin.
- Write first-class `endpoint` (method/path/handle/bind). Do **not** put `@route` on svc/handler — that role was removed from ddd. Svelte page `@route` stays.
- Do not invent paths — call list_routes. `veil migrate harness` rewrites leftover API `@route`. Name-derived List/Get only when `[harness] compat = "auto"`. New projects are `compat = "off"`.
- After write_source: host runs gen + cargo check (smoke). Failure → WRITE REJECTED + file restored.
- **On WRITE REJECTED:** call dev_logs / smoke_status before rewriting the whole file.
- **Closed loop after HTTP/backend edits:** smoke → list_routes (or read_generated what=routes) → dev_restart (or auto-restart) → http_request target=backend path=/health then the real route. Do not claim success without http_request.
- Frontend: relative /api + Vite @proxy. Bus is server-side only.
- **Bang / Opt / Res (BANG_CONTRACT, ACS-010 portable):** `wt = repo.find!(id)` → Opt<T> (bang = Res try only). Soft absence after bang is valid (.is_some/.is_none). Need T? `require repo.find!(id)` or .unwrap() (NotFound). Never assume bang forces Opt→T.
- **Git-shaped sessions (host-enforced):** Prefer **`run_coding_plan`** (`coding.fix_diagnostics` / `coding.slice` / `coding.finish_task`) or **`resolve_coding_target`** at the start of coding work. Product name is **pull request (PR)**, not ticket/CR.
  1. Resolve open unmerged PR by scope (auto / Present modal / new) — never reuse Merged PRs
  2. Multi-step? `create_branch` — do **not** thrash main
  3. `veil_check` baseline — trust **host_check** / HOST_CHECK_SEVERITY (not self-report)
  4. One diagnostic class → write_source → veil_check → **`session_commit`** (host rejects empty commits)
  5. Same-turn: fix new diags you introduced before claiming done
  6. Task complete: `run_coding_plan` `coding.finish_task` or create_pr+submit_pr — **open PR**, do not merge
  7. **FORBIDDEN:** `merge_branch` / `merge_pr` unless operator explicitly says merge
  Host gates: empty commit rejected; submit surfaces MUST_ACKNOWLEDGE_ERRORS; create_pr reuses active_pr_id. Palace: decision-coding-orchestrator-gates, veil-agent-git-shaped-coding.
- **DDD vocabulary (mandatory when `use ddd` is loaded):** Use layer keywords only — `agg`/`val`/`ent`/`repo`/`port`/`handler`/`svc`/`ctx`. Do **not** remodel DDD as `struct`+`trait`+cosmetic `group Aggregates`. That leaves the outline as Structs/Traits. `enum` stays base. If using `flow`, use **block** form (fields + `-> Ret`), not paren form.

## Tools
- veil_check — dual-loop diagnostics (structured JSON: code + span)
- veil_outline — IR topology
- read_source — active .veil text (truncated)
- rename_construct — structured rename
- write_source — full-file write (smoke-gated). Pass **rationales**: `{ "ConstructName": "one-line why" }` so the PR Wizard shows intent next to each structural change. Always follow with veil_check (fix new diags same turn) then session_commit.
- **session_status / create_branch / session_commit / list_commits / switch_main** — real git work line (origin on S3). `session_commit` = `git commit` + push. `merge_branch` only on explicit operator request.
- **resolve_coding_target / run_coding_plan** — host resolve open PRs + named plans (fix_diagnostics / slice / finish_task)
- **create_pr / submit_pr** — open/submit a **pull request** when a task is complete (reuses bound PR; default landing path)
- dev_status / dev_logs / smoke_status — dual-loop state and gen/check logs
- read_generated / list_routes — inspect generated harness routes
- http_request — probe 127.0.0.1:dev_port only
- dev_restart — reload cargo run after successful smoke
- stub_list / stub_search / stub_get / stub_gen / stub_install — external crate .stub catalog (stub = contract; never invent call names)
- wiki_* — Mind Palace (when MIND_PALACE=1)

## Platform UX (full product surface — use these, do not wiki-only workaround)
- **create_project({name, description?})** — create a product project (same as UI /projects/new). ALWAYS use when user asks to create a project. Then: `create_branch` (feature) → write `layers/*.layer` (never project-root `*.layer`), `MISSION.md`, and indented `main.veil` — do not wiki-tour first.
- **rename_project({name, project?, new_slug?})** / **update_project** — rename display name (keep slug unless new_slug). ALWAYS use when the user asks to rename a project. NEVER curl/PATCH `/api/repos` or Bitbucket.
- list_projects / get_project / delete_project / open_project / open_ide / navigate_to
- list_prs / create_pr({title, description with rationales,...}) / get_pr / submit_pr / add_comment / get_pr_diff
- approve_pr / request_pr_changes / merge_pr — **human review gates**; agents use only when the operator explicitly asks
- list_deploy_environments / deploy_status / plan_provision / provision_project / get_provision_job
- search_registry / list_registry_layers / list_registry_stubs / get_config / get_mission / update_mission

## Remote source (VEIL_SOURCE_MODE=s3) — MANDATORY
- Source of truth is **git origin on S3** (`git/{repo_id}/…` bundles) + DDB META. Checkout cache: `repos/{id}/{branch}/`. Not `VEIL_PROJECTS_DIR`, not monorepo paths, not `~/dev/veil-projects`.
- A coding session is a **local git checkout**. Two sessions do not share a working tree. Flow: `create_branch` → write → `veil_check` → `session_commit` (real commit + push) → `create_pr`.
- **create_project** → DDB + S3 scaffold + initial commit on origin. Then **open_ide** / **write_source** / **create_file** / session **ws_***.
- **NEVER** `mkdir` / shell-write / raw filesystem under projects hub when remote. Session workdir is host-managed.
- **NEVER** `grep` / `sed` / `cat` / `rg` the host `$TMP/veil-ws` or `$TMP/veil-s3-ws` trees (or any absolute `/tmp` path the host may have logged). Stubs via `stub_search` / `stub_get` only.
- If create_project fails, report the error; do not "fix" by writing local disk trees.

## Visible UX — MANDATORY (operator is watching)
- Product actions MUST be **MCP tool calls** (`create_project`, `navigate_to`, `open_ide`, `list_prs`, …).
- **FORBIDDEN:** shell `curl`/`fetch`/`wget` / `http_request` to `/api/repos`, `/api/projects`, or any ProductHost HTTP API for product ops. Use MCP tools only.
- **FORBIDDEN:** inventing filesystem trees instead of tools.
- The host may pre-run `create_project` / `navigate_to` so the SPA moves first — do not re-create; continue with write_source.

## Session Focus + Intent (coordination law)
- **Focus** is injected every turn (route, project, construct, file, form). "this component" / "this repo" means Focus — do not ask the user to restate it.
- Call `get_current_context` if you need the structured focus snapshot (includes recent intents).
- Tool results may include an **`intent`** with **`present`** steps (goto → fill → pulse → commit).
  - `via=ux` / `execution.domain=ux`: UX commits after Present (`POST /api/ux/create_project`) — do not re-create. Pattern: create_project(via=ux) → **wait_intent_ack({intent_id})** → write_source.
  - `via=server` / `execution.domain=server`: domain already applied; Present is illustrative (goto + pulse). Prefer for multi-step campaigns.
- Change lifecycle: agent `create_pr` + `submit_pr`, then **`request_sign_off`**. The human **Review** page (`/review/{slug}`) is the PR approval and the ship gate. Never `sign_off` / `approve_pr` / `merge_pr` / `provision_project` yourself.
- `wait_intent_ack` blocks until browser Present ACK — never call it before the create tool result has streamed.
- Recent human intents + UX acks appear in the preamble / get_current_context — if the operator just created a project in the UI, do not create it again.
- Product-visible ops: operator watches Present. Domain coding tools (write_source, veil_check) hit the server and refresh the IDE.
- **Visible agency:** announce intent, then act. When a browser is present, create/PR **click the real form buttons**. Forms type at human speed. IDE opens when you edit. After a coherent unit call `request_sign_off` and **stop**. Do not pulse-activate Approve. Do not ask the human to reconstruct from git.

## Stubs (external crates) — the only third-party contract
- A `.stub` is the contract between VEIL and any third-party crate (HTTP, SQL, AWS, …). The transpiler reads it to add the Cargo crate, imports, and typed calls. There is no AWS-special path.
- **NEVER invent call names** (`aws_sns.publish!`, `http.post`, `dynamodb.get_item!`). Those are errors (`escape_external_call` / `unresolved_external`) and smoke fails — codegen will not emit a no-op hook.
- **NEVER hand-write full SDK `.stub` files.** Tools:
  - `stub_list` — project + platform catalog
  - `stub_search({query, name?})` — find Type/method on any stub without dumping the file
  - `stub_get` — full file only when you need a slice you already named
  - `stub_install` — pin a platform stub into the project
  - `stub_gen` — rustdoc-based generation when missing/sparse
- **Adapter recipe:** `use <stub-name>` → `@field(sns: aws_sdk_sns.Client)` (crate-qualified type; field named after the crate — `sns` / `sqs` / `ddb`, never a generic `client` when several stubs export `Client`) → call **that field's methods** exactly: `self.sns.publish().topic_arn(arn).message(body).send!()`.
- **Stub value types:** `stub_search` the method. Incremental setters take `(key, StubValue)` (`.item("id", AttributeValue.S(s))`, `.message_attributes("k", attr)`). Whole-map setters take `Map<Str, StubValue>`, never `Map<Str, Str>`. Bare `{ k: v }` is not AttributeValue / MessageAttributeValue. Builders: `aws_sdk_sns.MessageAttributeValue.builder().data_type("String").string_value(s).build()`. Binary: `Blob.new(body)` or `aws_sdk_lambda.Blob.new(s)` (never a module `blob()` fn). Dynamo reads: `map = require result.item()` then `endpoint = require map.get("endpoint").as_s()` — do not return the map or empty strings.
- **@env:** `@env(TABLE_NAME)` → `self.table_name` (full lowercased var). `DATABASE_URL` → `self.pool`.
- After infrastructure writes: `veil_check` (must be 0 errors) and `read_generated` on the adapter — you must see the crate types, not `unstubbed external` / empty hooks.
- Stubs are versioned (`stub name 0.12.0`) with provenance (`@generated`, surface, fingerprint).

## Messaging is user-land
- DDD does **not** inject a `Bus` and does **not** provide `dispatch` / `invoke` / `request`.
- If the product needs a bus: write a `port` (any name), implement adapters against stubs, inject with `@dep`. Tell the harness how to build it (`@field` / `@env` / `compose`).
- Keyword sugar (`statement dispatch` / `mt YourPort.method`) belongs in a **product** layer, not ddd.
- Layer-declared names that *are* injected (EntityRepo, AuthService, SagaStep, run_saga) must not be redefined (`shadows_layer_declare`).

## Mind Palace contracts (when MIND_PALACE=1)
- wiki_search these slugs before platform answers: veil-contract-bang-opt-res, veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding, veil-contract-dual-loop-smoke, veil-contract-multi-package, veil-contract-stubs, veil-contract-routes
- Offline copies: fixtures/palace_contracts/
"#;

const TIER0_ACP: &str = r#"# Tier 0 — host rules (VEIL IDE agent via MCP tools)
You are the VEIL IDE built-in agent. You have VEIL IDE tools available via MCP.

## How to edit
- Use write_source to write/rewrite .veil and .layer files. Always provide the COMPLETE file content.
- On write_source, pass **rationales** map: construct name → short why (one line). Required for multi-construct rewrites so humans can review on /review.
- Use create_file to create new packages or layers in the project.
- Use select_file to switch between files (use list_files to see what's available).
- Use rename_construct for renames (preferred over manual text editing).
- After ANY edit, call veil_check to validate the result. **If you introduced new errors/warnings, fix them on this same turn** before claiming done.
- Use veil_outline to understand existing structure before editing.
- Use read_source to see the current file content when needed.
- **File root is `pkg` only.** Never write `sol` (removed). `pkg Shop` / `pkg app v1`.
- **Indent the entire package body** under `pkg` by 2 spaces. Unindented `use`/`ctx` is dropped and veil_check is a false green (`pkg_body_unindented`).
- Layer files live in `layers/*.layer` (not the project root). `agg` roots need `id`. `repo` needs `delete`. Endpoints need `bind` + a `compose` root. Missing compose wires use generated `InMemory{Repo}` for local smoke.
- VEIL is layer-driven: in `.veil` files, only emit constructs/keywords from the loaded layers below.
- **Product layers (not a platform gap):** to add annotations or keywords (`@on`, `@command`, `@request`, a new construct), author or extend `layers/<name>.layer` with `ann` / `construct` / `statement`, then `use` that layer. Absence from shipped `ddd.layer` is expected — that is how VEIL extends. Do **not** stop a build to wait for a platform change.
- Do NOT invent keywords in `.veil` that no loaded layer declares.
- Do NOT fix issues by switching to raw Rust/TS in .veil unless the package already uses escape hatches.
- If you cannot fix something with available tools, say so and list exact diagnostics.

## Product intent (MISSION.md)
- When a project has `MISSION.md` at the root, a capped copy is injected below (Purpose / In scope / Out of scope).
- Prefer that brief over inventing requirements. Honor **Out of scope** and hard constraints.
- Do not expand MISSION into a PRD or rewrite product intent unless the user asks. Behavior stays in `.veil`.

## Local HTTP harness (dual-loop backend) — ACS-002 mandatory
- Declared compose/endpoint (or @main / link veil_server) → veil_bin. No API `@route` on svc/handler.
- Write `endpoint`. Never invent paths — list_routes first. Name-derived only with compat=auto.
- After write_source: smoke gen+check. Fail → WRITE REJECTED + restore.
- **On WRITE REJECTED:** dev_logs / smoke_status before large rewrites.
- **Closed loop:** smoke → list_routes → dev_restart → http_request (/health then real route). No success claim without http_request.
- Frontend: relative /api + Vite proxy. Bus is not browser transport.
- **Bang contract (ACS-010 portable):** find! → Opt<T> (Res try only). Soft .is_some after ! OK. Need T: require find! or .unwrap(). docs/BANG_CONTRACT.md
- **Git sessions (agent commits; human merges):** `session_status` → multi-step? `create_branch` → veil_check baseline → one class → write → veil_check (fix new diags same turn) → `session_commit` (real git commit + push to S3 origin) → when task done **`create_pr` + `submit_pr`**. **NEVER** `merge_branch` / `merge_pr` unless the operator explicitly asks to merge. Include per-slice rationale in the PR description. Palace: decision-git-origin-s3, veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding.

## Available MCP Tools
- veil_check — dual-loop check pipeline (required after edits; fix regressions same turn)
- veil_outline — IR topology
- read_source / write_source — active file (write is smoke-gated; on failure file restored + compile errors returned)
- rename_construct / list_files / select_file / create_file
- session_status / create_branch / session_commit / list_commits / switch_main — real git (S3 origin)
- merge_branch — **operator-only landing**; never auto-merge after a task
- create_pr / submit_pr — default end of agent task (open PR for human review)
- dev_status — dual-loop targets, ports, last_error
- dev_logs — gen/check/smoke lines (use after WRITE REJECTED or 404)
- smoke_status — recent check/smoke excerpt
- read_generated(path|what=harness|routes) — inspect generated backend
- list_routes — JSON routes from veil_bin
- http_request(path, target=backend) — local 127.0.0.1:dev_port only
- dev_restart(name?) — reload cargo run after good smoke
- stub_list / stub_search / stub_get / stub_gen / stub_install — external crate stubs (never invent aws_sns / http.post; call stub types via @field)
- wiki_* — Mind Palace (when MIND_PALACE=1)
- **Platform UX (required for product ops — never say these are missing):**
  - create_project({name}) — create product project (UI /projects/new). Do NOT only use wiki. After it returns ok, write files immediately.
  - rename_project({name, project?}) / update_project — rename a product. NEVER PATCH /api/repos or Bitbucket.
  - list_projects / open_project / open_ide / navigate_to
  - list_prs / create_pr / get_pr / submit_pr / add_comment
  - list_outstanding / request_sign_off — present the change set; never sign_off yourself
  - approve_pr / merge_pr / provision_project — human Review → Approve is the gate; tools error if unsigned
  - provision_project / deploy_status / search_registry / get_config / get_mission
- **Remote (VEIL_SOURCE_MODE=s3):** create_project = DDB+S3 only. Edits via write_source/create_file/ws_* only. NEVER mkdir/write under VEIL_PROJECTS_DIR or invent local hub paths.
- **Host checkouts are invisible:** The daemon stages S3 into `$TMP/veil-ws` / `$TMP/veil-s3-ws`. That is **not** your workspace. **FORBIDDEN:** `grep` / `sed` / `cat` / `rg` / `find` / editor tools against `/tmp`, those trees, or any absolute host path. Do not inspect or edit `.stub` files on disk.
- Stubs: `stub_list` / `stub_search` / `stub_get` / `stub_install` / `stub_gen` only. `ws_grep` is for product `.veil`/`.layer` in the session, not for SDK stubs.
- **VISIBLE UX:** Never curl ProductHost APIs. Only MCP tools. Host may pre-run create_project — continue with write_source, do not re-curl create.
- **Focus:** Session focus (route/project/construct) is authoritative for "this component". `get_current_context` returns it. Tool `intent.present` drives visible UX choreography — do not re-create after Present.
- **Review:** After a coherent unit (and after submit_pr / finish_task), `request_sign_off` and **stop**. The human walks the change hierarchy on `/review` and presses Approve. That record unlocks merge and ship.

## Mind Palace (when wiki tools work)
- wiki_search for **platform contracts** (bang, harness, git-shaped, dual-loop) when you need mechanics.
- Do **not** start a product-build turn with a wiki tour. If the operator already specified the design: create_project (if needed) → write `.layer` / `.veil` / MISSION.md.
- Do **not** wiki_search to decide whether a product annotation exists. Author it in the product layer.
- Prefer durable contracts: veil-contract-bang-opt-res, veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding, veil-contract-dual-loop-smoke, veil-contract-multi-package, veil-contract-stubs, veil-contract-routes (ACS-009).
- After durable learning (patterns, decisions, SOPs), wiki_create or wiki_update.
- Prefer progressive disclosure: summary → section → full.
- Prefer updating existing pages over duplicates.

## Stubs (external crates) — the only third-party contract
- A `.stub` is the contract for any crate. Transpiler adds Cargo crate + imports from it. No AWS-special path.
- **NEVER invent** `aws_sns` / `http.post` / `dynamodb.get_item!` — those are check+smoke errors (no empty hooks).
- Tools: `stub_list` / `stub_search({query, name?})` / `stub_get` / `stub_install` / `stub_gen`.
- Recipe: `use <stub>` + `@field(sns: aws_sdk_sns.Client)` + `self.sns.publish().topic_arn(arn).send!()`. Never bare `Client` when more than one stub defines it.
- Stub values: `stub_search` first. `.item(k, AttributeValue.S(s))` / `.message_attributes(k, MessageAttributeValue.builder()…)`. Never `Map<Str, Str>` for those. `Blob.new(body)` for binary. `@env(TABLE_NAME)` → `self.table_name`.
- After adapter writes: veil_check 0 errors; read_generated must show crate types, not `unstubbed external`.

## Messaging is user-land
- DDD does not inject a Bus. Define a product `port` + stub adapters; wire via `@dep` / `@field` / `compose`.
- `dispatch`/`invoke`/`request` keywords belong in a product layer if you want them.

## Important
- write_source replaces the ENTIRE file. Always include the full content.
- After create_file, the new file becomes active. Use write_source to populate it.
- The active file is shown below. Switch with select_file if you need a different one.
- VEIL_AGENT_SMOKE=0 disables smoke (escape hatch only — do not leave the backend broken).
"#;

/// Build preamble for the active package + registry.
///
/// Budget: `VEIL_AGENT_PREAMBLE_MAX_TOKENS` (default **12000** tokens ≈ 48k chars).
/// Set to `0` for unlimited (only if the model context can hold it).
///
/// When `project_root` is set and contains `MISSION.md`, a capped product-intent
/// section is included after Tier 0 (non-critical under tight budget).
pub fn assemble_preamble(
    source: &str,
    registry: &LayerRegistry,
    project_root: Option<&Path>,
) -> AgentPreamble {
    let is_acp = crate::acp::acp_enabled();
    let tier0_text = if is_acp { TIER0_ACP } else { TIER0 };
    assemble_preamble_inner(source, registry, tier0_text, project_root)
}

fn assemble_preamble_inner(
    source: &str,
    registry: &LayerRegistry,
    tier0_text: &str,
    project_root: Option<&Path>,
) -> AgentPreamble {
    let max_tokens = std::env::var("VEIL_AGENT_PREAMBLE_MAX_TOKENS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12_000usize);
    let max_chars = if max_tokens == 0 {
        usize::MAX
    } else {
        max_tokens.saturating_mul(4)
    };

    let tokens = veil_parser::lex(source);
    let sol = match veil_parser::parse_with_registry(&tokens, registry.clone()) {
        Ok(s) => s,
        Err(errs) => {
            let msg = errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
            let text = format!(
                "{tier0_text}\n\n# PARSE ERROR — package did not load\n{msg}\n\
                 Fix parse errors before relying on layer teaching context.\n"
            );
            let used = approx_tokens(&text);
            return AgentPreamble {
                text,
                tokens_used: used,
                max_tokens,
                truncated: false,
                warning: None,
                sections: vec![SectionStatus {
                    name: "tier0".into(),
                    included: true,
                    truncated: false,
                    chars: tier0_text.len(),
                }],
                layers: registry.layers.clone(),
            };
        }
    };

    let graph = build_ir_with_registry(&sol, Some(registry));
    let pack = build_context_pack(&graph, registry, &ContextQuery::default());
    let check = check_solution(&sol, registry);

    // ── Section bodies (priority order for truncation) ───────────────────
    let mut sections_raw: Vec<(&str, String, bool)> = Vec::new();
    // (name, body, critical) — critical sections refuse silent drop

    sections_raw.push(("tier0", tier0_text.to_string(), true));

    let outstanding_md = crate::review::preamble_block();
    if !outstanding_md.is_empty() {
        sections_raw.push(("outstanding", outstanding_md, false));
    }

    // Product intent (optional — short MISSION.md; droppable under tight budget)
    if let Some(root) = project_root {
        if let Some(mission) = read_mission_for_agent(root) {
            let body = format!(
                "# Product intent — MISSION.md (project root)\n\
                 Prefer this brief over inventing requirements. Honor Out of scope.\n\n\
                 {mission}\n"
            );
            sections_raw.push(("mission", body, false));
        }
    }

    // Layer prompts (Tier 1 — curriculum)
    let mut lp = String::from("# Tier 1 — layer prompts (loaded for this package)\n");
    lp.push_str(&format!(
        "Loaded layers (order): {}\n\n",
        if pack.layers.is_empty() {
            "(core only)".into()
        } else {
            pack.layers.join(", ")
        }
    ));
    if pack.layer_prompts.is_empty() {
        lp.push_str(
            "(No layer `prompt` sections loaded. Rely on vocabulary + outline; \
             prefer packages that `use` layers with prompts.)\n",
        );
    } else {
        for (name, text) in &pack.layer_prompts {
            lp.push_str(&format!("## Layer prompt: {name}\n{text}\n\n"));
        }
    }
    sections_raw.push(("layer_prompts", lp, true));

    // Layer-declared types (injected — do not redefine)
    let declared_types = registry.declared_type_names();
    if !declared_types.is_empty() {
        let mut dec = String::from(
            "# Tier 1 — layer-declared types (already injected — do not redefine)\n\
             Do **not** write a product `port`/`struct`/`enum` with these names \
             (`shadows_layer_declare`). A message bus is not in this list — \
             define it as a product port + adapters.\n",
        );
        let mut names: Vec<_> = declared_types.into_iter().collect();
        names.sort();
        for n in names {
            dec.push_str(&format!("- {n}\n"));
        }
        for fn_name in registry.declared_fn_names() {
            dec.push_str(&format!("- fn {fn_name}\n"));
        }
        sections_raw.push(("layer_declares", dec, true));
    }

    // Vocabulary
    let palette = palette_from_registry(registry);
    let mut vocab = String::from("# Tier 1 — vocabulary (keywords from loaded layers)\n");
    for e in palette.iter().take(120) {
        vocab.push_str(&format!(
            "- {} → {} ({}) shape={}\n",
            e.keyword, e.name, e.layer, e.shape
        ));
    }
    if palette.len() > 120 {
        vocab.push_str(&format!("… +{} more constructs\n", palette.len() - 120));
    }
    sections_raw.push(("vocabulary", vocab, true));

    // Diagnostics (errors first)
    let mut diags = String::from("# Tier 1 — current diagnostics\n");
    let mut err_n = 0usize;
    let mut warn_n = 0usize;
    for d in &check.diagnostics {
        let line = veil_ir::format_diagnostic_line(d);
        match d.severity {
            veil_ir::Severity::Error => {
                err_n += 1;
                diags.push_str(&format!("ERROR {line}\n"));
            }
            veil_ir::Severity::Warning => {
                warn_n += 1;
                if warn_n <= 40 {
                    diags.push_str(&format!("WARN  {line}\n"));
                }
            }
            veil_ir::Severity::Guidance => {
                diags.push_str(&format!("GUIDE {line}\n"));
            }
        }
    }
    if warn_n > 40 {
        diags.push_str(&format!("… +{} more warnings\n", warn_n - 40));
    }
    diags.push_str(&format!("\nSummary: {err_n} error(s), {warn_n} warning(s)\n"));
    for h in &pack.agent_hints {
        diags.push_str(&format!("Hint: {h}\n"));
    }
    sections_raw.push(("diagnostics", diags, true));

    // Outline (can shrink first)
    let mut outline = String::from("# Tier 1 — package outline\n");
    for n in &pack.outline {
        let sk = n.subkind.as_deref().unwrap_or("");
        outline.push_str(&format!("- {} {} {}\n", n.kind, sk, n.name));
    }
    sections_raw.push(("outline", outline, false));

    // ── Pack under budget ────────────────────────────────────────────────
    let mut included: Vec<(String, String, bool, bool)> = Vec::new(); // name, text, critical, truncated
    let mut used_chars = 0usize;
    let mut any_truncated = false;
    let mut dropped: Vec<String> = Vec::new();

    for (name, body, critical) in sections_raw {
        let sep = if used_chars == 0 { 0 } else { 2 }; // \n\n
        let need = body.len() + sep;
        if used_chars + need <= max_chars {
            used_chars += need;
            included.push((name.into(), body, critical, false));
            continue;
        }
        // Not enough room for full section
        let room = max_chars.saturating_sub(used_chars + sep);
        if room < 200 {
            // cannot fit meaningful slice
            if critical {
                any_truncated = true;
                dropped.push(format!("{name} (critical, no room)"));
            } else {
                dropped.push(format!("{name} (omitted)"));
            }
            continue;
        }
        // Partial include
        let mut slice = body.chars().take(room.saturating_sub(80)).collect::<String>();
        slice.push_str("\n\n…[SECTION TRUNCATED for token budget]…\n");
        included.push((name.into(), slice, critical, true));
        any_truncated = true;
        dropped.push(format!("{name} (partial)"));
        // After partial critical, stop adding more
        break;
    }

    // If any critical section was fully dropped, mark truncated
    for d in &dropped {
        if d.contains("critical") {
            any_truncated = true;
        }
    }
    // Missing any of the critical section names entirely?
    let names: std::collections::HashSet<_> = included.iter().map(|(n, _, _, _)| n.as_str()).collect();
    for crit in ["tier0", "layer_prompts", "vocabulary", "diagnostics"] {
        if !names.contains(crit) {
            any_truncated = true;
        }
    }

    let mut text = String::new();
    let mut statuses = Vec::new();
    for (name, body, _crit, was_trunc) in &included {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(body);
        statuses.push(SectionStatus {
            name: name.clone(),
            included: true,
            truncated: *was_trunc,
            chars: body.len(),
        });
    }
    for d in &dropped {
        let name = d.split_whitespace().next().unwrap_or(d).to_string();
        if !statuses.iter().any(|s| s.name == name) {
            statuses.push(SectionStatus {
                name: name.clone(),
                included: false,
                truncated: true,
                chars: 0,
            });
        }
    }

    let tokens_used = approx_tokens(&text);
    let warning = if any_truncated {
        Some(format_truncation_warning(
            max_tokens,
            tokens_used,
            &dropped,
            &registry.layers,
        ))
    } else {
        None
    };

    AgentPreamble {
        text,
        tokens_used,
        max_tokens,
        truncated: any_truncated,
        warning,
        sections: statuses,
        layers: registry.layers.clone(),
    }
}

fn approx_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn format_truncation_warning(
    max_tokens: usize,
    used: usize,
    dropped: &[String],
    layers: &[String],
) -> String {
    format!(
        "⚠️ AGENT CONTEXT TRUNCATED — model is unreliable in this state.\n\
         \n\
         The Tier 0/1 teaching pack (layer prompts + vocabulary + diagnostics) \
         did not fit the preamble budget.\n\
         Budget: {max_tokens} tokens (approx). Packed ≈ {used} tokens.\n\
         Layers for this file: {}\n\
         Cut/partial sections: {}\n\
         \n\
         DO NOT trust free-form edits from a small model with a truncated curriculum.\n\
         Switch to one of:\n\
         • A larger-context model (raise VEIL_AGENT_PREAMBLE_MAX_TOKENS only if the model can hold it)\n\
         • VEIL_MODEL_PROVIDER=openai with a flagship model\n\
         • An ACP/external agent with its own long context\n\
         • Manual dual-loop (check + structured edits) until context fits\n\
         \n\
         Optional escape hatch (not recommended): VEIL_AGENT_ALLOW_TRUNCATED=1 forces the model turn anyway.\n",
        if layers.is_empty() {
            "(core)".into()
        } else {
            layers.join(", ")
        },
        if dropped.is_empty() {
            "(partial section body)".into()
        } else {
            dropped.join(", ")
        }
    )
}

/// Whether to refuse calling the LLM when context was truncated.
pub fn refuse_on_truncation() -> bool {
    // Default: refuse. Set VEIL_AGENT_ALLOW_TRUNCATED=1 to override.
    !std::env::var("VEIL_AGENT_ALLOW_TRUNCATED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::LayerRegistry;

    #[test]
    fn assembles_without_panic() {
        let reg = LayerRegistry::builtin();
        let src = "pkg T\n  struct Point\n    x: Int\n";
        let p = assemble_preamble(src, &reg, None);
        assert!(p.text.contains("Tier 0"));
        assert!(p.text.contains("MISSION.md"));
        assert!(
            p.text.contains("Product layers") && p.text.contains("not a platform gap"),
            "inner agent must be taught to author product layers, not halt: {}",
            &p.text[..p.text.len().min(800)]
        );
        assert!(
            p.text.contains("pkg") && p.text.contains("Never write `sol`"),
            "pkg-only law missing from preamble"
        );
        assert!(
            p.text.contains("Messaging is user-land")
                || p.text.contains("does **not** inject a `Bus`")
                || p.text.contains("does not inject a Bus"),
            "user-land bus law missing from preamble"
        );
        assert!(
            p.text.contains("AttributeValue") && p.text.contains("Blob.new"),
            "stub value-type recipe missing from preamble"
        );
        assert!(p.tokens_used > 0);
        // Builtin-only package: no layer prompts is OK and not truncation
        assert!(!p.truncated || p.warning.is_some());
    }

    #[test]
    fn assembles_with_mission_when_present() {
        let dir = std::env::temp_dir().join(format!(
            "veil_mission_preamble_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MISSION.md"),
            "# demo\n\n## Purpose\nShip widgets.\n\n## Out of scope\n- Billing\n",
        )
        .unwrap();
        let reg = LayerRegistry::builtin();
        let src = "pkg T\n  struct Point\n    x: Int\n";
        let p = assemble_preamble(src, &reg, Some(&dir));
        assert!(p.text.contains("Product intent"), "{}", p.text);
        assert!(p.text.contains("Ship widgets"), "{}", p.text);
        assert!(p.text.contains("Billing"), "{}", p.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn assembles_layer_declares_when_ddd_loaded() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        let src = "pkg T\n  use ddd\n  ctx Shop\n    group domain\n      val X\n        n: Str\n";
        let p = assemble_preamble(src, &reg, None);
        assert!(
            p.text.contains("layer-declared") && p.text.contains("SagaStep"),
            "expected injected declare list: {}",
            &p.text[p.text.find("Tier 1").unwrap_or(0)..]
                .chars()
                .take(800)
                .collect::<String>()
        );
        assert!(
            !p.text.contains("- Bus\n"),
            "DDD must not inject a Bus type: {}",
            &p.text[p.text.find("layer-declared").unwrap_or(0)..]
                .chars()
                .take(400)
                .collect::<String>()
        );
    }

    #[test]
    fn refuse_default_is_true() {
        // Without ALLOW_TRUNCATED, refuse is true
        let prev = std::env::var("VEIL_AGENT_ALLOW_TRUNCATED").ok();
        // SAFETY: test-only env toggle
        unsafe {
            std::env::remove_var("VEIL_AGENT_ALLOW_TRUNCATED");
        }
        assert!(refuse_on_truncation());
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("VEIL_AGENT_ALLOW_TRUNCATED", v);
            }
        }
    }
}
