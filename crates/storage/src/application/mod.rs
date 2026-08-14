//! Application services and flow functions.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::messages::*;
use crate::domain::types::*;
use crate::ports::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Injected dependencies (ports).
pub struct Deps {
    pub metadata_store: std::sync::Arc<dyn MetadataStore + Send + Sync>,
    pub object_storage: std::sync::Arc<dyn ObjectStorage + Send + Sync>,
}

/// DomainService: CreateRepo
#[tracing::instrument(skip_all)]
pub async fn create_repo(
    deps: &Deps,
    name: String,
    description: Option<String>,
) -> Result<Repo, DomainError> {
    // step: execute
    let id = RepoId {
        value: Uuid::new_v4().to_string(),
    };
    let now = Utc::now();
    let slug = name.to_lowercase().replace(" ", "-").replace("_", "-");
    let repo = Repo {
        id: id.clone(),
        name: name.clone(),
        slug: slug.clone(),
        description: description.clone(),
        default_branch: "main".to_string(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    deps.metadata_store.create_repo(repo.clone()).await?;
    let branch = BranchInfo {
        name: "main".to_string(),
        head_commit: "".to_string(),
        updated_at: now.clone(),
    };
    deps.metadata_store
        .put_branch(id.clone(), branch.clone())
        .await?;
    return Ok(repo);
}

/// DomainService: ListRepos
#[tracing::instrument(skip_all)]
pub async fn list_repos(deps: &Deps) -> Result<Vec<Repo>, DomainError> {
    // step: query
    let repos = deps.metadata_store.list_repos().await?;
    return Ok(repos);
}

/// DomainService: GetRepo
#[tracing::instrument(skip_all)]
pub async fn get_repo(deps: &Deps, id: String) -> Result<Repo, DomainError> {
    // step: query
    let rid = RepoId { value: id.clone() };
    let repo = deps.metadata_store.get_repo(rid.clone()).await?;
    return Ok(repo);
}

/// Resolve UUID, slug, or display name → Repo.
pub async fn resolve_repo(deps: &Deps, id_or_slug: &str) -> Result<Repo, DomainError> {
    let needle = id_or_slug.trim();
    if needle.is_empty() {
        return Err(DomainError::Validation("project id/slug required".into()));
    }
    if let Ok(repo) = get_repo(deps, needle.to_string()).await {
        return Ok(repo);
    }
    let repos = list_repos(deps).await?;
    let lower = needle.to_lowercase();
    repos
        .into_iter()
        .find(|r| {
            r.id.value.eq_ignore_ascii_case(needle)
                || r.slug.eq_ignore_ascii_case(&lower)
                || r.name.eq_ignore_ascii_case(&lower)
        })
        .ok_or(DomainError::NotFound)
}

/// DomainService: UpdateRepo (display name / slug / description).
///
/// S3 keys stay `repos/{repo_id}/…` — slug is metadata + URL only.
#[tracing::instrument(skip_all)]
pub async fn update_repo(
    deps: &Deps,
    id: String,
    name: Option<String>,
    slug: Option<String>,
    description: Option<String>,
    clear_description: bool,
) -> Result<Repo, DomainError> {
    let mut repo = resolve_repo(deps, &id).await?;
    if let Some(n) = name {
        let n = n.trim().to_string();
        if n.is_empty() {
            return Err(DomainError::Validation("name must not be empty".into()));
        }
        repo.name = n;
    }
    if let Some(s) = slug {
        let s = s
            .trim()
            .to_lowercase()
            .replace(' ', "-")
            .replace('_', "-");
        if s.is_empty() {
            return Err(DomainError::Validation("slug must not be empty".into()));
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(DomainError::Validation(
                "slug must be lowercase alphanumeric plus hyphens".into(),
            ));
        }
        if s != repo.slug {
            let existing = list_repos(deps).await?;
            if existing
                .iter()
                .any(|r| r.slug.eq_ignore_ascii_case(&s) && r.id.value != repo.id.value)
            {
                return Err(DomainError::Validation(format!(
                    "slug `{s}` is already used by another project"
                )));
            }
            repo.slug = s;
        }
    }
    if clear_description {
        repo.description = None;
    } else if let Some(d) = description {
        repo.description = Some(d);
    }
    repo.updated_at = Utc::now();
    deps.metadata_store.update_repo(repo.clone()).await?;
    Ok(repo)
}

/// DomainService: DeleteRepo
#[tracing::instrument(skip_all)]
pub async fn delete_repo(deps: &Deps, id: String) -> Result<(), DomainError> {
    // Accept UUID or slug — DDB PK is REPO#{uuid} only.
    let repo = resolve_repo(deps, &id).await?;
    deps.metadata_store
        .delete_repo(repo.id.clone())
        .await?;

    Ok(())
}

/// DomainService: GetProjectInfra
#[tracing::instrument(skip_all)]
pub async fn get_project_infra(
    deps: &Deps,
    id: String,
    environment: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: load
    let rid = RepoId { value: id.clone() };
    let repo = deps.metadata_store.get_repo(rid.clone()).await?;
    let s3_key = format!("repos/{}/{}/veil.toml", repo.id.value, repo.default_branch);
    let mut source = "disk".to_string();
    if deps.object_storage.exists(s3_key.clone()).await? {
        source = "s3".to_string();
    };
    let snap_str = veil_local_fs::LocalFs::read_project_deploy(repo.slug.clone())
        .map_err(|e| DomainError::External(e.to_string()))?;
    let mut snap: serde_json::Value = serde_json::from_str::<_>(&snap_str)?;
    let mut env_catalog: serde_json::Value = serde_json::from_str::<_>(&"{\"default\":\"dev\",\"environments\":[{\"name\":\"dev\",\"region\":\"us-west-2\",\"account_id\":null,\"has_assume_role\":false,\"assume_role_arn\":null,\"lambda_execution_role_arn\":null,\"gateways\":[{\"logical\":\"dashlx-services\",\"patterns\":[\"dlx-rust-*\",\"*-dev-service-api\",\"dashlx-services\"]}]},{\"name\":\"staging\",\"region\":\"us-west-2\",\"account_id\":null,\"has_assume_role\":false,\"assume_role_arn\":null,\"lambda_execution_role_arn\":null,\"gateways\":[{\"logical\":\"dashlx-services\",\"patterns\":[\"dlx-rust-*\",\"*-staging-service-api\",\"dashlx-services\"]}]},{\"name\":\"prod\",\"region\":\"us-west-2\",\"account_id\":null,\"has_assume_role\":false,\"assume_role_arn\":null,\"lambda_execution_role_arn\":null,\"gateways\":[{\"logical\":\"dashlx-services\",\"patterns\":[\"dlx-rust-*\",\"*-prod-service-api\",\"dashlx-services\"]}]}],\"config_path\":\"runtime/config/deploy.toml\"}".to_string())?;
    let mut env_name = "dev".to_string();
    if environment.is_some() {
        env_name = environment.clone().ok_or(DomainError::NotFound)?;
    };
    return Ok(
        serde_json::json!({ "repo": repo.clone(), "infra": snap.clone(), "environment": env_name.clone(), "environments": env_catalog.clone(), "source": source.clone(), "s3_key": s3_key.clone() }),
    );
}

/// DomainService: QueryProjectModules
#[tracing::instrument(skip_all)]
pub async fn query_project_modules(
    deps: &Deps,
    module: String,
    filters_json: String,
) -> Result<serde_json::Value, DomainError> {
    // step: scan
    let repos = deps.metadata_store.list_repos().await?;
    let mut toml_entries: Vec<serde_json::Value> = vec![];
    for repo in repos {
        let s3_key = format!("repos/{}/{}/veil.toml", repo.id.value, repo.default_branch);
        if deps.object_storage.exists(s3_key.clone()).await? {
            let raw_bytes = deps.object_storage.get(s3_key.clone()).await?;
            toml_entries.push(serde_json::json!({ "repo_id": serde_json::json!(serde_json::json!(repo.clone())["id"].clone())["value"].clone(), "repo_name": serde_json::json!(repo.clone())["name"].clone(), "slug": serde_json::json!(repo.clone())["slug"].clone(), "branch": serde_json::json!(repo.clone())["default_branch"].clone(), "raw": raw_bytes.clone() }));
        };
    }
    let result_str = veil_local_fs::LocalFs::query_modules_from_tomls(
        serde_json::to_string(&toml_entries)?,
        module.clone(),
        filters_json.clone(),
    )
    .map_err(|e| DomainError::External(e.to_string()))?;
    return Ok(serde_json::from_str::<_>(&result_str)?);
}

/// DomainService: SyncRepoToObjectStore
#[tracing::instrument(skip_all)]
pub async fn sync_repo_to_object_store(
    deps: &Deps,
    id: String,
    branch: String,
) -> Result<serde_json::Value, DomainError> {
    // step: sync
    let rid = RepoId { value: id.clone() };
    let repo = deps.metadata_store.get_repo(rid.clone()).await?;
    let hub = veil_local_fs::LocalFs::projects_dir();
    let root = veil_local_fs::LocalFs::join(hub.clone(), repo.slug.clone());
    let mut uploaded = 0;
    let mut skipped = 0;
    if veil_local_fs::LocalFs::path_exists(root.clone()) {
        let names = veil_local_fs::LocalFs::list_dir(root.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        for name in names {
            if name == ".git".to_string()
                || name == "target".to_string()
                || name == "generated".to_string()
                || name == "node_modules".to_string()
                || name == ".veil".to_string()
                || name == "dist".to_string()
            {
                skipped = skipped + 1;
            } else {
                let path = veil_local_fs::LocalFs::join(root.clone(), name.clone());
                if veil_local_fs::LocalFs::path_is_file(path.clone()) {
                    let body = veil_local_fs::LocalFs::read(path.clone())
                        .map_err(|e| DomainError::External(e.to_string()))?;
                    let key = format!("repos/{}/{}/{}", repo.id.value, branch, name);
                    deps.object_storage
                        .put(key.clone(), body.into_bytes())
                        .await?;
                    uploaded = uploaded + 1;
                } else {
                    skipped = skipped + 1;
                };
            };
        }
    };
    return Ok(
        serde_json::json!({ "repo_id": serde_json::json!(serde_json::json!(repo.clone())["id"].clone())["value"].clone(), "branch": branch.clone(), "uploaded": uploaded.clone(), "skipped": skipped.clone(), "local_root": root.clone() }),
    );
}

/// DomainService: WriteFile
#[tracing::instrument(skip_all)]
pub async fn write_file(
    deps: &Deps,
    repo_id: RepoId,
    branch: String,
    path: String,
    content: String,
    message: String,
) -> Result<CommitInfo, DomainError> {
    // step: execute
    let key = format!("repos/{}/{}/{}", repo_id.value, branch, path);
    let hash = (content.len() as i64).to_string();
    deps.object_storage
        .put(key.clone(), content.into_bytes())
        .await?;
    let now = Utc::now();
    let commit = CommitInfo {
        hash: hash.clone(),
        message: message.clone(),
        author: "system".to_string(),
        timestamp: now.clone(),
        parent_hashes: vec![],
        files_changed: vec![path],
    };
    // Git origin owns history. Do not write DDB COMMIT# facsimiles.
    let origin_off = matches!(
        std::env::var("VEIL_GIT_ORIGIN")
            .unwrap_or_else(|_| "auto".into())
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    );
    if origin_off {
        deps.metadata_store
            .put_commit(repo_id.clone(), commit.clone())
            .await?;
    }
    return Ok(commit);
}

/// DomainService: ReadFile
#[tracing::instrument(skip_all)]
pub async fn read_file(
    deps: &Deps,
    repo_id: RepoId,
    branch: String,
    path: String,
) -> Result<Vec<u8>, DomainError> {
    // step: query
    let key = format!("repos/{}/{}/{}", repo_id.value, branch, path);
    let data = deps.object_storage.get(key.clone()).await?;
    return Ok(data);
}

/// DomainService: ListFiles
#[tracing::instrument(skip_all)]
pub async fn list_files(
    deps: &Deps,
    repo_id: RepoId,
    branch: String,
    prefix: String,
) -> Result<Vec<String>, DomainError> {
    // step: query
    let key_prefix = format!("repos/{}/{}/{}", repo_id.value, branch, prefix);
    let files = deps.object_storage.list(key_prefix.clone()).await?;
    return Ok(files);
}

/// DomainService: CreateBranch
#[tracing::instrument(skip_all)]
pub async fn create_branch(
    deps: &Deps,
    repo_id: RepoId,
    name: String,
    from_ref: String,
) -> Result<BranchInfo, DomainError> {
    // step: execute
    let source = deps
        .metadata_store
        .get_branch(repo_id.clone(), from_ref.clone())
        .await?;
    let now = Utc::now();
    let branch = BranchInfo {
        name: name.clone(),
        head_commit: source.head_commit.clone(),
        updated_at: now.clone(),
    };
    deps.metadata_store
        .put_branch(repo_id.clone(), branch.clone())
        .await?;
    return Ok(branch);
}

/// DomainService: ListBranches
#[tracing::instrument(skip_all)]
pub async fn list_branches(deps: &Deps, repo_id: RepoId) -> Result<Vec<BranchInfo>, DomainError> {
    // step: query
    let branches = deps.metadata_store.list_branches(repo_id.clone()).await?;
    return Ok(branches);
}

/// DomainService: GetDiff
#[tracing::instrument(skip_all)]
pub async fn get_diff(
    deps: &Deps,
    repo_id: RepoId,
    from_ref: String,
    to_ref: String,
) -> Result<Vec<DiffEntry>, DomainError> {
    // step: query
    let from_files = deps
        .object_storage
        .list(format!("repos/{}/{}/", repo_id.value, from_ref))
        .await?;
    let to_files = deps
        .object_storage
        .list(format!("repos/{}/{}/", repo_id.value, to_ref))
        .await?;
    return Ok(vec![]);
}

/// DomainService: Compile
#[tracing::instrument(skip_all)]
pub async fn compile(
    deps: &Deps,
    repo_id: RepoId,
    branch: String,
    target: CompilationTarget,
) -> Result<ArtifactMetadata, DomainError> {
    // step: execute
    let branch_info = deps
        .metadata_store
        .get_branch(repo_id.clone(), branch.clone())
        .await?;
    let hash = branch_info.head_commit;
    let id = ArtifactId {
        value: hash.clone(),
    };
    let now = Utc::now();
    let s3_key = format!("artifacts/{}/binary", hash);
    let artifact = ArtifactMetadata {
        id: id.clone(),
        repo_id: repo_id.clone(),
        branch: branch.clone(),
        commit_hash: hash.clone(),
        content_hash: hash.clone(),
        target: target.clone(),
        s3_key: s3_key.clone(),
        size_bytes: 0,
        compiled_at: now.clone(),
    };
    deps.metadata_store.put_artifact(artifact.clone()).await?;
    return Ok(artifact);
}

/// DomainService: Deploy
#[tracing::instrument(skip_all)]
pub async fn deploy(
    deps: &Deps,
    artifact_id: ArtifactId,
    target: DeployTarget,
) -> Result<DeploymentRecord, DomainError> {
    // step: execute
    let now = Utc::now();
    let record = DeploymentRecord {
        artifact_id: artifact_id.clone(),
        target: target.clone(),
        deployed_at: now.clone(),
        status: DeploymentStatus::InProgress.clone(),
    };
    deps.metadata_store.put_deployment(record.clone()).await?;
    return Ok(record);
}

/// DomainService: GetCommitLog
#[tracing::instrument(skip_all)]
pub async fn get_commit_log(
    deps: &Deps,
    repo_id: RepoId,
    branch: Option<String>,
    limit: i64,
    offset: i64,
) -> Result<Vec<CommitInfo>, DomainError> {
    // step: query
    let commits = deps
        .metadata_store
        .list_commits(
            repo_id.clone(),
            branch.clone(),
            limit.clone(),
            offset.clone(),
        )
        .await?;
    return Ok(commits);
}
