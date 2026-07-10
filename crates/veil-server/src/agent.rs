//! Built-in agent vertical slice (AGT-001 / AGT-005 / AGT-006).
//!
//! **Agentic execution uses the [Rig](https://rig.rs) SDK** when
//! `VEIL_MODEL_PROVIDER` is `openai` or `ollama`. Tools are typed Rig
//! [`Tool`](rig_core::tool::Tool)s over the VEIL check/edit pipeline.
//!
//! Without a model provider, a small heuristic path remains for offline use
//! (`check` / `outline` / `rename`).

use serde::{Deserialize, Serialize};
use veil_ir::LayerRegistry;

use crate::provider::SourceProvider;
use crate::rig_tools::{self, Workspace};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRequest {
    pub prompt: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    /// AGT-014: propose edits without applying (also `VEIL_AGENT_PLAN_ONLY=1`).
    #[serde(default)]
    pub plan_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnResponse {
    pub turn_id: String,
    pub messages: Vec<AgentMessage>,
    pub tool_calls: Vec<AgentToolCall>,
    pub source_changed: bool,
    pub ok: bool,
    pub error: Option<String>,
    /// Which backend handled the turn (`rig-openai`, `rig-ollama`, `heuristic`).
    #[serde(default)]
    pub backend: String,
    /// AGT-014: when plan_only, human-readable planned ops (not applied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// True when Tier 0/1 teaching context was truncated to fit the budget.
    #[serde(default)]
    pub context_truncated: bool,
    /// Loud warning when truncated (also mirrored into assistant text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_warning: Option<String>,
    /// Approx tokens in the assembled preamble.
    #[serde(default)]
    pub context_tokens: usize,
    /// Preamble budget (0 = unlimited).
    #[serde(default)]
    pub context_budget_tokens: usize,
    /// Loaded layers for this turn (active file).
    #[serde(default)]
    pub context_layers: Vec<String>,
}

impl AgentTurnResponse {
    fn with_context(mut self, pre: &crate::agent_context::AgentPreamble) -> Self {
        self.context_truncated = pre.truncated;
        self.context_warning = pre.warning.clone();
        self.context_tokens = pre.tokens_used;
        self.context_budget_tokens = pre.max_tokens;
        self.context_layers = pre.layers.clone();
        self
    }
}

/// Run one agent turn against the active source.
pub async fn run_turn<P: SourceProvider>(
    provider: std::sync::Arc<P>,
    req: AgentTurnRequest,
) -> AgentTurnResponse {
    let turn_id = req
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("t-{}", chrono_like_id()));
    let prompt = req.prompt.trim();
    if prompt.is_empty() {
        return AgentTurnResponse {
            turn_id,
            messages: vec![AgentMessage {
                role: "assistant".into(),
                content: "Send a non-empty prompt. With Rig (openai/ollama): free-form + tools. Offline: `check`, `outline`, `rename X to Y`.".into(),
            }],
            tool_calls: vec![],
            source_changed: false,
            ok: true,
            error: None,
            backend: "none".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    let mut messages = vec![AgentMessage {
        role: "user".into(),
        content: prompt.to_string(),
    }];

    let loaded = provider.as_ref().list_files().await;
    let allowlist = crate::safety::allowlist_from_env(&loaded);

    let source = match provider.as_ref().read_source("").await {
        Ok(s) => s,
        Err(e) => {
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls: vec![],
                source_changed: false,
                ok: false,
                error: Some(e),
                backend: "error".into(),
                plan: None,
                context_truncated: false,
                context_warning: None,
                context_tokens: 0,
                context_budget_tokens: 0,
                context_layers: vec![],
            };
        }
    };
    let registry = provider.as_ref().registry();
    let confirm = std::env::var("VEIL_AGENT_CONFIRM_WRITES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let plan_only = req.plan_only
        || std::env::var("VEIL_AGENT_PLAN_ONLY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    // Tier 0+1 teaching pack for active file layers (deterministic, not vector RAG).
    let preamble_pack = crate::agent_context::assemble_preamble(&source, &registry);

    let cfg = crate::model::ModelConfig::from_env();

    // ── ACP external agent (Kiro, etc.) ───────────────────────────────────
    if cfg.supports_acp() {
        let active_name = loaded
            .iter()
            .find(|f| f.active)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "active.veil".into());
        let composed = format!(
            "{}\n\n# User request\n{}\n\n# Active VEIL file: `{active_name}`\n\
             Prefer editing this file with your tools. After edits, the IDE reloads from disk.\n",
            preamble_pack.text, prompt
        );
        // ACP is sync (stdio) — run on blocking pool so we don't stall the runtime.
        let acp_result = tokio::task::spawn_blocking(move || crate::acp::prompt_acp(&composed))
            .await
            .map_err(|e| e.to_string());
        match acp_result {
            Ok(Ok(turn)) => {
                // External agent may have written workspace files — reload cache.
                let reloaded = provider.as_ref().reload_from_disk().await.unwrap_or(0);
                let source_changed = reloaded > 0;
                let mut content = turn.text;
                if reloaded > 0 {
                    content.push_str(&format!(
                        "\n\n---\nVEIL reloaded {reloaded} file(s) from disk after ACP turn."
                    ));
                }
                if let Some(ref w) = preamble_pack.warning {
                    content = format!("{w}\n\n{content}");
                }
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content,
                });
                let mut tool_calls: Vec<AgentToolCall> = turn
                    .tool_hints
                    .into_iter()
                    .map(|n| AgentToolCall {
                        name: n,
                        detail: "acp".into(),
                    })
                    .collect();
                if tool_calls.is_empty() {
                    tool_calls.push(AgentToolCall {
                        name: "acp_session".into(),
                        detail: turn.session_id.clone(),
                    });
                }
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed,
                    ok: true,
                    error: None,
                    backend: "acp-kiro".into(),
                    plan: None,
                    context_truncated: preamble_pack.truncated,
                    context_warning: preamble_pack.warning.clone(),
                    context_tokens: preamble_pack.tokens_used,
                    context_budget_tokens: preamble_pack.max_tokens,
                    context_layers: preamble_pack.layers.clone(),
                };
            }
            Ok(Err(e)) | Err(e) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!(
                        "ACP agent error: {e}\n\
                         Falling back to offline heuristic tools.\n\
                         Check: `kiro-cli login`, `VEIL_ACP_COMMAND`, `VEIL_ACP_ARGS`."
                    ),
                });
                // fall through to heuristic
            }
        }
    }

    // Prefer Rig agent loop with tools when provider supports it.
    if cfg.supports_rig_agent() {
        // Truncated curriculum → refuse model turn (unless ALLOW_TRUNCATED).
        if preamble_pack.truncated && crate::agent_context::refuse_on_truncation() {
            let warn = preamble_pack
                .warning
                .clone()
                .unwrap_or_else(|| "Agent context truncated.".into());
            messages.push(AgentMessage {
                role: "assistant".into(),
                content: format!(
                    "{warn}\n\
                     --- \n\
                     Model turn **skipped** (VEIL_AGENT_ALLOW_TRUNCATED not set).\n\
                     Offline tools still available: prompt `check`, `outline`, or `rename A to B`.\n\
                     Or raise budget only if the model context window can hold it:\n\
                       VEIL_AGENT_PREAMBLE_MAX_TOKENS=12000 make serve\n"
                ),
            });
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls: vec![AgentToolCall {
                    name: "context_guard".into(),
                    detail: "truncated — model refused".into(),
                }],
                source_changed: false,
                ok: false,
                error: Some("agent context truncated — switch model/ACP or raise budget".into()),
                backend: format!("rig-{}-refused", cfg.kind_name()),
                plan: None,
                context_truncated: true,
                context_warning: Some(warn),
                context_tokens: preamble_pack.tokens_used,
                context_budget_tokens: preamble_pack.max_tokens,
                context_layers: preamble_pack.layers.clone(),
            };
        }

        let mut preamble = preamble_pack.text.clone();
        if preamble_pack.truncated {
            // ALLOW_TRUNCATED path — still scream in the system prompt
            if let Some(ref w) = preamble_pack.warning {
                preamble = format!(
                    "{w}\n\n# WARNING: continuing with truncated context (VEIL_AGENT_ALLOW_TRUNCATED=1)\n\n{preamble}"
                );
            }
        }

        // Mid-turn live flush: each tool write hits SourceProvider immediately
        // (SSE revision events fire from write_source → IDE badge updates).
        let writer: Option<crate::rig_tools::LiveWriter> = if plan_only {
            None
        } else {
            let p = provider.clone();
            let allow = allowlist.clone();
            let files = loaded.clone();
            Some(std::sync::Arc::new(move |src: String| {
                let p = p.clone();
                let allow = allow.clone();
                let files = files.clone();
                Box::pin(async move {
                    crate::safety::check_write_allowed("", &allow, &files)?;
                    p.write_source("", &src).await
                })
            }))
        };

        let mut ws = Workspace::new(source.clone(), registry.clone(), confirm);
        if let Some(w) = writer {
            ws = ws.with_live_writer(w);
        }

        match crate::model::prompt_with_tools(&cfg, &preamble, prompt, ws.clone()).await {
            Ok(content) => {
                let tool_calls = ws.take_log();
                let wants_write = ws.changed();
                if wants_write && plan_only {
                    let plan = Some(format!(
                        "plan_only: would write {} bytes after tools {:?}",
                        ws.source_snapshot().len(),
                        tool_calls.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()
                    ));
                    let mut content = content;
                    if let Some(ref w) = preamble_pack.warning {
                        content = format!("{w}\n\n{content}");
                    }
                    messages.push(AgentMessage {
                        role: "assistant".into(),
                        content: format!(
                            "{content}\n\n[plan_only] No write applied. Re-run without VEIL_AGENT_PLAN_ONLY / plan_only to apply."
                        ),
                    });
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: true,
                        error: None,
                        backend: format!("rig-{}", cfg.kind_name()),
                        plan,
                        context_truncated: preamble_pack.truncated,
                        context_warning: preamble_pack.warning.clone(),
                        context_tokens: preamble_pack.tokens_used,
                        context_budget_tokens: preamble_pack.max_tokens,
                        context_layers: preamble_pack.layers.clone(),
                    };
                }
                // Ensure final snapshot is on disk (covers tools that skipped live write)
                if wants_write && !plan_only {
                    if let Err(e) = crate::safety::check_write_allowed("", &allowlist, &loaded) {
                        return AgentTurnResponse {
                            turn_id,
                            messages,
                            tool_calls,
                            source_changed: false,
                            ok: false,
                            error: Some(e),
                            backend: format!("rig-{}", cfg.kind_name()),
                            plan: None,
                            context_truncated: preamble_pack.truncated,
                            context_warning: preamble_pack.warning.clone(),
                            context_tokens: preamble_pack.tokens_used,
                            context_budget_tokens: preamble_pack.max_tokens,
                            context_layers: preamble_pack.layers.clone(),
                        };
                    }
                    let new_src = ws.source_snapshot();
                    if let Err(e) = provider.write_source("", &new_src).await {
                        return AgentTurnResponse {
                            turn_id,
                            messages,
                            tool_calls,
                            source_changed: false,
                            ok: false,
                            error: Some(e),
                            backend: format!("rig-{}", cfg.kind_name()),
                            plan: None,
                            context_truncated: preamble_pack.truncated,
                            context_warning: preamble_pack.warning.clone(),
                            context_tokens: preamble_pack.tokens_used,
                            context_budget_tokens: preamble_pack.max_tokens,
                            context_layers: preamble_pack.layers.clone(),
                        };
                    }
                }
                let content = if let Some(ref w) = preamble_pack.warning {
                    format!("{w}\n\n{content}")
                } else {
                    content
                };
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content,
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: wants_write && !plan_only,
                    ok: !preamble_pack.truncated,
                    error: if preamble_pack.truncated {
                        Some("agent context was truncated (ran with VEIL_AGENT_ALLOW_TRUNCATED=1)".into())
                    } else {
                        None
                    },
                    backend: format!("rig-{}", cfg.kind_name()),
                    plan: None,
                    context_truncated: preamble_pack.truncated,
                    context_warning: preamble_pack.warning.clone(),
                    context_tokens: preamble_pack.tokens_used,
                    context_budget_tokens: preamble_pack.max_tokens,
                    context_layers: preamble_pack.layers.clone(),
                };
            }
            Err(e) => {
                // Fall through to heuristic with error note
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!(
                        "Rig agent error ({provider}): {e}\nFalling back to heuristic tools.",
                        provider = cfg.kind_name()
                    ),
                });
            }
        }
    }

    // Heuristic offline path (no Rig model) — same tools, host-dispatched.
    let mut resp = heuristic_turn(
        provider.as_ref(),
        turn_id,
        prompt,
        source,
        &registry,
        confirm,
        plan_only,
        allowlist,
        loaded,
        messages,
    )
    .await;
    resp = resp.with_context(&preamble_pack);
    resp
}

async fn heuristic_turn<P: SourceProvider>(
    provider: &P,
    turn_id: String,
    prompt: &str,
    source: String,
    registry: &LayerRegistry,
    confirm: bool,
    plan_only: bool,
    allowlist: Vec<String>,
    loaded: Vec<crate::provider::FileInfo>,
    mut messages: Vec<AgentMessage>,
) -> AgentTurnResponse {
    let mut tool_calls = Vec::new();
    let lower = prompt.to_lowercase();

    if lower == "check" || lower.starts_with("check ") || lower.contains("run check") {
        tool_calls.push(AgentToolCall {
            name: "veil_check".into(),
            detail: "target=rust".into(),
        });
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: rig_tools::run_check(&source, registry),
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
            backend: "heuristic".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    if lower == "outline" || lower.starts_with("outline") || lower.contains("show structure") {
        tool_calls.push(AgentToolCall {
            name: "veil_outline".into(),
            detail: "outline".into(),
        });
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: rig_tools::run_outline(&source, registry),
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
            backend: "heuristic".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    if let Some((from, to)) = parse_rename(prompt) {
        if confirm && !lower.contains("confirm") {
            messages.push(AgentMessage {
                role: "assistant".into(),
                content: format!(
                    "Permission: write would rename '{from}' → '{to}'. Re-send as `confirm rename {from} to {to}` (VEIL_AGENT_CONFIRM_WRITES)."
                ),
            });
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls: vec![AgentToolCall {
                    name: "permission_check".into(),
                    detail: "confirm required".into(),
                }],
                source_changed: false,
                ok: true,
                error: None,
                backend: "heuristic".into(),
                plan: None,
                context_truncated: false,
                context_warning: None,
                context_tokens: 0,
                context_budget_tokens: 0,
                context_layers: vec![],
            };
        }
        tool_calls.push(AgentToolCall {
            name: "rename_construct".into(),
            detail: format!("{from} → {to}"),
        });
        match rig_tools::apply_rename(&source, registry, &from, &to) {
            Ok((new_src, summary)) => {
                if plan_only {
                    let plan = format!("RenameConstruct {from} → {to}");
                    messages.push(AgentMessage {
                        role: "assistant".into(),
                        content: format!(
                            "[plan_only] Would apply: {plan}\n{summary}\n\nRe-run without plan_only / VEIL_AGENT_PLAN_ONLY to apply."
                        ),
                    });
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: true,
                        error: None,
                        backend: "heuristic".into(),
                        plan: Some(plan),
                        context_truncated: false,
                        context_warning: None,
                        context_tokens: 0,
                        context_budget_tokens: 0,
                        context_layers: vec![],
                    };
                }
                if let Err(e) = crate::safety::check_write_allowed("", &allowlist, &loaded) {
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: false,
                        error: Some(e),
                        backend: "heuristic".into(),
                        plan: None,
                        context_truncated: false,
                        context_warning: None,
                        context_tokens: 0,
                        context_budget_tokens: 0,
                        context_layers: vec![],
                    };
                }
                if let Err(e) = provider.write_source("", &new_src).await {
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: false,
                        error: Some(e),
                        backend: "heuristic".into(),
                        plan: None,
                        context_truncated: false,
                        context_warning: None,
                        context_tokens: 0,
                        context_budget_tokens: 0,
                        context_layers: vec![],
                    };
                }
                tool_calls.push(AgentToolCall {
                    name: "veil_check".into(),
                    detail: "post-edit".into(),
                });
                let check = rig_tools::run_check(&new_src, registry);
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("{summary}\n\n{check}"),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: true,
                    ok: true,
                    error: None,
                    backend: "heuristic".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
            }
            Err(e) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("Could not rename: {e}"),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: false,
                    ok: false,
                    error: Some(e),
                    backend: "heuristic".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
            }
        }
    }

    // Default help
    let outline = rig_tools::run_outline(&source, registry);
    let check = rig_tools::run_check(&source, registry);
    messages.push(AgentMessage {
        role: "assistant".into(),
        content: format!(
            "Offline heuristic agent (set VEIL_MODEL_PROVIDER=openai|ollama for Rig tools).\n\
             Commands: `check` · `outline` · `rename Old to New`\n\
             Safety: VEIL_AGENT_ALLOWLIST · VEIL_AGENT_PLAN_ONLY · VEIL_AGENT_CONFIRM_WRITES\n\n\
             Context:\n{outline}\n\n{check}"
        ),
    });
    AgentTurnResponse {
        turn_id,
        messages,
        tool_calls: vec![
            AgentToolCall {
                name: "veil_outline".into(),
                detail: "context".into(),
            },
            AgentToolCall {
                name: "veil_check".into(),
                detail: "context".into(),
            },
        ],
        source_changed: false,
        ok: true,
        error: None,
        backend: "heuristic".into(),
        plan: None,
        context_truncated: false,
        context_warning: None,
        context_tokens: 0,
        context_budget_tokens: 0,
        context_layers: vec![],
    }
}

fn chrono_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn parse_rename(prompt: &str) -> Option<(String, String)> {
    let p = prompt.trim();
    let lower = p.to_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("confirm rename ") {
        // keep original casing from prompt after "confirm rename "
        &p[prompt.len() - r.len()..]
    } else if lower.starts_with("rename ") {
        &p["rename ".len()..]
    } else {
        return None;
    };
    let rest = rest.trim();
    if let Some((a, b)) = rest.split_once(" to ") {
        let from = a.trim().to_string();
        let to = b.trim().to_string();
        if !from.is_empty() && !to.is_empty() {
            return Some((from, to));
        }
    }
    if let Some((a, b)) = rest.split_once(" -> ") {
        let from = a.trim().to_string();
        let to = b.trim().to_string();
        if !from.is_empty() && !to.is_empty() {
            return Some((from, to));
        }
    }
    None
}
