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
        let kind_raw = std::env::var("VEIL_MODEL_PROVIDER")
            .unwrap_or_else(|_| "echo".into())
            .to_lowercase();
        let kind = match kind_raw.as_str() {
            "openai" | "openai-compatible" => ProviderKind::OpenAi,
            "ollama" => ProviderKind::Ollama,
            "bedrock" => ProviderKind::Bedrock,
            "acp" | "kiro" => ProviderKind::Acp,
            "echo" | "heuristic" | "" => ProviderKind::Echo,
            other => {
                tracing::warn!(provider = %other, "unknown VEIL_MODEL_PROVIDER; using echo");
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
        Self {
            kind,
            model: std::env::var("VEIL_MODEL_NAME").unwrap_or(default_model),
            base_url: std::env::var("VEIL_MODEL_BASE_URL").ok(),
            api_key: std::env::var("VEIL_MODEL_API_KEY")
                .ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            region: std::env::var("VEIL_MODEL_REGION")
                .ok()
                .or_else(|| std::env::var("AWS_REGION").ok()),
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
    use crate::rig_tools::{
        CheckTool, CreateFileTool, ListFilesTool, OutlineTool, ReadSourceTool, RenameTool,
        SelectFileTool, WriteSourceTool,
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
                .tool(WriteSourceTool { ws: ws.clone() });
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
                .tool(WriteSourceTool { ws: ws.clone() });
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

pub fn list_provider_info() -> serde_json::Value {
    let cfg = ModelConfig::from_env();
    if cfg.supports_acp() {
        return crate::acp::acp_info();
    }
    serde_json::json!({
        "provider": cfg.kind_name(),
        "models": [cfg.model],
        "rig": cfg.supports_rig_agent(),
        "acp": false,
        "supports_tools": cfg.supports_rig_agent(),
        "config": {
            "kind": cfg.kind_name(),
            "model": cfg.model,
            "base_url": cfg.base_url,
            "region": cfg.region,
            "has_api_key": cfg.api_key.is_some(),
        }
    })
}
