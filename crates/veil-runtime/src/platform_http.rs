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
    match storage::application::resolve_repo(&st.deps, &id).await {
        Ok(repo) => Ok(Json(json!(repo))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct UpdateRepoBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    clear_description: bool,
}

async fn update_repo(
    State(st): State<StorageState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRepoBody>,
) -> Result<Json<Value>, StatusCode> {
    if body.name.is_none() && body.slug.is_none() && body.description.is_none() && !body.clear_description
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    match storage::application::update_repo(
        &st.deps,
        id,
        body.name,
        body.slug,
        body.description,
        body.clear_description,
    )
    .await
    {
        Ok(repo) => Ok(Json(json!(repo))),
        Err(e) => Err(domain_status(e)),
    }
}

async fn delete_repo(
    State(st): State<StorageState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let resolved = storage::application::get_repo(&st.deps, id.clone())
        .await
        .ok();
    match storage::application::delete_repo(&st.deps, id).await {
        Ok(()) => {
            if let Some(repo) = resolved {
                let rid = repo.id.value.clone();
                let slug = repo.slug.clone();
                let n = veil_server::review::close_for_deleted_project(&slug, Some(&rid));
                if n > 0 {
                    tracing::info!(%slug, repo_id = %rid, closed = n, "closed outstanding review items after delete");
                }
                veil_server::provider::s3_workspace::invalidate_identity(
                    Some(&slug),
                    Some(&rid),
                );
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = veil_server::provider::s3_workspace::purge_repo_store(&rid) {
                        tracing::warn!(repo_id = %rid, slug = %slug, error = %e, "purge_repo_store failed");
                    }
                });
            }
            Ok(Json(json!({ "ok": true })))
        }
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

async fn list_all_pull_requests(
    State(st): State<CmState>,
    Query(q): Query<ListAllQuery>,
) -> Result<Json<Value>, StatusCode> {
    let status = parse_pr_status(q.status.as_deref());
    match change_management::application::list_all_pull_requests(&st.deps, status).await {
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

async fn create_pull_request_flat(
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
    // Prefer an explicit *feature* work branch from the coding session.
    // Never overwrite the freshly created PR branch with `main`/`master` —
    // that makes merge a no-op and blocks session publish-to-branch.
    let preferred_branch = body
        .source_branch
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| {
            let l = s.to_ascii_lowercase();
            l != "main" && l != "master"
        })
        .map(|s| s.to_string());

    let mut description = body.description.clone();
    if extract_slug_from_description(&description).is_none() {
        description.push_str(&format!("\nslug: {slug}\n"));
    }

    match change_management::application::create_pull_request_flat(
        &st.deps,
        repo_id,
        slug.clone(),
        body.title.clone(),
        description.clone(),
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
                    if let Err(e) = st.deps.pr_repo.save(cr.clone()).await {
                        return Err(domain_status(e));
                    }
                }
            }
            let _ = ensure_ci_passed(&st.deps, cr.id, "pending", Some(slug.as_str())).await;
            let _ = veil_server::review::record_pr(&slug, &cr.title, Some(&cr.id.to_string()));
            Ok(Json(json!({
                "pull_request": cr,
                "slug": slug,
                "wizard_path": format!("/pulls/{}", cr.id),
            })))
        }
        Err(e) => {
            // Soft path: if git branch create failed, still persist a CR for PR Wizard.
            tracing::warn!(?e, "create_pull_request_flat failed — soft-creating META only");
            use change_management::domain::types::{PullRequest, PrStatus};
            let now = chrono::Utc::now();
            let pr_id = Uuid::new_v4();
            let source = preferred_branch.unwrap_or_else(|| {
                format!(
                    "pr/{}/{}",
                    jira,
                    body.title.to_lowercase().replace(' ', "-")
                )
            });
            let cr = PullRequest {
                id: pr_id,
                repo_id,
                title: body.title,
                description,
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
                .pr_repo
                .save(cr.clone())
                .await
                .map_err(domain_status)?;
            let _ = ensure_ci_passed(&st.deps, cr.id, "pending", Some(slug.as_str())).await;
            Ok(Json(json!({
                "pull_request": cr,
                "slug": slug,
                "wizard_path": format!("/pulls/{}", cr.id),
                "soft_create": true,
            })))
        }
    }
}

/// Record CI for a PR.
///
/// - `VEIL_DEV=1`: invent a Passed run if none exists (local toy).
/// - Production-shaped: write a run from the last host check (errors → Failed).
///   Opening a draft (`pending`) does not invent a run.
async fn ensure_ci_passed(
    deps: &change_management::application::Deps,
    pr_id: Uuid,
    commit_hash: &str,
    slug: Option<&str>,
) -> Result<(), StatusCode> {
    use change_management::domain::types::{CiRun, CiStatus};
    if let Ok(Some(run)) = deps.ci_repo.latest_for_pr(pr_id).await {
        if matches!(run.status, CiStatus::Passed) {
            return Ok(());
        }
    }
    if veil_server::review::veil_dev_enabled() {
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
        deps.ci_repo.save(run).await.map_err(domain_status)?;
        return Ok(());
    }
    if commit_hash == "pending" {
        return Ok(());
    }
    let check = slug
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| veil_server::coding_gates::peek_project_session(Some(s)))
        .and_then(|h| h.snapshot_meta().last_host_check);
    let now = chrono::Utc::now();
    let (status, logs_key, error_summary) = match check {
        Some(h) if h.error_count > 0 || h.severity == "errors" => (
            CiStatus::Failed,
            Some("host-check".into()),
            Some(h.summary),
        ),
        Some(h) => (
            CiStatus::Passed,
            Some(format!("host-check:{}", h.severity)),
            None,
        ),
        None => (
            CiStatus::Failed,
            Some("host-check".into()),
            Some("no host check recorded".into()),
        ),
    };
    let run = CiRun {
        id: Uuid::new_v4(),
        pr_id,
        commit_hash: commit_hash.to_string(),
        status,
        started_at: now,
        completed_at: Some(now),
        duration_ms: Some(0),
        logs_key,
        error_summary,
    };
    deps.ci_repo.save(run).await.map_err(domain_status)?;
    Ok(())
}

async fn get_pull_request(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management::application::get_pull_request(&st.deps, id).await {
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
    match change_management::application::list_pull_requests(&st.deps, id, status).await {
        Ok(items) => Ok(Json(json!({ "pull_requests": items }))),
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
    match change_management::application::create_pull_request(
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
        Ok(cr) => Ok(Json(json!({ "pull_request": cr }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct SubmitQuery {
    /// When true, allow ReadyForReview even with empty structural/file diff.
    #[serde(default)]
    force: bool,
    #[serde(default)]
    slug: Option<String>,
}

async fn submit_for_review(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Query(q): Query<SubmitQuery>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    // Gate empty structural walks — ReadyForReview with nothing to review is cruft.
    if !q.force {
        if let Ok(Some(pr)) = st.deps.pr_repo.find(id).await {
            let slug = q
                .slug
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| extract_slug_from_description(&pr.description))
                .unwrap_or_else(|| pr.repo_id.to_string());
            if let Ok(diff) = compute_branch_structural_diff(
                &st.deps,
                &slug,
                &pr.target_branch,
                &pr.source_branch,
            )
            .await
            {
                let items = diff
                    .get("items")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let files = diff
                    .get("files_changed")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if items == 0 && files == 0 {
                    return Ok(Json(json!({
                        "ok": false,
                        "error": "empty_diff",
                        "status": format!("{:?}", pr.status),
                        "message": format!(
                            "No structural or file changes between `{}` and `{}` for slug `{slug}`. \
Publish the coding session to the PR branch before submit, or pass force=1 to override.",
                            pr.target_branch, pr.source_branch
                        ),
                        "hint": "session_publish / create_pr after real edits; check product slug git root.",
                    })));
                }
            }
        }
    }
    match change_management::application::submit_for_review(&st.deps, id).await {
        Ok(()) => {
            let slug = st
                .deps
                .pr_repo
                .find(id)
                .await
                .ok()
                .flatten()
                .and_then(|p| extract_slug_from_description(&p.description));
            let _ = ensure_ci_passed(&st.deps, id, "submitted", slug.as_deref()).await;
            Ok(Json(json!({
                "ok": true,
                "status": "ReadyForReview",
                "hint": "Open Review to walk the change set. Approve is the ship gate.",
                "audit_env": veil_server::review::audit_env_json(),
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

async fn approve_pr(
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
    let mut approve_slug = None;
    if let Ok(Some(pr)) = st.deps.pr_repo.find(id).await {
        if reviewer == pr.author {
            reviewer = format!("{reviewer}-reviewer");
        }
        approve_slug = extract_slug_from_description(&pr.description);
    }
    let _ = ensure_ci_passed(&st.deps, id, "approved", approve_slug.as_deref()).await;
    match change_management::application::approve_pr(&st.deps, id, reviewer, body.comment)
        .await
    {
        Ok(()) => Ok(Json(json!({
            "ok": true,
            "status": "Approved",
            "audit_env": veil_server::review::audit_env_json(),
        }))),
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

async fn request_pr_changes(
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
    if let Ok(Some(pr)) = st.deps.pr_repo.find(id).await {
        if reviewer == pr.author {
            reviewer = format!("{reviewer}-reviewer");
        }
    }
    let comment = if body.comment.trim().is_empty() {
        "Changes requested via PR Wizard".into()
    } else {
        body.comment
    };
    match change_management::application::request_pr_changes(&st.deps, id, reviewer, comment)
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

async fn merge_pr(
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
    let pr = st
        .deps
        .pr_repo
        .find(id)
        .await
        .map_err(domain_status)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let slug = if body.slug.is_empty() {
        extract_slug_from_description(&pr.description)
            .unwrap_or_else(|| pr.repo_id.to_string())
    } else {
        body.slug
    };
    let source = pr.source_branch.clone();
    let target = pr.target_branch.clone();
    if source == target
        || source.eq_ignore_ascii_case("main")
        || source.eq_ignore_ascii_case("master")
    {
        return Ok(Json(json!({
            "ok": false,
            "error": "invalid_merge_branches",
            "message": format!(
                "Cannot merge `{source}` → `{target}`. This PR has no distinct feature branch \
(often created while the session was on main without publish). Re-open Review, Approve again \
so a `cr/…` branch is created and the session is published, then Merge."
            ),
            "source_branch": source,
            "target_branch": target,
            "hint": "create_pr + publish-branch to a non-main source, then merge.",
        })));
    }
    if let Err(e) = veil_server::review::may_ship(&slug, None) {
        return Ok(Json(json!({
            "ok": false,
            "error": "sign_off_required",
            "message": e,
            "hint": "Open Review, walk the change set, and press Approve. That record is the ship gate.",
            "audit_env": veil_server::review::audit_env_json(),
        })));
    }
    let _ = ensure_ci_passed(&st.deps, id, "merge", Some(slug.as_str())).await;
    match change_management::application::merge_pr(&st.deps, id, merger, slug.clone()).await {
        Ok(mut v) => {
            if veil_server::git_origin::origin_enabled() {
                let origin = veil_server::git_origin::GitOrigin::new(pr.repo_id.to_string());
                if origin.exists() {
                    let tmp = std::env::temp_dir().join(format!("veil-git-merge-{}", pr.repo_id));
                    let _ = std::fs::remove_dir_all(&tmp);
                    match origin.merge_and_push(&tmp, &source, &target) {
                        Ok(sha) => {
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert("merge_commit".into(), serde_json::json!(sha));
                                obj.insert("via".into(), serde_json::json!("git"));
                            }
                        }
                        Err(e) => {
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert("git_merge_error".into(), serde_json::json!(e));
                            }
                            tracing::error!(error = %e, "git origin merge failed");
                        }
                    }
                    let _ = std::fs::remove_dir_all(&tmp);
                    return Ok(Json(v));
                }
            }
            // Legacy: tree copy when no git origin exists yet.
            let promoted = promote_branch_trees(&slug, &pr.repo_id.to_string(), &source, &target).await;
            if let Some(obj) = v.as_object_mut() {
                obj.insert("file_promote".into(), promoted);
            }
            Ok(Json(v))
        }
        Err(e) => Err(domain_status(e)),
    }
}

/// Copy product file trees source → target in S3 (repo_id and/or slug prefixes).
async fn promote_branch_trees(
    slug: &str,
    repo_id: &str,
    source: &str,
    target: &str,
) -> Value {
    let bucket = std::env::var("BUCKET")
        .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
        .unwrap_or_else(|_| "veil-runtime-dev".into());
    let mut results = Vec::new();
    for key in [repo_id, slug] {
        if key.is_empty() || key == "main" {
            continue;
        }
        let src = format!("s3://{bucket}/repos/{key}/{source}/");
        let dst = format!("s3://{bucket}/repos/{key}/{target}/");
        let mut cmd = std::process::Command::new("aws");
        if let Ok(p) = std::env::var("AWS_PROFILE") {
            cmd.env("AWS_PROFILE", p);
        }
        let out = cmd
            .args(["s3", "sync", &src, &dst, "--only-show-errors"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                results.push(json!({ "prefix": key, "ok": true, "src": src, "dst": dst }));
            }
            Ok(o) => {
                results.push(json!({
                    "prefix": key,
                    "ok": false,
                    "stderr": String::from_utf8_lossy(&o.stderr).to_string(),
                }));
            }
            Err(e) => {
                results.push(json!({ "prefix": key, "ok": false, "error": e.to_string() }));
            }
        }
    }
    json!({ "promotions": results })
}

#[derive(Deserialize)]
struct CommentBody {
    #[serde(default)]
    author: String,
    #[serde(default)]
    construct_path: Option<String>,
    body: String,
}

/// GET /api/pull_requests/{id}/comments — same comments as the composite GET detail.
async fn list_review_comments(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    pr_detail_field(&st, &id, "comments").await
}

/// GET /api/pull_requests/{id}/approvals — embedded on GET /{id}; also a first-class GET.
async fn list_pr_approvals(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    pr_detail_field(&st, &id, "approvals").await
}

/// GET /api/pull_requests/{id}/ci — first CI run from composite `ci_runs`.
async fn get_pr_ci(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let detail = pr_detail_json(&st, &id).await?;
    let runs = detail.get("ci_runs").cloned().unwrap_or(Value::Array(vec![]));
    let first = runs.as_array().and_then(|a| a.first()).cloned().unwrap_or(json!({}));
    Ok(Json(first))
}

async fn pr_detail_json(st: &CmState, id: &str) -> Result<Value, StatusCode> {
    let id = Uuid::parse_str(id).map_err(|_| StatusCode::BAD_REQUEST)?;
    change_management::application::get_pull_request(&st.deps, id)
        .await
        .map_err(domain_status)
}

async fn pr_detail_field(st: &CmState, id: &str, field: &str) -> Result<Json<Value>, StatusCode> {
    let detail = pr_detail_json(st, id).await?;
    Ok(Json(
        detail
            .get(field)
            .cloned()
            .unwrap_or(Value::Array(vec![])),
    ))
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
        .pr_repo
        .find(id)
        .await
        .map_err(domain_status)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let slug = q
        .slug
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // Heuristic: description may contain "project: slug" from agent create_pr.
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

fn collect_veil_rels(root: &std::path::Path, out: &mut std::collections::BTreeSet<String>) {
    fn rec(root: &std::path::Path, dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if matches!(name.as_str(), ".git" | "target" | "generated" | "node_modules") {
                    continue;
                }
                rec(root, &p, out);
            } else if name.ends_with(".veil") || name.ends_with(".layer") {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    rec(root, root, out);
}

fn compute_diff_from_git_origin(
    origin: &veil_server::git_origin::GitOrigin,
    base_branch: &str,
    head_branch: &str,
) -> Result<Value, String> {
    let base_dir = origin.checkout_tmp(base_branch)?;
    let head_dir = origin.checkout_tmp(head_branch)?;
    let patch = origin
        .unified_diff_refs(base_branch, head_branch)
        .unwrap_or_default();
    let mut all = std::collections::BTreeSet::new();
    collect_veil_rels(&base_dir, &mut all);
    collect_veil_rels(&head_dir, &mut all);
    let mut file_diffs = Vec::new();
    let mut product_changes = Vec::new();
    let mut merged_items: Vec<veil_ir::DiffItem> = Vec::new();
    let mut merged_base = veil_ir::IrGraph::new();
    let mut merged_head = veil_ir::IrGraph::new();
    let mut used_layers = std::collections::BTreeSet::new();
    let mut parse_notes = Vec::new();
    let mut files_touched = 0i64;
    for path in all {
        let base_src = std::fs::read_to_string(base_dir.join(&path)).unwrap_or_default();
        let head_src = std::fs::read_to_string(head_dir.join(&path)).unwrap_or_default();
        if base_src == head_src {
            continue;
        }
        files_touched += 1;
        let status = if base_src.is_empty() {
            "added"
        } else if head_src.is_empty() {
            "removed"
        } else {
            "modified"
        };
        file_diffs.push(json!({
            "path": path,
            "status": status,
            "hunks": unified_hunks(&base_src, &head_src, 3),
            "base_lines": base_src.lines().count(),
            "head_lines": head_src.lines().count(),
        }));
        for line in head_src.lines().chain(base_src.lines()) {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("use ") {
                let dep = rest.split_whitespace().next().unwrap_or("").trim();
                if !dep.is_empty() {
                    used_layers.insert(dep.to_string());
                }
            }
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
        merge_ir_graph(&mut merged_base, &base_ir);
        merge_ir_graph(&mut merged_head, &head_ir);
        let d = veil_ir::structural_diff(&base_ir, &head_ir, base_branch, head_branch);
        for item in &d.items {
            product_changes.push(diff_item_to_product(item));
        }
        merged_items.extend(d.items);
    }
    let _ = std::fs::remove_dir_all(&base_dir);
    let _ = std::fs::remove_dir_all(&head_dir);
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
        item_impact: None,
    };
    veil_ir::enrich_diff_peeks(&mut tmp, &merged_base, &merged_head);
    veil_ir::enrich_diff_impact(&mut tmp, &merged_head, &merged_base);
    veil_ir::sort_diff_for_review(&mut tmp);
    let review_policies = resolve_review_policies_for_layers(
        used_layers.iter().map(|s| s.as_str()).collect(),
    );
    Ok(json!({
        "via": "git",
        "git_patch": patch,
        "base_label": base_branch,
        "head_label": head_branch,
        "items": tmp.items,
        "added": added,
        "removed": removed,
        "changed": changed,
        "changes": product_changes,
        "file_diffs": file_diffs,
        "files_changed": files_touched,
        "description": format!(
            "git diff {base_branch}...{head_branch} · +{added} −{removed} ~{changed} · {files_touched} files"
        ),
        "parse_notes": parse_notes,
        "review_policies": review_policies,
        "item_peeks": tmp.item_peeks,
        "item_impact": tmp.item_impact,
    }))
}

async fn compute_branch_structural_diff(
    deps: &change_management::application::Deps,
    slug: &str,
    base_branch: &str,
    head_branch: &str,
) -> Result<Value, String> {
    if veil_server::git_origin::origin_enabled() {
        if let Ok(rid) = veil_server::provider::s3_workspace::resolve_repo_id(slug) {
            let rid_b = rid.clone();
            let base_b = base_branch.to_string();
            let head_b = head_branch.to_string();
            match tokio::task::spawn_blocking(move || {
                let origin = veil_server::git_origin::GitOrigin::new(&rid_b);
                if !origin.exists() {
                    return None;
                }
                Some(compute_diff_from_git_origin(&origin, &base_b, &head_b))
            })
            .await
            {
                Ok(Some(Ok(v))) => return Ok(v),
                Ok(Some(Err(e))) => {
                    tracing::warn!(error = %e, %slug, "git origin PR diff failed; falling back");
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, %slug, "git origin PR diff join failed; falling back");
                }
            }
        }
    }
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
    let mut file_diffs: Vec<Value> = Vec::new();
    let mut files_touched = 0i64;
    let mut parse_notes: Vec<String> = Vec::new();
    let mut merged_base = veil_ir::IrGraph::new();
    let mut merged_head = veil_ir::IrGraph::new();
    let mut used_layers: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

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
        let status = if base_src.is_empty() {
            "added"
        } else if head_src.is_empty() {
            "removed"
        } else {
            "modified"
        };
        file_diffs.push(json!({
            "path": path,
            "status": status,
            "hunks": unified_hunks(&base_src, &head_src, 3),
            "base_lines": base_src.lines().count(),
            "head_lines": head_src.lines().count(),
        }));
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

        for line in head_src.lines().chain(base_src.lines()) {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("use ") {
                let dep = rest.split_whitespace().next().unwrap_or("").trim();
                if !dep.is_empty() {
                    used_layers.insert(dep.to_string());
                }
            }
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
        // Merge nodes/edges for cross-file blast radius (ids may collide across
        // files — rematerialize by shifting head/base next_id when merging).
        merge_ir_graph(&mut merged_base, &base_ir);
        merge_ir_graph(&mut merged_head, &head_ir);
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

    // Peeks + IR blast radius on the merged multi-file graphs, then risk sort.
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
        item_impact: None,
    };
    veil_ir::enrich_diff_peeks(&mut tmp, &merged_base, &merged_head);
    veil_ir::enrich_diff_impact(&mut tmp, &merged_head, &merged_base);
    veil_ir::sort_diff_for_review(&mut tmp);

    // Resolve layer review policies for packages touched by this PR.
    let review_policies = resolve_review_policies_for_layers(
        used_layers.iter().map(|s| s.as_str()).collect(),
    );

    let summary = if tmp.items.is_empty() && files_touched == 0 {
        format!(
            "No structural changes detected between `{base_branch}` and `{head_branch}` (slug={slug}). If work is only in the coding session, use the IDE working-tree diff."
        )
    } else {
        format!(
            "+{added} −{removed} ~{changed} structural · {files_touched} files · {base_branch} → {head_branch}"
        )
    };

    let items_json = serde_json::to_value(&tmp.items).unwrap_or_else(|_| json!([]));
    let peeks_json = tmp
        .item_peeks
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(json!(null));
    let peeks_base_json = tmp
        .item_peeks_base
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(json!(null));
    let impact_json = tmp
        .item_impact
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(json!(null));

    Ok(json!({
        // IDE / StructDiff shape
        "base_label": base_branch,
        "head_label": head_branch,
        "items": items_json,
        "added": added,
        "removed": removed,
        "changed": changed,
        "item_peeks": peeks_json,
        "item_peeks_base": peeks_base_json,
        "item_impact": impact_json,
        // Secondary git-style file diffs (not front-and-center; wizard key D)
        "file_diffs": file_diffs,
        // Product ChangeDetail shape
        "description": summary,
        "changes": product_changes,
        "files_changed": files_touched,
        "additions": added as i64,
        "removals": removed as i64,
        "slug": slug,
        "parse_notes": parse_notes,
        "used_layers": used_layers.into_iter().collect::<Vec<_>>(),
        "review_policies": review_policies,
    }))
}

/// Merge `src` into `dst`, shifting node ids so multi-file graphs don't collide.
fn merge_ir_graph(dst: &mut veil_ir::IrGraph, src: &veil_ir::IrGraph) {
    if src.nodes.is_empty() {
        return;
    }
    let offset = dst.next_id.saturating_sub(1).max(0);
    let shift = if offset == 0 && dst.nodes.is_empty() {
        0u64
    } else {
        dst.next_id
    };
    for n in &src.nodes {
        let mut nn = n.clone();
        nn.id = n.id.saturating_add(shift);
        if let Some(p) = nn.metadata.parent {
            nn.metadata.parent = Some(p.saturating_add(shift));
        }
        dst.nodes.push(nn);
    }
    for e in &src.edges {
        dst.edges.push(veil_ir::IrEdge {
            from: e.from.saturating_add(shift),
            to: e.to.saturating_add(shift),
            kind: e.kind.clone(),
        });
    }
    dst.next_id = dst
        .nodes
        .iter()
        .map(|n| n.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
}

fn resolve_review_policies_for_layers(layers: Vec<&str>) -> Value {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut reg = veil_ir::LayerRegistry::builtin();
    let mut names: Vec<String> = layers.iter().map(|s| (*s).to_string()).collect();
    // Always try base + common UI layers so policies are available.
    for extra in ["base", "svelte5", "ui"] {
        if !names.iter().any(|n| n == extra) {
            names.push(extra.into());
        }
    }
    for name in &names {
        let _ = reg.load_layer(name, &cwd);
    }
    let mut map = serde_json::Map::new();
    for (name, pol) in &reg.review_policies {
        map.insert(
            name.clone(),
            serde_json::to_value(pol).unwrap_or(json!({})),
        );
    }
    if map.is_empty() {
        for (name, rel) in [
            ("base", "layers/base.layer"),
            ("svelte5", "layers/svelte5.layer"),
        ] {
            let path = cwd.join(rel);
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some(pol) = veil_ir::parse_review_policy(&content) {
                    map.insert(
                        name.into(),
                        serde_json::to_value(pol).unwrap_or(json!({})),
                    );
                }
            }
        }
    }
    Value::Object(map)
}

/// Compact unified-style hunks for secondary file-diff review (line-based, not git plumbing).
fn unified_hunks(base: &str, head: &str, context: usize) -> Vec<Value> {
    let a: Vec<&str> = base.lines().collect();
    let b: Vec<&str> = head.lines().collect();
    // Simple LCS-free walk: emit full file as one hunk when small; else truncated.
    const MAX_LINES: usize = 400;
    if a.is_empty() && b.is_empty() {
        return vec![];
    }
    let mut lines_out: Vec<String> = Vec::new();
    if a.is_empty() {
        for (i, l) in b.iter().enumerate().take(MAX_LINES) {
            lines_out.push(format!("+{}", l));
            if i + 1 == MAX_LINES && b.len() > MAX_LINES {
                lines_out.push(format!("… +{} more lines", b.len() - MAX_LINES));
            }
        }
        return vec![json!({
            "header": format!("@@ -0,0 +1,{} @@", b.len().min(MAX_LINES)),
            "lines": lines_out,
        })];
    }
    if b.is_empty() {
        for (i, l) in a.iter().enumerate().take(MAX_LINES) {
            lines_out.push(format!("-{}", l));
            if i + 1 == MAX_LINES && a.len() > MAX_LINES {
                lines_out.push(format!("… −{} more lines", a.len() - MAX_LINES));
            }
        }
        return vec![json!({
            "header": format!("@@ -1,{} +0,0 @@", a.len().min(MAX_LINES)),
            "lines": lines_out,
        })];
    }
    // Myers-lite: mark unequal lines by index zip; good enough for review secondary panel.
    let max = a.len().max(b.len()).min(MAX_LINES);
    let mut changed = false;
    for i in 0..max {
        let al = a.get(i).copied();
        let bl = b.get(i).copied();
        match (al, bl) {
            (Some(x), Some(y)) if x == y => {
                if context > 0 {
                    lines_out.push(format!(" {}", x));
                }
            }
            (Some(x), Some(y)) => {
                changed = true;
                lines_out.push(format!("-{}", x));
                lines_out.push(format!("+{}", y));
            }
            (Some(x), None) => {
                changed = true;
                lines_out.push(format!("-{}", x));
            }
            (None, Some(y)) => {
                changed = true;
                lines_out.push(format!("+{}", y));
            }
            _ => {}
        }
    }
    if a.len().max(b.len()) > MAX_LINES {
        lines_out.push(format!(
            "… truncated ({} base / {} head lines)",
            a.len(),
            b.len()
        ));
    }
    if !changed && a.len() == b.len() {
        return vec![];
    }
    vec![json!({
        "header": format!("@@ -1,{} +1,{} @@", a.len().min(MAX_LINES), b.len().min(MAX_LINES)),
        "lines": lines_out,
    })]
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
        .pr_repo
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

/// Finalize wizard: all items approved → approve PR; any feedback → request_pr_changes with summary.
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
    if let Ok(Some(pr)) = st.deps.pr_repo.find(id).await {
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
    let wizard_slug = st
        .deps
        .pr_repo
        .find(id)
        .await
        .ok()
        .flatten()
        .and_then(|p| extract_slug_from_description(&p.description));
    let _ = ensure_ci_passed(&st.deps, id, "wizard", wizard_slug.as_deref()).await;

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
        match change_management::application::approve_pr(
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
        match change_management::application::request_pr_changes(&st.deps, id, reviewer, summary)
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
    match change_management::application::update_pull_request_status(&st.deps, id, status).await
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
        Err(veil_shared::DomainError::NotFound) => Ok(Json(json!({
            "ok": false,
            "error": "nothing_to_provision",
            "message": "No deployable stack for this project yet (no HTTP/compose units to place in the data center).",
        }))),
        Err(e) => Err(domain_status(e)),
    }
}

async fn provision_project(
    State(st): State<DeployState>,
    Json(body): Json<PlanBody>,
) -> Result<Json<Value>, StatusCode> {
    if let Err(e) = veil_server::review::may_ship(&body.project_slug, None) {
        return Ok(Json(json!({
            "ok": false,
            "error": "sign_off_required",
            "message": e,
            "hint": "Approve the change set on /review before shipping this SHA.",
            "audit_env": veil_server::review::audit_env_json(),
        })));
    }
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
        Err(veil_shared::DomainError::NotFound) => Ok(Json(json!({
            "ok": false,
            "error": "nothing_to_provision",
            "message": "No deployable stack for this project yet (no HTTP/compose units to place in the data center).",
        }))),
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
    veil_server::stub_ops::ensure_platform_stub_cache();
    let entries = veil_ir::list_platform_stubs();
    let names: Vec<Value> = entries
        .iter()
        .map(|e| json!({ "crate_name": e.name }))
        .collect();
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

// ─── Living Design Journal (DDB-durable + process write-through cache) ──

use std::sync::Mutex;

static DESIGN_JOURNAL: std::sync::LazyLock<Mutex<Vec<Value>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

#[derive(Clone)]
struct JournalDdb {
    client: aws_sdk_dynamodb::Client,
    table: String,
}

impl JournalDdb {
    async fn from_env() -> Option<Self> {
        if std::env::var("VEIL_PLATFORM_LOCAL").ok().as_deref() == Some("1") {
            return None;
        }
        if std::env::var("VEIL_DDB_TABLE").is_err() && std::env::var("AWS_PROFILE").is_err() {
            return None;
        }
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let table = std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
        Some(Self {
            client: aws_sdk_dynamodb::Client::new(&config),
            table,
        })
    }

    /// Dual-write: global JOURNAL stream + construct + PR secondary keys.
    async fn put_entry(&self, entry: &Value) -> Result<(), String> {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let ts = entry
            .get("ts")
            .and_then(|v| v.as_str())
            .unwrap_or("1970-01-01T00:00:00Z");
        let sk = format!("ENTRY#{ts}#{id}");
        let data = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        let mut keys: Vec<(String, String)> = vec![("JOURNAL".into(), sk.clone())];
        if let Some(name) = entry
            .get("construct_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            keys.push((
                format!("JOURNAL_C#{}", name.to_lowercase()),
                sk.clone(),
            ));
        }
        if let Some(pr) = entry
            .get("pr_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            keys.push((format!("JOURNAL_PR#{pr}"), sk.clone()));
        }
        for (pk, skv) in keys {
            self.client
                .put_item()
                .table_name(&self.table)
                .item(
                    "PK",
                    aws_sdk_dynamodb::types::AttributeValue::S(pk),
                )
                .item(
                    "SK",
                    aws_sdk_dynamodb::types::AttributeValue::S(skv),
                )
                .item(
                    "data",
                    aws_sdk_dynamodb::types::AttributeValue::S(data.clone()),
                )
                .item(
                    "GSI1PK",
                    aws_sdk_dynamodb::types::AttributeValue::S("JOURNAL".into()),
                )
                .item(
                    "GSI1SK",
                    aws_sdk_dynamodb::types::AttributeValue::S(sk.clone()),
                )
                .send()
                .await
                .map_err(|e| format!("{e:?}"))?;
        }
        Ok(())
    }

    async fn query_pk(&self, pk: &str, limit: i32) -> Result<Vec<Value>, String> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk")
            .expression_attribute_values(
                ":pk",
                aws_sdk_dynamodb::types::AttributeValue::S(pk.to_string()),
            )
            .scan_index_forward(false)
            .limit(limit)
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        let mut out = Vec::new();
        for item in resp.items() {
            if let Some(av) = item.get("data") {
                if let Ok(s) = av.as_s() {
                    if let Ok(v) = serde_json::from_str::<Value>(s) {
                        out.push(v);
                    }
                }
            }
        }
        Ok(out)
    }
}

static JOURNAL_DDB: std::sync::OnceLock<Option<JournalDdb>> = std::sync::OnceLock::new();

async fn journal_ddb() -> Option<&'static JournalDdb> {
    if JOURNAL_DDB.get().is_none() {
        let _ = JOURNAL_DDB.set(JournalDdb::from_env().await);
    }
    JOURNAL_DDB.get().and_then(|o| o.as_ref())
}

fn journal_cache_push(entry: Value) {
    if let Ok(mut j) = DESIGN_JOURNAL.lock() {
        j.push(entry);
        if j.len() > 2000 {
            let drop_n = j.len() - 2000;
            j.drain(0..drop_n);
        }
    }
}

fn journal_cache_filter(
    construct: Option<&str>,
    pr: Option<&str>,
    needle: Option<&str>,
    limit: usize,
) -> Vec<Value> {
    let Ok(lock) = DESIGN_JOURNAL.lock() else {
        return vec![];
    };
    let construct = construct.map(|s| s.to_lowercase());
    let needle = needle.map(|s| s.to_lowercase());
    let mut out: Vec<Value> = lock
        .iter()
        .rev()
        .filter(|e| journal_entry_matches(e, construct.as_deref(), pr, needle.as_deref()))
        .take(limit)
        .cloned()
        .collect();
    out.reverse();
    out
}

fn journal_entry_matches(
    e: &Value,
    construct: Option<&str>,
    pr: Option<&str>,
    needle: Option<&str>,
) -> bool {
    if let Some(c) = construct {
        let name = e
            .get("construct_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let path = e
            .get("construct_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        if !name.contains(c) && !path.contains(c) {
            return false;
        }
    }
    if let Some(p) = pr {
        if e.get("pr_id").and_then(|v| v.as_str()) != Some(p) {
            return false;
        }
    }
    if let Some(n) = needle {
        let blob = e.to_string().to_lowercase();
        if !blob.contains(n) {
            return false;
        }
    }
    true
}

#[derive(Deserialize)]
struct JournalBody {
    #[serde(default)]
    pr_id: Option<String>,
    construct_path: String,
    construct_name: String,
    decision: String,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    teaching_note: Option<String>,
    #[serde(default)]
    risk: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

#[derive(Deserialize)]
struct JournalQuery {
    #[serde(default)]
    construct: Option<String>,
    #[serde(default)]
    pr_id: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn post_journal(
    State(st): State<CmState>,
    Json(body): Json<JournalBody>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::new_v4().to_string();
    let ts = chrono::Utc::now().to_rfc3339();
    let entry = json!({
        "id": id,
        "ts": ts,
        "pr_id": body.pr_id,
        "construct_path": body.construct_path,
        "construct_name": body.construct_name,
        "decision": body.decision,
        "rationale": body.rationale,
        "teaching_note": body.teaching_note,
        "risk": body.risk,
        "package": body.package,
        "author": body.author.unwrap_or_else(|| "operator".into()),
    });
    journal_cache_push(entry.clone());
    // Durable write (best-effort; cache already has the entry).
    if let Some(ddb) = journal_ddb().await {
        if let Err(e) = ddb.put_entry(&entry).await {
            tracing::warn!(error = %e, "journal DDB put failed; entry kept in process cache");
        }
    }
    // Mirror onto PR history when bound
    if let Some(ref pr_id) = body.pr_id {
        if let Ok(uuid) = Uuid::parse_str(pr_id) {
            let mut lines = vec![
                "[pr-wizard:journal]".to_string(),
                format!("decision: {}", body.decision),
                format!("name: {}", body.construct_name),
                format!("path: {}", body.construct_path),
            ];
            if let Some(r) = &body.rationale {
                if !r.is_empty() {
                    lines.push(format!("rationale: {r}"));
                }
            }
            if let Some(n) = &body.teaching_note {
                if !n.is_empty() {
                    lines.push(format!("teaching_note: {n}"));
                }
            }
            let _ = change_management::application::add_review_comment(
                &st.deps,
                uuid,
                "operator".into(),
                Some(body.construct_path.clone()),
                lines.join("\n"),
            )
            .await;
        }
    }
    Ok(Json(json!({ "ok": true, "entry": entry, "durable": journal_ddb().await.is_some() })))
}

async fn list_journal(Query(q): Query<JournalQuery>) -> Result<Json<Value>, StatusCode> {
    let limit = q.limit.unwrap_or(50).min(200);
    let construct = q.construct.as_deref();
    let pr = q.pr_id.as_deref();
    let needle = q.q.as_deref();

    // Prefer DDB when available (survives host restart).
    if let Some(ddb) = journal_ddb().await {
        let pk = if let Some(p) = pr.filter(|s| !s.is_empty()) {
            format!("JOURNAL_PR#{p}")
        } else if let Some(c) = construct.filter(|s| !s.is_empty()) {
            // Exact secondary key is lowercase full construct name; for partial
            // search fall back to global + filter.
            if c.contains(' ') || c.len() < 2 {
                "JOURNAL".into()
            } else {
                // Try exact construct key first; if empty we'll re-query global.
                format!("JOURNAL_C#{}", c.to_lowercase())
            }
        } else {
            "JOURNAL".into()
        };
        let fetch_limit = (limit as i32).saturating_mul(3).min(500);
        match ddb.query_pk(&pk, fetch_limit).await {
            Ok(mut rows) => {
                if rows.is_empty() && pk.starts_with("JOURNAL_C#") {
                    // Partial construct name — scan global stream.
                    if let Ok(global) = ddb.query_pk("JOURNAL", fetch_limit).await {
                        rows = global;
                    }
                }
                let construct_l = construct.map(|s| s.to_lowercase());
                let needle_l = needle.map(|s| s.to_lowercase());
                let mut out: Vec<Value> = rows
                    .into_iter()
                    .filter(|e| {
                        journal_entry_matches(
                            e,
                            construct_l.as_deref(),
                            pr,
                            needle_l.as_deref(),
                        )
                    })
                    .take(limit)
                    .collect();
                // DDB returns newest-first; reverse to chronological for UI timelines.
                out.reverse();
                return Ok(Json(json!({
                    "entries": out,
                    "count": out.len(),
                    "source": "ddb",
                })));
            }
            Err(e) => {
                tracing::warn!(error = %e, "journal DDB query failed; using process cache");
            }
        }
    }

    let out = journal_cache_filter(construct, pr, needle, limit);
    Ok(Json(json!({
        "entries": out,
        "count": out.len(),
        "source": "memory",
    })))
}

/// Layer-declared review policies (from built-in layer files on disk).
async fn list_review_policies() -> Result<Json<Value>, StatusCode> {
    let mut reg = veil_ir::LayerRegistry::builtin();
    // Load known review-bearing layers when present on disk.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    for name in ["base", "svelte5", "sveltekit5", "ui", "ddd"] {
        let _ = reg.load_layer(name, &cwd);
    }
    // Also parse any layers already registered by name from VEIL_LAYERS_DIR.
    let mut map = serde_json::Map::new();
    for (name, pol) in &reg.review_policies {
        map.insert(
            name.clone(),
            serde_json::to_value(pol).unwrap_or(json!({})),
        );
    }
    // Fallback: parse files directly if registry load missed (cwd not monorepo root).
    if map.is_empty() {
        for (name, rel) in [
            ("base", "layers/base.layer"),
            ("svelte5", "layers/svelte5.layer"),
        ] {
            let path = cwd.join(rel);
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some(pol) = veil_ir::parse_review_policy(&content) {
                    map.insert(
                        name.into(),
                        serde_json::to_value(pol).unwrap_or(json!({})),
                    );
                }
            }
        }
    }
    Ok(Json(json!({ "policies": map })))
}

// ─── Artifact Registry ──────────────────────────────────────────────────────

#[derive(Clone)]
struct ArtifactRegistryState {
    store: Arc<crate::artifact_registry::ArtifactRegistryStore>,
}

// ─── Function Invoke (Phase 3) ──────────────────────────────────────────────

#[derive(Clone)]
struct FunctionInvokeState {
    registry: Arc<crate::function_invoke::FunctionRegistry>,
}

#[derive(Deserialize)]
struct InvokeFunctionBody {
    #[serde(default)]
    args: Value,
}

async fn invoke_function(
    State(st): State<FunctionInvokeState>,
    Path(function_id): Path<String>,
    tenant: crate::tenancy::ResolvedTenant,
    Json(body): Json<InvokeFunctionBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tenant_id = tenant.0;

    // Resolve the function under this tenant's context.
    let handle = st
        .registry
        .resolve(&tenant_id, &function_id)
        .await
        .map_err(|e| {
            let status = e.status_code();
            let body = json!({
                "error": e.to_string(),
                "code": status.as_u16(),
            });
            (status, Json(body))
        })?;

    // Invoke the function with the provided args.
    match handle.invoke(body.args) {
        Ok(result) => Ok(Json(json!({ "result": result }))),
        Err(e) => {
            tracing::error!(
                function_id = %function_id,
                tenant = %tenant_id,
                error = %e,
                "function invocation failed"
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": format!("invocation failed: {e}"),
                    "code": 500,
                })),
            ))
        }
    }
}

#[derive(Deserialize)]
struct RegisterArtifactBody {
    id: String,
    version: String,
    artifact_type: crate::artifact_registry::ArtifactType,
    tenant_visibility: crate::artifact_registry::TenantVisibility,
    #[serde(default)]
    contributions: Vec<crate::artifact_registry::Contribution>,
    #[serde(default)]
    signed_off_by: Option<String>,
    #[serde(default)]
    signed_off_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    blob_key: Option<String>,
    #[serde(default)]
    content_hash: Option<String>,
    #[serde(default)]
    bundle_path: Option<String>,
    #[serde(default)]
    bundle_size: Option<u64>,
    #[serde(default)]
    manifest: Option<crate::artifact_registry::ArtifactManifest>,
}

async fn register_artifact(
    State(st): State<ArtifactRegistryState>,
    Json(body): Json<RegisterArtifactBody>,
) -> Result<Json<Value>, StatusCode> {
    let now = chrono::Utc::now();
    let record = crate::artifact_registry::ArtifactRecord {
        id: body.id,
        version: body.version,
        artifact_type: body.artifact_type,
        tenant_visibility: body.tenant_visibility,
        contributions: body.contributions,
        signed_off_by: body.signed_off_by,
        signed_off_at: body.signed_off_at,
        blob_key: body.blob_key,
        content_hash: body.content_hash,
        bundle_path: body.bundle_path,
        bundle_size: body.bundle_size,
        manifest: body.manifest,
        created_at: now,
        updated_at: now,
    };
    st.store
        .put_artifact(&record)
        .await
        .map_err(registry_status)?;
    Ok(Json(json!(record)))
}

async fn list_artifacts_registry(
    State(st): State<ArtifactRegistryState>,
) -> Result<Json<Value>, StatusCode> {
    let records = st.store.list_all().await.map_err(registry_status)?;
    Ok(Json(json!(records)))
}

async fn get_artifact_registry(
    State(st): State<ArtifactRegistryState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let record = st.store.get_latest(&id).await.map_err(registry_status)?;
    Ok(Json(json!(record)))
}

#[derive(Deserialize)]
struct ResolveContributionsQuery {
    tenant_id: String,
    kind: crate::artifact_registry::ContributionKind,
    #[serde(default)]
    principal_id: Option<String>,
    #[serde(default)]
    roles: Option<String>,
}

async fn resolve_contributions_handler(
    State(st): State<ArtifactRegistryState>,
    Query(q): Query<ResolveContributionsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let roles: Vec<String> = q
        .roles
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let principal = crate::artifact_registry::Principal {
        id: q.principal_id.unwrap_or_default(),
        roles,
    };
    let results = st
        .store
        .resolve_contributions(&q.tenant_id, &principal, q.kind)
        .await
        .map_err(registry_status)?;
    Ok(Json(json!(results)))
}

#[derive(Deserialize)]
struct ResolveFunctionQuery {
    tenant_id: String,
    function_id: String,
}

async fn resolve_function_handler(
    State(st): State<ArtifactRegistryState>,
    Query(q): Query<ResolveFunctionQuery>,
) -> Result<Json<Value>, StatusCode> {
    let record = st
        .store
        .resolve_function(&q.tenant_id, &q.function_id)
        .await
        .map_err(registry_status)?;
    Ok(Json(json!(record)))
}

#[derive(Deserialize)]
struct ResolveUiArtifactQuery {
    tenant_id: String,
    artifact_id: String,
}

async fn resolve_ui_artifact_handler(
    State(st): State<ArtifactRegistryState>,
    Query(q): Query<ResolveUiArtifactQuery>,
) -> Result<Json<Value>, StatusCode> {
    let url = st
        .store
        .resolve_ui_artifact(&q.tenant_id, &q.artifact_id)
        .await
        .map_err(registry_status)?;
    Ok(Json(json!(url)))
}

fn registry_status(e: crate::artifact_registry::RegistryError) -> StatusCode {
    match e {
        crate::artifact_registry::RegistryError::NotFound(_) => StatusCode::NOT_FOUND,
        crate::artifact_registry::RegistryError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        crate::artifact_registry::RegistryError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ─── Phase 4: Artifact Serving (bundle, manifest, contributions) ────────────

/// GET /api/artifacts/:id/bundle?v={content_hash}
/// Serves the compiled bundle from S3 with immutable caching headers.
async fn serve_artifact_bundle(
    State(st): State<ArtifactRegistryState>,
    Path(id): Path<String>,
    Query(q): Query<BundleQuery>,
) -> Result<axum::response::Response, StatusCode> {
    use axum::response::IntoResponse;
    use axum::http::header;

    let record = st.store.get_latest(&id).await.map_err(registry_status)?;

    // Determine the S3 key: prefer bundle_path, fall back to blob_key.
    let s3_key = record
        .bundle_path
        .as_deref()
        .or(record.blob_key.as_deref())
        .ok_or(StatusCode::NOT_FOUND)?;

    // Fetch the blob from S3.
    let data = st.store.get_blob(s3_key).await.map_err(registry_status)?;

    // Content-Type based on the S3 key extension (runtime has no opinion about content).
    let content_type = guess_content_type(s3_key);

    // If the client passed ?v=<hash> and it matches, use immutable caching.
    // Otherwise still serve, but with shorter cache.
    let cache_control = if q.v.as_deref() == record.content_hash.as_deref()
        && record.content_hash.is_some()
    {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    };

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        content_type.parse().unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    headers.insert(
        header::CACHE_CONTROL,
        cache_control.parse().unwrap(),
    );
    if let Some(ref hash) = record.content_hash {
        headers.insert(
            header::ETAG,
            format!("\"{hash}\"").parse().unwrap(),
        );
    }

    Ok((headers, data).into_response())
}

#[derive(Deserialize)]
struct BundleQuery {
    #[serde(default)]
    v: Option<String>,
}

/// GET /api/artifacts/:id/manifest
/// Returns metadata the harness needs to load and mount the artifact.
async fn get_artifact_manifest(
    State(st): State<ArtifactRegistryState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let record = st.store.get_latest(&id).await.map_err(registry_status)?;

    let bundle_url = if record.bundle_path.is_some() || record.blob_key.is_some() {
        let hash_param = record
            .content_hash
            .as_deref()
            .map(|h| format!("?v={h}"))
            .unwrap_or_default();
        Some(format!("/api/artifacts/{}/bundle{}", record.id, hash_param))
    } else {
        None
    };

    let manifest = record
        .manifest
        .as_ref()
        .cloned()
        .unwrap_or_default();

    Ok(Json(json!({
        "id": record.id,
        "version": record.version,
        "artifact_type": record.artifact_type,
        "entrypoint": manifest.entrypoint,
        "exports": manifest.exports,
        "props": manifest.props,
        "bundle_url": bundle_url,
        "bundle_size": record.bundle_size,
        "content_hash": record.content_hash,
    })))
}

/// GET /api/contributions?kind=menu_item&tenant_id=...
/// Lists contributions visible to the current tenant, filtered by kind.
async fn list_contributions(
    State(st): State<ArtifactRegistryState>,
    Query(q): Query<ContributionsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let tenant_id = q.tenant_id.as_deref().unwrap_or("default");
    let roles: Vec<String> = q
        .roles
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let principal = crate::artifact_registry::Principal {
        id: q.principal_id.clone().unwrap_or_default(),
        roles,
    };

    // If kind is specified, filter by it; otherwise return all contributions.
    let artifacts = st
        .store
        .list_for_tenant(tenant_id)
        .await
        .map_err(registry_status)?;

    let mut results: Vec<Value> = Vec::new();
    for artifact in &artifacts {
        for contribution in &artifact.contributions {
            let matches_kind = match (&q.kind, contribution) {
                (Some(crate::artifact_registry::ContributionKind::MenuItem), crate::artifact_registry::Contribution::MenuItem { .. }) => true,
                (Some(crate::artifact_registry::ContributionKind::Route), crate::artifact_registry::Contribution::Route { .. }) => true,
                (Some(crate::artifact_registry::ContributionKind::SlotFill), crate::artifact_registry::Contribution::SlotFill { .. }) => true,
                (Some(crate::artifact_registry::ContributionKind::BackendFunction), crate::artifact_registry::Contribution::BackendFunction { .. }) => true,
                (None, _) => true, // no filter → return all
                _ => false,
            };
            if !matches_kind {
                continue;
            }

            // Role filtering for menu items.
            let role_ok = match contribution {
                crate::artifact_registry::Contribution::MenuItem { roles, .. } => {
                    roles.is_empty()
                        || roles.iter().any(|r| principal.roles.contains(r))
                }
                _ => true,
            };
            if !role_ok {
                continue;
            }

            let mut entry = serde_json::to_value(contribution).unwrap_or(json!({}));
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("artifact_id".into(), json!(artifact.id));
                obj.insert("artifact_version".into(), json!(artifact.version));
            }
            results.push(entry);
        }
    }

    Ok(Json(json!(results)))
}

#[derive(Deserialize)]
struct ContributionsQuery {
    #[serde(default)]
    kind: Option<crate::artifact_registry::ContributionKind>,
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    principal_id: Option<String>,
    #[serde(default)]
    roles: Option<String>,
}

/// Guess content-type from S3 key file extension.
pub(crate) fn guess_content_type(key: &str) -> &'static str {
    if let Some(ext) = key.rsplit('.').next() {
        match ext {
            "js" | "mjs" => "application/javascript",
            "css" => "text/css",
            "wasm" => "application/wasm",
            "json" => "application/json",
            "html" => "text/html",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "map" => "application/json",
            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}

/// Build a CORS layer for artifact serving.
///
/// Reads `VEIL_CORS_ORIGINS` env var (comma-separated list of allowed origins).
/// If not set, falls back to permissive CORS (matches the ProductHost default).
/// The Authorization header is always allowed for authenticated fetches.
pub(crate) fn build_artifact_cors_layer() -> tower_http::cors::CorsLayer {
    use axum::http::{header, Method};
    use tower_http::cors::CorsLayer;

    let origins_env = std::env::var("VEIL_CORS_ORIGINS").unwrap_or_default();

    if origins_env.is_empty() || origins_env == "*" {
        return CorsLayer::permissive();
    }

    let origins: Vec<axum::http::HeaderValue> = origins_env
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse().ok()
        })
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .max_age(std::time::Duration::from_secs(86400))
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
            get(get_repo).patch(update_repo).delete(delete_repo),
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
            "/api/pull_requests",
            get(list_all_pull_requests).post(create_pull_request_flat),
        )
        .route(
            "/api/pull_requests/{id}",
            get(get_pull_request),
        )
        .route(
            "/api/pull_requests/{id}/submit",
            post(submit_for_review),
        )
        .route(
            "/api/pull_requests/{id}/approve",
            post(approve_pr),
        )
        .route(
            "/api/pull_requests/{id}/request-changes",
            post(request_pr_changes),
        )
        .route(
            "/api/pull_requests/{id}/merge",
            post(merge_pr),
        )
        .route(
            "/api/pull_requests/{id}/comments",
            get(list_review_comments).post(add_review_comment),
        )
        .route(
            "/api/pull_requests/{id}/approvals",
            get(list_pr_approvals),
        )
        .route(
            "/api/pull_requests/{id}/ci",
            get(get_pr_ci),
        )
        .route(
            "/api/pull_requests/{id}/diff",
            get(get_structural_diff),
        )
        .route(
            "/api/pull_requests/{id}/review-item",
            post(review_item),
        )
        .route(
            "/api/pull_requests/{id}/finalize-wizard",
            post(finalize_wizard),
        )
        .route(
            "/api/pull_requests/{id}/status",
            put(update_status),
        )
        .route(
            "/api/repos/{id}/pull_requests",
            get(list_repo_changes).post(create_repo_change),
        )
        .route("/api/journal", get(list_journal).post(post_journal))
        .route("/api/review_policies", get(list_review_policies))
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

    // Artifact Registry (Phase 1 Platform Primitives)
    let art_reg = ArtifactRegistryState {
        store: Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        ),
    };
    let artifact_registry_r = Router::new()
        .route(
            "/api/artifact-registry",
            get(list_artifacts_registry).post(register_artifact),
        )
        .route(
            "/api/artifact-registry/{id}",
            get(get_artifact_registry),
        )
        .route(
            "/api/artifact-registry/resolve/contributions",
            get(resolve_contributions_handler),
        )
        .route(
            "/api/artifact-registry/resolve/function",
            get(resolve_function_handler),
        )
        .route(
            "/api/artifact-registry/resolve/ui-artifact",
            get(resolve_ui_artifact_handler),
        )
        // Phase 4: Artifact Serving
        .route(
            "/api/artifacts/{id}/bundle",
            get(serve_artifact_bundle),
        )
        .route(
            "/api/artifacts/{id}/manifest",
            get(get_artifact_manifest),
        )
        .route(
            "/api/contributions",
            get(list_contributions),
        )
        .with_state(art_reg)
        .layer(build_artifact_cors_layer());

    // Function Invoke (Phase 3 Platform Primitives)
    let fn_registry = Arc::new(crate::function_invoke::FunctionRegistry::new(
        Arc::new(crate::artifact_registry::ArtifactRegistryStore::from_env().await),
    ));
    let fn_invoke_state = FunctionInvokeState {
        registry: fn_registry,
    };
    let function_invoke_r = Router::new()
        .route(
            "/api/functions/{function_id}/invoke",
            post(invoke_function),
        )
        .with_state(fn_invoke_state);

    storage_r
        .merge(cm_r)
        .merge(deploy_r)
        .merge(registry_r)
        .merge(artifact_registry_r)
        .merge(function_invoke_r)
}
