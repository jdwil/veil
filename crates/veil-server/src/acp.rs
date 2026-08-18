//! ACP (Agent Client Protocol) client — spawn an external agent (Kiro, etc.).
//!
//! Env:
//! - `VEIL_MODEL_PROVIDER=acp`
//! - `VEIL_ACP_COMMAND` (default `kiro-cli`)
//! - `VEIL_ACP_ARGS` (default `acp --trust-all-tools`)
//! - `VEIL_ACP_CWD` — disk-mode fallback only. **Ignored when
//!   `VEIL_SOURCE_MODE` is s3/prefer_s3** so Kiro cannot grep staged
//!   checkouts under `$TMP/veil-ws` / `$TMP/veil-s3-ws`.
//! - `VEIL_ACP_AGENT` — Kiro agent name (default: `veil` when
//!   `~/.kiro/agents/veil.json` exists; see `config/kiro-agent-veil.json`)
//! - `VEIL_ACP_TIMEOUT_SECS` (default 300)
//!
//! VEIL does **not** rewrite `~/.kiro/agents/hive.json`. Use a dedicated
//! `veil` agent that includes mind-palace/jira **and** `veil-ide-tools`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
        // Idle budget, not a total-turn wall clock. Reset on every ACP line.
        let mut deadline = Instant::now() + timeout;
        let mut text_chunks: Vec<String> = Vec::new();
        let mut tool_hints: Vec<String> = Vec::new();
        loop {
            let line = self.read_line_timeout(deadline)?;
            deadline = Instant::now() + timeout;
            let msg: Value = serde_json::from_str(&line)
                .map_err(|e| format!("ACP JSON parse: {e}: {line}"))?;

            // Streamed session updates (collect text)
            if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
                if method == "session/update" || method.ends_with("/update") {
                    let before_text = text_chunks.len();
                    let before_tools = tool_hints.len();
                    collect_update(&msg, &mut text_chunks, &mut tool_hints);
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
        self.session_id = Some(sid.clone());
        Ok(sid)
    }

    fn prompt(&mut self, text: &str, timeout: Duration) -> Result<AcpTurnResult, String> {
        self.prompt_streaming(text, &AcpMedia::default(), timeout, None)
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
        .or_else(|| {
            std::env::var("PORT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
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
             stub_install, ws_*). Do not grep/sed/cat $TMP/veil-ws or $TMP/veil-s3-ws."
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
             Never inspect $TMP/veil-ws or $TMP/veil-s3-ws.\n",
        );
    }
    dir
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
            map.insert(
                "veil-ide-tools".into(),
                veil_ide_mcp_server_entry(&mcp_url),
            );
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
    let mut guard = ACP
        .lock()
        .map_err(|e| format!("ACP lock poisoned: {e}"))?;
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

// Silence unused Arc import warning path if any
#[allow(dead_code)]
fn _arc_marker() -> Arc<()> {
    Arc::new(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use std::time::Instant;

    static TEST: StdMutex<()> = StdMutex::new(());

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
        assert!(!s.contains("veil-ws/"), "sandbox must not be session checkout: {s}");
        assert!(!s.contains("veil-s3-ws"), "sandbox must not be S3 materialize: {s}");
        assert!(dir.join("README.txt").is_file());
        assert!(!dir.join("veil.toml").is_file());
        assert!(!dir.join("main.veil").is_file());
    }

    #[test]
    fn fs_and_terminal_acp_methods_are_refused() {
        let fs = acp_host_method_refusal("fs/readTextFile");
        assert!(fs.contains("veil-ide-tools"), "{fs}");
        assert!(fs.contains("stub_search"), "{fs}");
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
}
