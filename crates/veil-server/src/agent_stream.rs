//! Streaming agent turns for the IDE (SSE / WebSocket bridge).
//!
//! Events (SSE `event:` name):
//! - `status` — `{ "message": "…" }`
//! - `chunk`  — `{ "text": "…" }`  (live model deltas as-is; offline path uses mild pacing)
//! - `tool`   — `{ "name": "…", "detail": "…" }`
//! - `done`   — full [`AgentTurnResponse`] JSON
//! - `error`  — `{ "message": "…" }`
//!
//! **Streaming rule:** ACP/Kiro already emits `agent_message_chunk` deltas. Forward
//! those immediately — do **not** re-typewriter character-by-character. Char pacing
//! floods the WS (thousands of frames), freezes the Markdown renderer, and looks
//! like “nothing then dump” to the operator.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use crate::agent::{run_turn, AgentMessage, AgentToolCall, AgentTurnRequest, AgentTurnResponse};
use crate::model::ModelConfig;
use crate::provider::SourceProvider;

/// Mild pacing between **word** batches for offline/heuristic dumps only (ms).
const WORD_MS: u64 = 12;
/// Max chars per offline typewriter batch (keeps UI responsive without char flood).
const TYPE_BATCH: usize = 48;

/// Push SSE-ready payloads (event name, JSON data string).
pub type StreamTx = mpsc::Sender<(String, String)>;

async fn emit(tx: &StreamTx, event: &str, data: serde_json::Value) {
    let _ = tx
        .send((event.to_string(), data.to_string()))
        .await;
}

/// Live model path: one WS frame per ACP delta (no artificial delay).
async fn emit_live(tx: &StreamTx, text: &str) {
    if text.is_empty() {
        return;
    }
    emit(tx, "chunk", json!({ "text": text })).await;
}

/// Offline / post-hoc path: mild word-ish batches so a full reply still “types”
/// without 8ms×N character storms (which freeze Svelte + marked).
async fn emit_typed(tx: &StreamTx, text: &str) {
    if text.is_empty() {
        return;
    }
    let mut start = 0usize;
    let chars: Vec<char> = text.chars().collect();
    while start < chars.len() {
        let end = (start + TYPE_BATCH).min(chars.len());
        // Prefer breaking on whitespace near the batch end for smoother paint.
        let mut cut = end;
        if end < chars.len() {
            if let Some(rel) = chars[start..end].iter().rposition(|c| c.is_whitespace()) {
                if rel > 0 {
                    cut = start + rel + 1;
                }
            }
        }
        let batch: String = chars[start..cut].iter().collect();
        emit(tx, "chunk", json!({ "text": batch })).await;
        start = cut;
        if start < chars.len() {
            tokio::time::sleep(Duration::from_millis(WORD_MS)).await;
        }
    }
}

/// Emit a chunk as either a tool event or **live** text (ACP path).
async fn emit_chunk_or_tool(tx: &StreamTx, chunk: &str) {
    if let Some(name) = chunk.strip_prefix("\x01TOOL:").and_then(|s| s.strip_suffix('\x01')) {
        emit(tx, "tool", json!({ "name": name, "detail": "running" })).await;
    } else {
        emit_live(tx, chunk).await;
    }
}

/// Run a turn and stream text + final response on `tx`.
pub async fn run_turn_stream<P: SourceProvider>(
    provider: Arc<P>,
    req: AgentTurnRequest,
    tx: StreamTx,
) {
    let turn_id = req
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("t-{}", chrono_id()));
    emit(
        &tx,
        "status",
        json!({ "message": "starting", "turn_id": turn_id }),
    )
    .await;

    let cfg = ModelConfig::from_env();

    // Host-side structured commands (create package, list files, platform UX …)
    // must not go through ACP streaming — `run_turn` handles them immediately.
    // Platform UX (list_prs / open_deploy / …) is included so AgentDock chips
    // always emit navigation tool events without waiting on ACP MCP discovery.
    if crate::agent::is_structured_agent_command(&req.prompt)
        || crate::agent::parse_platform_ux_intent(&req.prompt).is_some()
    {
        emit(
            &tx,
            "status",
            json!({ "message": "host tools", "turn_id": turn_id }),
        )
        .await;
        let resp = run_turn(provider, req).await;
        stream_response_typed(&tx, resp).await;
        return;
    }

    // Multi-step product work ("create project X and design…"): host runs
    // create_project / navigate_to FIRST so the SPA moves and tool chips show.
    // ACP must not curl /api/repos — that bypasses UX and navigation.
    let prefix = crate::agent::host_platform_prefix_steps(&req.prompt);
    let mut prefix_notes: Vec<String> = Vec::new();
    let mut created_slug: Option<String> = None;
    if !prefix.is_empty() {
        emit(
            &tx,
            "status",
            json!({
                "message": "host platform tools (visible UX)",
                "turn_id": turn_id,
                "steps": prefix.len(),
            }),
        )
        .await;
        for step in &prefix {
            emit(
                &tx,
                "tool",
                json!({
                    "name": step.tool,
                    "detail": json!({ "status": "running", "summary": step.summary }),
                }),
            )
            .await;
            let detail = match crate::platform_tools::dispatch(&step.tool, &step.args).await {
                Ok(s) => s,
                Err(e) => json!({
                    "ok": false,
                    "summary": step.summary,
                    "error": e,
                })
                .to_string(),
            };
            // Stream full tool result so UI shows tool_call + SPA navigation / Present
            emit(
                &tx,
                "tool",
                json!({
                    "name": step.tool,
                    "detail": detail,
                }),
            )
            .await;
            if step.tool == "create_project" {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&detail) {
                    if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
                        let slug = v
                            .get("slug")
                            .or_else(|| v.pointer("/project/slug"))
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                step.args
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .map(|s| s.to_string())
                            });
                        if let Some(s) = slug {
                            created_slug = Some(s.clone());
                            prefix_notes.push(format!(
                                "HOST already ran create_project for `{s}` (DDB+S3). UX Present will animate create form then open IDE. Do NOT re-create, curl /api/repos, or mkdir disk hub. Continue with write_source/create_file only."
                            ));
                        }
                        // Let the browser finish Present before ACP floods write_source
                        // (intent already streamed above — no deadlock).
                        if crate::focus::client_present() {
                            if let Some(iid) = v
                                .pointer("/intent/id")
                                .or_else(|| v.get("intent_id"))
                                .and_then(|x| x.as_str())
                            {
                                let _ = emit(
                                    &tx,
                                    "status",
                                    json!({
                                        "message": format!("waiting for UX Present ACK ({iid})…"),
                                        "intent_id": iid,
                                    }),
                                )
                                .await;
                                match crate::focus::wait_intent_ack(iid, 14_000).await {
                                    Ok(_) => {
                                        prefix_notes.push(
                                            "UX Present ACK received — safe to write_source.".into(),
                                        );
                                        let _ = emit(
                                            &tx,
                                            "status",
                                            json!({ "message": "UX Present complete" }),
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        prefix_notes.push(format!(
                                            "UX Present ACK wait: {e} — continuing (domain already applied)."
                                        ));
                                    }
                                }
                            }
                        }
                    } else {
                        prefix_notes.push(format!(
                            "HOST create_project failed: {detail}. Report the error; do not invent local files."
                        ));
                    }
                }
            }
        }
    }

    // ── ACP path: real token stream from Kiro ─────────────────────────────
    if cfg.supports_acp() {
        let mut req_acp = req.clone();
        if !prefix_notes.is_empty() {
            req_acp.prompt = format!(
                "{}\n\n# Host platform tools already executed (visible in UI)\n{}\n\
                 FORBIDDEN: shell curl/fetch to /api/repos, /api/projects, or raw filesystem project create.\n\
                 REQUIRED: MCP tools only (write_source, create_file, veil_check, …).",
                req_acp.prompt,
                prefix_notes.join("\n")
            );
        }
        // Scope IDE tools to the project we just created (rebind task-local hub)
        let acp_result = if let Some(ref slug) = created_slug {
            // Rematerialize + bind session so first write_source isn't "empty project"
            let _ = crate::agent_scope::prepare_project(slug, Some(provider.as_ref()));
            crate::acp::ensure_acp_project_scope(Some(slug.clone()));
            crate::provider::hub::CURRENT_PROJECT
                .scope(
                    slug.clone(),
                    stream_acp_turn(provider.clone(), req_acp, &tx, &turn_id),
                )
                .await
        } else {
            stream_acp_turn(provider.clone(), req_acp, &tx, &turn_id).await
        };
        match acp_result {
            // stream_acp_turn always emits `done` (success or soft error).
            // Never re-run the full non-stream turn — that doubles wait and
            // dumps a second blob after the operator already watched tools.
            Ok(()) => return,
            Err(e) => {
                // Hard failure before any done (e.g. spawn panic) — one error frame.
                emit(
                    &tx,
                    "status",
                    json!({ "message": format!("ACP error: {e}") }),
                )
                .await;
                emit(
                    &tx,
                    "error",
                    json!({ "message": format!("ACP agent error: {e}") }),
                )
                .await;
                let resp = AgentTurnResponse {
                    turn_id: turn_id.clone(),
                    messages: vec![AgentMessage {
                        role: "assistant".into(),
                        content: format!(
                            "ACP agent error: {e}\n\
                             Check: `kiro-cli login`, `VEIL_ACP_COMMAND`, `VEIL_ACP_ARGS`."
                        ),
                    }],
                    tool_calls: vec![],
                    source_changed: false,
                    ok: false,
                    error: Some(e),
                    backend: "acp-error".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
                emit(
                    &tx,
                    "done",
                    serde_json::to_value(&resp).unwrap_or(json!({})),
                )
                .await;
                return;
            }
        }
    }

    // ── Rig / heuristic: run full turn, then mild typewriter of the reply ──
    emit(
        &tx,
        "status",
        json!({ "message": format!("running {}", cfg.kind_name()) }),
    )
    .await;
    let resp = run_turn(provider, req).await;
    stream_response_typed(&tx, resp).await;
}

async fn stream_acp_turn<P: SourceProvider>(
    provider: Arc<P>,
    req: AgentTurnRequest,
    tx: &StreamTx,
    turn_id: &str,
) -> Result<(), String> {
    let loaded = provider.list_files().await;
    let source = provider
        .read_source("")
        .await
        .map_err(|e| e.to_string())?;
    let registry = provider.registry();
    let project_root = provider.project_root();
    let preamble_pack = crate::agent_context::assemble_preamble(
        &source,
        &registry,
        project_root.as_deref(),
    );
    let active_name = loaded
        .iter()
        .find(|f| f.active)
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "active.veil".into());
    let prompt = req.prompt.trim().to_string();
    let composed = {
        let file_map: String = loaded
            .iter()
            .map(|f| {
                let mark = if f.active { " ← active" } else { "" };
                format!("  - {} ({}){}", f.name, f.kind.as_str(), mark)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let mut c = format!(
            "{}\n\n# Project files\n{}\n\
             Use `select_file` to switch between them. Frontend UI is typically in a `*_ui.veil` package.\n\n\
             # User request\n{}\n\n# Active VEIL file: `{active_name}`\n\
             If the request involves a different file (e.g. UI changes → *_ui.veil), switch to it first.\n",
            preamble_pack.text, file_map, prompt
        );
        if crate::mind_palace_tools::enabled() {
            c.push_str(crate::mind_palace_tools::preamble_addon());
        }
        c
    };

    // Route MCP URL to project-scoped IDE tools when we have a project.
    // Respawn ACP if the project changed so Kiro loads /api/p/{project}/mcp
    // (hub /api/mcp alone cannot see dual-loop list_files / read_source / …).
    let project_name = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok();
    crate::acp::ensure_acp_project_scope(project_name.clone());

    emit(
        tx,
        "status",
        json!({ "message": "acp: thinking…", "backend": "acp-kiro" }),
    )
    .await;

    let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<String>();
    // Track live-streamed body so done payload matches what the UI already painted
    // (and so soft ACP errors still finalize cleanly without a second full turn).
    let mut streamed_text = String::new();
    let mut streamed_tools: Vec<String> = Vec::new();
    let mut join = tokio::task::spawn_blocking(move || {
        crate::acp::prompt_acp_streaming(&composed, |s| {
            let _ = chunk_tx.send(s.to_string());
        })
    });

    let turn_result: Result<crate::acp::AcpTurnResult, String> = loop {
        tokio::select! {
            chunk = chunk_rx.recv() => {
                match chunk {
                    Some(t) => {
                        if let Some(name) =
                            t.strip_prefix("\x01TOOL:").and_then(|s| s.strip_suffix('\x01'))
                        {
                            streamed_tools.push(name.to_string());
                        } else {
                            streamed_text.push_str(&t);
                        }
                        emit_chunk_or_tool(tx, &t).await;
                    }
                    None => {
                        // All chunk senders dropped — blocking task finished (or panicked).
                        break join.await.map_err(|e| e.to_string()).and_then(|r| r);
                    }
                }
            }
            res = &mut join => {
                // Drain any remaining chunks still in the queue (live, not typewriter).
                while let Ok(t) = chunk_rx.try_recv() {
                    if let Some(name) =
                        t.strip_prefix("\x01TOOL:").and_then(|s| s.strip_suffix('\x01'))
                    {
                        streamed_tools.push(name.to_string());
                    } else {
                        streamed_text.push_str(&t);
                    }
                    emit_chunk_or_tool(tx, &t).await;
                }
                break res.map_err(|e| e.to_string()).and_then(|r| r);
            }
        }
    };

    let (content_base, tool_hints, session_id, acp_ok, acp_err) = match turn_result {
        Ok(turn) => {
            // Prefer the authoritative joined text from ACP; fall back to what we streamed.
            let text = if !turn.text.is_empty() {
                turn.text
            } else {
                streamed_text.clone()
            };
            // If ACP only delivered text in the final result (no session/update
            // chunks), paint it now so the UI is not empty until done backfill.
            if streamed_text.is_empty() && !text.is_empty() {
                emit_live(tx, &text).await;
            } else if text.len() > streamed_text.len() && text.starts_with(&streamed_text) {
                let rest = &text[streamed_text.len()..];
                if !rest.is_empty() {
                    emit_live(tx, rest).await;
                }
            }
            let mut hints = turn.tool_hints;
            if hints.is_empty() {
                hints = streamed_tools.clone();
            }
            (text, hints, turn.session_id, true, None)
        }
        Err(e) => {
            tracing::warn!(error = %e, "ACP stream turn failed — finalizing with partial stream");
            emit(
                tx,
                "status",
                json!({ "message": format!("ACP ended with error: {e}") }),
            )
            .await;
            let mut text = streamed_text.clone();
            if text.is_empty() {
                text = format!(
                    "ACP agent error: {e}\n\
                     Check: `kiro-cli login`, `VEIL_ACP_COMMAND`, `VEIL_ACP_ARGS`."
                );
                emit_live(tx, &text).await;
            } else {
                let note = format!("\n\n---\nACP ended with error: {e}");
                emit_live(tx, &note).await;
                text.push_str(&note);
            }
            (text, streamed_tools.clone(), String::new(), false, Some(e))
        }
    };

    let reloaded = provider.reload_from_disk().await.unwrap_or(0);
    let source_changed = reloaded > 0;
    let mut content = content_base;
    // Reload note as a single live chunk (not char typewriter)
    if reloaded > 0 {
        let note = format!("\n\n---\nVEIL reloaded {reloaded} file(s) from disk after ACP turn.");
        emit_live(tx, &note).await;
        content.push_str(&note);
    }
    if let Some(ref w) = preamble_pack.warning {
        // warning already should show in UI banner via done payload
        let _ = w;
    }

    let mut tool_calls: Vec<AgentToolCall> = tool_hints
        .into_iter()
        .map(|n| AgentToolCall {
            name: n,
            detail: "acp".into(),
        })
        .collect();
    if tool_calls.is_empty() {
        tool_calls.push(AgentToolCall {
            name: "acp_session".into(),
            detail: if session_id.is_empty() {
                "acp".into()
            } else {
                session_id
            },
        });
    }
    // Tool events already streamed in real-time via \x01TOOL: markers.
    // Don't re-emit them here.

    let resp = AgentTurnResponse {
        turn_id: turn_id.to_string(),
        messages: vec![
            AgentMessage {
                role: "user".into(),
                content: prompt,
            },
            AgentMessage {
                role: "assistant".into(),
                content: content.clone(),
            },
        ],
        tool_calls: tool_calls.clone(),
        source_changed,
        ok: acp_ok,
        error: acp_err,
        backend: if acp_ok {
            "acp-kiro".into()
        } else {
            "acp-partial".into()
        },
        plan: None,
        context_truncated: preamble_pack.truncated,
        context_warning: preamble_pack.warning.clone(),
        context_tokens: preamble_pack.tokens_used,
        context_budget_tokens: preamble_pack.max_tokens,
        context_layers: preamble_pack.layers.clone(),
    };
    // Durable PR history: agent reply under open pull request.
    let tool_names: Vec<String> = tool_calls.iter().map(|t| t.name.clone()).collect();
    let slug = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok();
    crate::pr_writeback::writeback_agent_turn_to_pr(
        slug.as_deref(),
        &content,
        &tool_names,
        source_changed,
    )
    .await;
    emit(
        tx,
        "done",
        serde_json::to_value(&resp).unwrap_or(json!({})),
    )
    .await;
    Ok(())
}

async fn stream_response_typed(tx: &StreamTx, resp: AgentTurnResponse) {
    // Tools first so SPA navigation + tool chips fire before the essay text.
    for t in &resp.tool_calls {
        emit(
            tx,
            "tool",
            json!({ "name": t.name, "detail": t.detail }),
        )
        .await;
    }
    let text = resp
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.as_str())
        .unwrap_or("");
    emit_typed(tx, text).await;
    let tool_names: Vec<String> = resp.tool_calls.iter().map(|t| t.name.clone()).collect();
    let slug = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok();
    crate::pr_writeback::writeback_agent_turn_to_pr(
        slug.as_deref(),
        text,
        &tool_names,
        resp.source_changed,
    )
    .await;
    emit(
        tx,
        "done",
        serde_json::to_value(&resp).unwrap_or(json!({})),
    )
    .await;
}

fn chrono_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}
