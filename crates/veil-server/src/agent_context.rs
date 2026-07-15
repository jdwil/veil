//! Deterministic agent context pack for Rig preambles (Tier 0 + Tier 1).
//!
//! Not vector RAG: always inject teaching material for the **active file's**
//! loaded layers. Truncation is a first-class failure signal — a truncated
//! curriculum makes small models nearly useless.

use veil_ir::layer::{palette_from_registry, LayerRegistry};
use veil_ir::{build_ir_with_registry, check_solution, build_context_pack, ContextQuery};

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

## Local HTTP harness (dual-loop backend) — ACS-002 mandatory
- Packages with context modules get crates/veil_bin REST harness even without @main.
- Prefer @route("GET /api/…") on svc/handlers. Name-derived List/Get/Create paths are fallback only — never invent paths; call list_routes.
- After write_source: host runs gen + cargo check (smoke). Failure → WRITE REJECTED + file restored.
- **On WRITE REJECTED:** call dev_logs / smoke_status before rewriting the whole file.
- **Closed loop after HTTP/backend edits:** smoke → list_routes (or read_generated what=routes) → dev_restart (or auto-restart) → http_request target=backend path=/health then the real route. Do not claim success without http_request.
- Frontend: relative /api + Vite @proxy. Bus is server-side only.
- **Bang / Opt / Res (BANG_CONTRACT):** `wt = repo.find!(id)` yields T after dual-loop unwrap. NEVER .unwrap() / .is_some() / .is_none() on that result. See docs/BANG_CONTRACT.md.

## Tools
- veil_check — dual-loop diagnostics (structured JSON: code + span)
- veil_outline — IR topology
- read_source — active .veil text (truncated)
- rename_construct — structured rename
- write_source — full-file write (smoke-gated)
- dev_status / dev_logs / smoke_status — dual-loop state and gen/check logs
- read_generated / list_routes — inspect generated harness routes
- http_request — probe 127.0.0.1:dev_port only
- dev_restart — reload cargo run after successful smoke
- wiki_* — Mind Palace (when MIND_PALACE=1)
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

## Local HTTP harness (dual-loop backend) — ACS-002 mandatory
- Context modules → veil_bin REST harness; @main optional for local HTTP.
- Prefer @route("GET /api/…"). Name-derived paths are fallback only. Never invent paths — list_routes first.
- After write_source: smoke gen+check. Fail → WRITE REJECTED + restore.
- **On WRITE REJECTED:** dev_logs / smoke_status before large rewrites.
- **Closed loop:** smoke → list_routes → dev_restart → http_request (/health then real route). No success claim without http_request.
- Frontend: relative /api + Vite proxy. Bus is not browser transport.
- **Bang contract:** find! → T (try + NotFound). NEVER .unwrap()/.is_some() after !. docs/BANG_CONTRACT.md## Available MCP Tools
- veil_check — dual-loop check pipeline
- veil_outline — IR topology
- read_source / write_source — active file (write is smoke-gated; on failure file restored + compile errors returned)
- rename_construct / list_files / select_file / create_file
- dev_status — dual-loop targets, ports, last_error
- dev_logs — gen/check/smoke lines (use after WRITE REJECTED or 404)
- smoke_status — recent check/smoke excerpt
- read_generated(path|what=harness|routes) — inspect generated backend
- list_routes — JSON routes from veil_bin
- http_request(path, target=backend) — local 127.0.0.1:dev_port only
- dev_restart(name?) — reload cargo run after good smoke
- wiki_* — Mind Palace (when MIND_PALACE=1)

## Mind Palace (when wiki tools work)
- Before answering VEIL language/platform questions, wiki_search first.
- After durable learning (patterns, decisions, SOPs), wiki_create or wiki_update.
- Prefer progressive disclosure: summary → section → full.
- Prefer updating existing pages over duplicates.

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
pub fn assemble_preamble(source: &str, registry: &LayerRegistry) -> AgentPreamble {
    let is_acp = crate::acp::acp_enabled();
    let tier0_text = if is_acp { TIER0_ACP } else { TIER0 };
    assemble_preamble_inner(source, registry, tier0_text)
}

fn assemble_preamble_inner(source: &str, registry: &LayerRegistry, tier0_text: &str) -> AgentPreamble {
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
        let p = assemble_preamble(src, &reg);
        assert!(p.text.contains("Tier 0"));
        assert!(p.tokens_used > 0);
        // Builtin-only package: no layer prompts is OK and not truncation
        assert!(!p.truncated || p.warning.is_some());
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
