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

fn is_platform_ux_tool(name: &str) -> bool {
    crate::platform_tools::is_platform_tool(name)
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
    let mut tools = vec![
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
            "description": "Replace the entire active file source. Always call veil_check afterward. Pass rationales: map of construct name → short why (one line each) so the PR Wizard shows agent intent next to each structural change.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Full new source text for the active file"
                    },
                    "rationales": {
                        "type": "object",
                        "description": "Optional map constructName → short intent/why (e.g. {\"Order\": \"Agg holding line items and status\"}). Shown in PR Wizard.",
                        "additionalProperties": { "type": "string" }
                    },
                    "rationale": {
                        "type": "string",
                        "description": "Optional single package-level why when rationales map is omitted"
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
        json!({
            "name": "stub_list",
            "description": "List project + platform .stub catalog (version, origin, sparse). NEVER hand-write full SDK stubs — use stub_install or stub_gen.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "stub_get",
            "description": "Resolve a .stub by crate use-name (project stubs/ first, then platform). Returns metadata + content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Crate use-name (reqwest, aws_sdk_s3)" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "stub_gen",
            "description": "Generate a .stub from crates.io via rustdoc (veil stub-gen). REQUIRED instead of inventing SDK APIs by hand. Writes to project stubs/ by default.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "crate_name": { "type": "string", "description": "Cargo crate name" },
                    "features": { "type": "array", "items": { "type": "string" } },
                    "write": { "type": "boolean", "description": "Write to project stubs/ (default true)" }
                },
                "required": ["crate_name"]
            }
        }),
        json!({
            "name": "stub_install",
            "description": "Copy a platform catalog stub into the project stubs/ directory (pin). Prefer for common SDKs before stub_gen.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Platform stub name" }
                },
                "required": ["name"]
            }
        }),
        // Durable session workspace tools (path-jailed, S3 write-through)
        json!({
            "name": "ws_list",
            "description": "List files under the durable session workspace (full tree, not only serve-set packages). Paths relative to project root.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative directory (default '')" },
                    "max": { "type": "integer", "description": "Max entries (default 500)" },
                    "session_id": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "ws_read",
            "description": "Read a file from the session workspace (local hot checkout; no S3 GET).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "max_bytes": { "type": "integer" },
                    "session_id": { "type": "string" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "ws_write",
            "description": "Write a file in the session workspace and durable write-through to S3. Prefer ws_str_replace for small edits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                    "if_match": { "type": "string", "description": "Optional etag for CAS" },
                    "session_id": { "type": "string" }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "ws_str_replace",
            "description": "Replace a unique string in a workspace file (agent-friendly sed). Fails if not unique. Write-through to S3.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                    "if_match": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["path", "old", "new"]
            }
        }),
        json!({
            "name": "ws_grep",
            "description": "Regex search across the session workspace (local only; efficient).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string", "description": "Optional path glob filter" },
                    "max_matches": { "type": "integer" },
                    "session_id": { "type": "string" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
            "name": "ws_rm",
            "description": "Remove a file from the session workspace and S3.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "ws_pull",
            "description": "Incremental pull from S3 into the session workspace (no --delete).",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": []
            }
        }),
        json!({
            "name": "ws_reset",
            "description": "Hard reset session workspace from S3 (sync --delete). Discards local-only files.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": []
            }
        }),
        // Git-shaped session workflow (branch / commit / merge)
        json!({
            "name": "session_status",
            "description": "Git-shaped coding session status: branch_name, base_branch, uncommitted, revision, head_commit, draft_mode. Call first before multi-step product work. Prefer this over guessing main vs feature branch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional; defaults to active project session" }
                },
                "required": []
            }
        }),
        json!({
            "name": "create_branch",
            "description": "Create an isolated feature branch (draft session) for multi-step work. Use automatically for fix campaigns / multi-file edits — do not thrash main. Returns new session_id; subsequent tools use it as the active work line. Name like fix-relay-opts or feat-auth.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "branch_name": { "type": "string", "description": "Feature branch name (e.g. fix-type-mismatch)" },
                    "slug": { "type": "string", "description": "Project slug (default: current project)" },
                    "base_branch": { "type": "string", "description": "Base to fork from (default main)" }
                },
                "required": ["branch_name"]
            }
        }),
        json!({
            "name": "session_commit",
            "description": "Create a named commit (checkpoint) of the working tree with a message. Call after each successful slice: write_source → veil_check improved → session_commit. Autosave is NOT a commit. Message e.g. 'fix: Opt force-present in HandleExecute'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Commit message describing the slice" },
                    "session_id": { "type": "string" }
                },
                "required": ["message"]
            }
        }),
        json!({
            "name": "list_commits",
            "description": "List named commits on the current coding session branch (newest first).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "merge_branch",
            "description": "DISABLED by default. Lands session work on main without PR review. Prefer create_pr + submit_pr; human uses PR Wizard → Approve → Merge. Only call if operator explicitly said merge AND pass force:true (or host VEIL_ALLOW_SESSION_MERGE=1).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "force": {
                        "type": "boolean",
                        "description": "Required true when operator explicitly asked to session-merge (escape hatch)"
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "switch_main",
            "description": "Switch active work line back to the sticky mainline session for the project (after merge or to abandon a feature branch). Does not delete the branch session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string", "description": "Project slug (default: current project)" }
                },
                "required": []
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
            "description": "JSON routes before inventing paths. source=auto|generated|ir (default auto: generated harness if present, else package IR endpoints). Use ir when gen failed (ACS-011).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "enum": ["auto", "generated", "ir"],
                        "description": "auto (default) | generated | ir"
                    }
                },
                "required": []
            }
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
    ];
    // Platform UX (create_project, SDLC, deploy, nav) — full product surface
    tools.extend(crate::platform_tools::tool_definitions());
    // Mind Palace wiki tools (when MIND_PALACE=1 + AWS configured)
    tools.extend([
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
    ]);
    tools
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
    // Platform UX tools (no project required) — agent controls the runtime dashboard.
    if is_platform_ux_tool(tool_name) {
        let result = crate::platform_tools::dispatch(tool_name, arguments).await?;
        // After open/create, bind hub provider so the *next* MCP tool call sees files.
        if matches!(
            tool_name,
            "open_ide"
                | "open_project"
                | "switch_project"
                | "create_project"
                | "create_repo"
        ) {
            if let Some(slug) =
                crate::agent_scope::slug_from_tool(tool_name, arguments, &result)
            {
                match crate::agent_scope::prepare_project(&slug, Some(provider.as_ref())) {
                    Ok(info) => {
                        // Merge bind info into tool result for the model
                        if let Ok(mut v) = serde_json::from_str::<Value>(&result) {
                            v["bound"] = json!(true);
                            v["session"] = info;
                            return Ok(v.to_string());
                        }
                    }
                    Err(e) => {
                        tracing::warn!(%slug, error = %e, "post-{tool_name} prepare_project failed");
                    }
                }
            }
        }
        return Ok(result);
    }

    // Hub `/api/mcp` may run without middleware project scope. Prefer task-local,
    // then ACP turn project, then optional `project` / `project_id` tool arg.
    let scoped = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok();
    if scoped.is_none() {
        let fallback = crate::acp::get_acp_project()
            .or_else(|| {
                arguments
                    .get("project")
                    .or_else(|| arguments.get("project_id"))
                    .and_then(|v| v.as_str())
                    .and_then(crate::agent_scope::normalize_slug)
            });
        if let Some(name) = fallback {
            // Ensure hub has a live session provider for this slug (not empty)
            let _ = crate::agent_scope::prepare_project(&name, Some(provider.as_ref()));
            return crate::provider::hub::CURRENT_PROJECT
                .scope(name, dispatch_tool_scoped(provider, tool_name, arguments))
                .await;
        }
        // Clear error for coding tools instead of silent empty list_files
        if matches!(
            tool_name,
            "list_files"
                | "read_source"
                | "write_source"
                | "create_file"
                | "select_file"
                | "veil_check"
                | "veil_outline"
        ) {
            return Err(
                "project scope missing — call open_ide({project:\"<slug>\"}) first \
                 (or ensure ChatRequest.focus.project / open a product IDE). \
                 list_files empty does NOT mean the product is empty."
                    .into(),
            );
        }
    }

    dispatch_tool_scoped(provider, tool_name, arguments).await
}

async fn dispatch_tool_scoped<P: SourceProvider>(
    provider: &Arc<P>,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, String> {
    // Workspace tools (durable session)
    if tool_name.starts_with("ws_") {
        return dispatch_ws_tool(tool_name, arguments).await;
    }

    // Git-shaped session tools
    if matches!(
        tool_name,
        "session_status"
            | "create_branch"
            | "session_commit"
            | "list_commits"
            | "merge_branch"
            | "switch_main"
    ) {
        return dispatch_session_git_tool(provider, tool_name, arguments).await;
    }

    // Runtime observability tools need project_root only (no active source).
    let proj = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok();
    if matches!(
        tool_name,
        "dev_status"
            | "dev_logs"
            | "read_generated"
            | "http_request"
            | "dev_restart"
            | "smoke_status"
    ) {
        let root = provider
            .project_root()
            .ok_or_else(|| "no project root — open a project first".to_string())?;
        return dispatch_runtime_tool(&root, tool_name, arguments, proj.as_deref()).await;
    }

    // Stub catalog tools — do not require active package source.
    match tool_name {
        "stub_list" => {
            let root = provider.project_root();
            return Ok(crate::stub_ops::tool_list_text(root.as_deref()));
        }
        "stub_get" => {
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "stub_get requires 'name'".to_string())?;
            let root = provider.project_root();
            let r = crate::stub_ops::get_stub(root.as_deref(), name)?;
            return serde_json::to_string_pretty(&r).map_err(|e| e.to_string());
        }
        "stub_gen" => {
            let crate_name = arguments
                .get("crate_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "stub_gen requires 'crate_name'".to_string())?;
            let features: Vec<String> = arguments
                .get("features")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let write = arguments
                .get("write")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let root = provider.project_root();
            let r = crate::stub_ops::generate_stub(root.as_deref(), crate_name, &features, write)?;
            return Ok(format!(
                "Generated {} @ {} → {:?}\n{}",
                r.entry.name,
                r.entry.version,
                r.entry.path,
                r.entry.notes.join("; ")
            ));
        }
        "stub_install" => {
            let name = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "stub_install requires 'name'".to_string())?;
            let root = provider
                .project_root()
                .ok_or_else(|| "no project root — open a project first".to_string())?;
            let r = crate::stub_ops::install_stub_to_project(&root, name)?;
            return Ok(format!(
                "Installed {} @ {} → {:?}",
                r.entry.name, r.entry.version, r.entry.path
            ));
        }
        _ => {}
    }

    // ACS-011: list_routes may need active source for IR mode
    if tool_name == "list_routes" {
        let root = provider
            .project_root()
            .ok_or_else(|| "no project root — open a project first".to_string())?;
        let mode = agent_runtime_tools::ListRoutesSource::parse(
            arguments.get("source").and_then(|v| v.as_str()),
        )?;
        let source = provider.read_source("").await.ok();
        let registry = provider.registry();
        return agent_runtime_tools::tool_list_routes_with(
            &root,
            source.as_deref(),
            Some(&registry),
            mode,
        );
    }

    let source = provider.read_source("").await.map_err(|e| format!("read_source: {e}"))?;
    let registry = provider.registry();

    match tool_name {
        "veil_check" => {
            let check = rig_tools::run_check(&source, &registry);
            let project = crate::provider::hub::CURRENT_PROJECT
                .try_with(|n| n.clone())
                .ok()
                .or_else(crate::acp::get_acp_project);
            crate::coding_gates::record_host_check_for_project(project.as_deref(), &check);
            let host = crate::coding_gates::parse_check_output(&check);
            Ok(format!(
                "{check}\n\nHOST_CHECK_SEVERITY={} error_count={} warning_count={} (source=host — do not claim clean if errors>0)",
                host.severity, host.error_count, host.warning_count
            ))
        }

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
            let content_raw = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "write_source requires 'content' string argument".to_string())?;
            // Unwrap accidental `ws_read` / platform `read_file` JSON envelopes so we
            // never persist `{"content":…,"path":…}` as a .veil body (→ Sol/LBrace 500).
            let content = crate::file_ops::normalize_source_body(content_raw);
            let content = content.as_str();
            // Per-construct intents for PR Wizard
            let mut rats: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            if let Some(obj) = arguments.get("rationales").and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    if let Some(s) = v.as_str() {
                        if !k.is_empty() && !s.trim().is_empty() {
                            rats.insert(k.clone(), s.trim().to_string());
                        }
                    }
                }
            }
            if let Some(one) = arguments.get("rationale").and_then(|v| v.as_str()) {
                if !one.trim().is_empty() {
                    rats.entry("*".into())
                        .or_insert_with(|| one.trim().to_string());
                }
            }
            if !rats.is_empty() {
                crate::api::record_rationales(rats);
            }
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
            // Backend smoke (gen + cargo check). Can take minutes on cold cargo;
            // chat WS heartbeats keep the agent dock alive meanwhile.
            if let Some(root) = provider.project_root() {
                let proj = crate::provider::hub::CURRENT_PROJECT
                    .try_with(|n| n.clone())
                    .ok();
                tracing::info!(
                    project = ?proj,
                    path = %active_path,
                    "write_source: starting dual-loop smoke (may take a while)"
                );
                let smoke_start = std::time::Instant::now();
                if let Err(smoke_err) =
                    crate::devloop::smoke_agent_write(&root, &active_path, proj.as_deref())
                {
                    tracing::warn!(
                        secs = smoke_start.elapsed().as_secs(),
                        "write_source: smoke FAILED"
                    );
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
                tracing::info!(
                    secs = smoke_start.elapsed().as_secs(),
                    "write_source: smoke OK"
                );
            }
            let check = rig_tools::run_check(content, &registry);
            // MultiProjectProvider::write_source records revision/uncommitted.
            // Surface status so the model is nudged to session_commit (History tab).
            let project = crate::provider::hub::CURRENT_PROJECT
                .try_with(|n| n.clone())
                .ok()
                .or_else(crate::acp::get_acp_project);
            crate::coding_gates::record_host_check_for_project(project.as_deref(), &check);
            let rev = project
                .as_ref()
                .and_then(|p| {
                    crate::session::SessionManager::global()
                        .resolve_for_project(p)
                        .ok()
                        .map(|h| h.revision())
                })
                .unwrap_or(0);
            let host = crate::coding_gates::parse_check_output(&check);
            let must_fix = host.severity == "errors" || host.error_count > 0;
            Ok(format!(
                "Wrote {} bytes to active file ({active_name}). revision={rev} (uncommitted until session_commit)\n\
                 Smoke: backend gen + cargo check OK.\n\
                 Host check: severity={} errors={} warnings={}\n\
                 Next (same turn): review diagnostics below — if you introduced new errors/warnings, fix them NOW before any other task claim.\n\
                 Then session_commit with a short message (include why). When the whole task is done: open PR via create_pr + submit_pr — do NOT merge_branch.\n\
                 MUST_FIX_DIAGNOSTICS={must_fix}\n\
                 HOST_CHECK_SEVERITY={}\n\n{check}",
                content.len(),
                host.severity,
                host.error_count,
                host.warning_count,
                host.severity,
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
async fn resolve_session_for_ws(
    arguments: &Value,
) -> Result<std::sync::Arc<crate::session::SessionHandle>, String> {
    use crate::session::SessionManager;
    if let Some(sid) = arguments
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        let h = SessionManager::global().attach(sid)?;
        SessionManager::global().set_active_for_project(&h.slug(), &h.session_id());
        return Ok(h);
    }
    if let Ok(sid) = crate::session::CURRENT_SESSION.try_with(|s| s.clone()) {
        return SessionManager::global().attach(&sid);
    }
    let slug = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .map_err(|_| {
            "ws_* tools need session_id, X-Veil-Session-Id, or project scope".to_string()
        })?;
    SessionManager::global().resolve_for_project(&slug)
}

fn session_status_json(h: &std::sync::Arc<crate::session::SessionHandle>) -> Value {
    let meta = h.snapshot_meta();
    let uncommitted = h.has_uncommitted();
    json!({
        "session_id": meta.session_id,
        "slug": meta.slug,
        "branch_name": meta.branch_name.clone().unwrap_or_else(|| {
            if meta.draft_mode { "work".into() } else { meta.branch.clone() }
        }),
        "base_branch": meta.base_branch.clone().unwrap_or_else(|| meta.branch.clone()),
        "draft_mode": meta.draft_mode,
        "on_feature_branch": meta.draft_mode,
        "revision": meta.revision,
        "committed_revision": meta.committed_revision,
        "head_commit": meta.head_commit,
        "uncommitted": uncommitted,
        "dirty_files": meta.dirty,
        "writes_since_commit": meta.writes_since_commit,
        "work_dir": h.work_dir.to_string_lossy(),
        "active_pr_id": meta.active_pr_id,
        "host_check": crate::coding_gates::host_check_value(&meta),
    })
}

async fn dispatch_session_git_tool<P: SourceProvider>(
    provider: &Arc<P>,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, String> {
    use crate::session::SessionManager;
    let mgr = SessionManager::global();
    if !crate::session::sessions_enabled() {
        return Err(
            "durable sessions disabled (set VEIL_SESSIONS=1 or VEIL_SOURCE_MODE=s3)".into(),
        );
    }

    match tool_name {
        "session_status" => {
            let h = resolve_session_for_ws(arguments).await?;
            Ok(serde_json::to_string_pretty(&session_status_json(&h)).unwrap_or_default())
        }
        "create_branch" => {
            let branch_name = arguments
                .get("branch_name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "create_branch requires branch_name".to_string())?;
            let slug = arguments
                .get("slug")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| crate::provider::hub::CURRENT_PROJECT.try_with(|n| n.clone()).ok())
                .ok_or_else(|| {
                    "create_branch needs project scope or slug argument".to_string()
                })?;
            let base = arguments.get("base_branch").and_then(|v| v.as_str());
            let h = mgr.create_branch(&slug, base, true, Some(branch_name))?;
            mgr.set_active_for_project(&slug, &h.session_id());
            // Immediate hub rebind so in-process / same-turn tools hit the branch tree.
            provider.bind_coding_session(&slug, h.provider.clone());
            let status = session_status_json(&h);
            Ok(serde_json::to_string_pretty(&json!({
                "ok": true,
                "switched": true,
                "codingSessionId": h.session_id(),
                "session_id": h.session_id(),
                "branch_name": branch_name,
                "message": format!(
                    "Created branch '{branch_name}'. Active work line is now this session. \
                     Continue with write_source / veil_check / session_commit on this branch."
                ),
                "session": status,
            }))
            .unwrap_or_default())
        }
        "session_commit" => {
            let message = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "session_commit requires message".to_string())?;
            let h = resolve_session_for_ws(arguments).await?;
            crate::coding_gates::gate_session_commit(&h)?;
            let c = h.commit(message)?;
            Ok(serde_json::to_string_pretty(&json!({
                "ok": true,
                "commit": {
                    "commit_id": c.commit_id,
                    "message": c.message,
                    "revision": c.revision,
                    "branch_name": c.branch_name,
                    "created_at": c.created_at,
                    "parent": c.parent,
                },
                "session": session_status_json(&h),
                "host_check": crate::coding_gates::host_check_value(&h.snapshot_meta()),
                "hint": "Slice committed. Continue edits or when task done open PR: create_pr + submit_pr (do NOT merge).",
            }))
            .unwrap_or_default())
        }
        "list_commits" => {
            let h = resolve_session_for_ws(arguments).await?;
            let commits = crate::session::list_session_commits(&h.session_id())?;
            Ok(serde_json::to_string_pretty(&json!({
                "session_id": h.session_id(),
                "commits": commits,
            }))
            .unwrap_or_default())
        }
        "merge_branch" => {
            let force = arguments
                .get("force")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let h = resolve_session_for_ws(arguments).await?;
            let slug = h.slug();
            // Default: refuse — human lands via PR Wizard after review.
            let v = h.merge_to_base_gated(force)?;
            if let Ok(main) = mgr.open_mainline(&slug) {
                mgr.set_active_for_project(&slug, &main.session_id());
                provider.bind_coding_session(&slug, main.provider.clone());
            }
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }
        "switch_main" => {
            let slug = arguments
                .get("slug")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| crate::provider::hub::CURRENT_PROJECT.try_with(|n| n.clone()).ok())
                .ok_or_else(|| "switch_main needs project scope or slug".to_string())?;
            // Drop preferred feature branch + any warm mainline handle so cold
            // attach re-syncs S3 (picks up merges).
            mgr.clear_active_for_project(&slug);
            if let Ok(list) = crate::session::list_sessions_for_user(&crate::session::current_user_id())
            {
                for m in list
                    .into_iter()
                    .filter(|m| m.slug == slug && !m.draft_mode)
                {
                    mgr.drop_handle(&m.session_id);
                }
            }
            let h = mgr.open_mainline(&slug)?;
            mgr.set_active_for_project(&slug, &h.session_id());
            provider.bind_coding_session(&slug, h.provider.clone());
            Ok(serde_json::to_string_pretty(&json!({
                "ok": true,
                "switched": true,
                "codingSessionId": h.session_id(),
                "session_id": h.session_id(),
                "session": session_status_json(&h),
                "message": "Switched to mainline session (synced from S3)",
            }))
            .unwrap_or_default())
        }
        other => Err(format!("unknown session git tool: {other}")),
    }
}

async fn dispatch_ws_tool(tool_name: &str, arguments: &Value) -> Result<String, String> {
    use crate::session::WorkspaceFs;
    let h = resolve_session_for_ws(arguments).await?;
    match tool_name {
        "ws_list" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let max = arguments
                .get("max")
                .and_then(|v| v.as_u64())
                .unwrap_or(500) as usize;
            let files = h.fs.list(path, max)?;
            Ok(serde_json::to_string_pretty(&serde_json::json!({ "files": files }))
                .unwrap_or_default())
        }
        "ws_read" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_read requires path".to_string())?;
            let max = arguments
                .get("max_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(200_000) as usize;
            let content = h.fs.read(path, max)?;
            Ok(content)
        }
        "ws_write" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_write requires path".to_string())?;
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_write requires content".to_string())?;
            let if_match = arguments.get("if_match").and_then(|v| v.as_str());
            let r = h.fs.write(path, content, if_match)?;
            let rev = h.bump_revision(path, r.etag.clone());
            crate::revision::bus().publish(r.bytes, path, "ws_write");
            Ok(format!(
                "wrote {} ({} bytes) revision={rev} etag={}",
                r.path,
                r.bytes,
                r.etag.unwrap_or_default()
            ))
        }
        "ws_str_replace" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_str_replace requires path".to_string())?;
            let old = arguments
                .get("old")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_str_replace requires old".to_string())?;
            let new = arguments
                .get("new")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_str_replace requires new".to_string())?;
            let if_match = arguments.get("if_match").and_then(|v| v.as_str());
            let r = h.fs.str_replace(path, old, new, if_match)?;
            let rev = h.bump_revision(path, r.etag.clone());
            crate::revision::bus().publish(r.bytes, path, "ws_str_replace");
            Ok(format!(
                "replaced in {} ({} bytes) revision={rev}",
                r.path, r.bytes
            ))
        }
        "ws_grep" => {
            let pattern = arguments
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_grep requires pattern".to_string())?;
            let path = arguments.get("path").and_then(|v| v.as_str());
            let max = arguments
                .get("max_matches")
                .and_then(|v| v.as_u64())
                .unwrap_or(50) as usize;
            let hits = h.fs.grep(pattern, path, max)?;
            Ok(serde_json::to_string_pretty(&hits).unwrap_or_default())
        }
        "ws_rm" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "ws_rm requires path".to_string())?;
            h.fs.rm(path)?;
            h.bump_revision(path, None);
            Ok(format!("removed {path}"))
        }
        "ws_pull" => {
            h.pull_remote()?;
            Ok("pull_remote ok".into())
        }
        "ws_reset" => {
            h.reset_to_remote()?;
            Ok("reset_to_remote ok".into())
        }
        other => Err(format!("unknown workspace tool: {other}")),
    }
}

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
