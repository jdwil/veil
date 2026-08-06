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
- Prefer rename_construct for renames. After any edit, call veil_check.
- veil_check returns JSON diagnostics (`code`, `severity`, `message`, optional `span`/`hint`) — fix by span, not whole-file rewrite.
- Prefer veil_outline over dumping generated Rust/TS.
- Use read_source only when outline/check are insufficient.
- VEIL is layer-driven: only emit constructs/keywords from the loaded layers below.
- Do NOT invent keywords from layers that are not listed.
- Do NOT fix issues by switching to raw Rust/TS in .veil unless the package already uses escape hatches.
- If you cannot fix something with available tools, say so and list exact diagnostics.

## Product intent (MISSION.md)
- When a project has `MISSION.md` at the root, a capped copy is injected below (Purpose / In scope / Out of scope).
- Prefer that brief over inventing requirements. Honor **Out of scope** and hard constraints.
- Do not expand MISSION into a PRD or rewrite product intent unless the user asks. Behavior stays in `.veil`.

## Local HTTP harness (dual-loop backend) — ACS-002 mandatory
- Packages with context modules get crates/veil_bin REST harness even without @main.
- Prefer @route("GET /api/…") on svc/handlers. Name-derived List/Get/Create paths are fallback only — never invent paths; call list_routes.
- After write_source: host runs gen + cargo check (smoke). Failure → WRITE REJECTED + file restored.
- **On WRITE REJECTED:** call dev_logs / smoke_status before rewriting the whole file.
- **Closed loop after HTTP/backend edits:** smoke → list_routes (or read_generated what=routes) → dev_restart (or auto-restart) → http_request target=backend path=/health then the real route. Do not claim success without http_request.
- Frontend: relative /api + Vite @proxy. Bus is server-side only.
- **Bang / Opt / Res (BANG_CONTRACT, ACS-010 portable):** `wt = repo.find!(id)` → Opt<T> (bang = Res try only). Soft absence after bang is valid (.is_some/.is_none). Need T? `require repo.find!(id)` or .unwrap() (NotFound). Never assume bang forces Opt→T.
- **Git-shaped sessions (mandatory for multi-step fixes):** Decide branch/commit/merge yourself — the operator should not micromanage git.
  1. `session_status` — see branch / uncommitted / head_commit
  2. Multi-step or fix campaign? `create_branch` with a short name (e.g. `fix-type-mismatch`) — do **not** thrash main
  3. `veil_check` → note error_count / warning_count
  4. Fix ONE diagnostic class → write_source → veil_check (report before→after counts)
  5. `session_commit` with a descriptive message after each successful slice
  6. `merge_branch` only when task complete with acceptable checks, or operator asked to land
  Autosave ≠ commit. Change list size ≠ errors fixed. Palace: veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding.

## Tools
- veil_check — dual-loop diagnostics (structured JSON: code + span)
- veil_outline — IR topology
- read_source — active .veil text (truncated)
- rename_construct — structured rename
- write_source — full-file write (smoke-gated)
- **session_status / create_branch / session_commit / list_commits / merge_branch / switch_main** — git-shaped work line
- dev_status / dev_logs / smoke_status — dual-loop state and gen/check logs
- read_generated / list_routes — inspect generated harness routes
- http_request — probe 127.0.0.1:dev_port only
- dev_restart — reload cargo run after successful smoke
- stub_list / stub_get / stub_gen / stub_install — external crate .stub catalog
- wiki_* — Mind Palace (when MIND_PALACE=1)

## Platform UX (full product surface — use these, do not wiki-only workaround)
- **create_project({name, description?})** — create a product project (same as UI /projects/new). ALWAYS use when user asks to create a project.
- list_projects / get_project / delete_project / open_project / open_ide / navigate_to
- list_changes / create_change({title,...}) / get_change / submit_change / approve_change / request_changes / merge_change / add_comment / get_change_diff
- list_deploy_environments / deploy_status / plan_provision / provision_project / get_provision_job
- search_registry / list_registry_layers / list_registry_stubs / get_config / get_mission / update_mission

## Remote source (VEIL_SOURCE_MODE=s3) — MANDATORY
- Source of truth is **DDB META + S3** (`repos/{id}/{branch}/…`). Not `VEIL_PROJECTS_DIR`, not monorepo paths, not `~/dev/veil-projects`.
- **create_project** → DDB + S3 scaffold only. Then **open_ide** / **write_source** / **create_file** / session **ws_***.
- **NEVER** `mkdir` / shell-write / raw filesystem under projects hub when remote. Materialize is `$TMP/veil-s3-ws` (or session workdir) with S3 write-through — host-managed.
- If create_project fails, report the error; do not "fix" by writing local disk trees.

## Visible UX — MANDATORY (operator is watching)
- Product actions MUST be **MCP tool calls** (`create_project`, `navigate_to`, `open_ide`, `list_changes`, …).
- **FORBIDDEN:** shell `curl`/`fetch`/`wget` to `/api/repos`, `/api/projects`, or any ProductHost HTTP API for product ops.
- **FORBIDDEN:** inventing filesystem trees instead of tools.
- The host may pre-run `create_project` / `navigate_to` so the SPA moves first — do not re-create; continue with write_source.

## Session Focus + Intent (coordination law)
- **Focus** is injected every turn (route, project, construct, file, form). "this component" / "this repo" means Focus — do not ask the user to restate it.
- Call `get_current_context` if you need the structured focus snapshot (includes recent intents).
- Tool results may include an **`intent`** with **`present`** steps (goto → fill → pulse → commit).
  - `via=ux` / `execution.domain=ux`: UX commits after Present (`POST /api/ux/create_project`) — do not re-create. Pattern: create_project(via=ux) → **wait_intent_ack({intent_id})** → write_source.
  - `via=server` / `execution.domain=server`: domain already applied; Present is illustrative (goto + pulse). Prefer for multi-step campaigns.
- Change lifecycle (`submit_change` / `approve_change` / `merge_change`) and deploy (`provision_project`) return Present so the operator sees the page update.
- `wait_intent_ack` blocks until browser Present ACK — never call it before the create tool result has streamed.
- Recent human intents + UX acks appear in the preamble / get_current_context — if the operator just created a project in the UI, do not create it again.
- Product-visible ops: operator watches Present. Domain coding tools (write_source, veil_check) hit the server and refresh the IDE.

## Stubs (external crates) — mandatory
- **NEVER invent or hand-write full SDK `.stub` files.** Use tools:
  - `stub_list` — project + platform catalog
  - `stub_get` — resolve content (project stubs/ first, then platform)
  - `stub_install` — pin a platform stub into the project
  - `stub_gen` — rustdoc-based generation (`veil stub-gen`) when missing/sparse
- Stubs are versioned (`stub name 0.12.0`) with provenance (`@generated`, surface, fingerprint).
- Curated tiny surfaces only when marked `surface curated` and version-pinned.

## Mind Palace contracts (when MIND_PALACE=1)
- wiki_search these slugs before platform answers: veil-contract-bang-opt-res, veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding, veil-contract-dual-loop-smoke, veil-contract-multi-package, veil-contract-stubs, veil-contract-routes
- Offline copies: fixtures/palace_contracts/
"#;

const TIER0_ACP: &str = r#"# Tier 0 — host rules (VEIL IDE agent via MCP tools)
You are the VEIL IDE built-in agent. You have VEIL IDE tools available via MCP.

## How to edit
- Use write_source to write/rewrite .veil and .layer files. Always provide the COMPLETE file content.
- Use create_file to create new packages or layers in the project.
- Use select_file to switch between files (use list_files to see what's available).
- Use rename_construct for renames (preferred over manual text editing).
- After ANY edit, call veil_check to validate the result.
- Use veil_outline to understand existing structure before editing.
- Use read_source to see the current file content when needed.
- VEIL is layer-driven: only emit constructs/keywords from the loaded layers below.
- Do NOT invent keywords from layers that are not listed.
- Do NOT fix issues by switching to raw Rust/TS in .veil unless the package already uses escape hatches.
- If you cannot fix something with available tools, say so and list exact diagnostics.

## Product intent (MISSION.md)
- When a project has `MISSION.md` at the root, a capped copy is injected below (Purpose / In scope / Out of scope).
- Prefer that brief over inventing requirements. Honor **Out of scope** and hard constraints.
- Do not expand MISSION into a PRD or rewrite product intent unless the user asks. Behavior stays in `.veil`.

## Local HTTP harness (dual-loop backend) — ACS-002 mandatory
- Context modules → veil_bin REST harness; @main optional for local HTTP.
- Prefer @route("GET /api/…"). Name-derived paths are fallback only. Never invent paths — list_routes first.
- After write_source: smoke gen+check. Fail → WRITE REJECTED + restore.
- **On WRITE REJECTED:** dev_logs / smoke_status before large rewrites.
- **Closed loop:** smoke → list_routes → dev_restart → http_request (/health then real route). No success claim without http_request.
- Frontend: relative /api + Vite proxy. Bus is not browser transport.
- **Bang contract (ACS-010 portable):** find! → Opt<T> (Res try only). Soft .is_some after ! OK. Need T: require find! or .unwrap(). docs/BANG_CONTRACT.md
- **Git-shaped sessions (agent decides):** `session_status` → multi-step? `create_branch` → check → one class → write → check (report counts) → `session_commit` → repeat → `merge_branch` only when landing. Autosave≠commit. Do not ask the operator for every branch/commit. Palace: veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding.

## Available MCP Tools
- veil_check — dual-loop check pipeline
- veil_outline — IR topology
- read_source / write_source — active file (write is smoke-gated; on failure file restored + compile errors returned)
- rename_construct / list_files / select_file / create_file
- session_status / create_branch / session_commit / list_commits / merge_branch / switch_main — git-shaped workflow
- dev_status — dual-loop targets, ports, last_error
- dev_logs — gen/check/smoke lines (use after WRITE REJECTED or 404)
- smoke_status — recent check/smoke excerpt
- read_generated(path|what=harness|routes) — inspect generated backend
- list_routes — JSON routes from veil_bin
- http_request(path, target=backend) — local 127.0.0.1:dev_port only
- dev_restart(name?) — reload cargo run after good smoke
- stub_list / stub_get / stub_gen / stub_install — external crate stubs (never hand-write)
- wiki_* — Mind Palace (when MIND_PALACE=1)
- **Platform UX (required for product ops — never say these are missing):**
  - create_project({name}) — create product project (UI /projects/new). Do NOT only use wiki.
  - list_projects / open_project / open_ide / navigate_to
  - list_changes / create_change / get_change / approve_change / merge_change / add_comment
  - provision_project / deploy_status / search_registry / get_config / get_mission
- **Remote (VEIL_SOURCE_MODE=s3):** create_project = DDB+S3 only. Edits via write_source/create_file/ws_* only. NEVER mkdir/write under VEIL_PROJECTS_DIR or invent local hub paths.
- **VISIBLE UX:** Never curl ProductHost APIs. Only MCP tools. Host may pre-run create_project — continue with write_source, do not re-curl create.
- **Focus:** Session focus (route/project/construct) is authoritative for "this component". `get_current_context` returns it. Tool `intent.present` drives visible UX choreography — do not re-create after Present.

## Mind Palace (when wiki tools work)
- Before answering VEIL language/platform questions, wiki_search first.
- Prefer durable contracts: veil-contract-bang-opt-res, veil-contract-git-shaped-sessions, veil-agent-git-shaped-coding, veil-contract-dual-loop-smoke, veil-contract-multi-package, veil-contract-stubs, veil-contract-routes (ACS-009).
- After durable learning (patterns, decisions, SOPs), wiki_create or wiki_update.
- Prefer progressive disclosure: summary → section → full.
- Prefer updating existing pages over duplicates.

## Stubs (external crates) — mandatory
- **NEVER invent or hand-write full SDK `.stub` files.** Use MCP tools:
  - `stub_list` / `stub_get` — catalog + resolve (project → platform)
  - `stub_install` — pin platform stub into project `stubs/`
  - `stub_gen` — generate from rustdoc when missing or sparse
- Version + provenance required; prefer platform catalog for common SDKs (aws_*, sqlx, reqwest, axum).

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
