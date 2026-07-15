//! MCP (Model Context Protocol) server endpoint for VEIL IDE tools.
//!
//! Exposes VEIL tools via MCP Streamable HTTP so external agents (Kiro via ACP)
//! can discover and call them. Registered as a remote MCP server in ACP sessions.
//!
//! Endpoint: `POST /api/mcp` (or `/api/p/{project}/mcp` in multi-project mode)
//!
//! Protocol: MCP Streamable HTTP transport (JSON-RPC 2.0 over POST, JSON responses).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

use crate::agent_runtime_tools;
use crate::provider::SourceProvider;
use crate::rig_tools;

async fn dispatch_runtime_tool(
    project_root: &std::path::Path,
    tool_name: &str,
    arguments: &Value,
    project_name: Option<&str>,
) -> Result<String, String> {
    match tool_name {
        "dev_status" => {
            let name = arguments.get("name").and_then(|v| v.as_str());
            agent_runtime_tools::tool_dev_status(project_root, name, project_name)
        }
        "dev_logs" => {
            let name = arguments.get("name").and_then(|v| v.as_str());
            let tail = arguments
                .get("tail")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            agent_runtime_tools::tool_dev_logs(project_root, name, tail, project_name)
        }
        "read_generated" => {
            let path = arguments.get("path").and_then(|v| v.as_str());
            let what = arguments.get("what").and_then(|v| v.as_str());
            let max_chars = arguments
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let list = arguments
                .get("list")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            agent_runtime_tools::tool_read_generated(project_root, path, what, max_chars, list)
        }
        "list_routes" => agent_runtime_tools::tool_list_routes(project_root),
        "http_request" => {
            let method = arguments.get("method").and_then(|v| v.as_str());
            let path = arguments.get("path").and_then(|v| v.as_str());
            let target = arguments.get("target").and_then(|v| v.as_str());
            let url = arguments.get("url").and_then(|v| v.as_str());
            let body = arguments.get("body").and_then(|v| v.as_str());
            let timeout_ms = arguments.get("timeout_ms").and_then(|v| v.as_u64());
            agent_runtime_tools::tool_http_request(
                project_root,
                method,
                path,
                target,
                url,
                body,
                timeout_ms,
            )
            .await
        }
        "dev_restart" => {
            let name = arguments.get("name").and_then(|v| v.as_str());
            agent_runtime_tools::tool_dev_restart(project_root, name, project_name)
        }
        "smoke_status" => {
            agent_runtime_tools::tool_smoke_status(project_root, project_name)
        }
        other => Err(format!("unknown runtime tool: {other}")),
    }
}

/// MCP protocol version we implement.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Server info returned in initialize response.
fn server_info() -> Value {
    json!({
        "name": "veil-ide-tools",
        "version": "0.1.0"
    })
}

/// MCP tool definitions derived from the VEIL IDE tool set.
fn mcp_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "veil_check",
            "description": "Run the VEIL dual-loop check pipeline (parse, validate, types, escape hatches) on the active package or layer. Call after any edit. Returns a one-line summary plus JSON: { ok, error_count, warning_count, diagnostics: [{ code, severity, message, span?, hint?, node_name? }] }. Prefer fixing by code+span (e.g. type_mismatch, parse_error) instead of rewriting whole files.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "veil_outline",
            "description": "Return a compact IR construct outline (topology) for the active package or layer. Use for navigation and understanding structure before editing.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "read_source",
            "description": "Read the active .veil or .layer source text (truncated if large). Prefer veil_outline + veil_check for overview; use this when you need the actual source text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "max_chars": {
                        "type": "integer",
                        "description": "Max characters to return (default 8000)"
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "write_source",
            "description": "Replace the entire active file source. Use this for writing or rewriting package/layer content. Always call veil_check afterward.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Full new source text for the active file"
                    }
                },
                "required": ["content"]
            }
        }),
        json!({
            "name": "rename_construct",
            "description": "Rename a construct by name via structured edit (preferred over raw text rewrite for renames).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Current construct name" },
                    "to": { "type": "string", "description": "New construct name" }
                },
                "required": ["from", "to"]
            }
        }),
        json!({
            "name": "list_files",
            "description": "List packages and layers in the IDE project. Shows index, name, kind, and which file is active.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "select_file",
            "description": "Switch the active IDE file by index or name. Subsequent tool calls operate on the newly selected file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "description": "File index from list_files" },
                    "name": { "type": "string", "description": "File name (e.g. 'wear_test.veil')" }
                },
                "required": []
            }
        }),
        json!({
            "name": "create_file",
            "description": "Create a new package (.veil) or layer (.layer) in the project. The new file becomes the active file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "File name or stem (e.g. 'engagement' or 'engagement.layer')" },
                    "kind": { "type": "string", "enum": ["package", "layer"], "description": "File type: 'package' (default) or 'layer'" },
                    "content": { "type": "string", "description": "Optional full file body; default is a minimal scaffold" }
                },
                "required": ["name"]
            }
        }),
        // Runtime observability (AGT-020–028)
        json!({
            "name": "dev_status",
            "description": "Dual-loop target status (running/stopped, ports, last_error). Call when unsure if the backend is up or after smoke failures.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Optional target name filter (e.g. backend)" }
                },
                "required": []
            }
        }),
        json!({
            "name": "dev_logs",
            "description": "Read dual-loop gen/check/smoke log lines. After WRITE REJECTED or a 404, call this to see cargo errors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Target name (e.g. backend)" },
                    "tail": { "type": "integer", "description": "Last N lines (default 40, max 200)" }
                },
                "required": []
            }
        }),
        json!({
            "name": "read_generated",
            "description": "Read files under codegen output dirs (veil.toml [[targets]].output). Use what=harness|routes or path= relative under outputs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path under project (must be under a target output)" },
                    "what": { "type": "string", "enum": ["harness", "routes"], "description": "Preset: harness main.rs or route lines" },
                    "max_chars": { "type": "integer" },
                    "list": { "type": "boolean", "description": "List files under path instead of reading" }
                },
                "required": []
            }
        }),
        json!({
            "name": "list_routes",
            "description": "Structured JSON list of axum routes from generated veil_bin harness. Prefer this before inventing API paths.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "http_request",
            "description": "HTTP request to local dual-loop servers only (127.0.0.1 + configured dev_port). Verify /health and APIs after gen/restart.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "method": { "type": "string", "description": "GET (default), POST, PUT, DELETE, …" },
                    "path": { "type": "string", "description": "Path e.g. /health or /api/wear_tests" },
                    "target": { "type": "string", "description": "veil.toml target name (uses its dev_port)" },
                    "url": { "type": "string", "description": "Absolute http://127.0.0.1:PORT/… (optional)" },
                    "body": { "type": "string", "description": "Optional JSON body" },
                    "timeout_ms": { "type": "integer" }
                },
                "required": []
            }
        }),
        json!({
            "name": "dev_restart",
            "description": "Stop and start a dual-loop target so cargo run picks up newly generated code after a successful smoke.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Target name (default: all with dev_command)" }
                },
                "required": []
            }
        }),
        json!({
            "name": "smoke_status",
            "description": "Last check/smoke log excerpt and VEIL_AGENT_SMOKE flag. Use after writes.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        // Mind Palace wiki tools (when MIND_PALACE=1 + AWS configured)
        json!({
            "name": "wiki_search",
            "description": "Semantic search across Mind Palace wiki pages. Call this before answering VEIL platform/language questions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                    "limit": { "type": "integer", "description": "Max results (default 5)" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "wiki_read",
            "description": "Read a wiki page at summary, section, or full detail. Prefer summary first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "level": { "type": "string", "enum": ["summary", "section", "full"] },
                    "section": { "type": "string", "description": "Section heading when level=section" }
                },
                "required": ["slug"]
            }
        }),
        json!({
            "name": "wiki_traverse",
            "description": "Graph walk from a page to neighboring summaries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "depth": { "type": "integer", "description": "Traversal depth (default 2)" }
                },
                "required": ["slug"]
            }
        }),
        json!({
            "name": "wiki_create",
            "description": "Create a new Mind Palace wiki page (platform knowledge, SOPs, decisions).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "slug": { "type": "string", "description": "URL slug, e.g. veil-stubs-and-sdks" },
                    "summary": { "type": "string" },
                    "sections": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "heading": { "type": "string" },
                                "content": { "type": "string" }
                            },
                            "required": ["heading", "content"]
                        }
                    },
                    "page_type": {
                        "type": "string",
                        "enum": ["Index", "Concept", "Entity", "Decision", "Leaf", "Sop", "Skill"]
                    },
                    "links": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Slugs of related pages"
                    }
                },
                "required": ["title", "slug", "summary", "sections", "page_type"]
            }
        }),
        json!({
            "name": "wiki_update",
            "description": "Update an existing wiki page (prefer over creating duplicates).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "title": { "type": "string" },
                    "summary": { "type": "string" },
                    "sections": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "heading": { "type": "string" },
                                "content": { "type": "string" }
                            },
                            "required": ["heading", "content"]
                        }
                    },
                    "links": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["slug"]
            }
        }),
        json!({
            "name": "wiki_list",
            "description": "List wiki pages, optionally filtered by page type.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "page_type": {
                        "type": "string",
                        "enum": ["Index", "Concept", "Entity", "Decision", "Leaf", "Sop", "Skill"]
                    }
                },
                "required": []
            }
        }),
    ]
}

/// Handle a single MCP JSON-RPC request and return the response.
async fn handle_mcp_request<P: SourceProvider>(
    provider: &Arc<P>,
    request: &Value,
) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(json!({}));

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": server_info()
            }
        }),

        "notifications/initialized" => {
            // Client acknowledgement — no response needed for notifications
            Value::Null
        }

        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": mcp_tools()
            }
        }),

        "tools/call" => {
            let tool_name = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));

            let result = dispatch_tool(provider, tool_name, &arguments).await;
            match result {
                Ok(text) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }
                }),
                Err(err) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": err }],
                        "isError": true
                    }
                }),
            }
        }

        "ping" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }),

        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {method}")
            }
        }),
    }
}

/// Dispatch a tool call to the underlying VEIL tool implementation.
async fn dispatch_tool<P: SourceProvider>(
    provider: &Arc<P>,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, String> {
    // Runtime observability tools need project_root only (no active source).
    let proj = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok();
    if matches!(
        tool_name,
        "dev_status"
            | "dev_logs"
            | "read_generated"
            | "list_routes"
            | "http_request"
            | "dev_restart"
            | "smoke_status"
    ) {
        let root = provider
            .project_root()
            .ok_or_else(|| "no project root — open a project first".to_string())?;
        return dispatch_runtime_tool(&root, tool_name, arguments, proj.as_deref()).await;
    }

    let source = provider.read_source("").await.map_err(|e| format!("read_source: {e}"))?;
    let registry = provider.registry();

    match tool_name {
        "veil_check" => Ok(rig_tools::run_check(&source, &registry)),

        "veil_outline" => Ok(rig_tools::run_outline(&source, &registry)),

        "read_source" => {
            let max = arguments
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(8000) as usize;
            if source.len() <= max {
                Ok(source)
            } else {
                Ok(format!(
                    "{}…\n\n[truncated {} / {} chars]",
                    &source[..max],
                    max,
                    source.len()
                ))
            }
        }

        "write_source" => {
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "write_source requires 'content' string argument".to_string())?;
            // Guardrail: verify the new content parses before persisting.
            // If it has parse errors, reject the write and return the errors.
            let tokens = veil_parser::lex(content);
            let parse_result = veil_parser::parse_file_with_registry(&tokens, registry.clone());
            if let Err(errors) = parse_result {
                let err_msg = errors
                    .iter()
                    .take(5)
                    .map(|e| format!("  {e}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(format!(
                    "WRITE REJECTED — parse errors in new content (file NOT saved):\n{err_msg}\n\n\
                     Fix the syntax errors and try again. Do NOT use JavaScript/TypeScript syntax \
                     in VEIL effect/fn bodies. Use VEIL expression forms only."
                ));
            }
            let prev = provider.read_source("").await.ok();
            let files = provider.list_files().await;
            let active_path = files
                .iter()
                .find(|f| f.active)
                .map(|f| f.path.clone())
                .unwrap_or_default();
            let active_name = files
                .iter()
                .find(|f| f.active)
                .map(|f| f.name.clone())
                .unwrap_or_default();
            provider
                .write_source("", content)
                .await
                .map_err(|e| format!("write failed: {e}"))?;
            // Backend smoke (gen + cargo check). Restore file if broken.
            if let Some(root) = provider.project_root() {
                let proj = crate::provider::hub::CURRENT_PROJECT
                    .try_with(|n| n.clone())
                    .ok();
                if let Err(smoke_err) =
                    crate::devloop::smoke_agent_write(&root, &active_path, proj.as_deref())
                {
                    if let Some(prev) = prev {
                        let _ = provider.write_source("", &prev).await;
                        let _ = crate::devloop::smoke_agent_write(
                            &root,
                            &active_path,
                            proj.as_deref(),
                        );
                    }
                    return Ok(format!(
                        "WRITE REJECTED — backend smoke test failed (file restored).\n\
                         Active file: {active_name}\n\n{smoke_err}\n\n\
                         Next: call dev_logs / smoke_status, fix the VEIL, retry write_source.\n\
                         After success: list_routes → dev_restart → http_request."
                    ));
                }
            }
            let check = rig_tools::run_check(content, &registry);
            Ok(format!(
                "Wrote {} bytes to active file.\nSmoke: backend gen + cargo check OK.\n\n{check}",
                content.len()
            ))
        }

        "rename_construct" => {
            let from = arguments
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "rename_construct requires 'from' argument".to_string())?;
            let to = arguments
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "rename_construct requires 'to' argument".to_string())?;
            match rig_tools::apply_rename(&source, &registry, from, to) {
                Ok((new_src, summary)) => {
                    provider
                        .write_source("", &new_src)
                        .await
                        .map_err(|e| format!("write after rename failed: {e}"))?;
                    let check = rig_tools::run_check(&new_src, &registry);
                    Ok(format!("{summary}\n\n{check}"))
                }
                Err(e) => Err(e),
            }
        }

        "list_files" => {
            let files = provider.list_files().await;
            if files.is_empty() {
                return Ok("No files loaded in this project.".into());
            }
            let mut lines = vec!["files:".to_string()];
            for f in &files {
                let mark = if f.active { " ●" } else { "" };
                let kind = f.kind.as_str();
                lines.push(format!(
                    "  [{idx}] {name} ({kind}){mark}",
                    idx = f.index,
                    name = f.name,
                ));
            }
            Ok(lines.join("\n"))
        }

        "select_file" => {
            let files = provider.list_files().await;
            let idx = if let Some(i) = arguments.get("index").and_then(|v| v.as_u64()) {
                i as usize
            } else if let Some(name) = arguments.get("name").and_then(|v| v.as_str()) {
                files
                    .iter()
                    .find(|f| {
                        f.name == name
                            || f.name.trim_end_matches(".veil") == name
                            || f.name.trim_end_matches(".layer") == name
                    })
                    .map(|f| f.index)
                    .ok_or_else(|| format!("no file named '{name}'"))?
            } else {
                return Err("select_file requires 'index' or 'name' argument".into());
            };
            provider
                .set_active(idx)
                .map_err(|e| format!("select_file: {e}"))?;
            let name = files
                .iter()
                .find(|f| f.index == idx)
                .map(|f| f.name.clone())
                .unwrap_or_else(|| format!("#{idx}"));
            Ok(format!("Active file is now '{name}'. Use read_source / veil_check / write_source on it."))
        }

        "create_file" => {
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "create_file requires 'name' argument".to_string())?;
            let kind = arguments
                .get("kind")
                .and_then(|v| v.as_str());
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let created = crate::file_ops::create_file_in_project(
                provider.as_ref(),
                name,
                kind,
                content,
            )
            .await
            .map_err(|e| e.message().to_string())?;
            Ok(format!(
                "Created {} ({}) at {} — now active. Use write_source to set content, then veil_check.",
                created.name,
                created.kind.as_str(),
                created.path
            ))
        }

        // ── Mind Palace wiki (MCP for ACP/Kiro) ──────────────────────────
        name if name.starts_with("wiki_") => dispatch_wiki_tool(tool_name, arguments).await,

        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

/// Dispatch wiki_* tools via Mind Palace Rig Tool impls.
async fn dispatch_wiki_tool(tool_name: &str, arguments: &Value) -> Result<String, String> {
    use rig_core::tool::Tool;

    if !crate::mind_palace_tools::enabled() {
        return Err(
            "Mind Palace is disabled. Set MIND_PALACE=1 and AWS resources (see docs/MIND_PALACE.md)."
                .into(),
        );
    }
    let palace = crate::mind_palace_tools::try_palace()
        .await
        .ok_or_else(|| {
            "Mind Palace failed to initialize — check MIND_PALACE_* env and AWS_PROFILE=dashlx_dev"
                .to_string()
        })?;
    let (search, read, traverse, create, update, list) =
        crate::mind_palace_tools::tools_for_agent(&palace);

    match tool_name {
        "wiki_search" => {
            let args: mind_palace_rig::tools::WikiSearchArgs =
                serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
            let out = search.call(args).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        "wiki_read" => {
            let args: mind_palace_rig::tools::WikiReadArgs =
                serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
            let out = read.call(args).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        "wiki_traverse" => {
            let args: mind_palace_rig::tools::WikiTraverseArgs =
                serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
            let out = traverse.call(args).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        "wiki_create" => {
            let args: mind_palace_rig::tools::WikiCreateArgs =
                serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
            let out = create.call(args).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        "wiki_update" => {
            let args: mind_palace_rig::tools::WikiUpdateArgs =
                serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
            let out = update.call(args).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        "wiki_list" => {
            let args: mind_palace_rig::tools::WikiListArgs =
                serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
            let out = list.call(args).await.map_err(|e| e.to_string())?;
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
        }
        _ => Err(format!("Unknown wiki tool: {tool_name}")),
    }
}

/// Axum handler for `POST /api/mcp` — MCP Streamable HTTP transport.
///
/// Accepts JSON-RPC 2.0 requests (single or batch) and returns JSON responses.
pub async fn post_mcp<P: SourceProvider>(
    State(state): State<Arc<P>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // Handle batch requests (array of JSON-RPC messages)
    if let Some(arr) = body.as_array() {
        let mut responses = Vec::new();
        for req in arr {
            let resp = handle_mcp_request(&state, req).await;
            if !resp.is_null() {
                responses.push(resp);
            }
        }
        if responses.is_empty() {
            return (StatusCode::NO_CONTENT, Json(Value::Null)).into_response();
        }
        return Json(Value::Array(responses)).into_response();
    }

    // Single request
    let resp = handle_mcp_request(&state, &body).await;
    if resp.is_null() {
        // Notification — no response
        (StatusCode::NO_CONTENT, Json(Value::Null)).into_response()
    } else {
        Json(resp).into_response()
    }
}
