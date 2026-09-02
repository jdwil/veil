//! Durable capture of full-fidelity inner-agent tool calls (audit-logging Part 1).
//!
//! The ACP tunnel ([`crate::acp`]) streams `tool_call` / `tool_call_update`
//! session updates that we coalesce into [`crate::acp::AcpToolRecord`]s
//! (name + arguments + result + status + order). This module turns those into
//! the JSON stored in `SessionTurn.tool_calls` (DDB `SESSION#/TURN#`), with two
//! audit guarantees:
//!
//! 1. **Secret redaction** — arguments and results run through the same
//!    [`crate::git_origin::redact_secrets`] used for git origins (strips
//!    `://user:secret@` userinfo + known tokens) before persistence.
//! 2. **Large-result offload** — any single tool result over
//!    [`INLINE_LIMIT`] bytes is written to an S3 blob
//!    (`s3://{bucket}/session-blobs/{sid}/{turn}/{n}.json`) and referenced by
//!    `content_ref` on the stored record, keeping DDB items small.
//!
//! Capture is best-effort: an S3 failure falls back to a truncated inline
//! result (never drops the record) and logs a warning.

use serde_json::{json, Value};

use crate::acp::AcpToolRecord;
use crate::agent::AgentToolCall;

/// Inline any tool result at or under this many bytes; offload larger ones to S3.
const INLINE_LIMIT: usize = 8 * 1024;
/// When S3 offload fails, keep at most this many bytes inline as a fallback.
const FALLBACK_TRUNCATE: usize = 4 * 1024;

/// Redact secrets from a captured string (args/results/prompt/assistant text).
pub fn redact(s: &str) -> String {
    crate::git_origin::redact_secrets(s)
}

/// Redact secrets recursively from a JSON value (tool arguments / output).
fn redact_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact(s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_value).collect()),
        Value::Object(o) => {
            let mut m = serde_json::Map::with_capacity(o.len());
            for (k, val) in o {
                m.insert(k.clone(), redact_value(val));
            }
            Value::Object(m)
        }
        other => other.clone(),
    }
}

/// Build the durable `SessionTurn.tool_calls` JSON from full-fidelity ACP
/// records. Falls back to name-only hints when no structured records exist
/// (heuristic / offline backends), so a turn is never left with placeholders.
pub fn durable_tool_calls(
    session_id: &str,
    turn_id: &str,
    records: &[AcpToolRecord],
    hints: &[AgentToolCall],
) -> Vec<Value> {
    if records.is_empty() {
        // No structured ACP records — capture whatever name hints we have so
        // the turn still lists the tools it used (better than an empty vec).
        return hints
            .iter()
            .enumerate()
            .filter(|(_, t)| t.name != "acp_session")
            .map(|(i, t)| {
                json!({
                    "name": t.name,
                    "order": i,
                    "detail": t.detail,
                    "fidelity": "name_only",
                })
            })
            .collect();
    }
    let mut out = Vec::with_capacity(records.len());
    for rec in records {
        let input = rec.input.as_ref().map(redact_value);
        let output = rec.output.as_ref().map(redact_value);
        let content = redact(&rec.content);
        let mut obj = json!({
            "name": rec.name,
            "tool_call_id": rec.tool_call_id,
            "kind": rec.kind,
            "status": rec.status,
            "order": rec.order,
            "started_at": rec.started_at,
            "input": input,
            "output": output,
            "fidelity": "full",
        });
        // Offload large results to S3; inline small ones.
        if content.len() > INLINE_LIMIT {
            let key = format!(
                "session-blobs/{session_id}/{turn_id}/{}.txt",
                rec.order
            );
            match put_blob(&key, content.as_bytes()) {
                Ok(uri) => {
                    obj["content_ref"] = json!(uri);
                    obj["content_bytes"] = json!(content.len());
                    // Keep a short preview inline for quick scanning.
                    let preview: String = content.chars().take(500).collect();
                    obj["content_preview"] = json!(preview);
                }
                Err(e) => {
                    tracing::warn!(error = %e, key, "tool-result S3 offload failed; truncating inline");
                    let trunc: String = content.chars().take(FALLBACK_TRUNCATE).collect();
                    obj["content"] = json!(trunc);
                    obj["content_truncated"] = json!(true);
                    obj["content_bytes"] = json!(content.len());
                }
            }
        } else if !content.is_empty() {
            obj["content"] = json!(content);
        }
        out.push(obj);
    }
    out
}

fn bucket() -> String {
    std::env::var("BUCKET")
        .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
        .unwrap_or_else(|_| "veil-runtime-dev".into())
}

/// Write a capture blob to S3 via the aws CLI (same transport as session
/// snapshots). Returns the `s3://…` URI on success.
fn put_blob(key: &str, bytes: &[u8]) -> Result<String, String> {
    let bucket = bucket();
    let tmp = std::env::temp_dir().join(format!(
        "veil-capture-{}.txt",
        key.replace(['/', '\\'], "_")
    ));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write tmp: {e}"))?;
    let dest = format!("s3://{bucket}/{key}");
    let mut cmd = std::process::Command::new("aws");
    if let Ok(p) = std::env::var("AWS_PROFILE") {
        cmd.env("AWS_PROFILE", p);
    }
    if let Ok(r) = std::env::var("AWS_REGION") {
        cmd.env("AWS_REGION", r);
    }
    let out = cmd
        .args([
            "s3",
            "cp",
            &tmp.to_string_lossy(),
            &dest,
            "--content-type",
            "text/plain",
        ])
        .output()
        .map_err(|e| format!("aws s3 cp: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(format!(
            "aws s3 cp: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(dest)
}

/// Read a capture blob back from S3 (used by the query/History surface to
/// expand an offloaded tool result). Returns the blob text.
pub fn get_blob(uri: &str) -> Result<String, String> {
    if !uri.starts_with("s3://") {
        return Err("not an s3 uri".into());
    }
    let tmp = std::env::temp_dir().join(format!(
        "veil-capture-read-{}.txt",
        uri.replace(['/', ':'], "_")
    ));
    let mut cmd = std::process::Command::new("aws");
    if let Ok(p) = std::env::var("AWS_PROFILE") {
        cmd.env("AWS_PROFILE", p);
    }
    if let Ok(r) = std::env::var("AWS_REGION") {
        cmd.env("AWS_REGION", r);
    }
    let out = cmd
        .args(["s3", "cp", uri, &tmp.to_string_lossy()])
        .output()
        .map_err(|e| format!("aws s3 cp read: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 cp read: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let body = std::fs::read_to_string(&tmp).map_err(|e| format!("read tmp: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, input: Value, content: &str, order: usize) -> AcpToolRecord {
        AcpToolRecord {
            tool_call_id: format!("tc_{order}"),
            name: name.into(),
            kind: Some("execute".into()),
            status: Some("completed".into()),
            input: Some(input),
            output: None,
            content: content.into(),
            order,
            started_at: "2026-09-02T12:00:00.000Z".into(),
        }
    }

    #[test]
    fn durable_tool_calls_capture_name_args_result_in_order() {
        let records = vec![
            rec("write_source", json!({"path": "a.veil"}), "wrote ok", 0),
            rec("veil_check", json!({}), "0 errors", 1),
        ];
        let out = durable_tool_calls("s1", "a_1", &records, &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["name"], "write_source");
        assert_eq!(out[0]["input"]["path"], "a.veil");
        assert_eq!(out[0]["content"], "wrote ok");
        assert_eq!(out[0]["order"], 0);
        assert_eq!(out[0]["fidelity"], "full");
        assert_eq!(out[1]["name"], "veil_check");
        assert_eq!(out[1]["order"], 1);
    }

    #[test]
    fn secrets_redacted_in_args_and_result() {
        let records = vec![rec(
            "clone_repo",
            json!({"url": "https://user:supersecrettoken@example.com/r.git"}),
            "cloned https://user:anothersecret@example.com/r.git",
            0,
        )];
        let out = durable_tool_calls("s1", "a_1", &records, &[]);
        let url = out[0]["input"]["url"].as_str().unwrap();
        assert!(url.contains(":***@"), "arg url should be redacted: {url}");
        assert!(!url.contains("supersecrettoken"));
        let content = out[0]["content"].as_str().unwrap();
        assert!(content.contains(":***@"), "result should be redacted: {content}");
        assert!(!content.contains("anothersecret"));
    }

    #[test]
    fn name_only_fallback_when_no_records() {
        let hints = vec![
            AgentToolCall { name: "write_source".into(), detail: "acp".into() },
            AgentToolCall { name: "acp_session".into(), detail: "sid".into() },
        ];
        let out = durable_tool_calls("s1", "a_1", &[], &hints);
        // acp_session placeholder filtered out.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["name"], "write_source");
        assert_eq!(out[0]["fidelity"], "name_only");
    }
}
