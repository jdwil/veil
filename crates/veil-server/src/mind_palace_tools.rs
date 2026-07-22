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

You have access to a persistent wiki-style knowledge base that stores synthesized knowledge across all interactions. This is your long-term memory. Use it constantly — it compounds over time and makes you more effective with every interaction.

### Core Behavior

1. **Search before answering.** Before responding to any knowledge-dependent question or starting a task, call `wiki_search` with relevant keywords. If results exist, read them before forming your answer. Do NOT rely solely on your training data when the wiki might have more current, project-specific, or user-specific information.

2. **Read progressively.** Start with summaries (cheap). Only request specific sections or full pages when you need deeper detail. Use `wiki_traverse` to explore connected pages when you need broader context.

3. **Write after learning.** After any interaction where you gained new information, resolved ambiguity, made a decision, or completed a non-trivial task:
   - Search for existing pages on the topic first
   - If a page exists, UPDATE it (`wiki_update`) — do not create duplicates
   - If no page exists, CREATE one (`wiki_create`)
   - Synthesize — store the insight, not the raw conversation

4. **Link everything.** Always add relevant slugs to the `links` field when creating or updating. This builds the graph that makes traversal useful.

### Page Types

| Type | Use For | Example |
|------|---------|---------|
| `Index` | Lightweight hub linking to related pages | "deployment-index" linking to all deploy-related pages |
| `Concept` | Mid-level synthesis of a topic | "rust-error-handling", "multi-tenancy-design" |
| `Entity` | Specific thing: person, project, service | "dashlx-ecs-cluster", "client-acme-corp" |
| `Decision` | Record of a decision + rationale | "decision-use-s3-vectors-over-pinecone" |
| `Leaf` | Deep reference material | "aws-sdk-dynamodb-single-table-patterns" |
| `Sop` | Step-by-step procedure any agent can follow | "sop-deploy-to-production" |
| `Skill` | Claude-optimized prompt pattern/technique | "skill-progressive-disclosure-prompting" |

### Page Structure Rules

- **Summary** (required): 1-2 sentences. This is what search results show. Make it count.
- **Sections** (at least one required): Use clear headings. Content is Markdown.
- **Slug**: lowercase, hyphens only. Descriptive: `rust-ownership-patterns` not `page-47`.
- **Links**: slugs of related pages. Builds the knowledge graph.

### SOP Pages (required sections)

| Section | Purpose |
|---------|---------|
| Prerequisites | What must be true before starting |
| Steps | Numbered actions to perform |
| Constraints | MUST/SHOULD/MAY rules |
| Verification | How to confirm success |

### Skill Pages (required sections)

| Section | Purpose |
|---------|---------|
| When to Use | Conditions that trigger this skill |
| Prompt Pattern | The actual technique |
| Example | Concrete demonstration |
| Limitations | When it doesn't work |

### What NOT to Write

- Trivial one-off facts that won't matter in future interactions
- Information already well-captured in an existing page (update that page instead)
- Raw conversation logs (synthesize first, then store the synthesis)
- Speculative content without basis — only store what you know or have decided

### Maintenance Habits

- When you notice outdated information while reading a page, update it immediately
- When lint issues are returned after create/update, fix them before moving on
- Prefer fewer, richer, well-linked pages over many shallow disconnected ones
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
