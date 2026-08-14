//! PR Wizard integration: publish session → CR branch, bind active PR, write agent replies.

use serde_json::{json, Value};

use crate::session::SessionManager;

/// Push the coding-session branch to git origin and bind the PR id.
pub fn publish_session_for_change(
    slug: &str,
    branch: &str,
    change_id: &str,
) -> Result<Value, String> {
    let slug = slug.trim();
    let branch = branch.trim();
    let change_id = change_id.trim();
    if slug.is_empty() {
        return Err("slug required to publish session for PR".into());
    }
    if branch.is_empty() {
        return Err("branch required to publish session for PR".into());
    }
    if change_id.is_empty() {
        return Err("change_id required".into());
    }
    if !crate::session::sessions_enabled() {
        return Ok(json!({
            "ok": false,
            "skipped": true,
            "reason": "sessions disabled",
        }));
    }
    let h = SessionManager::global().resolve_for_project(slug)?;
    let pub_result = h.publish_to_branch(branch)?;
    h.set_active_pr_id(Some(change_id))?;
    Ok(json!({
        "ok": true,
        "publish": pub_result,
        "active_pr_id": change_id,
        "slug": slug,
        "branch": branch,
    }))
}

/// Post agent assistant text onto the open PR (history). Best-effort; never fails the turn.
pub async fn writeback_agent_turn_to_pr(
    slug: Option<&str>,
    assistant_text: &str,
    tool_names: &[String],
    source_changed: bool,
) {
    let text = assistant_text.trim();
    if text.is_empty() && tool_names.is_empty() {
        return;
    }
    // Prefer active_pr_id on the project session
    let change_id = slug
        .filter(|s| !s.is_empty())
        .and_then(|s| {
            SessionManager::global()
                .resolve_for_project(s)
                .ok()
                .and_then(|h| h.snapshot_meta().active_pr_id)
        })
        .or_else(|| {
            // Any open handle with active_pr_id
            SessionManager::global()
                .open_handles_summary()
                .into_iter()
                .find_map(|v| {
                    v.get("active_pr_id")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                })
        })
        .or_else(|| {
            // After create_project rebound the new session has no PR — scan DDB.
            crate::session::list_sessions_for_user(&crate::session::current_user_id())
                .ok()
                .and_then(|list| {
                    list.into_iter().find_map(|m| m.active_pr_id.filter(|s| !s.is_empty()))
                })
        });

    let Some(change_id) = change_id else {
        return;
    };

    let mut body = String::from("[pr-wizard:agent_reply]\n");
    if source_changed {
        body.push_str("source_changed: true\n");
    }
    if !tool_names.is_empty() {
        body.push_str(&format!("tools: {}\n", tool_names.join(", ")));
    }
    body.push('\n');
    // Cap body so comments stay readable
    const MAX: usize = 6000;
    if text.len() > MAX {
        body.push_str(&text[..MAX]);
        body.push_str("\n\n…(truncated)");
    } else {
        body.push_str(text);
    }

    let base = std::env::var("VEIL_RUNTIME_URL")
        .or_else(|_| std::env::var("VEIL_API_BASE"))
        .unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let url = format!(
        "{}/api/pull_requests/{}/comments",
        base.trim_end_matches('/'),
        change_id
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let payload = json!({
        "author": "agent",
        "body": body,
        "construct_path": null,
    });
    match client.post(&url).json(&payload).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(%change_id, "wrote agent reply to PR history");
        }
        Ok(resp) => {
            tracing::warn!(
                %change_id,
                status = %resp.status(),
                "PR history writeback failed"
            );
        }
        Err(e) => {
            tracing::warn!(%change_id, error = %e, "PR history writeback error");
        }
    }
}
