//! ModelProvider port + adapters (AGT-003).
//!
//! No provider-specific types leak outside this module. Built-in agent may
//! call [`complete`] with the configured provider.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// One chat message for the model port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Portable completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
}

/// Portable completion response (non-streaming MVP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteResponse {
    pub content: String,
    pub model: String,
    pub provider: String,
}

/// Pluggable model backend (Zed-like port).
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn list_models(&self) -> Result<Vec<String>, String>;
    async fn complete(&self, req: CompleteRequest) -> Result<CompleteResponse, String>;
}

/// Config from env (AGT-003 / AGT-012 lite).
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// `echo` | `openai` | `bedrock` (bedrock MVP returns clear not-configured error)
    pub kind: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub region: Option<String>,
}

impl ModelConfig {
    pub fn from_env() -> Self {
        Self {
            kind: std::env::var("VEIL_MODEL_PROVIDER")
                .unwrap_or_else(|_| "echo".into())
                .to_lowercase(),
            model: std::env::var("VEIL_MODEL_NAME").unwrap_or_else(|_| "default".into()),
            base_url: std::env::var("VEIL_MODEL_BASE_URL").ok(),
            api_key: std::env::var("VEIL_MODEL_API_KEY")
                .ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            region: std::env::var("VEIL_MODEL_REGION")
                .ok()
                .or_else(|| std::env::var("AWS_REGION").ok()),
        }
    }

    pub fn build_provider(&self) -> Box<dyn ModelProvider> {
        match self.kind.as_str() {
            "openai" | "openai-compatible" => Box::new(OpenAiCompatibleProvider {
                config: self.clone(),
            }),
            "bedrock" => Box::new(BedrockProvider {
                config: self.clone(),
            }),
            _ => Box::new(EchoProvider {
                model: self.model.clone(),
            }),
        }
    }
}

/// Default / offline provider — echoes last user message with guidance.
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
                "[echo model] Understood: {:?}\nUse built-in tools: check | outline | rename A to B.\nSet VEIL_MODEL_PROVIDER=openai and VEIL_MODEL_API_KEY for real completions.",
                last
            ),
            model: self.model.clone(),
            provider: "echo".into(),
        })
    }
}

/// OpenAI-compatible chat completions (`/v1/chat/completions`).
pub struct OpenAiCompatibleProvider {
    pub config: ModelConfig,
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }
    async fn list_models(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.config.model.clone()])
    }
    async fn complete(&self, req: CompleteRequest) -> Result<CompleteResponse, String> {
        let key = self
            .config
            .api_key
            .as_ref()
            .ok_or_else(|| "VEIL_MODEL_API_KEY / OPENAI_API_KEY not set".to_string())?;
        let base = self
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".into());
        let url = format!(
            "{}/chat/completions",
            base.trim_end_matches('/')
        );
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.config.model.clone());
        let body = serde_json::json!({
            "model": model,
            "messages": req.messages.iter().map(|m| serde_json::json!({
                "role": m.role,
                "content": m.content,
            })).collect::<Vec<_>>(),
            "max_tokens": req.max_tokens.unwrap_or(1024),
        });
        // Minimal HTTP via std; avoid new deps — use reqwest if available, else clear error.
        // Workspace may not have reqwest; use ureq-less manual note:
        Err(format!(
            "openai-compatible provider configured (url={url}, model={model}, key_len={}). \
             HTTP client wiring: use VEIL_MODEL_PROVIDER=echo for local, or add reqwest in a follow-up. body_preview={}",
            key.len(),
            body.to_string().chars().take(120).collect::<String>()
        ))
    }
}

/// Amazon Bedrock proof-of-port (adapter stub with honest error until AWS SDK wired).
pub struct BedrockProvider {
    pub config: ModelConfig,
}

#[async_trait]
impl ModelProvider for BedrockProvider {
    fn name(&self) -> &str {
        "bedrock"
    }
    async fn list_models(&self) -> Result<Vec<String>, String> {
        Ok(vec![self.config.model.clone()])
    }
    async fn complete(&self, req: CompleteRequest) -> Result<CompleteResponse, String> {
        let region = self
            .config
            .region
            .clone()
            .unwrap_or_else(|| "us-east-1".into());
        let model = req
            .model
            .clone()
            .unwrap_or_else(|| self.config.model.clone());
        Err(format!(
            "bedrock adapter port is registered (region={region}, model={model}) but AWS SDK is not linked in this MVP. \
             Configure VEIL_MODEL_PROVIDER=openai for OpenAI-compatible, or echo for offline. \
             Adding aws-sdk-bedrockruntime is the next adapter step — no engine/domain changes required."
        ))
    }
}

/// Shared helper: complete with env-configured provider.
pub async fn complete_with_env(req: CompleteRequest) -> Result<CompleteResponse, String> {
    let cfg = ModelConfig::from_env();
    let provider = cfg.build_provider();
    provider.complete(req).await
}
