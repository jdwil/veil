//! Source resolver for deployed environments (DDB + S3).
//!
//! When `VEIL_DDB_TABLE` is set, the CLI can resolve layers and package source
//! from the runtime's project store instead of walking the filesystem.
//!
//! DDB schema (runtime table):
//! - Layers: PK=`LAYER#{name}`, SK=`META`, data=JSON with {name, repo_id, ...}
//!   Content: PK=`LAYER#{name}`, SK=`CONTENT`, data=layer file content
//! - Repos: PK=`REPO#{id}`, SK=`META`, data=JSON with {id, slug, ...}
//! - Package source: S3 at `repos/{repo_id}/{branch}/main.veil`
//!
//! Fallback: PK=`SOURCE#{slug}`, SK=`MAIN`, data=main.veil content (inline)

use crate::http;
use crate::StorageError;

/// Resolver that queries the runtime's DDB table and S3 bucket for source.
#[derive(Debug, Clone)]
pub struct SourceResolver {
    table: String,
    region: String,
    endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_endpoint: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    session_token: Option<String>,
}

impl SourceResolver {
    /// Build from environment.
    /// Requires `VEIL_DDB_TABLE`. Optional: `VEIL_DDB_ENDPOINT`, `BUCKET`/`VEIL_S3_BUCKET`.
    pub fn from_env() -> Option<Self> {
        let table = std::env::var("VEIL_DDB_TABLE").ok()?;
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("VEIL_DDB_REGION"))
            .unwrap_or_else(|_| "us-east-1".into());
        let endpoint = std::env::var("VEIL_DDB_ENDPOINT").ok();
        let s3_bucket = std::env::var("BUCKET")
            .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
            .ok();
        let s3_endpoint = std::env::var("VEIL_S3_ENDPOINT").ok();

        // Resolve credentials: direct env vars or from AWS_PROFILE
        let (access_key, secret_key, session_token) = resolve_aws_credentials();

        Some(Self {
            table,
            region,
            endpoint,
            s3_bucket,
            s3_endpoint,
            access_key,
            secret_key,
            session_token,
        })
    }

    /// Construct with explicit parameters (for testing).
    pub fn new(
        table: String,
        region: String,
        endpoint: Option<String>,
        s3_bucket: Option<String>,
        s3_endpoint: Option<String>,
    ) -> Self {
        let (access_key, secret_key, session_token) = resolve_aws_credentials();
        Self {
            table,
            region,
            endpoint,
            s3_bucket,
            s3_endpoint,
            access_key,
            secret_key,
            session_token,
        }
    }

    // ─── Layer resolution ──────────────────────────────────────────────────

    /// Resolve a layer by name. Tries:
    /// 1. `LAYER#{name}` SK=`CONTENT` (direct content storage)
    /// 2. `LAYER#{name}` SK=`META` → repo_id → S3 `repos/{repo_id}/main/{name}.layer`
    pub fn resolve_layer(&self, name: &str) -> Option<String> {
        // Try direct content first (fast path for seeded layers)
        if let Ok(content) = self.get_item_data(&format!("LAYER#{name}"), "CONTENT") {
            return Some(content);
        }
        // Fall back to metadata → S3
        if let Ok(meta_json) = self.get_item_data(&format!("LAYER#{name}"), "META") {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&meta_json) {
                if let Some(repo_id) = meta.get("repo_id")
                    .and_then(|v| v.get("value").and_then(|v| v.as_str())
                        .or_else(|| v.as_str()))
                {
                    let key = format!("repos/{repo_id}/main/{name}.layer");
                    if let Some(content) = self.get_s3_text(&key) {
                        return Some(content);
                    }
                    // Also try layers/{name}.layer
                    let key2 = format!("repos/{repo_id}/main/layers/{name}.layer");
                    if let Some(content) = self.get_s3_text(&key2) {
                        return Some(content);
                    }
                }
            }
        }
        None
    }

    // ─── Package source resolution ─────────────────────────────────────────

    /// Resolve a package's main.veil source by use-name (slug).
    /// Tries:
    /// 1. `SOURCE#{name}` SK=`MAIN` (direct inline content)
    /// 2. Scan repos for matching slug → S3 `repos/{repo_id}/main/main.veil`
    pub fn resolve_package_source(&self, use_name: &str) -> Option<String> {
        // Normalize use_name to slug form: dlx_auth → dlx-auth
        let slug = use_name.replace('_', "-");

        // Try direct source storage (fast path)
        if let Ok(content) = self.get_item_data(&format!("SOURCE#{slug}"), "MAIN") {
            return Some(content);
        }
        // Also try without normalization
        if slug != use_name {
            if let Ok(content) = self.get_item_data(&format!("SOURCE#{use_name}"), "MAIN") {
                return Some(content);
            }
        }

        // Look up repo by slug scan, then fetch from S3
        if let Some(repo_id) = self.find_repo_id_by_slug(&slug) {
            let key = format!("repos/{repo_id}/main/main.veil");
            if let Some(content) = self.get_s3_text(&key) {
                return Some(content);
            }
        }

        None
    }

    // ─── Stub resolution ───────────────────────────────────────────────────

    /// Resolve a .stub file by crate name.
    pub fn resolve_stub(&self, crate_name: &str) -> Option<String> {
        // Direct content: STUB#{crate_name} SK=CONTENT
        if let Ok(content) = self.get_item_data(&format!("STUB#{crate_name}"), "CONTENT") {
            return Some(content);
        }
        None
    }

    // ─── Seed helpers (for populating the store) ───────────────────────────

    /// Store a layer's content directly in DDB for fast resolution.
    pub fn put_layer_content(&self, name: &str, content: &str) -> Result<(), StorageError> {
        self.put_item_data(&format!("LAYER#{name}"), "CONTENT", content)
    }

    /// Store a package's main.veil source directly in DDB.
    pub fn put_package_source(&self, slug: &str, content: &str) -> Result<(), StorageError> {
        self.put_item_data(&format!("SOURCE#{slug}"), "MAIN", content)
    }

    /// Store a stub's content directly in DDB.
    pub fn put_stub_content(&self, crate_name: &str, content: &str) -> Result<(), StorageError> {
        self.put_item_data(&format!("STUB#{crate_name}"), "CONTENT", content)
    }

    // ─── Internal helpers ──────────────────────────────────────────────────

    fn ddb_endpoint(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| format!("https://dynamodb.{}.amazonaws.com", self.region))
    }

    fn ddb_call(&self, target: &str, body: &str) -> Result<(u16, String), StorageError> {
        let url = self.ddb_endpoint();
        let payload = body.as_bytes();
        let mut extra: Vec<(String, String)> = vec![
            ("Content-Type".into(), "application/x-amz-json-1.0".into()),
            ("X-Amz-Target".into(), target.into()),
        ];

        // SigV4 signing for real AWS (skip for LocalStack endpoints)
        let is_localhost = url.contains("localhost") || url.contains("127.0.0.1");
        if !is_localhost {
            if let (Some(ak), Some(sk)) = (&self.access_key, &self.secret_key) {
                let auth_headers = crate::s3::sigv4_headers_with_token(
                    "POST", &url, payload, ak, sk, &self.region, "dynamodb",
                    self.session_token.as_deref(),
                )
                .unwrap_or_default();
                extra.extend(auth_headers);
            }
        }

        let headers: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (code, bytes) = http::request("POST", &url, Some(payload), &headers)?;
        Ok((code, String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Get the `data` string field from a PK/SK item.
    fn get_item_data(&self, pk: &str, sk: &str) -> Result<String, StorageError> {
        let body = serde_json::json!({
            "TableName": &self.table,
            "Key": {
                "PK": { "S": pk },
                "SK": { "S": sk }
            }
        })
        .to_string();
        let (code, text) = self.ddb_call("DynamoDB_20120810.GetItem", &body)?;
        if !(200..300).contains(&code) {
            return Err(StorageError::Http(format!(
                "DDB GetItem → HTTP {code}: {text}"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| StorageError::Http(format!("DDB response JSON: {e}")))?;
        let item = v.get("Item")
            .ok_or_else(|| StorageError::NotFound(format!("{pk}/{sk}")))?;
        // Content is in the `data` field (String attribute)
        item.get("data")
            .and_then(|d| d.get("S"))
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| StorageError::NotFound(format!("{pk}/{sk} — no data.S")))
    }

    /// Put a `data` string field into a PK/SK item.
    fn put_item_data(&self, pk: &str, sk: &str, data: &str) -> Result<(), StorageError> {
        let body = serde_json::json!({
            "TableName": &self.table,
            "Item": {
                "PK": { "S": pk },
                "SK": { "S": sk },
                "data": { "S": data }
            }
        })
        .to_string();
        let (code, text) = self.ddb_call("DynamoDB_20120810.PutItem", &body)?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(StorageError::Http(format!(
                "DDB PutItem → HTTP {code}: {text}"
            )))
        }
    }

    /// Find a repo ID by scanning for a matching slug.
    fn find_repo_id_by_slug(&self, slug: &str) -> Option<String> {
        // Scan REPO# items and match slug in the data JSON.
        // In production this should use a GSI, but scan is fine for <100 repos.
        let body = serde_json::json!({
            "TableName": &self.table,
            "FilterExpression": "begins_with(PK, :prefix) AND SK = :sk",
            "ExpressionAttributeValues": {
                ":prefix": { "S": "REPO#" },
                ":sk": { "S": "META" }
            }
        })
        .to_string();
        let (code, text) = self.ddb_call("DynamoDB_20120810.Scan", &body).ok()?;
        if !(200..300).contains(&code) {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let items = v.get("Items")?.as_array()?;
        for item in items {
            let data_str = item.get("data")?.get("S")?.as_str()?;
            if let Ok(repo) = serde_json::from_str::<serde_json::Value>(data_str) {
                let repo_slug = repo.get("slug").and_then(|s| s.as_str()).unwrap_or("");
                if repo_slug == slug {
                    // Extract ID — could be {value: "..."} or plain string
                    if let Some(id_obj) = repo.get("id") {
                        if let Some(val) = id_obj.get("value").and_then(|v| v.as_str()) {
                            return Some(val.to_string());
                        }
                        if let Some(val) = id_obj.as_str() {
                            return Some(val.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    /// Fetch text content from S3.
    fn get_s3_text(&self, key: &str) -> Option<String> {
        let bucket = self.s3_bucket.as_ref()?;
        let default_endpoint = format!("https://s3.{}.amazonaws.com", self.region);
        let endpoint = self.s3_endpoint.as_deref().unwrap_or(&default_endpoint);
        let url = format!("{}/{}/{}", endpoint.trim_end_matches('/'), bucket, key);

        let mut extra: Vec<(String, String)> = Vec::new();
        let is_localhost = url.contains("localhost") || url.contains("127.0.0.1");
        if !is_localhost {
            if let (Some(ak), Some(sk)) = (&self.access_key, &self.secret_key) {
                let auth_headers = crate::s3::sigv4_headers_with_token(
                    "GET", &url, &[], ak, sk, &self.region, "s3",
                    self.session_token.as_deref(),
                )
                .unwrap_or_default();
                extra.extend(auth_headers);
            }
        }

        let headers: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (code, bytes) = http::request("GET", &url, None, &headers).ok()?;
        if code == 200 {
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        }
    }
}

/// Resolve AWS credentials from env vars or AWS_PROFILE.
/// Returns (access_key, secret_key, session_token).
fn resolve_aws_credentials() -> (Option<String>, Option<String>, Option<String>) {
    // Direct env vars first
    let ak = std::env::var("AWS_ACCESS_KEY_ID").ok();
    let sk = std::env::var("AWS_SECRET_ACCESS_KEY").ok();
    let token = std::env::var("AWS_SESSION_TOKEN").ok();

    if ak.is_some() && sk.is_some() {
        return (ak, sk, token);
    }

    // Fall back to AWS_PROFILE via `aws configure export-credentials`
    if std::env::var("AWS_PROFILE").is_ok() {
        if let Ok(output) = std::process::Command::new("aws")
            .args(["configure", "export-credentials", "--format", "env"])
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let mut ak = None;
                let mut sk = None;
                let mut token = None;
                for line in text.lines() {
                    if let Some(val) = line.strip_prefix("export AWS_ACCESS_KEY_ID=") {
                        ak = Some(val.to_string());
                    } else if let Some(val) = line.strip_prefix("export AWS_SECRET_ACCESS_KEY=") {
                        sk = Some(val.to_string());
                    } else if let Some(val) = line.strip_prefix("export AWS_SESSION_TOKEN=") {
                        token = Some(val.to_string());
                    }
                }
                if ak.is_some() && sk.is_some() {
                    return (ak, sk, token);
                }
            }
        }
    }

    (None, None, None)
}
