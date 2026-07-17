//! Extension persistence for VEIL-authored adapters.
//!
//! **Not** in the VEIL engine — referenced only via `veil_ext_store.stub`.
//! Backend selected by `VEIL_EXTENSIONS_BACKEND=file|ddb` (default `file`).

use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for StoreError {}
impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError(e.to_string())
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError(e.to_string())
    }
}

fn backend() -> String {
    std::env::var("VEIL_EXTENSIONS_BACKEND").unwrap_or_else(|_| "file".into())
}

fn use_aws() -> bool {
    matches!(backend().as_str(), "ddb" | "aws" | "s3")
}

/// Static facade matching `runtime/src/stubs/veil_ext_store.stub`.
pub struct ExtStore;

impl ExtStore {
    // ── Registry records (JSON documents) ────────────────────────────────

    pub fn put_record(root: String, id: String, json: String) -> Result<(), StoreError> {
        if use_aws() {
            return aws_put_item("meta", &id, &json);
        }
        let dir = PathBuf::from(&root);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{id}.json")), json.as_bytes())?;
        Ok(())
    }

    /// Empty string when missing (VEIL adapters treat `""` as null).
    pub fn get_record(root: String, id: String) -> Result<String, StoreError> {
        if use_aws() {
            return Ok(aws_get_item("meta", &id)?.unwrap_or_default());
        }
        let p = Path::new(&root).join(format!("{id}.json"));
        if !p.is_file() {
            return Ok(String::new());
        }
        Ok(std::fs::read_to_string(p)?)
    }

    pub fn list_records(root: String) -> Result<Vec<String>, StoreError> {
        if use_aws() {
            return aws_list_items("meta");
        }
        let dir = Path::new(&root);
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".json") {
                        out.push(std::fs::read_to_string(&p)?);
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn put_version(root: String, id: String, version: String, json: String) -> Result<(), StoreError> {
        if use_aws() {
            return aws_put_item(&format!("v#{version}"), &id, &json);
        }
        let dir = Path::new(&root).join(&id).join("versions");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{version}.json")), json.as_bytes())?;
        Ok(())
    }

    /// Empty string when missing.
    pub fn get_version(root: String, id: String, version: String) -> Result<String, StoreError> {
        if use_aws() {
            return Ok(aws_get_item(&format!("v#{version}"), &id)?.unwrap_or_default());
        }
        let p = Path::new(&root)
            .join(&id)
            .join("versions")
            .join(format!("{version}.json"));
        if !p.is_file() {
            return Ok(String::new());
        }
        Ok(std::fs::read_to_string(p)?)
    }

    pub fn list_versions(root: String, id: String) -> Result<Vec<String>, StoreError> {
        if use_aws() {
            return aws_list_versions(&id);
        }
        let dir = Path::new(&root).join(&id).join("versions");
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            if p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("json") {
                out.push(std::fs::read_to_string(p)?);
            }
        }
        Ok(out)
    }

    // ── Source tree ──────────────────────────────────────────────────────

    pub fn ensure_package(root: String, id: String) -> Result<String, StoreError> {
        if use_aws() {
            return Ok(format!("s3://extensions/src/{id}"));
        }
        let p = Path::new(&root).join("src").join(&id);
        std::fs::create_dir_all(&p)?;
        Ok(p.to_string_lossy().to_string())
    }

    pub fn write_source(root: String, id: String, rel: String, content: String) -> Result<(), StoreError> {
        if use_aws() {
            return aws_s3_put(&format!("src/{id}/{rel}"), content.as_bytes());
        }
        let p = Path::new(&root).join("src").join(&id).join(&rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, content.as_bytes())?;
        Ok(())
    }

    /// Empty string when missing.
    pub fn read_source(root: String, id: String, rel: String) -> Result<String, StoreError> {
        if use_aws() {
            return Ok(aws_s3_get(&format!("src/{id}/{rel}"))?.unwrap_or_default());
        }
        let p = Path::new(&root).join("src").join(&id).join(&rel);
        if !p.is_file() {
            return Ok(String::new());
        }
        Ok(std::fs::read_to_string(p)?)
    }

    pub fn list_source(root: String, id: String) -> Result<Vec<String>, StoreError> {
        if use_aws() {
            return aws_s3_list(&format!("src/{id}/"));
        }
        let dir = Path::new(&root).join("src").join(&id);
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            out.push(e.file_name().to_string_lossy().to_string());
        }
        out.sort();
        Ok(out)
    }

    pub fn package_root(root: String, id: String) -> String {
        if use_aws() {
            return format!("s3://extensions/src/{id}");
        }
        Path::new(&root)
            .join("src")
            .join(id)
            .to_string_lossy()
            .to_string()
    }

    // ── Artifacts ────────────────────────────────────────────────────────

    pub fn put_artifact(
        root: String,
        id: String,
        version: String,
        target: String,
        _data_b64_or_marker: String,
    ) -> Result<String, StoreError> {
        if use_aws() {
            let key = format!("artifacts/{id}/{version}/{target}");
            aws_s3_put(&key, b"artifact")?;
            return Ok(format!("s3://extensions/{key}"));
        }
        let p = Path::new(&root)
            .join("artifacts")
            .join(&id)
            .join(&version)
            .join(&target);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, b"artifact")?;
        Ok(p.to_string_lossy().to_string())
    }

    /// Empty string when missing.
    pub fn get_artifact_uri(
        root: String,
        id: String,
        version: String,
        target: String,
    ) -> Result<String, StoreError> {
        if use_aws() {
            let key = format!("artifacts/{id}/{version}/{target}");
            // Presence is assumed for AWS deploy; LocalStack contract can tighten later.
            return Ok(format!("s3://extensions/{key}"));
        }
        let p = Path::new(&root)
            .join("artifacts")
            .join(&id)
            .join(&version)
            .join(&target);
        if !p.is_file() {
            return Ok(String::new());
        }
        Ok(p.to_string_lossy().to_string())
    }
}

// ── AWS backends (feature-gated; file backend always available) ────────────

#[cfg(feature = "aws")]
fn table_name() -> String {
    std::env::var("EXTENSIONS_TABLE").unwrap_or_else(|_| "extensions".into())
}

#[cfg(feature = "aws")]
fn bucket_name() -> String {
    std::env::var("EXTENSIONS_BUCKET").unwrap_or_else(|_| "veil-extensions".into())
}

#[cfg(feature = "aws")]
fn runtime_block_on<F: std::future::Future>(f: F) -> F::Output {
    // Adapters are async_trait but ExtStore API is sync (matches LocalFs).
    // Use current handle if any, else a fresh runtime.
    match tokio::runtime::Handle::try_current() {
        Ok(h) => tokio::task::block_in_place(|| h.block_on(f)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt")
            .block_on(f),
    }
}

#[cfg(feature = "aws")]
async fn aws_config_load() -> aws_config::SdkConfig {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if let Ok(ep) = std::env::var("AWS_ENDPOINT_URL") {
        loader = loader.endpoint_url(ep);
    }
    loader.load().await
}

#[cfg(feature = "aws")]
fn aws_put_item(sk: &str, id: &str, json: &str) -> Result<(), StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_dynamodb::Client::new(&conf);
        client
            .put_item()
            .table_name(table_name())
            .item("id", aws_sdk_dynamodb::types::AttributeValue::S(id.into()))
            .item("sk", aws_sdk_dynamodb::types::AttributeValue::S(sk.into()))
            .item(
                "payload",
                aws_sdk_dynamodb::types::AttributeValue::S(json.into()),
            )
            .send()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    })
}

#[cfg(feature = "aws")]
fn aws_get_item(sk: &str, id: &str) -> Result<Option<String>, StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_dynamodb::Client::new(&conf);
        let out = client
            .get_item()
            .table_name(table_name())
            .key("id", aws_sdk_dynamodb::types::AttributeValue::S(id.into()))
            .key("sk", aws_sdk_dynamodb::types::AttributeValue::S(sk.into()))
            .send()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(out.item.and_then(|m| {
            m.get("payload")
                .and_then(|v| v.as_s().ok())
                .map(|s| s.to_string())
        }))
    })
}

#[cfg(feature = "aws")]
fn aws_list_items(sk_prefix: &str) -> Result<Vec<String>, StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_dynamodb::Client::new(&conf);
        // Scan + filter sk (fine for dual-loop / small tables)
        let out = client
            .scan()
            .table_name(table_name())
            .filter_expression("sk = :sk")
            .expression_attribute_values(
                ":sk",
                aws_sdk_dynamodb::types::AttributeValue::S(sk_prefix.into()),
            )
            .send()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        let mut rows = Vec::new();
        for item in out.items() {
            if let Some(p) = item.get("payload").and_then(|v| v.as_s().ok()) {
                rows.push(p.to_string());
            }
        }
        Ok(rows)
    })
}

#[cfg(feature = "aws")]
fn aws_list_versions(id: &str) -> Result<Vec<String>, StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_dynamodb::Client::new(&conf);
        let out = client
            .query()
            .table_name(table_name())
            .key_condition_expression("id = :id AND begins_with(sk, :p)")
            .expression_attribute_values(
                ":id",
                aws_sdk_dynamodb::types::AttributeValue::S(id.into()),
            )
            .expression_attribute_values(
                ":p",
                aws_sdk_dynamodb::types::AttributeValue::S("v#".into()),
            )
            .send()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        let mut rows = Vec::new();
        for item in out.items() {
            if let Some(p) = item.get("payload").and_then(|v| v.as_s().ok()) {
                rows.push(p.to_string());
            }
        }
        Ok(rows)
    })
}

#[cfg(feature = "aws")]
fn aws_s3_put(key: &str, bytes: &[u8]) -> Result<(), StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_s3::Client::new(&conf);
        client
            .put_object()
            .bucket(bucket_name())
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(bytes.to_vec()))
            .send()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    })
}

#[cfg(feature = "aws")]
fn aws_s3_get(key: &str) -> Result<Option<String>, StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_s3::Client::new(&conf);
        match client
            .get_object()
            .bucket(bucket_name())
            .key(key)
            .send()
            .await
        {
            Ok(out) => {
                let data = out
                    .body
                    .collect()
                    .await
                    .map_err(|e| StoreError(e.to_string()))?
                    .into_bytes();
                Ok(Some(String::from_utf8_lossy(&data).to_string()))
            }
            Err(_) => Ok(None),
        }
    })
}

#[cfg(feature = "aws")]
fn aws_s3_list(prefix: &str) -> Result<Vec<String>, StoreError> {
    runtime_block_on(async {
        let conf = aws_config_load().await;
        let client = aws_sdk_s3::Client::new(&conf);
        let out = client
            .list_objects_v2()
            .bucket(bucket_name())
            .prefix(prefix)
            .send()
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        let mut keys = Vec::new();
        for o in out.contents() {
            if let Some(k) = o.key() {
                // strip prefix for relative names
                let rel = k.strip_prefix(prefix).unwrap_or(k);
                if !rel.is_empty() {
                    keys.push(rel.to_string());
                }
            }
        }
        Ok(keys)
    })
}

#[cfg(not(feature = "aws"))]
fn aws_put_item(_sk: &str, _id: &str, _json: &str) -> Result<(), StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}
#[cfg(not(feature = "aws"))]
fn aws_get_item(_sk: &str, _id: &str) -> Result<Option<String>, StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}
#[cfg(not(feature = "aws"))]
fn aws_list_items(_sk: &str) -> Result<Vec<String>, StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}
#[cfg(not(feature = "aws"))]
fn aws_list_versions(_id: &str) -> Result<Vec<String>, StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}
#[cfg(not(feature = "aws"))]
fn aws_s3_put(_key: &str, _bytes: &[u8]) -> Result<(), StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}
#[cfg(not(feature = "aws"))]
fn aws_s3_get(_key: &str) -> Result<Option<String>, StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}
#[cfg(not(feature = "aws"))]
fn aws_s3_list(_prefix: &str) -> Result<Vec<String>, StoreError> {
    Err(StoreError(
        "AWS backend requested but veil_ext_store built without `aws` feature".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_record_roundtrip() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        std::env::set_var("VEIL_EXTENSIONS_BACKEND", "file");
        ExtStore::put_record(root.clone(), "id1".into(), r#"{"n":1}"#.into()).unwrap();
        let got = ExtStore::get_record(root.clone(), "id1".into()).unwrap();
        assert!(got.contains("\"n\":1"));
        let list = ExtStore::list_records(root.clone()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(ExtStore::get_record(root, "missing".into()).unwrap(), "");
    }
}
