//! Streaming agent turns for the IDE (SSE).
//!
//! Events (SSE `event:` name):
//! - `status` — `{ "message": "…" }`
//! - `chunk`  — `{ "text": "…" }`  (often single character for typewriter feel)
//! - `tool`   — `{ "name": "…", "detail": "…" }`
//! - `done`   — full [`AgentTurnResponse`] JSON
//! - `error`  — `{ "message": "…" }`

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc;

use crate::agent::{run_turn, AgentMessage, AgentToolCall, AgentTurnRequest, AgentTurnResponse};
use crate::model::ModelConfig;
use crate::provider::SourceProvider;

/// Delay between typewriter characters (ms). Fast typing, still readable.
const CHAR_MS: u64 = 8;

/// Push SSE-ready payloads (event name, JSON data string).
pub type StreamTx = mpsc::Sender<(String, String)>;

async fn emit(tx: &StreamTx, event: &str, data: serde_json::Value) {
    let _ = tx
        .send((event.to_string(), data.to_string()))
        .await;
}

async fn emit_typed(tx: &StreamTx, text: &str) {
    for ch in text.chars() {
        emit(tx, "chunk", json!({ "text": ch.to_string() })).await;
        tokio::time::sleep(Duration::from_millis(CHAR_MS)).await;
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

    // Host-side structured commands (create package, list files, …) must not
    // go through ACP streaming — `run_turn` handles them immediately.
    if crate::agent::is_structured_agent_command(&req.prompt) {
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

    // ── ACP path: real token stream from Kiro ─────────────────────────────
    if cfg.supports_acp() {
        match stream_acp_turn(provider.clone(), req.clone(), &tx, &turn_id).await {
            Ok(()) => return,
            Err(e) => {
                emit(
                    &tx,
                    "status",
                    json!({ "message": format!("ACP error — falling back: {e}") }),
                )
                .await;
                // fall through to non-stream path with typewriter
            }
        }
    }

    // ── Rig / heuristic: run full turn, then typewriter the reply ──────────
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
    let preamble_pack = crate::agent_context::assemble_preamble(&source, &registry);
    let active_name = loaded
        .iter()
        .find(|f| f.active)
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "active.veil".into());
    let prompt = req.prompt.trim().to_string();
    let composed = format!(
        "{}\n\n# User request\n{}\n\n# Active VEIL file: `{active_name}`\n\
         Prefer editing this file with your tools. After edits, the IDE reloads from disk.\n",
        preamble_pack.text, prompt
    );

    emit(
        tx,
        "status",
        json!({ "message": "acp: thinking…", "backend": "acp-kiro" }),
    )
    .await;

    let (chunk_tx, mut chunk_rx) = mpsc::unbounded_channel::<String>();
    let mut join = tokio::task::spawn_blocking(move || {
        crate::acp::prompt_acp_streaming(&composed, |s| {
            let _ = chunk_tx.send(s.to_string());
        })
    });

    let turn = loop {
        tokio::select! {
            chunk = chunk_rx.recv() => {
                match chunk {
                    Some(t) => emit_typed(tx, &t).await,
                    None => {
                        // All chunk senders dropped — blocking task finished (or panicked).
                        break join.await.map_err(|e| e.to_string())??;
                    }
                }
            }
            res = &mut join => {
                // Drain any remaining chunks still in the queue.
                while let Ok(t) = chunk_rx.try_recv() {
                    emit_typed(tx, &t).await;
                }
                break res.map_err(|e| e.to_string())??;
            }
        }
    };

    let reloaded = provider.reload_from_disk().await.unwrap_or(0);
    let source_changed = reloaded > 0;
    let mut content = turn.text.clone();
    // If streaming already painted the body, only append reload note as extra chunks
    if reloaded > 0 {
        let note = format!("\n\n---\nVEIL reloaded {reloaded} file(s) from disk after ACP turn.");
        emit_typed(tx, &note).await;
        content.push_str(&note);
    }
    if let Some(ref w) = preamble_pack.warning {
        // warning already should show in UI banner via done payload
        let _ = w;
    }

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
    for t in &tool_calls {
        emit(
            tx,
            "tool",
            json!({ "name": t.name, "detail": t.detail }),
        )
        .await;
    }

    let resp = AgentTurnResponse {
        turn_id: turn_id.to_string(),
        messages: vec![
            AgentMessage {
                role: "user".into(),
                content: prompt,
            },
            AgentMessage {
                role: "assistant".into(),
                content,
            },
        ],
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
    emit(
        tx,
        "done",
        serde_json::to_value(&resp).unwrap_or(json!({})),
    )
    .await;
    Ok(())
}

async fn stream_response_typed(tx: &StreamTx, resp: AgentTurnResponse) {
    let text = resp
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.as_str())
        .unwrap_or("");
    emit_typed(tx, text).await;
    for t in &resp.tool_calls {
        emit(
            tx,
            "tool",
            json!({ "name": t.name, "detail": t.detail }),
        )
        .await;
    }
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
