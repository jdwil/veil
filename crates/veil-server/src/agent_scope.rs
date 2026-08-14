//! Bind a product project for agent turns / MCP tools.
//!
//! Hub agent + ACP MCP are separate request contexts. Without this:
//! - `list_files` returns empty (MultiProjectProvider has no CURRENT_PROJECT)
//! - `write_source` fails "project scope missing"
//! - agent invents "empty project" stories for packages that exist on S3
//!
//! Call [`prepare_project`] at **turn start** (S3 rematerialize).
//! Mid-turn MCP (`write_source` after `create_project`) must use
//! [`ensure_bound`] — rematerialize + `reset_acp` on every tool call
//! deadlocks ACP and burns seconds per call.

use serde_json::{json, Value};

use crate::provider::SourceProvider;
use crate::session::{self, SessionHandle, SessionManager};

/// Normalize display names (`Agent Registry`) to slugs (`agent-registry`).
///
/// When the input is a **repo UUID**, prefer the product slug from DDB META so
/// sticky sessions / agent scope use one identity (not dual UUID+slug stickies).
pub fn normalize_slug(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s == "(none)" || s.starts_with("(none") {
        return None;
    }
    if let Ok(id) = crate::provider::s3_workspace::resolve_project_identity(s) {
        if id.slug.len() >= 2 {
            return Some(id.slug);
        }
    }
    let slug = crate::project_layout::slugify_name(s);
    if slug.len() < 2 {
        return None;
    }
    Some(slug)
}

/// Prepare coding session + ACP routing for `slug`.
///
/// - Rematerializes mainline from S3 (picks up merges)
/// - Rebuilds session provider from disk
/// - Sets ACP project so subsequent MCP calls scope correctly
/// - Optionally binds into MultiProjectProvider hub cache
pub fn prepare_project(
    slug_raw: &str,
    hub: Option<&dyn SourceProvider>,
) -> Result<Value, String> {
    let slug = normalize_slug(slug_raw)
        .ok_or_else(|| format!("invalid project slug: {slug_raw:?}"))?;
    let ident = crate::provider::s3_workspace::resolve_project_identity(&slug)
        .unwrap_or_else(|_| crate::provider::s3_workspace::ProjectIdentity {
            slug: slug.clone(),
            repo_id: slug.clone(),
        });
    // Prefer product slug for ACP + sticky (UUID route still works via dual sticky).
    let slug = ident.slug.clone();

    crate::acp::ensure_acp_project_scope(Some(slug.clone()));

    if !session::sessions_enabled() {
        return Ok(json!({
            "ok": true,
            "slug": slug,
            "repo_id": ident.repo_id,
            "sessions": false,
            "message": "Sessions disabled — ACP project scope set only",
        }));
    }

    let mgr = SessionManager::global();

    // Drop warm mainline handles so attach re-syncs S3 (merge promotions, etc.)
    if let Ok(list) = session::list_sessions_for_user(&session::current_user_id()) {
        for m in list
            .into_iter()
            .filter(|m| m.repo_id == ident.repo_id && !m.draft_mode)
        {
            mgr.drop_handle(&m.session_id);
        }
    }
    // Also clear active for slug + repo id aliases (will re-set)
    for key in [&slug, &ident.repo_id, slug_raw] {
        if let Some(sid) = mgr.active_for_project(key) {
            mgr.drop_handle(&sid);
        }
    }

    let h = mgr.resolve_for_project(&slug)?;
    // Extra pull + rebuild provider so in-memory source matches S3
    let sid = h.session_id();
    let _ = h.pull_remote();
    mgr.drop_handle(&sid);
    let h = mgr.attach(&sid).or_else(|_| mgr.resolve_for_project(&slug))?;
    mgr.set_active_for_project(&slug, &h.session_id());
    mgr.set_active_for_project(&ident.repo_id, &h.session_id());

    if let Some(hub) = hub {
        hub.bind_coding_session(&slug, h.provider.clone());
        // Also bind under repo UUID so middleware / CURRENT_PROJECT=uuid hits the same tree.
        if ident.repo_id != slug {
            hub.bind_coding_session(&ident.repo_id, h.provider.clone());
        }
    }

    let files = futures_list_files(&h);
    let active = files
        .iter()
        .find(|f| f.get("active").and_then(|a| a.as_bool()) == Some(true))
        .and_then(|f| f.get("name").and_then(|n| n.as_str()))
        .unwrap_or("")
        .to_string();

    // Update process focus so get_current_context isn't "none"
    crate::focus::set_focus(
        Some(&h.session_id()),
        json!({
            "route": format!("/projects/{slug}/ide"),
            "project": slug,
            "file": if active.is_empty() { Value::Null } else { json!(active) },
        }),
    );

    Ok(json!({
        "ok": true,
        "slug": slug,
        "session_id": h.session_id(),
        "codingSessionId": h.session_id(),
        "repo_id": h.snapshot_meta().repo_id,
        "draft_mode": h.snapshot_meta().draft_mode,
        "branch_name": h.snapshot_meta().branch_name,
        "files": files,
        "file_count": files.len(),
        "active_file": if active.is_empty() { Value::Null } else { json!(active) },
        "message": format!(
            "Bound project `{slug}` ({} files). Use write_source / read_source / veil_check — scope is set.",
            files.len()
        ),
    }))
}

/// Bind hub + ACP_PROJECT for `slug` **without** S3 rematerialize or ACP kill.
///
/// Use from hub `/api/mcp` on every unscoped coding tool. [`prepare_project`]
/// drops session handles and pulls S3 — fine once per turn, fatal mid-turn.
pub fn ensure_bound(
    slug_raw: &str,
    hub: Option<&dyn SourceProvider>,
) -> Result<Value, String> {
    let slug = normalize_slug(slug_raw)
        .ok_or_else(|| format!("invalid project slug: {slug_raw:?}"))?;
    let ident = crate::provider::s3_workspace::resolve_project_identity(&slug)
        .unwrap_or_else(|_| crate::provider::s3_workspace::ProjectIdentity {
            slug: slug.clone(),
            repo_id: slug.clone(),
        });
    let slug = ident.slug.clone();

    crate::acp::ensure_acp_project_scope(Some(slug.clone()));

    if hub.map(|h| h.has_coding_session(&slug)).unwrap_or(false) {
        return Ok(json!({
            "ok": true,
            "slug": slug,
            "repo_id": ident.repo_id,
            "hot": true,
            "message": format!("Project `{slug}` already bound — write_source/create_file now."),
        }));
    }

    if !session::sessions_enabled() {
        return Ok(json!({
            "ok": true,
            "slug": slug,
            "repo_id": ident.repo_id,
            "sessions": false,
            "hot": false,
            "message": "Sessions disabled — ACP project scope set only",
        }));
    }

    let mgr = SessionManager::global();
    let h = if let Some(sid) = mgr.active_for_project(&slug) {
        mgr.attach(&sid)
            .or_else(|_| mgr.resolve_for_project(&slug))?
    } else {
        mgr.resolve_for_project(&slug)?
    };
    mgr.set_active_for_project(&slug, &h.session_id());
    mgr.set_active_for_project(&ident.repo_id, &h.session_id());

    if let Some(hub) = hub {
        hub.bind_coding_session(&slug, h.provider.clone());
        if ident.repo_id != slug {
            hub.bind_coding_session(&ident.repo_id, h.provider.clone());
        }
    }

    Ok(json!({
        "ok": true,
        "slug": slug,
        "session_id": h.session_id(),
        "repo_id": h.snapshot_meta().repo_id,
        "hot": false,
        "message": format!(
            "Bound project `{slug}`. Write layers/*.layer, MISSION.md, main.veil via write_source — do not re-create."
        ),
    }))
}

fn futures_list_files(h: &SessionHandle) -> Vec<Value> {
    // Filesystem list from workdir (sync)
    let root = &h.work_dir;
    let mut out = Vec::new();
    let mut idx = 0usize;
    // packages at root
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".veil") {
                out.push(json!({
                    "index": idx,
                    "name": name,
                    "path": p.display().to_string(),
                    "kind": "package",
                    "active": idx == 0,
                }));
                idx += 1;
            }
        }
    }
    let layers = root.join("layers");
    if layers.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&layers) {
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".layer") {
                    out.push(json!({
                        "index": idx,
                        "name": name,
                        "path": p.display().to_string(),
                        "kind": "layer",
                        "active": false,
                    }));
                    idx += 1;
                }
            }
        }
    }
    out
}

/// Extract project slug from open_ide / create_project tool args or result JSON.
pub fn slug_from_tool(
    tool_name: &str,
    arguments: &Value,
    result_json: &str,
) -> Option<String> {
    // rename/update: `name` is the new display name — never treat it as the slug.
    if matches!(tool_name, "rename_project" | "update_project") {
        if let Ok(v) = serde_json::from_str::<Value>(result_json) {
            if let Some(s) = v
                .get("slug")
                .or_else(|| v.pointer("/project/slug"))
                .and_then(|x| x.as_str())
                .and_then(normalize_slug)
            {
                return Some(s);
            }
        }
        return arguments
            .get("project")
            .or_else(|| arguments.get("slug"))
            .or_else(|| arguments.get("id"))
            .and_then(|v| v.as_str())
            .and_then(normalize_slug);
    }
    let from_args = arguments
        .get("project")
        .or_else(|| arguments.get("slug"))
        .or_else(|| arguments.get("name"))
        .or_else(|| arguments.get("id"))
        .and_then(|v| v.as_str())
        .and_then(normalize_slug);
    if from_args.is_some() {
        return from_args;
    }
    if let Ok(v) = serde_json::from_str::<Value>(result_json) {
        if let Some(s) = v
            .get("slug")
            .or_else(|| v.get("project"))
            .or_else(|| v.pointer("/project/slug"))
            .or_else(|| v.pointer("/navigation/project"))
            .and_then(|x| x.as_str())
            .and_then(normalize_slug)
        {
            return Some(s);
        }
    }
    if matches!(
        tool_name,
        "open_ide"
            | "open_project"
            | "switch_project"
            | "create_project"
            | "create_repo"
            | "rename_project"
            | "update_project"
    ) {
        // nothing
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ensure_bound_rejects_empty_slug() {
        let err = ensure_bound(" ", None).unwrap_err();
        assert!(err.contains("invalid"), "{err}");
    }

    #[test]
    fn slug_from_create_project_result() {
        let result = json!({
            "ok": true,
            "slug": "dlx-bus",
            "name": "DLX Bus"
        })
        .to_string();
        assert_eq!(
            slug_from_tool("create_project", &json!({}), &result).as_deref(),
            Some("dlx-bus")
        );
    }
}
