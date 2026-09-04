//! Model providers via the **Rig** SDK (AGT-003).
//!
//! All LLM access goes through Rig. Configure with env vars — no engine/domain
//! knowledge of vendors.

use async_trait::async_trait;
use rig_core::client::{CompletionClient, Nothing, ProviderClient};
use rig_core::completion::Prompt;
use rig_core::providers::{ollama, openai};
use serde::{Deserialize, Serialize};

/// One chat message for the model port (UI / session history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Portable completion request (non-agent path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
}

/// Portable completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteResponse {
    pub content: String,
    pub model: String,
    pub provider: String,
}

/// Which model / agent backend to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    /// No network — local guidance text.
    Echo,
    /// OpenAI or any OpenAI-compatible base URL (Rig openai client).
    OpenAi,
    /// Local/remote Ollama (Rig ollama client).
    Ollama,
    /// Amazon Bedrock — reserved; use OpenAI-compatible gateway or future rig feature.
    Bedrock,
    /// External ACP agent (e.g. Kiro CLI via `kiro-cli acp`).
    Acp,
}

/// Config from env (AGT-003 / AGT-012).
#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub kind: ProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub region: Option<String>,
}

impl ModelConfig {
    pub fn from_env() -> Self {
        Self::resolve(&crate::config::VeilConfig::default())
    }

    /// Resolve the effective model config by layering **env over persisted
    /// config over built-in default**.
    ///
    /// Precedence (highest first):
    /// 1. Explicit env vars (`VEIL_MODEL_PROVIDER`, `VEIL_MODEL_NAME`,
    ///    `VEIL_MODEL_BASE_URL`, `VEIL_MODEL_API_KEY`/`OPENAI_API_KEY`,
    ///    `VEIL_MODEL_REGION`/`AWS_REGION`) — ops/CI (`.env.dlx`) force these.
    /// 2. Persisted [`crate::config::AgentConfig`] (`~/.veil/config.json`).
    /// 3. Built-in default (`echo`).
    ///
    /// For BYOK the persisted config never carries a raw key; it names an env
    /// var (`api_key_env`) that is read here.
    pub fn resolve(cfg: &crate::config::VeilConfig) -> Self {
        let agent = cfg.agent.as_ref();

        // Provider: env wins, else persisted, else echo.
        let kind_raw = std::env::var("VEIL_MODEL_PROVIDER")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| agent.map(|a| a.provider.clone()).filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| "echo".into())
            .to_lowercase();
        let kind = match kind_raw.as_str() {
            "openai" | "openai-compatible" => ProviderKind::OpenAi,
            "ollama" => ProviderKind::Ollama,
            "bedrock" => ProviderKind::Bedrock,
            "acp" | "kiro" => ProviderKind::Acp,
            "echo" | "heuristic" | "" => ProviderKind::Echo,
            other => {
                tracing::warn!(provider = %other, "unknown agent provider; using echo");
                ProviderKind::Echo
            }
        };
        let default_model = match kind {
            ProviderKind::Ollama => "llama3.2".to_string(),
            ProviderKind::OpenAi => "gpt-4o-mini".to_string(),
            ProviderKind::Bedrock => "anthropic.claude-3-sonnet".to_string(),
            ProviderKind::Acp => "kiro".to_string(),
            ProviderKind::Echo => "echo".to_string(),
        };
        let model = std::env::var("VEIL_MODEL_NAME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| agent.and_then(|a| a.model.clone()).filter(|s| !s.trim().is_empty()))
            .unwrap_or(default_model);
        let base_url = std::env::var("VEIL_MODEL_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| agent.and_then(|a| a.base_url.clone()).filter(|s| !s.trim().is_empty()));
        let region = std::env::var("VEIL_MODEL_REGION")
            .ok()
            .or_else(|| std::env::var("AWS_REGION").ok())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| agent.and_then(|a| a.region.clone()).filter(|s| !s.trim().is_empty()));
        // API key: explicit env vars first; else read the env var NAMED by the
        // persisted `api_key_env` (never a raw key from config.json).
        let api_key = std::env::var("VEIL_MODEL_API_KEY")
            .ok()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                agent
                    .and_then(|a| a.api_key_env.as_ref())
                    .filter(|n| !n.trim().is_empty())
                    .and_then(|name| std::env::var(name).ok())
                    .filter(|s| !s.trim().is_empty())
            });

        Self {
            kind,
            model,
            base_url,
            api_key,
            region,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            ProviderKind::Echo => "echo",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Ollama => "ollama",
            ProviderKind::Bedrock => "bedrock",
            ProviderKind::Acp => "acp",
        }
    }

    /// Whether this config can run a full Rig agent with tools.
    pub fn supports_rig_agent(&self) -> bool {
        matches!(self.kind, ProviderKind::OpenAi | ProviderKind::Ollama)
    }

    /// External ACP agent (Kiro, etc.).
    pub fn supports_acp(&self) -> bool {
        matches!(self.kind, ProviderKind::Acp)
    }
}

/// Pluggable port (thin over Rig) for simple completions and listing.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn list_models(&self) -> Result<Vec<String>, String>;
    async fn complete(&self, req: CompleteRequest) -> Result<CompleteResponse, String>;
}

pub struct EchoProvider {
    pub model: String,
}

#[async_trait]
impl ModelProvider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }
    async fn list_models(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.model.clone()])
    }
    async fn complete(&self, req: CompleteRequest) -> Result<CompleteResponse, String> {
        let last = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        Ok(CompleteResponse {
            content: format!(
                "[echo / offline] Understood: {last:?}\n\
                 Tools (heuristic or Rig when configured): check · outline · rename.\n\
                 Set VEIL_MODEL_PROVIDER=openai|ollama and credentials for Rig-backed agents."
            ),
            model: self.model.clone(),
            provider: "echo".into(),
        })
    }
}

/// Complete with env-configured Rig provider (single-shot prompt, no tools).
pub async fn complete_with_env(req: CompleteRequest) -> Result<CompleteResponse, String> {
    let cfg = ModelConfig::from_env();
    match cfg.kind {
        ProviderKind::Echo => {
            EchoProvider {
                model: cfg.model.clone(),
            }
            .complete(req)
            .await
        }
        ProviderKind::OpenAi => complete_openai(&cfg, req).await,
        ProviderKind::Ollama => complete_ollama(&cfg, req).await,
        ProviderKind::Bedrock => Err(format!(
            "bedrock via Rig: use an OpenAI-compatible Bedrock gateway \
             (VEIL_MODEL_PROVIDER=openai + VEIL_MODEL_BASE_URL) or set ollama. region={}",
            cfg.region.as_deref().unwrap_or("us-east-1")
        )),
        ProviderKind::Acp => Err(
            "ACP agents use POST /api/agent/turn (session/prompt), not complete_with_env".into(),
        ),
    }
}

async fn complete_openai(
    cfg: &ModelConfig,
    req: CompleteRequest,
) -> Result<CompleteResponse, String> {
    let model_name = req.model.clone().unwrap_or_else(|| cfg.model.clone());
    let client = if let Some(base) = &cfg.base_url {
        let key = cfg
            .api_key
            .clone()
            .unwrap_or_else(|| "not-needed".into());
        openai::Client::builder()
            .api_key(&key)
            .base_url(base)
            .build()
            .map_err(|e| e.to_string())?
    } else {
        openai::Client::from_env().map_err(|e| e.to_string())?
    };
    let agent = client.agent(&model_name).build();
    // Flatten messages into a single prompt (agent history later).
    let prompt = flatten_messages(&req.messages);
    let content = agent.prompt(prompt).await.map_err(|e| e.to_string())?;
    Ok(CompleteResponse {
        content,
        model: model_name,
        provider: "openai".into(),
    })
}

fn ollama_client(cfg: &ModelConfig) -> Result<ollama::Client, String> {
    // Local Ollama needs no API key. Custom base URL uses builder with empty key.
    if let Some(base) = &cfg.base_url {
        ollama::Client::builder()
            .api_key("")
            .base_url(base)
            .build()
            .map_err(|e| e.to_string())
    } else {
        ollama::Client::new(Nothing).map_err(|e| e.to_string())
    }
}

async fn complete_ollama(
    cfg: &ModelConfig,
    req: CompleteRequest,
) -> Result<CompleteResponse, String> {
    let model_name = req.model.clone().unwrap_or_else(|| cfg.model.clone());
    let client = ollama_client(cfg)?;
    let agent = client.agent(&model_name).build();
    let prompt = flatten_messages(&req.messages);
    let content = agent.prompt(prompt).await.map_err(|e| e.to_string())?;
    Ok(CompleteResponse {
        content,
        model: model_name,
        provider: "ollama".into(),
    })
}

fn flatten_messages(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Build a Rig agent with VEIL tools attached (AGT-006).
/// When Mind Palace is configured (`MIND_PALACE=1`), wiki tools are attached too.
pub async fn prompt_with_tools(
    cfg: &ModelConfig,
    preamble: &str,
    user_prompt: &str,
    ws: crate::rig_tools::Workspace,
) -> Result<String, String> {
    use crate::platform_tools::{
        ApproveChangeTool, CreateChangeTool, CreateProjectTool, DeployStatusTool, GetConfigTool,
        ListChangesTool, ListProjectsTool, MergeChangeTool, NavigateToTool, OpenIdeTool,
        OpenProjectTool, ProvisionProjectTool, RenameProjectTool,
    };
    use crate::rig_tools::{
        CheckTool, CreateFileTool, DevLogsTool, DevRestartTool, DevStatusTool, HttpRequestTool,
        ListFilesTool, ListRoutesTool, OutlineTool, ReadGeneratedTool, ReadSourceTool, RenameTool,
        SelectFileTool, SmokeStatusTool, StubGenTool, StubGetTool, StubInstallTool, StubListTool,
        StubSearchTool,
        WriteSourceTool,
    };

    let mut preamble = preamble.to_string();
    let palace = crate::mind_palace_tools::try_palace().await;
    if palace.is_some() {
        preamble.push_str(crate::mind_palace_tools::preamble_addon());
    }

    match cfg.kind {
        ProviderKind::OpenAi => {
            let client = if let Some(base) = &cfg.base_url {
                let key = cfg
                    .api_key
                    .clone()
                    .unwrap_or_else(|| "not-needed".into());
                openai::Client::builder()
                    .api_key(&key)
                    .base_url(base)
                    .build()
                    .map_err(|e| e.to_string())?
            } else {
                openai::Client::from_env().map_err(|e| e.to_string())?
            };
            let mut builder = client
                .agent(&cfg.model)
                .preamble(&preamble)
                .tool(CheckTool { ws: ws.clone() })
                .tool(OutlineTool { ws: ws.clone() })
                .tool(ReadSourceTool { ws: ws.clone() })
                .tool(RenameTool { ws: ws.clone() })
                .tool(ListFilesTool { ws: ws.clone() })
                .tool(SelectFileTool { ws: ws.clone() })
                .tool(CreateFileTool { ws: ws.clone() })
                .tool(WriteSourceTool { ws: ws.clone() })
                .tool(DevStatusTool { ws: ws.clone() })
                .tool(DevLogsTool { ws: ws.clone() })
                .tool(ReadGeneratedTool { ws: ws.clone() })
                .tool(ListRoutesTool { ws: ws.clone() })
                .tool(HttpRequestTool { ws: ws.clone() })
                .tool(DevRestartTool { ws: ws.clone() })
                .tool(SmokeStatusTool { ws: ws.clone() })
                .tool(StubListTool { ws: ws.clone() })
                .tool(StubGetTool { ws: ws.clone() })
                .tool(StubGenTool { ws: ws.clone() })
                .tool(StubInstallTool { ws: ws.clone() })
                .tool(StubSearchTool { ws: ws.clone() })
                // Platform UX (create_project, SDLC, deploy, nav)
                .tool(ListProjectsTool)
                .tool(CreateProjectTool)
                .tool(RenameProjectTool)
                .tool(OpenProjectTool)
                .tool(OpenIdeTool)
                .tool(NavigateToTool)
                .tool(ListChangesTool)
                .tool(CreateChangeTool)
                .tool(ApproveChangeTool)
                .tool(MergeChangeTool)
                .tool(ProvisionProjectTool)
                .tool(DeployStatusTool)
                .tool(GetConfigTool);
            if let Some(ref p) = palace {
                let (search, read, traverse, create, update, list) =
                    crate::mind_palace_tools::tools_for_agent(p);
                builder = builder
                    .tool(search)
                    .tool(read)
                    .tool(traverse)
                    .tool(create)
                    .tool(update)
                    .tool(list);
            }
            let agent = builder.build();
            agent.prompt(user_prompt).await.map_err(|e| e.to_string())
        }
        ProviderKind::Ollama => {
            let client = ollama_client(cfg)?;
            let mut builder = client
                .agent(&cfg.model)
                .preamble(&preamble)
                .tool(CheckTool { ws: ws.clone() })
                .tool(OutlineTool { ws: ws.clone() })
                .tool(ReadSourceTool { ws: ws.clone() })
                .tool(RenameTool { ws: ws.clone() })
                .tool(ListFilesTool { ws: ws.clone() })
                .tool(SelectFileTool { ws: ws.clone() })
                .tool(CreateFileTool { ws: ws.clone() })
                .tool(WriteSourceTool { ws: ws.clone() })
                .tool(DevStatusTool { ws: ws.clone() })
                .tool(DevLogsTool { ws: ws.clone() })
                .tool(ReadGeneratedTool { ws: ws.clone() })
                .tool(ListRoutesTool { ws: ws.clone() })
                .tool(HttpRequestTool { ws: ws.clone() })
                .tool(DevRestartTool { ws: ws.clone() })
                .tool(SmokeStatusTool { ws: ws.clone() })
                .tool(StubListTool { ws: ws.clone() })
                .tool(StubGetTool { ws: ws.clone() })
                .tool(StubGenTool { ws: ws.clone() })
                .tool(StubInstallTool { ws: ws.clone() })
                .tool(StubSearchTool { ws: ws.clone() })
                .tool(ListProjectsTool)
                .tool(CreateProjectTool)
                .tool(RenameProjectTool)
                .tool(OpenProjectTool)
                .tool(OpenIdeTool)
                .tool(NavigateToTool)
                .tool(ListChangesTool)
                .tool(CreateChangeTool)
                .tool(ApproveChangeTool)
                .tool(MergeChangeTool)
                .tool(ProvisionProjectTool)
                .tool(DeployStatusTool)
                .tool(GetConfigTool);
            if let Some(ref p) = palace {
                let (search, read, traverse, create, update, list) =
                    crate::mind_palace_tools::tools_for_agent(p);
                builder = builder
                    .tool(search)
                    .tool(read)
                    .tool(traverse)
                    .tool(create)
                    .tool(update)
                    .tool(list);
            }
            let agent = builder.build();
            agent.prompt(user_prompt).await.map_err(|e| e.to_string())
        }
        ProviderKind::Echo | ProviderKind::Bedrock | ProviderKind::Acp => Err(
            "Rig tool agent requires VEIL_MODEL_PROVIDER=openai or ollama (use acp via run_turn)".into(),
        ),
    }
}

/// Seed process env vars from the persisted `agent` config so downstream
/// consumers that read env directly (the ACP spawner in `acp.rs`, legacy
/// `ModelConfig::from_env` callers) honor the UI/tool selection at boot.
///
/// **Env still wins**: a var already set in the environment (shell / `.env` /
/// CI) is left untouched — this only fills the *gaps* from persisted config.
/// Call once at startup (after load) and again after a config PATCH.
///
/// Does NOT export any API key value — only `api_key_env` NAMES an env var the
/// operator sets themselves; secrets never transit config.json.
pub fn export_agent_env(cfg: &crate::config::VeilConfig) {
    let Some(agent) = cfg.agent.as_ref() else {
        return;
    };
    // SAFETY: called single-threaded at startup / on the config-write path.
    unsafe {
        set_env_if_absent("VEIL_MODEL_PROVIDER", &agent.provider);
        if let Some(m) = agent.model.as_deref() {
            set_env_if_absent("VEIL_MODEL_NAME", m);
        }
        if let Some(b) = agent.base_url.as_deref() {
            set_env_if_absent("VEIL_MODEL_BASE_URL", b);
        }
        if let Some(r) = agent.region.as_deref() {
            set_env_if_absent("VEIL_MODEL_REGION", r);
        }
        if let Some(c) = agent.acp_command.as_deref() {
            set_env_if_absent("VEIL_ACP_COMMAND", c);
        }
        if let Some(a) = agent.acp_args.as_deref() {
            set_env_if_absent("VEIL_ACP_ARGS", a);
        }
        if let Some(a) = agent.acp_agent.as_deref() {
            set_env_if_absent("VEIL_ACP_AGENT", a);
        }
    }
}

/// Set an env var only if it is unset or empty (env-over-config invariant).
///
/// # Safety
/// Must be called from a single-threaded context (startup / config-write path).
unsafe fn set_env_if_absent(key: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    let present = std::env::var(key).map(|v| !v.trim().is_empty()).unwrap_or(false);
    if !present {
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

/// Effective model config: env over persisted `~/.veil/config.json` over default.
///
/// This is the single source of truth for "what provider is the inner agent
/// actually using right now" — used by `/api/agent/status`, `/api/models`, and
/// `list_provider_info`.
pub fn effective_model_config() -> ModelConfig {
    ModelConfig::resolve(&crate::config::load_config_or_default())
}

/// Whether the effective ACP command resolves on `PATH` (readiness hint).
fn command_on_path(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    // Absolute / relative path: check directly.
    if cmd.contains('/') {
        return std::path::Path::new(cmd).is_file();
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join(cmd);
            if cand.is_file() {
                return true;
            }
        }
    }
    false
}

/// Readiness check for the *effective* provider. Never hard-fails — returns a
/// `(ready, hint)` pair the UI/status endpoint surfaces.
pub fn provider_readiness(cfg: &ModelConfig, agent: Option<&crate::config::AgentConfig>) -> (bool, String) {
    match cfg.kind {
        ProviderKind::Acp => {
            let cmd = agent
                .and_then(|a| a.acp_command.clone())
                .or_else(|| std::env::var("VEIL_ACP_COMMAND").ok())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "kiro-cli".into());
            if command_on_path(&cmd) {
                (true, format!("ACP command `{cmd}` found on PATH"))
            } else {
                (false, format!("ACP command `{cmd}` not found on PATH — install Kiro CLI or set the command"))
            }
        }
        ProviderKind::OpenAi => {
            if cfg.api_key.is_some() {
                (true, "API key resolved".into())
            } else if cfg.base_url.is_some() {
                (true, "base_url set (local/keyless gateway assumed)".into())
            } else {
                let env_name = agent
                    .and_then(|a| a.api_key_env.clone())
                    .unwrap_or_else(|| "OPENAI_API_KEY".into());
                (false, format!("no API key — set env `{env_name}` (or VEIL_MODEL_API_KEY / OPENAI_API_KEY), or a base_url"))
            }
        }
        ProviderKind::Ollama => {
            let base = cfg.base_url.clone().unwrap_or_else(|| "http://localhost:11434".into());
            (true, format!("Ollama at {base} (ensure the daemon is running)"))
        }
        ProviderKind::Bedrock => {
            let region = cfg.region.clone().unwrap_or_default();
            if region.is_empty() {
                (false, "Bedrock region not set — set region (or VEIL_MODEL_REGION/AWS_REGION). NOTE: Bedrock completions are config-only in v1 (not yet wired to a Rig client).".into())
            } else {
                (true, format!("region {region} set. NOTE: Bedrock completions are config-only in v1 (not yet wired to a Rig client)."))
            }
        }
        ProviderKind::Echo => (true, "offline echo provider (no model calls)".into()),
    }
}

/// The set of selectable providers + the fields each one needs (drives the
/// Config UI form and documents the agent tool contract).
pub fn available_providers() -> serde_json::Value {
    serde_json::json!([
        {
            "id": "acp",
            "label": "ACP (external agent — e.g. Kiro CLI)",
            "fields": ["acp_command", "acp_args", "acp_agent", "model"],
            "wired": true,
            "note": "Spawns an external agent process over ACP. Default command kiro-cli, args acp."
        },
        {
            "id": "bedrock",
            "label": "Amazon Bedrock",
            "fields": ["model", "region"],
            "wired": false,
            "note": "Config-only in v1: selection/persistence/readiness work, but Bedrock completions are not yet wired to a Rig client. Use an OpenAI-compatible Bedrock gateway (openai + base_url) for live calls."
        },
        {
            "id": "openai",
            "label": "OpenAI-compatible (BYOK: OpenAI, OpenRouter, Together, local gateway)",
            "fields": ["base_url", "model", "api_key_env"],
            "wired": true,
            "note": "api_key_env is the NAME of an env var holding the key — the key is never stored in config.json."
        },
        {
            "id": "ollama",
            "label": "Ollama (local)",
            "fields": ["base_url", "model"],
            "wired": true,
            "note": "Local or remote Ollama. No API key required."
        },
        {
            "id": "echo",
            "label": "Echo (offline / no model)",
            "fields": [],
            "wired": true,
            "note": "No network. Returns guidance text."
        }
    ])
}

pub fn list_provider_info() -> serde_json::Value {
    let stored = crate::config::load_config_or_default();
    let cfg = ModelConfig::resolve(&stored);
    let (ready, hint) = provider_readiness(&cfg, stored.agent.as_ref());
    // Whether the env is overriding the persisted provider (ops/CI signal).
    let env_provider = std::env::var("VEIL_MODEL_PROVIDER")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_override = env_provider.is_some();
    if cfg.supports_acp() {
        let mut info = crate::acp::acp_info();
        if let Some(obj) = info.as_object_mut() {
            obj.insert("ready".into(), serde_json::json!(ready));
            obj.insert("readiness_hint".into(), serde_json::json!(hint));
            obj.insert("env_override".into(), serde_json::json!(env_override));
            obj.insert("available_providers".into(), available_providers());
        }
        return info;
    }
    serde_json::json!({
        "provider": cfg.kind_name(),
        "models": [cfg.model],
        "model": cfg.model,
        "rig": cfg.supports_rig_agent(),
        "acp": false,
        "supports_tools": cfg.supports_rig_agent(),
        "ready": ready,
        "readiness_hint": hint,
        "env_override": env_override,
        "available_providers": available_providers(),
        "config": {
            "kind": cfg.kind_name(),
            "model": cfg.model,
            "base_url": cfg.base_url,
            "region": cfg.region,
            "has_api_key": cfg.api_key.is_some(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, VeilConfig};
    use std::sync::Mutex;

    // Env vars are process-global; serialize env-touching tests.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    const KEYS: &[&str] = &[
        "VEIL_MODEL_PROVIDER",
        "VEIL_MODEL_NAME",
        "VEIL_MODEL_BASE_URL",
        "VEIL_MODEL_API_KEY",
        "OPENAI_API_KEY",
        "VEIL_MODEL_REGION",
        "AWS_REGION",
    ];

    fn clear_env() {
        // SAFETY: guarded by ENV_GUARD; test-only single-threaded region.
        unsafe {
            for k in KEYS {
                std::env::remove_var(k);
            }
        }
    }

    fn cfg_with_agent(agent: AgentConfig) -> VeilConfig {
        VeilConfig {
            agent: Some(agent),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_uses_config_when_env_absent() {
        let _g = ENV_GUARD.lock().unwrap();
        clear_env();
        let cfg = ModelConfig::resolve(&cfg_with_agent(AgentConfig {
            provider: "ollama".into(),
            model: Some("mistral".into()),
            base_url: Some("http://localhost:11434".into()),
            ..Default::default()
        }));
        assert_eq!(cfg.kind, ProviderKind::Ollama);
        assert_eq!(cfg.model, "mistral");
        assert_eq!(cfg.base_url.as_deref(), Some("http://localhost:11434"));
        clear_env();
    }

    #[test]
    fn resolve_env_wins_over_config() {
        let _g = ENV_GUARD.lock().unwrap();
        clear_env();
        // SAFETY: guarded.
        unsafe {
            std::env::set_var("VEIL_MODEL_PROVIDER", "openai");
            std::env::set_var("VEIL_MODEL_NAME", "gpt-4o");
        }
        let cfg = ModelConfig::resolve(&cfg_with_agent(AgentConfig {
            provider: "ollama".into(),
            model: Some("mistral".into()),
            ..Default::default()
        }));
        assert_eq!(cfg.kind, ProviderKind::OpenAi, "env provider must win");
        assert_eq!(cfg.model, "gpt-4o", "env model must win");
        clear_env();
    }

    #[test]
    fn resolve_defaults_when_both_absent() {
        let _g = ENV_GUARD.lock().unwrap();
        clear_env();
        let cfg = ModelConfig::resolve(&VeilConfig::default());
        assert_eq!(cfg.kind, ProviderKind::Echo);
        assert_eq!(cfg.model, "echo");
        clear_env();
    }

    #[test]
    fn resolve_reads_api_key_from_named_env_var() {
        let _g = ENV_GUARD.lock().unwrap();
        clear_env();
        // SAFETY: guarded. Simulate a BYOK env var named in config.
        unsafe {
            std::env::set_var("MY_BYOK_KEY", "sk-test-123");
        }
        let cfg = ModelConfig::resolve(&cfg_with_agent(AgentConfig {
            provider: "openai".into(),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            api_key_env: Some("MY_BYOK_KEY".into()),
            ..Default::default()
        }));
        assert_eq!(cfg.kind, ProviderKind::OpenAi);
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test-123"));
        // SAFETY: guarded.
        unsafe {
            std::env::remove_var("MY_BYOK_KEY");
        }
        clear_env();
    }

    #[test]
    fn readiness_bedrock_requires_region() {
        let _g = ENV_GUARD.lock().unwrap();
        clear_env();
        let agent = AgentConfig {
            provider: "bedrock".into(),
            ..Default::default()
        };
        let cfg = ModelConfig::resolve(&cfg_with_agent(agent.clone()));
        let (ready, hint) = provider_readiness(&cfg, Some(&agent));
        assert!(!ready);
        assert!(hint.contains("region"), "{hint}");

        let agent2 = AgentConfig {
            provider: "bedrock".into(),
            region: Some("us-west-2".into()),
            ..Default::default()
        };
        let cfg2 = ModelConfig::resolve(&cfg_with_agent(agent2.clone()));
        let (ready2, _) = provider_readiness(&cfg2, Some(&agent2));
        assert!(ready2);
        clear_env();
    }

    #[test]
    fn available_providers_lists_five() {
        let list = available_providers();
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 5);
        let ids: Vec<&str> = arr.iter().filter_map(|p| p["id"].as_str()).collect();
        for want in ["acp", "bedrock", "openai", "ollama", "echo"] {
            assert!(ids.contains(&want), "missing provider {want}");
        }
        // Bedrock is flagged config-only.
        let bedrock = arr.iter().find(|p| p["id"] == "bedrock").unwrap();
        assert_eq!(bedrock["wired"], serde_json::json!(false));
    }
}
