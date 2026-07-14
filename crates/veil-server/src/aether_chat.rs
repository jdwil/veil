//! AetherUI-compatible WebSocket chat bridge.
//!
//! Protocol: [aether-ui streaming-protocol] — JSON frames
//! `{ "event": "<type>", "data": { ... } }` over WebSocket.
//!
//! First client message is a ChatRequest; subsequent control messages
//! may include `{ "type": "abort" }`. Reuses [`crate::agent_stream::run_turn_stream`]
//! and maps VEIL SSE events → Aether events.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use crate::agent::{AgentTurnRequest, AgentTurnResponse};
use crate::agent_stream::run_turn_stream;
use crate::provider::SourceProvider;

/// Aether ChatRequest (subset we need).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatRequest {
    messages: Vec<ChatMsg>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMsg {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ControlMsg {
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// WS upgrade handler for Aether clients.
pub async fn ws_aether_chat<P: SourceProvider + 'static>(
    ws: WebSocketUpgrade,
    State(provider): State<Arc<P>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, provider))
}

async fn handle_socket<P: SourceProvider + 'static>(socket: WebSocket, provider: Arc<P>) {
    let (mut sender, mut receiver) = socket.split();

    // First message must be ChatRequest
    let first = match receiver.next().await {
        Some(Ok(Message::Text(t))) => t,
        Some(Ok(Message::Close(_))) | None => return,
        Some(Ok(_)) => {
            let _ = send_event(
                &mut sender,
                "error",
                json!({ "message": "expected JSON ChatRequest text frame" }),
            )
            .await;
            return;
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "aether ws read error");
            return;
        }
    };

    let req: ChatRequest = match serde_json::from_str(&first) {
        Ok(r) => r,
        Err(e) => {
            let _ = send_event(
                &mut sender,
                "error",
                json!({ "message": format!("invalid ChatRequest: {e}") }),
            )
            .await;
            return;
        }
    };

    let prompt = extract_prompt(&req);
    if prompt.is_empty() {
        let _ = send_event(
            &mut sender,
            "error",
            json!({ "message": "no user message in ChatRequest" }),
        )
        .await;
        return;
    }

    let message_id = format!("msg_{}", short_id());
    let model = req.model.unwrap_or_else(|| "veil-agent".into());
    let provider_name = req.provider.unwrap_or_else(|| "veil".into());

    // message_start
    if send_event(
        &mut sender,
        "message_start",
        json!({
            "messageId": message_id,
            "role": "assistant",
            "model": model,
            "provider": provider_name,
        }),
    )
    .await
    .is_err()
    {
        return;
    }

    // Bridge: run_turn_stream → mpsc → Aether events
    let (tx, mut rx) = mpsc::channel::<(String, String)>(64);
    let turn_req = AgentTurnRequest {
        prompt,
        turn_id: Some(message_id.clone()),
        plan_only: false,
    };
    let provider_run = provider.clone();
    let turn_handle = tokio::spawn(async move {
        run_turn_stream(provider_run, turn_req, tx).await;
    });

    // Abort listener (best-effort)
    let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let abort_r = abort.clone();
    let abort_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(t) = msg {
                if let Ok(c) = serde_json::from_str::<ControlMsg>(&t) {
                    if c.kind.as_deref() == Some("abort") {
                        abort_r.store(true, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
    });

    let mut full_text = String::new();
    let mut tools: Vec<serde_json::Value> = Vec::new();
    let mut done_payload: Option<AgentTurnResponse> = None;

    while let Some((event, data_str)) = rx.recv().await {
        if abort.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        match event.as_str() {
            "status" => {
                // Optional: surface as thinking or ignore
            }
            "chunk" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
                    if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                        full_text.push_str(text);
                        if send_event(
                            &mut sender,
                            "content_delta",
                            json!({
                                "messageId": message_id,
                                "delta": text,
                            }),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            "tool" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
                    let call_id = format!("call_{}", short_id());
                    let name = v
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("tool")
                        .to_string();
                    let detail = v
                        .get("detail")
                        .cloned()
                        .unwrap_or(json!({}));
                    let _ = send_event(
                        &mut sender,
                        "tool_call_start",
                        json!({
                            "messageId": message_id,
                            "callId": call_id,
                            "name": name,
                        }),
                    )
                    .await;
                    let args = json!({ "detail": detail }).to_string();
                    let _ = send_event(
                        &mut sender,
                        "tool_call_stop",
                        json!({
                            "messageId": message_id,
                            "callId": call_id,
                            "arguments": args,
                        }),
                    )
                    .await;
                    let _ = send_event(
                        &mut sender,
                        "tool_result",
                        json!({
                            "messageId": message_id,
                            "callId": call_id,
                            "name": name,
                            "output": detail,
                            "isError": false,
                        }),
                    )
                    .await;
                    tools.push(json!({ "name": name, "detail": detail }));
                }
            }
            "done" => {
                if let Ok(resp) = serde_json::from_str::<AgentTurnResponse>(&data_str) {
                    done_payload = Some(resp);
                }
            }
            "error" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_str) {
                    let msg = v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("agent error");
                    let _ = send_event(
                        &mut sender,
                        "error",
                        json!({ "message": msg, "messageId": message_id }),
                    )
                    .await;
                }
            }
            _ => {}
        }
    }

    let _ = turn_handle.await;
    abort_task.abort();

    let _ = send_event(
        &mut sender,
        "content_stop",
        json!({ "messageId": message_id }),
    )
    .await;

    // Prefer full reply text from done payload if typewriter missed it
    if let Some(ref resp) = done_payload {
        if full_text.is_empty() {
            if let Some(last) = resp.messages.iter().rev().find(|m| m.role == "assistant") {
                if !last.content.is_empty() {
                    let _ = send_event(
                        &mut sender,
                        "content_delta",
                        json!({
                            "messageId": message_id,
                            "delta": last.content,
                        }),
                    )
                    .await;
                    full_text = last.content.clone();
                }
            }
        }
    }

    let source_changed = done_payload
        .as_ref()
        .map(|r| r.source_changed)
        .unwrap_or(false);
    let context_warning = done_payload
        .as_ref()
        .and_then(|r| r.context_warning.clone());
    let backend = done_payload
        .as_ref()
        .map(|r| r.backend.clone())
        .unwrap_or_else(|| "veil".into());

    let _ = send_event(
        &mut sender,
        "done",
        json!({
            "messageId": message_id,
            "sourceChanged": source_changed,
            "contextWarning": context_warning,
            "backend": backend,
            "tools": tools,
        }),
    )
    .await;
}

fn extract_prompt(req: &ChatRequest) -> String {
    // Prefer last user message; append earlier turns as light context if few.
    let users: Vec<&str> = req
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect();
    if let Some(last) = users.last() {
        return (*last).to_string();
    }
    req.messages
        .last()
        .map(|m| m.content.clone())
        .unwrap_or_default()
}

async fn send_event(
    sender: &mut (impl SinkExt<Message> + Unpin),
    event: &str,
    data: serde_json::Value,
) -> Result<(), ()> {
    let frame = json!({ "event": event, "data": data });
    sender
        .send(Message::Text(frame.to_string().into()))
        .await
        .map_err(|_| ())
}

fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{n:x}")
}
