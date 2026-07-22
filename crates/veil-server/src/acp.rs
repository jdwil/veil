//! ACP (Agent Client Protocol) client — spawn an external agent (Kiro, etc.).
//!
//! Env:
//! - `VEIL_MODEL_PROVIDER=acp`
//! - `VEIL_ACP_COMMAND` (default `kiro-cli`)
//! - `VEIL_ACP_ARGS` (default `acp --trust-all-tools`)
//! - `VEIL_ACP_CWD` (default: process cwd)
//! - `VEIL_ACP_AGENT` / `VEIL_MODEL_NAME` optional agent/model for first session
//! - `VEIL_ACP_TIMEOUT_SECS` (default 300)

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Result of one ACP prompt turn.
#[derive(Debug, Clone)]
pub struct AcpTurnResult {
    pub text: String,
    pub session_id: String,
    pub stop_reason: Option<String>,
    pub tool_hints: Vec<String>,
}

struct AcpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    session_id: Option<String>,
    cwd: String,
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl AcpProcess {
    fn spawn() -> Result<Self, String> {
        let cmd = std::env::var("VEIL_ACP_COMMAND").unwrap_or_else(|_| "kiro-cli".into());
        let args_raw = std::env::var("VEIL_ACP_ARGS")
            .unwrap_or_else(|_| "acp --trust-all-tools".into());
        let mut args: Vec<String> = args_raw
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if args.is_empty() {
            args.push("acp".into());
            args.push("--trust-all-tools".into());
        }
        if let Ok(agent) = std::env::var("VEIL_ACP_AGENT") {
            if !agent.is_empty() && !args.iter().any(|a| a == "--agent") {
                args.push("--agent".into());
                args.push(agent);
            }
        }
        // Only pass --model when explicitly set to a real Kiro model id.
        // Placeholders like "kiro" / "acp" / ollama defaults are NOT valid Kiro
        // model ids and cause: "The model 'kiro' is not available".
        // Prefer VEIL_ACP_MODEL; fall back to VEIL_MODEL_NAME only if it looks real.
        if let Some(model) = resolve_acp_model_arg() {
            if !args.iter().any(|a| a == "--model") {
                args.push("--model".into());
                args.push(model);
            }
        }

        let cwd = std::env::var("VEIL_ACP_CWD").unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".into())
        });

        let mut child = Command::new(&cmd)
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "failed to spawn ACP agent `{cmd} {}`: {e}\n\
                     Install Kiro CLI and ensure it is on PATH (or set VEIL_ACP_COMMAND).",
                    args.join(" ")
                )
            })?;

        let stdin = child.stdin.take().ok_or("ACP stdin missing")?;
        let stdout = child.stdout.take().ok_or("ACP stdout missing")?;
        let mut proc = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            session_id: None,
            cwd,
        };
        proc.initialize()?;
        Ok(proc)
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    fn write_msg(&mut self, msg: &Value) -> Result<(), String> {
        let line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|e| format!("ACP write: {e}"))?;
        self.stdin.flush().map_err(|e| format!("ACP flush: {e}"))?;
        Ok(())
    }

    fn read_line_timeout(&mut self, deadline: Instant) -> Result<String, String> {
        // Blocking read with process-level deadline checks between retries is
        // hard without async; use a simple loop with try_wait + set short
        // timeout via nonblocking is platform-specific. We use blocking
        // read_line and rely on overall turn timeout in the host.
        let mut line = String::new();
        loop {
            if Instant::now() > deadline {
                return Err("ACP read timed out".into());
            }
            // Check child still alive
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("ACP agent exited early ({status})"));
                }
                Ok(None) => {}
                Err(e) => return Err(format!("ACP wait: {e}")),
            }
            line.clear();
            // Blocking — for long model turns this is OK on a blocking thread.
            let n = self
                .stdout
                .read_line(&mut line)
                .map_err(|e| format!("ACP read: {e}"))?;
            if n == 0 {
                return Err("ACP stdout closed".into());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            return Ok(trimmed.to_string());
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.request_streaming(method, params, timeout, None)
    }

    /// Like [`request`], but invokes `on_text` for each assistant text chunk
    /// (Kiro `agent_message_chunk`) as it arrives.
    fn request_streaming(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        mut on_text: Option<&mut dyn FnMut(&str)>,
    ) -> Result<Value, String> {
        let id = self.next_id();
        self.write_msg(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + timeout;
        let mut text_chunks: Vec<String> = Vec::new();
        let mut tool_hints: Vec<String> = Vec::new();
        loop {
            let line = self.read_line_timeout(deadline)?;
            let msg: Value = serde_json::from_str(&line)
                .map_err(|e| format!("ACP JSON parse: {e}: {line}"))?;

            // Streamed session updates (collect text)
            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                if method == "session/update" || method.ends_with("/update") {
                    let before = text_chunks.len();
                    collect_update(&msg, &mut text_chunks, &mut tool_hints);
                    if let Some(cb) = on_text.as_mut() {
                        for t in &text_chunks[before..] {
                            cb(t);
                        }
                    }
                }
                // Agent may send requests (fs read/write, tool approval, etc.).
                // With MCP tools registered, Kiro routes tool calls through MCP.
                // For any remaining host requests, return method-not-found gracefully.
                if let Some(req_id) = msg.get("id").cloned() {
                    if msg.get("method").is_some() && msg.get("result").is_none() {
                        let req_method = msg
                            .get("method")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown");
                        // Log for debugging but don't break the session.
                        eprintln!(
                            "[veil-acp] unhandled agent request: {req_method} (responding method_not_found)"
                        );
                        let _ = self.write_msg(&json!({
                            "jsonrpc": "2.0",
                            "id": req_id,
                            "error": { "code": -32601, "message": format!("method not supported by VEIL host: {req_method}") }
                        }));
                    }
                }
                continue;
            }

            if msg.get("id").and_then(|i| i.as_u64()) == Some(id)
                || msg.get("id").and_then(|i| i.as_i64()) == Some(id as i64)
            {
                if let Some(err) = msg.get("error") {
                    return Err(format!("ACP {method} error: {err}"));
                }
                let mut result = msg
                    .get("result")
                    .cloned()
                    .unwrap_or(Value::Null);
                // Attach collected stream text for prompt calls
                if method == "session/prompt" {
                    if let Value::Object(ref mut map) = result {
                        if !text_chunks.is_empty() {
                            map.insert(
                                "_veil_text".into(),
                                Value::String(text_chunks.join("")),
                            );
                        }
                        if !tool_hints.is_empty() {
                            map.insert(
                                "_veil_tools".into(),
                                Value::Array(
                                    tool_hints
                                        .into_iter()
                                        .map(Value::String)
                                        .collect(),
                                ),
                            );
                        }
                    }
                }
                return Ok(result);
            }
        }
    }

    fn initialize(&mut self) -> Result<(), String> {
        let timeout = Duration::from_secs(30);
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": true
                },
                "clientInfo": { "name": "veil", "version": "0.1.0" }
            }),
            timeout,
        )?;
        Ok(())
    }

    fn ensure_session(&mut self, timeout: Duration) -> Result<String, String> {
        if let Some(ref s) = self.session_id {
            return Ok(s.clone());
        }
        // Use project directory as session cwd so Kiro loads .kiro/settings/mcp.json.
        // IMPORTANT: Do NOT pass a non-empty mcpServers array in session/new —
        // Kiro 2.12 exits (stdout closed) on that shape. Workspace mcp.json
        // with `{ "mcpServers": { "name": { "url": "http://..." } } }` works.
        let session_cwd = resolve_acp_cwd();
        write_workspace_mcp_json(&session_cwd);
        let result = self.request(
            "session/new",
            json!({
                "cwd": session_cwd,
                "mcpServers": []
            }),
            timeout,
        )?;
        let sid = result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("session/new missing sessionId: {result}"))?
            .to_string();
        self.session_id = Some(sid.clone());
        Ok(sid)
    }

    fn prompt(&mut self, text: &str, timeout: Duration) -> Result<AcpTurnResult, String> {
        self.prompt_streaming(text, timeout, None)
    }

    fn prompt_streaming(
        &mut self,
        text: &str,
        timeout: Duration,
        on_text: Option<&mut dyn FnMut(&str)>,
    ) -> Result<AcpTurnResult, String> {
        let sid = self.ensure_session(timeout)?;
        let result = self.request_streaming(
            "session/prompt",
            json!({
                "sessionId": sid,
                "prompt": [{ "type": "text", "text": text }]
            }),
            timeout,
            on_text,
        )?;
        let stop = result
            .get("stopReason")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let streamed = result
            .get("_veil_text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tools = result
            .get("_veil_tools")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let text = if streamed.is_empty() {
            // Some agents only put text in result
            result
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("(ACP turn finished with no text chunks — check agent tools/output.)")
                .to_string()
        } else {
            streamed
        };
        Ok(AcpTurnResult {
            text,
            session_id: sid,
            stop_reason: stop,
            tool_hints: tools,
        })
    }
}

fn collect_update(msg: &Value, text: &mut Vec<String>, tools: &mut Vec<String>) {
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let update = params.get("update").cloned().unwrap_or(params.clone());
    // Kiro: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "…" } }
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let kind_l = kind.to_lowercase();
    if kind_l.contains("message") || kind_l.contains("chunk") || kind_l.contains("text") {
        if let Some(t) = extract_text(&update) {
            text.push(t);
            return;
        }
    }
    if kind_l.contains("tool") {
        let name = update
            .get("title")
            .or_else(|| update.get("toolName"))
            .or_else(|| update.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("tool");
        tools.push(name.to_string());
        if let Some(t) = extract_text(&update) {
            text.push(format!("\n[{name}] {t}\n"));
        }
        return;
    }
    if let Some(t) = extract_text(&update) {
        text.push(t);
    }
}

fn extract_text(v: &Value) -> Option<String> {
    if let Some(s) = v.get("text").and_then(|t| t.as_str()) {
        return Some(s.to_string());
    }
    if let Some(c) = v.get("content") {
        if let Some(s) = c.as_str() {
            return Some(s.to_string());
        }
        if let Some(s) = c.get("text").and_then(|t| t.as_str()) {
            return Some(s.to_string());
        }
        if let Some(arr) = c.as_array() {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(s) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(s.to_string());
                } else if let Some(s) = item.as_str() {
                    parts.push(s.to_string());
                }
            }
            if !parts.is_empty() {
                return Some(parts.join(""));
            }
        }
    }
    None
}

/// Resolve the cwd for ACP sessions — use the active project directory.
///
/// Kiro loads MCP server config from `.kiro/settings/mcp.json` relative to cwd,
/// so pointing at the project root ensures it finds the VEIL tools config.
fn resolve_acp_cwd() -> String {
    // If we have a project name, resolve its path from projects_dir.
    if let Some(project) = ACP_PROJECT.lock().ok().and_then(|g| g.clone()) {
        let projects_dir = crate::config::resolve_projects_dir();
        let project_path = projects_dir.join(&project);
        if project_path.is_dir() {
            return project_path.to_string_lossy().to_string();
        }
    }
    // Fallback to env or process cwd.
    std::env::var("VEIL_ACP_CWD").unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into())
    })
}

/// Write `.kiro/settings/mcp.json` so Kiro discovers VEIL MCP (incl. wiki_*).
///
/// Proven working shape (Kiro 2.12): map of name → `{ "url": "http://host/api/mcp" }`.
/// Do not put non-empty mcpServers in session/new — that crashes the agent.
fn write_workspace_mcp_json(session_cwd: &str) {
    let dir = std::path::Path::new(session_cwd).join(".kiro/settings");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let port = std::env::var("VEIL_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .or_else(|| {
            std::env::var("PORT")
                .ok()
                .and_then(|s| s.parse::<u16>().ok())
        })
        .unwrap_or(3001);
    let project = ACP_PROJECT.lock().ok().and_then(|g| g.clone());
    let mcp_url = if let Some(ref proj) = project {
        format!("http://127.0.0.1:{port}/api/p/{proj}/mcp")
    } else {
        format!("http://127.0.0.1:{port}/api/mcp")
    };
    let tool_names = [
        "veil_check",
        "veil_outline",
        "read_source",
        "write_source",
        "rename_construct",
        "list_files",
        "select_file",
        "create_file",
        "wiki_search",
        "wiki_read",
        "wiki_traverse",
        "wiki_create",
        "wiki_update",
        "wiki_list",
    ];
    let doc = json!({
        "mcpServers": {
            "veil-ide-tools": {
                "url": mcp_url,
                "autoApprove": tool_names
            }
        }
    });
    let path = dir.join("mcp.json");
    if let Ok(s) = serde_json::to_string_pretty(&doc) {
        let _ = std::fs::write(path, s);
    }
}

/// Active project name for ACP sessions (set before spawn_blocking).
static ACP_PROJECT: Mutex<Option<String>> = Mutex::new(None);

/// Set the active project for ACP tool routing. Call before prompting.
pub fn set_acp_project(name: Option<String>) {
    if let Ok(mut g) = ACP_PROJECT.lock() {
        *g = name;
    }
}

/// Process-wide ACP session (one agent child).
static ACP: Mutex<Option<AcpProcess>> = Mutex::new(None);

fn timeout_secs() -> u64 {
    std::env::var("VEIL_ACP_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

/// Run one prompt against the long-lived ACP agent (spawn on first use).
pub fn prompt_acp(text: &str) -> Result<AcpTurnResult, String> {
    prompt_acp_streaming(text, |_| {})
}

/// Like [`prompt_acp`], but `on_chunk` is called for each text delta as Kiro streams.
pub fn prompt_acp_streaming(
    text: &str,
    mut on_chunk: impl FnMut(&str),
) -> Result<AcpTurnResult, String> {
    let timeout = Duration::from_secs(timeout_secs());
    let mut guard = ACP
        .lock()
        .map_err(|e| format!("ACP lock poisoned: {e}"))?;
    if guard.is_none() {
        *guard = Some(AcpProcess::spawn()?);
    }
    let proc = guard.as_mut().unwrap();
    let mut cb = |s: &str| on_chunk(s);
    match proc.prompt_streaming(text, timeout, Some(&mut cb)) {
        Ok(r) => Ok(r),
        Err(e) => {
            // Drop broken process so next call respawns
            *guard = None;
            Err(e)
        }
    }
}

/// Whether ACP is configured as the model provider.
pub fn acp_enabled() -> bool {
    std::env::var("VEIL_MODEL_PROVIDER")
        .map(|v| {
            let v = v.to_lowercase();
            v == "acp" || v == "kiro"
        })
        .unwrap_or(false)
}

/// Resolve optional `--model` for `kiro-cli acp`.
///
/// Kiro's default (often `auto` from `~/.kiro` settings) is used when we omit
/// `--model`. Never pass VEIL placeholders (`kiro`, `acp`, ollama model names).
fn resolve_acp_model_arg() -> Option<String> {
    let explicit = std::env::var("VEIL_ACP_MODEL").ok().filter(|s| !s.trim().is_empty());
    let from_name = std::env::var("VEIL_MODEL_NAME").ok().filter(|s| !s.trim().is_empty());
    let candidate = explicit.or(from_name)?;
    if is_placeholder_model(&candidate) {
        return None;
    }
    Some(candidate)
}

fn is_placeholder_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    matches!(
        m.as_str(),
        "" | "echo" | "kiro" | "acp" | "heuristic" | "none"
    ) || m.contains("qwen")
        || m.contains("llama")
        || m.starts_with("gpt-") // OpenAI ids — not Kiro ACP model ids
}

/// Info blob for GET /api/models.
pub fn acp_info() -> serde_json::Value {
    let model_arg = resolve_acp_model_arg();
    json!({
        "provider": "acp",
        "command": std::env::var("VEIL_ACP_COMMAND").unwrap_or_else(|_| "kiro-cli".into()),
        "args": std::env::var("VEIL_ACP_ARGS").unwrap_or_else(|_| "acp --trust-all-tools".into()),
        "cwd": std::env::var("VEIL_ACP_CWD").ok(),
        "model": model_arg.clone().unwrap_or_else(|| "(kiro default / auto)".into()),
        "model_flag": model_arg,
        "timeout_secs": timeout_secs(),
        "rig": false,
        "acp": true,
        "hint": "Set VEIL_ACP_MODEL to a real Kiro model id, or omit for default. Do not use VEIL_MODEL_NAME=kiro.",
    })
}

/// Force-drop the agent process (tests / config change).
pub fn reset_acp() {
    if let Ok(mut g) = ACP.lock() {
        *g = None;
    }
}

/// Abort the current ACP turn by killing the child process.
/// The next prompt will respawn a fresh session.
pub fn cancel_acp() {
    if let Ok(mut g) = ACP.lock() {
        if g.is_some() {
            tracing::info!("ACP turn cancelled — killing agent process for respawn");
            *g = None; // Drop triggers child.kill() + child.wait()
        }
    }
}

// Silence unused Arc import warning path if any
#[allow(dead_code)]
fn _arc_marker() -> Arc<()> {
    Arc::new(())
}
