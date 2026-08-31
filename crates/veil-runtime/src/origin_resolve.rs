//! Bridge the DDB `Repo.origin` binding to a `veil_server::git_origin::GitOrigin`.
//!
//! This is the single translation point between the storage-layer binding
//! (`storage::domain::types::OriginBinding`) and the transport backend
//! (`veil_server::git_origin::OriginBackend`). Keeping it in `veil-runtime`
//! avoids a `storage → veil-server` dependency cycle: `git_origin.rs` stays
//! self-contained, and only the runtime (which depends on both) knows how to
//! join them.

#![allow(dead_code)]

use storage::domain::types::{GitProvider as BindProvider, OriginBinding, Repo};
use veil_server::git_origin::{GitOrigin, GitProvider, RemoteConfig};

fn map_provider(p: BindProvider) -> GitProvider {
    match p {
        BindProvider::Github => GitProvider::GitHub,
        BindProvider::Bitbucket => GitProvider::Bitbucket,
    }
}

/// Build a `GitOrigin` for a repo, honouring its origin binding.
///
/// Absent binding or `OriginBinding::S3` → the S3 bundle backend (default).
/// `OriginBinding::Git { .. }` → a real provider remote.
pub fn git_origin_for(repo: &Repo) -> GitOrigin {
    match &repo.origin {
        Some(OriginBinding::Git {
            provider,
            repo: remote_repo,
            subpath,
            branch,
        }) => {
            let cfg = RemoteConfig {
                provider: map_provider(*provider),
                repo: remote_repo.clone(),
                subpath: subpath.clone(),
                branch: branch
                    .clone()
                    .unwrap_or_else(|| repo.default_branch.clone()),
            };
            veil_server::git_origin::register_origin(&repo.id.value, Some(cfg.clone()));
            GitOrigin::with_remote(repo.id.value.clone(), cfg)
        }
        _ => {
            veil_server::git_origin::register_origin(&repo.id.value, None);
            GitOrigin::new(repo.id.value.clone())
        }
    }
}

/// True if the repo is bound to a real git provider remote.
pub fn is_git_backed(repo: &Repo) -> bool {
    matches!(repo.origin, Some(OriginBinding::Git { .. }))
}

/// Build a provider REST client target (`org/name` + provider) for a git-backed
/// repo, or `None` for S3-backed repos.
pub fn provider_repo_for(repo: &Repo) -> Option<veil_server::git_provider::ProviderRepo> {
    match &repo.origin {
        Some(OriginBinding::Git {
            provider,
            repo: remote_repo,
            ..
        }) => Some(veil_server::git_provider::ProviderRepo::new(
            map_provider(*provider),
            remote_repo.clone(),
        )),
        _ => None,
    }
}

/// The subpath (normalised) for a git-backed project, if any.
pub fn origin_subpath(repo: &Repo) -> Option<String> {
    match &repo.origin {
        Some(OriginBinding::Git { subpath, .. }) => subpath
            .as_deref()
            .map(|s| s.trim().trim_matches('/').to_string())
            .filter(|s| !s.is_empty()),
        _ => None,
    }
}

/// Resolve a UUID / slug / display name to the full `Repo` record (so callers
/// can inspect the origin binding).
pub async fn resolve_repo_full(
    deps: &storage::application::Deps,
    id_or_slug: &str,
) -> Result<Repo, veil_shared::DomainError> {
    storage::application::resolve_repo(deps, id_or_slug).await
}
