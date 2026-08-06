//! Platform UX tools — agent drives the full runtime dashboard (projects, SDLC, deploy).
//!
//! These tools call ProductHost platform APIs (`/api/repos`, `/api/change_requests`, …)
//! and return structured JSON that always includes `navigation` when a SPA route applies
//! so the dashboard updates without hard-coded chips.
//!
//! Used by MCP (`mcp.rs`), host short-circuit (`agent.rs`), and Rig (`model.rs`).

use serde_json::{json, Value};

/// Prefer same-origin ProductHost (VEIL_PORT / PORT); legacy veil_bin used 3000.
pub fn runtime_base() -> String {
    std::env::var("VEIL_RUNTIME_API")
        .or_else(|_| {
            let port = std::env::var("VEIL_PORT")
                .or_else(|_| std::env::var("PORT"))
                .unwrap_or_else(|_| "8080".into());
            Ok::<String, std::env::VarError>(format!("http://127.0.0.1:{port}"))
        })
        .unwrap_or_else(|_| "http://127.0.0.1:8080".into())
        .trim_end_matches('/')
        .to_string()
}

fn arg_str(arguments: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = arguments.get(*k).and_then(|v| v.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn arg_bool(arguments: &Value, key: &str, default: bool) -> bool {
    arguments
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

async fn http_json(
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(u16, Value), String> {
    let base = runtime_base();
    let url = if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{base}{path}")
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        other => return Err(format!("unsupported method {other}")),
    };
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.map_err(|e| format!("HTTP {method} {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let val = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
    };
    Ok((status, val))
}

fn ok_status(code: u16) -> bool {
    (200..300).contains(&code)
}

/// Whether this tool name is a platform UX / product tool (no active .veil required).
pub fn is_platform_tool(name: &str) -> bool {
    matches!(
        name,
        "navigate_to"
            | "list_changes"
            | "open_changes"
            | "create_change"
            | "open_create_change"
            | "get_change"
            | "submit_change"
            | "approve_change"
            | "request_changes"
            | "merge_change"
            | "add_comment"
            | "get_change_diff"
            | "list_projects"
            | "open_projects"
            | "create_project"
            | "create_repo"
            | "get_project"
            | "delete_project"
            | "open_project"
            | "open_ide"
            | "switch_project"
            | "open_deploy"
            | "open_registry"
            | "open_dashboard"
            | "open_config"
            | "get_current_context"
            | "list_deploy_environments"
            | "deploy_status"
            | "plan_provision"
            | "provision_project"
            | "get_provision_job"
            | "list_registry_layers"
            | "list_registry_stubs"
            | "search_registry"
            | "get_config"
            | "get_mission"
            | "update_mission"
            | "wait_intent_ack"
    )
}

/// Dispatch a platform tool. Always returns JSON string (or Err).
pub async fn dispatch(tool_name: &str, arguments: &Value) -> Result<String, String> {
    let base = runtime_base();
    match tool_name {
        "navigate_to" => {
            let path = arg_str(arguments, &["path"]).unwrap_or_else(|| "/dashboard".into());
            let path = if path.starts_with('/') {
                path
            } else {
                format!("/{path}")
            };
            let intent = crate::focus::navigate_intent(&path, None);
            Ok(json!({
                "ok": true,
                "summary": format!("Navigate to {path}"),
                "navigation": { "action": "goto", "path": path },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        // ─── Projects ─────────────────────────────────────────────────────
        "list_projects" | "open_projects" => {
            let navigate = arg_bool(arguments, "navigate", true);
            let (status, data) = http_json("GET", "/api/repos", None).await.unwrap_or_else(|e| {
                (0, json!({ "error": e }))
            });
            // Fallback: /api/projects (respects source mode) — never invent disk hub lists in s3
            let (status, data) = if !ok_status(status) || status == 0 {
                match http_json("GET", "/api/projects", None).await {
                    Ok((s, d)) if ok_status(s) => (s, d),
                    Ok((s, d)) => (s, d),
                    Err(e) => {
                        if crate::provider::s3_workspace::allow_disk_project_create() {
                            let dir = crate::config::ensure_projects_dir_exists().ok();
                            if let Some(dir) = dir {
                                match crate::project_layout::list_projects(&dir) {
                                    Ok(list) => (200, json!(list)),
                                    Err(le) => (0, json!({ "error": e, "disk_error": le })),
                                }
                            } else {
                                (0, json!({ "error": e }))
                            }
                        } else {
                            (0, json!({
                                "error": e,
                                "source_mode": "s3",
                                "hint": "list_projects needs GET /api/repos or /api/projects with remote store"
                            }))
                        }
                    }
                }
            } else {
                (status, data)
            };
            let mut out = json!({
                "ok": ok_status(status),
                "summary": if ok_status(status) {
                    "Listed projects".to_string()
                } else {
                    format!("list_projects failed (HTTP {status})")
                },
                "http_status": status,
                "projects": data,
                "api": format!("{base}/api/repos"),
            });
            if navigate {
                out["navigation"] = json!({ "action": "goto", "path": "/projects" });
            }
            Ok(out.to_string())
        }

        "create_project" | "create_repo" => {
            let name = arg_str(arguments, &["name", "project", "slug"])
                .ok_or_else(|| "create_project requires name".to_string())?;
            let description = arg_str(arguments, &["description", "desc"]);
            let open = arg_bool(arguments, "open", true);
            let open_ide = arg_bool(arguments, "open_ide", false);

            // via: "ux" | "server" — explicit wins; pure host short-circuit may set ux.
            let via = arg_str(arguments, &["via"])
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let domain_mode = if via == "ux" {
                crate::focus::DomainMode::Ux
            } else if via == "server" {
                crate::focus::DomainMode::Server
            } else {
                // Default server for ACP/MCP mid-turn safety (follow-on write_source).
                crate::focus::DomainMode::Server
            };

            // ── UX path: no domain yet — Present commits via /api/ux/create_project ──
            if domain_mode == crate::focus::DomainMode::Ux {
                let path = if open_ide {
                    format!("/projects/{name}/ide")
                } else if open {
                    format!("/projects/{name}")
                } else {
                    "/projects".to_string()
                };
                let intent = crate::focus::create_project_intent(
                    &name,
                    description.as_deref(),
                    &path,
                    open_ide,
                    crate::focus::DomainMode::Ux,
                );
                let intent_id = intent
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                crate::focus::register_pending_intent(
                    &intent_id,
                    json!({ "tool": "create_project", "name": name, "pending_ux": true }),
                );
                crate::focus::push_intent_log(json!({
                    "type": "CreateProject",
                    "actor": "agent",
                    "summary": name,
                    "domain": "ux",
                    "intent_id": intent_id,
                    "ts": chrono_ms(),
                }));
                return Ok(json!({
                    "ok": true,
                    "summary": format!(
                        "Scheduled create_project `{name}` — UX will Present form then commit (Agent→UX→Server). Do not re-create. Wait for Present to finish before write_source."
                    ),
                    "slug": name,
                    "pending_ux": true,
                    "intent_id": intent_id,
                    "navigation": { "action": if open_ide { "open-ide" } else { "goto" }, "path": path, "project": name },
                    "intent": intent,
                    "execution": {
                        "domain": "ux",
                        "present": "ux_commit",
                        "note": "Domain runs after Present commit step via POST /api/ux/create_project; FE ACKs via /api/ux/intent_ack"
                    }
                })
                .to_string());
            }

            // ── Server domain path ──
            let result = create_project_domain(&name, description.as_deref()).await?;
            let status = result
                .get("http_status")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u16;
            let slug = result
                .get("slug")
                .and_then(|v| v.as_str())
                .unwrap_or(&name)
                .to_string();
            let path = if open_ide {
                format!("/projects/{slug}/ide")
            } else if open {
                format!("/projects/{slug}")
            } else {
                "/projects".to_string()
            };
            let action = if open_ide { "open-ide" } else { "goto" };
            let intent = if ok_status(status) {
                Some(crate::focus::create_project_intent(
                    &slug,
                    description.as_deref(),
                    &path,
                    open_ide,
                    crate::focus::DomainMode::Server,
                ))
            } else {
                None
            };
            if ok_status(status) {
                crate::focus::push_intent_log(json!({
                    "type": "CreateProject",
                    "actor": "agent",
                    "summary": slug,
                    "domain": "server",
                    "ts": chrono_ms(),
                }));
            }
            let mut out = result;
            out["navigation"] = json!({ "action": action, "path": path, "project": slug });
            out["intent"] = json!(intent);
            out["execution"] = json!({
                "domain": "server",
                "present": if ok_status(status) { "illustrate" } else { "none" },
                "note": "Domain applied on server; UX Present is illustrative. Do not re-create."
            });
            if let Some(s) = out.get("summary").and_then(|v| v.as_str()) {
                if ok_status(status) && !s.contains("UX") {
                    out["summary"] = json!(format!("{s}; UX will present create form choreography"));
                }
            }
            Ok(out.to_string())
        }

        "get_project" => {
            let id = arg_str(arguments, &["project", "slug", "id", "name"])
                .ok_or_else(|| "get_project requires project/slug/id".to_string())?;
            let (status, data) = http_json("GET", &format!("/api/repos/{}", urlencoding_path(&id)), None)
                .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Project `{id}`"),
                "project": data,
                "navigation": { "action": "goto", "path": format!("/projects/{id}"), "project": id }
            })
            .to_string())
        }

        "delete_project" => {
            let id = arg_str(arguments, &["project", "slug", "id", "name"])
                .ok_or_else(|| "delete_project requires project/slug/id".to_string())?;
            let (status, data) =
                http_json("DELETE", &format!("/api/repos/{}", urlencoding_path(&id)), None).await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": if ok_status(status) {
                    format!("Deleted project `{id}`")
                } else {
                    format!("delete_project failed for `{id}`")
                },
                "result": data,
                "navigation": { "action": "goto", "path": "/projects" }
            })
            .to_string())
        }

        "open_project" | "open_ide" | "switch_project" => {
            let project = arg_str(arguments, &["project", "slug", "id", "name"]).unwrap_or_default();
            if project.is_empty() {
                return Ok(json!({
                    "ok": true,
                    "summary": "Open projects (no project specified)",
                    "navigation": { "action": "goto", "path": "/projects" }
                })
                .to_string());
            }
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
            let intent = crate::focus::navigate_intent(&path, Some(&project));
            Ok(json!({
                "ok": true,
                "summary": summary,
                "navigation": { "action": action, "path": path, "project": project },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        // ─── SDLC / Change requests ───────────────────────────────────────
        "list_changes" | "open_changes" => {
            let navigate = arg_bool(arguments, "navigate", true);
            let status_filter = arg_str(arguments, &["status"]);
            let path = match &status_filter {
                Some(s) => format!("/api/change_requests?status={}", s),
                None => "/api/change_requests".to_string(),
            };
            let (status, data) = http_json("GET", &path, None).await.unwrap_or_else(|e| {
                (0, json!({ "error": e }))
            });
            let mut out = json!({
                "ok": ok_status(status) || status == 0 && data.get("error").is_none(),
                "summary": "Listed change requests (SDLC)",
                "http_status": status,
                "change_requests": data,
                "api": format!("{base}/api/change_requests"),
            });
            if navigate {
                out["navigation"] = json!({ "action": "goto", "path": "/changes" });
            }
            // ok even if API empty — navigation still useful
            if status == 0 {
                out["ok"] = json!(true);
                out["summary"] = json!("Open change requests (API unavailable — UI only)");
            } else {
                out["ok"] = json!(ok_status(status));
            }
            Ok(out.to_string())
        }

        "create_change" | "open_create_change" => {
            let via = arg_str(arguments, &["via"])
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let domain_mode = if via == "ux" {
                crate::focus::DomainMode::Ux
            } else {
                crate::focus::DomainMode::Server
            };

            // If title provided → create (or schedule UX commit); else open form
            if let Some(title) = arg_str(arguments, &["title"]) {
                let project = arg_str(arguments, &["slug", "project", "repo", "repo_id"]);
                let description = arg_str(arguments, &["description", "body"]);

                if domain_mode == crate::focus::DomainMode::Ux {
                    let path = "/changes".to_string();
                    let intent = crate::focus::create_change_intent(
                        Some(&title),
                        description.as_deref(),
                        project.as_deref(),
                        &path,
                        crate::focus::DomainMode::Ux,
                    );
                    crate::focus::push_intent_log(json!({
                        "type": "CreateChange",
                        "actor": "agent",
                        "summary": title,
                        "domain": "ux",
                        "ts": chrono_ms(),
                    }));
                    return Ok(json!({
                        "ok": true,
                        "summary": format!("Scheduled create_change `{title}` — UX Present then commit"),
                        "pending_ux": true,
                        "navigation": { "action": "goto", "path": "/changes/new" },
                        "intent": intent,
                        "execution": { "domain": "ux", "present": "ux_commit" }
                    })
                    .to_string());
                }

                let body = json!({
                    "title": title,
                    "description": description.clone().unwrap_or_default(),
                    "repo_id": arg_str(arguments, &["repo_id", "repo"]),
                    "slug": project.clone(),
                    "jira_ticket": arg_str(arguments, &["jira_ticket", "jira"]).unwrap_or_default(),
                    "author": arg_str(arguments, &["author"]).unwrap_or_default(),
                    "source_branch": arg_str(arguments, &["source_branch", "branch"]),
                });
                let (status, data) =
                    http_json("POST", "/api/change_requests", Some(body)).await?;
                let cr_id = data
                    .pointer("/change_request/id")
                    .or_else(|| data.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let path = if cr_id.is_empty() {
                    "/changes".to_string()
                } else {
                    format!("/changes/{cr_id}")
                };
                let intent = crate::focus::create_change_intent(
                    Some(&title),
                    description.as_deref(),
                    project.as_deref(),
                    &path,
                    crate::focus::DomainMode::Server,
                );
                if ok_status(status) {
                    crate::focus::push_intent_log(json!({
                        "type": "CreateChange",
                        "actor": "agent",
                        "summary": title,
                        "domain": "server",
                        "ts": chrono_ms(),
                    }));
                }
                Ok(json!({
                    "ok": ok_status(status),
                    "http_status": status,
                    "summary": if ok_status(status) {
                        format!("Created change request: {title}")
                    } else {
                        format!("create_change failed (HTTP {status})")
                    },
                    "change_request": data,
                    "navigation": { "action": "goto", "path": path },
                    "intent": intent,
                    "execution": {
                        "domain": "server",
                        "present": if ok_status(status) { "illustrate" } else { "none" }
                    }
                })
                .to_string())
            } else {
                let intent = crate::focus::create_change_intent(
                    None,
                    None,
                    arg_str(arguments, &["slug", "project"]).as_deref(),
                    "/changes/new",
                    crate::focus::DomainMode::Ux,
                );
                Ok(json!({
                    "ok": true,
                    "summary": "Open create change request form",
                    "navigation": { "action": "goto", "path": "/changes/new" },
                    "intent": intent,
                    "execution": { "domain": "none", "present": "goto" }
                })
                .to_string())
            }
        }

        "get_change" => {
            let id = arg_str(arguments, &["id", "change_id", "change_request_id"])
                .ok_or_else(|| "get_change requires id".to_string())?;
            let (status, data) =
                http_json("GET", &format!("/api/change_requests/{}", urlencoding_path(&id)), None)
                    .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Change request {id}"),
                "change_request": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") }
            })
            .to_string())
        }

        "submit_change" => {
            let id = arg_str(arguments, &["id", "change_id"])
                .ok_or_else(|| "submit_change requires id".to_string())?;
            let (status, data) = http_json(
                "POST",
                &format!("/api/change_requests/{}/submit", urlencoding_path(&id)),
                Some(json!({})),
            )
            .await?;
            let summary = format!("Submitted change {id} for review");
            let intent = crate::focus::change_action_intent("submit", &id, &summary);
            crate::focus::register_pending_intent(
                intent.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                json!({ "tool": "submit_change", "id": id }),
            );
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "approve_change" => {
            let id = arg_str(arguments, &["id", "change_id"])
                .ok_or_else(|| "approve_change requires id".to_string())?;
            let body = json!({
                "reviewer": arg_str(arguments, &["reviewer"]).unwrap_or_default(),
                "comment": arg_str(arguments, &["comment"]),
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/change_requests/{}/approve", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            let summary = format!("Approved change {id}");
            let intent = crate::focus::change_action_intent("approve", &id, &summary);
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "request_changes" => {
            let id = arg_str(arguments, &["id", "change_id"])
                .ok_or_else(|| "request_changes requires id".to_string())?;
            let body = json!({
                "reviewer": arg_str(arguments, &["reviewer"]).unwrap_or_default(),
                "comment": arg_str(arguments, &["comment"]).unwrap_or_default(),
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/change_requests/{}/request-changes", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            let summary = format!("Requested changes on {id}");
            let intent = crate::focus::change_action_intent("request_changes", &id, &summary);
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "merge_change" => {
            let id = arg_str(arguments, &["id", "change_id"])
                .ok_or_else(|| "merge_change requires id".to_string())?;
            let body = json!({
                "merger": arg_str(arguments, &["merger"]).unwrap_or_default(),
                "slug": arg_str(arguments, &["slug"]).unwrap_or_default(),
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/change_requests/{}/merge", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            let summary = format!("Merged change {id}");
            let intent = crate::focus::change_action_intent("merge", &id, &summary);
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "add_comment" => {
            let id = arg_str(arguments, &["id", "change_id"])
                .ok_or_else(|| "add_comment requires id".to_string())?;
            let body_text = arg_str(arguments, &["body", "comment", "text"])
                .ok_or_else(|| "add_comment requires body/comment".to_string())?;
            let body = json!({
                "author": arg_str(arguments, &["author"]).unwrap_or_default(),
                "construct_path": arg_str(arguments, &["construct_path", "path"]),
                "body": body_text,
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/change_requests/{}/comments", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            let summary = format!("Added comment on change {id}");
            let intent = crate::focus::change_action_intent("comment", &id, &summary);
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "get_change_diff" => {
            let id = arg_str(arguments, &["id", "change_id"])
                .ok_or_else(|| "get_change_diff requires id".to_string())?;
            let (status, data) = http_json(
                "GET",
                &format!("/api/change_requests/{}/diff", urlencoding_path(&id)),
                None,
            )
            .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Structural diff for change {id}"),
                "diff": data,
                "navigation": { "action": "goto", "path": format!("/changes/{id}") }
            })
            .to_string())
        }

        // ─── Deploy ───────────────────────────────────────────────────────
        "open_deploy" => {
            let intent = crate::focus::deploy_action_intent("open_deploy", "Open deploy", None);
            Ok(json!({
                "ok": true,
                "summary": "Open deploy view",
                "navigation": { "action": "goto", "path": "/deploy" },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        "list_deploy_environments" => {
            let (status, data) = http_json("GET", "/api/deploy_environments", None).await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": "Deploy environments",
                "environments": data,
                "navigation": { "action": "goto", "path": "/deploy" }
            })
            .to_string())
        }

        "deploy_status" => {
            let environment = arg_str(arguments, &["environment", "env"])
                .ok_or_else(|| "deploy_status requires environment".to_string())?;
            let unit_name = arg_str(arguments, &["unit_name", "unit", "project", "slug"])
                .ok_or_else(|| "deploy_status requires unit_name".to_string())?;
            let q = format!(
                "/api/deployment_status?environment={}&unit_name={}",
                urlencoding_path(&environment),
                urlencoding_path(&unit_name)
            );
            let (status, data) = http_json("GET", &q, None).await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Deploy status for {unit_name} in {environment}"),
                "status": data,
                "navigation": { "action": "goto", "path": "/deploy" }
            })
            .to_string())
        }

        "plan_provision" => {
            let project_slug = arg_str(arguments, &["project_slug", "project", "slug", "name"])
                .ok_or_else(|| "plan_provision requires project_slug".to_string())?;
            let environment = arg_str(arguments, &["environment", "env"])
                .unwrap_or_else(|| "dev".into());
            let repo_id = arg_str(arguments, &["repo_id", "repo"])
                .unwrap_or_else(|| project_slug.clone());
            let body = json!({
                "project_slug": project_slug,
                "environment": environment,
                "repo_id": repo_id,
                "branch": arg_str(arguments, &["branch"]).unwrap_or_else(|| "main".into()),
            });
            let (status, data) = http_json("POST", "/api/plan-provision", Some(body)).await?;
            let summary = format!("Plan provision {project_slug} → {environment}");
            let intent =
                crate::focus::deploy_action_intent("plan_provision", &summary, Some(&project_slug));
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "plan": data,
                "navigation": { "action": "goto", "path": "/deploy" },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "provision_project" => {
            let project_slug = arg_str(arguments, &["project_slug", "project", "slug", "name"])
                .ok_or_else(|| "provision_project requires project_slug".to_string())?;
            let environment = arg_str(arguments, &["environment", "env"])
                .unwrap_or_else(|| "dev".into());
            let repo_id = arg_str(arguments, &["repo_id", "repo"])
                .unwrap_or_else(|| project_slug.clone());
            let body = json!({
                "project_slug": project_slug,
                "environment": environment,
                "repo_id": repo_id,
                "branch": arg_str(arguments, &["branch"]).unwrap_or_else(|| "main".into()),
            });
            let (status, data) = http_json("POST", "/api/provision-project", Some(body)).await?;
            let summary = format!("Provision {project_slug} → {environment}");
            let intent =
                crate::focus::deploy_action_intent("provision", &summary, Some(&project_slug));
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": "/deploy" },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "get_provision_job" => {
            let job_id = arg_str(arguments, &["job_id", "id"])
                .ok_or_else(|| "get_provision_job requires job_id".to_string())?;
            let (status, data) = http_json(
                "GET",
                &format!("/api/provision_jobs?job_id={}", urlencoding_path(&job_id)),
                None,
            )
            .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Provision job {job_id}"),
                "job": data,
                "navigation": { "action": "goto", "path": "/deploy" }
            })
            .to_string())
        }

        // ─── Registry ─────────────────────────────────────────────────────
        "open_registry" => Ok(json!({
            "ok": true,
            "summary": "Open registry",
            "navigation": { "action": "goto", "path": "/registry" }
        })
        .to_string()),

        "list_registry_layers" | "search_registry" => {
            let (status, layers) = http_json("GET", "/api/registry/layers", None).await?;
            let (s2, stubs) = http_json("GET", "/api/registry/stubs", None)
                .await
                .unwrap_or((0, json!([])));
            let query = arg_str(arguments, &["query", "q", "name"]);
            let mut out = json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": "Registry layers + stubs",
                "layers": layers,
                "stubs": if ok_status(s2) { stubs } else { json!([]) },
                "navigation": { "action": "goto", "path": "/registry" }
            });
            if let Some(q) = query {
                let ql = q.to_lowercase();
                if let Some(arr) = out["layers"].as_array().cloned() {
                    let filtered: Vec<_> = arr
                        .into_iter()
                        .filter(|v| {
                            v.get("name")
                                .and_then(|n| n.as_str())
                                .map(|n| n.to_lowercase().contains(&ql))
                                .unwrap_or(false)
                        })
                        .collect();
                    out["layers"] = json!(filtered);
                    out["summary"] = json!(format!("Registry search: {q}"));
                }
            }
            Ok(out.to_string())
        }

        "list_registry_stubs" => {
            let (status, data) = http_json("GET", "/api/registry/stubs", None).await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": "Registry stubs",
                "stubs": data,
                "navigation": { "action": "goto", "path": "/registry" }
            })
            .to_string())
        }

        // ─── Config / meta ────────────────────────────────────────────────
        "open_dashboard" => Ok(json!({
            "ok": true,
            "summary": "Open dashboard",
            "navigation": { "action": "goto", "path": "/dashboard" }
        })
        .to_string()),

        "open_config" => {
            let intent =
                crate::focus::page_action_intent("open_config", "/config", "Open runtime config");
            Ok(json!({
                "ok": true,
                "summary": "Open runtime config",
                "navigation": { "action": "goto", "path": "/config" },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        "get_config" => {
            let (status, data) = http_json("GET", "/api/config", None).await.unwrap_or_else(|e| {
                let cfg = crate::config::load_config_or_default();
                (
                    200,
                    json!({
                        "version": cfg.version,
                        "projects_dir": cfg.projects_dir_path().to_string_lossy(),
                        "error_fallback": e,
                    }),
                )
            });
            let intent =
                crate::focus::page_action_intent("get_config", "/config", "Runtime config");
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": "Runtime config",
                "config": data,
                "navigation": { "action": "goto", "path": "/config" },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        "get_mission" => {
            let id = arg_str(arguments, &["project", "slug", "id", "name"])
                .ok_or_else(|| "get_mission requires project".to_string())?;
            let branch = arg_str(arguments, &["branch"]).unwrap_or_else(|| "main".into());
            let path = format!(
                "/api/repos/{}/mission?branch={}",
                urlencoding_path(&id),
                urlencoding_path(&branch)
            );
            let (status, data) = http_json("GET", &path, None).await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("MISSION.md for {id}"),
                "mission": data,
            })
            .to_string())
        }

        "update_mission" => {
            let id = arg_str(arguments, &["project", "slug", "id", "name"])
                .ok_or_else(|| "update_mission requires project".to_string())?;
            let content = arg_str(arguments, &["content", "mission", "body"])
                .ok_or_else(|| "update_mission requires content".to_string())?;
            let body = json!({
                "content": content,
                "branch": arg_str(arguments, &["branch"]).unwrap_or_else(|| "main".into()),
            });
            let (status, data) = http_json(
                "PUT",
                &format!("/api/repos/{}/mission", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Updated MISSION.md for {id}"),
                "result": data,
                "navigation": { "action": "goto", "path": format!("/projects/{id}") }
            })
            .to_string())
        }

        "get_current_context" => {
            // Prefer session-scoped focus when CURRENT_SESSION is set
            let sid = crate::session::CURRENT_SESSION
                .try_with(|s| s.clone())
                .ok();
            Ok(crate::focus::context_tool_json(sid.as_deref()).to_string())
        }

        "wait_intent_ack" => {
            let intent_id = arg_str(arguments, &["intent_id", "id", "intentId"])
                .ok_or_else(|| "wait_intent_ack requires intent_id".to_string())?;
            let timeout_ms = arguments
                .get("timeout_ms")
                .or_else(|| arguments.get("timeout"))
                .and_then(|v| v.as_u64())
                .unwrap_or(45_000);
            match crate::focus::wait_intent_ack(&intent_id, timeout_ms).await {
                Ok(ack) => Ok(json!({
                    "ok": true,
                    "summary": format!("Present ACK received for `{intent_id}`"),
                    "intent_id": intent_id,
                    "ack": ack,
                    "execution": { "domain": "none", "present": "acked" }
                })
                .to_string()),
                Err(e) => Ok(json!({
                    "ok": false,
                    "summary": e,
                    "intent_id": intent_id,
                    "hint": "Call after create_project(via=ux) / create_change(via=ux) so the browser can finish Present. Or use via=server for multi-step without wait."
                })
                .to_string()),
            }
        }

        other => Err(format!("unknown platform tool: {other}")),
    }
}

fn urlencoding_path(s: &str) -> String {
    // Minimal path-segment encoding (enough for slugs / UUIDs)
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => {
                for b in c.to_string().as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

fn chrono_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Domain create (POST /api/repos + S3 scaffold, disk fallback). Shared by tool + `/api/ux/create_project`.
pub async fn create_project_domain(name: &str, description: Option<&str>) -> Result<Value, String> {
    use crate::provider::s3_workspace::{
        allow_disk_project_create, ide_source_mode, seed_new_repo_scaffold, IdeSourceMode,
    };

    let mode = ide_source_mode();
    let remote_only = matches!(mode, IdeSourceMode::S3);
    let body = json!({
        "name": name,
        "description": description,
    });
    let (status, data) = http_json("POST", "/api/repos", Some(body))
        .await
        .unwrap_or((0, json!({})));

    let mut scaffold: Option<Value> = None;
    let mut scaffold_err: Option<String> = None;
    if ok_status(status) {
        let repo_id = data
            .pointer("/id/value")
            .or_else(|| data.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let slug_for_seed = data
            .get("slug")
            .or_else(|| data.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string();
        if let Some(rid) = repo_id {
            match seed_new_repo_scaffold(&rid, &slug_for_seed) {
                Ok(files) => {
                    scaffold = Some(json!({
                        "repo_id": rid,
                        "files": files,
                        "store": "s3",
                    }));
                }
                Err(e) => {
                    scaffold_err = Some(e);
                    if remote_only {
                        return Ok(json!({
                            "ok": false,
                            "summary": "Repo META created but S3 scaffold failed (VEIL_SOURCE_MODE=s3)",
                            "http_status": status,
                            "project": data,
                            "error": scaffold_err,
                            "hint": "Check AWS_PROFILE, BUCKET, and aws s3 put access",
                        }));
                    }
                }
            }
        } else if remote_only {
            scaffold_err = Some("POST /api/repos response missing id".into());
        }
    }

    let (status, data) = if !ok_status(status) {
        if !allow_disk_project_create() {
            (
                status,
                json!({
                    "error": data.get("error").cloned().unwrap_or(json!(
                        "create_project: remote create failed and VEIL_SOURCE_MODE=s3 forbids disk hub"
                    )),
                    "platform": data,
                    "source_mode": "s3",
                }),
            )
        } else {
            match http_json("POST", "/api/projects", Some(json!({ "name": name }))).await {
                Ok((s, d)) if ok_status(s) => (s, d),
                Ok((s, d)) => match crate::config::ensure_projects_dir_exists() {
                    Ok(dir) => match crate::project_layout::create_project(&dir, name) {
                        Ok(info) => (201, json!(info)),
                        Err(e) => (s, json!({ "error": e, "platform": d })),
                    },
                    Err(e) => (s, json!({ "error": e, "platform": d })),
                },
                Err(e) => match crate::config::ensure_projects_dir_exists() {
                    Ok(dir) => match crate::project_layout::create_project(&dir, name) {
                        Ok(info) => (201, json!(info)),
                        Err(ce) => (0, json!({ "error": ce, "http_error": e })),
                    },
                    Err(ce) => (0, json!({ "error": ce, "http_error": e })),
                },
            }
        }
    } else {
        (status, data)
    };

    let slug = data
        .get("slug")
        .or_else(|| data.get("name"))
        .or_else(|| data.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(name)
        .to_string();

    let source_mode = match mode {
        IdeSourceMode::S3 => "s3",
        IdeSourceMode::PreferS3 => "prefer_s3",
        IdeSourceMode::Disk => "disk",
    };

    Ok(json!({
        "ok": ok_status(status),
        "summary": if ok_status(status) {
            if scaffold.is_some() {
                format!("Created remote project `{slug}` (DDB + S3 scaffold)")
            } else {
                format!("Created project `{slug}`")
            }
        } else {
            format!("create_project failed for `{name}` (HTTP {status})")
        },
        "http_status": status,
        "project": data,
        "slug": slug,
        "source_mode": source_mode,
        "scaffold": scaffold,
        "scaffold_error": scaffold_err,
    }))
}

/// MCP / agent tool definitions for platform UX (superset of pure navigation).
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "navigate_to",
            "description": "Navigate the VEIL runtime dashboard SPA to a path. Use for any UI destination: /dashboard, /projects, /projects/{id}, /changes, /changes/new, /deploy, /registry, /config, /agents.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "SPA path starting with /" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "list_projects",
            "description": "List all product projects/repos (real data from GET /api/repos) and open /projects. Use when the user asks what projects exist or to show the project list. Prefer this before create_project if checking uniqueness.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "navigate": { "type": "boolean", "description": "Also navigate SPA to /projects (default true)" }
                },
                "required": []
            }
        }),
        json!({
            "name": "create_project",
            "description": "Create a new product project/repo. Returns intent.present (form fill + pulse). via=ux: UX commits after Present (browser); via=server: domain first (default for multi-step/ACP). Do NOT re-create or curl. ALWAYS use when the user asks to create a project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Project name/slug (e.g. agentic-workflows)" },
                    "description": { "type": "string", "description": "Optional short description" },
                    "open": { "type": "boolean", "description": "Navigate to project detail after create (default true)" },
                    "open_ide": { "type": "boolean", "description": "Open IDE embed after create (default false)" },
                    "via": { "type": "string", "enum": ["ux", "server"], "description": "ux = Agent→Present→UX→Server; server = domain first + illustrate Present" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "get_project",
            "description": "Fetch project metadata by id/slug and open project detail.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "id": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "delete_project",
            "description": "Delete a project/repo (DELETE /api/repos/{id}). Confirm with the user first for destructive deletes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "id": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "open_project",
            "description": "Open a project detail page in the runtime UI. Pass project id or slug.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "id": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "open_ide",
            "description": "Open the dual-loop IDE for a project inside the runtime shell (agent panel stays; path /projects/{id}/ide).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project id or slug" }
                },
                "required": ["project"]
            }
        }),
        json!({
            "name": "list_changes",
            "description": "List SDLC change requests (GET /api/change_requests) and open /changes. Optional status filter: Draft, ReadyForReview, Approved, Merged, …",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string" },
                    "navigate": { "type": "boolean" }
                },
                "required": []
            }
        }),
        json!({
            "name": "create_change",
            "description": "Create a change request (POST /api/change_requests) when title is provided; otherwise opens /changes/new form. Prefer passing title+description for agent-driven SDLC.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "repo_id": { "type": "string" },
                    "slug": { "type": "string", "description": "Project slug / source branch hint" },
                    "jira_ticket": { "type": "string" },
                    "author": { "type": "string" },
                    "source_branch": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "get_change",
            "description": "Get a change request by id and open its detail page.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "submit_change",
            "description": "Submit a change request for review.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "approve_change",
            "description": "Approve a change request.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "reviewer": { "type": "string" },
                    "comment": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "request_changes",
            "description": "Request changes on a change request (review feedback).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "reviewer": { "type": "string" },
                    "comment": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "merge_change",
            "description": "Merge an approved change request.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "merger": { "type": "string" },
                    "slug": { "type": "string" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "add_comment",
            "description": "Add a review comment on a change request (optional construct_path for structural comments).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "body": { "type": "string" },
                    "construct_path": { "type": "string" },
                    "author": { "type": "string" }
                },
                "required": ["id", "body"]
            }
        }),
        json!({
            "name": "get_change_diff",
            "description": "Fetch structural diff for a change request.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "open_deploy",
            "description": "Open the deploy surface in the runtime dashboard.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "list_deploy_environments",
            "description": "List deploy environments.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "deploy_status",
            "description": "Get deployment status for a unit in an environment.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "environment": { "type": "string" },
                    "unit_name": { "type": "string", "description": "Deploy unit / project name" }
                },
                "required": ["environment", "unit_name"]
            }
        }),
        json!({
            "name": "plan_provision",
            "description": "Plan infrastructure provision for a project (dry-run plan).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_slug": { "type": "string" },
                    "environment": { "type": "string", "description": "default dev" },
                    "repo_id": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["project_slug"]
            }
        }),
        json!({
            "name": "provision_project",
            "description": "Provision project infrastructure for an environment (imperative control plane).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_slug": { "type": "string" },
                    "environment": { "type": "string" },
                    "repo_id": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["project_slug"]
            }
        }),
        json!({
            "name": "get_provision_job",
            "description": "Poll a provision job by job_id.",
            "inputSchema": {
                "type": "object",
                "properties": { "job_id": { "type": "string" } },
                "required": ["job_id"]
            }
        }),
        json!({
            "name": "open_registry",
            "description": "Open the layer/stub registry page.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "list_registry_layers",
            "description": "List layers in the platform registry.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "list_registry_stubs",
            "description": "List stubs in the platform registry.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "search_registry",
            "description": "Search registry layers (and list stubs) by name query.",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": []
            }
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
        json!({
            "name": "get_config",
            "description": "Read runtime config (projects_dir, etc.).",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "get_mission",
            "description": "Read MISSION.md for a project (product intent).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["project"]
            }
        }),
        json!({
            "name": "update_mission",
            "description": "Write MISSION.md for a project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "content": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["project", "content"]
            }
        }),
        json!({
            "name": "get_current_context",
            "description": "Return SessionFocus (route, project, construct, file, form, diagnostics) plus recent_intents and recent_acks. Use for deictic references and to see if UX Present finished.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "wait_intent_ack",
            "description": "Block until the browser finishes Present for an intent_id (from create_project via=ux / create_change via=ux). Call AFTER the create tool so Present can stream first — then wait before write_source. timeout_ms default 45000.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "intent_id": { "type": "string", "description": "intent.id from prior tool result" },
                    "timeout_ms": { "type": "integer", "description": "Max wait ms (default 45000)" }
                },
                "required": ["intent_id"]
            }
        }),
    ]
}

// ─── Rig Tool wrappers (key platform actions for openai/ollama agent loop) ───

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};

use crate::rig_tools::ToolErr;

#[derive(Deserialize, Serialize, Default)]
pub struct EmptyArgs {}

#[derive(Clone, Default)]
pub struct ListProjectsTool;

impl Tool for ListProjectsTool {
    const NAME: &'static str = "list_projects";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List all product projects/repos and open /projects.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("list_projects", &json!({})).await.map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct CreateProjectArgs {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub open: Option<bool>,
    #[serde(default)]
    pub open_ide: Option<bool>,
}

#[derive(Clone, Default)]
pub struct CreateProjectTool;

impl Tool for CreateProjectTool {
    const NAME: &'static str = "create_project";
    type Error = ToolErr;
    type Args = CreateProjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Create a new product project (POST /api/repos + disk scaffold). Use when user asks to create a project.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "open": { "type": "boolean" },
                    "open_ide": { "type": "boolean" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut v = json!({ "name": args.name });
        if let Some(d) = args.description {
            v["description"] = json!(d);
        }
        if let Some(o) = args.open {
            v["open"] = json!(o);
        }
        if let Some(o) = args.open_ide {
            v["open_ide"] = json!(o);
        }
        dispatch("create_project", &v).await.map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct ProjectArgs {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Default)]
pub struct OpenProjectTool;

impl Tool for OpenProjectTool {
    const NAME: &'static str = "open_project";
    type Error = ToolErr;
    type Args = ProjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Open project detail page.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "id": { "type": "string" }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("open_project", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Clone, Default)]
pub struct OpenIdeTool;

impl Tool for OpenIdeTool {
    const NAME: &'static str = "open_ide";
    type Error = ToolErr;
    type Args = ProjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Open project IDE embed in runtime shell.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string" }
                },
                "required": ["project"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("open_ide", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct NavigateArgs {
    pub path: String,
}

#[derive(Clone, Default)]
pub struct NavigateToTool;

impl Tool for NavigateToTool {
    const NAME: &'static str = "navigate_to";
    type Error = ToolErr;
    type Args = NavigateArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Navigate SPA to path.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("navigate_to", &json!({ "path": args.path }))
            .await
            .map_err(ToolErr)
    }
}

#[derive(Clone, Default)]
pub struct ListChangesTool;

impl Tool for ListChangesTool {
    const NAME: &'static str = "list_changes";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List SDLC change requests and open /changes.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("list_changes", &json!({})).await.map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct CreateChangeArgs {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub repo_id: Option<String>,
}

#[derive(Clone, Default)]
pub struct CreateChangeTool;

impl Tool for CreateChangeTool {
    const NAME: &'static str = "create_change";
    type Error = ToolErr;
    type Args = CreateChangeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Create change request (with title) or open /changes/new.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "slug": { "type": "string" },
                    "repo_id": { "type": "string" }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("create_change", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct ChangeIdArgs {
    pub id: String,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub merger: Option<String>,
}

#[derive(Clone, Default)]
pub struct ApproveChangeTool;

impl Tool for ApproveChangeTool {
    const NAME: &'static str = "approve_change";
    type Error = ToolErr;
    type Args = ChangeIdArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Approve a change request by id.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "reviewer": { "type": "string" },
                    "comment": { "type": "string" }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("approve_change", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Clone, Default)]
pub struct MergeChangeTool;

impl Tool for MergeChangeTool {
    const NAME: &'static str = "merge_change";
    type Error = ToolErr;
    type Args = ChangeIdArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Merge an approved change request.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "merger": { "type": "string" }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("merge_change", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct ProvisionArgs {
    pub project_slug: String,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub repo_id: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Clone, Default)]
pub struct ProvisionProjectTool;

impl Tool for ProvisionProjectTool {
    const NAME: &'static str = "provision_project";
    type Error = ToolErr;
    type Args = ProvisionArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Provision project infrastructure for an environment.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_slug": { "type": "string" },
                    "environment": { "type": "string" },
                    "repo_id": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["project_slug"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("provision_project", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Clone, Default)]
pub struct DeployStatusTool;

#[derive(Deserialize, Serialize, Default)]
pub struct DeployStatusArgs {
    pub environment: String,
    pub unit_name: String,
}

impl Tool for DeployStatusTool {
    const NAME: &'static str = "deploy_status";
    type Error = ToolErr;
    type Args = DeployStatusArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Get deployment status for unit in environment.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "environment": { "type": "string" },
                    "unit_name": { "type": "string" }
                },
                "required": ["environment", "unit_name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("deploy_status", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Clone, Default)]
pub struct GetConfigTool;

impl Tool for GetConfigTool {
    const NAME: &'static str = "get_config";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Read runtime config.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("get_config", &json!({})).await.map_err(ToolErr)
    }
}

/// Names of platform tools attached to the Rig agent loop.
pub fn rig_platform_tool_names() -> &'static [&'static str] {
    &[
        "list_projects",
        "create_project",
        "open_project",
        "open_ide",
        "navigate_to",
        "list_changes",
        "create_change",
        "approve_change",
        "merge_change",
        "provision_project",
        "deploy_status",
        "get_config",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_project_is_platform_tool() {
        assert!(is_platform_tool("create_project"));
        assert!(is_platform_tool("create_repo"));
        assert!(is_platform_tool("list_projects"));
        assert!(is_platform_tool("approve_change"));
        assert!(is_platform_tool("provision_project"));
        assert!(!is_platform_tool("veil_check"));
        assert!(!is_platform_tool("write_source"));
    }

    #[test]
    fn disk_create_forbidden_when_source_mode_s3() {
        // Isolate from ambient env (set_var is unsafe on modern Rust)
        unsafe {
            std::env::set_var("VEIL_SOURCE_MODE", "s3");
        }
        assert!(!crate::provider::s3_workspace::allow_disk_project_create());
        let err = crate::project_layout::create_project(
            std::path::Path::new("/tmp/veil-should-not-create"),
            "nope",
        )
        .unwrap_err();
        assert!(
            err.contains("VEIL_SOURCE_MODE=s3") || err.contains("forbidden"),
            "{err}"
        );
        unsafe {
            std::env::set_var("VEIL_SOURCE_MODE", "prefer_s3");
        }
        assert!(crate::provider::s3_workspace::allow_disk_project_create());
    }

    #[test]
    fn tool_definitions_include_create_project() {
        let defs = tool_definitions();
        let names: Vec<_> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"create_project"));
        assert!(names.contains(&"list_projects"));
        assert!(names.contains(&"list_changes"));
        assert!(names.contains(&"approve_change"));
        assert!(names.contains(&"merge_change"));
        assert!(names.contains(&"provision_project"));
        assert!(names.contains(&"deploy_status"));
        assert!(names.contains(&"search_registry"));
        assert!(names.contains(&"get_config"));
        assert!(names.contains(&"get_mission"));
        assert!(names.contains(&"wait_intent_ack"));
        assert!(names.contains(&"get_current_context"));
    }

    #[tokio::test]
    async fn navigate_to_returns_navigation() {
        let out = dispatch("navigate_to", &json!({ "path": "/projects" }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["navigation"]["path"], "/projects");
    }

    /// Live: requires ProductHost + AWS. Always deletes the throwaway repo after.
    #[tokio::test]
    #[ignore = "needs live ProductHost + AWS"]
    async fn create_project_live_remote_no_disk_hub() {
        unsafe {
            std::env::set_var("VEIL_SOURCE_MODE", "s3");
        }
        let name = format!(
            "live-create-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                % 100_000
        );
        let out = dispatch(
            "create_project",
            &json!({ "name": name, "description": "live remote test — auto-deleted", "open": false }),
        )
        .await
        .expect("dispatch");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true, "{out}");
        assert!(v["scaffold"].is_object(), "expected S3 scaffold: {out}");
        let hub = crate::config::resolve_projects_dir().join(&name);
        assert!(
            !hub.exists(),
            "must not create disk hub at {}",
            hub.display()
        );
        // Do not leave junk in the operator's deployment.
        let rid = v
            .pointer("/project/id/value")
            .or_else(|| v.pointer("/scaffold/repo_id"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        if let Some(id) = rid {
            let _ = dispatch("delete_project", &json!({ "id": id })).await;
        } else {
            let _ = dispatch("delete_project", &json!({ "project": name })).await;
        }
    }
}
