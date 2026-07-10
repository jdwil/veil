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
}

/// Run one agent turn against the active source.
pub async fn run_turn<P: SourceProvider>(
    provider: &P,
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
        };
    }

    let mut messages = vec![AgentMessage {
        role: "user".into(),
        content: prompt.to_string(),
    }];

    let source = match provider.read_source("").await {
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
            };
        }
    };
    let registry = provider.registry().clone();
    let confirm = std::env::var("VEIL_AGENT_CONFIRM_WRITES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let cfg = crate::model::ModelConfig::from_env();

    // Prefer Rig agent loop with tools when provider supports it.
    if cfg.supports_rig_agent() {
        let ws = Workspace::new(source.clone(), registry.clone(), confirm);
        let outline = rig_tools::run_outline(&source, &registry);
        let preamble = format!(
            "You are the VEIL IDE built-in agent (Rig SDK).\n\
             Edit VEIL structure safely using tools — prefer rename_construct over inventing source.\n\
             After edits, call veil_check. Prefer veil_outline over dumping generated code.\n\
             Current package outline:\n{outline}\n"
        );
        match crate::model::prompt_with_tools(&cfg, &preamble, prompt, ws.clone()).await {
            Ok(content) => {
                let tool_calls = ws.take_log();
                let source_changed = ws.changed();
                if source_changed {
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
                        };
                    }
                }
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content,
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed,
                    ok: true,
                    error: None,
                    backend: format!("rig-{}", cfg.kind_name()),
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
    heuristic_turn(
        provider,
        turn_id,
        prompt,
        source,
        &registry,
        confirm,
        messages,
    )
    .await
}

async fn heuristic_turn<P: SourceProvider>(
    provider: &P,
    turn_id: String,
    prompt: &str,
    source: String,
    registry: &LayerRegistry,
    confirm: bool,
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
            };
        }
        tool_calls.push(AgentToolCall {
            name: "rename_construct".into(),
            detail: format!("{from} → {to}"),
        });
        match rig_tools::apply_rename(&source, registry, &from, &to) {
            Ok((new_src, summary)) => {
                if let Err(e) = provider.write_source("", &new_src).await {
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: false,
                        error: Some(e),
                        backend: "heuristic".into(),
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
             Commands: `check` · `outline` · `rename Old to New`\n\n\
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
