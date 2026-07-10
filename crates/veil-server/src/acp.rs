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
        if let Ok(model) = std::env::var("VEIL_ACP_MODEL")
            .or_else(|_| std::env::var("VEIL_MODEL_NAME"))
        {
            // Only pass model if using kiro-style CLI and not already set
            if !model.is_empty()
                && model != "echo"
                && !args.iter().any(|a| a == "--model")
            {
                // Skip ollama-looking names when user still has default
                if !model.contains("qwen") && !model.contains("llama") {
                    args.push("--model".into());
                    args.push(model);
                }
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

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
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
                    collect_update(&msg, &mut text_chunks, &mut tool_hints);
                }
                // Agent may send requests we should auto-ack (fs read/write)
                if let Some(req_id) = msg.get("id").cloned() {
                    if msg.get("method").is_some() && msg.get("result").is_none() {
                        // Best-effort cancel/allow — Kiro with --trust-all-tools
                        // rarely needs this; answer with empty error ignore.
                        let _ = self.write_msg(&json!({
                            "jsonrpc": "2.0",
                            "id": req_id,
                            "error": { "code": -32601, "message": "not implemented by VEIL host" }
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
        let _ = self.request(
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
        let result = self.request(
            "session/new",
            json!({
                "cwd": self.cwd,
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
        let sid = self.ensure_session(timeout)?;
        let result = self.request(
            "session/prompt",
            json!({
                "sessionId": sid,
                "prompt": [{ "type": "text", "text": text }]
            }),
            timeout,
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
    let update = params.get("update").cloned().unwrap_or(params);
    // Common shapes: { sessionUpdate: "agent_message_chunk", content: { type, text } }
    // or { type: "AgentMessageChunk", content: ... }
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let kind_l = kind.to_lowercase();
    if kind_l.contains("message") || kind_l.contains("chunk") || kind_l.contains("text") {
        if let Some(t) = extract_text(&update) {
            text.push(t);
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
    }
    // Nested content arrays
    if let Some(t) = extract_text(&update) {
        if text.last().map(|s| s.as_str()) != Some(t.as_str()) {
            // avoid dup when already pushed
            if !kind_l.contains("message") && !kind_l.contains("tool") {
                text.push(t);
            }
        }
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
    let timeout = Duration::from_secs(timeout_secs());
    let mut guard = ACP
        .lock()
        .map_err(|e| format!("ACP lock poisoned: {e}"))?;
    if guard.is_none() {
        *guard = Some(AcpProcess::spawn()?);
    }
    let proc = guard.as_mut().unwrap();
    match proc.prompt(text, timeout) {
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

/// Info blob for GET /api/models.
pub fn acp_info() -> serde_json::Value {
    json!({
        "provider": "acp",
        "command": std::env::var("VEIL_ACP_COMMAND").unwrap_or_else(|_| "kiro-cli".into()),
        "args": std::env::var("VEIL_ACP_ARGS").unwrap_or_else(|_| "acp --trust-all-tools".into()),
        "cwd": std::env::var("VEIL_ACP_CWD").ok(),
        "timeout_secs": timeout_secs(),
        "rig": false,
        "acp": true,
    })
}

/// Force-drop the agent process (tests / config change).
pub fn reset_acp() {
    if let Ok(mut g) = ACP.lock() {
        *g = None;
    }
}

// Silence unused Arc import warning path if any
#[allow(dead_code)]
fn _arc_marker() -> Arc<()> {
    Arc::new(())
}
