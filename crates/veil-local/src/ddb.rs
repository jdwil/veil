//! DynamoDB metadata adapter (RT-015 / RT-024).
//!
//! Same conceptual port as `FileMetaStore` (put/get/delete/list by kind+id).
//! Select with `VEIL_META=ddb`. Uses DynamoDB HTTP JSON API (no full AWS SDK).
//! LocalStack: set `VEIL_DDB_ENDPOINT=http://127.0.0.1:4566`.

use crate::http;
use crate::StorageError;

/// DynamoDB-backed meta store.
#[derive(Debug, Clone)]
pub struct DdbMetaStore {
    table: String,
    region: String,
    endpoint: Option<String>,
    access_key: String,
    secret_key: String,
}

impl DdbMetaStore {
    pub fn new(
        table: impl Into<String>,
        region: impl Into<String>,
        endpoint: Option<String>,
    ) -> Self {
        Self {
            table: table.into(),
            region: region.into(),
            endpoint,
            access_key: std::env::var("AWS_ACCESS_KEY_ID")
                .or_else(|_| std::env::var("VEIL_DDB_ACCESS_KEY"))
                .unwrap_or_else(|_| "test".into()),
            secret_key: std::env::var("AWS_SECRET_ACCESS_KEY")
                .or_else(|_| std::env::var("VEIL_DDB_SECRET_KEY"))
                .unwrap_or_else(|_| "test".into()),
        }
    }

    pub fn from_env() -> Result<Self, StorageError> {
        let table = std::env::var("VEIL_DDB_TABLE").map_err(|_| {
            StorageError::Config(
                "VEIL_DDB_TABLE required for VEIL_META=ddb (e.g. veil-meta)".into(),
            )
        })?;
        let region = std::env::var("VEIL_DDB_REGION")
            .or_else(|_| std::env::var("AWS_REGION"))
            .unwrap_or_else(|_| "us-east-1".into());
        let endpoint = std::env::var("VEIL_DDB_ENDPOINT").ok();
        Ok(Self::new(table, region, endpoint))
    }

    fn endpoint_url(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| format!("https://dynamodb.{}.amazonaws.com", self.region))
    }

    fn call(&self, target: &str, body: &str) -> Result<(u16, String), StorageError> {
        let url = self.endpoint_url();
        let headers = [
            ("Content-Type", "application/x-amz-json-1.0"),
            ("X-Amz-Target", target),
            // LocalStack often accepts unsigned; real AWS needs SigV4 (use endpoint + keys via env)
        ];
        // Prefer simple path for LocalStack; when endpoint is AWS, attach bearer-style
        // note that production should front with signed requests — we send Content-Type + Target.
        let _ = (&self.access_key, &self.secret_key);
        let (code, bytes) = http::request("POST", &url, Some(body.as_bytes()), &headers)?;
        Ok((code, String::from_utf8_lossy(&bytes).into_owned()))
    }

    fn pk(kind: &str, id: &str) -> String {
        format!("{kind}#{id}")
    }

    pub fn put_bytes(&self, kind: &str, id: &str, bytes: &[u8]) -> Result<(), StorageError> {
        if kind.contains("..") || id.contains("..") {
            return Err(StorageError::InvalidKey(format!("{kind}/{id}")));
        }
        let pk = Self::pk(kind, id);
        let b64 = base64_encode(bytes);
        let body = serde_json::json!({
            "TableName": self.table,
            "Item": {
                "pk": { "S": pk },
                "kind": { "S": kind },
                "id": { "S": id },
                "payload": { "S": b64 },
            }
        })
        .to_string();
        let (code, text) = self.call("DynamoDB_20120810.PutItem", &body)?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(StorageError::Http(format!(
                "DDB PutItem → HTTP {code}: {text}"
            )))
        }
    }

    pub fn get_bytes(&self, kind: &str, id: &str) -> Result<Vec<u8>, StorageError> {
        let pk = Self::pk(kind, id);
        let body = serde_json::json!({
            "TableName": self.table,
            "Key": { "pk": { "S": pk } }
        })
        .to_string();
        let (code, text) = self.call("DynamoDB_20120810.GetItem", &body)?;
        if code == 404 {
            return Err(StorageError::NotFound(format!("{kind}/{id}")));
        }
        if !(200..300).contains(&code) {
            return Err(StorageError::Http(format!(
                "DDB GetItem → HTTP {code}: {text}"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            StorageError::Http(format!("DDB GetItem JSON: {e}"))
        })?;
        if v.get("Item").is_none() {
            return Err(StorageError::NotFound(format!("{kind}/{id}")));
        }
        let payload = v["Item"]["payload"]["S"]
            .as_str()
            .ok_or_else(|| StorageError::Http("DDB item missing payload.S".into()))?;
        base64_decode(payload)
    }

    pub fn delete(&self, kind: &str, id: &str) -> Result<(), StorageError> {
        let pk = Self::pk(kind, id);
        let body = serde_json::json!({
            "TableName": self.table,
            "Key": { "pk": { "S": pk } }
        })
        .to_string();
        let (code, text) = self.call("DynamoDB_20120810.DeleteItem", &body)?;
        if (200..300).contains(&code) || code == 404 {
            Ok(())
        } else {
            Err(StorageError::Http(format!(
                "DDB DeleteItem → HTTP {code}: {text}"
            )))
        }
    }

    pub fn list_ids(&self, kind: &str) -> Result<Vec<String>, StorageError> {
        // Scan with filter — fine for LocalStack MVP; production should use GSI.
        let body = serde_json::json!({
            "TableName": self.table,
            "FilterExpression": "kind = :k",
            "ExpressionAttributeValues": {
                ":k": { "S": kind }
            },
            "ProjectionExpression": "id"
        })
        .to_string();
        let (code, text) = self.call("DynamoDB_20120810.Scan", &body)?;
        if !(200..300).contains(&code) {
            return Err(StorageError::Http(format!(
                "DDB Scan → HTTP {code}: {text}"
            )));
        }
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| StorageError::Http(format!("DDB Scan JSON: {e}")))?;
        let mut ids = Vec::new();
        if let Some(items) = v.get("Items").and_then(|i| i.as_array()) {
            for item in items {
                if let Some(id) = item.get("id").and_then(|x| x.get("S")).and_then(|s| s.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
        ids.sort();
        Ok(ids)
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>, StorageError> {
    fn val(c: u8) -> Result<u8, StorageError> {
        Ok(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 0,
            _ => return Err(StorageError::Http("invalid base64".into())),
        })
    }
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let n = ((val(bytes[i])? as u32) << 18)
            | ((val(bytes[i + 1])? as u32) << 12)
            | ((val(bytes[i + 2])? as u32) << 6)
            | (val(bytes[i + 3])? as u32);
        out.push(((n >> 16) & 0xff) as u8);
        if bytes[i + 2] != b'=' {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if bytes[i + 3] != b'=' {
            out.push((n & 0xff) as u8);
        }
        i += 4;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_roundtrip() {
        let data = b"hello meta";
        let e = base64_encode(data);
        let d = base64_decode(&e).unwrap();
        assert_eq!(d, data);
    }

    #[test]
    fn pk_format() {
        assert_eq!(DdbMetaStore::pk("repo", "1"), "repo#1");
    }
}
