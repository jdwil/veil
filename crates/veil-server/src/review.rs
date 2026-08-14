//! Outstanding change sets + recorded human sign-off.
//!
//! This is **review state**, not a second VCS. Git remains history
//! (commit / branch / merge / log / diff). Items point at slugs, paths,
//! and optional git SHAs so a human can sign off without reconstructing
//! the turn from `git blame`.
//!
//! Durable file: `VEIL_REVIEW_STORE` or `{veil_home}/review-state.json`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_ITEMS: usize = 400;
const MAX_AUDITS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    ProjectCreated,
    ProjectRenamed,
    FileEdit,
    FileCreated,
    Commit,
    PullRequest,
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Outstanding,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutstandingItem {
    pub id: String,
    #[serde(default)]
    pub repo_id: Option<String>,
    pub slug: String,
    #[serde(default)]
    pub project_name: Option<String>,
    pub kind: ItemKind,
    #[serde(default)]
    pub path: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub created_at: String,
    pub status: ItemStatus,
    #[serde(default)]
    pub decided_at: Option<String>,
    #[serde(default)]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub decision_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignOffRecord {
    pub id: String,
    pub at: String,
    pub actor: String,
    pub decision: String,
    pub item_ids: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReviewState {
    items: Vec<OutstandingItem>,
    audits: Vec<SignOffRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoReviewSummary {
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    pub outstanding: usize,
    pub needs_sign_off: bool,
    pub touched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_touched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecordSpec {
    pub kind: ItemKind,
    pub slug: Option<String>,
    pub repo_id: Option<String>,
    pub project_name: Option<String>,
    pub path: Option<String>,
    pub summary: String,
    pub rationale: Option<String>,
    pub git_sha: Option<String>,
    pub session_id: Option<String>,
}

fn store() -> &'static Mutex<ReviewState> {
    static S: OnceLock<Mutex<ReviewState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(load_from_disk()))
}

fn store_path() -> PathBuf {
    if let Ok(p) = std::env::var("VEIL_REVIEW_STORE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::config::veil_home_dir().join("review-state.json")
}

fn now_rfc3339() -> String {
    // Sortable millisecond epoch. Avoid chrono (not a veil-server dep).
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms}")
}

fn short_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("rv_{ms:x}_{}", (ms % 997) as u16)
}

fn load_from_disk() -> ReviewState {
    let path = store_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return ReviewState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn persist(state: &ReviewState) {
    let path = store_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(state) {
        let _ = atomic_write(&path, body.as_bytes());
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn infer_slug(explicit: Option<String>) -> String {
    if let Some(s) = explicit.filter(|s| !s.trim().is_empty()) {
        return crate::project_layout::slugify_name(&s);
    }
    crate::coding_gates::current_project_slug()
        .or_else(|| {
            crate::provider::hub::CURRENT_PROJECT
                .try_with(|n| n.clone())
                .ok()
        })
        .or_else(crate::acp::get_acp_project)
        .unwrap_or_else(|| "unknown".into())
}

fn infer_session(explicit: Option<String>) -> Option<String> {
    if let Some(s) = explicit.filter(|s| !s.is_empty()) {
        return Some(s);
    }
    crate::session::CURRENT_SESSION
        .try_with(|s| s.clone())
        .ok()
}

fn infer_repo_id(_slug: &str, explicit: Option<String>) -> Option<String> {
    // Do not probe DDB/S3 here — list_s3_projects already taught us that
    // per-row AWS CLI lookups hang the product surface.
    explicit.filter(|s| !s.is_empty())
}

/// Record an unreviewed mutation. Coalesces rapid edits to the same path.
pub fn record(spec: RecordSpec) -> OutstandingItem {
    let slug = infer_slug(spec.slug);
    let session_id = infer_session(spec.session_id);
    let repo_id = infer_repo_id(&slug, spec.repo_id);
    let now = now_rfc3339();
    let mut item = OutstandingItem {
        id: short_id(),
        repo_id,
        slug: slug.clone(),
        project_name: spec.project_name,
        kind: spec.kind,
        path: spec.path.clone(),
        summary: spec.summary,
        rationale: spec.rationale,
        git_sha: spec.git_sha,
        session_id,
        created_at: now,
        status: ItemStatus::Outstanding,
        decided_at: None,
        decided_by: None,
        decision_note: None,
    };

    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    // Coalesce: same slug+path+kind still outstanding → refresh in place.
    if matches!(spec.kind, ItemKind::FileEdit | ItemKind::FileCreated) {
        if let Some(existing) = guard.items.iter_mut().rev().find(|it| {
            it.status == ItemStatus::Outstanding
                && it.slug == slug
                && it.kind == spec.kind
                && it.path == spec.path
        }) {
            existing.summary = item.summary.clone();
            if item.rationale.is_some() {
                existing.rationale = item.rationale.clone();
            }
            if item.git_sha.is_some() {
                existing.git_sha = item.git_sha.clone();
            }
            existing.created_at = item.created_at.clone();
            item = existing.clone();
            persist(&guard);
            return item;
        }
    }
    guard.items.push(item.clone());
    if guard.items.len() > MAX_ITEMS {
        let drain = guard.items.len() - MAX_ITEMS;
        guard.items.drain(0..drain);
    }
    persist(&guard);
    item
}

/// Convenience constructors used by tools.
pub fn record_project_created(slug: &str, name: Option<&str>, repo_id: Option<&str>) -> OutstandingItem {
    record(RecordSpec {
        kind: ItemKind::ProjectCreated,
        slug: Some(slug.into()),
        repo_id: repo_id.map(str::to_string),
        project_name: name.map(str::to_string),
        path: None,
        summary: format!("Created project {}", name.unwrap_or(slug)),
        rationale: None,
        git_sha: None,
        session_id: None,
    })
}

pub fn record_project_renamed(slug: &str, name: &str) -> OutstandingItem {
    record(RecordSpec {
        kind: ItemKind::ProjectRenamed,
        slug: Some(slug.into()),
        repo_id: None,
        project_name: Some(name.into()),
        path: None,
        summary: format!("Renamed project to {name}"),
        rationale: None,
        git_sha: None,
        session_id: None,
    })
}

pub fn record_file_edit(slug: &str, path: &str, rationale: Option<&str>) -> OutstandingItem {
    record(RecordSpec {
        kind: ItemKind::FileEdit,
        slug: Some(slug.into()),
        repo_id: None,
        project_name: None,
        path: Some(path.into()),
        summary: format!("Edited {path}"),
        rationale: rationale.map(str::to_string),
        git_sha: None,
        session_id: None,
    })
}

pub fn record_file_created(slug: &str, path: &str) -> OutstandingItem {
    record(RecordSpec {
        kind: ItemKind::FileCreated,
        slug: Some(slug.into()),
        repo_id: None,
        project_name: None,
        path: Some(path.into()),
        summary: format!("Created {path}"),
        rationale: None,
        git_sha: None,
        session_id: None,
    })
}

pub fn record_commit(slug: &str, sha: &str, message: &str) -> OutstandingItem {
    // Stamp recent outstanding file items with this SHA.
    {
        let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
        for it in guard.items.iter_mut().rev() {
            if it.status == ItemStatus::Outstanding
                && it.slug == slug
                && it.git_sha.is_none()
                && matches!(it.kind, ItemKind::FileEdit | ItemKind::FileCreated)
            {
                it.git_sha = Some(sha.to_string());
            }
        }
        persist(&guard);
    }
    record(RecordSpec {
        kind: ItemKind::Commit,
        slug: Some(slug.into()),
        repo_id: None,
        project_name: None,
        path: None,
        summary: format!("Committed {}", message.lines().next().unwrap_or(message)),
        rationale: Some(message.into()),
        git_sha: Some(sha.into()),
        session_id: None,
    })
}

pub fn record_pr(slug: &str, title: &str, pr_id: Option<&str>) -> OutstandingItem {
    record(RecordSpec {
        kind: ItemKind::PullRequest,
        slug: Some(slug.into()),
        repo_id: None,
        project_name: None,
        path: None,
        summary: match pr_id {
            Some(id) => format!("Opened PR {id}: {title}"),
            None => format!("Opened PR: {title}"),
        },
        rationale: None,
        git_sha: None,
        session_id: None,
    })
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub slug: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<ItemStatus>,
}

pub fn list_items(filter: ListFilter) -> Vec<OutstandingItem> {
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .items
        .iter()
        .filter(|it| {
            if let Some(ref s) = filter.slug {
                if !it.slug.eq_ignore_ascii_case(s) && it.repo_id.as_deref() != Some(s.as_str()) {
                    return false;
                }
            }
            if let Some(ref sid) = filter.session_id {
                if it.session_id.as_deref() != Some(sid.as_str()) {
                    return false;
                }
            }
            if let Some(st) = filter.status {
                if it.status != st {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

pub fn outstanding() -> Vec<OutstandingItem> {
    list_items(ListFilter {
        status: Some(ItemStatus::Outstanding),
        ..Default::default()
    })
}

pub fn summary_by_slug() -> HashMap<String, RepoReviewSummary> {
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let mut map: HashMap<String, RepoReviewSummary> = HashMap::new();
    for it in &guard.items {
        let entry = map.entry(it.slug.clone()).or_insert_with(|| RepoReviewSummary {
            slug: it.slug.clone(),
            repo_id: it.repo_id.clone(),
            outstanding: 0,
            needs_sign_off: false,
            touched: true,
            last_touched_at: Some(it.created_at.clone()),
            last_kind: Some(kind_label(it.kind).into()),
        });
        if it.repo_id.is_some() && entry.repo_id.is_none() {
            entry.repo_id = it.repo_id.clone();
        }
        entry.touched = true;
        if it.status == ItemStatus::Outstanding {
            entry.outstanding += 1;
            entry.needs_sign_off = true;
        }
        if entry
            .last_touched_at
            .as_deref()
            .map(|t| t < it.created_at.as_str())
            .unwrap_or(true)
        {
            entry.last_touched_at = Some(it.created_at.clone());
            entry.last_kind = Some(kind_label(it.kind).into());
        }
    }
    map
}

fn kind_label(k: ItemKind) -> &'static str {
    match k {
        ItemKind::ProjectCreated => "created",
        ItemKind::ProjectRenamed => "renamed",
        ItemKind::FileEdit => "edited",
        ItemKind::FileCreated => "new file",
        ItemKind::Commit => "committed",
        ItemKind::PullRequest => "PR",
        ItemKind::Generated => "generated",
    }
}

#[derive(Debug, Clone)]
pub struct SignOffRequest {
    pub ids: Vec<String>,
    pub slug: Option<String>,
    pub all: bool,
    pub decision: String,
    pub actor: String,
    pub note: Option<String>,
}

pub fn sign_off(req: SignOffRequest) -> Result<(Vec<OutstandingItem>, SignOffRecord), String> {
    let decision = req.decision.trim().to_ascii_lowercase();
    let status = match decision.as_str() {
        "approve" | "approved" | "yes" => ItemStatus::Approved,
        "reject" | "rejected" | "no" => ItemStatus::Rejected,
        other => {
            return Err(format!(
                "decision must be approve or reject, got `{other}`"
            ))
        }
    };
    let actor = if req.actor.trim().is_empty() {
        "human".into()
    } else {
        req.actor.trim().to_string()
    };
    let now = now_rfc3339();
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let id_set: std::collections::HashSet<String> = req.ids.iter().cloned().collect();
    let mut chosen = Vec::new();
    for it in guard.items.iter_mut() {
        if it.status != ItemStatus::Outstanding {
            continue;
        }
        let match_id = !id_set.is_empty() && id_set.contains(&it.id);
        let match_slug = req
            .slug
            .as_ref()
            .map(|s| it.slug.eq_ignore_ascii_case(s) || it.repo_id.as_deref() == Some(s.as_str()))
            .unwrap_or(false);
        let match_all = req.all && req.slug.is_none() && id_set.is_empty();
        if !(match_id || match_slug || match_all) {
            continue;
        }
        it.status = status;
        it.decided_at = Some(now.clone());
        it.decided_by = Some(actor.clone());
        it.decision_note = req.note.clone();
        chosen.push(it.clone());
    }
    if chosen.is_empty() {
        return Err("no outstanding items matched".into());
    }
    let audit = SignOffRecord {
        id: format!("so_{}", short_id()),
        at: now,
        actor,
        decision: if status == ItemStatus::Approved {
            "approve".into()
        } else {
            "reject".into()
        },
        item_ids: chosen.iter().map(|i| i.id.clone()).collect(),
        note: req.note,
        slug: req.slug,
    };
    guard.audits.push(audit.clone());
    if guard.audits.len() > MAX_AUDITS {
        let drain = guard.audits.len() - MAX_AUDITS;
        guard.audits.drain(0..drain);
    }
    persist(&guard);
    Ok((chosen, audit))
}

pub fn audits(limit: usize) -> Vec<SignOffRecord> {
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard.audits.iter().rev().take(limit.max(1)).cloned().collect()
}

/// Markdown block for agent preamble (droppable; keep short).
pub fn preamble_block() -> String {
    let items = outstanding();
    if items.is_empty() {
        return String::new();
    }
    let mut by: HashMap<String, usize> = HashMap::new();
    for it in &items {
        *by.entry(it.slug.clone()).or_insert(0) += 1;
    }
    let mut lines = vec![
        "# Outstanding changes (needs human sign-off)".to_string(),
        format!(
            "{} unreviewed item(s) across {} project(s). After a coherent unit of work call `request_sign_off` and present what/why. Do not ask the human to dig through git.",
            items.len(),
            by.len()
        ),
    ];
    let mut slugs: Vec<_> = by.into_iter().collect();
    slugs.sort_by(|a, b| b.1.cmp(&a.1));
    for (slug, n) in slugs.into_iter().take(8) {
        lines.push(format!("- `{slug}`: {n} outstanding"));
    }
    lines.join("\n")
}

pub fn snapshot_json(filter: ListFilter) -> Value {
    let items = list_items(filter);
    let summaries: Vec<RepoReviewSummary> = {
        let map = summary_by_slug();
        let mut v: Vec<_> = map.into_values().collect();
        v.sort_by(|a, b| b.outstanding.cmp(&a.outstanding));
        v
    };
    json!({
        "ok": true,
        "outstanding": items.iter().filter(|i| i.status == ItemStatus::Outstanding).count(),
        "items": items,
        "by_project": summaries,
        "audits": audits(12),
    })
}

pub fn write_source_intent(slug: &str, path: &str) -> Value {
    let ide = format!("/projects/{}/ide", crate::project_layout::slugify_name(slug));
    json!({
        "type": "WriteSource",
        "id": format!("intent_write_{}", short_id()),
        "actor": "agent",
        "payload": { "project": slug, "path": path },
        "domain": { "mode": "server", "done": true },
        "navigation": { "action": "open-ide", "path": ide, "project": slug },
        "present": {
            "announce": format!("Editing {path} in {slug}"),
            "steps": [
                { "kind": "announce", "message": format!("Editing {path}") },
                { "kind": "goto", "path": ide, "ms": 320, "project": slug },
                { "kind": "wait", "ms": 220 },
                { "kind": "highlight", "selector": "[data-veil-shell], .graph-wrapper, .ide-app", "ms": 480 }
            ]
        }
    })
}

pub fn request_sign_off_intent(slug: Option<&str>, count: usize) -> Value {
    let path = match slug {
        Some(s) if !s.is_empty() => format!("/review/{}", crate::project_layout::slugify_name(s)),
        _ => "/review".to_string(),
    };
    json!({
        "type": "RequestSignOff",
        "id": format!("intent_signoff_{}", short_id()),
        "actor": "agent",
        "payload": { "slug": slug, "count": count },
        "domain": { "mode": "none" },
        "navigation": { "action": "goto", "path": path },
        "present": {
            "announce": if count == 1 {
                "Here is what I did — please sign off".to_string()
            } else {
                format!("Here is exactly what I did ({count} changes) — please sign off")
            },
            "steps": [
                { "kind": "goto", "path": path, "ms": 300 },
                { "kind": "wait", "ms": 200 },
                { "kind": "pulse", "target": "text:Sign off", "ms": 550 }
            ]
        }
    })
}

pub fn sign_off_intent(slug: Option<&str>, decision: &str) -> Value {
    let path = match slug {
        Some(s) if !s.is_empty() => format!("/review/{}", crate::project_layout::slugify_name(s)),
        _ => "/review".to_string(),
    };
    let label = if decision.eq_ignore_ascii_case("reject") {
        "Reject"
    } else {
        "Sign off"
    };
    json!({
        "type": "SignOff",
        "id": format!("intent_do_signoff_{}", short_id()),
        "actor": "agent",
        "payload": { "slug": slug, "decision": decision },
        "domain": { "mode": "ux", "done": false },
        "navigation": { "action": "goto", "path": path },
        "present": {
            "announce": format!("Requesting {label}"),
            "steps": [
                { "kind": "goto", "path": path, "ms": 280 },
                { "kind": "wait", "ms": 180 },
                { "kind": "pulse", "target": format!("text:{label}"), "ms": 500 },
                {
                    "kind": "commit",
                    "method": "POST",
                    "path": "/api/ux/sign_off",
                    "body": { "slug": slug, "decision": decision, "actor": "agent" },
                    "ms": 240
                }
            ]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn isolated() -> std::sync::MutexGuard<'static, ()> {
        let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = std::env::temp_dir().join(format!(
            "veil-review-test-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        // SAFETY: tests are serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("VEIL_REVIEW_STORE", path.display().to_string());
        }
        {
            let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
            *s = ReviewState::default();
        }
        g
    }

    #[test]
    fn record_and_sign_off_roundtrip() {
        let _g = isolated();
        let a = record_file_edit("foo", "main.veil", Some("add port"));
        let _b = record_file_edit("foo", "layers/x.layer", None);
        let _c = record_file_edit("bar", "main.veil", None);
        assert_eq!(a.status, ItemStatus::Outstanding);
        assert_eq!(outstanding().len(), 3);
        let sum = summary_by_slug();
        assert_eq!(sum.get("foo").map(|s| s.outstanding), Some(2));
        assert!(sum.get("foo").unwrap().needs_sign_off);

        let (done, audit) = sign_off(SignOffRequest {
            ids: vec![],
            slug: Some("foo".into()),
            all: false,
            decision: "approve".into(),
            actor: "human".into(),
            note: Some("looks good".into()),
        })
        .expect("sign off");
        assert_eq!(done.len(), 2);
        assert_eq!(audit.decision, "approve");
        assert_eq!(outstanding().len(), 1);
        assert_eq!(outstanding()[0].slug, "bar");
    }

    #[test]
    fn coalesces_same_file_edits() {
        let _g = isolated();
        record_file_edit("foo", "main.veil", Some("v1"));
        record_file_edit("foo", "main.veil", Some("v2"));
        let items = outstanding();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].rationale.as_deref(), Some("v2"));
    }

    #[test]
    fn commit_stamps_git_sha() {
        let _g = isolated();
        record_file_edit("foo", "main.veil", None);
        record_commit("foo", "abc123def", "feat: ports");
        let items = list_items(ListFilter {
            slug: Some("foo".into()),
            ..Default::default()
        });
        assert!(items.iter().any(|i| i.kind == ItemKind::Commit));
        assert!(items
            .iter()
            .filter(|i| i.kind == ItemKind::FileEdit)
            .all(|i| i.git_sha.as_deref() == Some("abc123def")));
    }
}
