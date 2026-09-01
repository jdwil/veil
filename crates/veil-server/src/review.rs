//! Outstanding change sets + recorded human sign-off.
//!
//! This is **review state**, not a second VCS. Git remains history
//! (commit / branch / merge / log / diff). A **change set** is the unit
//! a human signs: topology + critical bodies + host check, bound to a
//! git SHA. That record is the ship gate (merge / provision).
//!
//! Durable file: `VEIL_REVIEW_STORE` or `{veil_home}/review-state.json`.
//! Audits are append-only (not a 200-row diary).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_ITEMS: usize = 400;

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
    #[serde(default)]
    pub pr_id: Option<String>,
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
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub structural_diff_hash: Option<String>,
    #[serde(default)]
    pub host_check: Option<Value>,
    #[serde(default)]
    pub pr_id: Option<String>,
    #[serde(default)]
    pub changeset_id: Option<String>,
    /// `human` | `system` | `dev`
    #[serde(default)]
    pub actor_kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ReviewState {
    items: Vec<OutstandingItem>,
    audits: Vec<SignOffRecord>,
    /// Durable per-edit capture (Spec A). Additive / back-compat: older
    /// `review-state.json` files without this key load as an empty vec.
    #[serde(default)]
    edits: Vec<EditRecord>,
}

const MAX_EDITS: usize = 2000;

// ─── Durable edit-capture record (Spec A) ─────────────────────────────────
//
// One `EditRecord` per logical construct-level change the agent made in a turn.
// It captures everything the "delta-on-map" visual review UX needs at write
// time, keyed to `(slug, construct, turn)`. Reuses `veil_ir` edit + struct_diff
// types verbatim — this is the single edit model shared by the agent
// (whole-file `write_source` synthesis) and the viewer (`POST /api/edit`).

/// A durable, queryable record of one construct-level edit in a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRecord {
    pub id: String,
    pub slug: String,
    /// The agent turn this edit belongs to (filmstrip grouping key). May be
    /// empty when no turn context is threaded (e.g. a raw viewer edit).
    #[serde(default)]
    pub turn_id: String,
    /// Coding session id when known.
    #[serde(default)]
    pub session_id: Option<String>,
    /// File the construct lives in.
    #[serde(default)]
    pub path: Option<String>,
    /// Projection-aware container path (e.g. "Identity/Customer").
    #[serde(default)]
    pub container_path: Option<String>,
    pub construct_name: String,
    /// IR node kind (`TypeDef`, `Flow`, `Step`, `InterfaceMethod`, …).
    #[serde(default)]
    pub construct_kind: String,
    /// Structured operations (true EditOps on the viewer path; synthesized /
    /// empty on the whole-file path where the delta carries the shape).
    #[serde(default)]
    pub edit_ops: Vec<veil_ir::EditOp>,
    /// Intent / category / criticality. Criticality is always resolved via
    /// `infer_criticality` when the agent omits it.
    pub annotation: veil_ir::EditAnnotation,
    /// Criticality surfaced at record level for cheap querying / pip rendering.
    pub criticality: veil_ir::Criticality,
    /// Topology delta for this construct (the `struct_diff` items touching it).
    #[serde(default)]
    pub structural_delta: Vec<veil_ir::DiffItem>,
    /// Statement-sized VEIL body text for High/Critical body changes only.
    /// Never generated Rust — bodies are VEIL only (review principle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_after: Option<String>,
    pub created_at: String,
}

/// Fields a caller supplies to synthesize an `EditRecord`. `slug`,
/// `session_id`, `turn_id`, `id`, and `created_at` are filled by `record_edits`.
#[derive(Debug, Clone)]
pub struct EditRecordSpec {
    pub path: Option<String>,
    pub container_path: Option<String>,
    pub construct_name: String,
    pub construct_kind: String,
    pub edit_ops: Vec<veil_ir::EditOp>,
    pub annotation: veil_ir::EditAnnotation,
    pub criticality: veil_ir::Criticality,
    pub structural_delta: Vec<veil_ir::DiffItem>,
    pub body_before: Option<String>,
    pub body_after: Option<String>,
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

/// Condensed, operator-facing "what changed & why" for a change set.
///
/// This is a PRESENTATION synthesis of intent the agent already captured
/// (`OutstandingItem.rationale` / `.summary`) — not new agent work. It lets the
/// operator decide from a headline without reading the diff or the transcript.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeSummary {
    /// 1–3 sentence "what changed & why", plain prose.
    pub headline: String,
    /// Distinct files touched (repo-relative), for the "files touched" line.
    pub files: Vec<String>,
    /// Distinct rationale/why lines gathered from the items (deduped, short).
    pub why: Vec<String>,
    /// Count of file-level edits/creations in the set.
    pub file_changes: usize,
    /// Host check error count (0 when clean / unknown).
    pub error_count: u32,
    /// Host check warning count (0 when clean / unknown).
    pub warning_count: u32,
    /// One-line check status, e.g. "0 errors / 2 warnings" or "checks clean".
    pub check_status: String,
}

/// One human-reviewable unit of work for a product slug.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeSet {
    pub id: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    pub item_ids: Vec<String>,
    pub outstanding: usize,
    pub summary: String,
    /// Condensed operator-facing headline synthesized from item intent.
    pub change_summary: ChangeSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_check: Option<Value>,
    pub host_has_errors: bool,
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
    pub pr_id: Option<String>,
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
    crate::session::chrono_now()
}

pub fn veil_dev_enabled() -> bool {
    match std::env::var("VEIL_DEV") {
        Ok(v) => {
            let l = v.trim().to_ascii_lowercase();
            l == "1" || l == "true" || l == "yes" || l == "on"
        }
        Err(_) => false,
    }
}

fn actor_looks_like_agent(actor: &str) -> bool {
    let a = actor.trim().to_ascii_lowercase();
    a == "agent" || a.starts_with("agent-") || a == "acp" || a == "inner-agent"
}

/// Hash of the structural walk the human saw (names + kinds).
pub fn hash_diff_spec(parts: &[String]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("{:016x}", h.finish())
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
        pr_id: spec.pr_id,
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
        pr_id: None,
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
        pr_id: None,
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
        pr_id: None,
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
        pr_id: None,
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
        pr_id: None,
    })
}

pub fn record_pr(slug: &str, title: &str, pr_id: Option<&str>) -> OutstandingItem {
    if let Some(id) = pr_id.filter(|s| !s.is_empty()) {
        let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
        for it in guard.items.iter_mut() {
            if it.status == ItemStatus::Outstanding
                && it.slug.eq_ignore_ascii_case(slug)
                && it.pr_id.is_none()
            {
                it.pr_id = Some(id.to_string());
            }
        }
        persist(&guard);
    }
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
        pr_id: pr_id.map(str::to_string),
    })
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {    pub slug: Option<String>,
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

#[derive(Debug, Clone, Default)]
pub struct SignOffRequest {
    pub ids: Vec<String>,
    pub slug: Option<String>,
    pub all: bool,
    pub decision: String,
    pub actor: String,
    pub note: Option<String>,
    pub git_sha: Option<String>,
    pub structural_diff_hash: Option<String>,
    pub host_check: Option<Value>,
    pub pr_id: Option<String>,
    pub via: Option<String>,
}

fn resolve_actor(req: &SignOffRequest) -> Result<(String, String), String> {
    let actor = if req.actor.trim().is_empty() {
        crate::session::current_user_id()
    } else {
        req.actor.trim().to_string()
    };
    if actor.eq_ignore_ascii_case("system") {
        return Ok((actor, "system".into()));
    }
    if actor_looks_like_agent(&actor) {
        if !veil_dev_enabled() {
            return Err(
                "agent cannot record sign-off; a human must use the Approve button on /review".into(),
            );
        }
        return Ok((actor, "dev".into()));
    }
    let via = req.via.as_deref().unwrap_or("");
    if via.eq_ignore_ascii_case("server") && actor_looks_like_agent(&actor) && !veil_dev_enabled()
    {
        return Err("sign_off via=server is forbidden outside VEIL_DEV".into());
    }
    Ok((actor, "human".into()))
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
    let (actor, actor_kind) = resolve_actor(&req)?;
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
    let git_sha = req.git_sha.clone().or_else(|| {
        chosen.iter().rev().find_map(|i| i.git_sha.clone())
    });
    let pr_id = req.pr_id.clone().or_else(|| {
        chosen.iter().rev().find_map(|i| i.pr_id.clone())
    });
    let changeset_id = Some(changeset_id_for(
        req.slug.as_deref().unwrap_or(
            chosen
                .first()
                .map(|i| i.slug.as_str())
                .unwrap_or("unknown"),
        ),
        git_sha.as_deref(),
        chosen.first().and_then(|i| i.session_id.as_deref()),
    ));
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
        git_sha,
        structural_diff_hash: req.structural_diff_hash,
        host_check: req.host_check,
        pr_id,
        changeset_id,
        actor_kind,
    };
    guard.audits.push(audit.clone());
    persist(&guard);
    Ok((chosen, audit))
}

/// Auto-close leftover outstanding items when a product is deleted.
/// Review state is not git — these items cannot be opened in the IDE anymore.
pub fn close_for_deleted_project(slug: &str, repo_id: Option<&str>) -> usize {
    let slug = slug.trim();
    if slug.is_empty() && repo_id.map(|s| s.trim().is_empty()).unwrap_or(true) {
        return 0;
    }
    let now = now_rfc3339();
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let mut ids = Vec::new();
    for it in guard.items.iter_mut() {
        if it.status != ItemStatus::Outstanding {
            continue;
        }
        let match_slug = !slug.is_empty() && it.slug.eq_ignore_ascii_case(slug);
        let match_repo = repo_id
            .map(|r| !r.is_empty() && it.repo_id.as_deref() == Some(r))
            .unwrap_or(false);
        if !(match_slug || match_repo) {
            continue;
        }
        it.status = ItemStatus::Rejected;
        it.decided_at = Some(now.clone());
        it.decided_by = Some("system".into());
        it.decision_note = Some("project deleted".into());
        ids.push(it.id.clone());
    }
    if ids.is_empty() {
        return 0;
    }
    let n = ids.len();
    guard.audits.push(SignOffRecord {
        id: format!("so_{}", short_id()),
        at: now,
        actor: "system".into(),
        decision: "reject".into(),
        item_ids: ids,
        note: Some("project deleted".into()),
        slug: if slug.is_empty() {
            None
        } else {
            Some(slug.to_string())
        },
        git_sha: None,
        structural_diff_hash: None,
        host_check: None,
        pr_id: None,
        changeset_id: None,
        actor_kind: "system".into(),
    });
    persist(&guard);
    n
}

/// Drop outstanding items for products that are no longer in the catalog.
/// `live` is product slugs and/or repo UUIDs. An empty list is a no-op so a
/// failed catalog fetch cannot wipe review state.
pub fn close_unknown_projects(live: &[String]) -> usize {
    let live: HashSet<String> = live
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if live.is_empty() {
        return 0;
    }
    let now = now_rfc3339();
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let mut ids = Vec::new();
    let mut slugs: HashSet<String> = HashSet::new();
    for it in guard.items.iter_mut() {
        if it.status != ItemStatus::Outstanding {
            continue;
        }
        let slug_ok = live.contains(&it.slug.to_ascii_lowercase());
        let repo_ok = it
            .repo_id
            .as_deref()
            .map(|r| live.contains(&r.to_ascii_lowercase()))
            .unwrap_or(false);
        if slug_ok || repo_ok {
            continue;
        }
        it.status = ItemStatus::Rejected;
        it.decided_at = Some(now.clone());
        it.decided_by = Some("system".into());
        it.decision_note = Some("project no longer in catalog".into());
        ids.push(it.id.clone());
        slugs.insert(it.slug.clone());
    }
    if ids.is_empty() {
        return 0;
    }
    let n = ids.len();
    guard.audits.push(SignOffRecord {
        id: format!("so_{}", short_id()),
        at: now,
        actor: "system".into(),
        decision: "reject".into(),
        item_ids: ids,
        note: Some("project no longer in catalog".into()),
        slug: slugs.into_iter().next(),
        git_sha: None,
        structural_diff_hash: None,
        host_check: None,
        pr_id: None,
        changeset_id: None,
        actor_kind: "system".into(),
    });
    persist(&guard);
    n
}

pub fn audits(limit: usize) -> Vec<SignOffRecord> {
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard.audits.iter().rev().take(limit.max(1)).cloned().collect()
}

// ─── Edit-capture persistence (Spec A) ────────────────────────────────────

fn short_edit_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("ed_{ms:x}")
}

/// Persist a batch of `EditRecord`s for the current turn. Slug / session / turn
/// are inferred from the active context when not already set on the specs.
/// Returns the stored records (with ids + timestamps filled in).
pub fn record_edits(specs: Vec<EditRecordSpec>) -> Vec<EditRecord> {
    if specs.is_empty() {
        return Vec::new();
    }
    let slug = infer_slug(None);
    let session_id = infer_session(None);
    let turn_id = crate::session::current_turn_id().unwrap_or_default();
    let now = now_rfc3339();
    let mut records = Vec::with_capacity(specs.len());
    for (i, spec) in specs.into_iter().enumerate() {
        records.push(EditRecord {
            // nanos + index keeps ids unique within a fast batch.
            id: format!("{}_{i}", short_edit_id()),
            slug: slug.clone(),
            turn_id: turn_id.clone(),
            session_id: session_id.clone(),
            path: spec.path,
            container_path: spec.container_path,
            construct_name: spec.construct_name,
            construct_kind: spec.construct_kind,
            edit_ops: spec.edit_ops,
            annotation: spec.annotation,
            criticality: spec.criticality,
            structural_delta: spec.structural_delta,
            body_before: spec.body_before,
            body_after: spec.body_after,
            created_at: now.clone(),
        });
    }
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard.edits.extend(records.iter().cloned());
    if guard.edits.len() > MAX_EDITS {
        let drain = guard.edits.len() - MAX_EDITS;
        guard.edits.drain(0..drain);
    }
    persist(&guard);
    records
}

/// Query captured edits, optionally by slug and/or turn. Newest first.
pub fn edits_for(slug: Option<&str>, turn: Option<&str>) -> Vec<EditRecord> {
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .edits
        .iter()
        .rev()
        .filter(|e| {
            slug.map(|s| e.slug.eq_ignore_ascii_case(s)).unwrap_or(true)
                && turn.map(|t| e.turn_id == t).unwrap_or(true)
        })
        .cloned()
        .collect()
}

/// All captured edits (newest first), capped for response size.
pub fn list_edits(slug: Option<&str>, turn: Option<&str>, limit: usize) -> Vec<EditRecord> {
    edits_for(slug, turn)
        .into_iter()
        .take(limit.max(1))
        .collect()
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
    let items = list_items(filter.clone());
    let summaries: Vec<RepoReviewSummary> = {
        let map = summary_by_slug();
        let mut v: Vec<_> = map.into_values().collect();
        v.sort_by(|a, b| b.outstanding.cmp(&a.outstanding));
        v
    };
    let sets = change_sets(filter.slug.as_deref());
    let edits = list_edits(filter.slug.as_deref(), None, 200);
    json!({
        "ok": true,
        "outstanding": items.iter().filter(|i| i.status == ItemStatus::Outstanding).count(),
        "items": items,
        "change_sets": sets,
        "by_project": summaries,
        "audits": audits(40),
        "edits": edits,
        "audit_env": audit_env_json(),
    })
}

pub fn audit_env_json() -> Value {
    let dev = veil_dev_enabled();
    json!({
        "veil_dev": dev,
        "ci_auto_pass": dev,
        "audit_environment": !dev,
        "note": if dev {
            "VEIL_DEV=1 — local / not an audit environment. CI auto-pass is on. Sign-off still required to merge or ship."
        } else {
            "Production-shaped host. Merge and deploy require a recorded human sign-off."
        }
    })
}

fn changeset_id_for(slug: &str, sha: Option<&str>, session: Option<&str>) -> String {
    match (sha, session) {
        (Some(s), _) if !s.is_empty() => format!("cs_{slug}_{}", &s[..s.len().min(12)]),
        (_, Some(sid)) if !sid.is_empty() => format!("cs_{slug}_{}", &sid[..sid.len().min(12)]),
        _ => format!("cs_{slug}"),
    }
}

fn host_check_for_slug(slug: &str) -> (Option<Value>, bool) {
    // Peek only. `project_session` would `resolve_for_project` → DDB scan
    // per leftover slug and stall GET /api/review/outstanding (~7s cold).
    let Some(h) = crate::coding_gates::peek_project_session(Some(slug)) else {
        return (None, false);
    };
    let meta = h.snapshot_meta();
    let v = crate::coding_gates::host_check_value(&meta);
    let errs = crate::coding_gates::has_host_errors(&meta);
    (Some(v), errs)
}

/// Synthesize a condensed, operator-facing summary from the intent the agent
/// already captured on the outstanding items (rationale / summary / kind) plus
/// the host check counts. Pure + testable — no I/O.
pub fn synthesize_change_summary(
    slug: &str,
    rows: &[OutstandingItem],
    host_check: Option<&Value>,
) -> ChangeSummary {
    // Distinct files touched (repo-relative), preserving first-seen order.
    let mut files: Vec<String> = Vec::new();
    for it in rows {
        if let Some(p) = it.path.as_deref().filter(|p| !p.trim().is_empty()) {
            if !files.iter().any(|f| f == p) {
                files.push(p.to_string());
            }
        }
    }

    // Distinct "why" lines: prefer rationale, fall back to non-boilerplate
    // summaries. Keep them short and deduped so the headline stays scannable.
    fn push_why(why: &mut Vec<String>, raw: &str) {
        let line = raw.trim();
        if line.is_empty() {
            return;
        }
        // First line only; rationales/commit messages can be multi-line.
        let line = line.lines().next().unwrap_or(line).trim();
        if line.is_empty() {
            return;
        }
        if !why.iter().any(|w| w.eq_ignore_ascii_case(line)) {
            why.push(line.to_string());
        }
    }
    let mut why: Vec<String> = Vec::new();
    for it in rows {
        if let Some(r) = it.rationale.as_deref() {
            push_why(&mut why, r);
        }
    }
    // If no rationale at all, fall back to summaries so the headline is not empty.
    if why.is_empty() {
        for it in rows {
            push_why(&mut why, &it.summary);
        }
    }

    let file_changes = rows
        .iter()
        .filter(|it| matches!(it.kind, ItemKind::FileEdit | ItemKind::FileCreated))
        .count();

    // Check status from host_check (0/unknown → treated as clean).
    let error_count = host_check
        .and_then(|v| v.get("error_count"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let warning_count = host_check
        .and_then(|v| v.get("warning_count"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let check_status = if error_count == 0 && warning_count == 0 {
        "checks clean".to_string()
    } else {
        format!(
            "{error_count} error{} / {warning_count} warning{}",
            if error_count == 1 { "" } else { "s" },
            if warning_count == 1 { "" } else { "s" }
        )
    };

    // Compose a 1–3 sentence headline: what (count/files) + why (first reason).
    let n = rows.len();
    let what = if file_changes > 0 {
        let file_bit = if files.len() == 1 {
            format!("{} file", 1)
        } else {
            format!("{} files", files.len().max(file_changes))
        };
        format!("{n} change(s) across {file_bit} in {slug}")
    } else {
        format!("{n} change(s) in {slug}")
    };
    let mut headline = what;
    if let Some(first_why) = why.first() {
        headline = format!("{headline} — {first_why}");
    }

    ChangeSummary {
        headline,
        files,
        why,
        file_changes,
        error_count,
        warning_count,
        check_status,
    }
}

pub fn change_sets(slug: Option<&str>) -> Vec<ChangeSet> {
    let items = list_items(ListFilter {
        slug: slug.map(|s| s.to_string()),
        status: Some(ItemStatus::Outstanding),
        ..Default::default()
    });
    let mut by: HashMap<String, Vec<OutstandingItem>> = HashMap::new();
    for it in items {
        by.entry(it.slug.clone()).or_default().push(it);
    }
    let mut out = Vec::new();
    for (slug, rows) in by {
        let git_sha = rows.iter().rev().find_map(|i| i.git_sha.clone());
        let pr_id = rows.iter().rev().find_map(|i| i.pr_id.clone());
        let session_id = rows.iter().rev().find_map(|i| i.session_id.clone());
        let repo_id = rows.iter().find_map(|i| i.repo_id.clone());
        let (host_check, host_has_errors) = host_check_for_slug(&slug);
        let change_summary = synthesize_change_summary(&slug, &rows, host_check.as_ref());
        let n = rows.len();
        let summary = format!("{n} unreviewed change(s) in {slug}");
        out.push(ChangeSet {
            id: changeset_id_for(&slug, git_sha.as_deref(), session_id.as_deref()),
            slug,
            repo_id,
            session_id,
            pr_id,
            git_sha,
            item_ids: rows.iter().map(|i| i.id.clone()).collect(),
            outstanding: n,
            summary,
            change_summary,
            host_check,
            host_has_errors,
        });
    }
    out.sort_by(|a, b| b.outstanding.cmp(&a.outstanding));
    out
}

/// Latest approve audit that covers this product (and SHA when given).
pub fn latest_approve(slug: &str, sha: Option<&str>) -> Option<SignOffRecord> {
    let slug = slug.trim();
    if slug.is_empty() {
        return None;
    }
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    guard.audits.iter().rev().find(|a| {
        if a.decision != "approve" {
            return false;
        }
        let slug_ok = a
            .slug
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(slug))
            .unwrap_or(false);
        if !slug_ok {
            return false;
        }
        match (sha, a.git_sha.as_deref()) {
            (Some(want), Some(got)) if !got.is_empty() => {
                want.starts_with(got) || got.starts_with(want)
            }
            _ => true,
        }
    }).cloned()
}

// ─── Subpath attribution (hybrid model: N projects share one repo) ────────
//
// A PR / change touches repo-relative file paths. When several VEIL projects
// bind the SAME repo at distinct subpaths, we attribute the change to EVERY
// project whose subpath its files touch (path-prefix match), and the ship gate
// requires ALL touched projects' sign-offs (decision 2). A single-subpath PR is
// just one project's review — the common case.

/// One project bound to a shared repo: its product slug + its subpath.
/// `subpath = None` means the project owns the repo root (no other project can
/// share it, so it only matches when it is the sole binding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubpathProject {
    pub slug: String,
    pub subpath: Option<String>,
}

/// Attribute a set of repo-relative changed paths to the projects whose
/// subpaths they touch. Pure + order-independent so it is unit-testable without
/// a catalog. Returns the touched project slugs (sorted, de-duped).
///
/// Rules:
/// - A path `p` touches project `P` when `P.subpath` is `None` (repo root — it
///   owns everything) OR `p` is under `<P.subpath>/` (prefix on a path boundary).
/// - The most specific subpath wins is NOT applied: nested subpaths are a
///   configuration the design forbids (each project = a distinct subdir), so a
///   path is attributed to every project it is under. In practice subpaths are
///   siblings, so each path lands in exactly one project.
pub fn attribute_paths_to_projects(
    changed_paths: &[String],
    projects: &[SubpathProject],
) -> Vec<String> {
    let mut touched: HashSet<String> = HashSet::new();
    for raw in changed_paths {
        let p = raw.trim().trim_start_matches('/').replace('\\', "/");
        if p.is_empty() {
            continue;
        }
        for proj in projects {
            let hit = match proj.subpath.as_deref().map(|s| s.trim_matches('/')) {
                None | Some("") => true, // repo-root project owns all paths
                Some(sub) => p == sub || p.starts_with(&format!("{sub}/")),
            };
            if hit {
                touched.insert(proj.slug.clone());
            }
        }
    }
    let mut out: Vec<String> = touched.into_iter().collect();
    out.sort();
    out
}

/// Ship gate for a set of touched project slugs: EVERY project must pass
/// [`may_ship`]. Used when a PR touches multiple subpath projects — all of their
/// review gates must be green before the shared-repo change can merge.
pub fn may_ship_all(slugs: &[String], sha: Option<&str>) -> Result<(), String> {
    let mut blocked = Vec::new();
    for slug in slugs {
        if let Err(e) = may_ship(slug, sha) {
            blocked.push(format!("`{slug}`: {e}"));
        }
    }
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} of {} touched project(s) not ready to ship — all must be signed off:\n{}",
            blocked.len(),
            slugs.len(),
            blocked.join("\n")
        ))
    }
}

/// Merge / provision gate. New edits after sign-off re-open outstanding and block again.
pub fn may_ship(slug: &str, sha: Option<&str>) -> Result<(), String> {
    let slug = slug.trim();
    if slug.is_empty() {
        return Err("slug required to ship".into());
    }
    let open = list_items(ListFilter {
        slug: Some(slug.into()),
        status: Some(ItemStatus::Outstanding),
        ..Default::default()
    });
    if !open.is_empty() {
        return Err(format!(
            "sign off {n} outstanding change(s) for `{slug}` before merge / deploy",
            n = open.len()
        ));
    }
    if latest_approve(slug, sha).is_some() {
        return Ok(());
    }
    let any = list_items(ListFilter {
        slug: Some(slug.into()),
        ..Default::default()
    });
    if any.is_empty() {
        // Never touched by the agent — nothing to sign.
        return Ok(());
    }
    Err(format!(
        "no recorded human sign-off for `{slug}`{}",
        sha.map(|s| format!(" (sha {s})")).unwrap_or_default()
    ))
}

pub fn export_json() -> Value {    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    json!({
        "ok": true,
        "exported_at": now_rfc3339(),
        "audit_env": audit_env_json(),
        "items": guard.items,
        "audits": guard.audits,
        "edits": guard.edits,
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
                "Here is what I did — please review".to_string()
            } else {
                format!("Here is exactly what I did ({count} changes) — please review")
            },
            "steps": [
                { "kind": "goto", "path": path, "ms": 300 },
                { "kind": "wait", "ms": 200 },
                { "kind": "highlight", "selector": "[data-veil-role='sign-off'], [data-veil-action='sign-off']", "ms": 700 }
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
        "Request changes"
    } else {
        "Approve"
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
                { "kind": "highlight", "selector": "[data-veil-action='sign-off'], [data-veil-action='reject-sign-off']", "ms": 700 }
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

    /// A diff touching only A/ attributes to A; a diff touching A/ and B/
    /// attributes to both; the multi-project ship gate requires all touched.
    #[test]
    fn subpath_attribution_and_multi_gate() {
        let _g = isolated();
        let projects = vec![
            SubpathProject { slug: "dlx-auth".into(), subpath: Some("dlx-auth".into()) },
            SubpathProject { slug: "dlx-bus".into(), subpath: Some("dlx-bus".into()) },
        ];

        // Only A/ touched → attributes to dlx-auth alone.
        let only_a = attribute_paths_to_projects(
            &["dlx-auth/main.veil".into(), "dlx-auth/layers/x.layer".into()],
            &projects,
        );
        assert_eq!(only_a, vec!["dlx-auth".to_string()]);

        // A/ and B/ touched → both projects.
        let both = attribute_paths_to_projects(
            &["dlx-auth/main.veil".into(), "dlx-bus/main.veil".into()],
            &projects,
        );
        assert_eq!(both, vec!["dlx-auth".to_string(), "dlx-bus".to_string()]);

        // A leading slash + backslash normalise; unrelated path attributes to none.
        let mixed = attribute_paths_to_projects(
            &["/dlx-bus\\main.veil".into(), "README.md".into()],
            &projects,
        );
        assert_eq!(mixed, vec!["dlx-bus".to_string()]);

        // Repo-root project owns everything.
        let root = vec![SubpathProject { slug: "solo".into(), subpath: None }];
        let all = attribute_paths_to_projects(&["anything/here.veil".into()], &root);
        assert_eq!(all, vec!["solo".to_string()]);

        // Prefix boundary: "dlx-auth-extra/" must NOT match "dlx-auth".
        let boundary = attribute_paths_to_projects(
            &["dlx-auth-extra/main.veil".into()],
            &projects,
        );
        assert!(boundary.is_empty(), "prefix must respect path boundary: {boundary:?}");

        // Multi-project gate: with both touched and neither signed, ship blocks
        // and the error names both projects.
        record_file_edit("dlx-auth", "main.veil", None);
        record_file_edit("dlx-bus", "main.veil", None);
        let err = may_ship_all(&both, None).unwrap_err();
        assert!(err.contains("dlx-auth") && err.contains("dlx-bus"), "{err}");

        // Sign off ONLY A → still blocked (B outstanding).
        sign_off(SignOffRequest {
            slug: Some("dlx-auth".into()),
            decision: "approve".into(),
            actor: "operator".into(),
            ..Default::default()
        })
        .expect("approve A");
        assert!(may_ship_all(&both, None).is_err(), "must block until ALL touched projects pass");

        // Sign off B too → now shippable.
        sign_off(SignOffRequest {
            slug: Some("dlx-bus".into()),
            decision: "approve".into(),
            actor: "operator".into(),
            ..Default::default()
        })
        .expect("approve B");
        assert!(may_ship_all(&both, None).is_ok(), "all touched signed → ship allowed");
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
            actor: "operator".into(),
            note: Some("looks good".into()),
            git_sha: Some("abc123def".into()),
            structural_diff_hash: Some("deadbeef".into()),
            ..Default::default()
        })
        .expect("sign off");
        assert_eq!(done.len(), 2);
        assert_eq!(audit.decision, "approve");
        assert_eq!(audit.actor_kind, "human");
        assert!(audit.at.contains('T'), "RFC3339: {}", audit.at);
        assert_eq!(audit.git_sha.as_deref(), Some("abc123def"));
        assert_eq!(audit.structural_diff_hash.as_deref(), Some("deadbeef"));
        assert_eq!(outstanding().len(), 1);
        assert_eq!(outstanding()[0].slug, "bar");
        assert!(may_ship("foo", Some("abc123def")).is_ok());
        assert!(may_ship("bar", None).is_err());
    }

    #[test]
    fn agent_cannot_sign_off() {
        let _g = isolated();
        let prev = std::env::var("VEIL_DEV").ok();
        unsafe {
            std::env::remove_var("VEIL_DEV");
        }
        record_file_edit("foo", "main.veil", None);
        let err = sign_off(SignOffRequest {
            slug: Some("foo".into()),
            decision: "approve".into(),
            actor: "agent".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.contains("agent cannot record sign-off"), "{err}");
        assert_eq!(outstanding().len(), 1);
        unsafe {
            match prev {
                Some(v) => std::env::set_var("VEIL_DEV", v),
                None => std::env::remove_var("VEIL_DEV"),
            }
        }
    }

    #[test]
    fn change_sets_do_not_resolve_missing_projects() {
        let _g = isolated();
        record_file_edit("gone-project", "main.veil", Some("leftover"));
        let started = std::time::Instant::now();
        let sets = change_sets(None);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "change_sets must not DDB-scan leftover slugs ({:?})",
            started.elapsed()
        );
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].slug, "gone-project");
        assert!(sets[0].host_check.is_none());
        assert!(!sets[0].host_has_errors);
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

    #[test]
    fn delete_project_closes_outstanding() {
        let _g = isolated();
        record_file_edit("lumen-desk", "main.veil", Some("e2e"));
        record_file_edit("agent-core", "main.veil", None);
        assert_eq!(outstanding().len(), 2);
        let n = close_for_deleted_project("lumen-desk", None);
        assert_eq!(n, 1);
        let left = outstanding();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].slug, "agent-core");
    }

    #[test]
    fn unknown_catalog_slugs_are_closed() {
        let _g = isolated();
        record_file_edit("keep-me", "main.veil", None);
        record_file_edit("drop-a", "main.veil", None);
        record_file_edit("drop-b", "main.veil", None);
        assert_eq!(close_unknown_projects(&[]), 0);
        assert_eq!(outstanding().len(), 3);
        let n = close_unknown_projects(&["keep-me".into(), "other".into()]);
        assert_eq!(n, 2);
        let left = outstanding();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].slug, "keep-me");
    }

    /// Creating a PR records exactly ONE review-facing item (the single source
    /// of truth). change_management holds the transport PR separately; this
    /// asserts review.rs is not double-writing its own record on create.
    #[test]
    fn pr_create_records_single_review_item() {
        let _g = isolated();
        let item = record_pr("acme", "Add ports", Some("pr-123"));
        assert_eq!(item.kind, ItemKind::PullRequest);
        assert_eq!(item.pr_id.as_deref(), Some("pr-123"));
        let prs: Vec<_> = outstanding()
            .into_iter()
            .filter(|i| i.kind == ItemKind::PullRequest && i.slug == "acme")
            .collect();
        assert_eq!(prs.len(), 1, "exactly one review-facing PR item per create");
    }

    /// may_ship is the SOLE gate: it consults only the recorded review state
    /// (outstanding items + the human SignOffRecord). It has no knowledge of
    /// change_management `PrStatus`, so a transport "Approved" can never enable
    /// ship on its own — only a recorded sign-off flips may_ship to Ok.
    #[test]
    fn may_ship_is_gated_only_by_recorded_sign_off() {
        let _g = isolated();
        // A PR item + a file edit for the same slug are outstanding.
        record_pr("acme", "Add ports", Some("pr-9"));
        record_file_edit("acme", "main.veil", Some("ports"));
        // No sign-off yet: ship must be refused regardless of any external
        // transport status (review.rs cannot even see PrStatus).
        let blocked = may_ship("acme", None);
        assert!(
            blocked.is_err(),
            "may_ship must refuse while items are outstanding"
        );

        // The recorded human sign-off is the ONLY thing that flips the gate.
        let (signed, audit) = sign_off(SignOffRequest {
            slug: Some("acme".into()),
            all: false,
            decision: "approve".into(),
            actor: "operator".into(),
            ..Default::default()
        })
        .expect("human approve");
        assert_eq!(audit.decision, "approve");
        assert!(!signed.is_empty());
        assert!(
            may_ship("acme", None).is_ok(),
            "recorded sign-off must enable ship"
        );
    }

    /// A brand-new edit AFTER a sign-off re-opens outstanding work and blocks
    /// ship again — the SignOffRecord is bound to the reviewed set, so ship
    /// cannot ride a stale approval.
    #[test]
    fn new_edit_after_sign_off_reblocks_ship() {
        let _g = isolated();
        record_file_edit("acme", "main.veil", Some("v1"));
        sign_off(SignOffRequest {
            slug: Some("acme".into()),
            decision: "approve".into(),
            actor: "operator".into(),
            ..Default::default()
        })
        .expect("approve");
        assert!(may_ship("acme", None).is_ok());
        // New work lands: outstanding again → blocked.
        record_file_edit("acme", "layers/new.layer", Some("v2"));
        assert!(
            may_ship("acme", None).is_err(),
            "post-approval edits must re-block ship"
        );
    }

    /// The condensed summary synthesizes what/why/files/check-status from the
    /// intent already captured on the items — no new agent work required.
    #[test]
    fn synthesize_change_summary_from_captured_intent() {
        let _g = isolated();
        record_file_edit("acme", "main.veil", Some("Add rate-limit guard to checkout"));
        record_file_created("acme", "layers/limits.layer");
        let rows = list_items(ListFilter {
            slug: Some("acme".into()),
            status: Some(ItemStatus::Outstanding),
            ..Default::default()
        });
        // Clean check.
        let clean = super::synthesize_change_summary("acme", &rows, None);
        assert!(clean.headline.contains("acme"), "headline: {}", clean.headline);
        assert!(
            clean.headline.contains("rate-limit guard"),
            "headline carries the captured why: {}",
            clean.headline
        );
        assert_eq!(clean.file_changes, 2);
        assert!(clean.files.iter().any(|f| f == "main.veil"));
        assert!(clean.files.iter().any(|f| f == "layers/limits.layer"));
        assert!(clean.why.iter().any(|w| w.contains("rate-limit guard")));
        assert_eq!(clean.error_count, 0);
        assert_eq!(clean.warning_count, 0);
        assert_eq!(clean.check_status, "checks clean");

        // With host check counts, the status line reflects errors/warnings.
        let hc = json!({ "error_count": 0, "warning_count": 2, "severity": "warnings" });
        let warned = super::synthesize_change_summary("acme", &rows, Some(&hc));
        assert_eq!(warned.error_count, 0);
        assert_eq!(warned.warning_count, 2);
        assert_eq!(warned.check_status, "0 errors / 2 warnings");
    }

    /// change_sets() exposes the condensed summary on every set.
    #[test]
    fn change_sets_carry_condensed_summary() {
        let _g = isolated();
        record_file_edit("beta", "main.veil", Some("Wire the ports"));
        let sets = change_sets(Some("beta"));
        assert_eq!(sets.len(), 1);
        let cs = &sets[0];
        assert!(!cs.change_summary.headline.is_empty());
        assert!(cs.change_summary.headline.contains("Wire the ports"));
        assert!(cs.change_summary.files.iter().any(|f| f == "main.veil"));
    }

    fn sample_spec(name: &str, crit: veil_ir::Criticality) -> EditRecordSpec {
        EditRecordSpec {
            path: Some("main.veil".into()),
            container_path: Some("Checkout".into()),
            construct_name: name.into(),
            construct_kind: "Step".into(),
            edit_ops: vec![],
            annotation: veil_ir::EditAnnotation {
                intent: Some("why".into()),
                category: Some(veil_ir::EditCategory::Behavior),
                criticality: Some(crit),
            },
            criticality: crit,
            structural_delta: vec![],
            body_before: Some("guard ok".into()),
            body_after: Some("guard tightened".into()),
        }
    }

    /// EditRecords persist to the store file and survive a reload (the ship
    /// gate requires durable capture, not a transient cache).
    #[test]
    fn edit_records_survive_reload() {
        let _g = isolated();
        // No turn scope → turn_id is empty but the record still persists.
        let stored = record_edits(vec![sample_spec("Validate", veil_ir::Criticality::High)]);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].criticality, veil_ir::Criticality::High);
        assert_eq!(stored[0].body_after.as_deref(), Some("guard tightened"));

        // Force a reload from disk into a fresh store state.
        {
            let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
            *s = load_from_disk();
        }
        let reloaded = list_edits(None, None, 100);
        assert_eq!(reloaded.len(), 1, "edit must survive store reload");
        assert_eq!(reloaded[0].construct_name, "Validate");
        assert_eq!(reloaded[0].criticality, veil_ir::Criticality::High);
        assert_eq!(reloaded[0].body_before.as_deref(), Some("guard ok"));

        // snapshot_json exposes edits additively.
        let snap = snapshot_json(ListFilter::default());
        assert_eq!(snap["edits"].as_array().map(|a| a.len()), Some(1));
    }

    /// EditRecords are keyed to the active agent turn via CURRENT_TURN.
    #[test]
    fn edit_records_key_to_current_turn() {
        let _g = isolated();
        crate::session::CURRENT_TURN.sync_scope("turn-xyz".to_string(), || {
            record_edits(vec![sample_spec("Persist", veil_ir::Criticality::Normal)]);
        });
        let by_turn = edits_for(None, Some("turn-xyz"));
        assert_eq!(by_turn.len(), 1);
        assert_eq!(by_turn[0].turn_id, "turn-xyz");
        // A different turn filter finds nothing.
        assert!(edits_for(None, Some("other-turn")).is_empty());
    }
}
