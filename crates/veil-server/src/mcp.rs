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

/// Runtime shell UX tools — agent drives the dashboard (navigate, SDLC list, etc.).
/// Return JSON always includes `navigation` so the SPA can `goto` without hard-coded chips.
fn dispatch_platform_ux_tool(tool_name: &str, arguments: &Value) -> Result<String, String> {
    // Prefer same-origin ProductHost (VEIL_PORT / PORT); legacy veil_bin used 3000.
    let runtime_base = std::env::var("VEIL_RUNTIME_API")
        .or_else(|_| {
            let port = std::env::var("VEIL_PORT")
                .or_else(|_| std::env::var("PORT"))
                .unwrap_or_else(|_| "8080".into());
            Ok(format!("http://127.0.0.1:{port}"))
        })
        .unwrap_or_else(|_: std::env::VarError| "http://127.0.0.1:8080".into())
        .trim_end_matches('/')
        .to_string();

    match tool_name {
        "navigate_to" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("/dashboard");
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            };
            Ok(json!({
                "ok": true,
                "summary": format!("Navigate to {path}"),
                "navigation": { "action": "goto", "path": path }
            })
            .to_string())
        }
        "list_changes" | "open_changes" => Ok(json!({
            "ok": true,
            "summary": "Open change requests (SDLC)",
            "api": format!("{runtime_base}/api/change_requests"),
            "navigation": { "action": "goto", "path": "/changes" }
        })
        .to_string()),
        "create_change" | "open_create_change" => Ok(json!({
            "ok": true,
            "summary": "Open create change request form",
            "navigation": { "action": "goto", "path": "/changes/new" }
        })
        .to_string()),
        "list_projects" | "open_projects" => Ok(json!({
            "ok": true,
            "summary": "Open projects list",
            "api": format!("{runtime_base}/api/repos"),
            "navigation": { "action": "goto", "path": "/projects" }
        })
        .to_string()),
        "open_project" | "open_ide" | "switch_project" => {
            let project = arguments
                .get("project")
                .or_else(|| arguments.get("slug"))
                .or_else(|| arguments.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if project.is_empty() {
                return Ok(json!({
                    "ok": true,
                    "summary": "Open projects (no project specified)",
                    "navigation": { "action": "goto", "path": "/projects" }
                })
                .to_string());
            }
            // open_ide → shell embed (`/projects/{id}/ide`); open_project → detail page.
            let path = if tool_name == "open_ide" {
                format!("/projects/{project}/ide")
            } else {
                format!("/projects/{project}")
            };
            let action = if tool_name == "open_ide" {
                "open-ide"
            } else if tool_name == "switch_project" {
                "switch-project"
            } else {
                "goto"
            };
            let summary = if tool_name == "open_ide" {
                format!("Open {project} in IDE (in-shell embed)")
            } else {
                format!("Open project {project}")
            };
            Ok(json!({
                "ok": true,
                "summary": summary,
                "navigation": { "action": action, "path": path, "project": project }
            })
            .to_string())
        }
        "open_deploy" => Ok(json!({
            "ok": true,
            "summary": "Open deploy view",
            "navigation": { "action": "goto", "path": "/deploy" }
        })
        .to_string()),
        "open_registry" => Ok(json!({
            "ok": true,
            "summary": "Open registry",
            "navigation": { "action": "goto", "path": "/registry" }
        })
        .to_string()),
        "open_dashboard" => Ok(json!({
            "ok": true,
            "summary": "Open dashboard",
            "navigation": { "action": "goto", "path": "/dashboard" }
        })
        .to_string()),
        "open_config" => Ok(json!({
            "ok": true,
            "summary": "Open runtime config",
            "navigation": { "action": "goto", "path": "/config" }
        })
        .to_string()),
        "get_current_context" => Ok(json!({
            "ok": true,
            "summary": "Context is injected by the runtime UI each turn (page, project, surfaces). Use navigate_to to change pages.",
            "hint": "Call navigate_to / list_changes / open_project to control the UX."
        })
        .to_string()),
        other => Err(format!("unknown platform tool: {other}")),
    }
}

fn is_platform_ux_tool(name: &str) -> bool {
    matches!(
        name,
        "navigate_to"
            | "list_changes"
            | "open_changes"
            | "create_change"
            | "open_create_change"
            | "list_projects"
            | "open_projects"
            | "open_project"
            | "open_ide"
            | "switch_project"
            | "open_deploy"
            | "open_registry"
            | "open_dashboard"
            | "open_config"
            | "get_current_context"
    )
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
            "description": "JSON routes before inventing paths. source=auto|generated|ir (default auto: generated harness if present, else package IR @route/name). Use ir when gen failed (ACS-011).",
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
        // Runtime shell UX control (omnipresent agent — navigate dashboard, SDLC, deploy)
        json!({
            "name": "navigate_to",
            "description": "Navigate the VEIL runtime dashboard SPA to a path so the user sees the right page. Use for any UI destination: /dashboard, /projects, /projects/{id}, /changes, /changes/new, /deploy, /registry, /config, /agents. ALWAYS call this when the user asks to open/show a page or surface.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "SPA path starting with / (e.g. /changes, /projects/relay)"
                    }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "list_changes",
            "description": "Show open change requests (SDLC). Navigates the UI to /changes. Use when the user asks to open changes, review PRs, or list change requests.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "create_change",
            "description": "Open the create-change-request form (/changes/new). Use when the user wants a new PR/change request.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "list_projects",
            "description": "Open the projects list in the runtime dashboard (/projects).",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "open_project",
            "description": "Open a project detail page in the runtime UI (and IDE entry). Pass project id or slug.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project id or slug (e.g. relay)" },
                    "slug": { "type": "string", "description": "Alias for project" },
                    "id": { "type": "string", "description": "Alias for project" }
                },
                "required": []
            }
        }),
        json!({
            "name": "open_ide",
            "description": "Open the dual-loop IDE for a project inside the runtime shell (agent panel stays in the dashboard; path /projects/{id}/ide).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project id or slug" }
                },
                "required": ["project"]
            }
        }),
        json!({
            "name": "open_deploy",
            "description": "Open the deploy surface in the runtime dashboard.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "open_registry",
            "description": "Open the layer/stub registry page.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "open_dashboard",
            "description": "Open the runtime home dashboard.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "open_config",
            "description": "Open runtime configuration page.",
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
    // Platform UX tools (no project required) — agent controls the runtime dashboard.
    if is_platform_ux_tool(tool_name) {
        return dispatch_platform_ux_tool(tool_name, arguments);
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
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            });
        if let Some(name) = fallback {
            return crate::provider::hub::CURRENT_PROJECT
                .scope(name, dispatch_tool_scoped(provider, tool_name, arguments))
                .await;
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
async fn resolve_session_for_ws(
    arguments: &Value,
) -> Result<std::sync::Arc<crate::session::SessionHandle>, String> {
    use crate::session::SessionManager;
    if let Some(sid) = arguments
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return SessionManager::global().attach(sid);
    }
    if let Ok(sid) = crate::session::CURRENT_SESSION.try_with(|s| s.clone()) {
        return SessionManager::global().attach(&sid);
    }
    let slug = crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .map_err(|_| {
            "ws_* tools need session_id, X-Veil-Session-Id, or project scope".to_string()
        })?;
    SessionManager::global().get_or_create_default(&slug)
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
