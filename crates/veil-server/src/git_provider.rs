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

use serde_json::{json, Value};

use crate::git_origin::{redact_secrets, resolve_credential, GitProvider};

/// The status/check context VEIL posts on the PR head. Operators configure
/// branch protection to require this check before merge.
pub const VEIL_REVIEW_CONTEXT: &str = "veil/review";

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
            BitbucketVariant::Cloud => {
                ("VEIL_BITBUCKET_API_BASE", "https://api.bitbucket.org/2.0")
            }
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
        let v: Value = resp.json().map_err(|e| format!("github list prs json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!("github list prs failed ({status}): {v}")));
        }
        let first = v.as_array().and_then(|a| a.first());
        Ok(first.map(|p| ProviderPr {
            number: p.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: p
                .pointer("/head/sha")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: p.get("html_url").and_then(|s| s.as_str()).unwrap_or("").to_string(),
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
        let v: Value = resp.json().map_err(|e| format!("bitbucket list prs json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!("bitbucket list prs failed ({status}): {v}")));
        }
        let first = v.pointer("/values").and_then(|a| a.as_array()).and_then(|a| a.first());
        Ok(first.map(|p| ProviderPr {
            number: p.get("id").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: p
                .pointer("/source/commit/hash")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: p.pointer("/links/html/href").and_then(|s| s.as_str()).unwrap_or("").to_string(),
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
        let url = format!("{}/repos/{}/pulls", api_base(GitProvider::GitHub), self.repo);
        let resp = self
            .gh_auth(client()?.post(&url))?
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "title": title, "head": source, "base": target, "body": body }))
            .send()
            .map_err(|e| redact_secrets(&format!("github create pr: {e}")))?;
        let status = resp.status();
        let v: Value = resp.json().map_err(|e| format!("github pr json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!("github create pr failed ({status}): {v}")));
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
            return Err(redact_secrets(&format!("github post status failed ({status}): {body}")));
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
            return Err(redact_secrets(&format!("github merge failed ({status}): {v}")));
        }
        Ok(v.get("sha").and_then(|s| s.as_str()).unwrap_or("").to_string())
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
            return Err(redact_secrets(&format!("bitbucket create pr failed ({status}): {v}")));
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
            return Err(redact_secrets(&format!("bitbucket post status failed ({status}): {body}")));
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
            return Err(redact_secrets(&format!("bitbucket merge failed ({status}): {body}")));
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
            return Err(redact_secrets(&format!("bitbucket-server create pr failed ({status}): {v}")));
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
            return Err(redact_secrets(&format!("bitbucket-server post status failed ({status}): {body}")));
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
            return Err(redact_secrets(&format!("bitbucket-server merge failed ({status}): {v}")));
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
        let v: Value = resp.json().map_err(|e| format!("bb-server list json: {e}"))?;
        if !status.is_success() {
            return Err(redact_secrets(&format!("bitbucket-server list prs failed ({status}): {v}")));
        }
        let first = v.pointer("/values").and_then(|a| a.as_array()).and_then(|a| a.first());
        Ok(first.map(|p| ProviderPr {
            number: p.get("id").and_then(|n| n.as_u64()).unwrap_or(0),
            head_sha: p
                .pointer("/fromRef/latestCommit")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            url: p.pointer("/links/self/0/href").and_then(|s| s.as_str()).unwrap_or("").to_string(),
        }))
    }
}
