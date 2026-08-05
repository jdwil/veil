//! DynamoDB persistence for durable sessions (aws CLI, same style as s3_workspace).

use std::collections::HashMap;
use std::process::Command;

use serde::{Deserialize, Serialize};

fn table() -> String {
    std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into())
}

fn aws_base() -> Command {
    let mut c = Command::new("aws");
    if let Ok(p) = std::env::var("AWS_PROFILE") {
        c.env("AWS_PROFILE", p);
    }
    if let Ok(r) = std::env::var("AWS_REGION") {
        c.env("AWS_REGION", r);
    }
    c
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub user_id: String,
    pub slug: String,
    pub repo_id: String,
    pub branch: String,
    pub work_prefix: String,
    pub revision: u64,
    #[serde(default)]
    pub active_file: Option<String>,
    #[serde(default)]
    pub open_files: Vec<String>,
    #[serde(default)]
    pub etags: HashMap<String, String>,
    #[serde(default)]
    pub dirty: Vec<String>,
    #[serde(default)]
    pub draft_mode: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: String,
    #[serde(default)]
    pub agent_thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTurn {
    pub turn_id: String,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<serde_json::Value>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub active_file: Option<String>,
    pub ts: String,
    #[serde(default)]
    pub backend: Option<String>,
}

pub fn put_session_meta(meta: &SessionMeta) -> Result<(), String> {
    let pk = format!("SESSION#{}", meta.session_id);
    let data = serde_json::to_string(meta).map_err(|e| e.to_string())?;
    // Escape for shell via temp file item JSON
    let item = serde_json::json!({
        "PK": { "S": pk },
        "SK": { "S": "META" },
        "data": { "S": data },
        "GSI1PK": { "S": format!("USER#{}", meta.user_id) },
        "GSI1SK": { "S": format!("SESSION#{}", meta.updated_at) },
    });
    let tmp = std::env::temp_dir().join(format!("veil-sess-{}.json", meta.session_id));
    std::fs::write(&tmp, item.to_string()).map_err(|e| format!("write item: {e}"))?;
    let out = aws_base()
        .args([
            "dynamodb",
            "put-item",
            "--table-name",
            &table(),
            "--item",
            &format!("file://{}", tmp.display()),
        ])
        .output()
        .map_err(|e| format!("aws put-item: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(format!(
            "ddb put session: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

pub fn get_session_meta(session_id: &str) -> Result<SessionMeta, String> {
    let pk = format!("SESSION#{session_id}");
    let out = aws_base()
        .args([
            "dynamodb",
            "get-item",
            "--table-name",
            &table(),
            "--key",
            &format!(r#"{{"PK":{{"S":"{pk}"}},"SK":{{"S":"META"}}}}"#),
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws get-item: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ddb get session: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("json: {e}"))?;
    let data = v
        .pointer("/Item/data/S")
        .and_then(|s| s.as_str())
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    serde_json::from_str(data).map_err(|e| format!("session meta parse: {e}"))
}

pub fn touch_session(session_id: &str) -> Result<(), String> {
    let mut meta = get_session_meta(session_id)?;
    let now = super::chrono_now();
    meta.last_activity_at = now.clone();
    meta.updated_at = now;
    put_session_meta(&meta)
}

pub fn delete_session_meta(session_id: &str) -> Result<(), String> {
    let pk = format!("SESSION#{session_id}");
    let out = aws_base()
        .args([
            "dynamodb",
            "delete-item",
            "--table-name",
            &table(),
            "--key",
            &format!(r#"{{"PK":{{"S":"{pk}"}},"SK":{{"S":"META"}}}}"#),
        ])
        .output()
        .map_err(|e| format!("aws delete-item: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ddb delete session: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// List sessions for a user via scan filter (GSI optional; scan works on small tables).
pub fn list_sessions_for_user(user_id: &str) -> Result<Vec<SessionMeta>, String> {
    let out = aws_base()
        .args([
            "dynamodb",
            "scan",
            "--table-name",
            &table(),
            "--filter-expression",
            "begins_with(PK, :p) AND SK = :sk",
            "--expression-attribute-values",
            r#"{":p":{"S":"SESSION#"},":sk":{"S":"META"}}"#,
            "--projection-expression",
            "PK,SK,#d",
            "--expression-attribute-names",
            r##"{"#d":"data"}"##,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws scan: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ddb scan sessions: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("json: {e}"))?;
    let mut out_list = Vec::new();
    for item in v
        .get("Items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let data = item
            .pointer("/data/S")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if let Ok(m) = serde_json::from_str::<SessionMeta>(data) {
            if m.user_id == user_id {
                out_list.push(m);
            }
        }
    }
    out_list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out_list)
}

/// Prefer calling with RFC3339 `ts` from [`super::chrono_now`].
pub fn append_turn(session_id: &str, turn: &SessionTurn) -> Result<(), String> {
    let pk = format!("SESSION#{session_id}");
    let sk = format!("TURN#{}", turn.turn_id);
    let data = serde_json::to_string(turn).map_err(|e| e.to_string())?;
    let item = serde_json::json!({
        "PK": { "S": pk },
        "SK": { "S": sk },
        "data": { "S": data },
    });
    let tmp = std::env::temp_dir().join(format!("veil-turn-{}.json", turn.turn_id));
    std::fs::write(&tmp, item.to_string()).map_err(|e| format!("write: {e}"))?;
    let out = aws_base()
        .args([
            "dynamodb",
            "put-item",
            "--table-name",
            &table(),
            "--item",
            &format!("file://{}", tmp.display()),
        ])
        .output()
        .map_err(|e| format!("aws put turn: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(format!(
            "ddb put turn: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

pub fn list_turns(session_id: &str) -> Result<Vec<SessionTurn>, String> {
    let pk = format!("SESSION#{session_id}");
    let out = aws_base()
        .args([
            "dynamodb",
            "query",
            "--table-name",
            &table(),
            "--key-condition-expression",
            "PK = :pk AND begins_with(SK, :sk)",
            "--expression-attribute-values",
            &format!(r#"{{":pk":{{"S":"{pk}"}},":sk":{{"S":"TURN#"}}}}"#),
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws query turns: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ddb query turns: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("json: {e}"))?;
    let mut turns = Vec::new();
    for item in v
        .get("Items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default()
    {
        if let Some(data) = item.pointer("/data/S").and_then(|s| s.as_str()) {
            if let Ok(t) = serde_json::from_str::<SessionTurn>(data) {
                turns.push(t);
            }
        }
    }
    turns.sort_by(|a, b| a.turn_id.cmp(&b.turn_id));
    Ok(turns)
}
