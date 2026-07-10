//! S3-compatible object store (RT-015 / RT-025) — LocalStack / AWS via HTTP.
//!
//! Path-style by default (LocalStack). Optional AWS SigV4 when
//! `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` are set (RT-025).

use crate::http;
use crate::object::ObjectStorage;
use crate::FsObjectStore;
use crate::StorageError;

/// S3-compatible store. Fails closed with clear config errors when unset.
#[derive(Debug, Clone)]
pub struct S3ObjectStore {
    endpoint: String,
    bucket: String,
    region: String,
    /// When true, path-style URLs (`endpoint/bucket/key`).
    path_style: bool,
    access_key: Option<String>,
    secret_key: Option<String>,
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
        let access_key = std::env::var("AWS_ACCESS_KEY_ID")
            .or_else(|_| std::env::var("VEIL_S3_ACCESS_KEY"))
            .ok();
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
            .or_else(|_| std::env::var("VEIL_S3_SECRET_KEY"))
            .ok();
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            bucket,
            region,
            path_style,
            access_key,
            secret_key,
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

    fn auth_headers(
        &self,
        method: &str,
        url: &str,
        payload: &[u8],
    ) -> Result<Vec<(String, String)>, StorageError> {
        let (Some(ak), Some(sk)) = (&self.access_key, &self.secret_key) else {
            return Ok(vec![]);
        };
        // Minimal AWS SigV4 for S3 (RT-025). Enough for LocalStack + many AWS GETs/PUTs.
        sigv4_headers(method, url, payload, ak, sk, &self.region, "s3")
    }

    fn http(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>), StorageError> {
        let payload = body.unwrap_or(&[]);
        let auth = self.auth_headers(method, url, payload)?;
        let headers: Vec<(&str, &str)> = auth
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        http::request(method, url, body, &headers)
    }
}

/// AWS Signature Version 4 headers (minimal S3 subset).
fn sigv4_headers(
    method: &str,
    url: &str,
    payload: &[u8],
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
) -> Result<Vec<(String, String)>, StorageError> {
    use sha2::{Digest, Sha256};

    let parsed = reqwest::Url::parse(url).map_err(|e| StorageError::Http(format!("url: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| StorageError::Http("url missing host".into()))?
        .to_string();
    let path = if parsed.path().is_empty() {
        "/".to_string()
    } else {
        parsed.path().to_string()
    };
    let query = parsed.query().unwrap_or("");

    let amz_date = chrono_like_now();
    let date = amz_date[..8].to_string(); // YYYYMMDD

    let mut hasher = Sha256::new();
    hasher.update(payload);
    let payload_hash = hex::encode(hasher.finalize());

    let canonical_headers = format!(
        "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
    );
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let mut cr_hasher = Sha256::new();
    cr_hasher.update(canonical_request.as_bytes());
    let cr_hash = hex::encode(cr_hasher.finalize());

    let credential_scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{cr_hash}");

    let signing_key = aws_signing_key(secret_key, &date, region, service);
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    Ok(vec![
        ("Authorization".into(), auth),
        ("x-amz-date".into(), amz_date),
        ("x-amz-content-sha256".into(), payload_hash),
        ("host".into(), host),
    ])
}

fn aws_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    // HMAC-SHA256 without extra crate: pad key, inner/outer
    let mut k = key.to_vec();
    if k.len() > 64 {
        let mut h = Sha256::new();
        h.update(&k);
        k = h.finalize().to_vec();
    }
    k.resize(64, 0);
    let mut ipad = vec![0u8; 64];
    let mut opad = vec![0u8; 64];
    for i in 0..64 {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    outer.finalize().to_vec()
}

fn chrono_like_now() -> String {
    // UTC YYYYMMDDTHHMMSSZ via system time
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Approximate UTC breakdown without chrono dep
    let days = secs / 86400;
    let tod = secs % 86400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}{m:02}{d:02}T{hour:02}{min:02}{sec:02}Z")
}

/// Days since Unix epoch → (year, month, day) — Howard Hinnant algorithm.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_length() {
        let out = hmac_sha256(b"key", b"data");
        assert_eq!(out.len(), 32);
    }

    #[test]
    fn civil_epoch() {
        let (y, m, d) = civil_from_days(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }
}
