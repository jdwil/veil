//! Built-in agent vertical slice (AGT-001 / AGT-005 / AGT-006).
//!
//! MVP: host-owned tool loop with a heuristic model (no vendor lock-in).
//! Tools are ports over [`SourceProvider`] + check/edit pipeline — not
//! filesystem-only hacks.
//!
//! Optional: set `VEIL_AGENT_MODEL=echo` (default) or later wire a real
//! `ModelProvider` adapter (AGT-003).

use serde::{Deserialize, Serialize};
use veil_ir::{apply_edits_with, check_solution, EditOp, LayerRegistry};

use crate::provider::SourceProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRequest {
    pub prompt: String,
    /// Optional cancel token id (MVP: ignored; reserved for AGT-001 cancel).
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
}

/// Run one agent turn against the active source (AGT-006 built-in loop).
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
                content: "Send a non-empty prompt. Try: `check`, `outline`, or `rename X to Y`."
                    .into(),
            }],
            tool_calls: vec![],
            source_changed: false,
            ok: true,
            error: None,
        };
    }

    let mut messages = vec![AgentMessage {
        role: "user".into(),
        content: prompt.to_string(),
    }];
    let mut tool_calls = Vec::new();
    let mut source_changed = false;

    let source = match provider.read_source("").await {
        Ok(s) => s,
        Err(e) => {
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls,
                source_changed: false,
                ok: false,
                error: Some(e),
            };
        }
    };
    let registry = provider.registry();

    let lower = prompt.to_lowercase();
    if lower == "check" || lower.starts_with("check ") || lower.contains("run check") {
        tool_calls.push(AgentToolCall {
            name: "run_check".into(),
            detail: "target=rust".into(),
        });
        let reply = tool_check(&source, registry);
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: reply,
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
        };
    }

    if lower == "outline" || lower.starts_with("outline") || lower.contains("show structure") {
        tool_calls.push(AgentToolCall {
            name: "get_context".into(),
            detail: "outline".into(),
        });
        let reply = tool_outline(&source, registry);
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: reply,
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
        };
    }

    // rename Old to New  /  rename Old -> New
    if let Some((from, to)) = parse_rename(prompt) {
        tool_calls.push(AgentToolCall {
            name: "read_source".into(),
            detail: "active".into(),
        });
        match tool_rename(&source, registry, &from, &to) {
            Ok((new_src, summary)) => {
                tool_calls.push(AgentToolCall {
                    name: "apply_edit".into(),
                    detail: format!("rename {} → {}", from, to),
                });
                if let Err(e) = provider.write_source("", &new_src).await {
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: false,
                        error: Some(e),
                    };
                }
                source_changed = true;
                tool_calls.push(AgentToolCall {
                    name: "run_check".into(),
                    detail: "post-edit".into(),
                });
                let check = tool_check(&new_src, registry);
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("{}\n\n{}", summary, check),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed,
                    ok: true,
                    error: None,
                };
            }
            Err(e) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("Could not rename: {}", e),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: false,
                    ok: false,
                    error: Some(e),
                };
            }
        }
    }

    // Default: try ModelProvider (AGT-003), fall back to heuristic help + context
    tool_calls.push(AgentToolCall {
        name: "read_source".into(),
        detail: format!("{} bytes", source.len()),
    });
    tool_calls.push(AgentToolCall {
        name: "get_context".into(),
        detail: "outline".into(),
    });
    let outline = tool_outline(&source, registry);
    let check = tool_check(&source, registry);

    let model_reply = crate::model::complete_with_env(crate::model::CompleteRequest {
        messages: vec![
            crate::model::ChatMessage {
                role: "system".into(),
                content: format!(
                    "You are the VEIL built-in agent. Tools available to the host: check, outline, rename. Package outline:\n{}\n\n{}",
                    outline, check
                ),
            },
            crate::model::ChatMessage {
                role: "user".into(),
                content: prompt.to_string(),
            },
        ],
        model: None,
        max_tokens: Some(512),
    })
    .await;

    let content = match model_reply {
        Ok(r) => format!(
            "[{}/{}]\n{}\n\n—\nHeuristic tools still work: `check` · `outline` · `rename A to B`",
            r.provider, r.model, r.content
        ),
        Err(e) => format!(
            "Built-in agent (heuristic). ModelProvider note: {}\n\nI can:\n\
             • `check` — dual-loop check\n\
             • `outline` — constructs\n\
             • `rename Old to New` — EditOp rename + check\n\n\
             Context:\n{}\n\n{}",
            e, outline, check
        ),
    };
    messages.push(AgentMessage {
        role: "assistant".into(),
        content,
    });
    AgentTurnResponse {
        turn_id,
        messages,
        tool_calls,
        source_changed: false,
        ok: true,
        error: None,
    }
}

fn chrono_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms}")
}

fn parse_source(source: &str, registry: &LayerRegistry) -> Result<veil_ir::Solution, String> {
    let tokens = veil_parser::lex(source);
    veil_parser::parse_with_registry(&tokens, registry.clone())
        .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
}

fn tool_check(source: &str, registry: &LayerRegistry) -> String {
    match parse_source(source, registry) {
        Ok(sol) => {
            let result = check_solution(&sol, registry);
            let errs = result.error_count();
            let warns = result.warning_count();
            let mut lines = vec![format!(
                "check: {} error(s), {} warning(s) — {}",
                errs,
                warns,
                if result.has_errors() { "FAIL" } else { "OK" }
            )];
            for d in result.diagnostics.iter().take(12) {
                lines.push(format!(
                    "  [{:?}] {}{}",
                    d.severity,
                    d.message,
                    d.node_name
                        .as_ref()
                        .map(|n| format!(" ({})", n))
                        .unwrap_or_default()
                ));
            }
            if result.diagnostics.len() > 12 {
                lines.push(format!("  … +{} more", result.diagnostics.len() - 12));
            }
            lines.join("\n")
        }
        Err(e) => format!("parse error: {}", e),
    }
}

fn tool_outline(source: &str, registry: &LayerRegistry) -> String {
    match parse_source(source, registry) {
        Ok(sol) => {
            let graph = veil_ir::build_ir_with_registry(&sol, Some(registry));
            let mut lines = vec!["outline:".to_string()];
            for n in graph.nodes.iter().filter(|n| {
                !matches!(
                    n.kind,
                    veil_ir::NodeKind::Solution
                        | veil_ir::NodeKind::Action
                        | veil_ir::NodeKind::Inputs
                        | veil_ir::NodeKind::Return
                        | veil_ir::NodeKind::Field
                )
            }) {
                let sk = n.metadata.subkind.as_deref().unwrap_or("");
                lines.push(format!(
                    "  - {:?} {} {}",
                    n.kind,
                    if sk.is_empty() { "" } else { sk },
                    n.name
                ));
            }
            lines.join("\n")
        }
        Err(e) => format!("parse error: {}", e),
    }
}

fn parse_rename(prompt: &str) -> Option<(String, String)> {
    let p = prompt.trim();
    let lower = p.to_lowercase();
    if !lower.starts_with("rename ") {
        return None;
    }
    let rest = p["rename ".len()..].trim();
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

fn tool_rename(
    source: &str,
    registry: &LayerRegistry,
    from: &str,
    to: &str,
) -> Result<(String, String), String> {
    let sol = parse_source(source, registry)?;
    let graph = veil_ir::build_ir_with_registry(&sol, Some(registry));
    let node = graph
        .nodes
        .iter()
        .find(|n| n.name == from)
        .ok_or_else(|| format!("no construct named '{}'", from))?;
    let span_start = node.span.start;
    let ops = vec![EditOp::Rename {
        span_start,
        name: to.to_string(),
    }];
    let mut sol2 = sol;
    apply_edits_with(&mut sol2, &ops, |s| {
        veil_parser::parse_expr_str(s, registry).map_err(|e| e.to_string())
    })
    .map_err(|e| e.to_string())?;
    let new_src = veil_ir::serialize_solution(&sol2);
    Ok((
        new_src,
        format!("Renamed '{}' → '{}' via EditOp::Rename.", from, to),
    ))
}
