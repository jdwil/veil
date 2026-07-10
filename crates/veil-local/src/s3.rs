//! S3-compatible object store (RT-015) — LocalStack / AWS via HTTP API.
//!
//! Uses the AWS SigV4-free path when `VEIL_S3_ENDPOINT` points at LocalStack
//! with path-style addressing. For production AWS, set real credentials and
//! region; this MVP uses a minimal REST client without the full AWS SDK so
//! the engine stays free of cloud hardcoding.

use crate::object::ObjectStorage;
use crate::FsObjectStore;
use crate::StorageError;

/// S3-compatible store. Fails closed with clear config errors when unset.
#[derive(Debug, Clone)]
pub struct S3ObjectStore {
    endpoint: String,
    bucket: String,
    region: String,
    /// When true, skip SigV4 (LocalStack / minio often allow anonymous or simple auth).
    path_style: bool,
}

impl S3ObjectStore {
    pub fn from_env() -> Result<Self, StorageError> {
        let endpoint = std::env::var("VEIL_S3_ENDPOINT").map_err(|_| {
            StorageError::Config(
                "VEIL_S3_ENDPOINT required for VEIL_STORAGE=s3 (e.g. http://127.0.0.1:4566 for LocalStack)"
                    .into(),
            )
        })?;
        let bucket = std::env::var("VEIL_S3_BUCKET").unwrap_or_else(|_| "veil".into());
        let region = std::env::var("VEIL_S3_REGION")
            .or_else(|_| std::env::var("AWS_REGION"))
            .unwrap_or_else(|_| "us-east-1".into());
        let path_style = std::env::var("VEIL_S3_PATH_STYLE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            bucket,
            region,
            path_style,
        })
    }

    fn object_url(&self, key: &str) -> String {
        if self.path_style {
            format!("{}/{}/{}", self.endpoint, self.bucket, key)
        } else {
            format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                self.bucket, self.region, key
            )
        }
    }

    fn http(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), StorageError> {
        // Minimal dependency-free HTTP via std + curl subprocess for MVP.
        // Prefer real AWS SDK adapters in a follow-up package.
        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-sS", "-w", "\n%{http_code}", "-X", method, url]);
        if let Some(b) = body {
            let tmp = std::env::temp_dir().join(format!(
                "veil-s3-{}.bin",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::write(&tmp, b).map_err(StorageError::Io)?;
            cmd.args(["--data-binary", &format!("@{}", tmp.display())]);
            cmd.arg("-H").arg("Content-Type: application/octet-stream");
            let output = cmd.output().map_err(|e| {
                StorageError::Http(format!(
                    "curl failed ({e}). Install curl or use VEIL_STORAGE=fs. LocalStack: {url}"
                ))
            })?;
            let _ = std::fs::remove_file(&tmp);
            return parse_curl_output(output);
        }
        let output = cmd.output().map_err(|e| {
            StorageError::Http(format!(
                "curl failed ({e}). Install curl or use VEIL_STORAGE=fs"
            ))
        })?;
        parse_curl_output(output)
    }
}

fn parse_curl_output(output: std::process::Output) -> Result<(u16, Vec<u8>), StorageError> {
    if !output.status.success() && output.stdout.is_empty() {
        return Err(StorageError::Http(String::from_utf8_lossy(&output.stderr).into()));
    }
    let mut stdout = output.stdout;
    // Last line is status code from -w
    while stdout.last() == Some(&b'\n') {
        stdout.pop();
    }
    let split = stdout
        .iter()
        .rposition(|&b| b == b'\n')
        .unwrap_or(stdout.len());
    let (body, code_bytes): (&[u8], &[u8]) = if split < stdout.len() {
        (&stdout[..split], &stdout[split + 1..])
    } else {
        (stdout.as_slice(), b"000".as_slice())
    };
    let code: u16 = std::str::from_utf8(code_bytes)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    Ok((code, body.to_vec()))
}

impl ObjectStorage for S3ObjectStore {
    fn backend_name(&self) -> &str {
        "s3"
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        if key.is_empty() || key.contains("..") {
            return Err(StorageError::InvalidKey(key.into()));
        }
        let url = self.object_url(key);
        let (code, body) = self.http("PUT", &url, Some(bytes))?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(StorageError::Http(format!(
                "S3 PUT {url} → HTTP {code}: {}",
                String::from_utf8_lossy(&body)
            )))
        }
    }

    fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let url = self.object_url(key);
        let (code, body) = self.http("GET", &url, None)?;
        match code {
            200 => Ok(body),
            404 => Err(StorageError::NotFound(key.into())),
            c => Err(StorageError::Http(format!(
                "S3 GET {url} → HTTP {c}: {}",
                String::from_utf8_lossy(&body)
            ))),
        }
    }

    fn delete(&self, key: &str) -> Result<(), StorageError> {
        let url = self.object_url(key);
        let (code, body) = self.http("DELETE", &url, None)?;
        if (200..300).contains(&code) || code == 404 {
            Ok(())
        } else {
            Err(StorageError::Http(format!(
                "S3 DELETE {url} → HTTP {code}: {}",
                String::from_utf8_lossy(&body)
            )))
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        // ListObjectsV2 path-style
        let url = format!(
            "{}/{}?list-type=2&prefix={}",
            self.endpoint,
            self.bucket,
            urlencoding_minimal(prefix)
        );
        let (code, body) = self.http("GET", &url, None)?;
        if code != 200 {
            return Err(StorageError::Http(format!(
                "S3 LIST → HTTP {code}: {} (is LocalStack running?)",
                String::from_utf8_lossy(&body)
            )));
        }
        // Naive key extract from XML
        let text = String::from_utf8_lossy(&body);
        let mut keys = Vec::new();
        for part in text.split("<Key>").skip(1) {
            if let Some(end) = part.find("</Key>") {
                keys.push(part[..end].to_string());
            }
        }
        Ok(keys)
    }
}

fn urlencoding_minimal(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

impl ObjectStorage for FsObjectStore {
    fn backend_name(&self) -> &str {
        "fs"
    }
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        FsObjectStore::put(self, key, bytes)
    }
    fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        FsObjectStore::get(self, key)
    }
    fn delete(&self, key: &str) -> Result<(), StorageError> {
        FsObjectStore::delete(self, key)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        FsObjectStore::list(self, prefix)
    }
}
