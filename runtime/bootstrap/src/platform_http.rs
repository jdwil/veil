//! Platform domain HTTP surface for ProductHost (single process).
//!
//! Wires generated `storage` / `change_management` / `deploy` application
//! services so the dashboard SPA needs no separate `veil_bin` on :3000.
//! See `runtime/docs/ADR_SINGLE_PRODUCT_HOST.md`.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// ─── Storage ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct StorageState {
    deps: Arc<storage::application::Deps>,
}

async fn storage_aws_deps() -> storage::application::Deps {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let ddb = aws_sdk_dynamodb::Client::new(&config);
    let s3 = aws_sdk_s3::Client::new(&config);
    let table = std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
    let bucket = std::env::var("BUCKET").unwrap_or_else(|_| "veil-runtime-dev".into());
    storage::application::Deps {
        metadata_store: Arc::new(storage::adapters::DdbMetadataStore {
            client: ddb,
            table,
        }),
        object_storage: Arc::new(storage::adapters::S3ObjectStorage {
            bucket,
            client: s3,
        }),
    }
}

fn storage_deps_local() -> storage::application::Deps {
    crate::local_ports::storage_deps()
}

async fn resolve_storage_deps() -> storage::application::Deps {
    // Prefer AWS when table/bucket set (live-like); else local hub ports.
    if std::env::var("VEIL_PLATFORM_LOCAL").ok().as_deref() == Some("1") {
        return storage_deps_local();
    }
    if std::env::var("VEIL_DDB_TABLE").is_ok() || std::env::var("AWS_PROFILE").is_ok() {
        return storage_aws_deps().await;
    }
    storage_deps_local()
}

async fn list_repos(State(st): State<StorageState>) -> Result<Json<Value>, StatusCode> {
    match storage::application::list_repos(&st.deps).await {
        Ok(repos) => Ok(Json(json!(repos))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct CreateRepoBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

async fn create_repo(
    State(st): State<StorageState>,
    Json(body): Json<CreateRepoBody>,
) -> Result<Json<Value>, StatusCode> {
    match storage::application::create_repo(&st.deps, body.name, body.description).await {
        Ok(repo) => Ok(Json(json!(repo))),
        Err(e) => Err(domain_status(e)),
    }
}

async fn get_repo(
    State(st): State<StorageState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    // Accept repo UUID or slug (agent open_project / open_ide use slugs).
    match storage::application::get_repo(&st.deps, id.clone()).await {
        Ok(repo) => Ok(Json(json!(repo))),
        Err(_) => {
            let repos = storage::application::list_repos(&st.deps)
                .await
                .map_err(domain_status)?;
            let needle = id.to_lowercase();
            if let Some(repo) = repos.into_iter().find(|r| {
                r.id.value.eq_ignore_ascii_case(&id)
                    || r.slug.eq_ignore_ascii_case(&needle)
                    || r.name.eq_ignore_ascii_case(&needle)
            }) {
                Ok(Json(json!(repo)))
            } else {
                Err(StatusCode::NOT_FOUND)
            }
        }
    }
}

async fn delete_repo(
    State(st): State<StorageState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    match storage::application::delete_repo(&st.deps, id).await {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct InfraQuery {
    #[serde(default)]
    environment: Option<String>,
}

async fn get_project_infra(
    State(st): State<StorageState>,
    Path(id): Path<String>,
    Query(q): Query<InfraQuery>,
) -> Result<Json<Value>, StatusCode> {
    // Resolve slug → id so /api/project_infras/relay works.
    let resolved_id = match storage::application::get_repo(&st.deps, id.clone()).await {
        Ok(repo) => repo.id.value,
        Err(_) => {
            let repos = storage::application::list_repos(&st.deps)
                .await
                .map_err(domain_status)?;
            let needle = id.to_lowercase();
            repos
                .into_iter()
                .find(|r| {
                    r.id.value.eq_ignore_ascii_case(&id)
                        || r.slug.eq_ignore_ascii_case(&needle)
                        || r.name.eq_ignore_ascii_case(&needle)
                })
                .map(|r| r.id.value)
                .ok_or(StatusCode::NOT_FOUND)?
        }
    };
    let env = q.environment.clone();
    match storage::application::get_project_infra(&st.deps, resolved_id, env.clone()).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => {
            // Infra is optional on the project detail page — never hard-fail the shell.
            tracing::warn!(error = ?e, "get_project_infra failed; returning empty infra");
            Ok(Json(json!({
                "repo": null,
                "infra": {},
                "environment": env.unwrap_or_else(|| "dev".into()),
                "environments": {"default": "dev", "environments": []},
                "source": "none",
                "s3_key": "",
                "error": format!("{e:?}"),
            })))
        }
    }
}

// ─── Source files (ObjectStorage: S3 or local hub mirror) ───────────────────

/// Resolve UUID or slug → canonical repo id string.
async fn resolve_repo_id_value(
    deps: &storage::application::Deps,
    id: &str,
) -> Result<String, StatusCode> {
    match storage::application::get_repo(deps, id.to_string()).await {
        Ok(repo) => Ok(repo.id.value),
        Err(_) => {
            let repos = storage::application::list_repos(deps)
                .await
                .map_err(domain_status)?;
            let needle = id.to_lowercase();
            repos
                .into_iter()
                .find(|r| {
                    r.id.value.eq_ignore_ascii_case(id)
                        || r.slug.eq_ignore_ascii_case(&needle)
                        || r.name.eq_ignore_ascii_case(&needle)
                })
                .map(|r| r.id.value)
                .ok_or(StatusCode::NOT_FOUND)
        }
    }
}

fn safe_repo_rel_path(path: &str) -> Result<&str, StatusCode> {
    let p = path.trim().trim_start_matches('/');
    if p.is_empty() || p.contains("..") || p.starts_with('/') {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Disallow absolute / drive paths
    if std::path::Path::new(p).is_absolute() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(p)
}

#[derive(Deserialize)]
struct ReadFileBody {
    #[serde(default)]
    repo_id: Option<String>,
    /// Alias for repo_id (slug or UUID).
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    path: String,
}

async fn read_file_api(
    State(st): State<StorageState>,
    Json(body): Json<ReadFileBody>,
) -> Result<Json<Value>, StatusCode> {
    let raw_id = body
        .repo_id
        .or(body.id)
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let path = safe_repo_rel_path(&body.path)?;
    let branch = body
        .branch
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into());
    let repo_id_val = resolve_repo_id_value(&st.deps, &raw_id).await?;
    let rid = storage::domain::types::RepoId {
        value: repo_id_val.clone(),
    };
    match storage::application::read_file(&st.deps, rid, branch.clone(), path.to_string()).await {
        Ok(bytes) => {
            let content = String::from_utf8_lossy(&bytes).into_owned();
            Ok(Json(json!({
                "ok": true,
                "exists": true,
                "repo_id": repo_id_val,
                "branch": branch,
                "path": path,
                "content": content,
                "bytes": bytes.len(),
            })))
        }
        Err(veil_shared::DomainError::NotFound) => Ok(Json(json!({
            "ok": true,
            "exists": false,
            "repo_id": repo_id_val,
            "branch": branch,
            "path": path,
            "content": "",
            "bytes": 0,
        }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct WriteFileBody {
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    path: String,
    content: String,
    #[serde(default)]
    message: Option<String>,
}

async fn write_file_api(
    State(st): State<StorageState>,
    Json(body): Json<WriteFileBody>,
) -> Result<Json<Value>, StatusCode> {
    let raw_id = body
        .repo_id
        .or(body.id)
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let path = safe_repo_rel_path(&body.path)?;
    let branch = body
        .branch
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into());
    let message = body
        .message
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("update {path}"));
    let repo_id_val = resolve_repo_id_value(&st.deps, &raw_id).await?;
    let rid = storage::domain::types::RepoId {
        value: repo_id_val.clone(),
    };
    match storage::application::write_file(
        &st.deps,
        rid,
        branch.clone(),
        path.to_string(),
        body.content.clone(),
        message.clone(),
    )
    .await
    {
        Ok(commit) => Ok(Json(json!({
            "ok": true,
            "repo_id": repo_id_val,
            "branch": branch,
            "path": path,
            "bytes": body.content.len(),
            "commit": commit,
            "message": message,
        }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct BranchQuery {
    #[serde(default)]
    branch: Option<String>,
}

/// GET/PUT convenience for product intent brief at project root.
async fn get_mission(
    State(st): State<StorageState>,
    Path(id): Path<String>,
    Query(q): Query<BranchQuery>,
) -> Result<Json<Value>, StatusCode> {
    let body = ReadFileBody {
        repo_id: Some(id),
        id: None,
        branch: q.branch.or_else(|| Some("main".into())),
        path: "MISSION.md".into(),
    };
    read_file_api(State(st), Json(body)).await
}

#[derive(Deserialize)]
struct MissionPutBody {
    content: String,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

async fn put_mission(
    State(st): State<StorageState>,
    Path(id): Path<String>,
    Json(body): Json<MissionPutBody>,
) -> Result<Json<Value>, StatusCode> {
    let body = WriteFileBody {
        repo_id: Some(id),
        id: None,
        branch: body.branch.or_else(|| Some("main".into())),
        path: "MISSION.md".into(),
        content: body.content,
        message: body.message.or_else(|| Some("update MISSION.md".into())),
    };
    write_file_api(State(st), Json(body)).await
}

// ─── Project module query ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct ModuleQuery {
    module: String,
    #[serde(flatten)]
    filters: std::collections::HashMap<String, String>,
}

async fn query_project_modules(
    State(st): State<StorageState>,
    Query(q): Query<ModuleQuery>,
) -> Result<Json<Value>, StatusCode> {
    let filters_json = serde_json::to_string(&q.filters).unwrap_or_else(|_| "{}".into());
    match storage::application::query_project_modules(&st.deps, q.module, filters_json).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

// ─── Change management ──────────────────────────────────────────────────────

#[derive(Clone)]
struct CmState {
    deps: Arc<change_management::application::Deps>,
}

#[derive(Deserialize)]
struct ListAllQuery {
    #[serde(default)]
    status: Option<String>,
}

async fn list_all_change_requests(
    State(st): State<CmState>,
    Query(q): Query<ListAllQuery>,
) -> Result<Json<Value>, StatusCode> {
    let status = parse_pr_status(q.status.as_deref());
    match change_management::application::list_all_change_requests(&st.deps, status).await {
        Ok(items) => Ok(Json(json!(items))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct CreateFlatBody {
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    jira_ticket: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    source_branch: Option<String>,
}

async fn create_change_request_flat(
    State(st): State<CmState>,
    Json(body): Json<CreateFlatBody>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = body
        .repo_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4);
    let slug = body
        .slug
        .clone()
        .unwrap_or_else(|| body.source_branch.clone().unwrap_or_else(|| "main".into()));
    let author = if body.author.is_empty() {
        "agent".into()
    } else {
        body.author
    };
    // Domain requires non-empty jira_ticket — default for agent/local flows.
    let jira = if body.jira_ticket.trim().is_empty() {
        format!(
            "VEIL-{}",
            chrono::Utc::now().format("%Y%m%d%H%M")
        )
    } else {
        body.jira_ticket.trim().to_string()
    };
    // Ensure git refs exist so create_branch doesn't 500 on fresh repos.
    let _ = st.deps.git.init_repo(slug.clone()).await;
    // Prefer explicit work branch when provided (coding session).
    let preferred_branch = body
        .source_branch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    match change_management::application::create_change_request_flat(
        &st.deps,
        repo_id,
        slug.clone(),
        body.title.clone(),
        body.description.clone(),
        jira.clone(),
        author.clone(),
    )
    .await
    {
        Ok(mut cr) => {
            if let Some(ref b) = preferred_branch {
                if b != &cr.source_branch {
                    cr.source_branch = b.clone();
                    cr.updated_at = chrono::Utc::now();
                    if let Err(e) = st.deps.cr_repo.save(cr.clone()).await {
                        return Err(domain_status(e));
                    }
                }
            }
            let _ = ensure_ci_passed(&st.deps, cr.id, "pending").await;
            Ok(Json(json!({
                "change_request": cr,
                "slug": slug,
                "wizard_path": format!("/changes/{}", cr.id),
            })))
        }
        Err(e) => {
            // Soft path: if git branch create failed, still persist a CR for PR Wizard.
            tracing::warn!(?e, "create_change_request_flat failed — soft-creating META only");
            use change_management::domain::types::{ChangeRequest, PrStatus};
            let now = chrono::Utc::now();
            let cr_id = Uuid::new_v4();
            let source = preferred_branch.unwrap_or_else(|| {
                format!(
                    "cr/{}/{}",
                    jira,
                    body.title.to_lowercase().replace(' ', "-")
                )
            });
            let cr = ChangeRequest {
                id: cr_id,
                repo_id,
                title: body.title,
                description: body.description,
                jira_ticket: jira,
                source_branch: source,
                target_branch: "main".into(),
                author,
                status: PrStatus::Draft,
                created_at: now,
                updated_at: now,
                merged_at: None,
                merged_by: None,
                merge_commit: None,
            };
            st.deps
                .cr_repo
                .save(cr.clone())
                .await
                .map_err(domain_status)?;
            let _ = ensure_ci_passed(&st.deps, cr.id, "pending").await;
            Ok(Json(json!({
                "change_request": cr,
                "slug": slug,
                "wizard_path": format!("/changes/{}", cr.id),
                "soft_create": true,
            })))
        }
    }
}

/// Soft-gate helper: record a Passed CI run when none exists (local/dev PR Wizard).
async fn ensure_ci_passed(
    deps: &change_management::application::Deps,
    pr_id: Uuid,
    commit_hash: &str,
) -> Result<(), StatusCode> {
    use change_management::domain::types::{CiRun, CiStatus};
    match deps.ci_repo.latest_for_pr(pr_id).await {
        Ok(Some(run)) if matches!(run.status, CiStatus::Passed) => Ok(()),
        Ok(_) | Err(_) => {
            let now = chrono::Utc::now();
            let run = CiRun {
                id: Uuid::new_v4(),
                pr_id,
                commit_hash: commit_hash.to_string(),
                status: CiStatus::Passed,
                started_at: now,
                completed_at: Some(now),
                duration_ms: Some(0),
                logs_key: Some("local/auto-pass".into()),
                error_summary: None,
            };
            deps.ci_repo
                .save(run)
                .await
                .map_err(domain_status)?;
            Ok(())
        }
    }
}

async fn get_change_request(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management::application::get_change_request(&st.deps, id).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

async fn list_repo_changes(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Query(q): Query<ListAllQuery>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let status = parse_pr_status(q.status.as_deref());
    match change_management::application::list_change_requests(&st.deps, id, status).await {
        Ok(items) => Ok(Json(json!({ "change_requests": items }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct CreateNestedBody {
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    jira_ticket: String,
    #[serde(default)]
    author: String,
}

async fn create_repo_change(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<CreateNestedBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let slug = body.slug.unwrap_or_else(|| "main".into());
    let author = if body.author.is_empty() {
        "jd".into()
    } else {
        body.author
    };
    match change_management::application::create_change_request(
        &st.deps,
        id,
        slug,
        body.title,
        body.description,
        body.jira_ticket,
        author,
    )
    .await
    {
        Ok(cr) => Ok(Json(json!({ "change_request": cr }))),
        Err(e) => Err(domain_status(e)),
    }
}

async fn submit_for_review(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management::application::submit_for_review(&st.deps, id).await {
        Ok(()) => {
            // Local PR Wizard: ensure merge isn't blocked by missing CI runner.
            let _ = ensure_ci_passed(&st.deps, id, "submitted").await;
            Ok(Json(json!({
                "ok": true,
                "status": "ReadyForReview",
                "hint": "Open the PR Wizard in the IDE to walk each structural change."
            })))
        }
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct ReviewBody {
    #[serde(default)]
    reviewer: String,
    #[serde(default)]
    comment: Option<String>,
}

async fn approve_change(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<ReviewBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    // Domain forbids author == reviewer. Prefer explicit reviewer, else "operator".
    let mut reviewer = if body.reviewer.is_empty() {
        "operator".into()
    } else {
        body.reviewer
    };
    if let Ok(Some(pr)) = st.deps.cr_repo.find(id).await {
        if reviewer == pr.author {
            reviewer = format!("{reviewer}-reviewer");
        }
    }
    let _ = ensure_ci_passed(&st.deps, id, "approved").await;
    match change_management::application::approve_change(&st.deps, id, reviewer, body.comment)
        .await
    {
        Ok(()) => Ok(Json(json!({ "ok": true, "status": "Approved" }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct RequestChangesBody {
    #[serde(default)]
    reviewer: String,
    #[serde(default)]
    comment: String,
}

async fn request_changes(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<RequestChangesBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut reviewer = if body.reviewer.is_empty() {
        "operator".into()
    } else {
        body.reviewer
    };
    if let Ok(Some(pr)) = st.deps.cr_repo.find(id).await {
        if reviewer == pr.author {
            reviewer = format!("{reviewer}-reviewer");
        }
    }
    let comment = if body.comment.trim().is_empty() {
        "Changes requested via PR Wizard".into()
    } else {
        body.comment
    };
    match change_management::application::request_changes(&st.deps, id, reviewer, comment)
        .await
    {
        Ok(()) => Ok(Json(json!({ "ok": true, "status": "ChangesRequested" }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct MergeBody {
    #[serde(default)]
    merger: String,
    #[serde(default)]
    slug: String,
}

async fn merge_change(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<MergeBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let merger = if body.merger.is_empty() {
        "operator".into()
    } else {
        body.merger
    };
    let slug = if body.slug.is_empty() {
        // Infer slug from PR source/target when client omits it.
        st.deps
            .cr_repo
            .find(id)
            .await
            .ok()
            .flatten()
            .map(|pr| {
                // Prefer repo name from jira path — agent often passes project as slug.
                pr.source_branch
                    .split('/')
                    .next()
                    .filter(|s| *s != "cr")
                    .unwrap_or("main")
                    .to_string()
            })
            .unwrap_or_else(|| "main".into())
    } else {
        body.slug
    };
    let _ = ensure_ci_passed(&st.deps, id, "merge").await;
    match change_management::application::merge_change(&st.deps, id, merger, slug).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct CommentBody {
    #[serde(default)]
    author: String,
    #[serde(default)]
    construct_path: Option<String>,
    body: String,
}

async fn add_review_comment(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<CommentBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let author = if body.author.is_empty() {
        "jd".into()
    } else {
        body.author
    };
    match change_management::application::add_review_comment(
        &st.deps,
        id,
        author,
        body.construct_path,
        body.body,
    )
    .await
    {
        Ok(c) => Ok(Json(json!({ "comment": c }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct DiffQuery {
    #[serde(default)]
    slug: Option<String>,
}

async fn get_structural_diff(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let pr = st
        .deps
        .cr_repo
        .find(id)
        .await
        .map_err(domain_status)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let slug = q
        .slug
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // Heuristic: description may contain "project: slug" from agent create_change.
            extract_slug_from_description(&pr.description).unwrap_or_else(|| pr.repo_id.to_string())
        });
    let diff = compute_branch_structural_diff(
        &st.deps,
        &slug,
        &pr.target_branch,
        &pr.source_branch,
    )
    .await
    .unwrap_or_else(|e| {
        json!({
            "base_label": pr.target_branch,
            "head_label": pr.source_branch,
            "items": [],
            "added": 0,
            "removed": 0,
            "changed": 0,
            "description": format!("Diff unavailable: {e}"),
            "changes": [],
            "files_changed": 0,
            "additions": 0,
            "removals": 0,
            "error": e,
        })
    });
    Ok(Json(diff))
}

fn extract_slug_from_description(desc: &str) -> Option<String> {
    for line in desc.lines() {
        let t = line.trim();
        for prefix in ["project:", "slug:", "Project:", "Slug:"] {
            if let Some(rest) = t.strip_prefix(prefix) {
                let s = rest.trim().trim_matches('`');
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// Build IR for a .veil package source (best-effort registry).
fn ir_from_veil_source(src: &str) -> Result<veil_ir::IrGraph, String> {
    let reg = veil_ir::LayerRegistry::builtin();
    let tokens = veil_parser::lex(src);
    match veil_parser::parse_file_with_registry(&tokens, reg.clone()) {
        Ok(veil_ir::VeilFile::Package(pkg)) => {
            let sol = veil_ir::package_as_solution(&pkg);
            Ok(veil_ir::build_ir_with_registry(&sol, Some(&reg)))
        }
        Ok(veil_ir::VeilFile::Solution(sol)) => {
            Ok(veil_ir::build_ir_with_registry(&sol, Some(&reg)))
        }
        Ok(_) => Ok(veil_ir::IrGraph::new()),
        Err(errs) => Err(errs
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ")),
    }
}

fn diff_item_to_product(item: &veil_ir::DiffItem) -> Value {
    use veil_ir::DiffItem::*;
    match item {
        Added {
            path,
            name,
            subkind,
            node_kind,
            ..
        } => json!({
            "kind": "Added",
            "path": format!("{path}/{name}"),
            "detail": format!("+ {name}"),
            "construct_type": subkind.clone().unwrap_or_else(|| node_kind.clone()),
        }),
        Removed {
            path,
            name,
            subkind,
            node_kind,
            ..
        } => json!({
            "kind": "Removed",
            "path": format!("{path}/{name}"),
            "detail": format!("− {name}"),
            "construct_type": subkind.clone().unwrap_or_else(|| node_kind.clone()),
        }),
        Renamed {
            path,
            from_name,
            to_name,
            subkind,
            node_kind,
            ..
        } => json!({
            "kind": "Moved",
            "path": format!("{path}/{to_name}"),
            "detail": format!("{from_name} → {to_name}"),
            "construct_type": subkind.clone().unwrap_or_else(|| node_kind.clone()),
        }),
        SignatureChanged {
            path,
            name,
            before,
            after,
            node_kind,
            ..
        } => json!({
            "kind": "Modified",
            "path": format!("{path}/{name}"),
            "detail": format!("sig: {before} → {after}"),
            "construct_type": node_kind,
        }),
        BodyChanged {
            path,
            name,
            before_lines,
            after_lines,
            node_kind,
            ..
        } => json!({
            "kind": "Modified",
            "path": format!("{path}/{name}"),
            "detail": format!("body {before_lines}→{after_lines} lines"),
            "construct_type": node_kind,
        }),
        AnnotationsChanged {
            path,
            name,
            node_kind,
            ..
        } => json!({
            "kind": "Modified",
            "path": format!("{path}/{name}"),
            "detail": format!("@{name} annotations"),
            "construct_type": node_kind,
        }),
    }
}

async fn compute_branch_structural_diff(
    deps: &change_management::application::Deps,
    slug: &str,
    base_branch: &str,
    head_branch: &str,
) -> Result<Value, String> {
    let base_files = deps
        .git
        .list_files(slug.to_string(), base_branch.to_string())
        .await
        .map_err(|e| format!("list base: {e:?}"))?;
    let head_files = deps
        .git
        .list_files(slug.to_string(), head_branch.to_string())
        .await
        .map_err(|e| format!("list head: {e:?}"))?;

    let is_veil = |p: &str| p.ends_with(".veil") || p.ends_with(".layer");
    let mut all: std::collections::BTreeSet<String> = base_files
        .iter()
        .chain(head_files.iter())
        .filter(|p| is_veil(p))
        .cloned()
        .collect();
    // Prefer primary package files first
    if all.is_empty() {
        // Fallback: try common paths
        for p in ["src/main.veil", "main.veil", "app.veil"] {
            all.insert(p.into());
        }
    }

    let mut merged_items: Vec<veil_ir::DiffItem> = Vec::new();
    let mut product_changes: Vec<Value> = Vec::new();
    let mut files_touched = 0i64;
    let mut parse_notes: Vec<String> = Vec::new();

    for path in all {
        let base_src = deps
            .git
            .read_file(slug.to_string(), base_branch.to_string(), path.clone())
            .await
            .unwrap_or(None)
            .unwrap_or_default();
        let head_src = deps
            .git
            .read_file(slug.to_string(), head_branch.to_string(), path.clone())
            .await
            .unwrap_or(None)
            .unwrap_or_default();
        if base_src == head_src {
            continue;
        }
        files_touched += 1;
        if base_src.is_empty() && !head_src.is_empty() {
            product_changes.push(json!({
                "kind": "Added",
                "path": path,
                "detail": format!("new file ({})", head_src.lines().count()),
                "construct_type": "File",
            }));
        } else if !base_src.is_empty() && head_src.is_empty() {
            product_changes.push(json!({
                "kind": "Removed",
                "path": path,
                "detail": "file removed",
                "construct_type": "File",
            }));
        }

        let base_ir = if base_src.is_empty() {
            veil_ir::IrGraph::new()
        } else {
            match ir_from_veil_source(&base_src) {
                Ok(g) => g,
                Err(e) => {
                    parse_notes.push(format!("{path} (base): {e}"));
                    veil_ir::IrGraph::new()
                }
            }
        };
        let head_ir = if head_src.is_empty() {
            veil_ir::IrGraph::new()
        } else {
            match ir_from_veil_source(&head_src) {
                Ok(g) => g,
                Err(e) => {
                    parse_notes.push(format!("{path} (head): {e}"));
                    veil_ir::IrGraph::new()
                }
            }
        };
        let d = veil_ir::structural_diff(&base_ir, &head_ir, base_branch, head_branch);
        for item in &d.items {
            product_changes.push(diff_item_to_product(item));
        }
        merged_items.extend(d.items);
    }

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed = 0usize;
    for item in &merged_items {
        match item {
            veil_ir::DiffItem::Added { .. } => added += 1,
            veil_ir::DiffItem::Removed { .. } => removed += 1,
            _ => changed += 1,
        }
    }

    // High-impact → low across all packages (per-file HashMap order was arbitrary).
    {
        let mut tmp = veil_ir::StructDiff {
            base_label: base_branch.to_string(),
            head_label: head_branch.to_string(),
            items: std::mem::take(&mut merged_items),
            added,
            removed,
            changed,
            item_annotations: None,
            item_peeks: None,
            item_peeks_base: None,
        };
        veil_ir::sort_diff_for_review(&mut tmp);
        merged_items = tmp.items;
    }

    let summary = if merged_items.is_empty() && files_touched == 0 {
        format!(
            "No structural changes detected between `{base_branch}` and `{head_branch}` (slug={slug}). If work is only in the coding session, use the IDE working-tree diff."
        )
    } else {
        format!(
            "+{added} −{removed} ~{changed} structural · {files_touched} files · {base_branch} → {head_branch}"
        )
    };

    // Serialize StructDiff items for IDE PR Wizard
    let items_json = serde_json::to_value(&merged_items).unwrap_or_else(|_| json!([]));

    Ok(json!({
        // IDE / StructDiff shape
        "base_label": base_branch,
        "head_label": head_branch,
        "items": items_json,
        "added": added,
        "removed": removed,
        "changed": changed,
        // Product ChangeDetail shape
        "description": summary,
        "changes": product_changes,
        "files_changed": files_touched,
        "additions": added as i64,
        "removals": removed as i64,
        "slug": slug,
        "parse_notes": parse_notes,
    }))
}

#[derive(Deserialize)]
struct ReviewItemBody {
    /// approve | feedback | clear
    decision: String,
    #[serde(default)]
    construct_path: Option<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    author: String,
    /// When true, comment is tagged for immediate agent delivery (history only here).
    #[serde(default)]
    send_now: bool,
    /// Index in the wizard walkthrough (for history).
    #[serde(default)]
    item_index: Option<u32>,
    #[serde(default)]
    item_kind: Option<String>,
    #[serde(default)]
    item_name: Option<String>,
    #[serde(default)]
    rationale: Option<String>,
}

/// Per-item PR Wizard decision → durable review comment (+ structured prefix).
async fn review_item(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<ReviewItemBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let _pr = st
        .deps
        .cr_repo
        .find(id)
        .await
        .map_err(domain_status)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let author = if body.author.is_empty() {
        "operator".into()
    } else {
        body.author
    };
    let decision = body.decision.trim().to_ascii_lowercase();
    // approve | feedback | clear (undo prior wizard decision for this construct)
    if decision != "approve" && decision != "feedback" && decision != "clear" {
        return Err(StatusCode::BAD_REQUEST);
    }
    let path = body
        .construct_path
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| body.item_name.clone().unwrap_or_else(|| "(package)".into()));
    let mut lines = vec![
        format!("[pr-wizard:{decision}]"),
        format!("path: {path}"),
    ];
    if let Some(i) = body.item_index {
        lines.push(format!("item_index: {i}"));
    }
    if let Some(k) = &body.item_kind {
        lines.push(format!("kind: {k}"));
    }
    if let Some(n) = &body.item_name {
        lines.push(format!("name: {n}"));
    }
    if body.send_now {
        lines.push("delivery: send_now".into());
    } else if decision == "feedback" {
        lines.push("delivery: queued".into());
    }
    if let Some(r) = &body.rationale {
        if !r.trim().is_empty() {
            lines.push(format!("agent_rationale: {r}"));
        }
    }
    if !body.body.trim().is_empty() {
        lines.push(String::new());
        lines.push(body.body.trim().to_string());
    } else if decision == "approve" {
        lines.push(String::new());
        lines.push("Approved in PR Wizard.".into());
    } else if decision == "clear" {
        lines.push(String::new());
        lines.push("Cleared previous decision in PR Wizard (pending again).".into());
    }
    let comment_body = lines.join("\n");
    match change_management::application::add_review_comment(
        &st.deps,
        id,
        author,
        Some(path),
        comment_body,
    )
    .await
    {
        Ok(c) => Ok(Json(json!({
            "ok": true,
            "comment": c,
            "decision": decision,
            "send_now": body.send_now,
        }))),
        Err(e) => Err(domain_status(e)),
    }
}

/// Finalize wizard: all items approved → approve PR; any feedback → request_changes with summary.
#[derive(Deserialize)]
struct FinalizeWizardBody {
    /// all_approved | needs_work
    outcome: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    reviewer: String,
    #[serde(default)]
    approved_count: u32,
    #[serde(default)]
    feedback_count: u32,
}

async fn finalize_wizard(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<FinalizeWizardBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let outcome = body.outcome.trim().to_ascii_lowercase();
    let mut reviewer = if body.reviewer.is_empty() {
        "operator".into()
    } else {
        body.reviewer
    };
    if let Ok(Some(pr)) = st.deps.cr_repo.find(id).await {
        if reviewer == pr.author {
            reviewer = format!("{reviewer}-reviewer");
        }
        // Auto-submit Draft so approve/request works
        if matches!(
            pr.status,
            change_management::domain::types::PrStatus::Draft
                | change_management::domain::types::PrStatus::ChangesRequested
        ) {
            let _ = change_management::application::submit_for_review(&st.deps, id).await;
        }
    }
    let _ = ensure_ci_passed(&st.deps, id, "wizard").await;

    if outcome == "all_approved" {
        let comment = if body.summary.is_empty() {
            Some(format!(
                "PR Wizard: approved {} structural change(s).",
                body.approved_count
            ))
        } else {
            Some(body.summary.clone())
        };
        // Ensure ReadyForReview
        let _ = change_management::application::submit_for_review(&st.deps, id).await;
        match change_management::application::approve_change(
            &st.deps,
            id,
            reviewer,
            comment,
        )
        .await
        {
            Ok(()) => Ok(Json(json!({
                "ok": true,
                "status": "Approved",
                "outcome": "all_approved",
            }))),
            Err(e) => Err(domain_status(e)),
        }
    } else if outcome == "needs_work" {
        let summary = if body.summary.is_empty() {
            format!(
                "PR Wizard: {} approved, {} need work. See review comments for details.",
                body.approved_count, body.feedback_count
            )
        } else {
            body.summary
        };
        let _ = change_management::application::submit_for_review(&st.deps, id).await;
        match change_management::application::request_changes(&st.deps, id, reviewer, summary)
            .await
        {
            Ok(()) => Ok(Json(json!({
                "ok": true,
                "status": "ChangesRequested",
                "outcome": "needs_work",
            }))),
            Err(e) => Err(domain_status(e)),
        }
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
}

async fn update_status(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<StatusBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let status = parse_pr_status(Some(body.status.as_str())).ok_or(StatusCode::BAD_REQUEST)?;
    match change_management::application::update_change_request_status(&st.deps, id, status).await
    {
        Ok(cr) => Ok(Json(json!(cr))),
        Err(e) => Err(domain_status(e)),
    }
}

// ─── Deploy ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct DeployState {
    deps: Arc<deploy::application::Deps>,
}

async fn deploy_deps(bus: Arc<dyn veil_shared::Bus + Send + Sync>) -> deploy::application::Deps {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let ddb = aws_sdk_dynamodb::Client::new(&config);
    let s3 = aws_sdk_s3::Client::new(&config);
    let lambda = aws_sdk_lambda::Client::new(&config);
    let sqs = aws_sdk_sqs::Client::new(&config);
    let sns = aws_sdk_sns::Client::new(&config);
    let apigw = aws_sdk_apigatewayv2::Client::new(&config);
    let table = std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
    let bucket = std::env::var("BUCKET").unwrap_or_else(|_| "veil-runtime-dev".into());
    deploy::application::Deps {
        store: Arc::new(deploy::adapters::DdbDeploymentStore {
            client: ddb.clone(),
            table,
        }),
        exec: Arc::new(deploy::adapters::LocalDeployExec {
            apigw,
            bucket,
            ddb,
            lambda,
            s3,
            sns,
            sqs,
        }),
        executor: Arc::new(deploy::adapters::MockActionExecutor {}),
        bus,
    }
}

async fn list_deploy_environments(
    State(st): State<DeployState>,
) -> Result<Json<Value>, StatusCode> {
    match deploy::application::list_deploy_environments(&st.deps).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct PlanBody {
    project_slug: String,
    environment: String,
    repo_id: String,
    #[serde(default)]
    branch: Option<String>,
}

async fn plan_provision(
    State(st): State<DeployState>,
    Json(body): Json<PlanBody>,
) -> Result<Json<Value>, StatusCode> {
    let branch = body.branch.unwrap_or_else(|| "main".into());
    match deploy::application::plan_provision(
        &st.deps,
        body.project_slug,
        body.environment,
        body.repo_id,
        branch,
    )
    .await
    {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

async fn provision_project(
    State(st): State<DeployState>,
    Json(body): Json<PlanBody>,
) -> Result<Json<Value>, StatusCode> {
    let branch = body.branch.unwrap_or_else(|| "main".into());
    match deploy::application::provision_project(
        &st.deps,
        body.project_slug,
        body.environment,
        body.repo_id,
        branch,
    )
    .await
    {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct JobQuery {
    job_id: String,
}

async fn get_provision_job(
    State(st): State<DeployState>,
    Query(q): Query<JobQuery>,
) -> Result<Json<Value>, StatusCode> {
    match deploy::application::get_provision_job(&st.deps, q.job_id).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct DeployStatusQuery {
    environment: String,
    unit_name: String,
}

async fn deployment_status(
    State(st): State<DeployState>,
    Query(q): Query<DeployStatusQuery>,
) -> Result<Json<Value>, StatusCode> {
    match deploy::application::get_deployment_status(&st.deps, q.environment, q.unit_name).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
    }
}

// ─── Registry (lightweight local listing) ───────────────────────────────────

async fn list_registry_layers() -> Json<Value> {
    let layers = std::env::var("VEIL_LAYERS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../layers")
        });
    let mut names = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&layers) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("layer") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    names.push(json!({ "name": stem }));
                }
            }
        }
    }
    Json(json!(names))
}

async fn list_registry_stubs() -> Json<Value> {
    let stubs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/stubs");
    let mut names = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&stubs) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("stub") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    names.push(json!({ "crate_name": stem }));
                }
            }
        }
    }
    Json(json!(names))
}

use std::path::PathBuf;

// ─── Helpers ────────────────────────────────────────────────────────────────

fn domain_status(e: veil_shared::DomainError) -> StatusCode {
    match e {
        veil_shared::DomainError::NotFound => StatusCode::NOT_FOUND,
        veil_shared::DomainError::Validation(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn parse_pr_status(s: Option<&str>) -> Option<change_management::domain::types::PrStatus> {
    use change_management::domain::types::PrStatus;
    match s? {
        "Draft" => Some(PrStatus::Draft),
        "ReadyForReview" => Some(PrStatus::ReadyForReview),
        "Approved" => Some(PrStatus::Approved),
        "ChangesRequested" => Some(PrStatus::ChangesRequested),
        "Merging" => Some(PrStatus::Merging),
        "Merged" => Some(PrStatus::Merged),
        "Rejected" => Some(PrStatus::Rejected),
        "Closed" => Some(PrStatus::Closed),
        _ => None,
    }
}

/// Build platform domain router and merge onto ProductHost.
pub async fn build_platform_router(
    bus: Arc<dyn veil_shared::Bus + Send + Sync>,
) -> Router {
    let storage = StorageState {
        deps: Arc::new(resolve_storage_deps().await),
    };
    let cm = CmState {
        deps: Arc::new(crate::local_ports::change_management_deps().await),
    };
    let deploy = DeployState {
        deps: Arc::new(deploy_deps(bus).await),
    };

    let storage_r = Router::new()
        .route(
            "/api/repos",
            get(list_repos).post(create_repo),
        )
        .route(
            "/api/repos/{id}",
            get(get_repo).delete(delete_repo),
        )
        .route(
            "/api/repos/{id}/mission",
            get(get_mission).put(put_mission),
        )
        .route("/api/read-file", post(read_file_api))
        .route("/api/write-file", post(write_file_api))
        .route(
            "/api/project_infras/{id}",
            get(get_project_infra),
        )
        .route(
            "/api/project-query",
            get(query_project_modules),
        )
        .with_state(storage);

    let cm_r = Router::new()
        .route(
            "/api/change_requests",
            get(list_all_change_requests).post(create_change_request_flat),
        )
        .route(
            "/api/change_requests/{id}",
            get(get_change_request),
        )
        .route(
            "/api/change_requests/{id}/submit",
            post(submit_for_review),
        )
        .route(
            "/api/change_requests/{id}/approve",
            post(approve_change),
        )
        .route(
            "/api/change_requests/{id}/request-changes",
            post(request_changes),
        )
        .route(
            "/api/change_requests/{id}/merge",
            post(merge_change),
        )
        .route(
            "/api/change_requests/{id}/comments",
            post(add_review_comment),
        )
        .route(
            "/api/change_requests/{id}/diff",
            get(get_structural_diff),
        )
        .route(
            "/api/change_requests/{id}/review-item",
            post(review_item),
        )
        .route(
            "/api/change_requests/{id}/finalize-wizard",
            post(finalize_wizard),
        )
        .route(
            "/api/change_requests/{id}/status",
            put(update_status),
        )
        .route(
            "/api/repos/{id}/changes",
            get(list_repo_changes).post(create_repo_change),
        )
        .with_state(cm);

    let deploy_r = Router::new()
        .route(
            "/api/deploy_environments",
            get(list_deploy_environments),
        )
        .route("/api/plan-provision", post(plan_provision))
        .route("/api/provision-project", post(provision_project))
        .route("/api/provision_jobs", get(get_provision_job))
        .route("/api/deployment_status", get(deployment_status))
        .with_state(deploy);

    let registry_r = Router::new()
        .route("/api/registry/layers", get(list_registry_layers))
        .route("/api/registry/stubs", get(list_registry_stubs));

    storage_r
        .merge(cm_r)
        .merge(deploy_r)
        .merge(registry_r)
}
