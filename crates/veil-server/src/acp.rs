//! ACP (Agent Client Protocol) client — spawn an external agent (Kiro, etc.).
//!
//! Env:
//! - `VEIL_MODEL_PROVIDER=acp`
//! - `VEIL_ACP_COMMAND` (default `kiro-cli`)
//! - `VEIL_ACP_ARGS` (default `acp`; set `acp --trust-all-tools` explicitly if needed)
//! - `VEIL_ACP_CWD` — disk-mode fallback only. **Ignored when
//!   `VEIL_SOURCE_MODE` is s3/prefer_s3/local** so Kiro cannot grep staged
//!   checkouts under `$TMP/veil-ws` / `$TMP/veil-s3-ws`.
//! - `VEIL_REFERENCE_DIRS` — optional colon-separated local trees the agent
//!   may **read** via MCP `reference_*` (not ACP fs).
//! - `VEIL_ACP_AGENT` — Kiro agent name (default: `veil` when
//!   `~/.kiro/agents/veil.json` exists; see `config/kiro-agent-veil.json`)
//! - `VEIL_ACP_TIMEOUT_SECS` (default 300)
//!
//! ## Inner-agent session tagging (Core Fix C)
//!
//! Kiro session transcripts live in `~/.kiro/sessions/cli/{id}.jsonl` (+
//! `{id}.json` meta) and are **shared** across every Kiro on the box — the
//! core/dev agent (`agent=hive`, cwd=repo) *and* this runtime's inner agent
//! (`agent=veil`, cwd=`$TMP/veil-acp-cwd`). To find the inner agent's turn
//! deterministically (not by a fragile cwd/agent heuristic) we tag it two ways:
//!
//! 1. **Env markers** on the spawned child — `VEIL_INNER_AGENT=1`,
//!    `VEIL_INNER_PROJECT={slug}`, `VEIL_INNER_SESSION_TAG={tag}`. Queryable
//!    while the child is alive via `/proc/{pid}/environ`.
//! 2. **Sidecar mapping record** — when `session/new` returns a `sessionId`,
//!    the runtime writes `~/.kiro/sessions/cli/{id}.veil-inner.json`
//!    ({inner_agent, project, tag, kiro_session_id, created_at, runtime_pid})
//!    **and** appends to `$TMP/veil-acp-cwd/.veil-inner-sessions.jsonl`. A
//!    reader locates the inner agent's transcript by the presence of the
//!    `*.veil-inner.json` sidecar — no cwd guessing. See palace
//!    `incident-inner-agent-stale-branch`.
//!
//! VEIL does **not** rewrite `~/.kiro/agents/hive.json`. Use a dedicated
//! `veil` agent that includes mind-palace/jira **and** `veil-ide-tools`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Full-fidelity record of ONE tool invocation within an ACP turn.
///
/// The ACP protocol streams a `tool_call` update (initial: name/kind/input,
/// status `pending`/`in_progress`) followed by one or more `tool_call_update`
/// updates (status `completed`/`failed`, plus `content`/`rawOutput`). We
/// coalesce those by `tool_call_id` into a single record so the durable turn
/// captures name + arguments + result + status + ordering (audit-logging
/// Part 1). Content is captured raw here; secret redaction + S3 offload happen
/// at persist time in `agent_stream`.
#[derive(Debug, Clone, Default)]
pub struct AcpToolRecord {
    /// ACP `toolCallId` (coalescing key). May be empty for agents that omit it.
    pub tool_call_id: String,
    /// Tool name (`title` / `toolName` / `name` / `kind`).
    pub name: String,
    /// Tool category/kind when the agent supplies one (`read`/`edit`/`execute`…).
    pub kind: Option<String>,
    /// Latest status: `pending` | `in_progress` | `completed` | `failed`.
    pub status: Option<String>,
    /// Tool arguments (`rawInput`) as sent by the model.
    pub input: Option<Value>,
    /// Structured tool output (`rawOutput`) when the agent provides it.
    pub output: Option<Value>,
    /// Flattened human-readable result text from `content` blocks.
    pub content: String,
    /// Monotonic order index (first-seen order within the turn).
    pub order: usize,
    /// RFC3339 timestamp when this record was first observed.
    pub started_at: String,
}

/// Result of one ACP prompt turn.
#[derive(Debug, Clone)]
pub struct AcpTurnResult {
    pub text: String,
    pub session_id: String,
    pub stop_reason: Option<String>,
    pub tool_hints: Vec<String>,
    /// Full-fidelity tool call+result records, in invocation order.
    pub tool_calls: Vec<AcpToolRecord>,
}

/// Extra `session/prompt` content blocks (raster diagrams from the chat pane).
#[derive(Debug, Clone, Default)]
pub struct AcpMedia {
    pub images: Vec<AcpImagePart>,
}

#[derive(Debug, Clone)]
pub struct AcpImagePart {
    pub mime_type: String,
    pub data_base64: String,
}

struct AcpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
    session_id: Option<String>,
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
        let args_raw = std::env::var("VEIL_ACP_ARGS").unwrap_or_else(|_| "acp".into());
        let mut args: Vec<String> = args_raw.split_whitespace().map(|s| s.to_string()).collect();
        if args.is_empty() {
            args.push("acp".into());
        }
        // Workspace mcp.json only (never rewrite ~/.kiro/agents/*.json).
        let cwd_pre = resolve_acp_cwd();
        write_workspace_mcp_json(&cwd_pre);

        let agent = resolve_acp_agent_name();
        if !agent.is_empty() && !args.iter().any(|a| a == "--agent") {
            args.push("--agent".into());
            args.push(agent);
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

        // Prefer project materialize / hub path so session cwd matches open product.
        let cwd = resolve_acp_cwd();

        let mut command = Command::new(&cmd);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Core Fix C: tag this child as the inner (veil/ACP) agent so its Kiro
        // session is identifiable without a cwd heuristic.
        apply_inner_agent_env(&mut command);

        let mut child = command.spawn().map_err(|e| {
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
                return Err(format!(
                    "ACP idle timed out (no agent traffic for {}s)",
                    timeout_secs()
                ));
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
        // Idle budget, not a total-turn wall clock. Reset on every ACP line.
        let mut deadline = Instant::now() + timeout;
        let mut text_chunks: Vec<String> = Vec::new();
        let mut tool_hints: Vec<String> = Vec::new();
        let mut tool_records: Vec<AcpToolRecord> = Vec::new();
        loop {
            let line = self.read_line_timeout(deadline)?;
            deadline = Instant::now() + timeout;
            let msg: Value =
                serde_json::from_str(&line).map_err(|e| format!("ACP JSON parse: {e}: {line}"))?;

            // Streamed session updates (collect text)
            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                if method == "session/update" || method.ends_with("/update") {
                    let before_text = text_chunks.len();
                    let before_tools = tool_hints.len();
                    collect_update_full(
                        &msg,
                        &mut text_chunks,
                        &mut tool_hints,
                        &mut tool_records,
                    );
                    if let Some(cb) = on_text.as_mut() {
                        for t in &text_chunks[before_text..] {
                            cb(t);
                        }
                        // Emit tool markers as they arrive (real-time)
                        for t in &tool_hints[before_tools..] {
                            cb(&format!("\x01TOOL:{}\x01", t));
                        }
                    }
                }
                // Agent may send host requests (fs/terminal/permissions).
                // Product source is MCP-only — never implement ACP filesystem
                // against the staged checkout.
                if let Some(req_id) = msg.get("id").cloned() {
                    if msg.get("method").is_some() && msg.get("result").is_none() {
                        let req_method = msg
                            .get("method")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown");
                        let message = acp_host_method_refusal(req_method);
                        eprintln!("[veil-acp] refuse agent request: {req_method}");
                        let _ = self.write_msg(&json!({
                            "jsonrpc": "2.0",
                            "id": req_id,
                            "error": { "code": -32601, "message": message }
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
                let mut result = msg.get("result").cloned().unwrap_or(Value::Null);
                // Attach collected stream text for prompt calls
                if method == "session/prompt" {
                    if let Value::Object(ref mut map) = result {
                        if !text_chunks.is_empty() {
                            map.insert("_veil_text".into(), Value::String(text_chunks.join("")));
                        }
                        if !tool_hints.is_empty() {
                            map.insert(
                                "_veil_tools".into(),
                                Value::Array(tool_hints.into_iter().map(Value::String).collect()),
                            );
                        }
                        if !tool_records.is_empty() {
                            let recs: Vec<Value> = tool_records
                                .iter()
                                .map(acp_tool_record_to_json)
                                .collect();
                            map.insert("_veil_tool_calls".into(), Value::Array(recs));
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
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false
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
        // Agent mcpServers (hive + veil-ide-tools merge) are the primary tool source.
        // session/new must pass mcpServers: [] — non-empty array crashes Kiro 2.12.
        // Cwd is a sandbox in remote source mode — not the S3/session checkout.
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
        // Core Fix C: emit the deterministic inner-agent discriminator now that
        // Kiro has minted the session id. Only when this is the sandboxed inner
        // agent (remote source mode) — disk `veil serve` shares the operator's
        // own cwd and needs no marker.
        if acp_should_sandbox() {
            record_inner_session(&sid);
        }
        self.session_id = Some(sid.clone());
        Ok(sid)
    }

    fn prompt_streaming(
        &mut self,
        text: &str,
        media: &AcpMedia,
        timeout: Duration,
        on_text: Option<&mut dyn FnMut(&str)>,
    ) -> Result<AcpTurnResult, String> {
        let sid = self.ensure_session(timeout)?;
        let mut prompt_blocks = vec![json!({ "type": "text", "text": text })];
        for img in &media.images {
            if img.data_base64.is_empty() {
                continue;
            }
            prompt_blocks.push(json!({
                "type": "image",
                "mimeType": img.mime_type,
                "data": img.data_base64,
            }));
        }
        let result = self.request_streaming(
            "session/prompt",
            json!({
                "sessionId": sid,
                "prompt": prompt_blocks
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
        let tool_calls = result
            .get("_veil_tool_calls")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(acp_tool_record_from_json).collect())
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
            tool_calls,
        })
    }
}

/// Collect assistant text + tool name hints from a `session/update`, and
/// coalesce full tool call+result records into `records` (keyed by
/// `toolCallId`) for durable audit capture (Part 1).
fn collect_update_full(
    msg: &Value,
    text: &mut Vec<String>,
    tools: &mut Vec<String>,
    records: &mut Vec<AcpToolRecord>,
) {
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
        // First observation of a NEW tool_call → push a name hint (the marker
        // stream that drives live UI chips). A `tool_call_update` for an
        // already-seen id must NOT re-emit the chip.
        let call_id = update
            .get("toolCallId")
            .or_else(|| update.get("toolCallID"))
            .or_else(|| update.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let existing = if call_id.is_empty() {
            None
        } else {
            records.iter_mut().position(|r| r.tool_call_id == call_id)
        };
        let tool_kind = update
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let status = update
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let raw_input = update
            .get("rawInput")
            .or_else(|| update.get("input"))
            .cloned()
            .filter(|v| !v.is_null());
        let raw_output = update
            .get("rawOutput")
            .or_else(|| update.get("output"))
            .cloned()
            .filter(|v| !v.is_null());
        let content_text = extract_text(&update);
        match existing {
            Some(idx) => {
                // Coalesce a later tool_call_update onto the initial record.
                let rec = &mut records[idx];
                if tool_kind.is_some() {
                    rec.kind = tool_kind;
                }
                if status.is_some() {
                    rec.status = status;
                }
                if raw_input.is_some() {
                    rec.input = raw_input;
                }
                if raw_output.is_some() {
                    rec.output = raw_output;
                }
                if let Some(ref t) = content_text {
                    if !t.is_empty() {
                        if !rec.content.is_empty() {
                            rec.content.push('\n');
                        }
                        rec.content.push_str(t);
                    }
                }
            }
            None => {
                tools.push(name.to_string());
                let order = records.len();
                records.push(AcpToolRecord {
                    tool_call_id: call_id,
                    name: name.to_string(),
                    kind: tool_kind,
                    status,
                    input: raw_input,
                    output: raw_output,
                    content: content_text.clone().unwrap_or_default(),
                    order,
                    started_at: chrono_now_rfc3339(),
                });
            }
        }
        if let Some(t) = content_text {
            text.push(format!("\n[{name}] {t}\n"));
        }
        return;
    }
    if let Some(t) = extract_text(&update) {
        text.push(t);
    }
}

/// RFC3339 timestamp (UTC) for capture records without pulling chrono here.
fn chrono_now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    // Minimal epoch→RFC3339 (UTC) without a date lib. Days since 1970.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Serialize an [`AcpToolRecord`] to the transport JSON used on the prompt
/// result (`_veil_tool_calls`) and, ultimately, the durable turn.
fn acp_tool_record_to_json(r: &AcpToolRecord) -> Value {
    json!({
        "tool_call_id": r.tool_call_id,
        "name": r.name,
        "kind": r.kind,
        "status": r.status,
        "input": r.input,
        "output": r.output,
        "content": r.content,
        "order": r.order,
        "started_at": r.started_at,
    })
}

/// Parse a transport JSON tool-call record back into an [`AcpToolRecord`].
fn acp_tool_record_from_json(v: &Value) -> AcpToolRecord {
    AcpToolRecord {
        tool_call_id: v
            .get("tool_call_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("tool")
            .to_string(),
        kind: v.get("kind").and_then(|x| x.as_str()).map(String::from),
        status: v.get("status").and_then(|x| x.as_str()).map(String::from),
        input: v.get("input").cloned().filter(|x| !x.is_null()),
        output: v.get("output").cloned().filter(|x| !x.is_null()),
        content: v
            .get("content")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        order: v.get("order").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        started_at: v
            .get("started_at")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
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

/// Active ACP agent name (default `veil` when installed, else `hive`).
fn resolve_acp_agent_name() -> String {
    if let Ok(a) = std::env::var("VEIL_ACP_AGENT") {
        let t = a.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    // Prefer dedicated agent if installed; never invent/mutate hive.json.
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let agents = PathBuf::from(home).join(".kiro/agents");
    if agents.join("veil.json").is_file() {
        "veil".into()
    } else if agents.join("veil-runtime.json").is_file() {
        // legacy name from earlier setup
        "veil-runtime".into()
    } else {
        "hive".into()
    }
}

fn product_host_port() -> u16 {
    std::env::var("VEIL_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| std::env::var("PORT").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(8080)
}

/// MCP HTTP URL for the current project scope (hub or `/api/p/{slug}/mcp`).
fn veil_ide_mcp_url() -> String {
    let port = product_host_port();
    if let Some(ref proj) = ACP_PROJECT.lock().ok().and_then(|g| g.clone()) {
        format!("http://127.0.0.1:{port}/api/p/{proj}/mcp")
    } else {
        format!("http://127.0.0.1:{port}/api/mcp")
    }
}

/// Full autoApprove list for VEIL IDE + platform tools.
fn veil_ide_tool_names() -> Vec<&'static str> {
    vec![
        "veil_check",
        "veil_outline",
        "read_source",
        "write_source",
        "rename_construct",
        "list_files",
        "select_file",
        "create_file",
        "stub_list",
        "stub_get",
        "stub_gen",
        "stub_install",
        "stub_search",
        "dev_status",
        "dev_logs",
        "dev_restart",
        "smoke_status",
        "read_generated",
        "list_routes",
        "http_request",
        "session_status",
        "create_branch",
        "session_commit",
        "list_commits",
        "merge_branch",
        "switch_main",
        "ws_list",
        "ws_read",
        "ws_write",
        "ws_str_replace",
        "ws_grep",
        "ws_rm",
        "ws_pull",
        "ws_reset",
        "navigate_to",
        "list_projects",
        "create_project",
        "create_repo",
        "get_project",
        "rename_project",
        "update_project",
        "delete_project",
        "open_project",
        "open_ide",
        "switch_project",
        "list_prs",
        "list_prs",
        "resolve_coding_target",
        "run_coding_plan",
        "create_pr",
        "create_pr",
        "open_pr",
        "get_pr",
        "get_pr",
        "submit_pr",
        "submit_pr",
        "approve_pr",
        "approve_pr",
        "request_pr_changes",
        "merge_pr",
        "merge_pr",
        "add_comment",
        "get_pr_diff",
        "get_pr_diff",
        "open_deploy",
        "list_deploy_environments",
        "deploy_status",
        "plan_provision",
        "provision_project",
        "get_provision_job",
        "open_registry",
        "list_registry_layers",
        "list_registry_stubs",
        "search_registry",
        "open_dashboard",
        "open_config",
        "get_config",
        "get_git_status",
        "get_origin",
        "bind_origin",
        "reference_roots",
        "reference_list",
        "reference_read",
        "reference_grep",
        "get_mission",
        "update_mission",
        "get_current_context",
        "wait_intent_ack",
        "wiki_search",
        "wiki_read",
        "wiki_traverse",
        "wiki_create",
        "wiki_update",
        "wiki_list",
    ]
}

fn veil_ide_mcp_server_entry(mcp_url: &str) -> Value {
    json!({
        "url": mcp_url,
        "disabled": false,
        "autoApprove": veil_ide_tool_names(),
    })
}

fn acp_host_method_refusal(method: &str) -> String {
    if method.starts_with("fs/") || method.starts_with("terminal/") {
        return format!(
            "VEIL host does not expose the product checkout over ACP {method}. \
             Use veil-ide-tools MCP (read_source, write_source, stub_search, stub_get, \
             stub_install, ws_*, reference_roots / reference_list / reference_read / reference_grep). \
             Do not grep/sed/cat $TMP/veil-ws or $TMP/veil-s3-ws. Operator local code is read-only \
             via reference_*; product VEIL is write_source."
        );
    }
    format!("method not supported by VEIL host: {method}")
}

/// Remote ProductHost (DDB/S3) must not seat Kiro inside a staged checkout.
fn acp_should_sandbox() -> bool {
    !matches!(
        crate::provider::s3_workspace::ide_source_mode(),
        crate::provider::s3_workspace::IdeSourceMode::Disk
    )
}

/// Empty ACP working directory — not a product tree.
fn ensure_acp_sandbox() -> PathBuf {
    let dir = std::env::temp_dir().join("veil-acp-cwd");
    let _ = std::fs::create_dir_all(&dir);
    let readme = dir.join("README.txt");
    if !readme.is_file() {
        let _ = std::fs::write(
            readme,
            "VEIL ACP sandbox. Product source is not here.\n\
             Use veil-ide-tools MCP: read_source, write_source, stub_search, stub_get.\n\
             Operator local code (conversion): reference_roots / reference_read (read-only).\n\
             Never inspect $TMP/veil-ws or $TMP/veil-s3-ws.\n",
        );
    }
    dir
}

/// Project slug baked into the inner-agent tag (`(none)` when hub-scoped).
fn inner_project_slug() -> String {
    get_acp_project().unwrap_or_else(|| "(none)".into())
}

/// Recognizable session tag for the inner (veil/ACP) agent.
///
/// Shape: `veil-runtime-inner:{project}:{kiro_session_id}`. Written into the
/// sidecar record and the child env so a reader never has to guess on cwd.
fn inner_session_tag(project: &str, kiro_session_id: &str) -> String {
    format!("veil-runtime-inner:{project}:{kiro_session_id}")
}

/// Apply inner-agent env markers to the spawned Kiro child.
///
/// These land in the child's environment (queryable via `/proc/{pid}/environ`
/// while alive) so the inner agent is identifiable without a cwd heuristic.
/// The session id isn't known until `session/new`, so the tag env carries the
/// project-scoped prefix; the full tag (with session id) is in the sidecar.
fn apply_inner_agent_env(cmd: &mut Command) {
    let project = inner_project_slug();
    cmd.env("VEIL_INNER_AGENT", "1");
    cmd.env("VEIL_INNER_PROJECT", &project);
    cmd.env(
        "VEIL_INNER_SESSION_TAG",
        format!("veil-runtime-inner:{project}"),
    );
}

/// Kiro session directory (`~/.kiro/sessions/cli`).
fn kiro_sessions_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".kiro/sessions/cli")
}

/// Deterministic sidecar mapping so the inner agent's transcript is findable
/// by marker (not cwd): given a kiro `session_id`, write
/// `~/.kiro/sessions/cli/{id}.veil-inner.json` and append a line to
/// `$TMP/veil-acp-cwd/.veil-inner-sessions.jsonl`.
///
/// A reader lists `*.veil-inner.json` (most-recent by mtime) → reads the paired
/// `{id}.jsonl` transcript. No `agent_name`/`cwd` guessing required.
fn record_inner_session(kiro_session_id: &str) {
    let project = inner_project_slug();
    let tag = inner_session_tag(&project, kiro_session_id);
    let created_at = crate::session::chrono_now();
    let record = json!({
        "inner_agent": true,
        "kiro_session_id": kiro_session_id,
        "project": project,
        "tag": tag,
        "created_at": created_at,
        "runtime_pid": std::process::id(),
    });

    // 1) Sidecar next to the Kiro session files (primary discriminator).
    let dir = kiro_sessions_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        let sidecar = dir.join(format!("{kiro_session_id}.veil-inner.json"));
        if let Ok(s) = serde_json::to_string_pretty(&record) {
            if let Err(e) = std::fs::write(&sidecar, s) {
                tracing::warn!(error = %e, path = %sidecar.display(),
                    "failed to write inner-agent session sidecar");
            }
        }
    } else {
        tracing::warn!(dir = %dir.display(),
            "could not create Kiro sessions dir for inner-agent sidecar");
    }

    // 2) Append to the runtime-owned log in the ACP sandbox (audit trail).
    let log = ensure_acp_sandbox().join(".veil-inner-sessions.jsonl");
    if let Ok(line) = serde_json::to_string(&record) {
        use std::io::Write as _;
        match std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            Ok(mut f) => {
                let _ = writeln!(f, "{line}");
            }
            Err(e) => tracing::warn!(error = %e, path = %log.display(),
                "failed to append inner-agent session log"),
        }
    }

    tracing::info!(
        kiro_session_id = %kiro_session_id,
        project = %project,
        tag = %tag,
        "tagged inner-agent Kiro session (sidecar + log)"
    );
}

/// Resolve the cwd for ACP sessions.
///
/// **Remote (s3 / prefer_s3):** always the sandbox (`$TMP/veil-acp-cwd`).
/// The daemon still materializes S3 → `$TMP/veil-ws` / `$TMP/veil-s3-ws` for
/// itself; the inner agent must not see those trees.
///
/// **Disk serve:** project hub / `VEIL_ACP_CWD` / process cwd (local `veil serve`).
fn resolve_acp_cwd() -> String {
    resolve_acp_cwd_inner(acp_should_sandbox())
}

fn resolve_acp_cwd_inner(sandbox: bool) -> String {
    if sandbox {
        return ensure_acp_sandbox().to_string_lossy().to_string();
    }
    if let Some(project) = ACP_PROJECT.lock().ok().and_then(|g| g.clone()) {
        let projects_dir = crate::config::resolve_projects_dir();
        let project_path = projects_dir.join(&project);
        if project_path.is_dir() {
            return project_path.to_string_lossy().to_string();
        }
    }
    std::env::var("VEIL_ACP_CWD").unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into())
    })
}

/// Workspace `.kiro/settings/mcp.json` — used when agent has empty mcpServers
/// (e.g. personal). Still written always so IDE and ACP stay aligned.
fn write_workspace_mcp_json(session_cwd: &str) {
    let dir = Path::new(session_cwd).join(".kiro/settings");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mcp_url = veil_ide_mcp_url();
    // Merge with existing workspace servers if any (don't wipe other workspace MCPs)
    let path = dir.join("mcp.json");
    let mut doc: Value = if path.is_file() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({ "mcpServers": {} }))
    } else {
        json!({ "mcpServers": {} })
    };
    if let Some(obj) = doc.as_object_mut() {
        let servers = obj
            .entry("mcpServers".to_string())
            .or_insert_with(|| json!({}));
        if let Some(map) = servers.as_object_mut() {
            map.insert("veil-ide-tools".into(), veil_ide_mcp_server_entry(&mcp_url));
        }
    }
    if let Ok(s) = serde_json::to_string_pretty(&doc) {
        let _ = std::fs::write(path, s);
    }
}

/// Active project name for ACP sessions (set before spawn_blocking).
static ACP_PROJECT: Mutex<Option<String>> = Mutex::new(None);

/// Project baked into the last ACP spawn's workspace `mcp.json` (MCP URL scope).
/// When this differs from [`ACP_PROJECT`], we must respawn so Kiro picks up
/// `/api/p/{project}/mcp` instead of hub-only tools.
static ACP_SPAWN_PROJECT: Mutex<Option<String>> = Mutex::new(None);

/// Set the active project for ACP tool routing. Call before prompting.
pub fn set_acp_project(name: Option<String>) {
    if let Ok(mut g) = ACP_PROJECT.lock() {
        *g = name;
    }
}

/// Current ACP project scope (if any).
pub fn get_acp_project() -> Option<String> {
    ACP_PROJECT.lock().ok().and_then(|g| g.clone())
}

/// Live `session/prompt` holds [`ACP`]. MCP `create_project` → `prepare_project`
/// must **not** take that mutex or the turn deadlocks (Kiro waits for MCP,
/// host waits for Kiro stdout).
static ACP_TURN_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Drop the child after the current turn returns (project changed mid-turn).
static ACP_DEFER_RESET: AtomicBool = AtomicBool::new(false);

/// Ensure ACP MCP routing matches `name` (project-scoped IDE tools).
///
/// Always updates [`ACP_PROJECT`] so hub `/api/mcp` scopes `write_source`
/// via `get_acp_project()`. Rewrites workspace `mcp.json` for the *next* spawn.
///
/// Does **not** kill a live ACP child mid-turn (that deadlocks the mutex
/// held by [`prompt_acp_streaming_media`]). Respawn is deferred until the
/// turn ends. The `veil` Kiro agent uses hub `/api/mcp` anyway — routing
/// is `ACP_PROJECT`, not a process restart.
pub fn ensure_acp_project_scope(name: Option<String>) {
    let prev_spawn = ACP_SPAWN_PROJECT.lock().ok().and_then(|g| g.clone());
    set_acp_project(name.clone());
    let cwd = resolve_acp_cwd();
    write_workspace_mcp_json(&cwd);
    if prev_spawn == name {
        return;
    }
    if ACP_TURN_ACTIVE.load(Ordering::SeqCst) {
        // Hub `/api/mcp` routes via ACP_PROJECT. Do not kill Kiro after the
        // turn — that wiped session memory on every create/bind.
        tracing::debug!(
            project = ?name,
            prev_spawn = ?prev_spawn,
            "ACP project bound mid-turn via ACP_PROJECT (no respawn)"
        );
        return;
    }
    reset_acp();
    if let Ok(mut g) = ACP_SPAWN_PROJECT.lock() {
        *g = name;
    }
    tracing::info!(
        project = ?ACP_PROJECT.lock().ok().and_then(|g| g.clone()),
        "ACP project scope changed — workspace mcp rewritten, will respawn"
    );
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
    on_chunk: impl FnMut(&str),
) -> Result<AcpTurnResult, String> {
    prompt_acp_streaming_media(text, &AcpMedia::default(), on_chunk)
}

/// Like [`prompt_acp_streaming`], plus raster images as ACP content blocks
/// (diagrams dropped on the runtime agent chat).
pub fn prompt_acp_streaming_media(
    text: &str,
    media: &AcpMedia,
    mut on_chunk: impl FnMut(&str),
) -> Result<AcpTurnResult, String> {
    struct TurnGuard;
    impl Drop for TurnGuard {
        fn drop(&mut self) {
            ACP_TURN_ACTIVE.store(false, Ordering::SeqCst);
        }
    }

    ACP_TURN_ACTIVE.store(true, Ordering::SeqCst);
    let _turn = TurnGuard;
    let result = prompt_acp_streaming_media_locked(text, media, &mut on_chunk);
    drop(_turn);
    if ACP_DEFER_RESET.swap(false, Ordering::SeqCst) {
        reset_acp();
        if let Ok(mut g) = ACP_SPAWN_PROJECT.lock() {
            *g = None;
        }
        tracing::info!("ACP child dropped after mid-turn project change — next prompt respawns");
    }
    result
}

fn prompt_acp_streaming_media_locked(
    text: &str,
    media: &AcpMedia,
    on_chunk: &mut dyn FnMut(&str),
) -> Result<AcpTurnResult, String> {
    let timeout = Duration::from_secs(timeout_secs());
    let mut guard = ACP.lock().map_err(|e| format!("ACP lock poisoned: {e}"))?;
    if guard.is_none() {
        let cwd = resolve_acp_cwd();
        write_workspace_mcp_json(&cwd);
        if let Ok(mut g) = ACP_SPAWN_PROJECT.lock() {
            *g = ACP_PROJECT.lock().ok().and_then(|p| p.clone());
        }
        *guard = Some(AcpProcess::spawn()?);
    }
    let proc = guard.as_mut().unwrap();
    match proc.prompt_streaming(text, media, timeout, Some(on_chunk)) {
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
    let explicit = std::env::var("VEIL_ACP_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let from_name = std::env::var("VEIL_MODEL_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty());
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
        "args": std::env::var("VEIL_ACP_ARGS").unwrap_or_else(|_| "acp".into()),
        "cwd": std::env::var("VEIL_ACP_CWD").ok(),
        "model": model_arg.clone().unwrap_or_else(|| "(kiro default / auto)".into()),
        "model_flag": model_arg,
        "timeout_secs": timeout_secs(),
        "rig": false,
        "acp": true,
        "hint": "Set VEIL_ACP_MODEL to a real Kiro model id, or omit for default. Do not use VEIL_MODEL_NAME=kiro.",
    })
}

/// Force-drop the agent process (tests / config change / next-turn respawn).
///
/// Mid-turn this only sets [`ACP_DEFER_RESET`] — taking [`ACP`] while
/// [`prompt_acp_streaming_media`] holds it deadlocks create_project MCP.
pub fn reset_acp() {
    if ACP_TURN_ACTIVE.load(Ordering::SeqCst) {
        ACP_DEFER_RESET.store(true, Ordering::SeqCst);
        tracing::info!("ACP reset deferred until turn ends (live prompt holds the process lock)");
        return;
    }
    if let Ok(mut g) = ACP.lock() {
        *g = None;
    }
    ACP_DEFER_RESET.store(false, Ordering::SeqCst);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::Instant;

    static TEST: StdMutex<()> = StdMutex::new(());

    #[test]
    fn collect_update_coalesces_tool_call_and_update_into_one_record() {
        let mut text = Vec::new();
        let mut tools = Vec::new();
        let mut records = Vec::new();
        // Initial tool_call: name + rawInput, pending.
        let call = json!({
            "method": "session/update",
            "params": { "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": "tc_1",
                "title": "write_source",
                "kind": "edit",
                "status": "pending",
                "rawInput": { "path": "a.veil", "content": "..." }
            }}
        });
        collect_update_full(&call, &mut text, &mut tools, &mut records);
        // Later tool_call_update: completed + content result.
        let upd = json!({
            "method": "session/update",
            "params": { "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tc_1",
                "status": "completed",
                "content": [{ "type": "text", "text": "wrote 12 lines" }]
            }}
        });
        collect_update_full(&upd, &mut text, &mut tools, &mut records);

        // ONE coalesced record; name-hint emitted once (not on the update).
        assert_eq!(records.len(), 1, "updates must coalesce by toolCallId");
        assert_eq!(tools, vec!["write_source".to_string()]);
        let r = &records[0];
        assert_eq!(r.name, "write_source");
        assert_eq!(r.tool_call_id, "tc_1");
        assert_eq!(r.status.as_deref(), Some("completed"));
        assert_eq!(r.kind.as_deref(), Some("edit"));
        assert_eq!(
            r.input.as_ref().and_then(|v| v.get("path")).and_then(|v| v.as_str()),
            Some("a.veil")
        );
        assert!(r.content.contains("wrote 12 lines"));
    }

    fn reset_turn_flags() {
        ACP_TURN_ACTIVE.store(false, Ordering::SeqCst);
        ACP_DEFER_RESET.store(false, Ordering::SeqCst);
    }

    #[test]
    fn reset_acp_during_turn_does_not_take_process_lock() {
        let _t = TEST.lock().unwrap();
        reset_turn_flags();
        ACP_TURN_ACTIVE.store(true, Ordering::SeqCst);

        // Hold ACP as the live prompt does. reset_acp must return immediately.
        let hold = thread::spawn(|| {
            let _g = ACP.lock().unwrap();
            thread::sleep(Duration::from_millis(400));
        });
        // Give the holder time to acquire
        thread::sleep(Duration::from_millis(30));
        let start = Instant::now();
        reset_acp();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(150),
            "reset_acp blocked {elapsed:?} — would deadlock create_project MCP"
        );
        assert!(ACP_DEFER_RESET.load(Ordering::SeqCst));
        hold.join().unwrap();
        reset_turn_flags();
    }

    #[test]
    fn ensure_scope_mid_turn_sets_project_without_reset() {
        let _t = TEST.lock().unwrap();
        reset_turn_flags();
        set_acp_project(Some("agent-core".into()));
        if let Ok(mut g) = ACP_SPAWN_PROJECT.lock() {
            *g = Some("agent-core".into());
        }
        ACP_TURN_ACTIVE.store(true, Ordering::SeqCst);

        let hold = thread::spawn(|| {
            let _g = ACP.lock().unwrap();
            thread::sleep(Duration::from_millis(250));
        });
        thread::sleep(Duration::from_millis(20));
        let start = Instant::now();
        ensure_acp_project_scope(Some("shop".into()));
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(150),
            "ensure_acp_project_scope blocked {elapsed:?}"
        );
        assert_eq!(get_acp_project().as_deref(), Some("shop"));
        assert!(
            !ACP_DEFER_RESET.load(Ordering::SeqCst),
            "mid-turn bind must not schedule a Kiro kill"
        );
        let spawn = ACP_SPAWN_PROJECT.lock().ok().and_then(|g| g.clone());
        assert_eq!(spawn.as_deref(), Some("agent-core"));
        hold.join().unwrap();
        reset_turn_flags();
        set_acp_project(None);
        if let Ok(mut g) = ACP_SPAWN_PROJECT.lock() {
            *g = None;
        }
    }

    #[test]
    fn sandbox_dir_is_not_a_source_checkout() {
        let dir = ensure_acp_sandbox();
        let s = dir.to_string_lossy();
        assert!(s.contains("veil-acp-cwd"), "sandbox={s}");
        assert!(
            !s.contains("veil-ws/"),
            "sandbox must not be session checkout: {s}"
        );
        assert!(
            !s.contains("veil-s3-ws"),
            "sandbox must not be S3 materialize: {s}"
        );
        assert!(dir.join("README.txt").is_file());
        assert!(!dir.join("veil.toml").is_file());
        assert!(!dir.join("main.veil").is_file());
    }

    #[test]
    fn fs_and_terminal_acp_methods_are_refused() {
        let fs = acp_host_method_refusal("fs/readTextFile");
        assert!(fs.contains("veil-ide-tools"), "{fs}");
        assert!(fs.contains("stub_search"), "{fs}");
        assert!(fs.contains("reference_"), "{fs}");
        let term = acp_host_method_refusal("terminal/create");
        assert!(term.contains("Do not grep/sed"), "{term}");
        let other = acp_host_method_refusal("session/foo");
        assert!(other.contains("not supported"), "{other}");
    }

    #[test]
    fn remote_source_mode_seats_kiro_in_sandbox() {
        let cwd = resolve_acp_cwd_inner(true);
        assert!(
            cwd.contains("veil-acp-cwd"),
            "s3 mode cwd must be sandbox, got {cwd}"
        );
        assert!(!cwd.contains("veil-s3-ws"), "got {cwd}");
    }

    #[test]
    fn inner_session_tag_is_recognizable_and_scoped() {
        let tag = inner_session_tag("shop", "abc-123");
        assert_eq!(tag, "veil-runtime-inner:shop:abc-123");
        // Prefix is stable so a reader can match on `veil-runtime-inner:` alone.
        assert!(tag.starts_with("veil-runtime-inner:"), "{tag}");
        // Hub-scoped (no project) still tags unambiguously.
        let none = inner_session_tag("(none)", "sid-9");
        assert_eq!(none, "veil-runtime-inner:(none):sid-9");
    }

    #[test]
    fn record_inner_session_writes_sidecar_and_log() {
        let _t = TEST.lock().unwrap();
        // Isolate HOME + TMP so we don't touch the real Kiro sessions dir.
        let tmp = std::env::temp_dir().join(format!("veil-acp-tag-test-{}", std::process::id()));
        let home = tmp.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let prev_home = std::env::var("HOME").ok();
        let prev_tmp = std::env::var("TMPDIR").ok();
        // SAFETY: single-threaded test region guarded by TEST mutex.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("TMPDIR", &tmp);
        }
        set_acp_project(Some("shop".into()));

        let sid = "sess-tag-1234";
        record_inner_session(sid);

        // 1) Sidecar next to Kiro session files.
        let sidecar = home
            .join(".kiro/sessions/cli")
            .join(format!("{sid}.veil-inner.json"));
        assert!(sidecar.is_file(), "missing sidecar: {}", sidecar.display());
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(doc["inner_agent"], serde_json::json!(true));
        assert_eq!(doc["kiro_session_id"], serde_json::json!(sid));
        assert_eq!(doc["project"], serde_json::json!("shop"));
        assert_eq!(doc["tag"], serde_json::json!("veil-runtime-inner:shop:sess-tag-1234"));
        assert!(doc["created_at"].as_str().is_some());
        assert!(doc["runtime_pid"].as_u64().is_some());

        // 2) Append-only audit log in the ACP sandbox.
        let log = tmp.join("veil-acp-cwd").join(".veil-inner-sessions.jsonl");
        assert!(log.is_file(), "missing log: {}", log.display());
        let line = std::fs::read_to_string(&log).unwrap();
        assert!(line.contains(sid), "log missing session id: {line}");
        assert!(line.contains("veil-runtime-inner:shop"), "log missing tag: {line}");

        // Restore env.
        set_acp_project(None);
        unsafe {
            match prev_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            match prev_tmp {
                Some(t) => std::env::set_var("TMPDIR", t),
                None => std::env::remove_var("TMPDIR"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn apply_inner_agent_env_sets_markers() {
        let _t = TEST.lock().unwrap();
        set_acp_project(Some("relay".into()));
        let mut cmd = Command::new("true");
        apply_inner_agent_env(&mut cmd);
        // Command exposes configured envs via get_envs().
        let envs: std::collections::HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|s| s.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert_eq!(envs.get("VEIL_INNER_AGENT").unwrap().as_deref(), Some("1"));
        assert_eq!(
            envs.get("VEIL_INNER_PROJECT").unwrap().as_deref(),
            Some("relay")
        );
        assert_eq!(
            envs.get("VEIL_INNER_SESSION_TAG").unwrap().as_deref(),
            Some("veil-runtime-inner:relay")
        );
        set_acp_project(None);
    }
}
