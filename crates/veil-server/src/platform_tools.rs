//! Platform UX tools — agent drives the full runtime dashboard (projects, SDLC, deploy).
//!
//! These tools call ProductHost platform APIs (`/api/repos`, `/api/pull_requests`, …)
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
        "PATCH" => client.patch(&url),
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
            | "list_prs"
            | "open_prs"
            | "list_pull_requests"
            | "create_pr"
            | "open_create_pr"
            | "open_pr"
            | "create_pull_request"
            | "get_pr"
            | "get_pull_request"
            | "submit_pr"
            | "submit_pull_request"
            | "approve_pr"
            | "request_pr_changes"
            | "merge_pr"
            | "add_comment"
            | "get_pr_diff"
            | "list_projects"
            | "open_projects"
            | "create_project"
            | "create_repo"
            | "get_project"
            | "rename_project"
            | "update_project"
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
            | "resolve_coding_target"
            | "run_coding_plan"
            | "list_outstanding"
            | "request_sign_off"
            | "sign_off"
    )
}

/// Canonicalize PR-facing aliases → existing handlers (product language: PR).
fn canonicalize_tool(name: &str) -> &str {
    match name {
        "list_prs" | "list_pull_requests" => "list_prs",
        "create_pr" | "open_pr" | "create_pull_request" => "create_pr",
        "get_pr" | "get_pull_request" => "get_pr",
        "submit_pr" | "submit_pull_request" => "submit_pr",
        "approve_pr" => "approve_pr",
        "merge_pr" => "merge_pr",
        "get_pr_diff" => "get_pr_diff",
        "rename_project" | "update_project" => "update_project",
        other => other,
    }
}

/// Dispatch a platform tool. Always returns JSON string (or Err).
pub async fn dispatch(tool_name: &str, arguments: &Value) -> Result<String, String> {
    let tool_name = canonicalize_tool(tool_name);
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
            let review = crate::review::snapshot_json(crate::review::ListFilter {
                status: Some(crate::review::ItemStatus::Outstanding),
                ..Default::default()
            });
            let mut out = json!({
                "ok": ok_status(status),
                "summary": if ok_status(status) {
                    let n = review.get("outstanding").and_then(|v| v.as_u64()).unwrap_or(0);
                    if n > 0 {
                        format!("Listed projects — {n} outstanding change(s) need sign-off")
                    } else {
                        "Listed projects".to_string()
                    }
                } else {
                    format!("list_projects failed (HTTP {status})")
                },
                "http_status": status,
                "projects": data,
                "outstanding": review.get("outstanding"),
                "review": review.get("by_project"),
                "api": format!("{base}/api/repos"),
            });
            if navigate {
                out["navigation"] = json!({ "action": "goto", "path": "/projects" });
                out["intent"] = crate::focus::page_action_intent(
                    "list_projects",
                    "/projects",
                    "Open projects",
                );
                out["execution"] = json!({ "domain": "none", "present": "goto" });
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
            } else if crate::focus::client_present() {
                // Browser is watching — same surfaces as the human (Present → click → UX).
                crate::focus::DomainMode::Ux
            } else {
                // Headless ACP / no UI: domain first so follow-on write_source can bind.
                crate::focus::DomainMode::Server
            };
            // Path/id segment is always the slug; display name may contain spaces.
            let path_slug = crate::project_layout::slugify_name(&name);

            // ── UX path: no domain yet — Present commits via /api/ux/create_project ──
            if domain_mode == crate::focus::DomainMode::Ux {
                let path = if open_ide {
                    format!("/projects/{path_slug}/ide")
                } else if open {
                    format!("/projects/{path_slug}")
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
                        "Scheduled create_project `{name}` (slug `{path_slug}`) — UX will Present form then commit (Agent→UX→Server). Do not re-create. Wait for Present to finish before write_source."
                    ),
                    "name": name,
                    "slug": path_slug,
                    "pending_ux": true,
                    "intent_id": intent_id,
                    "navigation": { "action": if open_ide { "open-ide" } else { "goto" }, "path": path, "project": path_slug },
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
                let repo_id = result
                    .get("id")
                    .or_else(|| result.pointer("/project/id"))
                    .and_then(|v| v.as_str());
                let _ = crate::review::record_project_created(
                    &slug,
                    Some(&name),
                    repo_id,
                );
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

        "update_project" => {
            let id = arg_str(arguments, &["project", "slug", "id"])
                .or_else(|| crate::coding_gates::current_project_slug())
                .or_else(|| {
                    crate::provider::hub::CURRENT_PROJECT
                        .try_with(|n| n.clone())
                        .ok()
                })
                .ok_or_else(|| {
                    "update_project / rename_project requires project/slug/id (or a bound project)"
                        .to_string()
                })?;
            let name = arg_str(arguments, &["name", "new_name", "title", "display_name"]);
            let new_slug = arg_str(arguments, &["new_slug"]);
            let description = arg_str(arguments, &["description", "desc"]);
            let clear_description = arg_bool(arguments, "clear_description", false);
            if name.is_none() && new_slug.is_none() && description.is_none() && !clear_description
            {
                return Err(
                    "update_project requires name, new_slug, and/or description".into(),
                );
            }
            let mut body = json!({});
            if let Some(ref n) = name {
                body["name"] = json!(n);
            }
            if let Some(ref s) = new_slug {
                body["slug"] = json!(s);
            }
            if let Some(ref d) = description {
                body["description"] = json!(d);
            }
            if clear_description {
                body["clear_description"] = json!(true);
            }
            let (status, data) = http_json(
                "PATCH",
                &format!("/api/repos/{}", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            let slug = data
                .get("slug")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let display = data
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&slug)
                .to_string();
            if ok_status(status) {
                crate::focus::push_intent_log(json!({
                    "type": "UpdateProject",
                    "actor": "agent",
                    "summary": format!("{id} → {display}"),
                    "domain": "server",
                    "ts": chrono_ms(),
                }));
                let _ = crate::review::record_project_renamed(&slug, &display);
            }
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": if ok_status(status) {
                    format!("Renamed project `{id}` to `{display}` (slug `{slug}`)")
                } else {
                    format!("update_project failed for `{id}` (HTTP {status})")
                },
                "project": data,
                "name": display,
                "slug": slug,
                "navigation": {
                    "action": "goto",
                    "path": format!("/projects/{slug}"),
                    "project": slug
                },
                "execution": {
                    "domain": "server",
                    "present": if ok_status(status) { "illustrate" } else { "none" }
                }
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
            let slug = crate::project_layout::slugify_name(&project);
            // Bind ACP + session + S3 rematerialize so follow-on list_files/write_source work.
            // (MCP layer also re-binds hub provider when available.)
            let bind = crate::agent_scope::prepare_project(&slug, None);
            let path = if tool_name == "open_ide" {
                format!("/projects/{slug}/ide")
            } else {
                format!("/projects/{slug}")
            };
            let action = if tool_name == "open_ide" {
                "open-ide"
            } else if tool_name == "switch_project" {
                "switch-project"
            } else {
                "goto"
            };
            let (summary, file_count) = match &bind {
                Ok(info) => {
                    let n = info
                        .get("file_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    (
                        format!(
                            "Open {slug} in IDE — bound session, {n} file(s) on disk/S3"
                        ),
                        n,
                    )
                }
                Err(e) => (
                    format!("Open {slug} (nav only; bind failed: {e})"),
                    0u64,
                ),
            };
            let intent = crate::focus::navigate_intent(&path, Some(&slug));
            let mut out = json!({
                "ok": true,
                "summary": summary,
                "slug": slug,
                "project": slug,
                "file_count": file_count,
                "navigation": { "action": action, "path": path, "project": slug },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            });
            if let Ok(info) = bind {
                out["session"] = info;
                out["bound"] = json!(true);
            }
            Ok(out.to_string())
        }

        // ─── Coding orchestrator plans (step runner) ─────────────────────
        "run_coding_plan" => {
            let plan_name = arg_str(arguments, &["plan", "name", "id"]).unwrap_or_else(|| {
                "coding.fix_diagnostics".into()
            });
            let plan_id = crate::coding_orchestrator::PlanId::parse(&plan_name).ok_or_else(|| {
                format!(
                    "unknown plan `{plan_name}` — use coding.slice | coding.fix_diagnostics | coding.finish_task"
                )
            })?;
            let action = arg_str(arguments, &["action"])
                .unwrap_or_else(|| "start".into())
                .to_lowercase();
            let request = arg_str(arguments, &["request", "task", "message", "query"])
                .unwrap_or_else(|| "".into());
            let project = arg_str(arguments, &["slug", "project"]).or_else(|| {
                crate::coding_gates::current_project_slug()
            });

            // status — inspect cursor without advancing
            if action == "status" {
                return Ok(match crate::coding_orchestrator::get_run(project.as_deref(), plan_id)
                {
                    Some(run) => json!({
                        "ok": true,
                        "plan": plan_id.as_str(),
                        "action": "status",
                        "run": crate::coding_orchestrator::run_status_json(&run),
                        "next": crate::coding_orchestrator::next_action_json(&run),
                    })
                    .to_string(),
                    None => json!({
                        "ok": true,
                        "plan": plan_id.as_str(),
                        "action": "status",
                        "run": null,
                        "summary": "No active run — call action=start",
                    })
                    .to_string(),
                });
            }

            // next — agent completed current step; advance and return next action
            if action == "next" || action == "advance" {
                if crate::coding_orchestrator::get_run(project.as_deref(), plan_id).is_none() {
                    return Ok(json!({
                        "ok": false,
                        "error": "no active plan run — call run_coding_plan with action=start first",
                        "plan": plan_id.as_str(),
                    })
                    .to_string());
                }
                // skip=true still advances (e.g. already on feature branch)
                let Some(run) =
                    crate::coding_orchestrator::advance_run(project.as_deref(), plan_id)
                else {
                    return Ok(json!({
                        "ok": true,
                        "plan": plan_id.as_str(),
                        "phase": "done",
                        "summary": "Plan complete",
                    })
                    .to_string());
                };
                // Host-owned finish step: auto-execute finish_task
                if let Some(step) = crate::coding_orchestrator::current_step(&run) {
                    if step.owner == "host" && step.id == "finish" {
                        let mut fin = arguments.clone();
                        if let Some(obj) = fin.as_object_mut() {
                            obj.insert("plan".into(), json!("coding.finish_task"));
                            obj.insert("action".into(), json!("start"));
                            if !request.is_empty() {
                                obj.insert("request".into(), json!(request));
                            } else if !run.request.is_empty() {
                                obj.insert("request".into(), json!(run.request));
                            }
                        }
                        let fin_raw = Box::pin(dispatch("run_coding_plan", &fin))
                            .await
                            .unwrap_or_else(|e| json!({ "ok": false, "error": e }).to_string());
                        let fin_val: Value = serde_json::from_str(&fin_raw)
                            .unwrap_or_else(|_| json!({ "raw": fin_raw }));
                        crate::coding_orchestrator::clear_run(project.as_deref(), plan_id);
                        return Ok(json!({
                            "ok": fin_val.get("ok").and_then(|v| v.as_bool()).unwrap_or(true),
                            "plan": plan_id.as_str(),
                            "phase": "done",
                            "summary": "Advanced to finish — open/submit PR complete",
                            "finish": fin_val,
                            "run": crate::coding_orchestrator::run_status_json(&run),
                        })
                        .to_string());
                    }
                }
                let next = crate::coding_orchestrator::next_action_json(&run);
                return Ok(json!({
                    "ok": true,
                    "plan": plan_id.as_str(),
                    "action": "next",
                    "summary": format!(
                        "Advanced — next: {}",
                        next.get("step_id").and_then(|v| v.as_str()).unwrap_or("done")
                    ),
                    "next": next,
                    "run": crate::coding_orchestrator::run_status_json(&run),
                })
                .to_string());
            }

            // start (default)
            let resolve_args = json!({
                "request": request,
                "project": project,
            });
            let resolve_raw = if matches!(
                plan_id,
                crate::coding_orchestrator::PlanId::FinishTask
            ) {
                if request.trim().is_empty() {
                    json!({
                        "ok": true,
                        "decision": "bind_or_new",
                        "summary": "finish_task — using session active PR or create at open step",
                    })
                    .to_string()
                } else {
                    Box::pin(dispatch("resolve_coding_target", &resolve_args))
                        .await
                        .unwrap_or_else(|e| json!({ "ok": false, "error": e }).to_string())
                }
            } else {
                Box::pin(dispatch("resolve_coding_target", &resolve_args))
                    .await
                    .unwrap_or_else(|e| json!({ "ok": false, "error": e }).to_string())
            };
            let resolve_val: Value =
                serde_json::from_str(&resolve_raw).unwrap_or_else(|_| json!({ "raw": resolve_raw }));

            if resolve_val.get("decision").and_then(|v| v.as_str()) == Some("needs_choice") {
                return Ok(json!({
                    "ok": true,
                    "plan": plan_id.as_str(),
                    "phase": "await_choice",
                    "summary": "Coding plan paused — operator must choose pull request",
                    "resolve": resolve_val,
                    "plan_spec": crate::coding_orchestrator::plan_json(plan_id),
                    "playbook": crate::coding_orchestrator::agent_playbook(plan_id, &resolve_val),
                    "intent": resolve_val.get("intent").cloned(),
                    "pending_ux": true,
                    "execution": { "domain": "ux", "present": "choose" },
                    "next": "wait_intent_ack then resolve_coding_target({choice}) then run_coding_plan action=start"
                })
                .to_string());
            }

            // finish_task: host-driven open+submit (full plan in one shot)
            if matches!(plan_id, crate::coding_orchestrator::PlanId::FinishTask) {
                let title = arg_str(arguments, &["title"]).unwrap_or_else(|| {
                    if request.trim().is_empty() {
                        "Agent coding changes".into()
                    } else {
                        let t: String = request.chars().take(72).collect();
                        if request.len() > 72 {
                            format!("{t}…")
                        } else {
                            t
                        }
                    }
                });
                let mut create_args = json!({
                    "title": title,
                    "description": request,
                });
                if let Some(ref p) = project {
                    create_args["slug"] = json!(p);
                    create_args["project"] = json!(p);
                }
                let create_raw = Box::pin(dispatch("create_pr", &create_args))
                    .await
                    .unwrap_or_else(|e| json!({ "ok": false, "error": e }).to_string());
                let create_val: Value = serde_json::from_str(&create_raw)
                    .unwrap_or_else(|_| json!({ "raw": create_raw }));
                let pr_id = create_val
                    .pointer("/pull_request/id")
                    .or_else(|| create_val.pointer("/pull_request/pull_request/id"))
                    .or_else(|| create_val.pointer("/pull_request/id"))
                    .and_then(|v| v.as_str())
                    .or_else(|| create_val.get("pr_id").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                let mut submit_val = json!(null);
                if !pr_id.is_empty() {
                    let mut sub_args = json!({ "id": pr_id });
                    if let Some(ref p) = project {
                        sub_args["slug"] = json!(p);
                    }
                    let sub_raw = Box::pin(dispatch("submit_pr", &sub_args))
                        .await
                        .unwrap_or_else(|e| json!({ "ok": false, "error": e }).to_string());
                    submit_val = serde_json::from_str(&sub_raw)
                        .unwrap_or_else(|_| json!({ "raw": sub_raw }));
                }
                crate::coding_orchestrator::clear_run(project.as_deref(), plan_id);
                let sign_count = crate::review::list_items(crate::review::ListFilter {
                    slug: project.clone(),
                    status: Some(crate::review::ItemStatus::Outstanding),
                    ..Default::default()
                })
                .len();
                let sign_intent =
                    crate::review::request_sign_off_intent(project.as_deref(), sign_count);
                let review_path = project
                    .as_deref()
                    .map(|s| format!("/review/{s}"))
                    .unwrap_or_else(|| "/review".into());
                return Ok(json!({
                    "ok": create_val.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                    "plan": plan_id.as_str(),
                    "phase": "done",
                    "summary": "finish_task: PR submitted — human Sign-off is next (no auto-merge)",
                    "resolve": resolve_val,
                    "create_pr": create_val,
                    "submit_pr": submit_val,
                    "navigation": { "action": "goto", "path": review_path },
                    "intent": sign_intent,
                    "plan_spec": crate::coding_orchestrator::plan_json(plan_id),
                    "host_check": submit_val.get("host_check").cloned()
                        .or_else(|| create_val.get("host_check").cloned()),
                    "gate_notes": submit_val.get("gate_notes").cloned()
                        .or_else(|| create_val.get("gate_notes").cloned()),
                    "audit_env": crate::review::audit_env_json(),
                })
                .to_string());
            }

            // Start run + skip resolve host step already executed
            let mut run = crate::coding_orchestrator::start_run(
                plan_id,
                request.clone(),
                project.clone(),
                resolve_val.clone(),
            );
            crate::coding_orchestrator::skip_completed_host_prefix(&mut run, &["resolve"]);
            let next = crate::coding_orchestrator::next_action_json(&run);
            Ok(json!({
                "ok": true,
                "plan": plan_id.as_str(),
                "action": "start",
                "phase": next.get("phase").cloned().unwrap_or(json!("agent_step")),
                "summary": format!(
                    "Started {} — next agent step: {}",
                    plan_id.as_str(),
                    next.get("step_id").and_then(|v| v.as_str()).unwrap_or("?")
                ),
                "resolve": resolve_val,
                "plan_spec": crate::coding_orchestrator::plan_json(plan_id),
                "playbook": crate::coding_orchestrator::agent_playbook(plan_id, &resolve_val),
                "next": next,
                "run": crate::coding_orchestrator::run_status_json(&run),
                "hint": "Perform next.tool, then run_coding_plan({plan, action:\"next\"}). When fix loop done: action=next through finish or plan coding.finish_task"
            })
            .to_string())
        }

        // ─── Coding target resolve (open unmerged PRs) ───────────────────
        "resolve_coding_target" => {
            let request = arg_str(arguments, &["request", "task", "query", "message"])
                .unwrap_or_else(|| "".into());
            let project = arg_str(arguments, &["slug", "project"]).or_else(|| {
                crate::coding_gates::current_project_slug()
            });
            // Explicit operator/agent choice (after modal ACK or tool arg)
            if let Some(choice) = arg_str(arguments, &["choice", "pr_id", "pr_id"]) {
                if choice == "__new__" || choice.eq_ignore_ascii_case("new") {
                    if let Some(ref slug) = project {
                        if let Some(h) = crate::coding_gates::project_session(Some(slug)) {
                            let _ = h.set_active_pr_id(None);
                        }
                    }
                    return Ok(json!({
                        "ok": true,
                        "decision": "new",
                        "summary": "Coding target: create new work line / new PR at task end",
                        "project": project,
                        "active_pr_id": null,
                        "hint": "create_branch for multi-step; session_commit per slice; create_pr+submit_pr when done"
                    })
                    .to_string());
                }
                if let Some(ref slug) = project {
                    if let Some(h) = crate::coding_gates::project_session(Some(slug)) {
                        crate::coding_resolve::bind_session_to_pr(&h, &choice)?;
                    }
                }
                return Ok(json!({
                    "ok": true,
                    "decision": "bind",
                    "pr_id": choice,
                    "pr_id": choice,
                    "summary": format!("Bound coding session to open pull request {choice}"),
                    "project": project,
                    "active_pr_id": choice,
                    "hint": "Reuse this PR — session_commit slices; submit_pr when task done (do not open a second PR)"
                })
                .to_string());
            }

            let path = match &project {
                Some(p) if !p.is_empty() => {
                    // Prefer open statuses; client also filters Merged
                    format!("/api/pull_requests")
                }
                _ => "/api/pull_requests".to_string(),
            };
            let (_status, data) = http_json("GET", &path, None).await.unwrap_or_else(|e| {
                (0, json!({ "error": e, "pull_requests": [] }))
            });
            let mut candidates = crate::coding_resolve::candidates_from_list(
                &data,
                project.as_deref(),
                &request,
            );
            // Prefer session's already-bound PR when still open
            if let Some(ref slug) = project {
                if let Some(h) = crate::coding_gates::project_session(Some(slug)) {
                    if let Some(aid) = h.snapshot_meta().active_pr_id.clone() {
                        if candidates.iter().any(|c| c.id == aid) {
                            // Boost bound PR slightly
                            for c in &mut candidates {
                                if c.id == aid {
                                    c.score = (c.score + 0.15).min(1.0);
                                }
                            }
                            candidates.sort_by(|a, b| {
                                b.score
                                    .partial_cmp(&a.score)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                    }
                }
            }

            let decision = crate::coding_resolve::decide(&candidates, &request);
            let cand_json: Vec<Value> = candidates
                .iter()
                .take(12)
                .map(crate::coding_resolve::candidate_json)
                .collect();

            match decision {
                crate::coding_resolve::ResolveDecision::New => Ok(json!({
                    "ok": true,
                    "decision": "new",
                    "summary": "No matching open pull request — use a new work line; open PR when task done",
                    "project": project,
                    "candidates": cand_json,
                    "request": request,
                    "hint": "create_branch if multi-step; do not create_pr until task complete"
                })
                .to_string()),
                crate::coding_resolve::ResolveDecision::Bind { pr_id } => {
                    if let Some(ref slug) = project {
                        if let Some(h) = crate::coding_gates::project_session(Some(slug)) {
                            let _ = crate::coding_resolve::bind_session_to_pr(&h, &pr_id);
                        }
                    }
                    let branch = candidates
                        .iter()
                        .find(|c| c.id == pr_id)
                        .and_then(|c| c.source_branch.clone());
                    Ok(json!({
                        "ok": true,
                        "decision": "bind",
                        "pr_id": pr_id,
                        "pr_id": pr_id,
                        "source_branch": branch,
                        "summary": format!("Auto-bound to open pull request {pr_id} (scope match)"),
                        "project": project,
                        "candidates": cand_json,
                        "request": request,
                        "active_pr_id": pr_id,
                        "hint": "Continue on this PR's branch; session_commit per slice; submit when done"
                    })
                    .to_string())
                }
                crate::coding_resolve::ResolveDecision::NeedsChoice => {
                    let intent = crate::coding_resolve::choose_pr_intent(
                        &request,
                        &candidates,
                        project.as_deref(),
                    );
                    let intent_id = intent
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    crate::focus::register_pending_intent(
                        &intent_id,
                        json!({ "tool": "resolve_coding_target", "request": request }),
                    );
                    Ok(json!({
                        "ok": true,
                        "decision": "needs_choice",
                        "summary": "Multiple open pull requests — operator must choose (Present modal)",
                        "project": project,
                        "candidates": cand_json,
                        "request": request,
                        "intent": intent,
                        "pending_ux": true,
                        "execution": { "domain": "ux", "present": "choose" },
                        "hint": format!(
                            "Wait for Present ACK (wait_intent_ack intent_id={intent_id}) then \
                             resolve_coding_target(choice=<pr_id|new>) if not auto-applied"
                        )
                    })
                    .to_string())
                }
            }
        }

        // ─── SDLC / Pull requests (API path still /api/pull_requests) ───
        "list_prs" | "open_prs" => {
            let navigate = arg_bool(arguments, "navigate", true);
            let status_filter = arg_str(arguments, &["status"]);
            let path = match &status_filter {
                Some(s) => format!("/api/pull_requests?status={}", s),
                None => "/api/pull_requests".to_string(),
            };
            let (status, data) = http_json("GET", &path, None).await.unwrap_or_else(|e| {
                (0, json!({ "error": e }))
            });
            let mut out = json!({
                "ok": ok_status(status) || status == 0 && data.get("error").is_none(),
                "summary": "Listed pull requests (SDLC)",
                "http_status": status,
                "pull_requests": data,
                "pull_requests": data,
                "api": format!("{base}/api/pull_requests"),
            });
            if navigate {
                out["navigation"] = json!({ "action": "goto", "path": "/pulls" });
                out["intent"] = crate::focus::page_action_intent(
                    "list_prs",
                    "/pulls",
                    "Open pull requests",
                );
                out["execution"] = json!({ "domain": "none", "present": "goto" });
            }
            // ok even if API empty — navigation still useful
            if status == 0 {
                out["ok"] = json!(true);
                out["summary"] = json!("Open pull requests (API unavailable — UI only)");
            } else {
                out["ok"] = json!(ok_status(status));
            }
            Ok(out.to_string())
        }

        "create_pr" | "open_create_pr" => {
            let via = arg_str(arguments, &["via"])
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            let domain_mode = if via == "ux" {
                crate::focus::DomainMode::Ux
            } else if via == "server" {
                crate::focus::DomainMode::Server
            } else if crate::focus::client_present() {
                crate::focus::DomainMode::Ux
            } else {
                crate::focus::DomainMode::Server
            };
            let force_new = arg_bool(arguments, "force_new", false);

            // Reuse **open** PR bound on session (never Merged / Closed).
            if !force_new {
                let project = arg_str(arguments, &["slug", "project", "repo", "repo_id"]).or_else(
                    crate::coding_gates::current_project_slug,
                );
                if let Some(ref slug) = project {
                    if let Some(h) = crate::coding_gates::project_session(Some(slug)) {
                        if let Some(aid) = h
                            .snapshot_meta()
                            .active_pr_id
                            .clone()
                            .filter(|s| !s.is_empty())
                        {
                            let path = format!("/api/pull_requests/{}", urlencoding_path(&aid));
                            let (st, data) =
                                http_json("GET", &path, None).await.unwrap_or((0, json!({})));
                            let status = data
                                .pointer("/pull_request/status")
                                .or_else(|| data.get("status"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let still_open =
                                ok_status(st) && crate::coding_resolve::is_open_status(status);
                            if still_open {
                                let ui_path = format!("/pulls/{aid}");
                                let host_check =
                                    crate::coding_gates::host_check_value(&h.snapshot_meta());
                                let branch = h
                                    .snapshot_meta()
                                    .branch_name
                                    .clone()
                                    .filter(|s| !s.is_empty() && s != "main" && s != "master");
                                let mut publish = json!(null);
                                if let Some(ref b) = branch {
                                    if let Ok(v) = crate::pr_writeback::publish_session_for_change(
                                        slug, b, &aid,
                                    ) {
                                        publish = v;
                                    }
                                }
                                return Ok(json!({
                                    "ok": true,
                                    "reused": true,
                                    "summary": format!(
                                        "Reusing open pull request {aid} (session active_pr_id). \
                                         Pass force_new=true to open another PR. Call submit_pr when ready."
                                    ),
                                    "pull_request": { "id": aid, "status": status },
                                    "host_check": host_check,
                                    "publish": publish,
                                    "gate_notes": [
                                        "HINT: scope already bound — prefer submit_pr over a second create_pr"
                                    ],
                                    "navigation": { "action": "goto", "path": ui_path },
                                    "execution": { "domain": "server", "present": "illustrate" }
                                })
                                .to_string());
                            }
                            let _ = h.set_active_pr_id(None);
                        }
                    }
                }
            }

            // If title provided → create (or schedule UX commit); else open form
            if let Some(title) = arg_str(arguments, &["title"]) {
                let project = arg_str(arguments, &["slug", "project", "repo", "repo_id"]);
                let description = arg_str(arguments, &["description", "body"]);

                if domain_mode == crate::focus::DomainMode::Ux {
                    let path = "/pulls".to_string();
                    let intent = crate::focus::create_pr_intent(
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
                        "summary": format!("Scheduled create_pr `{title}` — UX Present then commit"),
                        "pending_ux": true,
                        "navigation": { "action": "goto", "path": "/pulls/new" },
                        "intent": intent,
                        "execution": { "domain": "ux", "present": "ux_commit" }
                    })
                    .to_string());
                }

                // Prefer coding-session branch when agent omits source_branch.
                let source_branch = arg_str(arguments, &["source_branch", "branch"]).or_else(|| {
                    let slug = project.as_deref().unwrap_or("");
                    if slug.is_empty() {
                        return None;
                    }
                    crate::session::SessionManager::global()
                        .resolve_for_project(slug)
                        .ok()
                        .and_then(|h| {
                            let m = h.snapshot_meta();
                            m.branch_name.clone().or_else(|| {
                                if m.draft_mode {
                                    Some("work".into())
                                } else {
                                    None
                                }
                            })
                        })
                });
                let mut desc = description.clone().unwrap_or_default();
                if let Some(slug) = &project {
                    if !desc.to_lowercase().contains("project:") {
                        desc = format!("project: {slug}\n\n{desc}");
                    }
                }
                // Inject agent rationales from edit/write_source cache for PR Wizard.
                let rats = crate::api::snapshot_rationales();
                if !rats.is_empty() {
                    desc.push_str("\n\n## Rationales\n");
                    for (name, intent) in rats.iter().take(40) {
                        if name == "*" {
                            desc.push_str(&format!("- **package**: {intent}\n"));
                        } else {
                            desc.push_str(&format!("- **{name}**: {intent}\n"));
                        }
                    }
                }
                // Attach recent commits so PR Wizard has rationales/history.
                let mut commit_count = 0usize;
                let sess = project
                    .as_deref()
                    .and_then(|s| crate::coding_gates::project_session(Some(s)));
                if let Some(ref h) = sess {
                    if let Ok(commits) = crate::session::list_session_commits(&h.session_id()) {
                        commit_count = commits.len();
                        if !commits.is_empty() {
                            desc.push_str("\n\n## Commits\n");
                            for c in commits.iter().take(20) {
                                desc.push_str(&format!("- {}\n", c.message));
                            }
                        }
                    }
                }
                let gate_notes =
                    crate::coding_gates::gate_open_pr_notes(sess.as_deref(), commit_count);
                let host_check = sess
                    .as_ref()
                    .map(|h| crate::coding_gates::host_check_value(&h.snapshot_meta()))
                    .unwrap_or_else(|| json!({ "severity": "unknown", "source": "host" }));
                let jira = arg_str(arguments, &["jira_ticket", "jira"]).unwrap_or_else(|| {
                    let secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    format!("VEIL-{secs}")
                });
                let body = json!({
                    "title": title,
                    "description": desc,
                    "repo_id": arg_str(arguments, &["repo_id", "repo"]),
                    "slug": project.clone(),
                    "jira_ticket": jira,
                    "author": arg_str(arguments, &["author"]).unwrap_or_else(|| "agent".into()),
                    "source_branch": source_branch,
                });
                let (status, data) =
                    http_json("POST", "/api/pull_requests", Some(body)).await?;
                let pr_id = data
                    .pointer("/pull_request/id")
                    .or_else(|| data.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Publish session worktree onto PR source_branch so structural diff sees edits.
                let mut publish = json!(null);
                if ok_status(status) && !pr_id.is_empty() {
                    if let (Some(slug), Some(branch)) = (project.as_deref(), source_branch.as_deref())
                    {
                        match crate::pr_writeback::publish_session_for_change(slug, branch, &pr_id)
                        {
                            Ok(v) => publish = v,
                            Err(e) => {
                                tracing::warn!(error = %e, "publish session for PR failed");
                                publish = json!({ "ok": false, "error": e });
                            }
                        }
                    } else if let Some(slug) = project.as_deref() {
                        // Fallback branch name from PR payload
                        let branch = data
                            .pointer("/pull_request/source_branch")
                            .and_then(|v| v.as_str())
                            .unwrap_or("work");
                        match crate::pr_writeback::publish_session_for_change(slug, branch, &pr_id)
                        {
                            Ok(v) => publish = v,
                            Err(e) => publish = json!({ "ok": false, "error": e }),
                        }
                    }
                }
                let path = if pr_id.is_empty() {
                    "/pulls".to_string()
                } else {
                    format!("/pulls/{pr_id}")
                };
                let intent = crate::focus::create_pr_intent(
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
                    if let Some(slug) = project.as_deref() {
                        let _ = crate::review::record_pr(
                            slug,
                            &title,
                            if pr_id.is_empty() { None } else { Some(pr_id.as_str()) },
                        );
                    }
                }
                Ok(json!({
                    "ok": ok_status(status),
                    "http_status": status,
                    "summary": if ok_status(status) {
                        format!(
                            "Opened pull request: {title}. Session published for PR Wizard review. Call submit_pr next — do NOT merge."
                        )
                    } else {
                        format!("create_pr (open PR) failed (HTTP {status})")
                    },
                    "pull_request": data,
                    "pull_request": data,
                    "publish": publish,
                    "host_check": host_check,
                    "gate_notes": gate_notes,
                    "navigation": { "action": "goto", "path": path },
                    "intent": intent,
                    "execution": {
                        "domain": "server",
                        "present": if ok_status(status) { "illustrate" } else { "none" }
                    }
                })
                .to_string())
            } else {
                // Agent often omits title — synthesize so Present can fill the form
                // and we still create a real CR (empty /pulls/new + bare submit_pr
                // left operators on a blank form and reported "fixed" with no PR).
                let project = arg_str(arguments, &["slug", "project", "repo", "repo_id"]);
                let branch = arg_str(arguments, &["source_branch", "branch"]).or_else(|| {
                    let slug = project.as_deref().unwrap_or("");
                    if slug.is_empty() {
                        return None;
                    }
                    crate::session::SessionManager::global()
                        .resolve_for_project(slug)
                        .ok()
                        .and_then(|h| {
                            let m = h.snapshot_meta();
                            m.branch_name.clone().or_else(|| {
                                let b = m.branch.clone();
                                if b.is_empty() {
                                    None
                                } else {
                                    Some(b)
                                }
                            })
                        })
                });
                let title = branch
                    .as_deref()
                    .filter(|b| !b.is_empty() && *b != "main" && *b != "master")
                    .map(|b| format!("Agent: {b}"))
                    .unwrap_or_else(|| "Agent coding changes".to_string());
                let mut desc = arg_str(arguments, &["description", "body"]).unwrap_or_default();
                if let Some(slug) = &project {
                    if !desc.to_lowercase().contains("project:") {
                        desc = format!("project: {slug}\n\n{desc}");
                    }
                }
                let rats = crate::api::snapshot_rationales();
                if !rats.is_empty() {
                    desc.push_str("\n\n## Rationales\n");
                    for (name, intent) in rats.iter().take(40) {
                        if name == "*" {
                            desc.push_str(&format!("- **package**: {intent}\n"));
                        } else {
                            desc.push_str(&format!("- **{name}**: {intent}\n"));
                        }
                    }
                }
                let mut commit_count = 0usize;
                let sess = project
                    .as_deref()
                    .and_then(|s| crate::coding_gates::project_session(Some(s)));
                if let Some(ref h) = sess {
                    if let Ok(commits) = crate::session::list_session_commits(&h.session_id()) {
                        commit_count = commits.len();
                        if !commits.is_empty() {
                            desc.push_str("\n\n## Commits\n");
                            for c in commits.iter().take(20) {
                                desc.push_str(&format!("- {}\n", c.message));
                            }
                        }
                    }
                }
                let gate_notes =
                    crate::coding_gates::gate_open_pr_notes(sess.as_deref(), commit_count);
                let host_check = sess
                    .as_ref()
                    .map(|h| crate::coding_gates::host_check_value(&h.snapshot_meta()))
                    .unwrap_or_else(|| json!({ "severity": "unknown", "source": "host" }));
                let mut body = json!({
                    "title": title,
                    "description": desc,
                    "author": "agent",
                });
                if let Some(b) = &branch {
                    body["source_branch"] = json!(b);
                }
                if let Some(slug) = &project {
                    body["project"] = json!(slug);
                    body["slug"] = json!(slug);
                }
                let (status, data) =
                    http_json("POST", "/api/pull_requests", Some(body)).await?;
                let path = if ok_status(status) {
                    data.get("pull_request")
                        .and_then(|c| c.get("id"))
                        .and_then(|id| id.as_str())
                        .map(|id| format!("/pulls/{id}"))
                        .unwrap_or_else(|| "/pulls".into())
                } else {
                    "/pulls/new".into()
                };
                let intent = crate::focus::create_pr_intent(
                    Some(&title),
                    Some(&desc),
                    project.as_deref(),
                    &path,
                    crate::focus::DomainMode::Server,
                );
                Ok(json!({
                    "ok": ok_status(status),
                    "http_status": status,
                    "summary": if ok_status(status) {
                        format!(
                            "Opened pull request (synthesized title `{title}` — pass title next time). Call submit_pr next — do NOT merge."
                        )
                    } else {
                        format!("create_pr (open PR) failed (HTTP {status})")
                    },
                    "pull_request": data,
                    "pull_request": data,
                    "host_check": host_check,
                    "gate_notes": gate_notes,
                    "navigation": { "action": "goto", "path": path },
                    "intent": intent,
                    "execution": {
                        "domain": "server",
                        "present": if ok_status(status) { "illustrate" } else { "none" }
                    }
                })
                .to_string())
            }
        }

        "get_pr" => {
            let id = arg_str(arguments, &["id", "pr_id", "pull_request_id"])
                .ok_or_else(|| "get_pr requires id".to_string())?;
            let (status, data) =
                http_json("GET", &format!("/api/pull_requests/{}", urlencoding_path(&id)), None)
                    .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Pull request {id}"),
                "pull_request": data,
                "pull_request": data,
                "navigation": { "action": "goto", "path": format!("/pulls/{id}") }
            })
            .to_string())
        }

        "submit_pr" => {
            let id = arg_str(arguments, &["id", "pr_id"])
                .ok_or_else(|| "submit_pr requires id".to_string())?;
            // Re-publish latest session tree before review so PR Wizard is current.
            let mut publish = json!(null);
            let slug = arg_str(arguments, &["slug", "project"]).or_else(|| {
                crate::provider::hub::CURRENT_PROJECT
                    .try_with(|n| n.clone())
                    .ok()
            });
            let sess = slug
                .as_deref()
                .and_then(|s| crate::coding_gates::project_session(Some(s)));
            if let (Some(slug), Some(h)) = (slug.as_deref(), sess.as_ref()) {
                let m = h.snapshot_meta();
                let branch = m
                    .branch_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "work".into());
                match crate::pr_writeback::publish_session_for_change(slug, &branch, &id) {
                    Ok(v) => publish = v,
                    Err(e) => {
                        tracing::warn!(error = %e, "submit_pr re-publish failed");
                        let _ = h.set_active_pr_id(Some(&id));
                    }
                }
            }
            let gate_notes = crate::coding_gates::gate_submit_pr_notes(sess.as_deref());
            let host_check = sess
                .as_ref()
                .map(|h| crate::coding_gates::host_check_value(&h.snapshot_meta()))
                .unwrap_or_else(|| json!({ "severity": "unknown", "source": "host" }));
            // Hard refuse when VEIL_STRICT_SUBMIT=1 and host still has errors
            if crate::coding_gates::strict_submit_enabled() {
                if let Some(ref h) = sess {
                    if crate::coding_gates::has_host_errors(&h.snapshot_meta()) {
                        return Ok(json!({
                            "ok": false,
                            "error": "GATE: VEIL_STRICT_SUBMIT=1 — host_check still has Errors; fix before submit_pr",
                            "host_check": host_check,
                            "gate_notes": gate_notes,
                            "strict_submit": true,
                            "summary": "Submit blocked: working set has host Errors (strict mode)",
                        })
                        .to_string());
                    }
                }
            }
            let (status, data) = http_json(
                "POST",
                &format!("/api/pull_requests/{}/submit", urlencoding_path(&id)),
                Some(json!({})),
            )
            .await?;
            let mut summary = format!(
                "Submitted pull request {id} for review — open IDE PR Wizard (Review). Do not merge_branch."
            );
            if gate_notes.iter().any(|n| n.contains("MUST_ACKNOWLEDGE_ERRORS")) {
                summary.push_str(
                    " HOST still reports Errors on the working set — do not claim clean check.",
                );
            }
            let intent = crate::focus::change_action_intent("submit", &id, &summary);
            crate::focus::register_pending_intent(
                intent.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                json!({ "tool": "submit_pr", "id": id }),
            );
            let sign_slug = slug.clone();
            let sign_count = crate::review::list_items(crate::review::ListFilter {
                slug: sign_slug.clone(),
                status: Some(crate::review::ItemStatus::Outstanding),
                ..Default::default()
            })
            .len();
            let sign_intent = crate::review::request_sign_off_intent(sign_slug.as_deref(), sign_count);
            let review_path = sign_slug
                .as_deref()
                .map(|s| format!("/review/{s}"))
                .unwrap_or_else(|| "/review".into());
            summary.push_str(" Present the set on Sign-off — do not merge.");
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "publish": publish,
                "host_check": host_check,
                "gate_notes": gate_notes,
                "navigation": { "action": "goto", "path": review_path },
                "intent": sign_intent,
                "submit_intent": intent,
                "execution": { "domain": "server", "present": "goto" },
                "audit_env": crate::review::audit_env_json(),
            })
            .to_string())
        }

        "approve_pr" => {
            let id = arg_str(arguments, &["id", "pr_id"])
                .ok_or_else(|| "approve_pr requires id".to_string())?;
            let body = json!({
                "reviewer": arg_str(arguments, &["reviewer"]).unwrap_or_default(),
                "comment": arg_str(arguments, &["comment"]),
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/pull_requests/{}/approve", urlencoding_path(&id)),
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
                "navigation": { "action": "goto", "path": format!("/pulls/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "request_pr_changes" => {
            let id = arg_str(arguments, &["id", "pr_id"])
                .ok_or_else(|| "request_pr_changes requires id".to_string())?;
            let body = json!({
                "reviewer": arg_str(arguments, &["reviewer"]).unwrap_or_default(),
                "comment": arg_str(arguments, &["comment"]).unwrap_or_default(),
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/pull_requests/{}/request-changes", urlencoding_path(&id)),
                Some(body),
            )
            .await?;
            let summary = format!("Requested changes on {id}");
            let intent = crate::focus::change_action_intent("request_pr_changes", &id, &summary);
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": summary,
                "result": data,
                "navigation": { "action": "goto", "path": format!("/pulls/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "merge_pr" => {
            let id = arg_str(arguments, &["id", "pr_id"])
                .ok_or_else(|| "merge_pr requires id".to_string())?;
            let slug_gate = arg_str(arguments, &["slug", "project"]);
            if let Some(ref s) = slug_gate {
                if let Err(e) = crate::review::may_ship(s, None) {
                    return Ok(json!({
                        "ok": false,
                        "error": "sign_off_required",
                        "summary": e,
                        "hint": "Call request_sign_off and wait for the human. Do not merge.",
                        "navigation": { "action": "goto", "path": format!("/review/{s}") },
                    })
                    .to_string());
                }
            }
            let body = json!({
                "merger": arg_str(arguments, &["merger"]).unwrap_or_default(),
                "slug": arg_str(arguments, &["slug"]).unwrap_or_default(),
            });
            let (status, data) = http_json(
                "POST",
                &format!("/api/pull_requests/{}/merge", urlencoding_path(&id)),
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
                "navigation": { "action": "goto", "path": format!("/pulls/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "add_comment" => {
            let id = arg_str(arguments, &["id", "pr_id"])
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
                &format!("/api/pull_requests/{}/comments", urlencoding_path(&id)),
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
                "navigation": { "action": "goto", "path": format!("/pulls/{id}") },
                "intent": intent,
                "execution": { "domain": "server", "present": "illustrate" }
            })
            .to_string())
        }

        "get_pr_diff" => {
            let id = arg_str(arguments, &["id", "pr_id"])
                .ok_or_else(|| "get_pr_diff requires id".to_string())?;
            let (status, data) = http_json(
                "GET",
                &format!("/api/pull_requests/{}/diff", urlencoding_path(&id)),
                None,
            )
            .await?;
            Ok(json!({
                "ok": ok_status(status),
                "http_status": status,
                "summary": format!("Structural diff for change {id}"),
                "diff": data,
                "navigation": { "action": "goto", "path": format!("/pulls/{id}") }
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
            if let Err(e) = crate::review::may_ship(&project_slug, None) {
                return Ok(json!({
                    "ok": false,
                    "error": "sign_off_required",
                    "summary": e,
                    "hint": "Call request_sign_off and wait. Do not ship unsigned work.",
                    "navigation": { "action": "goto", "path": format!("/review/{project_slug}") },
                })
                .to_string());
            }
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
        "open_registry" => {
            let intent =
                crate::focus::page_action_intent("open_registry", "/registry", "Open registry");
            Ok(json!({
                "ok": true,
                "summary": "Open registry",
                "navigation": { "action": "goto", "path": "/registry" },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

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
                "navigation": { "action": "goto", "path": "/registry" },
                "intent": crate::focus::page_action_intent(
                    "search_registry",
                    "/registry",
                    "Open registry",
                ),
                "execution": { "domain": "none", "present": "goto" }
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
                "navigation": { "action": "goto", "path": "/registry" },
                "intent": crate::focus::page_action_intent(
                    "list_registry_stubs",
                    "/registry",
                    "Open registry stubs",
                ),
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        // ─── Config / meta ────────────────────────────────────────────────
        "open_dashboard" => {
            let intent =
                crate::focus::page_action_intent("open_dashboard", "/dashboard", "Open dashboard");
            Ok(json!({
                "ok": true,
                "summary": "Open dashboard",
                "navigation": { "action": "goto", "path": "/dashboard" },
                "intent": intent,
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

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
            let branch = arg_str(arguments, &["branch"]).unwrap_or_else(|| {
                crate::session::current_session_id()
                    .and_then(|sid| crate::session::get_session_meta(&sid).ok())
                    .map(|m| m.branch)
                    .filter(|b| !b.is_empty())
                    .unwrap_or_else(|| "main".into())
            });
            let body = json!({
                "content": content,
                "branch": branch,
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
            let mut ctx = crate::focus::context_tool_json(sid.as_deref());
            // Enrich with live ACP / task-local project so agent never confuses
            // "UI focus not published" with "no product exists".
            let live_proj = crate::provider::hub::CURRENT_PROJECT
                .try_with(|n| n.clone())
                .ok()
                .or_else(crate::acp::get_acp_project);
            if let Some(slug) = live_proj {
                ctx["live_project"] = json!(slug);
                ctx["ok"] = json!(true);
                if ctx.get("focus").and_then(|f| f.as_object()).is_none()
                    || ctx
                        .pointer("/focus/project")
                        .and_then(|p| p.as_str())
                        .filter(|s| !s.is_empty())
                        .is_none()
                {
                    ctx["focus"] = json!({
                        "project": slug,
                        "route": format!("/projects/{slug}/ide"),
                        "source": "server_scope",
                    });
                    ctx["summary"] = json!(format!(
                        "Project `{slug}` is bound server-side (ACP/task scope). Use list_files / read_source."
                    ));
                }
                ctx["hint"] = json!(
                    "live_project is authoritative for coding tools. If focus.project was empty, still use live_project."
                );
            }
            Ok(ctx.to_string())
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
                    "hint": "Call after create_project(via=ux) / create_pr(via=ux) so the browser can finish Present. Or use via=server for multi-step without wait."
                })
                .to_string()),
            }
        }

        "list_outstanding" => {
            let slug = arg_str(arguments, &["project", "slug", "id"]);
            let snap = crate::review::snapshot_json(crate::review::ListFilter {
                slug: slug.clone(),
                session_id: None,
                status: Some(crate::review::ItemStatus::Outstanding),
            });
            let count = snap
                .get("outstanding")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let path = match slug.as_deref() {
                Some(s) if !s.is_empty() => format!("/review/{s}"),
                _ => "/review".to_string(),
            };
            Ok(json!({
                "ok": true,
                "summary": if count == 0 {
                    "No outstanding unreviewed changes.".to_string()
                } else {
                    format!("{count} outstanding change(s) need sign-off. Present this set to the human; call request_sign_off.")
                },
                "outstanding": count,
                "items": snap.get("items"),
                "by_project": snap.get("by_project"),
                "navigation": { "action": "goto", "path": path },
            })
            .to_string())
        }

        "request_sign_off" => {
            let slug = arg_str(arguments, &["project", "slug", "id"]);
            let items = crate::review::list_items(crate::review::ListFilter {
                slug: slug.clone(),
                session_id: None,
                status: Some(crate::review::ItemStatus::Outstanding),
            });
            let count = items.len();
            let intent = crate::review::request_sign_off_intent(slug.as_deref(), count);
            let intent_id = intent
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            crate::focus::register_pending_intent(
                &intent_id,
                json!({ "tool": "request_sign_off", "count": count }),
            );
            let bullets: Vec<String> = items
                .iter()
                .take(24)
                .map(|it| {
                    let why = it
                        .rationale
                        .as_deref()
                        .map(|r| format!(" — {r}"))
                        .unwrap_or_default();
                    format!("- [{}] {}{why}", it.slug, it.summary)
                })
                .collect();
            Ok(json!({
                "ok": true,
                "summary": if count == 0 {
                    "Nothing outstanding to sign off.".to_string()
                } else {
                    format!(
                        "Here is exactly what I did and why ({count} items). I need you to sign off before I proceed / merge / deploy.\n{}",
                        bullets.join("\n")
                    )
                },
                "outstanding": count,
                "items": items,
                "intent_id": intent_id,
                "intent": intent,
                "navigation": intent.get("navigation"),
                "execution": { "domain": "none", "present": "goto" }
            })
            .to_string())
        }

        "sign_off" => {
            let slug = arg_str(arguments, &["project", "slug", "id"]);
            let decision = arg_str(arguments, &["decision"])
                .unwrap_or_else(|| "approve".into());
            let via = arg_str(arguments, &["via"]).unwrap_or_default();
            // Agent may navigate + highlight. It must not press Sign off.
            if via == "ux" || crate::focus::client_present() {
                let intent = crate::review::request_sign_off_intent(
                    slug.as_deref(),
                    crate::review::list_items(crate::review::ListFilter {
                        slug: slug.clone(),
                        status: Some(crate::review::ItemStatus::Outstanding),
                        ..Default::default()
                    })
                    .len(),
                );
                let intent_id = intent
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                crate::focus::register_pending_intent(
                    &intent_id,
                    json!({ "tool": "sign_off", "present_only": true }),
                );
                return Ok(json!({
                    "ok": true,
                    "summary": "Opened Sign-off for the human. Do not approve this set yourself.",
                    "pending_ux": true,
                    "intent_id": intent_id,
                    "intent": intent,
                    "execution": { "domain": "none", "present": "goto" }
                })
                .to_string());
            }
            if !crate::review::veil_dev_enabled() {
                return Ok(json!({
                    "ok": false,
                    "error": "human_sign_off_required",
                    "summary": "sign_off via=server is forbidden. Call request_sign_off and wait.",
                    "navigation": {
                        "action": "goto",
                        "path": slug.as_deref().map(|s| format!("/review/{s}")).unwrap_or_else(|| "/review".into())
                    }
                })
                .to_string());
            }
            match crate::review::sign_off(crate::review::SignOffRequest {
                ids: arguments
                    .get("ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                slug: slug.clone(),
                all: slug.is_none(),
                decision: decision.clone(),
                actor: arg_str(arguments, &["actor"]).unwrap_or_else(|| "agent".into()),
                note: arg_str(arguments, &["note", "message"]),
                via: Some("server".into()),
                ..Default::default()
            }) {
                Ok((items, audit)) => Ok(json!({
                    "ok": true,
                    "summary": format!(
                        "Recorded {} sign-off on {} item(s). Remaining outstanding: {}.",
                        audit.decision,
                        items.len(),
                        crate::review::outstanding().len()
                    ),
                    "items": items,
                    "audit": audit,
                    "navigation": {
                        "action": "goto",
                        "path": slug.as_deref().map(|s| format!("/review/{s}")).unwrap_or_else(|| "/review".into())
                    }
                })
                .to_string()),
                Err(e) => Err(e),
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
///
/// `name` is the **display name** (e.g. `"Agent Registry"`). Storage derives
/// `slug = name.to_lowercase().replace(' ', '-')` → `agent-registry`.
///
/// **Idempotent:** if a repo with the same slug already exists, returns that
/// project (ok: true, existing: true) instead of creating a duplicate.
pub async fn create_project_domain(name: &str, description: Option<&str>) -> Result<Value, String> {
    use crate::provider::s3_workspace::{
        allow_disk_project_create, ide_source_mode, seed_new_repo_scaffold, IdeSourceMode,
    };

    let mode = ide_source_mode();
    let remote_only = matches!(mode, IdeSourceMode::S3);
    let want_slug = crate::project_layout::slugify_name(name);

    // Idempotent: reuse existing product with same slug
    if let Ok((st, list)) = http_json("GET", "/api/repos", None).await {
        if ok_status(st) {
            let items = list
                .as_array()
                .cloned()
                .or_else(|| list.get("repos").and_then(|r| r.as_array()).cloned())
                .unwrap_or_default();
            for repo in items {
                let slug = repo
                    .get("slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if slug == want_slug || crate::project_layout::slugify_name(
                    repo.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                ) == want_slug
                {
                    let bind = crate::agent_scope::prepare_project(&want_slug, None).ok();
                    return Ok(json!({
                        "ok": true,
                        "existing": true,
                        "summary": format!(
                            "Project `{want_slug}` already exists — reusing (not creating a duplicate)"
                        ),
                        "http_status": 200,
                        "project": repo,
                        "name": repo.get("name").cloned().unwrap_or(json!(name)),
                        "slug": want_slug,
                        "source_mode": match mode {
                            IdeSourceMode::S3 => "s3",
                            IdeSourceMode::PreferS3 => "prefer_s3",
                            IdeSourceMode::Disk => "disk",
                        },
                        "session": bind,
                        "hint": "Continue with write_source / create_file / update_mission — do NOT create_project again. Product annotations belong in layers/*.layer (`ann`), not a platform ticket.",
                    }));
                }
            }
        }
    }

    // Keep human title for DDB `name`; slug is derived server-side.
    let body = json!({
        "name": name,
        "description": description,
    });
    let (status, data) = http_json("POST", "/api/repos", Some(body))
        .await
        .unwrap_or((0, json!({})));

    let mut scaffold: Option<Value> = None;
    let mut scaffold_err: Option<String> = None;
    let mut rebound_session: Option<Value> = None;
    if ok_status(status) {
        let repo_id = data
            .pointer("/id/value")
            .or_else(|| data.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let slug_for_seed = data
            .get("slug")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| crate::project_layout::slugify_name(name));
        if let Some(rid) = repo_id {
            // Seed with slug (filesystem-safe); MISSION title still gets display name via scaffold.
            match seed_new_repo_scaffold(&rid, name) {
                Ok(files) => {
                    scaffold = Some(json!({
                        "repo_id": rid.clone(),
                        "files": files,
                        "store": "s3",
                    }));
                    // Drop orphan same-slug sessions (wrong repo_id) and open mainline.
                    if crate::session::sessions_enabled() {
                        match crate::session::SessionManager::global()
                            .rebind_after_repo_create(&slug_for_seed)
                        {
                            Ok(h) => {
                                rebound_session = Some(json!({
                                    "session_id": h.session_id(),
                                    "repo_id": h.snapshot_meta().repo_id,
                                    "slug": slug_for_seed,
                                }));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    slug = %slug_for_seed,
                                    error = %e,
                                    "rebind_after_repo_create failed"
                                );
                            }
                        }
                    }
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
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| crate::project_layout::slugify_name(name));
    let display_name = data
        .get("name")
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
                format!(
                    "Created remote project `{display_name}` (slug `{slug}`, DDB + S3 scaffold)"
                )
            } else {
                format!("Created project `{display_name}` (slug `{slug}`)")
            }
        } else {
            format!("create_project failed for `{name}` (HTTP {status})")
        },
        "http_status": status,
        "project": data,
        "name": display_name,
        "slug": slug,
        "source_mode": source_mode,
        "scaffold": scaffold,
        "scaffold_error": scaffold_err,
        "session": rebound_session,
        "hint": if ok_status(status) {
            "Project is bound. Write layers/*.layer (declare product `ann`s there), MISSION.md, and main.veil via write_source/create_file NOW. Do not wiki-search for whether @on/@command exist in ddd — author them in the product layer. Do not create_project again."
        } else {
            "Report the error; do not invent a local disk tree."
        },
    }))
}

/// MCP / agent tool definitions for platform UX (superset of pure navigation).
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "navigate_to",
            "description": "Navigate the VEIL runtime dashboard SPA to a path. Use for any UI destination: /dashboard, /projects, /projects/{id}, /pulls, /pulls/new, /review, /deploy, /registry, /config, /agents.",
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
            "description": "Create a new product project/repo. Returns intent.present (form fill + pulse). via=ux: UX commits after Present (browser); via=server: domain first (default for multi-step/ACP). Do NOT re-create or curl. ALWAYS use when the user asks to create a project. On success the project is bound — immediately write layers/*.layer (declare product annotations with `ann`), MISSION.md, and main.veil. Do not wiki-search for whether those annotations exist in ddd.layer.",
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
            "name": "rename_project",
            "description": "Rename a product project (display name and optionally slug). ALWAYS use this when the user asks to rename/retitle a project — NEVER curl/PATCH /api/repos or use Bitbucket. Default: update display name only; pass new_slug to also change the URL slug. S3 keys stay on repo UUID. Bound project is used when project is omitted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Current id or slug (default: bound project)" },
                    "slug": { "type": "string", "description": "Current slug (alias of project)" },
                    "id": { "type": "string" },
                    "name": { "type": "string", "description": "New display name (e.g. Agent Core)" },
                    "new_slug": { "type": "string", "description": "Optional new URL slug; omit to keep the current slug" },
                    "description": { "type": "string" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "update_project",
            "description": "Update project metadata (name, optional new_slug, description). Alias of rename_project. Do NOT PATCH /api/repos yourself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "new_slug": { "type": "string" },
                    "description": { "type": "string" },
                    "clear_description": { "type": "boolean" }
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
            "name": "list_outstanding",
            "description": "List unreviewed mutations (outstanding change set) across projects or for one slug. Review state — not git status. Use before request_sign_off. Surfaces what the human must sign off.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "request_sign_off",
            "description": "Present the outstanding change set to the human and navigate to the review / sign-off surface. Call after a coherent unit of work. Says what changed and why. Does not merge.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "sign_off",
            "description": "Do NOT approve as the agent. Navigates the human to Sign-off (highlight only). via=server is forbidden unless VEIL_DEV. The human button writes the audit and unlocks merge/deploy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "ids": { "type": "array", "items": { "type": "string" } },
                    "decision": { "type": "string", "enum": ["approve", "reject"] },
                    "note": { "type": "string" },
                    "via": { "type": "string", "enum": ["ux", "server"] }
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
            "name": "list_prs",
            "description": "List pull requests (GET /api/pull_requests) and open /pulls. Optional status filter: Draft, ReadyForReview, Approved, Merged, …. Prefer open/unmerged PRs when reusing a work line.",
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
            "name": "create_pr",
            "description": "Open a pull request (PR) for human review (POST /api/pull_requests; product name: PR, not ticket). Default end of agent coding work — prefer this over merge_branch. Pass title + description with per-slice ## headings and rationales for the PR Wizard. Host attaches session commits + project slug + host_check gate notes. Then call submit_pr. Operator reviews in IDE PR Wizard (not auto-merge).",
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
            "name": "get_pr",
            "description": "Get a pull request by id and open its detail page.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "submit_pr",
            "description": "Submit a pull request for human review (PR Wizard). Call after create_pr when agent work is ready for the operator — not after auto-merge. Response includes host_check; if severity=errors the agent must not claim a clean working set.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "approve_pr",
            "description": "Approve a pull request. Human review action — agents use only when the operator explicitly requests approval.",
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
            "name": "request_pr_changes",
            "description": "Request changes on a pull request (review feedback).",
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
            "name": "merge_pr",
            "description": "Merge an approved pull request. OPERATOR GATE — agents must not call this unless the human explicitly asks to merge after review.",
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
            "description": "Add a review comment on a pull request (optional construct_path for structural comments).",
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
            "name": "get_pr_diff",
            "description": "Fetch structural diff for a pull request.",
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
            "name": "resolve_coding_target",
            "description": "At the start of coding work: match the task against open unmerged pull requests (not tickets). Auto-binds when one PR strongly matches scope; returns needs_choice + Present modal when multiple candidates; decision=new when none. Pass choice=pr_id or choice=new after modal ACK. Prefer this before create_pr.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "request": { "type": "string", "description": "Operator task / fix request text for scope matching" },
                    "project": { "type": "string", "description": "Project slug" },
                    "slug": { "type": "string" },
                    "choice": { "type": "string", "description": "After modal: PR id, or 'new' / '__new__'" }
                },
                "required": []
            }
        }),
        json!({
            "name": "run_coding_plan",
            "description": "Host coding step runner. plan=coding.fix_diagnostics|coding.slice|coding.finish_task. action=start|next|status. start resolves open PR + returns next agent step; after each agent tool success call action=next; finish_task opens/submits PR. Aliases create_pr/submit_pr also work for open/submit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "plan": { "type": "string", "description": "coding.fix_diagnostics | coding.slice | coding.finish_task" },
                    "action": { "type": "string", "description": "start (default) | next | status" },
                    "request": { "type": "string", "description": "Operator task / diagnostics summary" },
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "title": { "type": "string", "description": "PR title for finish_task" },
                    "skip": { "type": "boolean", "description": "With action=next: skip current agent step (e.g. branch already exists)" }
                },
                "required": ["plan"]
            }
        }),
        json!({
            "name": "create_pr",
            "description": "Alias for create_pr — open a pull request for human review (not a ticket).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "source_branch": { "type": "string" },
                    "force_new": { "type": "boolean" }
                },
                "required": []
            }
        }),
        json!({
            "name": "submit_pr",
            "description": "Alias for submit_pr — submit pull request to PR Wizard. Respects VEIL_STRICT_SUBMIT=1 (hard-block on host Errors).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "pr_id": { "type": "string" },
                    "project": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "list_prs",
            "description": "Alias for list_prs — list pull requests.",
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
            "name": "wait_intent_ack",
            "description": "Block until the browser finishes Present for an intent_id (from create_project via=ux / create_pr via=ux / resolve_coding_target needs_choice). Call AFTER the create tool so Present can stream first — then wait before write_source. timeout_ms default 45000.",
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
pub struct RenameProjectArgs {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub new_slug: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Default)]
pub struct RenameProjectTool;

impl Tool for RenameProjectTool {
    const NAME: &'static str = "rename_project";
    type Error = ToolErr;
    type Args = RenameProjectArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Rename a product project display name (and optional slug). Use when the user asks to rename a project. Never curl /api/repos.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "slug": { "type": "string" },
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "new_slug": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut v = json!({});
        if let Some(p) = args.project {
            v["project"] = json!(p);
        }
        if let Some(s) = args.slug {
            v["slug"] = json!(s);
        }
        if let Some(i) = args.id {
            v["id"] = json!(i);
        }
        if let Some(n) = args.name {
            v["name"] = json!(n);
        }
        if let Some(s) = args.new_slug {
            v["new_slug"] = json!(s);
        }
        if let Some(d) = args.description {
            v["description"] = json!(d);
        }
        dispatch("rename_project", &v).await.map_err(ToolErr)
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
    const NAME: &'static str = "list_prs";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List SDLC pull requests and open /pulls.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        dispatch("list_prs", &json!({})).await.map_err(ToolErr)
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
    const NAME: &'static str = "create_pr";
    type Error = ToolErr;
    type Args = CreateChangeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Create pull request (with title) or open /pulls/new.".into(),
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
        dispatch("create_pr", &serde_json::to_value(args).unwrap_or_default())
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
    const NAME: &'static str = "approve_pr";
    type Error = ToolErr;
    type Args = ChangeIdArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Approve a pull request by id.".into(),
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
        dispatch("approve_pr", &serde_json::to_value(args).unwrap_or_default())
            .await
            .map_err(ToolErr)
    }
}

#[derive(Clone, Default)]
pub struct MergeChangeTool;

impl Tool for MergeChangeTool {
    const NAME: &'static str = "merge_pr";
    type Error = ToolErr;
    type Args = ChangeIdArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Merge an approved pull request.".into(),
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
        dispatch("merge_pr", &serde_json::to_value(args).unwrap_or_default())
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
        "rename_project",
        "open_project",
        "open_ide",
        "navigate_to",
        "list_prs",
        "create_pr",
        "approve_pr",
        "merge_pr",
        "provision_project",
        "deploy_status",
        "get_config",
        "list_outstanding",
        "request_sign_off",
        "sign_off",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_project_is_platform_tool() {
        assert!(is_platform_tool("create_project"));
        assert!(is_platform_tool("list_outstanding"));
        assert!(is_platform_tool("request_sign_off"));
        assert!(is_platform_tool("sign_off"));
        assert!(is_platform_tool("create_repo"));
        assert!(is_platform_tool("rename_project"));
        assert!(is_platform_tool("update_project"));
        assert_eq!(canonicalize_tool("rename_project"), "update_project");
        assert!(is_platform_tool("list_projects"));
        assert!(is_platform_tool("approve_pr"));
        assert!(is_platform_tool("provision_project"));
        assert!(is_platform_tool("create_pr"));
        assert!(is_platform_tool("submit_pr"));
        assert!(is_platform_tool("list_prs"));
        assert!(is_platform_tool("run_coding_plan"));
        assert_eq!(canonicalize_tool("create_pr"), "create_pr");
        assert_eq!(canonicalize_tool("submit_pr"), "submit_pr");
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
        let create_desc = defs
            .iter()
            .find(|d| d.get("name").and_then(|n| n.as_str()) == Some("create_project"))
            .and_then(|d| d.get("description").and_then(|s| s.as_str()))
            .unwrap_or("");
        assert!(
            create_desc.contains("layers/*.layer") && create_desc.contains("ann"),
            "{create_desc}"
        );
        assert!(names.contains(&"rename_project"));
        assert!(names.contains(&"list_outstanding"));
        assert!(names.contains(&"request_sign_off"));
        assert!(names.contains(&"sign_off"));
        assert!(names.contains(&"update_project"));
        assert!(names.contains(&"list_projects"));
        assert!(names.contains(&"list_prs"));
        assert!(names.contains(&"approve_pr"));
        assert!(names.contains(&"merge_pr"));
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
