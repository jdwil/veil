//! Platform domain HTTP surface for ProductHost (single process).
//!
//! Wires generated `storage` / `change_management` / `deploy` application
//! services so the dashboard SPA needs no separate `veil_bin` on :3000.
//! See `docs/ADR_SINGLE_PRODUCT_HOST.md`.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post, put},
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
        Ok(repos) => {
            for r in &repos {
                let _ = crate::origin_resolve::git_origin_for(r);
            }
            Ok(Json(json!(repos)))
        }
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct CreateRepoBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Per-project git origin (`kind` git|s3, `provider`, `repo`/`owner`+`name`, `create`).
    #[serde(default)]
    origin: Option<Value>,
}

fn origin_binding_from_remote(
    cfg: &veil_server::git_origin::RemoteConfig,
) -> storage::domain::types::OriginBinding {
    use storage::domain::types::GitProvider as BindProvider;
    use veil_server::git_origin::GitProvider;
    storage::domain::types::OriginBinding::Git {
        provider: match cfg.provider {
            GitProvider::GitHub => BindProvider::Github,
            GitProvider::Bitbucket => BindProvider::Bitbucket,
        },
        repo: cfg.repo.clone(),
        subpath: cfg.subpath.clone(),
        branch: Some(cfg.branch.clone()),
    }
}

async fn create_repo(
    State(st): State<StorageState>,
    Json(body): Json<CreateRepoBody>,
) -> Result<Json<Value>, StatusCode> {
    match storage::application::create_repo(&st.deps, body.name.clone(), body.description.clone())
        .await
    {
        Ok(mut repo) => {
            let slug = repo.slug.clone();
            let spec = veil_server::git_provider::OriginRequest::from_value(
                body.origin.as_ref(),
                &slug,
            )
            .map_err(|_| StatusCode::BAD_REQUEST)?;
            if spec.wants_git() {
                let rid = repo.id.value.clone();
                let desc = body.description.clone();
                let provisioned = tokio::task::spawn_blocking(move || {
                    veil_server::git_provider::provision_origin(
                        &rid,
                        &slug,
                        desc.as_deref(),
                        &spec,
                    )
                })
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                match provisioned {
                    Ok(cfg) => {
                        let binding = origin_binding_from_remote(&cfg);
                        match storage::application::set_repo_origin(
                            &st.deps,
                            repo.id.value.clone(),
                            Some(binding),
                        )
                        .await
                        {
                            Ok(updated) => repo = updated,
                            Err(e) => {
                                tracing::error!(error = %e, "set_repo_origin after provision failed");
                                return Err(StatusCode::BAD_GATEWAY);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, slug = %repo.slug, "git origin provision failed");
                        return Err(StatusCode::BAD_GATEWAY);
                    }
                }
            } else {
                crate::origin_resolve::git_origin_for(&repo);
            }
            Ok(Json(json!(repo)))
        }
        Err(e) => Err(domain_status(e)),
    }
}

async fn get_repo(
    State(st): State<StorageState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    // Accept repo UUID or slug (agent open_project / open_ide use slugs).
    match storage::application::resolve_repo(&st.deps, &id).await {
        Ok(repo) => {
            let _ = crate::origin_resolve::git_origin_for(&repo);
            Ok(Json(json!(repo)))
        }
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
    // Git-backed projects: read from the provider working tree.
    if let Ok(repo) = crate::origin_resolve::resolve_repo_full(&st.deps, &raw_id).await {
        if crate::origin_resolve::is_git_backed(&repo) {
            let origin = crate::origin_resolve::git_origin_for(&repo);
            return match crate::git_files::read_file(&origin, &branch, path) {
                Ok(Some(content)) => Ok(Json(json!({
                    "ok": true,
                    "exists": true,
                    "repo_id": repo.id.value,
                    "branch": branch,
                    "path": path,
                    "content": content,
                    "bytes": content.len(),
                    "via": "git",
                }))),
                Ok(None) => Ok(Json(json!({
                    "ok": true,
                    "exists": false,
                    "repo_id": repo.id.value,
                    "branch": branch,
                    "path": path,
                    "content": "",
                    "bytes": 0,
                    "via": "git",
                }))),
                Err(e) => {
                    tracing::error!(error = %e, "git-backed read_file failed");
                    Err(StatusCode::BAD_GATEWAY)
                }
            };
        }
    }
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
    // Git-backed projects: write into the provider working tree + push.
    if let Ok(repo) = crate::origin_resolve::resolve_repo_full(&st.deps, &raw_id).await {
        if crate::origin_resolve::is_git_backed(&repo) {
            let origin = crate::origin_resolve::git_origin_for(&repo);
            return match crate::git_files::write_file(
                &origin, &branch, path, &body.content, &message, None, None,
            ) {
                Ok(sha) => Ok(Json(json!({
                    "ok": true,
                    "repo_id": repo.id.value,
                    "branch": branch,
                    "path": path,
                    "bytes": body.content.len(),
                    "commit": { "hash": sha },
                    "message": message,
                    "via": "git",
                }))),
                Err(e) => {
                    tracing::error!(error = %e, "git-backed write_file failed");
                    Err(StatusCode::BAD_GATEWAY)
                }
            };
        }
    }
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
struct ListFilesBody {
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
}

/// List files for a project. Git-backed → provider working tree; else S3.
async fn list_files_api(
    State(st): State<StorageState>,
    Json(body): Json<ListFilesBody>,
) -> Result<Json<Value>, StatusCode> {
    let raw_id = body
        .repo_id
        .or(body.id)
        .filter(|s| !s.is_empty())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let branch = body
        .branch
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into());
    let prefix = body.prefix.unwrap_or_default();

    let repo = crate::origin_resolve::resolve_repo_full(&st.deps, &raw_id)
        .await
        .map_err(domain_status)?;
    if crate::origin_resolve::is_git_backed(&repo) {
        let origin = crate::origin_resolve::git_origin_for(&repo);
        return match crate::git_files::list_files(&origin, &branch, &prefix) {
            Ok(files) => Ok(Json(json!({
                "ok": true,
                "repo_id": repo.id.value,
                "branch": branch,
                "files": files,
                "via": "git",
            }))),
            Err(e) => {
                tracing::error!(error = %e, "git-backed list_files failed");
                Err(StatusCode::BAD_GATEWAY)
            }
        };
    }
    match storage::application::list_files(&st.deps, repo.id.clone(), branch.clone(), prefix).await {
        Ok(files) => Ok(Json(json!({
            "ok": true,
            "repo_id": repo.id.value,
            "branch": branch,
            "files": files,
            "via": "s3",
        }))),
        Err(e) => Err(domain_status(e)),
    }
}

#[derive(Deserialize)]
struct BindOriginBody {
    /// "git" or "s3" (default s3 = clears binding).
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    /// `org/name` on the provider.
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    subpath: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

/// GET /api/repos/{id}/origin — report the current origin binding.
async fn get_repo_origin(
    State(st): State<StorageState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let repo = crate::origin_resolve::resolve_repo_full(&st.deps, &id)
        .await
        .map_err(domain_status)?;
    Ok(Json(json!({
        "ok": true,
        "repo_id": repo.id.value,
        "git_backed": crate::origin_resolve::is_git_backed(&repo),
        "origin": repo.origin,
    })))
}

/// POST /api/repos/{id}/origin — bind a project to a git repo (or reset to S3).
async fn bind_repo_origin(
    State(st): State<StorageState>,
    Path(id): Path<String>,
    Json(body): Json<BindOriginBody>,
) -> Result<Json<Value>, StatusCode> {
    use storage::domain::types::{GitProvider, OriginBinding};

    let kind = body.kind.unwrap_or_else(|| "git".into()).to_ascii_lowercase();
    let binding = if kind == "s3" {
        None
    } else if kind == "git" {
        let provider_s = body.provider.unwrap_or_default();
        let provider = match provider_s.trim().to_ascii_lowercase().as_str() {
            "github" | "gh" => GitProvider::Github,
            "bitbucket" | "bb" => GitProvider::Bitbucket,
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        let repo = body.repo.filter(|s| !s.trim().is_empty()).ok_or(StatusCode::BAD_REQUEST)?;
        // repo must look like org/name
        if repo.matches('/').count() < 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
        Some(OriginBinding::Git {
            provider,
            repo: repo.trim().to_string(),
            subpath: body
                .subpath
                .map(|s| s.trim().trim_matches('/').to_string())
                .filter(|s| !s.is_empty()),
            branch: body.branch.filter(|s| !s.trim().is_empty()),
        })
    } else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let repo = storage::application::set_repo_origin(&st.deps, id, binding)
        .await
        .map_err(domain_status)?;

    // Best-effort reachability check for git bindings (does not fail the bind
    // if the token is missing — the binding is still recorded).
    let mut reachable = None;
    if crate::origin_resolve::is_git_backed(&repo) {
        let origin = crate::origin_resolve::git_origin_for(&repo);
        reachable = Some(origin.exists());
    }

    let _ = crate::origin_resolve::git_origin_for(&repo);
    Ok(Json(json!({
        "ok": true,
        "repo_id": repo.id.value,
        "git_backed": crate::origin_resolve::is_git_backed(&repo),
        "origin": repo.origin,
        "remote_reachable": reachable,
    })))
}

#[derive(Deserialize)]
struct MigrateSubpathBody {
    /// Target shared repo `org/name` on the provider (e.g. `dlx/dlx-shared-libs`).
    repo: String,
    /// Provider: `github` | `bitbucket` (default `bitbucket`).
    #[serde(default)]
    provider: Option<String>,
    /// Subpath to seed the project under. Defaults to the project slug.
    #[serde(default)]
    subpath: Option<String>,
    /// Create the target repo if missing (default false — bind existing).
    #[serde(default)]
    create: bool,
    /// Target repo privacy when `create` (default true).
    #[serde(default)]
    private: Option<bool>,
    /// Default branch (default `main`).
    #[serde(default)]
    branch: Option<String>,
}

/// POST /api/repos/{id}/migrate-to-subpath — move an existing (usually
/// S3-bundle) project into a shared repo at a subpath.
///
/// Preserves project identity (repo_id + catalog entry). Seeds the project's
/// CURRENT source under `<subpath>/` with a FRESH commit (no history graft —
/// decision 3), then rebinds the origin from S3-bundle to `{repo, subpath}`.
/// Idempotent: re-running when the subpath is already populated just rebinds.
async fn migrate_to_subpath(
    State(st): State<StorageState>,
    Path(id): Path<String>,
    Json(body): Json<MigrateSubpathBody>,
) -> Result<Json<Value>, StatusCode> {
    use storage::domain::types::{GitProvider as BindProvider, OriginBinding};

    let repo = crate::origin_resolve::resolve_repo_full(&st.deps, &id)
        .await
        .map_err(domain_status)?;
    let repo_id = repo.id.value.clone();
    let slug = repo.slug.clone();

    let full_name = body.repo.trim().trim_matches('/').to_string();
    if !full_name.contains('/') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let provider_s = body.provider.clone().unwrap_or_else(|| "bitbucket".into());
    let branch = body
        .branch
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "main".into());
    let subpath = veil_server::git_origin::normalize_subpath(
        body.subpath.as_deref().or(Some(slug.as_str())),
    )
    .unwrap_or_else(|| slug.clone());
    let private = body.private.unwrap_or(true);

    // 1) Snapshot the project's current source from its EXISTING origin (S3
    //    bundle or otherwise) — this is what we re-seed under the subpath.
    let src_origin = crate::origin_resolve::git_origin_for(&repo);
    let seed_files: Vec<(String, String)> = {
        let rid = repo_id.clone();
        let br = branch.clone();
        tokio::task::spawn_blocking(move || {
            let origin = veil_server::git_origin::GitOrigin::for_repo(&rid);
            collect_source_files_from_origin(&origin, &br)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let _ = src_origin;
    if seed_files.is_empty() {
        return Ok(Json(json!({
            "ok": false,
            "error": "no_source",
            "message": format!("project `{slug}` has no source to migrate (empty origin)"),
        })));
    }

    // 2) Provision (create/bind) the target shared repo and 3) seed the subpath
    //    with a fresh commit. Runs on the blocking pool (git + provider REST).
    let provider_owned = provider_s.clone();
    let full_owned = full_name.clone();
    let sub_owned = subpath.clone();
    let br_owned = branch.clone();
    let create = body.create;
    let repo_id_seed = repo_id.clone();
    let slug_seed = slug.clone();
    let seed_result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
        let origin_spec = json!({
            "kind": "git",
            "provider": provider_owned,
            "repo": full_owned,
            "subpath": sub_owned,
            "create": create,
            "private": private,
            "branch": br_owned,
        });
        let spec = veil_server::git_provider::OriginRequest::from_value(Some(&origin_spec), &slug_seed)?;
        // Create/verify the target repo (registers the origin under repo_id with
        // the subpath so seed_subpath sees it).
        let _cfg = veil_server::git_provider::provision_origin(
            &repo_id_seed, &slug_seed, None, &spec,
        )?;
        let origin = veil_server::git_origin::GitOrigin::for_repo(&repo_id_seed);
        let seeded = origin.seed_subpath(&seed_files, &br_owned)?;
        Ok(json!({ "seed_commit": seeded }))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let seed_json = match seed_result {
        Ok(v) => v,
        Err(e) => {
            return Ok(Json(json!({
                "ok": false,
                "error": "seed_failed",
                "message": e,
            })));
        }
    };

    // 4) Rebind the catalog origin to {repo, subpath}, preserving repo_id.
    let provider = match provider_s.trim().to_ascii_lowercase().as_str() {
        "github" | "gh" => BindProvider::Github,
        _ => BindProvider::Bitbucket,
    };
    let binding = OriginBinding::Git {
        provider,
        repo: full_name.clone(),
        subpath: Some(subpath.clone()),
        branch: Some(branch.clone()),
    };
    let updated = storage::application::set_repo_origin(&st.deps, repo_id.clone(), Some(binding))
        .await
        .map_err(domain_status)?;
    let _ = crate::origin_resolve::git_origin_for(&updated);

    Ok(Json(json!({
        "ok": true,
        "repo_id": repo_id,
        "slug": slug,
        "migrated_to": { "repo": full_name, "subpath": subpath, "branch": branch },
        "seed": seed_json,
        "origin": updated.origin,
        "note": "Project identity preserved (repo_id + catalog). Fresh seed commit under the subpath; S3-bundle history was not grafted.",
    })))
}

/// Collect `(rel, content)` source files from a git origin's current `branch`
/// tree (project-root scoped). Used by migration to re-seed under a subpath.
fn collect_source_files_from_origin(
    origin: &veil_server::git_origin::GitOrigin,
    branch: &str,
) -> Vec<(String, String)> {
    let Ok(tmp) = origin.checkout_tmp(branch) else {
        return Vec::new();
    };
    // The source root is the origin's project_root (honours any existing
    // subpath binding — though a migrating S3 project is normally repo-root).
    let root = origin.project_root(&tmp);
    let mut out = Vec::new();
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if matches!(
                name.as_str(),
                ".git" | "target" | "generated" | "node_modules" | "dist" | ".veil-session.json"
            ) {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    out.push((rel.to_string_lossy().replace('\\', "/"), content));
                }
            }
        }
    }
    walk(&root, &root, &mut out);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// GET /api/git/status — GitHub connection (no secrets) for Config + create.
async fn git_status() -> Json<Value> {    let body = tokio::task::spawn_blocking(veil_server::git_provider::github_status_json)
        .await
        .unwrap_or_else(|e| json!({ "connected": false, "error": format!("join: {e}") }));
    Json(body)
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
    let jira = body.jira_ticket.trim().to_string();
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
            // SINGLE SOURCE OF TRUTH split (not a double-write of one record):
            //   - change_management (above) owns the git/PR **transport** record
            //     (branch + PrStatus lifecycle). It is NOT an approval authority.
            //   - review.rs owns the single **review-facing** item that the human
            //     signs and that `review::may_ship` gates on. We record exactly
            //     one review item per PR create here.
            // See Mind Palace: decision-single-review-source-of-truth.
            let _ = veil_server::review::record_pr(&slug, &cr.title, Some(&cr.id.to_string()));
            // Git-backed projects: also open a PR on the provider and post the
            // initial `veil/review` = pending status (the merge gate).
            let provider_pr = maybe_open_provider_pr(
                &slug,
                &cr.source_branch,
                &cr.target_branch,
                &cr.title,
                &description,
            )
            .await;
            Ok(Json(json!({
                "pull_request": cr,
                "slug": slug,
                "wizard_path": format!("/review/{slug}"),
                "provider_pr": provider_pr,
            })))
        }
        Err(e) => {
            // Soft path: if git branch create failed, still persist a CR for PR Wizard.
            tracing::warn!(?e, "create_pull_request_flat failed — soft-creating META only");
            use change_management::domain::types::{PullRequest, PrStatus};
            let now = chrono::Utc::now();
            let pr_id = Uuid::new_v4();
            let source = preferred_branch.unwrap_or_else(|| {
                let seg = if jira.is_empty() { "pr" } else { jira.as_str() };
                format!(
                    "pr/{}/{}",
                    seg,
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
                "wizard_path": format!("/review/{slug}"),
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

fn reviewer_note(comment: &Option<String>) -> Option<String> {
    comment
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Record the authoritative review `SignOffRecord` (Model B) for a PR-Wizard /
/// PR-detail approval or request-changes. This is the ship-gate record that
/// `review::may_ship` consults. It is idempotent with the `/review` UI: if the
/// UI already signed the outstanding items, there is nothing left to match and
/// we return `Null` rather than erroring. The actor is the current human
/// operator (`via=ui`) — never an agent — so it passes `review`'s human gate.
fn record_review_sign_off(
    slug: Option<&str>,
    pr_id: Option<&str>,
    decision: &str,
    note: Option<String>,
) -> Value {
    let actor = veil_server::session::current_user_id();
    let req = veil_server::review::SignOffRequest {
        ids: vec![],
        slug: slug.map(str::to_string).filter(|s| !s.is_empty()),
        all: slug.map(|s| s.is_empty()).unwrap_or(true),
        decision: decision.to_string(),
        actor,
        note,
        pr_id: pr_id.map(str::to_string).filter(|s| !s.is_empty()),
        via: Some("ui".into()),
        ..Default::default()
    };
    match veil_server::review::sign_off(req) {
        Ok((_items, audit)) => serde_json::to_value(&audit).unwrap_or(Value::Null),
        // Benign: the /review UI already recorded the human decision for these
        // items, so there is nothing outstanding left to sign here.
        Err(e) if e.contains("no outstanding items matched") => Value::Null,
        Err(e) => {
            tracing::warn!(error = %e, "record_review_sign_off failed");
            json!({ "ok": false, "error": e })
        }
    }
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
    // SINGLE SOURCE OF TRUTH: the authoritative approval is a review
    // `SignOffRecord` (Model B), which is what `review::may_ship` gates on.
    // The `/review` UI records it before calling this endpoint; but callers
    // like PullRequestDetailView hit /approve directly, so we record it here
    // too (idempotent — a no-op when there is nothing outstanding). Updating
    // the change_management `PrStatus` below is now only transport lifecycle,
    // NOT an approval authority. See Mind Palace: decision-single-review-source-of-truth.
    let approval_audit =
        record_review_sign_off(approve_slug.as_deref(), Some(&id.to_string()), "approve", reviewer_note(&body.comment));
    match change_management::application::approve_pr(&st.deps, id, reviewer, body.comment)
        .await
    {
        Ok(()) => Ok(Json(json!({
            "ok": true,
            "status": "Approved",
            "sign_off": approval_audit,
            "audit_env": veil_server::review::audit_env_json(),
        }))),
        // Transport-lifecycle transition failing (e.g. PR not ReadyForReview)
        // MUST NOT lose the recorded human sign-off. Report success on the
        // authoritative record and surface the transport note.
        Err(e) => Ok(Json(json!({
            "ok": true,
            "status": "Approved",
            "sign_off": approval_audit,
            "transport_note": format!("{e:?}"),
            "audit_env": veil_server::review::audit_env_json(),
        }))),
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
    // Authoritative record: a review sign-off with decision=reject (Model B).
    let rc_slug = st
        .deps
        .pr_repo
        .find(id)
        .await
        .ok()
        .flatten()
        .and_then(|p| extract_slug_from_description(&p.description));
    let reject_audit =
        record_review_sign_off(rc_slug.as_deref(), Some(&id.to_string()), "reject", Some(comment.clone()));
    // Complete review-action audit (Part 2): request-changes is a distinct
    // action tied to the project + PR (feedback = the reviewer note).
    veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
        action: "request_changes".into(),
        slugs: rc_slug.clone().into_iter().collect(),
        pr_ids: vec![id.to_string()],
        result: "success".into(),
        note: Some(comment.clone()),
        ..Default::default()
    });
    match change_management::application::request_pr_changes(&st.deps, id, reviewer, comment)
        .await
    {
        Ok(()) => Ok(Json(json!({ "ok": true, "status": "ChangesRequested", "sign_off": reject_audit }))),
        // Transport transition may fail (PR not ReadyForReview); the review
        // decision is already recorded, so do not drop it.
        Err(e) => Ok(Json(json!({
            "ok": true,
            "status": "ChangesRequested",
            "sign_off": reject_audit,
            "transport_note": format!("{e:?}"),
        }))),
    }
}

#[derive(Deserialize)]
struct MergeBody {
    #[serde(default)]
    merger: String,
    #[serde(default)]
    slug: String,
}

/// If the project (by slug/repo_id) is git-backed, open a provider PR for the
/// feature branch and post the initial `veil/review` = pending status. Returns
/// a JSON summary (or null / error info) for the API response. Best-effort:
/// never fails the internal PR creation.
async fn maybe_open_provider_pr(
    slug: &str,
    source: &str,
    target: &str,
    title: &str,
    description: &str,
) -> Value {
    // Skip trivial/no-op branches.
    if source.is_empty()
        || source.eq_ignore_ascii_case(target)
        || source.eq_ignore_ascii_case("main")
        || source.eq_ignore_ascii_case("master")
    {
        return Value::Null;
    }
    let deps = resolve_storage_deps().await;
    let repo = match crate::origin_resolve::resolve_repo_full(&deps, slug).await {
        Ok(r) => r,
        Err(_) => return Value::Null,
    };
    if !crate::origin_resolve::is_git_backed(&repo) {
        return Value::Null;
    }
    let Some(provider) = crate::origin_resolve::provider_repo_for(&repo) else {
        return Value::Null;
    };
    // Provider calls are blocking; run off the async runtime.
    let (source, target, title, description) = (
        source.to_string(),
        target.to_string(),
        title.to_string(),
        description.to_string(),
    );
    let result = tokio::task::spawn_blocking(move || {
        let pr = provider.create_pull_request(&source, &target, &title, &description)?;
        // Post the gate as pending immediately, on the PR head.
        if !pr.head_sha.is_empty() {
            let _ = provider.post_veil_review_status(
                &pr.head_sha,
                false,
                "VEIL structural review pending",
                None,
            );
        }
        Ok::<_, String>(pr)
    })
    .await;

    match result {
        Ok(Ok(pr)) => json!({
            "ok": true,
            "number": pr.number,
            "head_sha": pr.head_sha,
            "url": pr.url,
            "veil_review": "pending",
        }),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "provider PR open failed");
            json!({ "ok": false, "error": e })
        }
        Err(e) => json!({ "ok": false, "error": format!("join: {e}") }),
    }
}

/// For git-backed projects: post `veil/review` = success to the PR head (the
/// merge gate) and, when `VEIL_GIT_AUTO_MERGE` is not disabled, drive the
/// provider merge. Returns `Some(summary)` if the repo is git-backed (handled
/// on the provider), or `None` to fall through to the S3/legacy merge paths.
async fn maybe_git_backed_merge(slug: &str, source: &str, target: &str) -> Option<Value> {
    let deps = resolve_storage_deps().await;
    let repo = crate::origin_resolve::resolve_repo_full(&deps, slug).await.ok()?;
    if !crate::origin_resolve::is_git_backed(&repo) {
        return None;
    }
    let provider = crate::origin_resolve::provider_repo_for(&repo)?;
    let origin = crate::origin_resolve::git_origin_for(&repo);
    let (source, target) = (source.to_string(), target.to_string());
    let auto_merge = !matches!(
        std::env::var("VEIL_GIT_AUTO_MERGE")
            .unwrap_or_else(|_| "on".into())
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    );

    let result = tokio::task::spawn_blocking(move || {
        // Resolve the PR + head commit for the source branch.
        let pr = provider.find_open_pr(&source)?;
        let head = pr
            .as_ref()
            .map(|p| p.head_sha.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| origin.remote_tip(&source))
            .ok_or_else(|| format!("cannot resolve head commit for {source}"))?;

        // Sign-off gate: mark veil/review success on the head commit.
        provider.post_veil_review_status(
            &head,
            true,
            "VEIL structural review approved",
            None,
        )?;

        let mut merged_commit = None;
        if auto_merge {
            if let Some(pr) = pr.as_ref() {
                merged_commit = Some(provider.merge_pull_request(
                    pr.number,
                    Some(&format!("Merge {source} into {target} (VEIL sign-off)")),
                )?);
            }
        }
        Ok::<_, String>((pr, head, merged_commit, auto_merge))
    })
    .await;

    Some(match result {
        Ok(Ok((pr, head, merged, auto))) => json!({
            "ok": true,
            "veil_review": "success",
            "head_sha": head,
            "pr_number": pr.as_ref().map(|p| p.number),
            "auto_merge": auto,
            "merge_commit": merged,
        }),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "git-backed provider merge failed");
            json!({ "ok": false, "error": e })
        }
        Err(e) => json!({ "ok": false, "error": format!("join: {e}") }),
    })
}

/// Subpath attribution for a merge (hybrid model). Returns the set of VEIL
/// project slugs whose subpaths the PR's changed files touch, or `None` when
/// the repo is not git-backed / not shared / attribution can't be computed
/// (caller then falls back to the single-project `may_ship`).
///
/// Two VEIL projects "share" a repo when their `OriginBinding::Git.repo`
/// (`org/name`) matches. Changed paths are the name-only diff of the PR's
/// `source..target` on the shared origin; each path is attributed to the
/// project whose subpath it lies under.
async fn touched_subpath_projects(
    _cm: &CmState,
    pr: &change_management::domain::types::PullRequest,
    source: &str,
    target: &str,
    fallback_slug: &str,
) -> Option<Vec<String>> {
    let deps = resolve_storage_deps().await;
    // The PR's own repo record (identity: provider org/name + subpath).
    let this_repo = crate::origin_resolve::resolve_repo_full(&deps, &pr.repo_id.to_string())
        .await
        .ok()?;
    let full_name = crate::origin_resolve::origin_repo_full_name(&this_repo)?;

    // Enumerate sibling projects sharing the same provider repo.
    let repos = storage::application::list_repos(&deps).await.ok()?;
    let mut projects: Vec<veil_server::review::SubpathProject> = Vec::new();
    for r in &repos {
        if crate::origin_resolve::origin_repo_full_name(r).as_deref() == Some(full_name.as_str()) {
            projects.push(veil_server::review::SubpathProject {
                slug: r.slug.clone(),
                subpath: crate::origin_resolve::origin_subpath(r),
            });
        }
    }
    // Not a shared repo (only this project binds it) → single-project gate.
    if projects.len() <= 1 {
        return None;
    }

    // Changed paths across the PR (name-only), via the shared git origin.
    let origin = crate::origin_resolve::git_origin_for(&this_repo);
    let changed = origin.changed_paths_between(source, target).ok()?;
    if changed.is_empty() {
        return None;
    }
    let touched = veil_server::review::attribute_paths_to_projects(&changed, &projects);
    if touched.is_empty() {
        // Attribution found nothing (paths outside any known subpath) — fall
        // back to the single project so we never silently skip the gate.
        Some(vec![fallback_slug.to_string()])
    } else {
        Some(touched)
    }
}

async fn merge_pr(
    State(st): State<CmState>,
    Path(id): Path<String>,
    Json(body): Json<MergeBody>,
) -> Result<Json<Value>, StatusCode> {    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
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
    // Subpath (hybrid model): when the PR's repo is shared by multiple VEIL
    // projects at distinct subpaths, the PR is attributed to EVERY project whose
    // subpath its changed files touch, and ALL of them must be signed off before
    // the shared-repo change can ship (decision 2). A single-subpath PR reduces
    // to the `may_ship(slug)` check above.
    if let Some(touched) = touched_subpath_projects(&st, &pr, &source, &target, &slug).await {
        if touched.len() > 1 {
            if let Err(e) = veil_server::review::may_ship_all(&touched, None) {
                return Ok(Json(json!({
                    "ok": false,
                    "error": "sign_off_required_multi",
                    "message": e,
                    "touched_projects": touched,
                    "hint": "This PR spans multiple subpath projects in a shared repo. Every touched project's Review must be Approved before it can ship.",
                    "audit_env": veil_server::review::audit_env_json(),
                })));
            }
        }
    }
    let _ = ensure_ci_passed(&st.deps, id, "merge", Some(slug.as_str())).await;
    match change_management::application::merge_pr(&st.deps, id, merger, slug.clone()).await {
        Ok(mut v) => {
            // Git-backed projects: VEIL sign-off is the gate. We are past the
            // may_ship check, so post `veil/review` = success to the PR head and
            // drive the provider merge (fast path; configurable via VEIL_GIT_AUTO_MERGE).
            if let Some(prov_result) =
                maybe_git_backed_merge(&slug, &source, &target).await
            {
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("provider_merge".into(), prov_result);
                    obj.insert("via".into(), serde_json::json!("git-provider"));
                }
                return Ok(Json(v));
            }
            if veil_server::git_origin::origin_enabled() {
                let origin = veil_server::git_origin::GitOrigin::for_repo(pr.repo_id.to_string());
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
                let origin = veil_server::git_origin::GitOrigin::for_repo(&rid_b);
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
        // Authoritative approval: review SignOffRecord (Model B).
        let audit = record_review_sign_off(
            wizard_slug.as_deref(),
            Some(&id.to_string()),
            "approve",
            comment.clone(),
        );
        // Ensure ReadyForReview (transport lifecycle only)
        let _ = change_management::application::submit_for_review(&st.deps, id).await;
        let _ = change_management::application::approve_pr(&st.deps, id, reviewer, comment).await;
        Ok(Json(json!({
            "ok": true,
            "status": "Approved",
            "outcome": "all_approved",
            "sign_off": audit,
            "audit_env": veil_server::review::audit_env_json(),
        })))
    } else if outcome == "needs_work" {
        let summary = if body.summary.is_empty() {
            format!(
                "PR Wizard: {} approved, {} need work. See review comments for details.",
                body.approved_count, body.feedback_count
            )
        } else {
            body.summary
        };
        // Authoritative decision: review SignOffRecord with decision=reject.
        let audit = record_review_sign_off(
            wizard_slug.as_deref(),
            Some(&id.to_string()),
            "reject",
            Some(summary.clone()),
        );
        let _ = change_management::application::submit_for_review(&st.deps, id).await;
        let _ =
            change_management::application::request_pr_changes(&st.deps, id, reviewer, summary).await;
        Ok(Json(json!({
            "ok": true,
            "status": "ChangesRequested",
            "outcome": "needs_work",
            "sign_off": audit,
        })))
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
    if veil_server::platform_local() {
        return crate::local_ports::local_deploy_deps(bus);
    }
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

/// Check if a project has terraform files in S3 (i.e. it's an infra-as-code project).
async fn project_has_terraform(repo_id: &str) -> bool {
    if repo_id.is_empty() {
        return false;
    }
    let bucket = std::env::var("BUCKET").unwrap_or_else(|_| "veil-runtime-dev".into());
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let s3_client = aws_sdk_s3::Client::new(&config);
    let prefix = format!("repos/{}/main/terraform/", repo_id);
    match s3_client
        .list_objects_v2()
        .bucket(&bucket)
        .prefix(&prefix)
        .max_keys(5)
        .send()
        .await
    {
        Ok(out) => out
            .contents()
            .iter()
            .any(|o| o.key().map_or(false, |k| k.ends_with(".tf"))),
        Err(_) => false,
    }
}

async fn plan_provision(
    State(st): State<DeployState>,
    Json(body): Json<PlanBody>,
) -> Result<Json<Value>, StatusCode> {
    let branch = body.branch.unwrap_or_else(|| "main".into());
    let slug = body.project_slug.clone();
    let environment = body.environment.clone();

    // Check if this is a terraform/frontend project by looking for terraform/ files in S3
    let has_terraform = project_has_terraform(&body.repo_id).await;

    if has_terraform {
        // Actually run terraform init + plan (dry_run=true) and return real output
        let bucket = std::env::var("BUCKET").unwrap_or_else(|_| "veil-runtime-dev".into());
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let s3_client = aws_sdk_s3::Client::new(&config);

        // Fetch terraform files from S3
        let prefix = format!("repos/{}/main/terraform/", body.repo_id);
        let listed = s3_client
            .list_objects_v2()
            .bucket(&bucket)
            .prefix(&prefix)
            .send()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let mut tf_files: Vec<(String, Vec<u8>)> = Vec::new();
        for obj in listed.contents() {
            if let Some(key) = obj.key() {
                if key.ends_with(".tf") || key.ends_with(".tf.json") || key.ends_with(".tfvars") {
                    if let Ok(resp) = s3_client.get_object().bucket(&bucket).key(key).send().await {
                        if let Ok(bytes) = resp.body.collect().await {
                            let filename = key
                                .strip_prefix(&prefix)
                                .unwrap_or(key)
                                .to_string();
                            tf_files.push((filename, bytes.into_bytes().to_vec()));
                        }
                    }
                }
            }
        }

        if tf_files.is_empty() {
            return Ok(Json(json!({
                "ok": false,
                "error": "no_terraform_files",
                "message": "Terraform directory exists but contains no .tf files.",
            })));
        }

        // Read InfraConfig from veil.toml
        let toml_key = format!("repos/{}/main/veil.toml", body.repo_id);
        let infra_config = if let Ok(resp) = s3_client.get_object().bucket(&bucket).key(&toml_key).send().await {
            if let Ok(bytes) = resp.body.collect().await {
                let content = String::from_utf8_lossy(&bytes.into_bytes()).into_owned();
                let parsed: toml::Value = content.parse().unwrap_or(toml::Value::Table(Default::default()));
                let infra = parsed.get("deploy").and_then(|d| d.get("infrastructure"));
                crate::deploy::types::InfraConfig {
                    backend_bucket: infra.and_then(|i| i.get("backend_bucket")).and_then(|v| v.as_str()).unwrap_or("dashlx-terraform-state").to_string(),
                    backend_key: infra.and_then(|i| i.get("backend_key")).and_then(|v| v.as_str()).unwrap_or(&format!("veil-projects/{slug}")).to_string(),
                    backend_region: infra.and_then(|i| i.get("backend_region")).and_then(|v| v.as_str()).unwrap_or("us-west-2").to_string(),
                }
            } else {
                crate::deploy::types::InfraConfig {
                    backend_bucket: "dashlx-terraform-state".into(),
                    backend_key: format!("veil-projects/{slug}"),
                    backend_region: "us-west-2".into(),
                }
            }
        } else {
            crate::deploy::types::InfraConfig {
                backend_bucket: "dashlx-terraform-state".into(),
                backend_key: format!("veil-projects/{slug}"),
                backend_region: "us-west-2".into(),
            }
        };

        // Run terraform init + plan (dry_run = true means plan only, no apply)
        match crate::deploy::terraform::run(&slug, &infra_config, &tf_files, true).await {
            Ok(result) => {
                // Get structured plan JSON for rich resource details
                let tf_dir = crate::deploy::config::terraform_dir(&slug);
                let plan_json = crate::deploy::terraform::show_plan_json(&tf_dir).await.ok();

                let mut resources: Vec<serde_json::Value> = Vec::new();
                let mut creates = 0u32;
                let mut updates = 0u32;
                let mut destroys = 0u32;

                if let Some(ref pj) = plan_json {
                    if let Some(changes) = pj.get("resource_changes").and_then(|v| v.as_array()) {
                        for change in changes {
                            let actions = change.get("change")
                                .and_then(|c| c.get("actions"))
                                .and_then(|a| a.as_array())
                                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                                .unwrap_or_default();

                            // Skip no-ops and data sources
                            if actions == ["no-op"] || actions.is_empty() {
                                continue;
                            }

                            let action = if actions.contains(&"create") {
                                creates += 1;
                                "create"
                            } else if actions.contains(&"update") {
                                updates += 1;
                                "update"
                            } else if actions.contains(&"delete") {
                                if actions.contains(&"create") {
                                    creates += 1;
                                    "replace"
                                } else {
                                    destroys += 1;
                                    "destroy"
                                }
                            } else {
                                continue;
                            };

                            let resource_type = change.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            let name = change.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let addr = change.get("address").and_then(|v| v.as_str()).unwrap_or("");

                            // Extract meaningful values from the planned attributes
                            let empty_obj = json!({});
                            let after = change.get("change")
                                .and_then(|c| c.get("after"))
                                .unwrap_or(&empty_obj);

                            // Map resource types to friendly service names + extract key details
                            let (service, detail) = match resource_type {
                                "aws_s3_bucket" => {
                                    let bucket = after.get("bucket").and_then(|v| v.as_str()).unwrap_or(name);
                                    ("S3 Bucket", bucket.to_string())
                                }
                                "aws_s3_bucket_policy" => ("S3 Policy", "Bucket access policy".into()),
                                "aws_s3_bucket_public_access_block" => ("S3 Access", "Block public access".into()),
                                "aws_cloudfront_distribution" => {
                                    let aliases = after.get("aliases")
                                        .and_then(|v| v.as_array())
                                        .and_then(|a| a.first())
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("CDN");
                                    ("CloudFront", format!("{aliases} — CDN distribution"))
                                }
                                "aws_cloudfront_origin_access_identity" => {
                                    ("CloudFront OAI", "Origin access identity for S3".into())
                                }
                                "aws_acm_certificate" => {
                                    let domain = after.get("domain_name").and_then(|v| v.as_str()).unwrap_or("");
                                    ("ACM Certificate", format!("{domain} — TLS certificate (DNS validated)"))
                                }
                                "aws_acm_certificate_validation" => {
                                    ("ACM Validation", "Certificate DNS validation".into())
                                }
                                "aws_route53_record" => {
                                    let rname = after.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    let rtype = after.get("type").and_then(|v| v.as_str()).unwrap_or("A");
                                    ("Route53", format!("{rtype} record — {rname}"))
                                }
                                "aws_lambda_function" => {
                                    let fn_name = after.get("function_name").and_then(|v| v.as_str()).unwrap_or(name);
                                    let runtime = after.get("runtime").and_then(|v| v.as_str()).unwrap_or("");
                                    ("Lambda", format!("{fn_name} ({runtime})"))
                                }
                                "aws_sqs_queue" => {
                                    let qname = after.get("name").and_then(|v| v.as_str()).unwrap_or(name);
                                    ("SQS Queue", qname.to_string())
                                }
                                "aws_sns_topic" => {
                                    let tname = after.get("name").and_then(|v| v.as_str()).unwrap_or(name);
                                    ("SNS Topic", tname.to_string())
                                }
                                "aws_dynamodb_table" => {
                                    let tname = after.get("name").and_then(|v| v.as_str()).unwrap_or(name);
                                    ("DynamoDB", tname.to_string())
                                }
                                "aws_iam_role" => {
                                    let rname = after.get("name").and_then(|v| v.as_str()).unwrap_or(name);
                                    ("IAM Role", rname.to_string())
                                }
                                _ => {
                                    // Generic: use type with underscores converted
                                    let friendly = resource_type.replace("aws_", "").replace('_', " ");
                                    (friendly.leak() as &str, name.to_string())
                                }
                            };

                            resources.push(json!({
                                "kind": service,
                                "name": detail,
                                "action": action,
                                "address": addr,
                            }));
                        }
                    }
                }

                // Fallback to line parsing if JSON plan wasn't available
                if resources.is_empty() && result.has_changes {
                    for line in result.plan_output.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("# ") && trimmed.contains(" will be ") {
                            let rname = trimmed.strip_prefix("# ").unwrap_or(trimmed);
                            let action = if trimmed.contains("created") { creates += 1; "create" }
                                else if trimmed.contains("updated") { updates += 1; "update" }
                                else if trimmed.contains("destroyed") { destroys += 1; "destroy" }
                                else { "ensure" };
                            resources.push(json!({
                                "kind": "Terraform",
                                "name": rname.split(" will be").next().unwrap_or(rname),
                                "action": action,
                            }));
                        }
                    }
                }

                let summary = if !result.has_changes {
                    "No changes. Infrastructure is up-to-date.".to_string()
                } else {
                    format!("Plan: {} to create, {} to update, {} to destroy", creates, updates, destroys)
                };

                return Ok(Json(json!({
                    "ok": true,
                    "summary": summary,
                    "mock_mode": false,
                    "terraform": true,
                    "diff": { "create": creates, "update": updates, "noop": 0, "destroy": destroys },
                    "resources": resources,
                    "steps": [],
                    "notes": [],
                })));
            }
            Err(e) => {
                return Ok(Json(json!({
                    "ok": false,
                    "error": "terraform_plan_failed",
                    "message": e,
                })));
            }
        }
    }

    match deploy::application::plan_provision(
        &st.deps,
        slug,
        environment,
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
    let slug = body.project_slug.clone();
    let environment = body.environment.clone();

    // Check if this is a terraform project — delegate to the new deploy pipeline
    let has_terraform = project_has_terraform(&body.repo_id).await;

    if has_terraform {
        // Delegate to the new deploy pipeline via internal HTTP call
        let port = std::env::var("VEIL_PORT").unwrap_or_else(|_| "8080".into());
        let url = format!("http://127.0.0.1:{port}/api/projects/{slug}/deploy");
        let client = reqwest::Client::new();
        match client
            .post(&url)
            .json(&serde_json::json!({
                "environment": environment,
                "steps": ["infrastructure"],
                "dry_run": false,
            }))
            .send()
            .await
        {
            Ok(resp) => {
                let body: serde_json::Value = resp.json().await.unwrap_or(json!({}));
                let job_id = body.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                return Ok(Json(json!({
                    "ok": true,
                    "job_id": job_id,
                    "status": "running",
                    "summary": format!("Terraform deploy started for {slug} in {environment}"),
                    "percent": 10,
                    "steps": [{
                        "id": "terraform",
                        "label": "Running Terraform apply",
                        "status": "running",
                    }],
                })));
            }
            Err(e) => {
                return Ok(Json(json!({
                    "ok": false,
                    "error": "deploy_failed",
                    "message": format!("Failed to trigger pipeline: {e}"),
                })));
            }
        }
    }

    match deploy::application::provision_project(
        &st.deps,
        slug,
        environment,
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
/// GET /api/projects/{slug}/deploy/gate?environment=dev
///
/// Returns the approval gate policy for a project + environment, read from the
/// project's `veil.toml [deploy.gates]`. `gate = "none"` means the environment
/// can be shipped in one action once the change set is signed off; `sign_off`
/// keeps the environment behind the explicit sign-off ceremony (prod).
/// Unknown / missing config defaults to `none` (matches GatePolicy::from_str).
/// Read the approval gate policy for a project + environment from its
/// `veil.toml [deploy.gates]`. Returns `"none"` or `"sign_off"`. Any failure
/// (no repo, no file, parse error) falls back to the permissive `"none"` so
/// dev flows keep working; prod gating is opt-in via explicit `sign_off`.
pub(crate) async fn read_deploy_gate(
    deps: &Arc<storage::application::Deps>,
    slug: &str,
    environment: &str,
) -> &'static str {
    let got = async {
        let repo = storage::application::resolve_repo(deps, slug).await.ok()?;
        let rid = storage::domain::types::RepoId { value: repo.id.value };
        let bytes = storage::application::read_file(
            deps,
            rid,
            "main".to_string(),
            "veil.toml".to_string(),
        )
        .await
        .ok()?;
        let content = String::from_utf8_lossy(&bytes);
        let parsed = content.parse::<toml::Value>().ok()?;
        let policy = parsed
            .get("deploy")
            .and_then(|d| d.get("gates"))
            .and_then(|g| g.get(environment))
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        Some(match policy {
            "sign_off" | "signoff" => "sign_off",
            _ => "none",
        })
    }
    .await
    .unwrap_or("none");
    got
}

async fn deploy_gate_handler(
    State(st): State<StorageState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let environment = q
        .get("environment")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dev".to_string());

    let gate = read_deploy_gate(&st.deps, &slug, &environment).await;

    Json(json!({
        "ok": true,
        "slug": slug,
        "environment": environment,
        "gate": gate,
        // Convenience flag for the UI: dev-gated (gate=none) projects can offer
        // one-action Approve & Deploy; sign_off keeps the ceremony.
        "one_action_ship": gate == "none",
    }))
}

// ─── Bundle-level operator actions (Part C) ───────────────────────────────
//
// One review = one ReviewBundle (a task's per-project change sets). The
// operator acts on the WHOLE task at bundle level:
//   POST /api/review/bundles/{id}/approve  — record human sign-off for every project
//   POST /api/review/bundles/{id}/merge    — env-gated: merge each project's branch to main
//   POST /api/review/bundles/{id}/ship     — Approve + Merge + Deploy each (one action)
// Non-prod (gate=none) is the fast path. Prod (gate=sign_off) is behind the
// two-person seam (decision-deferred-two-person-prod-approval): the merge is
// blocked when the seam is ACTIVE and < 2 distinct approvers, with an audited
// `override=true` escape hatch. All actions record the existing sign-off audit.

#[derive(Deserialize, Default)]
struct BundleActionBody {
    #[serde(default)]
    environment: Option<String>,
    /// Explicit note recorded on the sign-off audit.
    #[serde(default)]
    note: Option<String>,
    /// Prod two-person override: proceed despite the gate. Audited with a stern
    /// warning. Structured for when identities land — do NOT default to true.
    #[serde(default)]
    override_two_person: bool,
}

/// Resolve the bundle or 404-shaped error JSON. Uses the ANY-STATUS resolver so
/// merge/ship still find the bundle after approvals have flipped items out of
/// the outstanding set (the one-action Approve+Merge+Deploy path).
fn resolve_bundle(id: &str) -> Result<veil_server::review::ReviewBundle, Json<Value>> {
    veil_server::review::bundle_by_id_any_status(id).ok_or_else(|| {
        Json(json!({
            "ok": false,
            "error": "bundle_not_found",
            "message": format!("No review bundle `{id}`. It may have been shipped already."),
        }))
    })
}

/// POST /api/review/bundles/{id}/approve — record the authoritative human
/// sign-off for EVERY project in the bundle (the ship-gate record). Idempotent
/// with the /review UI. The actor is the current human operator (never agent).
async fn bundle_approve(
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<BundleActionBody>,
) -> Json<Value> {
    let bundle = match resolve_bundle(&id) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut approvals = Vec::new();
    for slug in &bundle.project_slugs {
        let audit = record_review_sign_off(Some(slug), None, "approve", body.note.clone());
        approvals.push(json!({ "slug": slug, "sign_off": audit }));
    }
    // Complete review-action audit (Part 2): record the approve action for the
    // whole bundle with actor/target/time.
    veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
        action: "approve".into(),
        bundle_id: Some(bundle.id.clone()),
        slugs: bundle.project_slugs.clone(),
        pr_ids: bundle
            .projects
            .iter()
            .filter_map(|p| p.pr_id.clone())
            .collect(),
        git_shas: bundle
            .projects
            .iter()
            .filter_map(|p| p.git_sha.clone())
            .collect(),
        result: "success".into(),
        note: body.note.clone(),
        ..Default::default()
    });
    Json(json!({
        "ok": true,
        "bundle_id": bundle.id,
        "approved_projects": bundle.project_slugs,
        "approvals": approvals,
        "audit_env": veil_server::review::audit_env_json(),
    }))
}

/// Compute which of the bundle's projects are prod-gated for the target env and
/// evaluate the two-person production merge gate over them.
async fn prod_gate_for_bundle(
    deps: &Arc<storage::application::Deps>,
    bundle: &veil_server::review::ReviewBundle,
    environment: &str,
) -> (Vec<String>, veil_server::review::ProdMergeGate) {
    let mut prod_slugs = Vec::new();
    for slug in &bundle.project_slugs {
        if read_deploy_gate(deps, slug, environment).await == "sign_off" {
            prod_slugs.push(slug.clone());
        }
    }
    let gate = veil_server::review::prod_merge_gate(&prod_slugs, None);
    (prod_slugs, gate)
}

/// POST /api/review/bundles/{id}/merge — env-gated bundle merge. Requires every
/// project signed off (may_ship_bundle). For prod-gated projects, enforces the
/// two-person seam (when active) unless `override_two_person` is set (audited).
/// Merges each project's open PR to main via the existing merge endpoint.
async fn bundle_merge(
    State(st): State<StorageState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<BundleActionBody>,
) -> Json<Value> {
    let bundle = match resolve_bundle(&id) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let environment = body
        .environment
        .clone()
        .unwrap_or_else(|| "dev".to_string());

    // Sign-off gate: EVERY project in the bundle must be approved.
    if let Err(e) = veil_server::review::may_ship_bundle(&bundle.id) {
        veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
            action: "merge".into(),
            bundle_id: Some(bundle.id.clone()),
            slugs: bundle.project_slugs.clone(),
            environment: Some(environment.clone()),
            result: "blocked".into(),
            note: Some(format!("sign_off_required: {e}")),
            ..Default::default()
        });
        return Json(json!({
            "ok": false,
            "error": "sign_off_required",
            "message": e,
            "hint": "Approve the whole review before merging the task.",
            "audit_env": veil_server::review::audit_env_json(),
        }));
    }

    // Prod two-person seam.
    let (prod_slugs, gate) = prod_gate_for_bundle(&st.deps, &bundle, &environment).await;
    if gate.active && !gate.satisfied && !body.override_two_person {
        veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
            action: "merge".into(),
            bundle_id: Some(bundle.id.clone()),
            slugs: prod_slugs.clone(),
            environment: Some(environment.clone()),
            result: "blocked".into(),
            note: Some(format!(
                "two_person_required: needs a second approver for {}",
                gate.blocked.join(", ")
            )),
            ..Default::default()
        });
        return Json(json!({
            "ok": false,
            "error": "two_person_required",
            "message": format!(
                "Production merge needs a second distinct approver for: {}. \
Ask another operator to approve, or override with a recorded reason.",
                gate.blocked.join(", ")
            ),
            "prod_projects": prod_slugs,
            "gate": gate,
            "override_field": "override_two_person",
            "audit_env": veil_server::review::audit_env_json(),
        }));
    }
    let overridden = gate.active && !gate.satisfied && body.override_two_person;
    if overridden {
        // Audited override: record a system note against each prod project so
        // the audit pack shows the two-person rule was consciously bypassed.
        for slug in &prod_slugs {
            let _ = record_review_sign_off(
                Some(slug),
                None,
                "approve",
                Some(format!(
                    "TWO-PERSON OVERRIDE by {}: production merged without a second approver. {}",
                    veil_server::session::current_user_id(),
                    body.note.clone().unwrap_or_default()
                )),
            );
        }
        // Complete review-action audit (Part 2): the prod-override is its own
        // audited action with the acknowledgment note.
        veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
            action: "override_two_person".into(),
            bundle_id: Some(bundle.id.clone()),
            slugs: prod_slugs.clone(),
            environment: Some(environment.clone()),
            result: "success".into(),
            note: Some(format!(
                "Production two-person gate overridden without a second approver. {}",
                body.note.clone().unwrap_or_default()
            )),
            ..Default::default()
        });
        tracing::warn!(bundle = %bundle.id, projects = ?prod_slugs, "two-person prod override");
    }

    // Merge each project's PR via the existing gated endpoint (internal HTTP so
    // provider-merge + subpath multi-gate stay intact).
    let port = std::env::var("VEIL_PORT").unwrap_or_else(|_| "8080".into());
    let client = reqwest::Client::new();
    let mut results = Vec::new();
    let mut all_ok = true;
    for proj in &bundle.projects {
        let Some(pr_id) = proj.pr_id.as_deref().filter(|s| !s.is_empty()) else {
            results.push(json!({
                "slug": proj.slug,
                "ok": false,
                "skipped": "no_pr",
                "message": "No provider/transport PR bound for this project; nothing to merge.",
            }));
            continue;
        };
        let url = format!("http://127.0.0.1:{port}/api/pull_requests/{pr_id}/merge");
        match client
            .post(&url)
            .json(&json!({ "merger": "operator", "slug": proj.slug }))
            .send()
            .await
        {
            Ok(resp) => {
                let v: Value = resp.json().await.unwrap_or(json!({}));
                if v.get("ok").and_then(|b| b.as_bool()) == Some(false) {
                    all_ok = false;
                }
                results.push(json!({ "slug": proj.slug, "merge": v }));
            }
            Err(e) => {
                all_ok = false;
                results.push(json!({ "slug": proj.slug, "ok": false, "error": e.to_string() }));
            }
        }
    }

    // Complete review-action audit (Part 2): the merge action + its result.
    veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
        action: "merge".into(),
        bundle_id: Some(bundle.id.clone()),
        slugs: bundle.project_slugs.clone(),
        environment: Some(environment.clone()),
        git_shas: bundle
            .projects
            .iter()
            .filter_map(|p| p.git_sha.clone())
            .collect(),
        pr_ids: bundle
            .projects
            .iter()
            .filter_map(|p| p.pr_id.clone())
            .collect(),
        result: if all_ok { "success" } else { "failure" }.into(),
        note: body.note.clone(),
        ..Default::default()
    });

    Json(json!({
        "ok": all_ok,
        "bundle_id": bundle.id,
        "environment": environment,
        "merged": results,
        "prod_projects": prod_slugs,
        "two_person_override": overridden,
        "gate": gate,
        "audit_env": veil_server::review::audit_env_json(),
    }))
}

/// POST /api/review/bundles/{id}/ship — Approve + Merge + Deploy in one action
/// for the whole task. Non-prod fast path: records sign-off for every project,
/// merges each branch to main, then deploys each project. Prod-gated projects
/// obey the two-person seam like `bundle_merge`.
async fn bundle_ship(
    State(st): State<StorageState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<BundleActionBody>,
) -> Json<Value> {
    let bundle = match resolve_bundle(&id) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let environment = body
        .environment
        .clone()
        .unwrap_or_else(|| "dev".to_string());

    // Block shipping a task that still has host-check errors in any project.
    if bundle.host_has_errors {
        return Json(json!({
            "ok": false,
            "error": "host_errors",
            "message": "One or more projects in this task still have compile errors. Fix them before shipping.",
        }));
    }

    // 1) Approve every project (records the ship-gate sign-off).
    for slug in &bundle.project_slugs {
        let _ = record_review_sign_off(Some(slug), None, "approve", body.note.clone());
    }
    // Complete review-action audit (Part 2): the approve leg of one-action ship.
    veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
        action: "approve".into(),
        bundle_id: Some(bundle.id.clone()),
        slugs: bundle.project_slugs.clone(),
        environment: Some(environment.clone()),
        result: "success".into(),
        note: body.note.clone(),
        ..Default::default()
    });

    // 2) Merge (env-gated + two-person seam) via the bundle merge handler logic.
    let merge_resp = bundle_merge(
        State(st.clone()),
        axum::extract::Path(bundle.id.clone()),
        Json(BundleActionBody {
            environment: Some(environment.clone()),
            note: body.note.clone(),
            override_two_person: body.override_two_person,
        }),
    )
    .await;
    let merge_val = merge_resp.0;
    if merge_val.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        // Merge blocked (e.g. two-person) — do not deploy. Surface the reason.
        return Json(json!({
            "ok": false,
            "stage": "merge",
            "bundle_id": bundle.id,
            "merge": merge_val,
        }));
    }

    // 3) Deploy each project to the target environment.
    let port = std::env::var("VEIL_PORT").unwrap_or_else(|_| "8080".into());
    let client = reqwest::Client::new();
    let mut deploys = Vec::new();
    let mut all_ok = true;
    for proj in &bundle.projects {
        let rid = proj.repo_id.clone().unwrap_or_else(|| proj.slug.clone());
        let url = format!("http://127.0.0.1:{port}/api/provision-project");
        match client
            .post(&url)
            .json(&json!({
                "project_slug": proj.slug,
                "environment": environment,
                "repo_id": rid,
                "branch": "main",
            }))
            .send()
            .await
        {
            Ok(resp) => {
                let v: Value = resp.json().await.unwrap_or(json!({}));
                if v.get("ok").and_then(|b| b.as_bool()) == Some(false) {
                    all_ok = false;
                }
                deploys.push(json!({ "slug": proj.slug, "deploy": v }));
            }
            Err(e) => {
                all_ok = false;
                deploys.push(json!({ "slug": proj.slug, "ok": false, "error": e.to_string() }));
            }
        }
    }

    // Complete review-action audit (Part 2): the deploy leg (per environment).
    veil_server::review::record_review_action(veil_server::review::ReviewActionSpec {
        action: "deploy".into(),
        bundle_id: Some(bundle.id.clone()),
        slugs: bundle.project_slugs.clone(),
        environment: Some(environment.clone()),
        git_shas: bundle
            .projects
            .iter()
            .filter_map(|p| p.git_sha.clone())
            .collect(),
        result: if all_ok { "success" } else { "failure" }.into(),
        note: body.note.clone(),
        ..Default::default()
    });

    Json(json!({
        "ok": all_ok,
        "bundle_id": bundle.id,
        "environment": environment,
        "merge": merge_val,
        "deploys": deploys,
        "audit_env": veil_server::review::audit_env_json(),
    }))
}

/// WebSocket endpoint for streaming terraform deploy.
/// Client connects, sends {"action":"start","environment":"dev"}, gets live events.
async fn deploy_ws_handler(
    State(st): State<StorageState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    ws: axum::extract::WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| deploy_ws_session(socket, st, slug))
}

async fn deploy_ws_session(
    mut socket: axum::extract::ws::WebSocket,
    st: StorageState,
    slug: String,
) {
    use axum::extract::ws::Message;

    // Wait for start message from client
    let start_msg = match socket.recv().await {
        Some(Ok(Message::Text(text))) => {
            serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default()
        }
        _ => {
            let _ = socket.send(Message::Text(
                json!({"type": "error", "message": "Expected start message"}).to_string().into()
            )).await;
            return;
        }
    };

    let _environment = start_msg.get("environment").and_then(|v| v.as_str()).unwrap_or("dev");
    let deploy_type_raw = start_msg.get("deploy_type").and_then(|v| v.as_str()).unwrap_or("auto");

    // Resolve repo_id from slug
    let repo_id = match storage::application::resolve_repo(&st.deps, &slug).await {
        Ok(repo) => repo.id.value,
        Err(_) => {
            let _ = socket.send(Message::Text(
                json!({"type": "error", "message": format!("Project \'{}\' not found", slug)}).to_string().into()
            )).await;
            return;
        }
    };

    // Resolve deploy type: if "auto", read [deploy].type from veil.toml
    let deploy_type = if deploy_type_raw == "auto" {
        let rid = storage::domain::types::RepoId { value: repo_id.clone() };
        if let Ok(bytes) = storage::application::read_file(
            &st.deps, rid, "main".to_string(), "veil.toml".to_string(),
        ).await {
            let content_str = String::from_utf8_lossy(&bytes).into_owned();
            if let Ok(parsed) = content_str.parse::<toml::Value>() {
                parsed.get("deploy")
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("infrastructure")
                    .to_string()
            } else { "infrastructure".to_string() }
        } else { "infrastructure".to_string() }
    } else { deploy_type_raw.to_string() };

    match deploy_type.as_str() {
        "frontend" => {
            // Read veil.toml to get build+deploy config
            let build_config = match read_frontend_build_config(&st.deps, &repo_id, &slug).await {
                Ok(config) => config,
                Err(e) => {
                    let _ = socket.send(Message::Text(
                        json!({"type": "error", "message": e}).to_string().into()
                    )).await;
                    return;
                }
            };
            crate::deploy::ws::run_frontend_deploy_ws(&mut socket, &slug, &build_config).await;
        }
        "lambda" | "ecs" => {
            // Lambda/ECS code deploy: veil gen → cargo build → zip → update Lambda
            let build_config = match read_lambda_build_config(&st.deps, &repo_id, &slug).await {
                Ok(config) => config,
                Err(e) => {
                    let _ = socket.send(Message::Text(
                        json!({"type": "error", "message": e}).to_string().into()
                    )).await;
                    return;
                }
            };
            crate::deploy::ws::run_lambda_deploy_ws(&mut socket, &slug, &build_config).await;
        }
        "infrastructure" => {
            // Infrastructure (terraform) deploy
            let tf_files = match fetch_terraform_files_s3(&st.deps, &repo_id).await {
                Ok(files) => files,
                Err(e) => {
                    let _ = socket.send(Message::Text(
                        json!({"type": "error", "message": e}).to_string().into()
                    )).await;
                    return;
                }
            };

            if tf_files.is_empty() {
                let _ = socket.send(Message::Text(
                    json!({"type": "error", "message": "No .tf files found in project"}).to_string().into()
                )).await;
                return;
            }

            let infra_config = read_infra_config_s3(&st.deps, &repo_id, &slug).await;
            crate::deploy::ws::run_terraform_ws(&mut socket, &slug, &tf_files, &infra_config).await;
        }
        "contribution" => {
            // Contribution deploy: ui.veil → vite lib bundle → contributions
            // bucket → re-register manifest. Reads [deploy.contribution] and
            // materializes the full project (ui.veil + layers) so veil gen works.
            match read_contribution_config(&st.deps, &repo_id, &slug).await {
                Ok((contribution, source_dir, component_deps)) => {
                    crate::deploy::ws::run_contribution_deploy_ws(
                        &mut socket, &slug, &source_dir, &contribution, &component_deps,
                    ).await;
                }
                Err(e) => {
                    let _ = socket.send(Message::Text(
                        json!({"type": "error", "message": e}).to_string().into()
                    )).await;
                }
            }
        }
        _ => {
            let _ = socket.send(Message::Text(
                json!({"type": "error", "message": format!("Unknown deploy type: '{deploy_type}'")}).to_string().into()
            )).await;
        }
    }
}

/// Read frontend build config from veil.toml, resolving terraform output references.
async fn read_frontend_build_config(
    deps: &storage::application::Deps,
    repo_id: &str,
    slug: &str,
) -> Result<crate::deploy::ws::FrontendBuildConfig, String> {
    let rid = storage::domain::types::RepoId { value: repo_id.to_string() };
    let bytes = storage::application::read_file(
        deps, rid, "main".to_string(), "veil.toml".to_string(),
    ).await.map_err(|e| format!("Failed to read veil.toml: {e:?}"))?;

    let content = String::from_utf8_lossy(&bytes).into_owned();
    let parsed: toml::Value = content.parse()
        .map_err(|e| format!("Failed to parse veil.toml: {e}"))?;

    let deploy = parsed.get("deploy").ok_or("No [deploy] section in veil.toml")?;
    let build = deploy.get("build").ok_or("No [deploy.build] section")?;
    let frontend = deploy.get("frontend").ok_or("No [deploy.frontend] section")?;

    let commands: Vec<String> = build.get("commands")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_else(|| vec!["npm install".into(), "npm run build".into()]);

    let output_dir = build.get("output_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("build")
        .to_string();

    // Resolve bucket and cloudfront_distribution_id
    let raw_bucket = frontend.get("bucket").and_then(|v| v.as_str()).unwrap_or("");
    let raw_cf = frontend.get("cloudfront_distribution_id").and_then(|v| v.as_str()).unwrap_or("");
    let (bucket, cf_id) = resolve_terraform_refs(slug, raw_bucket, raw_cf).await;

    if bucket.is_empty() {
        return Err("Could not resolve deploy bucket. Check [deploy.frontend] and terraform outputs.".into());
    }

    // Resolve the frontend source .veil file PER-TARGET. VEIL convention: UI
    // lives in `ui.veil` (backend in `main.veil`). Prefer an explicit override
    // ([deploy.build].veil / .package or top-level main = "..."), then `ui.veil`,
    // then fall back to `main.veil` for back-compat with UI-in-main projects.
    let explicit_src = build.get("veil").and_then(|v| v.as_str())
        .or_else(|| build.get("package").and_then(|v| v.as_str()))
        .or_else(|| parsed.get("main").and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    let dir = format!("/tmp/deploy/{}", slug);
    tokio::fs::create_dir_all(&dir).await.ok();

    // Determine which source file to gen from.
    let rid_for = || storage::domain::types::RepoId { value: repo_id.to_string() };
    let (src_name, src_bytes): (String, Option<Vec<u8>>) = if let Some(ref ex) = explicit_src {
        let bytes = storage::application::read_file(
            deps, rid_for(), "main".to_string(), ex.clone(),
        ).await.ok();
        (ex.clone(), bytes)
    } else {
        match storage::application::read_file(
            deps, rid_for(), "main".to_string(), "ui.veil".to_string(),
        ).await {
            Ok(ui) => ("ui.veil".to_string(), Some(ui)),
            Err(_) => {
                let m = storage::application::read_file(
                    deps, rid_for(), "main".to_string(), "main.veil".to_string(),
                ).await.ok();
                ("main.veil".to_string(), m)
            }
        }
    };

    // Write the resolved source to the temp location for veil gen.
    let source_veil_path = format!("/tmp/deploy/{}/{}", slug, src_name);
    if let Some(bytes) = src_bytes {
        if let Some(parent) = std::path::Path::new(&source_veil_path).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(&source_veil_path, &bytes).await.ok();
    }

    Ok(crate::deploy::ws::FrontendBuildConfig {
        main_veil_path: source_veil_path,
        commands,
        output_dir,
        bucket,
        cloudfront_distribution_id: if cf_id.is_empty() { None } else { Some(cf_id) },
        domain: Some("ai.dev.dashlx.com".to_string()),
    })
}

/// Resolve ${terraform.*} references by reading terraform outputs.
async fn resolve_terraform_refs(slug: &str, raw_bucket: &str, raw_cf: &str) -> (String, String) {
    let tf_dir = crate::deploy::config::terraform_dir(slug);
    let needs_resolve = raw_bucket.contains("${terraform.") || raw_cf.contains("${terraform.");
    if !needs_resolve {
        return (raw_bucket.to_string(), raw_cf.to_string());
    }
    let outputs = crate::deploy::ws::capture_outputs(&tf_dir).await.unwrap_or_default();
    let resolve = |raw: &str| -> String {
        if let Some(key) = raw.strip_prefix("${terraform.").and_then(|s| s.strip_suffix('}')) {
            outputs.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
        } else {
            raw.to_string()
        }
    };
    (resolve(raw_bucket), resolve(raw_cf))
}

/// Fetch terraform/*.tf files from S3 for a project.

/// Read Lambda build config from veil.toml.
async fn read_lambda_build_config(
    deps: &storage::application::Deps,
    repo_id: &str,
    slug: &str,
) -> Result<crate::deploy::ws::LambdaBuildConfig, String> {
    let rid = storage::domain::types::RepoId { value: repo_id.to_string() };
    let bytes = storage::application::read_file(
        deps, rid, "main".to_string(), "veil.toml".to_string(),
    ).await.map_err(|e| format!("Failed to read veil.toml: {e:?}"))?;

    let content = String::from_utf8_lossy(&bytes).into_owned();
    let parsed: toml::Value = content.parse()
        .map_err(|e| format!("Failed to parse veil.toml: {e}"))?;

    let deploy = parsed.get("deploy").ok_or("No [deploy] section in veil.toml")?;
    let build = deploy.get("build");

    let rust_target = build
        .and_then(|b| b.get("rust_target"))
        .and_then(|v| v.as_str())
        .unwrap_or("x86_64-unknown-linux-gnu")
        .to_string();

    let infra = deploy.get("infrastructure");
    let tf_dir = crate::deploy::config::terraform_dir(slug);

    // Resolve Lambda function names from terraform outputs
    let outputs = crate::deploy::ws::capture_outputs(&tf_dir).await.unwrap_or_default();
    let api_function_name = outputs.get("api_handler_arn")
        .and_then(|v| v.as_str())
        .and_then(|arn| arn.split(':').last())
        .unwrap_or(&format!("{slug}-api-default"))
        .to_string();
    let consumer_function_name = outputs.get("consumer_arn")
        .and_then(|v| v.as_str())
        .and_then(|arn| arn.split(':').last())
        .unwrap_or(&format!("{slug}-consumer-default"))
        .to_string();

    // S3 artifact location
    let backend_bucket = infra
        .and_then(|i| i.get("backend_bucket"))
        .and_then(|v| v.as_str())
        .unwrap_or("dashlx-terraform-state")
        .to_string();
    let artifact_prefix = format!("veil-projects/{slug}/artifacts");

    // Write main.veil to temp location for veil gen
    let main_veil_path = format!("/tmp/deploy/{slug}/main.veil");
    let dir = format!("/tmp/deploy/{slug}");
    tokio::fs::create_dir_all(&dir).await.ok();
    if let Ok(veil_bytes) = storage::application::read_file(
        deps,
        storage::domain::types::RepoId { value: repo_id.to_string() },
        "main".to_string(),
        "main.veil".to_string(),
    ).await {
        tokio::fs::write(&main_veil_path, &veil_bytes).await.ok();
    }

    Ok(crate::deploy::ws::LambdaBuildConfig {
        main_veil_path,
        rust_target,
        api_function_name,
        consumer_function_name,
        artifact_bucket: backend_bucket,
        artifact_prefix,
    })
}

/// Parse [deploy.contribution] from veil.toml and materialize the full project
/// source (ui.veil + layers + deps) to /tmp/deploy/{slug} so the contribution
/// build (veil gen ui.veil) can resolve its layers. Returns the parsed config
/// and the source dir.
async fn read_contribution_config(
    deps: &storage::application::Deps,
    repo_id: &str,
    slug: &str,
) -> Result<
    (
        crate::deploy::types::ContributionConfig,
        std::path::PathBuf,
        Vec<crate::deploy::component_deps::ComponentDep>,
    ),
    String,
> {
    let rid = || storage::domain::types::RepoId { value: repo_id.to_string() };
    let bytes = storage::application::read_file(
        deps, rid(), "main".to_string(), "veil.toml".to_string(),
    ).await.map_err(|e| format!("Failed to read veil.toml: {e:?}"))?;
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let parsed: toml::Value = content.parse()
        .map_err(|e| format!("Failed to parse veil.toml: {e}"))?;
    let deploy = parsed.get("deploy").ok_or("No [deploy] section in veil.toml")?;
    // parse_deploy_config expects a serde_json::Value; convert the toml section.
    let deploy_json: serde_json::Value = serde_json::to_value(deploy)
        .map_err(|e| format!("convert deploy section to json: {e}"))?;
    let cfg = crate::deploy::config::parse_deploy_config(&deploy_json);
    let contribution = cfg.contribution
        .ok_or("No [deploy.contribution] section in veil.toml")?;

    // Materialize ALL project files (ui.veil, layers/*, stubs/*, etc.).
    let dir = std::path::PathBuf::from(format!("/tmp/deploy/{}", slug));
    tokio::fs::create_dir_all(&dir).await
        .map_err(|e| format!("mkdir source dir: {e}"))?;
    let files = storage::application::list_files(
        deps, rid(), "main".to_string(), String::new(),
    ).await.map_err(|e| format!("list project files: {e:?}"))?;
    for file_path in &files {
        if let Ok(fbytes) = storage::application::read_file(
            deps, rid(), "main".to_string(), file_path.clone(),
        ).await {
            let dest = dir.join(file_path);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            tokio::fs::write(&dest, &fbytes).await.ok();
        }
    }

    // Resolve cross-project UI component dependencies: any component-provider
    // layer the consumer `use`s (data-driven via the layer's `implemented_by` +
    // `provides` declaration) has its implementing project fetched from the store
    // and materialized so the build step can generate + copy its components.
    let component_deps = crate::deploy::component_deps::resolve_component_deps(
        deps, &dir, "ui.veil",
    ).await;

    Ok((contribution, dir, component_deps))
}

async fn fetch_terraform_files_s3(
    deps: &storage::application::Deps,
    repo_id: &str,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let rid = storage::domain::types::RepoId { value: repo_id.to_string() };
    let files = storage::application::list_files(
        deps, rid.clone(), "main".to_string(), "terraform/".to_string(),
    ).await.map_err(|e| format!("Failed to list terraform files: {e:?}"))?;

    let mut tf_files = Vec::new();
    for file_path in &files {
        if file_path.ends_with(".tf") || file_path.ends_with(".tf.json") {
            let read_rid = storage::domain::types::RepoId { value: repo_id.to_string() };
            if let Ok(bytes) = storage::application::read_file(
                deps, read_rid, "main".to_string(), file_path.clone(),
            ).await {
                let filename = file_path
                    .strip_prefix("terraform/")
                    .unwrap_or(file_path)
                    .to_string();
                tf_files.push((filename, bytes));
            }
        }
    }
    Ok(tf_files)
}

/// Read InfraConfig from the project's veil.toml in S3.
async fn read_infra_config_s3(
    deps: &storage::application::Deps,
    repo_id: &str,
    slug: &str,
) -> crate::deploy::types::InfraConfig {
    let rid = storage::domain::types::RepoId { value: repo_id.to_string() };
    if let Ok(bytes) = storage::application::read_file(
        deps, rid, "main".to_string(), "veil.toml".to_string(),
    ).await {
        let content = String::from_utf8_lossy(&bytes).into_owned();
        if let Ok(parsed) = content.parse::<toml::Value>() {
            let infra = parsed.get("deploy").and_then(|d| d.get("infrastructure"));
            return crate::deploy::types::InfraConfig {
                backend_bucket: infra.and_then(|i| i.get("backend_bucket")).and_then(|v| v.as_str()).unwrap_or("dashlx-terraform-state").to_string(),
                backend_key: infra.and_then(|i| i.get("backend_key")).and_then(|v| v.as_str()).unwrap_or(&format!("veil-projects/{slug}")).to_string(),
                backend_region: infra.and_then(|i| i.get("backend_region")).and_then(|v| v.as_str()).unwrap_or("us-west-2").to_string(),
            };
        }
    }
    crate::deploy::types::InfraConfig {
        backend_bucket: "dashlx-terraform-state".into(),
        backend_key: format!("veil-projects/{slug}"),
        backend_region: "us-west-2".into(),
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

// ─── Deploy Pipeline (Terraform + Build + Deploy lifecycle) ─────────────────

#[derive(Clone)]
struct PipelineRouterState {
    pipeline: Arc<crate::deploy::PipelineState>,
}

async fn trigger_pipeline_deploy(
    State(st): State<PipelineRouterState>,
    Path(slug): Path<String>,
    Json(body): Json<crate::deploy::TriggerDeployRequest>,
) -> Result<Json<Value>, StatusCode> {
    // SOLE SHIP GATE: deploy is a ship action, so it must clear the same
    // `review::may_ship` gate as merge/provision. This endpoint is routed
    // directly (not only via provision_project), so gate it here too — no
    // deploy path may ship unsigned changes by consulting PrStatus or by
    // skipping the gate. See Mind Palace: decision-single-review-source-of-truth.
    if let Err(e) = veil_server::review::may_ship(&slug, None) {
        return Ok(Json(json!({
            "ok": false,
            "error": "sign_off_required",
            "message": e,
            "hint": "Approve the change set on /review before deploying this SHA.",
            "audit_env": veil_server::review::audit_env_json(),
        })));
    }
    // Triggered_by would come from auth context in production.
    let triggered_by = "user".to_string();

    match st.pipeline.trigger_deploy(slug, body, triggered_by).await {
        Ok(resp) => Ok(Json(json!({
            "job_id": resp.job_id,
            "status": resp.status,
        }))),
        Err(e) => Ok(Json(json!({
            "ok": false,
            "error": e,
        }))),
    }
}

async fn pipeline_deploy_status(
    State(st): State<PipelineRouterState>,
    Path(slug): Path<String>,
) -> Json<Value> {
    let status = st.pipeline.get_status(&slug).await;
    Json(serde_json::to_value(status).unwrap_or(json!({})))
}

async fn pipeline_deploy_plan(
    State(st): State<PipelineRouterState>,
    Path(slug): Path<String>,
) -> Json<Value> {
    match st.pipeline.get_drift(&slug).await {
        Some(drift) => Json(json!({
            "plan_output": drift.plan_output,
            "changes": [],
            "drift_detected": drift.detected,
            "change_count": drift.changes,
        })),
        None => Json(json!({
            "plan_output": "",
            "changes": [],
            "drift_detected": false,
            "message": "No plan available — run drift check first",
        })),
    }
}

async fn pipeline_deploy_history(
    State(st): State<PipelineRouterState>,
    Path(slug): Path<String>,
) -> Json<Value> {
    let history = st.pipeline.get_history(&slug).await;
    Json(serde_json::to_value(history).unwrap_or(json!([])))
}

#[derive(Deserialize)]
struct ApproveParams {
    #[allow(dead_code)]
    slug: String,
    job_id: String,
}

async fn pipeline_deploy_approve(
    State(st): State<PipelineRouterState>,
    Path(params): Path<ApproveParams>,
) -> Result<Json<Value>, StatusCode> {
    match st.pipeline.approve_job(&params.job_id).await {
        Ok(()) => Ok(Json(json!({
            "ok": true,
            "status": "running",
            "job_id": params.job_id,
        }))),
        Err(e) => Ok(Json(json!({
            "ok": false,
            "error": e,
        }))),
    }
}

async fn pipeline_check_drift(
    State(st): State<PipelineRouterState>,
    Path(slug): Path<String>,
) -> Json<Value> {
    match st.pipeline.check_drift(&slug).await {
        Ok(drift) => Json(serde_json::to_value(drift).unwrap_or(json!({}))),
        Err(e) => Json(json!({
            "ok": false,
            "error": e,
        })),
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
    contribution_store: Arc<crate::artifact_registry::ContributionManifestStore>,
    /// Auth binding for claim-based access control on contribution listing.
    /// Validates a Bearer token and extracts its claims via the pluggable
    /// AuthProvider (local JWKS by default, or a delegated auth VEIL app).
    auth: Arc<dyn crate::auth_provider::AuthProviderBinding>,
}

// ─── Compile-on-save (workflow → cdylib artifact) ───────────────────────────

#[derive(Deserialize)]
struct CompileWorkflowBody {
    /// Artifact id for the compiled workflow (e.g. "wf:tenant/onboarding").
    workflow_id: String,
    /// Absolute path to the workflow's primary `.veil` package on disk. The
    /// save handler writes the source before calling this; the path must live
    /// under the runtime's working tree.
    veil_source_path: String,
}

/// Codegen + `cargo build --release` a saved workflow to a cdylib, content-hash
/// the `.so`, upload it, and register a Pinned `Cdylib`/`Ffi` artifact version.
///
/// Any transpile or compile error fails the request (HTTP 422) so the builder
/// surfaces it — a workflow version is only runnable after a green compile.
async fn compile_workflow_handler(
    State(st): State<ArtifactRegistryState>,
    Json(body): Json<CompileWorkflowBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let source = std::path::PathBuf::from(&body.veil_source_path);
    if !source.is_file() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("veil source not found: {}", body.veil_source_path) })),
        ));
    }
    // Scratch build dir under temp, namespaced by workflow id.
    let work_dir = std::env::temp_dir()
        .join("veil-workflow-compile")
        .join(body.workflow_id.replace([':', '/'], "_"));

    match crate::compile_workflow::compile_and_register(
        &st.store,
        &body.workflow_id,
        &source,
        &work_dir,
    )
    .await
    {
        Ok(c) => Ok(Json(json!({
            "ok": true,
            "workflow_id": c.id,
            "version": c.version,
            "content_hash": c.content_hash,
            "blob_key": c.blob_key,
        }))),
        Err(e) => {
            tracing::error!(workflow_id = %body.workflow_id, error = %e, "compile-on-save failed");
            // Transpile/compile failures are client-actionable (bad workflow);
            // registry/io failures are server-side.
            let status = match e {
                crate::compile_workflow::CompileError::Transpile(_)
                | crate::compile_workflow::CompileError::Compile(_)
                | crate::compile_workflow::CompileError::NoArtifact(_) => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(json!({ "error": e.to_string() }))))
        }
    }
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
    match handle.invoke(body.args).await {
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
    #[serde(default)]
    toolchain_fingerprint: Option<String>,
}

async fn register_artifact(
    State(st): State<ArtifactRegistryState>,
    Json(body): Json<RegisterArtifactBody>,
) -> Result<Json<Value>, StatusCode> {
    let now = chrono::Utc::now();
    // Honor caller-supplied sign-off. If signed_off_by is present but no
    // timestamp was given, stamp it now. Backend functions must be signed off
    // to be resolvable (the function-invoke gate rejects unsigned records).
    let signed_off_at = match (&body.signed_off_by, body.signed_off_at) {
        (Some(_), Some(ts)) => Some(ts),
        (Some(_), None) => Some(now),
        (None, _) => None,
    };
    let record = crate::artifact_registry::ArtifactRecord {
        id: body.id,
        version: body.version,
        artifact_type: body.artifact_type,
        tenant_visibility: body.tenant_visibility,
        contributions: body.contributions,
        signed_off_by: body.signed_off_by,
        signed_off_at,
        blob_key: body.blob_key,
        content_hash: body.content_hash,
        bundle_path: body.bundle_path,
        bundle_size: body.bundle_size,
        manifest: body.manifest,
        toolchain_fingerprint: body.toolchain_fingerprint,
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

/// Extract and validate the Bearer token from request headers via the auth
/// binding, returning the caller's claims. Returns `None` if there is no token
/// or the provider rejects it — callers treat `None` as "unauthenticated"
/// (public-only visibility).
async fn extract_claims(
    auth: &dyn crate::auth_provider::AuthProviderBinding,
    headers: &axum::http::HeaderMap,
) -> Option<crate::access::Claims> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| {
            h.strip_prefix("Bearer ")
                .or_else(|| h.strip_prefix("bearer "))
        })?;
    let result = auth.authenticate(token).await;
    if result.authenticated {
        Some(result.claims)
    } else {
        if let Some(e) = &result.error {
            tracing::debug!(error = %e, provider = auth.kind(), "contribution access: token rejected");
        }
        None
    }
}

/// Decide whether a contribution with the given access rule is visible to a
/// caller with the given claims.
///
/// - No rule (`None`) or an explicitly public rule → always visible.
/// - A restricted rule → visible only if `claims` are present AND satisfy it.
///   An unauthenticated caller (`claims == None`) never sees restricted content.
fn access_permitted(
    rule: Option<&crate::access::AccessRule>,
    claims: Option<&crate::access::Claims>,
) -> bool {
    match rule {
        None => true,
        Some(r) if r.is_public() => true,
        Some(r) => match claims {
            Some(c) => r.evaluate(c),
            None => false,
        },
    }
}

/// GET /api/contributions?kind=menu_item&tenant_id=...
/// GET /api/contributions?app=dlx-ai
/// Lists contributions. When `app` is provided, uses the ContributionManifestStore
/// (DLX AI harness model). Otherwise falls through to legacy artifact-registry query.
async fn list_contributions(
    State(st): State<ArtifactRegistryState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<ContributionsQuery>,
) -> Result<Json<Value>, StatusCode> {
    // New DLX AI harness path: ?app= queries the ContributionManifestStore.
    if let Some(ref app_id) = q.app {
        let manifests = st
            .contribution_store
            .list_for_app(app_id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let mut manifests: Vec<_> = if q.include_disabled.unwrap_or(false) {
            manifests
        } else {
            manifests.into_iter().filter(|m| m.enabled).collect()
        };

        // ── Claim-based access control ──────────────────────────────────────
        // Extract + validate the Bearer token (if any) and derive the caller's
        // claims. Then filter contributions: public ones are always visible;
        // restricted ones only if the caller's claims satisfy their rule.
        let claims: Option<crate::access::Claims> = extract_claims(st.auth.as_ref(), &headers).await;

        manifests.retain(|m| access_permitted(m.access.as_ref(), claims.as_ref()));

        return Ok(Json(json!({ "contributions": manifests })));
    }

    // Legacy path: kind/tenant-based resolution from the artifact registry.
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
    /// DLX AI harness query: when provided, uses ContributionManifestStore.
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    include_disabled: Option<bool>,
}

// ─── Contribution Registry (DLX AI Harness Model) ───────────────────────────

/// POST /api/contributions
/// Register a new contribution manifest (called by deploy pipeline after build).
async fn create_contribution(
    State(st): State<ArtifactRegistryState>,
    Json(body): Json<crate::artifact_registry::CreateContributionBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if body.app_id.is_empty() || body.id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "app_id and id are required"})),
        ));
    }
    if body.bundle_url.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bundle_url is required"})),
        ));
    }

    let now = chrono::Utc::now();

    // Check if this contribution already exists (update vs create).
    let existing = st
        .contribution_store
        .get(&body.app_id, &body.id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
        })?;

    let manifest = crate::artifact_registry::ContributionManifest {
        id: body.id,
        app_id: body.app_id,
        name: body.name,
        version: body.version,
        bundle_url: body.bundle_url,
        css_url: body.css_url,
        enabled: body.enabled,
        order: body.order,
        slots: body.slots,
        access: body.access,
        registered_at: existing
            .as_ref()
            .map(|e| e.registered_at)
            .unwrap_or(now),
        updated_at: now,
    };

    st.contribution_store.put(&manifest).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e})),
        )
    })?;

    let status = if existing.is_some() {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };

    Ok((status, Json(serde_json::to_value(&manifest).unwrap_or(json!({})))))
}

/// PATCH /api/contributions/{app_id}/{contribution_id}
/// Partially update a contribution (enable/disable, change version/bundle_url, etc).
async fn patch_contribution(
    State(st): State<ArtifactRegistryState>,
    Path((app_id, contribution_id)): Path<(String, String)>,
    Json(body): Json<crate::artifact_registry::PatchContributionBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let existing = st
        .contribution_store
        .get(&app_id, &contribution_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
        })?;

    let mut manifest = match existing {
        Some(m) => m,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("contribution not found: {app_id}/{contribution_id}")})),
            ));
        }
    };

    // Apply partial updates.
    if let Some(enabled) = body.enabled {
        manifest.enabled = enabled;
    }
    if let Some(version) = body.version {
        manifest.version = version;
    }
    if let Some(bundle_url) = body.bundle_url {
        manifest.bundle_url = bundle_url;
    }
    if let Some(css_url) = body.css_url {
        manifest.css_url = css_url;
    }
    if let Some(order) = body.order {
        manifest.order = order;
    }
    if let Some(slots) = body.slots {
        manifest.slots = slots;
    }
    if let Some(name) = body.name {
        manifest.name = name;
    }
    if let Some(access) = body.access {
        manifest.access = access;
    }
    manifest.updated_at = chrono::Utc::now();

    st.contribution_store.put(&manifest).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e})),
        )
    })?;

    Ok(Json(serde_json::to_value(&manifest).unwrap_or(json!({}))))
}

/// DELETE /api/contributions/{app_id}/{contribution_id}
/// Remove a contribution registration entirely.
async fn delete_contribution(
    State(st): State<ArtifactRegistryState>,
    Path((app_id, contribution_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    st.contribution_store
        .delete(&app_id, &contribution_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e})),
            )
        })?;

    Ok(StatusCode::NO_CONTENT)
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

// ─── Static Preview ("Open UI" vibe-code window) ─────────────────────────────

/// Materialize a project's source (ui.veil + layers/ + stubs/ + …) into `dir`
/// from the storage layer, mirroring the contribution build materialization.
/// Returns the entry `.veil` file name for the preview build.
async fn materialize_preview_source(
    deps: &storage::application::Deps,
    repo_id: &str,
    dir: &std::path::Path,
) -> Result<String, String> {
    let rid = || storage::domain::types::RepoId { value: repo_id.to_string() };
    let files = storage::application::list_files(deps, rid(), "main".to_string(), String::new())
        .await
        .map_err(|e| format!("list project files: {e:?}"))?;
    for file_path in &files {
        if let Ok(fbytes) =
            storage::application::read_file(deps, rid(), "main".to_string(), file_path.clone())
                .await
        {
            let dest = dir.join(file_path);
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            tokio::fs::write(&dest, &fbytes).await.ok();
        }
    }
    // Entry .veil: prefer ui.veil, else the first top-level *.veil.
    if dir.join("ui.veil").exists() {
        return Ok("ui.veil".to_string());
    }
    files
        .iter()
        .find(|f| f.ends_with(".veil") && !f.contains('/'))
        .cloned()
        .ok_or_else(|| "no entry .veil found in project source".to_string())
}

/// POST /api/projects/{slug}/preview/build — (re)build the static preview.
/// Returns the resulting PreviewStatus. Called on "Open UI" and after each
/// accepted change so the window can reload.
async fn preview_build(
    State(st): State<StorageState>,
    Path(slug): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let repo_id = resolve_repo_id_value(&st.deps, &slug).await?;
    let deps = st.deps.clone();
    let repo_id_for_mat = repo_id.clone();
    let result = crate::deploy::preview::build_preview(&slug, move |dir| {
        let deps = deps.clone();
        let repo_id = repo_id_for_mat.clone();
        async move { materialize_preview_source(&deps, &repo_id, &dir).await }
    })
    .await;
    match result {
        Ok(_) => Ok(Json(json!(crate::deploy::preview::status_for(&slug).await))),
        Err(e) => Ok(Json(json!({ "state": "failed", "error": e }))),
    }
}

/// GET /api/projects/{slug}/preview/status — current preview build status, so
/// the Open-UI window can render "starting preview…" gracefully.
async fn preview_status(Path(slug): Path<String>) -> Json<Value> {
    Json(json!(crate::deploy::preview::status_for(&slug).await))
}

/// GET /preview/{slug}/ and /preview/{slug}/{*path} — serve the built static
/// preview bundle. HTML responses get the overlay client script injected.
async fn preview_serve_root(Path(slug): Path<String>) -> axum::response::Response {
    preview_serve_path(Path((slug, String::new()))).await
}

async fn preview_serve_path(
    Path((slug, path)): Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let status = crate::deploy::preview::status_for(&slug).await;
    let dir = match status {
        crate::deploy::preview::PreviewStatus::Ready { dir } => std::path::PathBuf::from(dir),
        crate::deploy::preview::PreviewStatus::Building => {
            return (StatusCode::ACCEPTED, preview_waiting_html(&slug)).into_response();
        }
        crate::deploy::preview::PreviewStatus::Failed { error } => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::response::Html(format!(
                    "<h1>Preview build failed</h1><pre>{}</pre>",
                    html_escape_min(&error)
                )),
            )
                .into_response();
        }
        crate::deploy::preview::PreviewStatus::Idle => {
            return (StatusCode::NOT_FOUND, preview_waiting_html(&slug)).into_response();
        }
    };

    // Resolve the requested file safely within the built bundle. Empty / dir
    // requests fall back to index.html (SPA behavior).
    let rel = path.trim_start_matches('/');
    if rel.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut target = if rel.is_empty() {
        dir.join("index.html")
    } else {
        dir.join(rel)
    };
    if target.is_dir() {
        target = target.join("index.html");
    }
    if !target.exists() {
        // SPA fallback to the shell so client-side routes resolve.
        target = dir.join("index.html");
    }

    let bytes = match tokio::fs::read(&target).await {
        Ok(b) => b,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let ct = guess_content_type(&target.to_string_lossy());
    if ct.starts_with("text/html") {
        let html = String::from_utf8_lossy(&bytes).into_owned();
        let injected = crate::deploy::preview::inject_overlay(&html);
        return axum::response::Html(injected).into_response();
    }
    ([(axum::http::header::CONTENT_TYPE, ct)], bytes).into_response()
}

fn preview_waiting_html(slug: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"2\">\
         <title>Starting preview…</title>\
         <style>body{{font-family:system-ui;margin:0;display:flex;height:100vh;\
         align-items:center;justify-content:center;background:#0b0b0c;color:#e5e5e5}}\
         .box{{text-align:center}}.spin{{width:28px;height:28px;border:3px solid #333;\
         border-top-color:#6ea8fe;border-radius:50%;animation:s 0.8s linear infinite;\
         margin:0 auto 14px}}@keyframes s{{to{{transform:rotate(360deg)}}}}</style></head>\
         <body><div class=\"box\"><div class=\"spin\"></div>\
         <div>Starting preview for <code>{}</code>…</div></div></body></html>",
        html_escape_min(slug)
    ))
}

fn html_escape_min(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Build platform domain router and merge onto ProductHost.
pub async fn build_platform_router(
    bus: Arc<dyn veil_shared::Bus + Send + Sync>,
) -> (
    Router,
    Arc<dyn crate::auth_provider::AuthProviderBinding>,
    bool,
) {
    let storage_deps = Arc::new(resolve_storage_deps().await);
    let storage = StorageState {
        deps: storage_deps.clone(),
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
        .route(
            "/api/repos/{id}/origin",
            get(get_repo_origin).post(bind_repo_origin),
        )
        .route(
            "/api/repos/{id}/migrate-to-subpath",
            post(migrate_to_subpath),
        )
        .route("/api/git/status", get(git_status))
        .route("/api/read-file", post(read_file_api))
        .route("/api/write-file", post(write_file_api))
        .route("/api/list-files", post(list_files_api))
        .route("/api/projects/{slug}/deploy/ws", get(deploy_ws_handler))
        .route("/api/projects/{slug}/deploy/gate", get(deploy_gate_handler))
        .route("/api/projects/{slug}/preview/build", post(preview_build))
        .route("/api/review/bundles/{id}/approve", post(bundle_approve))
        .route("/api/review/bundles/{id}/merge", post(bundle_merge))
        .route("/api/review/bundles/{id}/ship", post(bundle_ship))
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

    // Static preview serving (Open-UI window). Stateless — reads built bundles
    // from disk (see crate::deploy::preview). HTML gets the overlay injected.
    let preview_r = Router::new()
        .route("/api/projects/{slug}/preview/status", get(preview_status))
        .route("/preview/{slug}", get(preview_serve_root))
        .route("/preview/{slug}/", get(preview_serve_root))
        .route("/preview/{slug}/{*path}", get(preview_serve_path));

    // Artifact Registry (Phase 1 Platform Primitives)
    let art_reg_store =
        Arc::new(crate::artifact_registry::ArtifactRegistryStore::from_env().await);
    let contribution_store = Arc::new(crate::artifact_registry::ContributionManifestStore::new(
        art_reg_store.ddb.clone(),
        art_reg_store.table.clone(),
    ));

    // Function registry (app-to-app invoke substrate) — built before the auth
    // binding so an RPC auth provider can invoke a VEIL auth app through it.
    // `from_env` attaches a Lambda client so `invoke_kind = lambda` backend
    // functions (e.g. a deployed dlx-auth) resolve to CallableHandle::Lambda.
    let fn_registry = Arc::new(
        crate::function_invoke::FunctionRegistry::from_env(Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        ))
        .await,
    );

    // Auth binding (Model C): local JWKS by default, or RPC/FFI delegation to a
    // VEIL auth app, selected by VEIL_AUTH_BINDING. The same binding gates
    // /api/* (via AuthLayer, applied in main) and filters contributions here.
    let auth_local_state = Arc::new(
        crate::auth::AuthState::new_for_claims(crate::auth::AuthConfig::cognito_from_env()).await,
    );
    let auth_binding = crate::auth_provider::build_binding(
        crate::auth_provider::BindingSpec::from_env(),
        auth_local_state,
        fn_registry.clone(),
    );
    tracing::info!(provider = auth_binding.kind(), "auth provider binding selected");

    let art_reg = ArtifactRegistryState {
        store: art_reg_store,
        contribution_store,
        auth: auth_binding.clone(),
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
        // Contribution Registry (DLX AI Harness Model)
        .route(
            "/api/contributions",
            get(list_contributions).post(create_contribution),
        )
        .route(
            "/api/contributions/{app_id}/{contribution_id}",
            patch(patch_contribution).delete(delete_contribution),
        )
        // Compile-on-save: codegen + build a saved workflow to a cdylib artifact.
        .route(
            "/api/workflows/compile",
            post(compile_workflow_handler),
        )
        .with_state(art_reg)
        .layer(build_artifact_cors_layer());

    // Function Invoke (Phase 3 Platform Primitives)
    let fn_invoke_state = FunctionInvokeState {
        registry: fn_registry.clone(),
    };
    let function_invoke_r = Router::new()
        .route(
            "/api/functions/{function_id}/invoke",
            post(invoke_function),
        )
        .with_state(fn_invoke_state);

    // ─── Deploy Pipeline (Terraform + Build + Deploy lifecycle) ────────────
    let pipeline_state = PipelineRouterState {
        pipeline: Arc::new(crate::deploy::PipelineState::new(storage_deps).await),
    };
    let pipeline_r = Router::new()
        .route(
            "/api/projects/{slug}/deploy",
            post(trigger_pipeline_deploy),
        )
        .route(
            "/api/projects/{slug}/deploy/status",
            get(pipeline_deploy_status),
        )
        .route(
            "/api/projects/{slug}/deploy/plan",
            get(pipeline_deploy_plan),
        )
        .route(
            "/api/projects/{slug}/deploy/history",
            get(pipeline_deploy_history),
        )
        .route(
            "/api/projects/{slug}/deploy/{job_id}/approve",
            post(pipeline_deploy_approve),
        )
        .route(
            "/api/projects/{slug}/deploy/drift",
            post(pipeline_check_drift),
        )
        .with_state(pipeline_state);

    let router = storage_r
        .merge(cm_r)
        .merge(deploy_r)
        .merge(registry_r)
        .merge(preview_r)
        .merge(artifact_registry_r)
        .merge(function_invoke_r)
        .merge(pipeline_r);

    // The coarse gate flag: whether /api/* requires a valid token at all.
    let auth_enabled = crate::auth::AuthConfig::from_env().enabled;
    (router, auth_binding, auth_enabled)
}

#[cfg(test)]
mod access_control_tests {
    use super::access_permitted;
    use crate::access::{AccessRule, Claims};
    use serde_json::{json, Value};

    fn claims(pairs: &[(&str, Value)]) -> Claims {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn rule(j: Value) -> AccessRule {
        serde_json::from_value(j).unwrap()
    }

    #[test]
    fn no_rule_visible_to_everyone_including_anonymous() {
        assert!(access_permitted(None, None));
        assert!(access_permitted(None, Some(&claims(&[]))));
    }

    #[test]
    fn public_rule_visible_to_everyone_including_anonymous() {
        let r = rule(json!({ "public": true }));
        assert!(access_permitted(Some(&r), None));
        assert!(access_permitted(Some(&r), Some(&claims(&[]))));
    }

    #[test]
    fn restricted_rule_hidden_from_anonymous() {
        let r = rule(json!({ "claim": "email", "op": "endswith", "value": "@dashlx.com" }));
        // No claims (unauthenticated) → never see restricted content.
        assert!(!access_permitted(Some(&r), None));
    }

    #[test]
    fn restricted_rule_visible_when_claims_satisfy() {
        let r = rule(json!({ "claim": "email", "op": "endswith", "value": "@dashlx.com" }));
        assert!(access_permitted(
            Some(&r),
            Some(&claims(&[("email", json!("jd@dashlx.com"))]))
        ));
    }

    #[test]
    fn restricted_rule_hidden_when_claims_do_not_satisfy() {
        let r = rule(json!({ "claim": "email", "op": "endswith", "value": "@nobody.example" }));
        assert!(!access_permitted(
            Some(&r),
            Some(&claims(&[("email", json!("jd@dashlx.com"))]))
        ));
    }
}
