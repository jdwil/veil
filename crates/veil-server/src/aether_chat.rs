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
use crate::provider::hub::CURRENT_PROJECT;
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
    /// Active project slug/name for hub agent → scopes dual-loop IDE tools.
    /// Runtime UI sends this when on `/projects/{id}` or IDE embed.
    #[serde(default)]
    project: Option<String>,
    /// Durable coding/agent session id (DDB SESSION#…). JSON: `sessionId`.
    #[serde(default)]
    session_id: Option<String>,
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

    let mut prompt = extract_prompt(&req);
    if prompt.is_empty() {
        let _ = send_event(
            &mut sender,
            "error",
            json!({ "message": "no user message in ChatRequest" }),
        )
        .await;
        return;
    }

    // Runtime UI injects platform system prompt (tools, current page). Fold it
    // into the user turn so ACP / Rig see navigation instructions.
    if let Some(sys) = req.system_prompt.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        prompt = format!(
            "# Runtime agent instructions\n{sys}\n\n# User request\n{prompt}"
        );
    }

    // Scope dual-loop IDE tools (list_files, read_source, …) to the UI project.
    // Prefer explicit ChatRequest.project; fall back to system-prompt context lines.
    let project_scope = resolve_chat_project(&req);
    let coding_session = req.session_id.clone().filter(|s| !s.is_empty());

    // Ensure durable session when project known
    let coding_session = if coding_session.is_none() {
        if let Some(ref slug) = project_scope {
            if crate::session::sessions_enabled() {
                crate::session::SessionManager::global()
                    .get_or_create_default(slug)
                    .ok()
                    .map(|h| h.session_id())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        coding_session
    };

    // Persist user turn
    if let Some(ref sid) = coding_session {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .to_string();
        let _ = crate::session::append_turn(
            sid,
            &crate::session::SessionTurn {
                turn_id: format!("u_{}", short_id()),
                role: "user".into(),
                content: extract_prompt(&req),
                tool_calls: vec![],
                project: project_scope.clone(),
                active_file: None,
                ts,
                backend: None,
            },
        );
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
            "project": project_scope,
            "sessionId": coding_session,
        }),
    )
    .await
    .is_err()
    {
        return;
    }

    // Bridge: run_turn_stream → mpsc → Aether events
    let (tx, mut rx) = mpsc::channel::<(String, String)>(64);
    let prompt_for_log = prompt.clone();
    let turn_req = AgentTurnRequest {
        prompt,
        turn_id: Some(message_id.clone()),
        plan_only: false,
    };
    let provider_run = provider.clone();
    // Project scope from request (or middleware) — task-locals do not inherit across spawn.
    let project_scope_spawn = project_scope
        .clone()
        .or_else(|| CURRENT_PROJECT.try_with(|n| n.clone()).ok());
    let turn_handle = tokio::spawn(async move {
        let fut = run_turn_stream(provider_run, turn_req, tx);
        if let Some(name) = project_scope_spawn {
            CURRENT_PROJECT.scope(name, fut).await;
        } else {
            fut.await;
        }
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
                    // Omnipresent agent: UX tools must drive SPA navigation for the user.
                    if let Some(nav) = navigation_for_platform_tool(&name, &detail) {
                        let _ = send_event(&mut sender, "navigation", nav).await;
                    }
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

    // If abort was signaled, kill the ACP agent process so the turn actually stops.
    // Otherwise await the turn normally (it already finished naturally).
    if abort.load(std::sync::atomic::Ordering::SeqCst) {
        // Cancel the ACP child process — next prompt will respawn.
        if crate::acp::acp_enabled() {
            tokio::task::spawn_blocking(crate::acp::cancel_acp)
                .await
                .ok();
        }
        turn_handle.abort();
    } else {
        let _ = turn_handle.await;
    }
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
        // Surface hard failures that left no assistant text (e.g. missing project
        // used to yield empty UI with backend:error only).
        if full_text.is_empty() {
            if let Some(err) = resp.error.as_ref().filter(|e| !e.is_empty()) {
                let _ = send_event(
                    &mut sender,
                    "error",
                    json!({ "message": err, "messageId": message_id }),
                )
                .await;
                let _ = send_event(
                    &mut sender,
                    "content_delta",
                    json!({
                        "messageId": message_id,
                        "delta": format!("Agent error: {err}"),
                    }),
                )
                .await;
                full_text = format!("Agent error: {err}");
            } else if !resp.ok {
                let msg = "Agent turn finished with no response (check provider / ACP).";
                let _ = send_event(
                    &mut sender,
                    "error",
                    json!({ "message": msg, "messageId": message_id }),
                )
                .await;
                let _ = send_event(
                    &mut sender,
                    "content_delta",
                    json!({
                        "messageId": message_id,
                        "delta": msg,
                    }),
                )
                .await;
                full_text = msg.to_string();
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

    // Log the chat turn (local JSONL, future: remote transport).
    {
        let was_aborted = abort.load(std::sync::atomic::Ordering::SeqCst);
        let project = provider
            .project_root()
            .map(|p| crate::project_layout::project_display_name(&p))
            .unwrap_or_else(|| "unknown".into());
        let active_file = provider
            .list_files()
            .await
            .into_iter()
            .find(|f| f.active)
            .map(|f| f.name);
        let tool_entries: Vec<crate::chat_log::ToolCallEntry> = tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?.to_string();
                let detail = t
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(crate::chat_log::ToolCallEntry { name, detail })
            })
            .collect();
        let entry = crate::chat_log::ChatLogEntry {
            timestamp: crate::chat_log::now_iso(),
            turn_id: message_id.clone(),
            project,
            active_file,
            prompt: prompt_for_log.clone(),
            response: full_text.clone(),
            tool_calls: tool_entries,
            source_changed,
            backend: backend.clone(),
            model: None,
            duration_ms: None,
            aborted: was_aborted,
            error: done_payload
                .as_ref()
                .and_then(|r| r.error.clone()),
        };
        crate::chat_log::log_turn(&entry).await;

        // Durable conversation turn (DDB) for resume across crashes/restarts.
        if let Some(ref sid) = coding_session {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .to_string();
            let _ = crate::session::append_turn(
                sid,
                &crate::session::SessionTurn {
                    turn_id: message_id.clone(),
                    role: "assistant".into(),
                    content: full_text.clone(),
                    tool_calls: tools.clone(),
                    project: project_scope.clone(),
                    active_file: entry.active_file.clone(),
                    ts,
                    backend: Some(backend.clone()),
                },
            );
        }
    }

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

/// Map platform UX tool names → SPA navigation payloads (runtime-omnipresent-agent-design).
fn navigation_for_platform_tool(name: &str, detail: &serde_json::Value) -> Option<serde_json::Value> {
    // Prefer structured navigation from tool output JSON (string or object).
    if let Some(nav) = detail.get("navigation") {
        return Some(nav.clone());
    }
    if let Some(s) = detail.as_str() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            if let Some(nav) = v.get("navigation") {
                return Some(nav.clone());
            }
        }
    }
    // Fall back to well-known tool → path mapping when ACP only reports the name.
    let path: Option<String> = match name {
        "list_changes" | "open_changes" => Some("/changes".into()),
        "create_change" | "open_create_change" => Some("/changes/new".into()),
        "list_projects" | "open_projects" => Some("/projects".into()),
        "open_deploy" => Some("/deploy".into()),
        "open_registry" => Some("/registry".into()),
        "open_dashboard" => Some("/dashboard".into()),
        "open_config" => Some("/config".into()),
        "navigate_to" => detail
            .get("path")
            .and_then(|p| p.as_str())
            .map(|p| {
                if p.starts_with('/') {
                    p.to_string()
                } else {
                    format!("/{p}")
                }
            })
            .or_else(|| {
                detail
                    .get("detail")
                    .and_then(|d| d.get("path"))
                    .and_then(|p| p.as_str())
                    .map(|p| p.to_string())
            }),
        "open_project" | "open_ide" | "switch_project" => {
            let project = detail
                .get("project")
                .or_else(|| detail.get("slug"))
                .or_else(|| detail.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if project.is_empty() {
                Some("/projects".into())
            } else if name == "open_ide" {
                // Shell embed route — runtime keeps AgentDock; not bare /viewer.
                Some(format!("/projects/{project}/ide"))
            } else {
                Some(format!("/projects/{project}"))
            }
        }
        _ => None,
    };
    path.map(|p| {
        let action = if name == "open_ide" {
            "open-ide"
        } else if name == "switch_project" {
            "switch-project"
        } else {
            "goto"
        };
        let mut nav = json!({
            "action": action,
            "path": p
        });
        if matches!(name, "open_ide" | "open_project" | "switch_project") {
            if let Some(project) = detail
                .get("project")
                .or_else(|| detail.get("slug"))
                .or_else(|| detail.get("id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                nav["project"] = json!(project);
            } else if let Some(proj) = p.strip_prefix("/projects/").and_then(|rest| {
                rest.split('/').next().filter(|s| !s.is_empty()).map(|s| s.to_string())
            }) {
                nav["project"] = json!(proj);
            }
        }
        nav
    })
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

/// Resolve active project for hub agent turns (IDE dual-loop tools need scope).
fn resolve_chat_project(req: &ChatRequest) -> Option<String> {
    if let Some(p) = req
        .project
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && *s != "(none — home/dashboard)" && *s != "(none)")
    {
        return sanitize_project_slug(p);
    }
    let sys = req.system_prompt.as_deref().unwrap_or("");
    // `- Project: relay` from runtimeAgentSession buildSystemPrompt
    for line in sys.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("- Project:") {
            let p = rest.trim();
            if !p.is_empty() && !p.starts_with('(') {
                return sanitize_project_slug(p);
            }
        }
        // `- Page: /projects/relay/ide`
        if let Some(rest) = t.strip_prefix("- Page:") {
            if let Some(slug) = project_from_page_path(rest.trim()) {
                return Some(slug);
            }
        }
    }
    None
}

fn project_from_page_path(path: &str) -> Option<String> {
    // /projects/{slug} or /projects/{slug}/ide
    let path = path.trim().trim_start_matches('/');
    let mut parts = path.split('/');
    if parts.next()? != "projects" {
        return None;
    }
    let slug = parts.next()?;
    sanitize_project_slug(slug)
}

fn sanitize_project_slug(raw: &str) -> Option<String> {
    let s = raw.trim().trim_matches('`').trim_matches('"').trim_matches('\'');
    if s.is_empty() {
        return None;
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(s.to_string())
    } else {
        None
    }
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
