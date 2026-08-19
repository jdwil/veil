//! Built-in agent vertical slice (AGT-001 / AGT-005 / AGT-006).
//!
//! **Agentic execution uses the [Rig](https://rig.rs) SDK** when
//! `VEIL_MODEL_PROVIDER` is `openai` or `ollama`. Tools are typed Rig
//! [`Tool`](rig_core::tool::Tool)s over the VEIL check/edit pipeline.
//!
//! Without a model provider, a small heuristic path remains for offline use
//! (`check` / `outline` / `rename`).

use serde::{Deserialize, Serialize};
use veil_ir::LayerRegistry;

use crate::provider::SourceProvider;
use crate::rig_tools::{self, Workspace};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnRequest {
    pub prompt: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    /// AGT-014: propose edits without applying (also `VEIL_AGENT_PLAN_ONLY=1`).
    #[serde(default)]
    pub plan_only: bool,
    /// Raster diagrams dropped on the agent chat (ACP vision blocks).
    #[serde(default)]
    pub images: Vec<crate::chat_attachments::AgentImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnResponse {
    pub turn_id: String,
    pub messages: Vec<AgentMessage>,
    pub tool_calls: Vec<AgentToolCall>,
    pub source_changed: bool,
    pub ok: bool,
    pub error: Option<String>,
    /// Which backend handled the turn (`rig-openai`, `rig-ollama`, `heuristic`).
    #[serde(default)]
    pub backend: String,
    /// AGT-014: when plan_only, human-readable planned ops (not applied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// True when Tier 0/1 teaching context was truncated to fit the budget.
    #[serde(default)]
    pub context_truncated: bool,
    /// Loud warning when truncated (also mirrored into assistant text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_warning: Option<String>,
    /// Approx tokens in the assembled preamble.
    #[serde(default)]
    pub context_tokens: usize,
    /// Preamble budget (0 = unlimited).
    #[serde(default)]
    pub context_budget_tokens: usize,
    /// Loaded layers for this turn (active file).
    #[serde(default)]
    pub context_layers: Vec<String>,
}

impl AgentTurnResponse {
    fn with_context(mut self, pre: &crate::agent_context::AgentPreamble) -> Self {
        self.context_truncated = pre.truncated;
        self.context_warning = pre.warning.clone();
        self.context_tokens = pre.tokens_used;
        self.context_budget_tokens = pre.max_tokens;
        self.context_layers = pre.layers.clone();
        self
    }
}

/// Run one agent turn against the active source.
pub async fn run_turn<P: SourceProvider>(
    provider: std::sync::Arc<P>,
    req: AgentTurnRequest,
) -> AgentTurnResponse {
    let turn_id = req
        .turn_id
        .clone()
        .unwrap_or_else(|| format!("t-{}", chrono_like_id()));
    let prompt = req.prompt.trim();
    if prompt.is_empty() {
        return AgentTurnResponse {
            turn_id,
            messages: vec![AgentMessage {
                role: "assistant".into(),
                content: "Send a non-empty prompt. With Rig (openai/ollama): free-form + tools. Offline: `check`, `outline`, `rename X to Y`.".into(),
            }],
            tool_calls: vec![],
            source_changed: false,
            ok: true,
            error: None,
            backend: "none".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    let mut messages = vec![AgentMessage {
        role: "user".into(),
        content: prompt.to_string(),
    }];

    // Platform UX tools (navigate + real product ops) — host path, no project required.
    // Chips / "create project X" / list_prs must work even when ACP MCP is incomplete.
    if let Some(ux) = parse_platform_ux_intent(prompt) {
        let mut args = ux.args.clone().unwrap_or_else(|| serde_json::json!({}));
        // Pure create on a browser turn → via=ux (Agent→Present→UX→Server). Multi-step
        // prefix forces via=server so follow-on write_source sees the project.
        if (ux.tool == "create_project" || ux.tool == "create_repo")
            && args.get("via").is_none()
            && crate::focus::client_present()
        {
            args["via"] = serde_json::json!("ux");
            if args.get("open_ide").is_none() {
                args["open_ide"] = serde_json::json!(true);
            }
        }
        if (ux.tool == "create_pr" || ux.tool == "open_create_pr")
            && args.get("via").is_none()
            && crate::focus::client_present()
            && args.get("title").is_some()
        {
            args["via"] = serde_json::json!("ux");
        }
        let detail = match crate::platform_tools::dispatch(&ux.tool, &args).await {
            Ok(s) => s,
            Err(e) => {
                // Fallback: pure navigation JSON when dispatch fails
                let mut nav = serde_json::json!({
                    "action": ux.action,
                    "path": ux.path,
                });
                if let Some(ref p) = ux.project {
                    nav["project"] = serde_json::json!(p);
                }
                serde_json::json!({
                    "ok": false,
                    "summary": ux.summary,
                    "error": e,
                    "navigation": nav,
                    "project": ux.project,
                })
                .to_string()
            }
        };
        let ok = serde_json::from_str::<serde_json::Value>(&detail)
            .ok()
            .and_then(|v| v.get("ok").and_then(|o| o.as_bool()))
            .unwrap_or(true);
        let summary = serde_json::from_str::<serde_json::Value>(&detail)
            .ok()
            .and_then(|v| {
                v.get("summary")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| ux.summary.clone());
        let path_shown = serde_json::from_str::<serde_json::Value>(&detail)
            .ok()
            .and_then(|v| {
                v.pointer("/navigation/path")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| ux.path.clone());
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: format!(
                "{summary}\n\nTool `{tool}` → `{path}`.",
                tool = ux.tool,
                path = path_shown
            ),
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls: vec![AgentToolCall {
                name: ux.tool,
                detail,
            }],
            source_changed: false,
            ok,
            error: None,
            backend: "host-platform".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    let loaded = provider.as_ref().list_files().await;
    let allowlist = crate::safety::allowlist_from_env(&loaded);

    let source = match provider.as_ref().read_source("").await {
        Ok(s) => s,
        Err(e) => {
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls: vec![],
                source_changed: false,
                ok: false,
                error: Some(e),
                backend: "error".into(),
                plan: None,
                context_truncated: false,
                context_warning: None,
                context_tokens: 0,
                context_budget_tokens: 0,
                context_layers: vec![],
            };
        }
    };
    let registry = provider.as_ref().registry();
    let confirm = std::env::var("VEIL_AGENT_CONFIRM_WRITES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let plan_only = req.plan_only
        || std::env::var("VEIL_AGENT_PLAN_ONLY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    // Tier 0+1 teaching pack for active file layers (deterministic, not vector RAG).
    // Optional MISSION.md inject when project root is known.
    let project_root = provider.as_ref().project_root();
    let preamble_pack = crate::agent_context::assemble_preamble(
        &source,
        &registry,
        project_root.as_deref(),
    );

    let cfg = crate::model::ModelConfig::from_env();

    // Structured IDE commands — run immediately (no LLM wait). Same tools as
    // Rig create_file / list_files / rename / check / outline.
    if is_structured_agent_command(prompt) {
        let mut resp = heuristic_turn(
            provider.as_ref(),
            turn_id,
            prompt,
            source,
            &registry,
            confirm,
            plan_only,
            allowlist,
            loaded,
            messages,
        )
        .await;
        resp = resp.with_context(&preamble_pack);
        // Tag backend so UI shows we used the fast host path, not "offline help"
        if resp.backend == "heuristic" {
            resp.backend = "host".into();
        }
        return resp;
    }

    // ── ACP external agent (Kiro, etc.) ───────────────────────────────────
    if cfg.supports_acp() {
        let active_name = loaded
            .iter()
            .find(|f| f.active)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "active.veil".into());
        let user_prompt = if is_seed_wiki_command(prompt) {
            format!("{}\n\n# Operator request\n{}", SEED_MIND_PALACE_PROMPT, prompt)
        } else {
            prompt.to_string()
        };
        let composed = {
            // Build project file map so the agent knows what's available
            let all_files = provider.list_files().await;
            let file_map: String = all_files
                .iter()
                .map(|f| {
                    let mark = if f.active { " ← active" } else { "" };
                    format!("  - {} ({}){}", f.name, f.kind.as_str(), mark)
                })
                .collect::<Vec<_>>()
                .join("\n");

            let mut c = format!(
                "{}\n\n# Project files\n{}\n\
                 Use `select_file` to switch between them. Frontend UI is typically in a `*_ui.veil` package.\n\n\
                 # User request\n{}\n\n# Active VEIL file: `{active_name}`\n\
                 If the request involves a different file (e.g. UI changes → *_ui.veil), switch to it first.\n",
                preamble_pack.text, file_map, user_prompt
            );
            // Inject full Mind Palace instructions when palace is enabled.
            if crate::mind_palace_tools::enabled() {
                c.push_str(crate::mind_palace_tools::preamble_addon());
            }
            c
        };
        // Set project context for ACP MCP server URL routing.
        let project_name = crate::provider::hub::CURRENT_PROJECT
            .try_with(|n| n.clone())
            .ok();
        crate::acp::set_acp_project(project_name.clone());
        // ACP is sync (stdio) — run on blocking pool so we don't stall the runtime.
        let media = crate::acp::AcpMedia {
            images: req
                .images
                .iter()
                .filter(|i| !i.data_base64.is_empty())
                .map(|i| crate::acp::AcpImagePart {
                    mime_type: i.mime_type.clone(),
                    data_base64: i.data_base64.clone(),
                })
                .collect(),
        };
        let acp_result = tokio::task::spawn_blocking(move || {
            crate::acp::prompt_acp_streaming_media(&composed, &media, |_| {})
        })
        .await
        .map_err(|e| e.to_string());
        match acp_result {
            Ok(Ok(turn)) => {
                // External agent may have written workspace files — reload cache.
                let reloaded = provider.as_ref().reload_from_disk().await.unwrap_or(0);
                let source_changed = reloaded > 0;
                let mut content = turn.text;
                if reloaded > 0 {
                    content.push_str(&format!(
                        "\n\n---\nVEIL reloaded {reloaded} file(s) from disk after ACP turn."
                    ));
                }
                if let Some(ref w) = preamble_pack.warning {
                    content = format!("{w}\n\n{content}");
                }
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content,
                });
                let mut tool_calls: Vec<AgentToolCall> = turn
                    .tool_hints
                    .into_iter()
                    .map(|n| AgentToolCall {
                        name: n,
                        detail: "acp".into(),
                    })
                    .collect();
                if tool_calls.is_empty() {
                    tool_calls.push(AgentToolCall {
                        name: "acp_session".into(),
                        detail: turn.session_id.clone(),
                    });
                }
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed,
                    ok: true,
                    error: None,
                    backend: "acp-kiro".into(),
                    plan: None,
                    context_truncated: preamble_pack.truncated,
                    context_warning: preamble_pack.warning.clone(),
                    context_tokens: preamble_pack.tokens_used,
                    context_budget_tokens: preamble_pack.max_tokens,
                    context_layers: preamble_pack.layers.clone(),
                };
            }
            Ok(Err(e)) | Err(e) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!(
                        "ACP agent error: {e}\n\
                         Falling back to offline heuristic tools.\n\
                         Check: `kiro-cli login`, `VEIL_ACP_COMMAND`, `VEIL_ACP_ARGS`."
                    ),
                });
                // fall through to heuristic
            }
        }
    }

    // Prefer Rig agent loop with tools when provider supports it.
    if cfg.supports_rig_agent() {
        // Truncated curriculum → refuse model turn (unless ALLOW_TRUNCATED).
        if preamble_pack.truncated && crate::agent_context::refuse_on_truncation() {
            let warn = preamble_pack
                .warning
                .clone()
                .unwrap_or_else(|| "Agent context truncated.".into());
            messages.push(AgentMessage {
                role: "assistant".into(),
                content: format!(
                    "{warn}\n\
                     --- \n\
                     Model turn **skipped** (VEIL_AGENT_ALLOW_TRUNCATED not set).\n\
                     Offline tools still available: prompt `check`, `outline`, or `rename A to B`.\n\
                     Or raise budget only if the model context window can hold it:\n\
                       VEIL_AGENT_PREAMBLE_MAX_TOKENS=12000 make serve\n"
                ),
            });
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls: vec![AgentToolCall {
                    name: "context_guard".into(),
                    detail: "truncated — model refused".into(),
                }],
                source_changed: false,
                ok: false,
                error: Some("agent context truncated — switch model/ACP or raise budget".into()),
                backend: format!("rig-{}-refused", cfg.kind_name()),
                plan: None,
                context_truncated: true,
                context_warning: Some(warn),
                context_tokens: preamble_pack.tokens_used,
                context_budget_tokens: preamble_pack.max_tokens,
                context_layers: preamble_pack.layers.clone(),
            };
        }

        let mut preamble = preamble_pack.text.clone();
        if preamble_pack.truncated {
            // ALLOW_TRUNCATED path — still scream in the system prompt
            if let Some(ref w) = preamble_pack.warning {
                preamble = format!(
                    "{w}\n\n# WARNING: continuing with truncated context (VEIL_AGENT_ALLOW_TRUNCATED=1)\n\n{preamble}"
                );
            }
        }

        // Multi-project name (if any) — re-enter task-local when tools run on
        // other tasks (Rig / live writer) that lost CURRENT_PROJECT.
        let project_name = crate::provider::hub::CURRENT_PROJECT
            .try_with(|n| n.clone())
            .ok();

        // Mid-turn live flush: each tool write hits SourceProvider immediately
        // (SSE revision events fire from write_source → IDE badge updates).
        // After write: smoke-test Rust backend (gen + cargo check). On failure,
        // restore previous source and return the compile errors to the agent.
        let writer: Option<crate::rig_tools::LiveWriter> = if plan_only {
            None
        } else {
            let p = provider.clone();
            let proj = project_name.clone();
            Some(std::sync::Arc::new(move |src: String| {
                let p = p.clone();
                let proj = proj.clone();
                Box::pin(async move {
                    let proj_for_smoke = proj.clone();
                    let write = async move {
                        let files = p.list_files().await;
                        let allow = crate::safety::allowlist_from_env(&files);
                        crate::safety::check_write_allowed("", &allow, &files)?;
                        let registry = p.registry();
                        if let Some(block) =
                            crate::agent_context::veil_errors_blocking_write(&src, &registry)
                        {
                            return Err(block);
                        }
                        let prev = p.read_source("").await.ok();
                        let active_name = files
                            .iter()
                            .find(|f| f.active)
                            .map(|f| f.name.clone())
                            .unwrap_or_default();
                        let active_path = files
                            .iter()
                            .find(|f| f.active)
                            .map(|f| f.path.clone())
                            .unwrap_or_default();
                        p.write_source("", &src).await?;

                        // Smoke: gen + cargo check; rollback source on failure.
                        if let Some(root) = p.project_root() {
                            if let Err(smoke_err) = crate::devloop::smoke_agent_write(
                                &root,
                                &active_path,
                                proj_for_smoke.as_deref(),
                            ) {
                                if let Some(prev) = prev {
                                    let _ = p.write_source("", &prev).await;
                                    // Re-gen from restored source so backend stays good.
                                    let _ = crate::devloop::smoke_agent_write(
                                        &root,
                                        &active_path,
                                        proj_for_smoke.as_deref(),
                                    );
                                }
                                return Err(format!(
                                    "WRITE REJECTED — backend smoke test failed (file restored).\n\
                                     Active file: {active_name}\n\n{smoke_err}\n\n\
                                     Next: call dev_logs / smoke_status, fix the VEIL, retry write_source.\
                                     After success: list_routes → dev_restart → http_request."
                                ));
                            }
                        }
                        if let Some(slug) = proj_for_smoke.as_deref() {
                            let _ = crate::review::record_file_edit(
                                slug,
                                if active_path.is_empty() {
                                    &active_name
                                } else {
                                    &active_path
                                },
                                None,
                            );
                        }
                        Ok(())
                    };
                    if let Some(name) = proj {
                        crate::provider::hub::CURRENT_PROJECT
                            .scope(name, write)
                            .await
                    } else {
                        write.await
                    }
                })
            }))
        };

        let mut ws = Workspace::new(source.clone(), registry.clone(), confirm);
        if let Some(w) = writer {
            ws = ws.with_live_writer(w);
        }
        // Host ops so agent tools can match IDE UI (create/list/select files).
        {
            struct ProviderHost<P: crate::provider::SourceProvider> {
                provider: std::sync::Arc<P>,
                /// Multi-project name to re-scope if task-local is missing.
                project: Option<String>,
            }
            impl<P: crate::provider::SourceProvider> ProviderHost<P> {
                async fn with_scope<R, F, Fut>(&self, f: F) -> R
                where
                    F: FnOnce(std::sync::Arc<P>) -> Fut,
                    Fut: std::future::Future<Output = R>,
                {
                    let p = self.provider.clone();
                    if let Some(ref name) = self.project {
                        crate::provider::hub::CURRENT_PROJECT
                            .scope(name.clone(), f(p))
                            .await
                    } else {
                        f(p).await
                    }
                }
            }
            #[async_trait::async_trait]
            impl<P: crate::provider::SourceProvider> crate::rig_tools::AgentHost for ProviderHost<P> {
                async fn list_files(&self) -> Vec<crate::provider::FileInfo> {
                    self.with_scope(|p| async move { p.list_files().await })
                        .await
                }
                async fn create_file(
                    &self,
                    name: &str,
                    kind: Option<&str>,
                    content: Option<String>,
                ) -> Result<crate::file_ops::CreatedFile, String> {
                    let name = name.to_string();
                    let kind = kind.map(|s| s.to_string());
                    self.with_scope(|p| {
                        let name = name.clone();
                        let kind = kind.clone();
                        let content = content.clone();
                        async move {
                            crate::file_ops::create_file_in_project(
                                p.as_ref(),
                                &name,
                                kind.as_deref(),
                                content,
                            )
                            .await
                            .map_err(|e| e.message().to_string())
                        }
                    })
                    .await
                }
                async fn select_file(&self, index: usize) -> Result<(), String> {
                    self.with_scope(|p| async move { p.set_active(index) })
                        .await
                }
                async fn read_active_source(&self) -> Result<String, String> {
                    self.with_scope(|p| async move { p.read_source("").await })
                        .await
                }
                async fn registry(&self) -> veil_ir::LayerRegistry {
                    self.with_scope(|p| async move { p.registry() }).await
                }
                async fn reload_from_disk(&self) -> Result<usize, String> {
                    self.with_scope(|p| async move { p.reload_from_disk().await })
                        .await
                }
                fn project_root(&self) -> Option<std::path::PathBuf> {
                    self.provider.project_root()
                }
                fn project_name(&self) -> Option<String> {
                    self.project.clone()
                }
            }
            ws = ws.with_host(std::sync::Arc::new(ProviderHost {
                provider: provider.clone(),
                project: project_name.clone(),
            }));
        }

        let rig_prompt = if is_seed_wiki_command(prompt) {
            format!("{}\n\n# Operator request\n{}", SEED_MIND_PALACE_PROMPT, prompt)
        } else {
            prompt.to_string()
        };
        match crate::model::prompt_with_tools(&cfg, &preamble, &rig_prompt, ws.clone()).await {
            Ok(content) => {
                let tool_calls = ws.take_log();
                let wants_write = ws.changed();
                if wants_write && plan_only {
                    let plan = Some(format!(
                        "plan_only: would write {} bytes after tools {:?}",
                        ws.source_snapshot().len(),
                        tool_calls.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()
                    ));
                    let mut content = content;
                    if let Some(ref w) = preamble_pack.warning {
                        content = format!("{w}\n\n{content}");
                    }
                    messages.push(AgentMessage {
                        role: "assistant".into(),
                        content: format!(
                            "{content}\n\n[plan_only] No write applied. Re-run without VEIL_AGENT_PLAN_ONLY / plan_only to apply."
                        ),
                    });
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: true,
                        error: None,
                        backend: format!("rig-{}", cfg.kind_name()),
                        plan,
                        context_truncated: preamble_pack.truncated,
                        context_warning: preamble_pack.warning.clone(),
                        context_tokens: preamble_pack.tokens_used,
                        context_budget_tokens: preamble_pack.max_tokens,
                        context_layers: preamble_pack.layers.clone(),
                    };
                }
                // Ensure final snapshot is on disk (covers tools that skipped live write)
                if wants_write && !plan_only {
                    let loaded_now = provider.as_ref().list_files().await;
                    let allow_now = crate::safety::allowlist_from_env(&loaded_now);
                    if let Err(e) = crate::safety::check_write_allowed("", &allow_now, &loaded_now) {
                        return AgentTurnResponse {
                            turn_id,
                            messages,
                            tool_calls,
                            source_changed: false,
                            ok: false,
                            error: Some(e),
                            backend: format!("rig-{}", cfg.kind_name()),
                            plan: None,
                            context_truncated: preamble_pack.truncated,
                            context_warning: preamble_pack.warning.clone(),
                            context_tokens: preamble_pack.tokens_used,
                            context_budget_tokens: preamble_pack.max_tokens,
                            context_layers: preamble_pack.layers.clone(),
                        };
                    }
                    let new_src = ws.source_snapshot();
                    // Final flush (if tools skipped live write). Smoke-gate like live writer.
                    let prev = provider.read_source("").await.ok();
                    let files = provider.list_files().await;
                    let active_path = files
                        .iter()
                        .find(|f| f.active)
                        .map(|f| f.path.clone())
                        .unwrap_or_default();
                    if let Err(e) = provider.write_source("", &new_src).await {
                        return AgentTurnResponse {
                            turn_id,
                            messages,
                            tool_calls,
                            source_changed: false,
                            ok: false,
                            error: Some(e),
                            backend: format!("rig-{}", cfg.kind_name()),
                            plan: None,
                            context_truncated: preamble_pack.truncated,
                            context_warning: preamble_pack.warning.clone(),
                            context_tokens: preamble_pack.tokens_used,
                            context_budget_tokens: preamble_pack.max_tokens,
                            context_layers: preamble_pack.layers.clone(),
                        };
                    }
                    if let Some(root) = provider.project_root() {
                        if let Err(smoke_err) = crate::devloop::smoke_agent_write(
                            &root,
                            &active_path,
                            project_name.as_deref(),
                        ) {
                            if let Some(prev) = prev {
                                let _ = provider.write_source("", &prev).await;
                                let _ = crate::devloop::smoke_agent_write(
                                    &root,
                                    &active_path,
                                    project_name.as_deref(),
                                );
                            }
                            return AgentTurnResponse {
                                turn_id,
                                messages,
                                tool_calls,
                                source_changed: false,
                                ok: false,
                                error: Some(format!(
                                    "WRITE REJECTED — backend smoke test failed (restored):\n{smoke_err}"
                                )),
                                backend: format!("rig-{}", cfg.kind_name()),
                                plan: None,
                                context_truncated: preamble_pack.truncated,
                                context_warning: preamble_pack.warning.clone(),
                                context_tokens: preamble_pack.tokens_used,
                                context_budget_tokens: preamble_pack.max_tokens,
                                context_layers: preamble_pack.layers.clone(),
                            };
                        }
                    }
                }
                let content = if let Some(ref w) = preamble_pack.warning {
                    format!("{w}\n\n{content}")
                } else {
                    content
                };
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content,
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: wants_write && !plan_only,
                    ok: !preamble_pack.truncated,
                    error: if preamble_pack.truncated {
                        Some("agent context was truncated (ran with VEIL_AGENT_ALLOW_TRUNCATED=1)".into())
                    } else {
                        None
                    },
                    backend: format!("rig-{}", cfg.kind_name()),
                    plan: None,
                    context_truncated: preamble_pack.truncated,
                    context_warning: preamble_pack.warning.clone(),
                    context_tokens: preamble_pack.tokens_used,
                    context_budget_tokens: preamble_pack.max_tokens,
                    context_layers: preamble_pack.layers.clone(),
                };
            }
            Err(e) => {
                // Fall through to heuristic with error note
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!(
                        "Rig agent error ({provider}): {e}\nFalling back to heuristic tools.",
                        provider = cfg.kind_name()
                    ),
                });
            }
        }
    }

    // Heuristic offline path (no Rig model) — same tools, host-dispatched.
    let mut resp = heuristic_turn(
        provider.as_ref(),
        turn_id,
        prompt,
        source,
        &registry,
        confirm,
        plan_only,
        allowlist,
        loaded,
        messages,
    )
    .await;
    resp = resp.with_context(&preamble_pack);
    resp
}

async fn heuristic_turn<P: SourceProvider>(
    provider: &P,
    turn_id: String,
    prompt: &str,
    source: String,
    registry: &LayerRegistry,
    confirm: bool,
    plan_only: bool,
    allowlist: Vec<String>,
    loaded: Vec<crate::provider::FileInfo>,
    mut messages: Vec<AgentMessage>,
) -> AgentTurnResponse {
    let mut tool_calls = Vec::new();
    let lower = prompt.to_lowercase();

    if lower == "check" || lower.starts_with("check ") || lower.contains("run check") {
        tool_calls.push(AgentToolCall {
            name: "veil_check".into(),
            detail: "target=rust".into(),
        });
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: rig_tools::run_check(&source, registry),
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
            backend: "heuristic".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    if lower == "outline" || lower.starts_with("outline") || lower.contains("show structure") {
        tool_calls.push(AgentToolCall {
            name: "veil_outline".into(),
            detail: "outline".into(),
        });
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: rig_tools::run_outline(&source, registry),
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
            backend: "heuristic".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    if let Some((from, to)) = parse_rename(prompt) {
        if confirm && !lower.contains("confirm") {
            messages.push(AgentMessage {
                role: "assistant".into(),
                content: format!(
                    "Permission: write would rename '{from}' → '{to}'. Re-send as `confirm rename {from} to {to}` (VEIL_AGENT_CONFIRM_WRITES)."
                ),
            });
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls: vec![AgentToolCall {
                    name: "permission_check".into(),
                    detail: "confirm required".into(),
                }],
                source_changed: false,
                ok: true,
                error: None,
                backend: "heuristic".into(),
                plan: None,
                context_truncated: false,
                context_warning: None,
                context_tokens: 0,
                context_budget_tokens: 0,
                context_layers: vec![],
            };
        }
        tool_calls.push(AgentToolCall {
            name: "rename_construct".into(),
            detail: format!("{from} → {to}"),
        });
        match rig_tools::apply_rename(&source, registry, &from, &to) {
            Ok((new_src, summary)) => {
                if plan_only {
                    let plan = format!("RenameConstruct {from} → {to}");
                    messages.push(AgentMessage {
                        role: "assistant".into(),
                        content: format!(
                            "[plan_only] Would apply: {plan}\n{summary}\n\nRe-run without plan_only / VEIL_AGENT_PLAN_ONLY to apply."
                        ),
                    });
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: true,
                        error: None,
                        backend: "heuristic".into(),
                        plan: Some(plan),
                        context_truncated: false,
                        context_warning: None,
                        context_tokens: 0,
                        context_budget_tokens: 0,
                        context_layers: vec![],
                    };
                }
                if let Err(e) = crate::safety::check_write_allowed("", &allowlist, &loaded) {
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: false,
                        error: Some(e),
                        backend: "heuristic".into(),
                        plan: None,
                        context_truncated: false,
                        context_warning: None,
                        context_tokens: 0,
                        context_budget_tokens: 0,
                        context_layers: vec![],
                    };
                }
                if let Err(e) = provider.write_source("", &new_src).await {
                    return AgentTurnResponse {
                        turn_id,
                        messages,
                        tool_calls,
                        source_changed: false,
                        ok: false,
                        error: Some(e),
                        backend: "heuristic".into(),
                        plan: None,
                        context_truncated: false,
                        context_warning: None,
                        context_tokens: 0,
                        context_budget_tokens: 0,
                        context_layers: vec![],
                    };
                }
                tool_calls.push(AgentToolCall {
                    name: "veil_check".into(),
                    detail: "post-edit".into(),
                });
                let check = rig_tools::run_check(&new_src, registry);
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("{summary}\n\n{check}"),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: true,
                    ok: true,
                    error: None,
                    backend: "heuristic".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
            }
            Err(e) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("Could not rename: {e}"),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: false,
                    ok: false,
                    error: Some(e),
                    backend: "heuristic".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
            }
        }
    }

    // list files
    if lower == "list files"
        || lower == "files"
        || lower.starts_with("list file")
        || lower.contains("what files")
        || lower.contains("show files")
    {
        tool_calls.push(AgentToolCall {
            name: "list_files".into(),
            detail: "list".into(),
        });
        let files = provider.list_files().await;
        let mut lines = vec!["files:".to_string()];
        for f in &files {
            let mark = if f.active { " ●" } else { "" };
            lines.push(format!(
                "  [{idx}] {name} ({kind}){mark}",
                idx = f.index,
                name = f.name,
                kind = f.kind.as_str(),
            ));
        }
        if files.is_empty() {
            lines.push("  (none)".into());
        }
        messages.push(AgentMessage {
            role: "assistant".into(),
            content: lines.join("\n"),
        });
        return AgentTurnResponse {
            turn_id,
            messages,
            tool_calls,
            source_changed: false,
            ok: true,
            error: None,
            backend: "heuristic".into(),
            plan: None,
            context_truncated: false,
            context_warning: None,
            context_tokens: 0,
            context_budget_tokens: 0,
            context_layers: vec![],
        };
    }

    // create package / layer file (same path as IDE + and Rig create_file tool)
    if let Some((name, kind)) = parse_create_file(prompt) {
        tool_calls.push(AgentToolCall {
            name: "create_file".into(),
            detail: format!("{name} kind={kind}"),
        });
        if plan_only {
            messages.push(AgentMessage {
                role: "assistant".into(),
                content: format!(
                    "[plan_only] Would create {kind} file '{name}'. Re-run without plan_only to apply."
                ),
            });
            return AgentTurnResponse {
                turn_id,
                messages,
                tool_calls,
                source_changed: false,
                ok: true,
                error: None,
                backend: "heuristic".into(),
                plan: Some(format!("create_file {name} ({kind})")),
                context_truncated: false,
                context_warning: None,
                context_tokens: 0,
                context_budget_tokens: 0,
                context_layers: vec![],
            };
        }
        match crate::file_ops::create_file_in_project(provider, &name, Some(&kind), None).await {
            Ok(created) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!(
                        "Created {} ({}) at {} — now active.\n\
                         Use the file picker or say `list files` to confirm.",
                        created.name,
                        created.kind.as_str(),
                        created.path
                    ),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: true,
                    ok: true,
                    error: None,
                    backend: "heuristic".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
            }
            Err(e) => {
                messages.push(AgentMessage {
                    role: "assistant".into(),
                    content: format!("Could not create file: {}", e.message()),
                });
                return AgentTurnResponse {
                    turn_id,
                    messages,
                    tool_calls,
                    source_changed: false,
                    ok: false,
                    error: Some(e.message().to_string()),
                    backend: "heuristic".into(),
                    plan: None,
                    context_truncated: false,
                    context_warning: None,
                    context_tokens: 0,
                    context_budget_tokens: 0,
                    context_layers: vec![],
                };
            }
        }
    }

    // Default help
    let outline = rig_tools::run_outline(&source, registry);
    let check = rig_tools::run_check(&source, registry);
    messages.push(AgentMessage {
        role: "assistant".into(),
        content: format!(
            "Offline heuristic agent (set VEIL_MODEL_PROVIDER=openai|ollama for Rig tools).\n\
             Commands: `check` · `outline` · `list files` · `create package Name` · \
             `create layer Name` · `rename Old to New`\n\
             Safety: VEIL_AGENT_ALLOWLIST · VEIL_AGENT_PLAN_ONLY · VEIL_AGENT_CONFIRM_WRITES\n\n\
             Context:\n{outline}\n\n{check}"
        ),
    });
    AgentTurnResponse {
        turn_id,
        messages,
        tool_calls: vec![
            AgentToolCall {
                name: "veil_outline".into(),
                detail: "context".into(),
            },
            AgentToolCall {
                name: "veil_check".into(),
                detail: "context".into(),
            },
        ],
        source_changed: false,
        ok: true,
        error: None,
        backend: "heuristic".into(),
        plan: None,
        context_truncated: false,
        context_warning: None,
        context_tokens: 0,
        context_budget_tokens: 0,
        context_layers: vec![],
    }
}

fn chrono_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn parse_rename(prompt: &str) -> Option<(String, String)> {
    let p = prompt.trim();
    let lower = p.to_lowercase();
    // Product rename is rename_project — never steal as construct rename.
    if looks_like_rename_project_prompt(&lower) {
        return None;
    }
    let rest = if let Some(r) = lower.strip_prefix("confirm rename ") {
        // keep original casing from prompt after "confirm rename "
        &p[prompt.len() - r.len()..]
    } else if lower.starts_with("rename ") {
        &p["rename ".len()..]
    } else {
        return None;
    };
    let rest = rest.trim();
    if let Some((a, b)) = rest.split_once(" to ") {
        let from = a.trim().to_string();
        let to = b.trim().to_string();
        if !from.is_empty() && !to.is_empty() {
            return Some((from, to));
        }
    }
    if let Some((a, b)) = rest.split_once(" -> ") {
        let from = a.trim().to_string();
        let to = b.trim().to_string();
        if !from.is_empty() && !to.is_empty() {
            return Some((from, to));
        }
    }
    None
}

/// Parse natural create-file prompts → (name, kind) where kind is package|layer.
///
/// Examples:
/// - `create package AcmeWear`
/// - `create layer engagement`
/// - `create file Foo.veil`
/// - `new package Bar`
/// - `make a new file called Widget`
/// Host-resolved platform UX navigation (runtime shell agent).
#[derive(Debug, Clone)]
pub struct PlatformUxIntent {
    pub tool: String,
    pub path: String,
    pub summary: String,
    /// SPA action: `goto` | `open-ide` | `switch-project`
    pub action: String,
    /// Project slug when action is open-ide / open_project
    pub project: Option<String>,
    /// Arguments for [`crate::platform_tools::dispatch`] (e.g. create_project name).
    pub args: Option<serde_json::Value>,
}

/// Map dashboard / chip prompts → platform UX tool + SPA path.
///
/// Prefer explicit tool names (`list_prs`, `navigate_to`, …) and common
/// natural phrases used by AgentDock chips.
///
/// When aether_chat folds a system prompt under `# User request`, only the
/// user section is considered so tool docs in the system preamble do not
/// false-trigger navigation.
pub fn parse_platform_ux_intent(prompt: &str) -> Option<PlatformUxIntent> {
    let user_part = prompt
        .rsplit_once("# User request\n")
        .map(|(_, u)| u)
        .unwrap_or(prompt);
    let lower = user_part.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }

    // create_project — real product create (not wiki-only).
    // Only short-circuit *simple* create prompts; multi-step ("create X and design…")
    // stays with the LLM/MCP tool loop so create_project can run mid-turn.
    let looks_like_create_project = looks_like_create_project_prompt(&lower);
    let looks_like_rename_project = looks_like_rename_project_prompt(&lower);
    let multi_step = lower.contains(" and ")
        || lower.contains(" then ")
        || lower.contains(" with ")
        || lower.contains(" that ")
        || lower.len() > 120;
    if looks_like_rename_project && !multi_step {
        if let Some(intent) = rename_project_ux_intent(user_part, prompt) {
            return Some(intent);
        }
    }
    if looks_like_create_project && !multi_step {
        let name = extract_create_project_name(user_part).or_else(|| {
            // "create_project foo" / tool-style
            extract_project_slug_from_prompt(&lower).filter(|s| {
                !matches!(
                    s.as_str(),
                    "create"
                        | "project"
                        | "projects"
                        | "new"
                        | "a"
                        | "the"
                        | "named"
                        | "called"
                        | "repo"
                        | "scaffold"
                        | "make"
                )
            })
        });
        if let Some(name) = name {
            return Some(PlatformUxIntent {
                tool: "create_project".into(),
                path: format!("/projects/{name}"),
                summary: format!("Creating project `{name}`"),
                action: "goto".into(),
                project: Some(name.clone()),
                args: Some(serde_json::json!({ "name": name, "open": true })),
            });
        }
        // No name yet — open create form so the user can finish in UI
        return Some(PlatformUxIntent {
            tool: "navigate_to".into(),
            path: "/projects/new".into(),
            summary: "Open create project form (no name given — pass name: create project <slug>)"
                .into(),
            action: "goto".into(),
            project: None,
            args: Some(serde_json::json!({ "path": "/projects/new" })),
        });
    }

    // Explicit tool / phrase → path.
    // IMPORTANT: only short-circuit when the *user intent* is navigation/UX.
    // Multi-line Fix-button / coding prompts often *mention* tool names
    // (`create_pr`, `session_commit`, …) as SOP — those must reach ACP/LLM,
    // not open an empty form and stop (regression: Diagnostics "Fix" → create_pr).
    if is_primary_platform_nav_prompt(&lower) {
        let pairs: &[(&[&str], &str, &str, &str)] = &[
            (
                &[
                    "list_prs",
                    "open_prs",
                    "open changes",
                    "show me open change",
                    "show open change",
                    "pull requests",
                    "navigate to /pulls",
                    "go to /pulls",
                    "go to changes",
                ],
                "list_prs",
                "/pulls",
                "Opening pull requests",
            ),
            (
                &[
                    "create_pr",
                    "open_create_pr",
                    "create pull request",
                    "new pull request",
                    "navigate to /pulls/new",
                ],
                "create_pr",
                "/pulls/new",
                "Opening create pull request",
            ),
            (
                &[
                    "list_projects",
                    "open_projects",
                    "open projects",
                    "show projects",
                    "navigate to /projects",
                    "go to projects",
                ],
                "list_projects",
                "/projects",
                "Opening projects",
            ),
            (
                &[
                    "open_deploy",
                    "open deploy",
                    "deploy to staging",
                    "go to deploy",
                    "navigate to /deploy",
                ],
                "open_deploy",
                "/deploy",
                "Opening deploy",
            ),
            (
                &[
                    "open_registry",
                    "open registry",
                    "go to registry",
                    "navigate to /registry",
                ],
                "open_registry",
                "/registry",
                "Opening registry",
            ),
            (
                &[
                    "open_dashboard",
                    "open dashboard",
                    "go to dashboard",
                    "navigate to /dashboard",
                ],
                "open_dashboard",
                "/dashboard",
                "Opening dashboard",
            ),
            (
                &["open_config", "open config", "go to config", "navigate to /config"],
                "open_config",
                "/config",
                "Opening config",
            ),
        ];
        for (needles, tool, path, summary) in pairs {
            if needles.iter().any(|n| lower.contains(n)) {
                return Some(PlatformUxIntent {
                    tool: (*tool).into(),
                    path: (*path).into(),
                    summary: (*summary).into(),
                    action: "goto".into(),
                    project: None,
                    args: None,
                });
            }
        }
    }
    // open_ide / open_project with a named project: "open the relay project".
    // Long task text that merely *mentions* those tools is not a nav command
    // (substring match used to hijack whole ACP turns).
    let nav_only = lower.len() < 200 && lower.matches('\n').count() < 2;
    if nav_only
        && (lower.contains("open_ide")
            || (lower.contains("open the ") && lower.contains(" project"))
            || lower.contains("open project")
            || lower.contains("open_project")
            || (lower.contains(" in the ide") && lower.contains("open")))
    {
        // Extract a simple project slug token after "project" or "the … project"
        let project = extract_project_slug_from_prompt(&lower).unwrap_or_default();
        if !project.is_empty() {
            let is_ide = lower.contains("open_ide")
                || lower.contains(" in the ide")
                || lower.contains(" in ide");
            let tool = if is_ide { "open_ide" } else { "open_project" };
            // IDE → shell embed route (runtime keeps AgentDock); project page for open_project.
            let path = if is_ide {
                format!("/projects/{project}/ide")
            } else {
                format!("/projects/{project}")
            };
            let action = if is_ide {
                "open-ide".to_string()
            } else {
                "goto".to_string()
            };
            return Some(PlatformUxIntent {
                tool: tool.into(),
                path,
                summary: if is_ide {
                    format!("Opening {project} in the IDE")
                } else {
                    format!("Opening project {project}")
                },
                action,
                project: Some(project.clone()),
                args: Some(serde_json::json!({ "project": project })),
            });
        }
        return Some(PlatformUxIntent {
            tool: "list_projects".into(),
            path: "/projects".into(),
            summary: "Opening projects (no project specified)".into(),
            action: "goto".into(),
            project: None,
            args: None,
        });
    }
    None
}

/// Pull project name from "create project foo", "create a project named bar", etc.
fn extract_create_project_name(prompt: &str) -> Option<String> {
    let s = prompt.trim();
    // named/called X — support quoted multi-word ("Agent Registry" → agent-registry)
    for marker in [" named ", " called ", " name "] {
        if let Some(idx) = s.to_lowercase().find(marker) {
            let rest = s[idx + marker.len()..].trim();
            if let Some(name) = extract_project_name_phrase(rest) {
                return Some(name);
            }
        }
    }
    // "create project <slug>" / "create a project <slug>" / "new project <slug>"
    let lower = s.to_lowercase();
    for prefix in [
        "create_project ",
        "create_repo ",
        "create a new project ",
        "create a project ",
        "create project ",
        "new project ",
        "scaffold a project ",
        "scaffold project ",
        "make a project ",
        "make a new project ",
    ] {
        if let Some((before, after)) = lower.split_once(prefix) {
            // Prefer mid-sentence matches; map offset back to original string.
            let off = before.len() + prefix.len();
            let rest_orig = s.get(off..).unwrap_or(after);
            if let Some(name) = extract_project_name_phrase(rest_orig) {
                if !matches!(
                    name.as_str(),
                    "named" | "called" | "please" | "for" | "with" | "and" | "the"
                ) {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Parse a project name phrase after "called"/"named" or "create project".
///
/// Returns the **display name** (e.g. `"Agent Registry"`), not a forced kebab
/// slug. `POST /api/repos` derives `slug = name.lower().replace(' ', '-')`.
fn extract_project_name_phrase(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    // Quoted multi-word: "Agent Registry" / 'Agent Registry'
    let bytes = rest.as_bytes();
    let open = bytes[0] as char;
    if matches!(open, '"' | '\'' | '“' | '”' | '‘' | '’') {
        let close = match open {
            '“' => '”',
            '‘' => '’',
            c => c,
        };
        if let Some(end) = rest[1..].find(close) {
            let inner = rest[1..1 + end].trim();
            if inner.chars().filter(|c| c.is_ascii_alphanumeric()).count() >= 2 {
                return Some(inner.to_string());
            }
        }
    }
    // Unquoted: take words until stop-word / sentence boundary.
    // Preserve original casing so "Agent Registry" stays a display name.
    let stop = [
        "and", "then", "with", "that", "which", "for", "please", "it", "this",
        "the", "a", "an", "to", "in", "on", "from", "into", "of", "will",
        "should", "must", "can", "need", "needs", "contains", "bring",
        "called", "named",
    ];
    let mut parts: Vec<String> = Vec::new();
    for w in rest.split_whitespace() {
        let ends_sentence = w.ends_with('.') || w.ends_with(',') || w.ends_with(';') || w.ends_with('!');
        let cleaned = w
            .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .to_string();
        if cleaned.is_empty() {
            break;
        }
        let lower = cleaned.to_lowercase();
        if stop.contains(&lower.as_str()) {
            break;
        }
        parts.push(cleaned);
        if ends_sentence {
            break;
        }
        // First token already looks like a finished slug → stop (agent-registry)
        if parts.len() == 1
            && (parts[0].contains('-')
                || parts[0].contains('_')
                || parts[0]
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'))
        {
            break;
        }
        // Cap multi-word title names
        if parts.len() >= 4 {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    // Multi-word Title Case → display name with spaces; single token as-is
    let name = if parts.len() > 1 {
        parts.join(" ")
    } else {
        parts.into_iter().next().unwrap_or_default()
    };
    if name.chars().filter(|c| c.is_ascii_alphanumeric()).count() >= 2 {
        Some(name)
    } else {
        None
    }
}

/// URL/path slug from a display name (`Agent Registry` → `agent-registry`).
pub fn slugify_project_name(raw: &str) -> String {
    raw.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

fn extract_project_slug_from_prompt(lower: &str) -> Option<String> {
    // "open the relay project" / "open relay in the ide"
    for part in lower.split_whitespace() {
        let t = part.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
        if t.is_empty()
            || matches!(
                t,
                "open"
                    | "the"
                    | "a"
                    | "an"
                    | "project"
                    | "projects"
                    | "in"
                    | "ide"
                    | "use"
                    | "open_ide"
                    | "open_project"
                    | "or"
                    | "and"
                    | "to"
            )
        {
            continue;
        }
        // Prefer tokens that look like project slugs (relay, flow, …)
        if t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && t.len() >= 2 {
            return Some(t.to_string());
        }
    }
    None
}

/// Host-executed platform step before ACP (so the SPA navigates visibly).
#[derive(Debug, Clone)]
pub struct HostPlatformStep {
    pub tool: String,
    pub args: serde_json::Value,
    pub summary: String,
}

/// Product ops the **host** must run via `platform_tools` (with SPA `navigation`)
/// even when the rest of the turn goes to ACP.
///
/// Without this, multi-step prompts ("create project X and build domain…") skip
/// host short-circuit and ACP often curls `/api/repos` — no tool UI, no UX.
pub fn host_platform_prefix_steps(prompt: &str) -> Vec<HostPlatformStep> {
    let user_part = prompt
        .rsplit_once("# User request\n")
        .map(|(_, u)| u)
        .unwrap_or(prompt);
    let lower = user_part.trim().to_lowercase();
    if lower.is_empty() {
        return Vec::new();
    }

    let mut steps = Vec::new();

    let looks_like_create = looks_like_create_project_prompt(&lower);
    let looks_like_rename = looks_like_rename_project_prompt(&lower);

    if looks_like_rename {
        if let Some(name) = extract_rename_project_name(user_part) {
            let project = extract_rename_project_target(user_part)
                .or_else(|| bound_project_from_prompt(prompt));
            let mut args = serde_json::json!({ "name": name });
            if let Some(ref p) = project {
                args["project"] = serde_json::json!(p);
            }
            steps.push(HostPlatformStep {
                tool: "rename_project".into(),
                args,
                summary: format!("rename_project → `{name}` (host — PATCH /api/repos)"),
            });
        }
    }

    if looks_like_create {
        if let Some(name) = extract_create_project_name(user_part)
            .or_else(|| extract_bootstrap_project_name(user_part))
        {
            // 1) Show projects list so the operator sees the dashboard move
            steps.push(HostPlatformStep {
                tool: "navigate_to".into(),
                args: serde_json::json!({ "path": "/projects" }),
                summary: "Navigate to projects (visible UX)".into(),
            });
            // 2) Real create via platform tool (DDB+S3) — not curl, not disk.
            // Browser watching → via=ux (click the form). Headless → via=server.
            let via = if crate::focus::client_present() {
                "ux"
            } else {
                "server"
            };
            steps.push(HostPlatformStep {
                tool: "create_project".into(),
                args: serde_json::json!({
                    "name": name,
                    "open": true,
                    "open_ide": true,
                    "via": via,
                }),
                summary: format!("create_project `{name}` (host — via={via})"),
            });
        }
    }

    steps
}

/// "bootstrap the Agent Registry project" / "create the agent-registry project"
fn extract_bootstrap_project_name(prompt: &str) -> Option<String> {
    let lower = prompt.to_lowercase();
    // "… the X Y project" or "… project X"
    if let Some(idx) = lower.find(" project") {
        // tokens before " project" that look like a name
        let before = prompt[..idx.min(prompt.len())].trim_end();
        let words: Vec<&str> = before.split_whitespace().collect();
        // take last 1–3 content words after stop-words
        let mut name_parts: Vec<String> = Vec::new();
        for w in words.iter().rev() {
            let t = w
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                .to_lowercase();
            if t.is_empty()
                || matches!(
                    t.as_str(),
                    "the"
                        | "a"
                        | "an"
                        | "create"
                        | "new"
                        | "bootstrap"
                        | "scaffold"
                        | "make"
                        | "build"
                        | "open"
                        | "for"
                        | "and"
                        | "with"
                        | "full"
                        | "named"
                        | "called"
                        | "go"
                        | "ahead"
                )
            {
                if !name_parts.is_empty() {
                    break;
                }
                continue;
            }
            name_parts.push(t);
            if name_parts.len() >= 3 {
                break;
            }
        }
        name_parts.reverse();
        if !name_parts.is_empty() {
            return Some(name_parts.join("-"));
        }
    }
    extract_create_project_name(prompt)
}

/// True when the prompt is a host-side command (no LLM/ACP required).
pub fn is_structured_agent_command(prompt: &str) -> bool {
    let lower = prompt.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    // Seed wiki is NOT structured — it needs the LLM + wiki_* tools.
    if is_seed_wiki_command(prompt) {
        return false;
    }
    // Multi-step create-project must go through host_platform_prefix_steps +
    // ACP/LLM (not the offline create_file heuristic).
    if looks_like_create_project_prompt(&lower) && parse_platform_ux_intent(prompt).is_none() {
        return false;
    }
    // Platform navigation must short-circuit before ACP so SPA tools fire.
    if parse_platform_ux_intent(prompt).is_some() {
        return true;
    }
    if lower == "check"
        || lower.starts_with("check ")
        || lower.contains("run check")
        || lower == "outline"
        || lower.starts_with("outline")
        || lower.contains("show structure")
        || lower == "list files"
        || lower == "files"
        || lower.starts_with("list file")
        || lower.contains("what files")
        || lower.contains("show files")
    {
        return true;
    }
    if parse_rename(prompt).is_some() {
        return true;
    }
    if parse_create_file(prompt).is_some() {
        return true;
    }
    false
}

/// Operator asked to seed Mind Palace with VEIL platform knowledge.
pub fn is_seed_wiki_command(prompt: &str) -> bool {
    let lower = prompt.trim().to_lowercase();
    lower == "seed wiki"
        || lower == "seed mind palace"
        || lower == "seed palace"
        || lower.starts_with("seed wiki ")
        || lower.starts_with("seed mind palace")
        || lower.contains("seed the wiki")
        || lower.contains("seed mind palace")
        || lower.contains("populate mind palace")
        || lower.contains("populate the wiki")
}

/// Expanded task for agents: synthesize VEIL docs into Mind Palace pages via wiki_*.
const SEED_MIND_PALACE_PROMPT: &str = r#"# Task: Seed Mind Palace with VEIL platform knowledge

You have MCP tools wiki_search, wiki_create, wiki_update, wiki_read, wiki_list, wiki_traverse.
Mind Palace is empty or sparse. Your job is to CREATE (or UPDATE if present) durable wiki pages
that future agents will search before writing VEIL.

## Procedure
1. wiki_list (no filter) and wiki_search for each topic below — skip create if a good page exists; update instead.
2. wiki_create each missing page with page_type Index or Concept, clear summary, 2–4 sections.
3. Link related pages via the `links` field (array of slugs).
4. Do NOT invent false APIs. Prefer patterns from your Tier 0/1 teaching context and this brief.

## Pages to ensure (slug → focus)

### ACS-009 durable contracts (MUST seed first — short, bullets + example)

Use body from repo fixtures when available: `fixtures/palace_contracts/<slug>.md`.

1. **veil-contract-bang-opt-res** (Concept)
   - Decl `name!` = fallible; call `find!` → Opt<T> (portable bang); force with require / .unwrap() when need T
   - Link: docs/BANG_CONTRACT.md

1b. **veil-contract-git-shaped-sessions** (Concept) + **veil-agent-git-shaped-coding** (Sop)
   - Tools: session_status → create_branch → veil_check → one class → write → check (fix new diags same turn) → session_commit → create_pr + submit_pr
   - Agent decides branch/commit; human merges after PR review. NEVER auto-merge. Autosave ≠ commit; change list ≠ error count
   - Fixture: fixtures/palace_contracts/veil-contract-git-shaped-sessions.md

2. **veil-contract-dual-loop-smoke** (Concept)
   - write → smoke → list_routes → restart → http_request; on reject: dev_logs first

3. **veil-contract-multi-package** (Concept)
   - [dev].packages + gen-harness; multi-package ≠ multi-project hub
   - fixtures/multi_harness/

4. **veil-contract-stubs** (Concept)
   - .stub + harness_field + @field/@env; engine does not hardcode SDKs

5. **veil-contract-routes** (Concept)
   - endpoint authoritative; list_routes; no API @route on svc/handler; name-derived = compat=auto only

### Platform overview (also ensure)

6. **veil-language-overview** (Index)
   - pkg/ctx/port/impl/svc, layers (`use ddd`), dual-loop idea
   - What belongs in domain vs infrastructure

7. **veil-stubs-and-sdks** (Concept) — may merge with veil-contract-stubs if one page is enough
   - cargo_deps, types_module, root_types, harness_field
   - Fluent builders; PascalCase enum variants

8. **veil-bus-vs-rest** (Concept)
   - Bus = backend only; frontends use HTTPS REST/WebSockets
   - Local harness `/api/...` for dual-loop / Vite proxy

9. **veil-dual-loop** (Concept) — may link veil-contract-dual-loop-smoke
   - gen → cargo/npm; veil.toml targets; relative `/api` + proxy

10. **veil-ui-sveltekit5** (Concept)
   - Only template/style raw; logic is VEIL fn/effect/state → TS
   - ApiClient.fetch/mutate; layer = tech commitment

11. **sop-seed-and-extend-wiki** (Sop)
   - Prerequisites: MIND_PALACE=1, AWS profile, wiki tools
   - Steps: search → read → create/update → link → list
   - MUST progressive disclosure; SHOULD update before create

12. **sop-add-cloud-adapter** (Sop)
   - Port + @dep @field @env + stub; gen + env smoke

After each create/update, note lint_issues. Finish with wiki_list summarizing what exists.
Link all five **veil-contract-*** pages to each other and to veil-language-overview.

If wiki_* tools return "disabled" or init errors, report the env fix from docs/MIND_PALACE.md and stop.
"#;

fn parse_create_file(prompt: &str) -> Option<(String, String)> {
    let p = prompt.trim();
    let lower = p.to_lowercase();

    // Never treat product create as create_file. Multi-step prompts like
    // "create a new project called Agent Registry and …" used to match the
    // loose "called X" branch, skip host_platform_prefix_steps, and fail with
    // "project scope missing" on MultiProjectProvider.
    if looks_like_create_project_prompt(&lower) {
        return None;
    }

    // Explicit: create/new [package|layer|file] <name>
    for prefix in [
        "create package ",
        "create layer ",
        "create file ",
        "new package ",
        "new layer ",
        "new file ",
        "add package ",
        "add layer ",
        "add file ",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let name = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.')
                .to_string();
            if name.is_empty() {
                return None;
            }
            // Preserve casing from original prompt roughly: find token in original
            let name = extract_name_token(p, &name).unwrap_or(name);
            let kind = if prefix.contains("layer") {
                "layer".into()
            } else if name.ends_with(".layer") {
                "layer".into()
            } else {
                "package".into()
            };
            return Some((name, kind));
        }
    }

    // Looser: "make a new file called X" / "create a package named X"
    // Require package/layer/file cue so "create … called …" alone is not enough.
    let called_markers = [" called ", " named ", " name "];
    let file_cue = lower.contains("package")
        || lower.contains("layer")
        || lower.contains("file")
        || lower.contains(".veil")
        || lower.contains(".layer");
    if file_cue && (lower.contains("create") || lower.contains("new ") || lower.contains("add ")) {
        for m in called_markers {
            if let Some(idx) = lower.find(m) {
                let after = &p[idx + m.len()..];
                let name = after
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.')
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let kind = if lower.contains("layer") || name.ends_with(".layer") {
                    "layer".into()
                } else {
                    "package".into()
                };
                return Some((name, kind));
            }
        }
    }
    None
}

/// Product-level rename (repo display name / slug), not construct rename.
fn looks_like_rename_project_prompt(lower: &str) -> bool {
    if lower.contains("rename_construct") || lower.contains("confirm rename ") {
        return false;
    }
    if lower.contains("rename_project") || lower.contains("update_project") {
        return true;
    }
    if (lower.contains("rename") || lower.contains("retitle"))
        && lower.contains("project")
        && (lower.contains(" to ") || lower.contains(" name"))
    {
        return true;
    }
    if lower.contains("change the project name")
        || lower.contains("update the project name")
        || lower.contains("change this project's name")
        || lower.contains("change this project name")
    {
        return true;
    }
    // "rename agent-registry to Agent Core" — display name (space/quotes), not a construct.
    if lower.contains("rename") {
        if let Some(idx) = lower.rfind(" to ") {
            let dest = lower[idx + 4..].trim();
            if dest.starts_with('"') || dest.starts_with('\'') || dest.contains(' ') {
                return true;
            }
        }
    }
    false
}

fn rename_project_ux_intent(user_part: &str, full_prompt: &str) -> Option<PlatformUxIntent> {
    let name = extract_rename_project_name(user_part)?;
    let project = extract_rename_project_target(user_part)
        .or_else(|| bound_project_from_prompt(full_prompt));
    let mut args = serde_json::json!({ "name": name });
    if let Some(ref p) = project {
        args["project"] = serde_json::json!(p);
    }
    let path_slug = project
        .clone()
        .unwrap_or_else(|| crate::project_layout::slugify_name(&name));
    Some(PlatformUxIntent {
        tool: "rename_project".into(),
        path: format!("/projects/{path_slug}"),
        summary: format!("Renaming project to `{name}`"),
        action: "goto".into(),
        project: project.or(Some(path_slug)),
        args: Some(args),
    })
}

fn extract_rename_project_name(prompt: &str) -> Option<String> {
    let s = prompt.trim();
    let lower = s.to_lowercase();
    if let Some(idx) = lower.rfind(" to ") {
        let rest = s.get(idx + 4..).unwrap_or("");
        if let Some(name) = extract_project_name_phrase(rest) {
            if !matches!(
                name.to_lowercase().as_str(),
                "it" | "this" | "that" | "the" | "project"
            ) {
                return Some(name);
            }
        }
    }
    for prefix in ["rename_project ", "update_project "] {
        if let Some(idx) = lower.find(prefix) {
            let rest = s.get(idx + prefix.len()..).unwrap_or("");
            if let Some(name) = extract_project_name_phrase(rest) {
                return Some(name);
            }
        }
    }
    None
}

/// Explicit current project in "rename agent-registry to …" (not "this"/"the").
fn extract_rename_project_target(prompt: &str) -> Option<String> {
    let s = prompt.trim();
    let lower = s.to_lowercase();
    let idx = lower.find("rename ")?;
    let rest = s.get(idx + "rename ".len()..)?;
    let rest_l = rest.to_lowercase();
    let until = rest_l.find(" to ").unwrap_or(rest.len());
    let mid = rest.get(..until).unwrap_or("").trim();
    let mid_l = mid.to_lowercase();
    if mid_l.is_empty()
        || mid_l == "this project"
        || mid_l == "the project"
        || mid_l == "this"
        || mid_l == "it"
        || mid_l == "project"
    {
        return None;
    }
    extract_project_name_phrase(mid).map(|n| crate::project_layout::slugify_name(&n))
}

/// Slug from the host-injected bound-project preamble, else CURRENT_PROJECT.
fn bound_project_from_prompt(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        if lower.starts_with("- slug:") || lower.starts_with("slug:") {
            if let Some(start) = t.find('`') {
                if let Some(end) = t[start + 1..].find('`') {
                    let slug = t[start + 1..start + 1 + end].trim();
                    if !slug.is_empty() {
                        return Some(slug.to_string());
                    }
                }
            }
            let after = t.split(':').nth(1).unwrap_or("").trim();
            let slug = after.trim_matches('`').trim();
            if !slug.is_empty() {
                return Some(slug.to_string());
            }
        }
    }
    crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok()
        .filter(|s| !s.is_empty())
}

/// Product-level create (repo/project), not package/layer file create.
fn looks_like_create_project_prompt(lower: &str) -> bool {
    lower.contains("create_project")
        || lower.contains("create_repo")
        || lower.contains("create a project")
        || lower.contains("create project")
        || lower.contains("create a new project")
        || lower.contains("create new project")
        || lower.contains("new project")
        || lower.contains("make a project")
        || lower.contains("make a new project")
        || lower.contains("scaffold a project")
        || lower.contains("scaffold project")
        || (lower.contains("bootstrap") && lower.contains("project"))
}

fn extract_name_token(original: &str, lower_token: &str) -> Option<String> {
    for w in original.split_whitespace() {
        let cleaned = w.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.'
        });
        if cleaned.eq_ignore_ascii_case(lower_token) {
            return Some(cleaned.to_string());
        }
    }
    None
}

/// True when the prompt is a short, primary platform-navigation request
/// (AgentDock chips, "open changes", "create pull request") — safe to
/// short-circuit without the LLM.
///
/// False for multi-line coding / Fix-button prompts that merely *document*
/// tool names in SOP text (must not steal the turn).
fn is_primary_platform_nav_prompt(lower: &str) -> bool {
    let lines = lower.lines().filter(|l| !l.trim().is_empty()).count();
    // Long or multi-section prompts are agent work, not nav chips.
    if lower.len() > 160 || lines > 3 {
        return false;
    }
    // Diagnostics / git-shaped SOP keywords ⇒ coding turn, not pure nav.
    const CODING_MARKERS: &[&str] = &[
        "## issues",
        "investigate and fix",
        "fix this issue",
        "fix these issues",
        "fix all open issues",
        "veil_check",
        "session_commit",
        "session_status",
        "write_source",
        "create_branch",
        "submit_pr",
        "git-shaped",
        "per-slice",
        "rationales",
        "hint:",
        "[acs-",
        "[ddd-",
        "prefer minimal correct",
    ];
    if CODING_MARKERS.iter().any(|m| lower.contains(m)) {
        return false;
    }
    true
}

#[cfg(test)]
mod platform_ux_tests {
    use super::*;

    #[test]
    fn create_project_intent_extracts_name() {
        let ux = parse_platform_ux_intent("create project agentic-workflows").unwrap();
        assert_eq!(ux.tool, "create_project");
        assert_eq!(ux.project.as_deref(), Some("agentic-workflows"));
        let name = ux.args.as_ref().unwrap().get("name").unwrap().as_str().unwrap();
        assert_eq!(name, "agentic-workflows");
    }

    #[test]
    fn create_project_named_phrase() {
        let ux = parse_platform_ux_intent("create a project named foo-bar").unwrap();
        assert_eq!(ux.tool, "create_project");
        assert_eq!(ux.project.as_deref(), Some("foo-bar"));
    }

    #[test]
    fn multi_step_create_does_not_short_circuit() {
        assert!(parse_platform_ux_intent(
            "create project foo and then design the full domain model with entities"
        )
        .is_none());
    }

    #[test]
    fn list_prs_still_maps() {
        let ux = parse_platform_ux_intent("open changes").unwrap();
        assert_eq!(ux.tool, "list_prs");
    }

    #[test]
    fn create_pr_chip_still_maps() {
        let ux = parse_platform_ux_intent("create pull request").unwrap();
        assert_eq!(ux.tool, "create_pr");
        let ux2 = parse_platform_ux_intent("create_pr").unwrap();
        assert_eq!(ux2.tool, "create_pr");
    }

    /// Regression: Diagnostics "Fix" / formatIssuePrompt embeds `create_pr`
    /// in SOP text. Must NOT short-circuit to open empty /pulls/new.
    #[test]
    fn fix_issue_prompt_does_not_false_trigger_create_pr() {
        let prompt = r#"Investigate and fix this issue on construct `Agent` in project `agent-registry`.

Use IDE / project tools as needed (read source, apply edits, re-check). Prefer minimal correct fixes.
After every edit: veil_check. Fix any new errors/warnings you introduced on this same turn.
Git-shaped workflow (you decide branch/commit — do not ask the operator for every step):
session_status → multi-step? create_branch → veil_check baseline → fix one class → write → veil_check → session_commit.
When the task is complete: create_pr (title + description with per-slice rationales) → submit_pr.
NEVER merge_branch or merge_pr unless the operator explicitly asks to merge. Humans review on /review.

## Issues
1. Warning [ACS-010] @ Agent: optional field should use bang
   Hint: use field!: Type
"#;
        assert!(
            parse_platform_ux_intent(prompt).is_none(),
            "Fix-button prompt must reach ACP, not host create_pr short-circuit"
        );
    }

    #[test]
    fn multi_step_create_gets_host_prefix_tools() {
        let steps = host_platform_prefix_steps(
            "create project agent-registry and then design the full domain model with entities",
        );
        assert!(
            steps.iter().any(|s| s.tool == "create_project"),
            "expected create_project step: {steps:?}"
        );
        assert!(
            steps.iter().any(|s| s.tool == "navigate_to"),
            "expected navigate_to first: {steps:?}"
        );
        let create = steps.iter().find(|s| s.tool == "create_project").unwrap();
        assert_eq!(create.args["name"], "agent-registry");
    }

    #[test]
    fn bootstrap_agent_registry_name() {
        let steps = host_platform_prefix_steps(
            "Go ahead and bootstrap the Agent Registry project. Pull in dashbot.",
        );
        assert!(
            steps.iter().any(|s| s.tool == "create_project"),
            "{steps:?}"
        );
        let create = steps.iter().find(|s| s.tool == "create_project").unwrap();
        let name = create.args["name"].as_str().unwrap();
        assert!(
            name.contains("agent") && name.contains("registry"),
            "got name={name}"
        );
    }

    /// Regression: multi-step "create a new project called …" must NOT be
    /// parsed as create_file (that path hits MultiProjectProvider without
    /// CURRENT_PROJECT → "project scope missing").
    #[test]
    fn agent_registry_prompt_is_create_project_not_create_file() {
        let prompt = r#"We need to create a new project called "Agent Registry". It will contain most of the domain model and functionality that is currently in "dashbot" crate in the dlx-core repository. Create the project and bring all that code/functionlity in to the veil."#;
        assert!(
            parse_create_file(prompt).is_none(),
            "must not false-positive create_file"
        );
        assert!(
            !is_structured_agent_command(prompt),
            "must not short-circuit structured host create_file path"
        );
        assert!(
            parse_platform_ux_intent(prompt).is_none(),
            "multi-step create stays off simple UX short-circuit"
        );
        let steps = host_platform_prefix_steps(prompt);
        let create = steps
            .iter()
            .find(|s| s.tool == "create_project")
            .expect("host prefix must schedule create_project");
        // Display name preserved; API derives slug agent-registry
        assert_eq!(create.args["name"], "Agent Registry");
        assert_eq!(create.args["via"], "server");
        assert_eq!(slugify_project_name("Agent Registry"), "agent-registry");
    }

    #[test]
    fn long_task_mentioning_open_ide_is_not_nav() {
        let prompt = "Complete the pre-deploy hook.\n\
             Follow layer teaching. After open_ide / select_file the pack is bound.\n\
             Do not redefine declared types.";
        assert!(
            !is_structured_agent_command(prompt),
            "long task that mentions open_ide must go to ACP, not host nav"
        );
        assert!(is_structured_agent_command("open_ide dlx-bus"));
    }

    #[test]
    fn create_file_still_matches_package_named() {
        let got = parse_create_file("create a package named Widget").unwrap();
        assert_eq!(got.0.to_lowercase(), "widget");
        assert_eq!(got.1, "package");
    }

    #[test]
    fn quoted_multi_word_project_name() {
        let name = extract_create_project_name(
            r#"create a new project called "Agent Registry" and design domain"#,
        )
        .unwrap();
        assert_eq!(name, "Agent Registry");
        assert_eq!(slugify_project_name(&name), "agent-registry");
    }

    #[test]
    fn rename_project_prompt_is_not_construct_rename() {
        assert!(parse_rename("rename this project to Agent Core").is_none());
        assert!(looks_like_rename_project_prompt(
            "rename this project to \"agent core\""
        ));
        let ux = parse_platform_ux_intent(r#"rename this project to "Agent Core""#).unwrap();
        assert_eq!(ux.tool, "rename_project");
        assert_eq!(ux.args.as_ref().unwrap()["name"], "Agent Core");
    }

    #[test]
    fn rename_project_host_prefix_from_bound_slug() {
        let prompt = r#"## Bound project (server — authoritative)
- Slug: `agent-registry`
- Files on disk/S3: 2

# User request
we fixed your tools... now rename this project to "Agent Core"
"#;
        assert!(
            parse_rename(prompt).is_none(),
            "must not steal as construct rename"
        );
        let steps = host_platform_prefix_steps(prompt);
        let step = steps
            .iter()
            .find(|s| s.tool == "rename_project")
            .expect("host prefix must schedule rename_project");
        assert_eq!(step.args["name"], "Agent Core");
        assert_eq!(step.args["project"], "agent-registry");
    }

    #[test]
    fn rename_explicit_slug_target() {
        let ux = parse_platform_ux_intent("rename agent-registry to Agent Core").unwrap();
        assert_eq!(ux.tool, "rename_project");
        assert_eq!(ux.args.as_ref().unwrap()["project"], "agent-registry");
        assert_eq!(ux.args.as_ref().unwrap()["name"], "Agent Core");
    }

    #[test]
    fn construct_rename_still_parses() {
        let (from, to) = parse_rename("rename Widget to Gadget").unwrap();
        assert_eq!(from, "Widget");
        assert_eq!(to, "Gadget");
    }
}
