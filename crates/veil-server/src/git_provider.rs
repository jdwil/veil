//! Provider REST clients for the review-as-merge-gate (Phase 2).
//!
//! Talks to GitHub / Bitbucket Cloud REST APIs to:
//!   - open a pull request (feature branch → default),
//!   - post the `veil/review` commit status/check on the PR head,
//!   - merge the PR (fast path once VEIL signs off).
//!
//! Credentials come from the same runtime env as the transport
//! (`VEIL_GITHUB_TOKEN` / `VEIL_BITBUCKET_TOKEN`). Never hardcoded, never in
//! the engine. Base-URL overrides (`VEIL_GITHUB_API_BASE` /
//! `VEIL_BITBUCKET_API_BASE`) support enterprise hosts and test doubles.
//!
//! The client is synchronous (`reqwest::blocking`) to match `git_origin`.

use serde_json::{Value, json};

use crate::git_origin::{
    GitProvider, RemoteConfig, bitbucket_owner, bitbucket_token_for_api, github_owner,
    github_repos_private, github_token_for_api, redact_secrets, register_origin,
    resolve_credential,
};

/// The status/check context VEIL posts on the PR head. Operators configure
/// branch protection to require this check before merge.
pub const VEIL_REVIEW_CONTEXT: &str = "veil/review";

/// Authenticated GitHub account (GET /user). Token is never returned.
#[derive(Debug, Clone)]
pub struct GithubUser {
    pub login: String,
    pub html_url: String,
}

/// A GitHub repository VEIL created or bound.
#[derive(Debug, Clone)]
pub struct GithubRepo {
    pub full_name: String,
    pub html_url: String,
    pub private: bool,
    /// GitHub `size` is zero (no git objects yet).
    pub empty: bool,
}

fn github_bearer() -> Result<String, String> {
    github_token_for_api().ok_or_else(|| {
        "no GitHub token — set VEIL_GITHUB_TOKEN / GITHUB_TOKEN / GH_TOKEN, or run `gh auth login`"
            .into()
    })
}

fn github_authed(
    req: reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    Ok(req
        .bearer_auth(github_bearer()?)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "veil-runtime")
        .header("X-GitHub-Api-Version", "2022-11-28"))
}

/// `GET /user` with the configured token.
pub fn github_whoami() -> Result<GithubUser, String> {
    let url = format!("{}/user", api_base(GitProvider::GitHub));
    let resp = github_authed(client()?.get(&url))?
        .send()
        .map_err(|e| redact_secrets(&format!("github /user: {e}")))?;
    let status = resp.status();
    let v: Value = resp.json().map_err(|e| format!("github /user json: {e}"))?;
    if !status.is_success() {
        return Err(redact_secrets(&format!(
            "github /user failed ({status}): {v}"
        )));
    }
    let login = v
        .get("login")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if login.is_empty() {
        return Err("github /user returned no login".into());
    }
    Ok(GithubUser {
        html_url: v
            .get("html_url")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        login,
    })
}

fn parse_github_repo(v: &Value) -> GithubRepo {
    let size = v.get("size").and_then(|n| n.as_u64()).unwrap_or(0);
    GithubRepo {
        full_name: v
            .get("full_name")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        html_url: v
            .get("html_url")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        private: v.get("private").and_then(|b| b.as_bool()).unwrap_or(true),
        empty: size == 0,
    }
}

/// `GET /repos/{owner}/{name}`.
pub fn github_get_repo(owner: &str, name: &str) -> Result<GithubRepo, String> {
    let owner = owner.trim().trim_matches('/');
    let name = name.trim().trim_matches('/');
    let url = format!("{}/repos/{owner}/{name}", api_base(GitProvider::GitHub));
    let resp = github_authed(client()?.get(&url))?
        .send()
        .map_err(|e| redact_secrets(&format!("github get repo: {e}")))?;
    let status = resp.status();
    let v: Value = resp
        .json()
        .map_err(|e| format!("github get repo json: {e}"))?;
    if !status.is_success() {
        return Err(redact_secrets(&format!(
            "github get repo {owner}/{name} failed ({status}): {v}"
        )));
    }
    Ok(parse_github_repo(&v))
}

/// Create `owner/name` (user repo if owner is the token user, else org).
/// If it already exists (422), fetch and return it.
pub fn github_create_repo(
    owner: &str,
    name: &str,
    private: bool,
    description: Option<&str>,
) -> Result<GithubRepo, String> {
    let owner = owner.trim().trim_matches('/');
    let name = name.trim().trim_matches('/');
    if owner.is_empty() || name.is_empty() {
        return Err("github create repo: owner and name required".into());
    }
    let me = github_whoami()?;
    let mut payload = json!({
        "name": name,
        "private": private,
        "auto_init": false,
    });
    if let Some(d) = description.map(str::trim).filter(|s| !s.is_empty()) {
        payload["description"] = json!(d);
    }
    let url = if owner.eq_ignore_ascii_case(&me.login) {
        format!("{}/user/repos", api_base(GitProvider::GitHub))
    } else {
        format!("{}/orgs/{owner}/repos", api_base(GitProvider::GitHub))
    };
    let resp = github_authed(client()?.post(&url))?
        .json(&payload)
        .send()
        .map_err(|e| redact_secrets(&format!("github create repo: {e}")))?;
    let status = resp.status();
    let v: Value = resp
        .json()
        .map_err(|e| format!("github create repo json: {e}"))?;
    if status.as_u16() == 422 {
        // Name taken — reuse if we can see it.
        return github_get_repo(owner, name);
    }
    if !status.is_success() {
        return Err(redact_secrets(&format!(
            "github create repo {owner}/{name} failed ({status}): {v}"
        )));
    }
    Ok(parse_github_repo(&v))
}

/// How a new VEIL project should attach to git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginKind {
    /// Caller omitted origin — apply runtime default.
    Unspecified,
    S3,
    Git,
}

#[derive(Debug, Clone)]
pub struct OriginRequest {
    pub kind: OriginKind,
    pub provider: GitProvider,
    /// `org/name` on the provider.
    pub full_name: String,
    /// Create the remote repo if missing. `false` = bind an existing remote.
    pub create: bool,
    pub private: bool,
    pub subpath: Option<String>,
    pub branch: String,
}

impl OriginRequest {
    /// Parse UI / API / tool payload. `default_slug` fills the repo name when
    /// only an owner is given.
    pub fn from_value(v: Option<&Value>, default_slug: &str) -> Result<Self, String> {
        let Some(v) = v.filter(|x| !x.is_null()) else {
            return Ok(Self::unspecified(default_slug));
        };
        let kind_s = v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("git")
            .trim()
            .to_ascii_lowercase();
        if kind_s == "s3" || kind_s == "none" || kind_s == "off" {
            return Ok(Self {
                kind: OriginKind::S3,
                provider: GitProvider::GitHub,
                full_name: String::new(),
                create: false,
                private: true,
                subpath: None,
                branch: "main".into(),
            });
        }
        let provider = GitProvider::parse(
            v.get("provider")
                .and_then(|p| p.as_str())
                .unwrap_or("github"),
        )
        .unwrap_or(GitProvider::GitHub);
        let full_name = parse_full_name(v, default_slug)?;
        let create = v.get("create").and_then(|c| c.as_bool()).unwrap_or(true);
        let private = v
            .get("private")
            .and_then(|c| c.as_bool())
            .unwrap_or_else(github_repos_private);
        let subpath = v
            .get("subpath")
            .and_then(|s| s.as_str())
            .map(|s| s.trim().trim_matches('/').to_string())
            .filter(|s| !s.is_empty());
        let branch = v
            .get("branch")
            .and_then(|s| s.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".into());
        Ok(Self {
            kind: OriginKind::Git,
            provider,
            full_name,
            create,
            private,
            subpath,
            branch,
        })
    }

    fn unspecified(default_slug: &str) -> Self {
        let owner = github_owner().unwrap_or_default();
        let full_name = if owner.is_empty() {
            String::new()
        } else {
            format!("{owner}/{default_slug}")
        };
        Self {
            kind: OriginKind::Unspecified,
            provider: GitProvider::GitHub,
            full_name,
            create: true,
            private: github_repos_private(),
            subpath: None,
            branch: "main".into(),
        }
    }

    pub fn wants_git(&self) -> bool {
        match self.kind {
            OriginKind::Git => true,
            OriginKind::S3 => false,
            OriginKind::Unspecified => {
                crate::git_origin::default_origin_is_github() && !self.full_name.is_empty()
            }
        }
    }
}

fn parse_full_name(v: &Value, default_slug: &str) -> Result<String, String> {
    if let Some(repo) = v
        .get("repo")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().trim_matches('/').to_string())
        .filter(|s| !s.is_empty())
    {
        if repo.contains('/') {
            return Ok(repo);
        }
        let owner = v
            .get("owner")
            .and_then(|s| s.as_str())
            .map(|s| s.trim().trim_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .or_else(github_owner)
            .ok_or_else(|| "origin.repo needs owner/name (or set origin.owner)".to_string())?;
        return Ok(format!("{owner}/{repo}"));
    }
    let owner = v
        .get("owner")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().trim_matches('/').to_string())
        .filter(|s| !s.is_empty());
    let name = v
        .get("name")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().trim_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_slug.to_string());
    let owner = owner.or_else(github_owner).ok_or_else(|| {
        "git origin needs owner/org (origin.owner or VEIL_GITHUB_OWNER)".to_string()
    })?;
    if name.is_empty() {
        return Err("git origin needs a repository name".into());
    }
    Ok(format!("{owner}/{name}"))
}

/// Create or bind a provider repo and return the VEIL remote config.
pub fn provision_origin(
    repo_id: &str,
    _slug: &str,
    description: Option<&str>,
    spec: &OriginRequest,
) -> Result<RemoteConfig, String> {
    let (owner, name) = spec
        .full_name
        .split_once('/')
        .ok_or_else(|| format!("git origin `{}` is not owner/name", spec.full_name))?;
    let empty = match spec.provider {
        GitProvider::GitHub => {
            if spec.create {
                let created = github_create_repo(owner, name, spec.private, description)?;
                created.empty
            } else {
                let existing = github_get_repo(owner, name)?;
                existing.empty
            }
        }
        GitProvider::Bitbucket => {
            if spec.create {
                bitbucket_create_repo(owner, name, spec.private, description)?.empty
            } else {
                bitbucket_get_repo(owner, name)?.empty
            }
        }
    };
    if spec.create && !empty {
        return Err(format!(
            "{} repo {} already has commits — bind it with create=false or pick another name",
            spec.provider.as_str(),
            spec.full_name
        ));
    }
    let _ = empty;
    let cfg = RemoteConfig {
        provider: spec.provider,
        repo: spec.full_name.clone(),
        subpath: spec.subpath.clone(),
        branch: spec.branch.clone(),
    };
    register_origin(repo_id, Some(cfg.clone()));
    Ok(cfg)
}

/// Back-compat wrapper used by older call sites.
pub fn provision_github_origin(
    repo_id: &str,
    slug: &str,
    description: Option<&str>,
) -> Result<RemoteConfig, String> {
    let spec = OriginRequest::unspecified(slug);
    if !spec.wants_git() {
        return Err("GitHub origin is not the runtime default (set VEIL_GITHUB_OWNER)".into());
    }
    provision_origin(repo_id, slug, description, &spec)
}

fn github_list_orgs() -> Vec<String> {
    let url = format!("{}/user/orgs?per_page=50", api_base(GitProvider::GitHub));
    let Ok(client) = client() else {
        return Vec::new();
    };
    let Ok(req) = github_authed(client.get(&url)) else {
        return Vec::new();
    };
    let Ok(resp) = req.send() else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(v) = resp.json::<Value>() else {
        return Vec::new();
    };
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    o.get("login")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn bitbucket_authed(
    req: reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    let tok = bitbucket_token_for_api()
        .ok_or_else(|| "no Bitbucket token (VEIL_BITBUCKET_TOKEN)".to_string())?;
    let user = std::env::var("VEIL_BITBUCKET_USER")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "x-token-auth".into());
    Ok(req
        .basic_auth(user, Some(tok))
        .header("User-Agent", "veil-runtime"))
}

/// Bitbucket Cloud repo.
struct BitbucketRepo {
    full_name: String,
    empty: bool,
}

fn parse_bitbucket_repo(v: &Value) -> BitbucketRepo {
    let full = v
        .get("full_name")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let size = v.get("size").and_then(|n| n.as_u64()).unwrap_or(0);
    BitbucketRepo {
        full_name: full,
        empty: size == 0,
    }
}

fn bitbucket_get_repo(workspace: &str, name: &str) -> Result<BitbucketRepo, String> {
    let url = format!(
        "{}/repositories/{workspace}/{name}",
        api_base(GitProvider::Bitbucket)
    );
    let resp = bitbucket_authed(client()?.get(&url))?
        .send()
        .map_err(|e| redact_secrets(&format!("bitbucket get repo: {e}")))?;
    let status = resp.status();
    let v: Value = resp
        .json()
        .map_err(|e| format!("bitbucket get repo json: {e}"))?;
    if !status.is_success() {
        return Err(redact_secrets(&format!(
            "bitbucket get repo {workspace}/{name} failed ({status}): {v}"
        )));
    }
    Ok(parse_bitbucket_repo(&v))
}

fn bitbucket_create_repo(
    workspace: &str,
    name: &str,
    private: bool,
    description: Option<&str>,
) -> Result<BitbucketRepo, String> {
    let url = format!(
        "{}/repositories/{workspace}/{name}",
        api_base(GitProvider::Bitbucket)
    );
    let mut payload = json!({
        "scm": "git",
        "is_private": private,
        "name": name,
    });
    if let Some(d) = description.map(str::trim).filter(|s| !s.is_empty()) {
        payload["description"] = json!(d);
    }
    let resp = bitbucket_authed(client()?.post(&url))?
        .json(&payload)
        .send()
        .map_err(|e| redact_secrets(&format!("bitbucket create repo: {e}")))?;
    let status = resp.status();
    let v: Value = resp
        .json()
        .map_err(|e| format!("bitbucket create repo json: {e}"))?;
    if status.as_u16() == 400 || status.as_u16() == 409 {
        return bitbucket_get_repo(workspace, name);
    }
    if !status.is_success() {
        return Err(redact_secrets(&format!(
            "bitbucket create repo {workspace}/{name} failed ({status}): {v}"
        )));
    }
    Ok(parse_bitbucket_repo(&v))
}

/// Operator-facing status (no secrets) for Config + create-project UI.
pub fn github_status_json() -> Value {
    let token = github_token_for_api().is_some();
    let owner = github_owner();
    let default = crate::git_origin::default_origin_kind();
    let default_s = match default {
        crate::git_origin::DefaultOriginKind::GitHub => "github",
        crate::git_origin::DefaultOriginKind::S3 => "s3",
    };
    let mut login = None;
    let mut html_url = None;
    let mut error = None;
    let mut orgs: Vec<String> = Vec::new();
    if token {
        match github_whoami() {
            Ok(u) => {
                login = Some(u.login.clone());
                html_url = Some(u.html_url);
                orgs = github_list_orgs();
            }
            Err(e) => error = Some(e),
        }
    }
    let bb_token = bitbucket_token_for_api().is_some();
    json!({
        "connected": token && error.is_none(),
        "token_present": token,
        "login": login,
        "html_url": html_url,
        "owner": owner,
        "orgs": orgs,
        "default_origin": default_s,
        "new_projects_on_github": matches!(default, crate::git_origin::DefaultOriginKind::GitHub),
        "repos_private": github_repos_private(),
        "bitbucket": {
            "token_present": bb_token,
            "owner": bitbucket_owner(),
        },
        "error": error,
        "hint": if !token && !bb_token {
            Some("Set a GitHub token (`gh auth login` or VEIL_GITHUB_TOKEN) and/or VEIL_BITBUCKET_TOKEN. Each project picks provider + owner/name.")
        } else if owner.is_none() && token {
            Some("GitHub is connected. Default owner is VEIL_GITHUB_OWNER; you can still put a project under any org you can write (jdwil, veil, …).")
        } else {
            None
        },
    })
}

#[derive(Debug, Clone)]
pub struct ProviderRepo {
    pub provider: GitProvider,
    /// `org/name`.
    pub repo: String,
}

/// A created/opened pull request on the provider.
#[derive(Debug, Clone)]
pub struct ProviderPr {
    /// Provider PR number/id.
    pub number: u64,
    /// Head commit SHA (for status posting).
    pub head_sha: String,
    /// Web URL.
    pub url: String,
}

/// Bitbucket deployment variant. Cloud and Server/Data Center have different
/// API base URLs, auth conventions, and endpoint shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitbucketVariant {
    /// bitbucket.org — `api.bitbucket.org/2.0`, app-password/OAuth/access-token.
    Cloud,
    /// Self-hosted Server / Data Center — `{base}/rest/...`, HTTP access token.
    Server,
}

impl BitbucketVariant {
    /// Resolve from `VEIL_BITBUCKET_VARIANT` (`cloud` | `server`|`datacenter`|`dc`).
    /// Defaults to Cloud (DLX must set `server` if self-hosted).
    fn resolve() -> Self {
        match std::env::var("VEIL_BITBUCKET_VARIANT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "server" | "datacenter" | "data-center" | "dc" => BitbucketVariant::Server,
            _ => BitbucketVariant::Cloud,
        }
    }
}

fn api_base(provider: GitProvider) -> String {
    let (env, default) = match provider {
        GitProvider::GitHub => ("VEIL_GITHUB_API_BASE", "https://api.github.com"),
        GitProvider::Bitbucket => match BitbucketVariant::resolve() {
            BitbucketVariant::Cloud => ("VEIL_BITBUCKET_API_BASE", "https://api.bitbucket.org/2.0"),
            // Server/DC has no fixed default host — operators MUST set the base.
            BitbucketVariant::Server => ("VEIL_BITBUCKET_API_BASE", ""),
        },
    };
    std::env::var(env)
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("veil-runtime")
        .build()
        .map_err(|e| format!("build http client: {e}"))
}

/// Minimal percent-encoding for query values (space, quotes, a few specials).
fn urlencoding_min(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

impl ProviderRepo {
    pub fn new(provider: GitProvider, repo: impl Into<String>) -> Self {
        Self {
            provider,
            repo: repo.into(),
        }
    }

    /// Resolve the credential for this repo (per-repo → per-org → global), or an
    /// error if none is configured (private repos require auth on every call).
    fn require_cred(&self) -> Result<crate::git_origin::Credential, String> {
        resolve_credential(self.provider, self.repo.trim().trim_matches('/')).ok_or_else(|| {
            format!(
                "no credential configured for {} repo `{}` (set VEIL_GIT_CRED_* or the global token)",
                self.provider.as_str(),
                self.repo
            )
        })
    }

    /// Apply auth to a GitHub request (Bearer token).
    fn gh_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder, String> {
        Ok(req.bearer_auth(self.require_cred()?.bearer()))
    }

    /// Open a PR from `source` → `target`. If one already exists for the branch,
    /// providers return an error; callers should treat that as non-fatal and
    /// look up the existing PR via `find_open_pr`.
    pub fn create_pull_request(
        &self,
        source: &str,
        target: &str,
        title: &str,
        body: &str,
    ) -> Result<ProviderPr, String> {
        match self.provider {
            GitProvider::GitHub => self.github_create_pr(source, target, title, body),
            GitProvider::Bitbucket => match BitbucketVariant::resolve() {
                BitbucketVariant::Cloud => self.bitbucket_create_pr(source, target, title, body),
                BitbucketVariant::Server => {
                    self.bitbucket_server_create_pr(source, target, title, body)
                }
            },
        }
    }

    /// Post the `veil/review` status/check to a commit (the PR head).
    /// `success = true` marks the gate satisfied; `false` = pending/failed.
    pub fn post_veil_review_status(
        &self,
        head_sha: &str,
        success: bool,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<(), String> {
        match self.provider {
            GitProvider::GitHub => {
                self.github_post_status(head_sha, success, description, target_url)
            }
            GitProvider::Bitbucket => match BitbucketVariant::resolve() {
                BitbucketVariant::Cloud => {
                    self.bitbucket_post_status(head_sha, success, description, target_url)
                }
                BitbucketVariant::Server => {
                    self.bitbucket_server_post_status(head_sha, success, description, target_url)
                }
            },
        }
    }

    /// Merge the PR (fast path once VEIL signs off).
    pub fn merge_pull_request(&self, number: u64, message: Option<&str>) -> Result<String, String> {
        match self.provider {
            GitProvider::GitHub => self.github_merge(number, message),
            GitProvider::Bitbucket => match BitbucketVariant::resolve() {
                BitbucketVariant::Cloud => self.bitbucket_merge(number, message),
                BitbucketVariant::Server => self.bitbucket_server_merge(number, message),
            },
        }
    }

    /// Find an open PR whose source (head) branch matches `source`.
    pub fn find_open_pr(&self, source: &str) -> Result<Option<ProviderPr>, String> {
        match self.provider {
            GitProvider::GitHub => self.github_find_open_pr(source),
            GitProvider::Bitbucket => match BitbucketVariant::resolve() {
                BitbucketVariant::Cloud => self.bitbucket_find_open_pr(source),
                BitbucketVariant::Server => self.bitbucket_server_find_open_pr(source),
            },
        }
    }

    fn github_find_open_pr(&self, source: &str) -> Result<Option<ProviderPr>, String> {
        // GitHub head filter wants `owner:branch`.
        let owner = self.repo.split('/').next().unwrap_or("");
        let url = format!(
            "{}/repos/{}/pulls?state=open&head={}:{}",
            api_base(GitProvider::GitHub),
            self.repo,
            owner,
            source
        );
        let resp = self
            .gh_auth(client()?.get(&url))?
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(|e| redact_secrets(&format!("github list prs: {e}")))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .map_err(|e| format!("github list prs json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "github list prs failed ({status}): {v}"
            )));
        }
        let first = v.as_array().and_then(|a| a.first());
        Ok(first.map(|p| ProviderPr {
            number: p.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: p
                .pointer("/head/sha")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: p
                .get("html_url")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        }))
    }

    fn bitbucket_find_open_pr(&self, source: &str) -> Result<Option<ProviderPr>, String> {
        let q = format!("source.branch.name=\"{source}\" AND state=\"OPEN\"");
        let url = format!(
            "{}/repositories/{}/pullrequests?q={}",
            api_base(GitProvider::Bitbucket),
            self.repo,
            urlencoding_min(&q)
        );
        let req = client()?.get(&url);
        let resp = self
            .bb_auth(req)?
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket list prs: {e}")))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .map_err(|e| format!("bitbucket list prs json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "bitbucket list prs failed ({status}): {v}"
            )));
        }
        let first = v
            .pointer("/values")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first());
        Ok(first.map(|p| ProviderPr {
            number: p.get("id").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: p
                .pointer("/source/commit/hash")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: p
                .pointer("/links/html/href")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        }))
    }

    // ---- GitHub ----------------------------------------------------------

    fn github_create_pr(
        &self,
        source: &str,
        target: &str,
        title: &str,
        body: &str,
    ) -> Result<ProviderPr, String> {
        let url = format!(
            "{}/repos/{}/pulls",
            api_base(GitProvider::GitHub),
            self.repo
        );
        let resp = self
            .gh_auth(client()?.post(&url))?
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "title": title, "head": source, "base": target, "body": body }))
            .send()
            .map_err(|e| redact_secrets(&format!("github create pr: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().map_err(|e| format!("github pr json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "github create pr failed ({status}): {v}"
            )));
        }
        Ok(ProviderPr {
            number: v.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: v
                .pointer("/head/sha")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: v
                .get("html_url")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    fn github_post_status(
        &self,
        head_sha: &str,
        success: bool,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<(), String> {
        let url = format!(
            "{}/repos/{}/statuses/{}",
            api_base(GitProvider::GitHub),
            self.repo,
            head_sha
        );
        let mut payload = json!({
            "state": if success { "success" } else { "pending" },
            "context": VEIL_REVIEW_CONTEXT,
            "description": description,
        });
        if let Some(u) = target_url {
            payload["target_url"] = json!(u);
        }
        let resp = self
            .gh_auth(client()?.post(&url))?
            .header("Accept", "application/vnd.github+json")
            .json(&payload)
            .send()
            .map_err(|e| redact_secrets(&format!("github post status: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(redact_secrets(&format!(
                "github post status failed ({status}): {body}"
            )));
        }
        Ok(())
    }

    fn github_merge(&self, number: u64, message: Option<&str>) -> Result<String, String> {
        let url = format!(
            "{}/repos/{}/pulls/{}/merge",
            api_base(GitProvider::GitHub),
            self.repo,
            number
        );
        let mut payload = json!({ "merge_method": "merge" });
        if let Some(m) = message {
            payload["commit_title"] = json!(m);
        }
        let resp = self
            .gh_auth(client()?.put(&url))?
            .header("Accept", "application/vnd.github+json")
            .json(&payload)
            .send()
            .map_err(|e| redact_secrets(&format!("github merge: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().map_err(|e| format!("github merge json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "github merge failed ({status}): {v}"
            )));
        }
        Ok(v.get("sha")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string())
    }

    // ---- Bitbucket Cloud -------------------------------------------------

    fn bb_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder, String> {
        let cred = self.require_cred()?;
        let (user, token) = cred.as_basic();
        // Cloud: x-token-auth:<token> (access token) or user:app_password → Basic.
        Ok(req.basic_auth(user, Some(token)))
    }

    fn bitbucket_create_pr(
        &self,
        source: &str,
        target: &str,
        title: &str,
        body: &str,
    ) -> Result<ProviderPr, String> {
        let url = format!(
            "{}/repositories/{}/pullrequests",
            api_base(GitProvider::Bitbucket),
            self.repo
        );
        let req = client()?.post(&url).json(&json!({
            "title": title,
            "description": body,
            "source": { "branch": { "name": source } },
            "destination": { "branch": { "name": target } },
        }));
        let resp = self
            .bb_auth(req)?
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket create pr: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().map_err(|e| format!("bitbucket pr json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "bitbucket create pr failed ({status}): {v}"
            )));
        }
        Ok(ProviderPr {
            number: v.get("id").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: v
                .pointer("/source/commit/hash")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: v
                .pointer("/links/html/href")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    fn bitbucket_post_status(
        &self,
        head_sha: &str,
        success: bool,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<(), String> {
        let url = format!(
            "{}/repositories/{}/commit/{}/statuses/build",
            api_base(GitProvider::Bitbucket),
            self.repo,
            head_sha
        );
        let payload = json!({
            "key": VEIL_REVIEW_CONTEXT,
            "state": if success { "SUCCESSFUL" } else { "INPROGRESS" },
            "name": "VEIL Review",
            "description": description,
            "url": target_url.unwrap_or("https://veil.local/review"),
        });
        let req = client()?.post(&url).json(&payload);
        let resp = self
            .bb_auth(req)?
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket post status: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(redact_secrets(&format!(
                "bitbucket post status failed ({status}): {body}"
            )));
        }
        Ok(())
    }

    fn bitbucket_merge(&self, number: u64, message: Option<&str>) -> Result<String, String> {
        let url = format!(
            "{}/repositories/{}/pullrequests/{}/merge",
            api_base(GitProvider::Bitbucket),
            self.repo,
            number
        );
        let mut payload = json!({ "merge_strategy": "merge_commit" });
        if let Some(m) = message {
            payload["message"] = json!(m);
        }
        let req = client()?.post(&url).json(&payload);
        let resp = self
            .bb_auth(req)?
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket merge: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(redact_secrets(&format!(
                "bitbucket merge failed ({status}): {body}"
            )));
        }
        // Bitbucket returns the merged PR; the merge commit hash is nested.
        let v: Value = resp.json().unwrap_or(json!({}));
        Ok(v.pointer("/merge_commit/hash")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string())
    }

    // ---- Bitbucket Server / Data Center ----------------------------------
    //
    // Server/DC uses `{base}/rest/api/1.0/projects/{PROJECT}/repos/{slug}` for
    // PRs and `{base}/rest/build-status/1.0/commits/{sha}` for build statuses.
    // `repo` is given as `PROJECT/slug`. Auth is an HTTP access token (Bearer).

    /// Split `repo` (`PROJECT/slug`) into `(project, slug)`.
    fn bb_server_parts(&self) -> Result<(String, String), String> {
        let mut it = self.repo.trim().trim_matches('/').splitn(2, '/');
        let project = it.next().unwrap_or("").to_string();
        let slug = it.next().unwrap_or("").to_string();
        if project.is_empty() || slug.is_empty() {
            return Err(format!(
                "bitbucket server repo must be `PROJECT/slug`, got `{}`",
                self.repo
            ));
        }
        Ok((project, slug))
    }

    fn bb_server_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder, String> {
        // Server/DC HTTP access tokens are presented as Bearer.
        Ok(req.bearer_auth(self.require_cred()?.bearer()))
    }

    fn bitbucket_server_create_pr(
        &self,
        source: &str,
        target: &str,
        title: &str,
        body: &str,
    ) -> Result<ProviderPr, String> {
        let (project, slug) = self.bb_server_parts()?;
        let url = format!(
            "{}/rest/api/1.0/projects/{}/repos/{}/pull-requests",
            api_base(GitProvider::Bitbucket),
            project,
            slug
        );
        let payload = json!({
            "title": title,
            "description": body,
            "fromRef": { "id": format!("refs/heads/{source}") },
            "toRef": { "id": format!("refs/heads/{target}") },
        });
        let resp = self
            .bb_server_auth(client()?.post(&url))?
            .json(&payload)
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket-server create pr: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().map_err(|e| format!("bb-server pr json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "bitbucket-server create pr failed ({status}): {v}"
            )));
        }
        Ok(ProviderPr {
            number: v.get("id").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: v
                .pointer("/fromRef/latestCommit")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: v
                .pointer("/links/self/0/href")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    fn bitbucket_server_post_status(
        &self,
        head_sha: &str,
        success: bool,
        description: &str,
        target_url: Option<&str>,
    ) -> Result<(), String> {
        // Server/DC build-status is keyed by commit, not repo.
        let url = format!(
            "{}/rest/build-status/1.0/commits/{}",
            api_base(GitProvider::Bitbucket),
            head_sha
        );
        let payload = json!({
            "state": if success { "SUCCESSFUL" } else { "INPROGRESS" },
            "key": VEIL_REVIEW_CONTEXT,
            "name": "VEIL Review",
            "description": description,
            "url": target_url.unwrap_or("https://veil.local/review"),
        });
        let resp = self
            .bb_server_auth(client()?.post(&url))?
            .json(&payload)
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket-server post status: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(redact_secrets(&format!(
                "bitbucket-server post status failed ({status}): {body}"
            )));
        }
        Ok(())
    }

    fn bitbucket_server_merge(&self, number: u64, message: Option<&str>) -> Result<String, String> {
        let (project, slug) = self.bb_server_parts()?;
        let url = format!(
            "{}/rest/api/1.0/projects/{}/repos/{}/pull-requests/{}/merge",
            api_base(GitProvider::Bitbucket),
            project,
            slug,
            number
        );
        let mut payload = json!({});
        if let Some(m) = message {
            payload["message"] = json!(m);
        }
        let resp = self
            .bb_server_auth(client()?.post(&url))?
            .json(&payload)
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket-server merge: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().unwrap_or(json!({}));
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "bitbucket-server merge failed ({status}): {v}"
            )));
        }
        Ok(v.pointer("/properties/mergeCommit/id")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string())
    }

    fn bitbucket_server_find_open_pr(&self, source: &str) -> Result<Option<ProviderPr>, String> {
        let (project, slug) = self.bb_server_parts()?;
        let url = format!(
            "{}/rest/api/1.0/projects/{}/repos/{}/pull-requests?state=OPEN&at=refs/heads/{}&direction=OUTGOING",
            api_base(GitProvider::Bitbucket),
            project,
            slug,
            urlencoding_min(source)
        );
        let resp = self
            .bb_server_auth(client()?.get(&url))?
            .send()
            .map_err(|e| redact_secrets(&format!("bitbucket-server list prs: {e}")))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .map_err(|e| format!("bb-server list json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!(
                "bitbucket-server list prs failed ({status}): {v}"
            )));
        }
        let first = v
            .pointer("/values")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first());
        Ok(first.map(|p| ProviderPr {
            number: p.get("id").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: p
                .pointer("/fromRef/latestCommit")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: p
                .pointer("/links/self/0/href")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        }))
    }
}

#[cfg(test)]
mod origin_request_tests {
    use super::*;

    #[test]
    fn parses_full_repo_and_owner_name() {
        let s = OriginRequest::from_value(
            Some(&json!({"provider": "github", "repo": "veil/shop"})),
            "ignored",
        )
        .unwrap();
        assert_eq!(s.full_name, "veil/shop");
        assert!(s.create);
        assert_eq!(s.provider, GitProvider::GitHub);

        let s = OriginRequest::from_value(
            Some(&json!({"owner": "jdwil", "name": "widgets", "create": false, "provider": "github"})),
            "x",
        )
        .unwrap();
        assert_eq!(s.full_name, "jdwil/widgets");
        assert!(!s.create);

        let s = OriginRequest::from_value(Some(&json!({"kind": "s3"})), "x").unwrap();
        assert!(!s.wants_git());
    }
}
