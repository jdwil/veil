//! Host coding policy gates (backend-agnostic).
//!
//! Enforced around shared tools (`session_commit`, `create_pr` / open PR,
//! `submit_pr`) so ACP and future Bedrock share the same rules. Soft SOP
//! in `agent_context` remains documentation; this module fails closed where
//! agreed and always surfaces **host** check state (not agent self-report).
//!
//! Product language: **Pull Request (PR)** only — not "Change Request".
//! Tools/API: `create_pr`, `/api/pull_requests`, `active_pr_id`. CR reserved.

use serde_json::{json, Value};

use crate::session::{HostCheckSnapshot, SessionHandle, SessionMeta};

/// Parse structured check text from `run_check` / write_source post-check.
///
/// Accepts the human line + JSON body format, or a bare JSON object.
pub fn parse_check_output(text: &str) -> HostCheckSnapshot {
    let now = chrono_now();
    let json_slice = extract_json_object(text).unwrap_or(text);
    if let Ok(v) = serde_json::from_str::<Value>(json_slice) {
        let error_count = v
            .get("error_count")
            .and_then(|x| x.as_u64())
            .or_else(|| {
                v.get("errors")
                    .and_then(|x| x.as_array())
                    .map(|a| a.len() as u64)
            })
            .unwrap_or(0) as u32;
        let warning_count = v
            .get("warning_count")
            .and_then(|x| x.as_u64())
            .or_else(|| {
                v.get("warnings")
                    .and_then(|x| x.as_array())
                    .map(|a| a.len() as u64)
            })
            .unwrap_or(0) as u32;
        let severity = if error_count > 0 {
            "errors"
        } else if warning_count > 0 {
            "warnings"
        } else if v.get("ok").and_then(|x| x.as_bool()) == Some(false) {
            "errors"
        } else {
            "ok"
        };
        let summary = v
            .get("summary")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                format!("host check: {severity} (errors={error_count} warnings={warning_count})")
            });
        return HostCheckSnapshot {
            severity: severity.into(),
            error_count,
            warning_count,
            summary,
            updated_at: now,
        };
    }
    // Fallback: keyword scan of agent-facing check blob
    let lower = text.to_lowercase();
    let (severity, error_count, warning_count) =
        if lower.contains("\"severity\":\"error\"")
            || lower.contains("\"severity\": \"error\"")
            || (lower.contains("error_count")
                && !lower.contains("\"error_count\":0")
                && !lower.contains("\"error_count\": 0"))
        {
            ("errors", 1, 0)
        } else if lower.contains("warning") {
            ("warnings", 0, 1)
        } else {
            ("ok", 0, 0)
        };
    HostCheckSnapshot {
        severity: severity.into(),
        error_count,
        warning_count,
        summary: text.lines().next().unwrap_or("host check").to_string(),
        updated_at: now,
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end > start {
        Some(&text[start..=end])
    } else {
        None
    }
}

fn chrono_now() -> String {
    // Match session timestamps (ISO-ish via system time)
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// JSON block for tool responses so models cannot ignore host truth.
pub fn host_check_value(meta: &SessionMeta) -> Value {
    match &meta.last_host_check {
        Some(h) => json!({
            "severity": h.severity,
            "error_count": h.error_count,
            "warning_count": h.warning_count,
            "summary": h.summary,
            "updated_at": h.updated_at,
            "source": "host",
        }),
        None => json!({
            "severity": "unknown",
            "error_count": 0,
            "warning_count": 0,
            "summary": "no host check recorded yet — run veil_check or write_source",
            "source": "host",
        }),
    }
}

pub fn has_host_errors(meta: &SessionMeta) -> bool {
    meta.last_host_check
        .as_ref()
        .map(|h| h.severity == "errors" || h.error_count > 0)
        .unwrap_or(false)
}

/// Pre-flight for `session_commit`. Empty tree is a hard reject.
pub fn gate_session_commit(h: &SessionHandle) -> Result<(), String> {
    if !h.has_uncommitted() {
        return Err(
            "GATE: nothing to commit — working tree clean. \
             After a successful write slice call session_commit once; \
             skip on pure Q&A / explore turns."
                .into(),
        );
    }
    Ok(())
}

/// Soft gate notes for opening a PR (`create_pr`).
/// Does not block; returns warnings the caller must include in the tool result.
pub fn gate_open_pr_notes(h: Option<&SessionHandle>, commit_count: usize) -> Vec<String> {
    let mut notes = Vec::new();
    match h {
        None => {
            notes.push(
                "WARN: no coding session bound — PR may lack published branch work. \
                 Prefer create_branch / session_status first."
                    .into(),
            );
        }
        Some(handle) => {
            let meta = handle.snapshot_meta();
            if commit_count == 0 && !handle.has_uncommitted() {
                notes.push(
                    "WARN: no session commits and clean tree — PR may be empty. \
                     Prefer session_commit after each write slice before open PR."
                        .into(),
                );
            } else if commit_count == 0 && handle.has_uncommitted() {
                notes.push(
                    "WARN: uncommitted dirty files — call session_commit before open PR so \
                     the PR Wizard has named slices."
                        .into(),
                );
            }
            if has_host_errors(&meta) {
                notes.push(format!(
                    "MUST_ACKNOWLEDGE_ERRORS: host check still has errors ({}). \
                     Do not claim a clean working set. Fix or disclose before human review.",
                    meta.last_host_check
                        .as_ref()
                        .map(|c| c.summary.as_str())
                        .unwrap_or("errors")
                ));
            }
            if let Some(id) = meta.active_pr_id.as_ref().filter(|s| !s.is_empty()) {
                notes.push(format!(
                    "HINT: session already bound to open PR id `{id}` — prefer reuse \
                     (update/submit that PR) when scope matches; avoid a second PR."
                ));
            }
        }
    }
    notes
}

/// When `VEIL_STRICT_SUBMIT=1` (or true/yes), submit_pr hard-refuses if host has errors.
pub fn strict_submit_enabled() -> bool {
    match std::env::var("VEIL_STRICT_SUBMIT") {
        Ok(v) => {
            let l = v.trim().to_lowercase();
            l == "1" || l == "true" || l == "yes" || l == "on"
        }
        Err(_) => false,
    }
}

/// Soft gate for `submit_pr` / submit PR.
/// Returns notes; always attach `host_check` in the tool JSON.
pub fn gate_submit_pr_notes(h: Option<&SessionHandle>) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(handle) = h else {
        notes.push("WARN: no coding session for host_check attachment.".into());
        return notes;
    };
    let meta = handle.snapshot_meta();
    if handle.has_uncommitted() {
        notes.push(
            "WARN: uncommitted work on session — consider session_commit before submit \
             so the published PR tree matches intent."
                .into(),
        );
    }
    if has_host_errors(&meta) {
        if strict_submit_enabled() {
            notes.push(format!(
                "STRICT_SUBMIT: host Errors (error_count={}) — submit will be rejected until fixed.",
                meta.last_host_check
                    .as_ref()
                    .map(|c| c.error_count)
                    .unwrap_or(0)
            ));
        } else {
            notes.push(format!(
                "MUST_ACKNOWLEDGE_ERRORS: host working set still has Errors \
                 (error_count={}). Submit may proceed for human review, but the agent \
                 must not claim 0 errors / clean veil_check. Set VEIL_STRICT_SUBMIT=1 to hard-block.",
                meta.last_host_check
                    .as_ref()
                    .map(|c| c.error_count)
                    .unwrap_or(0)
            ));
        }
    } else if meta
        .last_host_check
        .as_ref()
        .map(|c| c.severity == "warnings")
        .unwrap_or(false)
    {
        notes.push(
            "NOTE: host check has warnings remaining — disclose in PR description if relevant."
                .into(),
        );
    }
    notes
}

/// Prefer recording check on the project session when one exists.
pub fn record_host_check_for_project(slug: Option<&str>, check_text: &str) {
    let Some(slug) = slug.filter(|s| !s.is_empty()) else {
        return;
    };
    if !crate::session::sessions_enabled() {
        return;
    }
    let snap = parse_check_output(check_text);
    if let Ok(h) = crate::session::SessionManager::global().resolve_for_project(slug) {
        h.set_last_host_check(snap);
    }
}

/// Resolve project session if sessions enabled.
pub fn project_session(slug: Option<&str>) -> Option<std::sync::Arc<SessionHandle>> {
    let slug = slug.filter(|s| !s.is_empty())?;
    if !crate::session::sessions_enabled() {
        return None;
    }
    crate::session::SessionManager::global()
        .resolve_for_project(slug)
        .ok()
}

/// Current project slug from hub / ACP.
pub fn current_project_slug() -> Option<String> {
    crate::provider::hub::CURRENT_PROJECT
        .try_with(|n| n.clone())
        .ok()
        .or_else(crate::acp::get_acp_project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_errors() {
        let text = r#"package 2 errors
{"ok":false,"error_count":2,"warning_count":1,"summary":"2 errors"}"#;
        let s = parse_check_output(text);
        assert_eq!(s.severity, "errors");
        assert_eq!(s.error_count, 2);
        assert_eq!(s.warning_count, 1);
    }

    #[test]
    fn parse_json_ok() {
        let text = r#"{"ok":true,"error_count":0,"warning_count":0}"#;
        let s = parse_check_output(text);
        assert_eq!(s.severity, "ok");
        assert_eq!(s.error_count, 0);
    }

    #[test]
    fn parse_json_warnings_only() {
        let text = r#"{"ok":true,"error_count":0,"warning_count":3,"summary":"3 warnings"}"#;
        let s = parse_check_output(text);
        assert_eq!(s.severity, "warnings");
        assert_eq!(s.warning_count, 3);
    }

    #[test]
    fn host_check_value_unknown_when_none() {
        let meta = SessionMeta {
            session_id: "s".into(),
            user_id: "u".into(),
            slug: "p".into(),
            repo_id: "r".into(),
            branch: "main".into(),
            work_prefix: "".into(),
            revision: 0,
            active_file: None,
            open_files: vec![],
            etags: Default::default(),
            dirty: vec![],
            draft_mode: false,
            branch_name: None,
            base_branch: None,
            head_commit: None,
            committed_revision: None,
            created_at: "".into(),
            updated_at: "".into(),
            last_activity_at: "".into(),
            agent_thread_id: None,
            last_focus: None,
            intent_log: vec![],
            active_pr_id: None,
            writes_since_commit: 0,
            last_host_check: None,
            rationales: Default::default(),
        };
        let v = host_check_value(&meta);
        assert_eq!(v["severity"], "unknown");
        assert_eq!(v["source"], "host");
    }

    #[test]
    fn open_pr_notes_clean_empty() {
        // No handle → warn about session
        let notes = gate_open_pr_notes(None, 0);
        assert!(notes.iter().any(|n| n.contains("no coding session")));
    }
}
