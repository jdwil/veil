//! Optional Mind Palace (wiki) tools for Rig agents.
//!
//! Enabled when `MIND_PALACE=1` (or `true`) and AWS resources are configured.
//! Uses the default AWS credential chain (`AWS_PROFILE=dashlx_dev`, etc.).
//!
//! Env (required when enabled):
//! - `MIND_PALACE_S3_BUCKET`
//! - `MIND_PALACE_DYNAMO_TABLE`
//! - `MIND_PALACE_S3VECTORS_BUCKET`
//!
//! Optional:
//! - `MIND_PALACE_REGION` (default `us-east-1`)
//! - `MIND_PALACE_S3VECTORS_INDEX` (default `wiki-pages`)
//! - `MIND_PALACE_BEDROCK_MODEL` (default `amazon.titan-embed-text-v2:0`)
//! - `MIND_PALACE_S3_PREFIX` (default `v1`)

use std::sync::Arc;
use std::sync::OnceLock;

use mind_palace::{
    BedrockConfig, DynamoConfig, MindPalace, MindPalaceBuilder, S3Config, S3VectorsConfig,
};
use mind_palace::core::domain::tenant::TenantContext;
use mind_palace_rig::tools::{
    WikiCreateTool, WikiListTool, WikiReadTool, WikiSearchTool, WikiTraverseTool, WikiUpdateTool,
};

static PALACE: OnceLock<Option<Arc<MindPalace>>> = OnceLock::new();

fn env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

/// Whether Mind Palace tools should be attached (env gate only).
pub fn enabled() -> bool {
    env_truthy("MIND_PALACE") || env_truthy("VEIL_MIND_PALACE")
}

/// Extra preamble for agents when palace tools are available.
pub fn preamble_addon() -> &'static str {
    r#"

## Knowledge Base (Mind Palace)

You have wiki tools: wiki_search, wiki_read, wiki_traverse, wiki_create, wiki_update, wiki_list.
Before answering VEIL language / platform questions, search the wiki first.
After learning something durable (patterns, decisions, SOP), update or create a page.
Prefer progressive disclosure: summary → section → full.
"#
}

/// Lazy-init Mind Palace from env. Returns None if disabled or build fails.
pub async fn try_palace() -> Option<Arc<MindPalace>> {
    if !enabled() {
        return None;
    }
    static INIT: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);
    {
        let mut g = INIT.lock().await;
        if !*g {
            *g = true;
            match build_from_env().await {
                Ok(p) => {
                    let _ = PALACE.set(Some(Arc::new(p)));
                    tracing::info!("Mind Palace tools enabled (dashlx AWS credentials)");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Mind Palace init failed — tools unavailable");
                    let _ = PALACE.set(None);
                }
            }
        }
    }
    PALACE.get().and_then(|o| o.clone())
}

/// Strip accidental `s3://` / trailing slashes from bucket env values.
fn normalize_bucket(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix("s3://").unwrap_or(s);
    s.trim_end_matches('/').to_string()
}

async fn build_from_env() -> Result<MindPalace, String> {
    let region = std::env::var("MIND_PALACE_REGION").unwrap_or_else(|_| "us-east-1".into());
    let s3_bucket = normalize_bucket(
        &std::env::var("MIND_PALACE_S3_BUCKET")
            .map_err(|_| "MIND_PALACE_S3_BUCKET required when MIND_PALACE=1".to_string())?,
    );
    let table = std::env::var("MIND_PALACE_DYNAMO_TABLE")
        .map_err(|_| "MIND_PALACE_DYNAMO_TABLE required when MIND_PALACE=1".to_string())?;
    // Accept either MIND_PALACE_S3VECTORS_BUCKET or MIND_PALACE_VECTORS_BUCKET
    let vec_raw = std::env::var("MIND_PALACE_S3VECTORS_BUCKET")
        .or_else(|_| std::env::var("MIND_PALACE_VECTORS_BUCKET"))
        .map_err(|_| {
            "MIND_PALACE_S3VECTORS_BUCKET required when MIND_PALACE=1".to_string()
        })?;
    let vec_bucket = normalize_bucket(&vec_raw);
    let vec_index = std::env::var("MIND_PALACE_S3VECTORS_INDEX")
        .or_else(|_| std::env::var("MIND_PALACE_VECTORS_INDEX"))
        .unwrap_or_else(|_| "wiki-pages".into());
    let bedrock_model = std::env::var("MIND_PALACE_BEDROCK_MODEL")
        .unwrap_or_else(|_| "amazon.titan-embed-text-v2:0".into());
    let prefix = std::env::var("MIND_PALACE_S3_PREFIX").unwrap_or_else(|_| "v1".into());

    MindPalaceBuilder::new()
        .s3(S3Config {
            bucket_name: s3_bucket,
            region: region.clone(),
            prefix,
        })
        .dynamo(DynamoConfig {
            table_name: table,
            region: region.clone(),
        })
        .s3vectors(S3VectorsConfig {
            bucket_name: vec_bucket,
            index_name: vec_index,
            region: region.clone(),
        })
        .bedrock(BedrockConfig {
            model_id: bedrock_model,
            region,
        })
        .build()
        .await
        .map_err(|e| e.to_string())
}

/// Fresh tool instances for one Rig agent (clones Arc service handles).
pub fn tools_for_agent(palace: &MindPalace) -> (
    WikiSearchTool,
    WikiReadTool,
    WikiTraverseTool,
    WikiCreateTool,
    WikiUpdateTool,
    WikiListTool,
) {
    let t = palace.tools();
    let service = t.search.service.clone();
    let ctx = TenantContext::global();
    (
        WikiSearchTool {
            service: service.clone(),
            ctx: ctx.clone(),
        },
        WikiReadTool {
            service: service.clone(),
            ctx: ctx.clone(),
        },
        WikiTraverseTool {
            service: service.clone(),
            ctx: ctx.clone(),
        },
        WikiCreateTool {
            service: service.clone(),
            ctx: ctx.clone(),
        },
        WikiUpdateTool {
            service: service.clone(),
            ctx: ctx.clone(),
        },
        WikiListTool { service, ctx },
    )
}
