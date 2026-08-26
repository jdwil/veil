//! DynamoDB-backed Contribution Manifest Store.
//!
//! Stores contribution registrations in the DLX AI harness format:
//!   PK = "CONTRIBUTION#{app_id}#{contribution_id}"  SK = "META"
//!
//! This is separate from the ArtifactRecord model and optimized for the
//! `GET /api/contributions?app={app_id}` query pattern used by harness apps.
//!
//! Deploy pipeline flow:
//!   1. Build contribution bundle (Vite library mode)
//!   2. Upload to S3: `{contribution_id}/{version}/index.js`
//!   3. POST /api/contributions → persists here
//!   4. Harness fetches manifests on page load via GET ?app=
//!
//! Rollback: PATCH with previous version/bundle_url, or disable via `enabled: false`.

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── Types ───────────────────────────────────────────────────────────────────

/// A contribution manifest as stored in DDB and returned by the API.
/// Matches the DLX AI harness spec data model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContributionManifest {
    /// Unique contribution identifier (e.g. "agent-core").
    pub id: String,
    /// Target harness app (e.g. "dlx-ai").
    pub app_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Deployed version (timestamp-based, e.g. "20260826101500").
    pub version: String,
    /// CDN URL for the ES module bundle.
    pub bundle_url: String,
    /// Optional CDN URL for the CSS bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css_url: Option<String>,
    /// Whether this contribution is active (false = disabled/rolled back).
    pub enabled: bool,
    /// Sort order for merged menus.
    #[serde(default = "default_order")]
    pub order: u32,
    /// Slot manifest — maps slot names to their entries.
    /// e.g. { "main-menu": [...], "main-content": [...] }
    #[serde(default)]
    pub slots: serde_json::Value,
    /// When this contribution was first registered.
    pub registered_at: DateTime<Utc>,
    /// When this contribution was last updated.
    pub updated_at: DateTime<Utc>,
}

fn default_order() -> u32 {
    100
}

/// Body for `POST /api/contributions`.
#[derive(Debug, Deserialize)]
pub struct CreateContributionBody {
    pub app_id: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub bundle_url: String,
    #[serde(default)]
    pub css_url: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_order")]
    pub order: u32,
    #[serde(default)]
    pub slots: serde_json::Value,
}

fn default_enabled() -> bool {
    true
}

/// Body for `PATCH /api/contributions/{app_id}/{contribution_id}`.
/// All fields are optional — only provided fields are updated.
#[derive(Debug, Deserialize)]
pub struct PatchContributionBody {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub bundle_url: Option<String>,
    #[serde(default)]
    pub css_url: Option<Option<String>>,
    #[serde(default)]
    pub order: Option<u32>,
    #[serde(default)]
    pub slots: Option<serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
}

// ─── Store ───────────────────────────────────────────────────────────────────

/// DDB-backed store for contribution manifests.
#[derive(Clone)]
pub struct ContributionManifestStore {
    ddb: aws_sdk_dynamodb::Client,
    table: String,
}

impl ContributionManifestStore {
    pub fn new(ddb: aws_sdk_dynamodb::Client, table: String) -> Self {
        Self { ddb, table }
    }

    /// Create from standard env vars (VEIL_DDB_TABLE).
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let ddb = aws_sdk_dynamodb::Client::new(&config);
        let table =
            std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
        Self::new(ddb, table)
    }

    /// Compose the PK for a contribution.
    fn pk(app_id: &str, contribution_id: &str) -> String {
        format!("CONTRIBUTION#{app_id}#{contribution_id}")
    }

    /// Register or update a contribution manifest (upsert).
    pub async fn put(&self, manifest: &ContributionManifest) -> Result<(), String> {
        let pk = Self::pk(&manifest.app_id, &manifest.id);
        let data = serde_json::to_string(manifest)
            .map_err(|e| format!("serialize contribution: {e}"))?;

        self.ddb
            .put_item()
            .table_name(&self.table)
            .item("PK", AttributeValue::S(pk))
            .item("SK", AttributeValue::S("META".into()))
            .item("data", AttributeValue::S(data))
            // GSI-friendly: store app_id as a top-level attribute for potential future GSI.
            .item("app_id", AttributeValue::S(manifest.app_id.clone()))
            .send()
            .await
            .map_err(|e| format!("DDB put contribution: {e:?}"))?;

        Ok(())
    }

    /// Get a specific contribution manifest.
    pub async fn get(
        &self,
        app_id: &str,
        contribution_id: &str,
    ) -> Result<Option<ContributionManifest>, String> {
        let pk = Self::pk(app_id, contribution_id);
        let resp = self
            .ddb
            .get_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S("META".into()))
            .send()
            .await
            .map_err(|e| format!("DDB get contribution: {e:?}"))?;

        match resp.item() {
            Some(item) => {
                let data = item
                    .get("data")
                    .and_then(|v| v.as_s().ok())
                    .ok_or_else(|| "missing data field".to_string())?;
                let manifest: ContributionManifest = serde_json::from_str(data)
                    .map_err(|e| format!("deserialize contribution: {e}"))?;
                Ok(Some(manifest))
            }
            None => Ok(None),
        }
    }

    /// List all contributions for an app_id.
    /// Uses scan with filter on PK prefix (acceptable for Phase 1 scale).
    pub async fn list_for_app(
        &self,
        app_id: &str,
    ) -> Result<Vec<ContributionManifest>, String> {
        let prefix = format!("CONTRIBUTION#{app_id}#");
        let mut items = Vec::new();
        let mut exclusive_start_key = None;

        loop {
            let mut req = self
                .ddb
                .scan()
                .table_name(&self.table)
                .filter_expression(
                    "begins_with(PK, :prefix) AND SK = :sk",
                )
                .expression_attribute_values(":prefix", AttributeValue::S(prefix.clone()))
                .expression_attribute_values(":sk", AttributeValue::S("META".into()));

            if let Some(key) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(key));
            }

            let resp = req
                .send()
                .await
                .map_err(|e| format!("DDB scan contributions: {e:?}"))?;

            for item in resp.items() {
                if let Some(data) = item.get("data").and_then(|v| v.as_s().ok()) {
                    if let Ok(manifest) = serde_json::from_str::<ContributionManifest>(data) {
                        items.push(manifest);
                    }
                }
            }

            match resp.last_evaluated_key() {
                Some(key) if !key.is_empty() => exclusive_start_key = Some(key.clone()),
                _ => break,
            }
        }

        // Sort by order, then by name for stability.
        items.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
        Ok(items)
    }

    /// Delete a contribution manifest.
    pub async fn delete(
        &self,
        app_id: &str,
        contribution_id: &str,
    ) -> Result<(), String> {
        let pk = Self::pk(app_id, contribution_id);
        self.ddb
            .delete_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S("META".into()))
            .send()
            .await
            .map_err(|e| format!("DDB delete contribution: {e:?}"))?;
        Ok(())
    }
}
