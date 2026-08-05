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
        .unwrap_or_else(|| body.source_branch.clone().unwrap_or_else(|| "main".into()));
    let author = if body.author.is_empty() {
        "jd".into()
    } else {
        body.author
    };
    match change_management::application::create_change_request_flat(
        &st.deps,
        repo_id,
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
        Ok(()) => Ok(Json(json!({ "ok": true }))),
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
    let reviewer = if body.reviewer.is_empty() {
        "reviewer".into()
    } else {
        body.reviewer
    };
    match change_management::application::approve_change(&st.deps, id, reviewer, body.comment)
        .await
    {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
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
    let reviewer = if body.reviewer.is_empty() {
        "reviewer".into()
    } else {
        body.reviewer
    };
    match change_management::application::request_changes(
        &st.deps,
        id,
        reviewer,
        body.comment,
    )
    .await
    {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
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
        "merger".into()
    } else {
        body.merger
    };
    match change_management::application::merge_change(&st.deps, id, merger, body.slug).await {
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

async fn get_structural_diff(
    State(st): State<CmState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    match change_management::application::get_structural_diff(&st.deps, id, String::new()).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(domain_status(e)),
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
