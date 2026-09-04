//! Real git origin on the ProductHost bucket.
//!
//! On-bucket layout matches `git-remote-object-store` (gix + S3) **bundle**
//! engine: one git bundle per ref tip. Session workdirs are native git
//! checkouts. See `docs/ADR_GIT_ORIGIN_S3.md`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

const FORMAT_BUNDLE: &str = "bundle";

/// Origin is on when durable sessions are on, unless `VEIL_GIT_ORIGIN` forces it.
/// Self-contained (no `session` / `s3_workspace` import) to avoid a module cycle.
pub fn origin_enabled() -> bool {
    match env_flag("VEIL_GIT_ORIGIN", "auto") {
        Flag::Off => false,
        Flag::On => true,
        Flag::Auto => match env_flag("VEIL_SESSIONS", "auto") {
            Flag::Off => false,
            Flag::On => true,
            Flag::Auto => {
                if crate::config::platform_local() {
                    return true;
                }
                let mode = std::env::var("VEIL_SOURCE_MODE")
                    .unwrap_or_else(|_| "prefer_s3".into())
                    .to_ascii_lowercase();
                !matches!(mode.as_str(), "disk" | "fs" | "filesystem")
            }
        },
    }
}

enum Flag {
    On,
    Off,
    Auto,
}

fn env_flag(name: &str, default: &str) -> Flag {
    match std::env::var(name)
        .unwrap_or_else(|_| default.into())
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "no" => Flag::Off,
        "1" | "true" | "on" | "yes" => Flag::On,
        _ => Flag::Auto,
    }
}

pub fn origin_prefix(repo_id: &str) -> String {
    format!("git/{}/", repo_id.trim().trim_matches('/'))
}

/// Normalise a subpath string: trimmed, no leading/trailing slashes, backslashes
/// to forward slashes, `None` if empty. Rejects `..` traversal segments (returns
/// `None`) so a stored/incoming subpath can never point outside the checkout.
pub fn normalize_subpath(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim().replace('\\', "/");
    let s = s.trim_matches('/');
    if s.is_empty() {
        return None;
    }
    if s.split('/').any(|seg| seg == ".." || seg == ".") {
        return None;
    }
    Some(s.to_string())
}

/// Project root under a checkout for an optional (already-untrusted) subpath.
/// `<work>/<subpath>` when a valid subpath is given, else `<work>`.
pub fn project_root_under(work: &Path, subpath: Option<&str>) -> PathBuf {
    match normalize_subpath(subpath) {
        Some(sub) => work.join(sub),
        None => work.to_path_buf(),
    }
}

pub fn default_git_branch() -> String {
    std::env::var("VEIL_SOURCE_BRANCH").unwrap_or_else(|_| "main".into())
}

/// Git hosting provider for a `GitRemote` backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitProvider {
    GitHub,
    Bitbucket,
}

impl GitProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            GitProvider::GitHub => "github",
            GitProvider::Bitbucket => "bitbucket",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" | "gh" => Some(GitProvider::GitHub),
            "bitbucket" | "bb" => Some(GitProvider::Bitbucket),
            _ => None,
        }
    }

    fn host(&self) -> &'static str {
        match self {
            GitProvider::GitHub => "github.com",
            GitProvider::Bitbucket => "bitbucket.org",
        }
    }

    fn env_key(&self) -> &'static str {
        match self {
            GitProvider::GitHub => "GITHUB",
            GitProvider::Bitbucket => "BITBUCKET",
        }
    }
}

/// A resolved credential for a private (or public) provider repo.
///
/// Held only in memory for the duration of a git invocation. The token is
/// injected via an `http.extraHeader` on the command line (`git -c …`) so it is
/// **never written to the checked-out repo's `.git/config` on disk**. §1.3.1.
#[derive(Clone)]
pub(crate) struct Credential {
    /// HTTP Basic username component (e.g. `x-access-token`, `x-token-auth`, or
    /// a Bitbucket account for app passwords).
    username: String,
    /// Secret token / app password. Never logged.
    token: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the secret.
        write!(
            f,
            "Credential {{ username: {:?}, token: <redacted> }}",
            self.username
        )
    }
}

impl Credential {
    /// `Authorization: Basic base64(username:token)` header value.
    fn basic_auth_header(&self) -> String {
        let raw = format!("{}:{}", self.username, self.token);
        format!("Authorization: Basic {}", base64_encode(raw.as_bytes()))
    }

    /// The `(username, token)` for building an HTTP client Authorization header.
    pub(crate) fn as_basic(&self) -> (String, String) {
        (self.username.clone(), self.token.clone())
    }

    /// The bearer token (for providers that take a raw Bearer, e.g. GitHub
    /// REST, Bitbucket Server HTTP access tokens).
    pub(crate) fn bearer(&self) -> String {
        self.token.clone()
    }
}

/// Config for a real provider remote (GitHub / Bitbucket).
///
/// Self-contained (no `storage` dependency) — the caller translates the DDB
/// `Repo.origin` binding into this struct. Credentials are resolved from the
/// runtime environment, never stored here at rest by the engine.
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    pub provider: GitProvider,
    /// `org/name` on the provider.
    pub repo: String,
    /// Project root within the repo (hybrid model). Empty = repo root.
    pub subpath: Option<String>,
    /// Default branch on the remote.
    pub branch: String,
}

impl RemoteConfig {
    /// Normalise the subpath: trimmed, no leading/trailing slashes, `None` if empty.
    /// Also rejects `..`/`.` traversal segments (defense in depth: a
    /// hand-corrupted catalog binding must never yield a project root that
    /// escapes the checkout via `work.join("../x")`).
    pub fn subpath_norm(&self) -> Option<String> {
        normalize_subpath(self.subpath.as_deref())
    }

    /// The `org` / workspace segment of `org/name`.
    // Retained: repo-identity accessor for provider API routing (not yet wired).
    #[allow(dead_code)]
    fn org(&self) -> &str {
        self.repo
            .trim()
            .trim_matches('/')
            .split('/')
            .next()
            .unwrap_or("")
    }

    /// Resolve the credential for this repo (per-repo → per-org/workspace →
    /// global provider), or `None` for anonymous (public) access.
    fn credential(&self) -> Option<Credential> {
        resolve_credential(self.provider, self.repo.trim().trim_matches('/'))
    }

    /// Tokenless remote URL — this is what gets written to `.git/config`.
    /// Credentials are supplied per-invocation via `auth_args`, never on disk.
    ///
    /// A base-URL override (`VEIL_GITHUB_BASE_URL` / `VEIL_BITBUCKET_BASE_URL`)
    /// supports GitHub Enterprise / Bitbucket Server and local testing
    /// (`file:///path/to/bare` or `http://localhost:.../`).
    fn remote_url(&self) -> String {
        let repo = self.repo.trim().trim_matches('/');
        if let Some(base) = provider_base_url(self.provider) {
            let base = base.trim_end_matches('/');
            if base.starts_with("file://") || base.starts_with('/') {
                // Local/file remotes: use the base directly (single bare repo).
                return base.to_string();
            }
            return format!("{base}/{repo}.git");
        }
        format!("https://{}/{repo}.git", self.provider.host())
    }

    /// Per-invocation git `-c` args that carry the credential as an HTTP header
    /// scoped to the remote URL. The token never touches on-disk config.
    /// Returns an empty vec for anonymous access or `file://` remotes.
    fn auth_args(&self) -> Vec<String> {
        let url = self.remote_url();
        if url.starts_with("file://") || url.starts_with('/') {
            return Vec::new();
        }
        match self.credential() {
            Some(cred) => vec![
                "-c".into(),
                // Scope the header to this remote so it is not sent elsewhere.
                format!("http.{url}.extraHeader={}", cred.basic_auth_header()),
                // Defense in depth: never prompt, never use on-disk helpers.
                "-c".into(),
                "credential.helper=".into(),
            ],
            None => Vec::new(),
        }
    }
}

/// Resolve a credential for `provider` + `org/name`, most specific first:
///   1. per-repo:  `VEIL_GIT_CRED_<PROVIDER>__<ORG>_<NAME>`
///   2. per-org/workspace: `VEIL_GIT_CRED_<PROVIDER>__<ORG>`
///   3. global provider token: `VEIL_<PROVIDER>_TOKEN` (+ aliases)
/// Returns `None` for anonymous (public) access.
///
/// Env keys are upper-cased and non-alphanumerics become `_` (so
/// `dashlx/veil-projects` on github → `VEIL_GIT_CRED_GITHUB__DASHLX_VEIL_PROJECTS`
/// and workspace `VEIL_GIT_CRED_GITHUB__DASHLX`).
pub(crate) fn resolve_credential(provider: GitProvider, repo: &str) -> Option<Credential> {
    let pkey = provider.env_key();
    let mut parts = repo.splitn(2, '/');
    let org = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");

    let sanitize = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };

    // 1) per-repo, 2) per-org
    let candidates = [
        format!("VEIL_GIT_CRED_{pkey}__{}_{}", sanitize(org), sanitize(name)),
        format!("VEIL_GIT_CRED_{pkey}__{}", sanitize(org)),
    ];
    for key in candidates.iter().filter(|k| !k.ends_with("__")) {
        if let Ok(v) = std::env::var(key) {
            if let Some(cred) = parse_cred_value(provider, &v) {
                return Some(cred);
            }
        }
    }

    // 3) global provider token (env, then `gh auth token` for GitHub).
    let tok = provider_token(provider)?;
    Some(default_cred(provider, tok))
}

/// A per-repo/per-org cred value may be `token` or `user:token`.
fn parse_cred_value(provider: GitProvider, raw: &str) -> Option<Credential> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some((user, token)) = raw.split_once(':') {
        let token = token.trim();
        if !token.is_empty() {
            return Some(Credential {
                username: user.trim().to_string(),
                token: token.to_string(),
            });
        }
    }
    Some(default_cred(provider, raw.to_string()))
}

/// Default username convention for a bare token per provider.
fn default_cred(provider: GitProvider, token: String) -> Credential {
    let username = match provider {
        // GitHub App/installation/PAT over HTTPS basic.
        GitProvider::GitHub => "x-access-token".to_string(),
        // Bitbucket: explicit user (app password) if set, else access-token user.
        GitProvider::Bitbucket => {
            provider_user(provider).unwrap_or_else(|| "x-token-auth".to_string())
        }
    };
    Credential { username, token }
}

/// Resolve a global provider token from runtime config (env). Never hardcoded.
fn provider_token(provider: GitProvider) -> Option<String> {
    let names: &[&str] = match provider {
        GitProvider::GitHub => &["VEIL_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"],
        GitProvider::Bitbucket => &["VEIL_BITBUCKET_TOKEN", "BITBUCKET_TOKEN"],
    };
    for n in names {
        if let Ok(v) = std::env::var(n) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    if matches!(provider, GitProvider::GitHub) {
        return gh_cli_token();
    }
    None
}

fn gh_cli_token() -> Option<String> {
    match std::env::var("VEIL_GITHUB_GH_CLI")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "no" => return None,
        _ => {}
    }
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let out = Command::new("gh")
                .args(["auth", "token"])
                .env("GH_PROMPT_DISABLED", "1")
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        })
        .clone()
}

/// DDB META → RemoteConfig. Skipped under `VEIL_GIT_STORE_ROOT` (unit tests).
fn load_origin_from_ddb(repo_id: &str) -> Option<RemoteConfig> {
    if fs_store_root().is_some() {
        return None;
    }
    if crate::config::platform_local() {
        let path = crate::config::local_catalog_path();
        let text = std::fs::read_to_string(path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let repos = v.get("repos")?.as_object()?;
        if let Some(meta) = repos.get(repo_id) {
            return remote_config_from_json(meta.get("origin"));
        }
        for meta in repos.values() {
            let id = meta
                .pointer("/id/value")
                .or_else(|| meta.get("id"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if id == repo_id {
                return remote_config_from_json(meta.get("origin"));
            }
        }
        return None;
    }
    let table = std::env::var("VEIL_DDB_TABLE").ok()?;
    if table.is_empty() {
        return None;
    }
    let key = format!(r##"{{"PK":{{"S":"REPO#{repo_id}"}},"SK":{{"S":"META"}}}}"##);
    let out = aws_base()
        .args([
            "dynamodb",
            "get-item",
            "--table-name",
            &table,
            "--key",
            &key,
            "--projection-expression",
            "#d",
            "--expression-attribute-names",
            r##"{"#d":"data"}"##,
            "--output",
            "json",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let data = v.pointer("/Item/data/S").and_then(|s| s.as_str())?;
    let meta: serde_json::Value = serde_json::from_str(data).ok()?;
    remote_config_from_json(meta.get("origin"))
}

/// Optional provider username (Bitbucket app-password style). Env only.
fn provider_user(provider: GitProvider) -> Option<String> {
    let name = match provider {
        GitProvider::GitHub => "VEIL_GITHUB_USER",
        GitProvider::Bitbucket => "VEIL_BITBUCKET_USER",
    };
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Optional base-URL override for enterprise hosts / self-hosted / local test
/// remotes. E.g. `https://git.corp.example.com` or `file:///tmp/bare.git`.
fn provider_base_url(provider: GitProvider) -> Option<String> {
    let name = match provider {
        GitProvider::GitHub => "VEIL_GITHUB_BASE_URL",
        GitProvider::Bitbucket => "VEIL_BITBUCKET_BASE_URL",
    };
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Standard base64 (for the Authorization header). No external dep.
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Redact any secrets that could appear in git stderr / URLs before logging or
/// returning in an error. Strips `user:token@` from URLs and any known tokens.
pub(crate) fn redact_secrets(s: &str) -> String {
    // 1) Strip `scheme://user:secret@host` → `scheme://user:***@host`.
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for `://`.
        if bytes[i..].starts_with(b"://") {
            out.push_str("://");
            i += 3;
            // Capture up to the next `@`, `/`, whitespace, or end.
            let start = i;
            while i < bytes.len()
                && !matches!(bytes[i], b'@' | b'/' | b' ' | b'\n' | b'\t' | b'"' | b'\'')
            {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'@' {
                // userinfo present → redact the secret portion.
                let userinfo = &s[start..i];
                if let Some((user, _secret)) = userinfo.split_once(':') {
                    out.push_str(user);
                    out.push_str(":***");
                } else {
                    out.push_str("***");
                }
                // keep the '@'
                out.push('@');
                i += 1;
            } else {
                out.push_str(&s[start..i]);
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    // 2) Blanket-redact any configured tokens that slipped through (e.g. headers).
    let mut redacted = out;
    for tok in known_tokens() {
        if tok.len() >= 6 {
            redacted = redacted.replace(&tok, "***");
        }
    }
    redacted
}

/// Collect all tokens the runtime might hold, for blanket redaction.
fn known_tokens() -> Vec<String> {
    let names = [
        "VEIL_GITHUB_TOKEN",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "VEIL_BITBUCKET_TOKEN",
        "BITBUCKET_TOKEN",
    ];
    let mut out = Vec::new();
    for n in names {
        if let Ok(v) = std::env::var(n) {
            let v = v.trim().to_string();
            if v.len() >= 6 {
                out.push(v);
            }
        }
    }
    // Per-repo/per-org cred vars.
    for (k, v) in std::env::vars() {
        if k.starts_with("VEIL_GIT_CRED_") {
            let val = v.trim();
            // value may be `user:token` — redact the token part.
            let tok = val.rsplit(':').next().unwrap_or(val).trim();
            if tok.len() >= 6 {
                out.push(tok.to_string());
            }
        }
    }
    out
}

/// Origin transport backend. `S3Bundle` is the default (existing behaviour);
/// `GitRemote` pushes/fetches a real provider repo.
#[derive(Debug, Clone)]
pub enum OriginBackend {
    S3Bundle,
    GitRemote(RemoteConfig),
}

impl Default for OriginBackend {
    fn default() -> Self {
        OriginBackend::S3Bundle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutMode {
    /// Fetch remotes; do not discard local uncommitted work.
    FetchKeepDirty,
    /// `reset --hard` to the remote tip.
    ResetHard,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub parent: Option<String>,
    pub branch: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub sha: String,
    pub message: String,
    pub author: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusFile {
    pub path: String,
    /// Porcelain XY status (`M`, `A`, `D`, `??`, …).
    pub status: String,
}

pub struct GitOrigin {
    pub repo_id: String,
    pub backend: OriginBackend,
}

/// Process-local origin binding (S3 bundle vs GitHub/Bitbucket remote).
///
/// Populated from DDB `Repo.origin` on catalog reads, create, and bind.
/// Sessions call [`GitOrigin::for_repo`] so a git-backed project never
/// silently falls back to the S3 bundle backend.
#[derive(Clone)]
enum CachedOrigin {
    S3,
    Git(RemoteConfig),
}

fn origin_cache() -> &'static Mutex<HashMap<String, CachedOrigin>> {
    static C: OnceLock<Mutex<HashMap<String, CachedOrigin>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Process-wide lock serializing tests that mutate the global git env vars
/// (`VEIL_GIT_STORE_ROOT` / `VEIL_GIT_ORIGIN`). Env vars are process-global, so
/// per-module locks in different test files do NOT serialize against each other
/// — that races (e.g. `mcp.rs` branch-visibility tests vs `git_origin.rs` origin
/// tests running in parallel). Every test that touches those vars, in ANY module
/// of this crate, MUST acquire THIS lock so there is a single serialization
/// point. Test-only (`#[cfg(test)]`), so it is not compiled into shipped code.
#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static Mutex<()> {
    static ENV: OnceLock<Mutex<()>> = OnceLock::new();
    ENV.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
pub fn clear_origin_cache() {
    if let Ok(mut g) = origin_cache().lock() {
        g.clear();
    }
}

/// Record how a product's git history is stored. `remote = None` means S3.
pub fn register_origin(repo_id: &str, remote: Option<RemoteConfig>) {
    let id = repo_id.trim();
    if id.is_empty() {
        return;
    }
    if let Ok(mut g) = origin_cache().lock() {
        g.insert(
            id.to_string(),
            match remote {
                Some(r) => CachedOrigin::Git(r),
                None => CachedOrigin::S3,
            },
        );
    }
}

/// Parse a DDB/API `origin` object (`{kind, provider, repo, subpath, branch}`).
pub fn remote_config_from_json(origin: Option<&serde_json::Value>) -> Option<RemoteConfig> {
    let o = origin?;
    let kind = o.get("kind").and_then(|k| k.as_str()).unwrap_or("s3");
    if !kind.eq_ignore_ascii_case("git") {
        return None;
    }
    let provider = GitProvider::parse(o.get("provider").and_then(|p| p.as_str()).unwrap_or(""))?;
    let repo = o
        .get("repo")
        .and_then(|r| r.as_str())
        .map(|s| s.trim().trim_matches('/').to_string())
        .filter(|s| s.contains('/'))?;
    let subpath = o
        .get("subpath")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().trim_matches('/').to_string())
        .filter(|s| !s.is_empty());
    let branch = o
        .get("branch")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".into());
    Some(RemoteConfig {
        provider,
        repo,
        subpath,
        branch,
    })
}

/// Register from a full Repo JSON blob (`origin` field optional).
pub fn register_origin_from_repo_json(repo_id: &str, meta: &serde_json::Value) {
    register_origin(repo_id, remote_config_from_json(meta.get("origin")));
}

/// Where new projects store git history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultOriginKind {
    S3,
    GitHub,
}

/// `VEIL_GIT_DEFAULT_ORIGIN`: `s3` | `github` | `auto` (default).
///
/// `auto` picks GitHub only when a token is present **and** `VEIL_GITHUB_OWNER`
/// is set — that env is the operator opt-in so a stray `GITHUB_TOKEN` does not
/// reroute every create onto GitHub.
pub fn default_origin_kind() -> DefaultOriginKind {
    match std::env::var("VEIL_GIT_DEFAULT_ORIGIN")
        .unwrap_or_else(|_| "auto".into())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "github" | "gh" | "git" => DefaultOriginKind::GitHub,
        "s3" | "bundle" => DefaultOriginKind::S3,
        _ => {
            if github_token_for_api().is_some() && github_owner().is_some() {
                DefaultOriginKind::GitHub
            } else {
                DefaultOriginKind::S3
            }
        }
    }
}

pub fn default_origin_is_github() -> bool {
    matches!(default_origin_kind(), DefaultOriginKind::GitHub)
}

/// Owner/org for newly created GitHub repos (`VEIL_GITHUB_OWNER` / `VEIL_GITHUB_ORG`).
pub fn github_owner() -> Option<String> {
    for key in ["VEIL_GITHUB_OWNER", "VEIL_GITHUB_ORG"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().trim_matches('/').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

pub fn github_repos_private() -> bool {
    match std::env::var("VEIL_GITHUB_REPO_PRIVATE")
        .unwrap_or_else(|_| "1".into())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "no" | "public" => false,
        _ => true,
    }
}

/// Token for GitHub HTTPS + REST. Env first, then `gh auth token` (local).
pub fn github_token_for_api() -> Option<String> {
    provider_token(GitProvider::GitHub)
}

pub fn bitbucket_token_for_api() -> Option<String> {
    provider_token(GitProvider::Bitbucket)
}

/// Workspace for new Bitbucket repos (`VEIL_BITBUCKET_OWNER` / `VEIL_BITBUCKET_WORKSPACE`).
pub fn bitbucket_owner() -> Option<String> {
    for key in ["VEIL_BITBUCKET_OWNER", "VEIL_BITBUCKET_WORKSPACE"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().trim_matches('/').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

impl GitOrigin {
    pub fn new(repo_id: impl Into<String>) -> Self {
        Self {
            repo_id: repo_id.into(),
            backend: OriginBackend::S3Bundle,
        }
    }

    /// Origin for a product: GitHub/Bitbucket remote when bound, else S3.
    ///
    /// Looks at the process cache, then DDB META (skipped in unit tests that
    /// set `VEIL_GIT_STORE_ROOT`).
    pub fn for_repo(repo_id: impl AsRef<str>) -> Self {
        let repo_id = repo_id.as_ref().trim();
        if repo_id.is_empty() {
            return GitOrigin::new(repo_id);
        }
        if let Ok(g) = origin_cache().lock() {
            if let Some(c) = g.get(repo_id) {
                return match c {
                    CachedOrigin::S3 => GitOrigin::new(repo_id),
                    CachedOrigin::Git(r) => GitOrigin::with_remote(repo_id, r.clone()),
                };
            }
        }
        if let Some(remote) = load_origin_from_ddb(repo_id) {
            register_origin(repo_id, Some(remote.clone()));
            return GitOrigin::with_remote(repo_id, remote);
        }
        register_origin(repo_id, None);
        GitOrigin::new(repo_id)
    }

    /// Construct a git-backed origin bound to a real provider remote.
    pub fn with_remote(repo_id: impl Into<String>, remote: RemoteConfig) -> Self {
        Self {
            repo_id: repo_id.into(),
            backend: OriginBackend::GitRemote(remote),
        }
    }

    /// True when this origin uses a real provider remote (not S3 bundles).
    pub fn is_git_remote(&self) -> bool {
        matches!(self.backend, OriginBackend::GitRemote(_))
    }

    fn remote(&self) -> Option<&RemoteConfig> {
        match &self.backend {
            OriginBackend::GitRemote(r) => Some(r),
            OriginBackend::S3Bundle => None,
        }
    }

    pub fn exists(&self) -> bool {
        match &self.backend {
            OriginBackend::S3Bundle => {
                store_get(&format!("{}FORMAT", origin_prefix(&self.repo_id))).is_some()
                    || store_get(&format!("{}HEAD", origin_prefix(&self.repo_id))).is_some()
            }
            OriginBackend::GitRemote(cfg) => {
                // Remote exists if we can list refs (auth + repo present).
                git_ls_remote(&cfg.remote_url(), &cfg.auth_args()).is_ok()
            }
        }
    }

    /// Create origin from a working tree if the remote has no tip yet.
    ///
    /// A reachable empty GitHub repo (`auto_init: false`) counts as "exists"
    /// for `ls-remote` but has no heads — we still push the seed.
    pub fn ensure_from_workdir(&self, seed: &Path, branch: &str) -> Result<String, String> {
        if let Some(tip) = self
            .remote_tip(branch)
            .or_else(|| self.remote_tip(&default_git_branch()))
        {
            return Ok(tip);
        }
        init_repo(seed, branch)?;
        if !has_source_files(seed) {
            return Err(format!(
                "cannot init origin {}: no source files in {}",
                self.repo_id,
                seed.display()
            ));
        }
        git(seed, &["add", "-A"])?;
        git(seed, &["commit", "-m", "Initial commit"])?;
        git(seed, &["branch", "-M", branch])?;
        self.push(seed, branch)
    }

    /// Seed a project **subpath** into a (possibly shared, non-empty) repo:
    /// checkout the branch, and if `<subpath>/` has no source files yet, write
    /// the provided scaffold there and push a FRESH commit. No-op if the subpath
    /// already carries a project (idempotent create). `files` = (rel-in-subpath,
    /// content). Returns the pushed commit sha, or `Ok(None)` when the subpath
    /// was already populated.
    ///
    /// Only meaningful for a GitRemote origin bound to `subpath`.
    pub fn seed_subpath(&self, files: &[(String, String)], branch: &str) -> Result<Option<String>, String> {
        let Some(sub) = self.subpath() else {
            return Err("seed_subpath: origin has no subpath binding".into());
        };
        let work = unique_tmp(&format!(
            "veil-subpath-seed-{}",
            &self.repo_id[..8.min(self.repo_id.len())]
        ));
        // Materialize the shared repo. An empty remote is fine — we create the
        // first commit under the subpath.
        let checkout_res = self.checkout(&work, branch, CheckoutMode::ResetHard);
        if let Err(e) = checkout_res {
            // Empty remote: initialise a fresh working tree on `branch`.
            init_repo(&work, branch).map_err(|_| e)?;
        }
        let proj_root = work.join(&sub);
        if has_source_files(&proj_root) {
            let _ = fs::remove_dir_all(&work);
            return Ok(None);
        }
        fs::create_dir_all(&proj_root)
            .map_err(|e| format!("mkdir subpath {}: {e}", proj_root.display()))?;
        for (rel, content) in files {
            let rel = rel.trim_start_matches('/');
            let p = proj_root.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            fs::write(&p, content).map_err(|e| format!("write seed {}: {e}", p.display()))?;
        }
        ensure_gitignore(&work)?;
        // Ensure we are on the target branch (fresh repo starts detached/default).
        if current_branch(&work).ok().as_deref() != Some(branch) {
            let _ = git(&work, &["checkout", "-B", branch]);
        }
        // Commit only the subpath (do not sweep sibling projects).
        git(&work, &["add", "-A", "--", &sub])?;
        if !status_dirty_under(&work, Some(&sub))? {
            let _ = fs::remove_dir_all(&work);
            return Ok(None);
        }
        git(&work, &["commit", "-m", &format!("seed {sub}: initial project scaffold")])?;
        let sha = self.push(&work, branch);
        let _ = fs::remove_dir_all(&work);
        sha.map(Some)
    }

    /// Detect whether the checked-out repo ROOT is already a multi-project VEIL
    /// workspace (has a root `veil.toml [workspace]`). Spec 3
    /// (decision-registry-repo-structure §"Detect-and-Offer-Fix Behavior").
    ///
    /// Checks out the branch into a scratch dir and inspects the ROOT — NOT the
    /// subpath — via [`veil_ir::is_workspace_root`]. An empty/uninitialised
    /// remote counts as "not a workspace" (`Ok(false)`), never an error, so the
    /// caller can offer to initialise. Only meaningful for a subpath-bound
    /// GitRemote origin.
    pub fn subpath_root_is_workspace(&self, branch: &str) -> Result<bool, String> {
        if self.subpath().is_none() {
            return Err("subpath_root_is_workspace: origin has no subpath binding".into());
        }
        let work = unique_tmp(&format!(
            "veil-ws-detect-{}",
            &self.repo_id[..8.min(self.repo_id.len())]
        ));
        // Fresh/empty remote → not a workspace yet (offer to init).
        if self.checkout(&work, branch, CheckoutMode::ResetHard).is_err() {
            let _ = fs::remove_dir_all(&work);
            return Ok(false);
        }
        let is_ws = veil_ir::is_workspace_root(&work);
        let _ = fs::remove_dir_all(&work);
        Ok(is_ws)
    }

    /// Combined, idempotent "seed subproject into a workspace repo" operation
    /// (Spec 3 fix operation). In ONE checkout / commit / push it:
    ///
    /// 1. Optionally initialises the workspace ROOT (`veil.toml [workspace]`)
    ///    when `init_workspace_root` is true (safe if the root already carries
    ///    `[workspace]` — [`project_layout::init_workspace_root`] is a no-op then).
    /// 2. Seeds the subproject scaffold under `<subpath>/` — no-op if the
    ///    subpath already has source files (mirrors [`Self::seed_subpath`]).
    /// 3. Appends `<subpath>` to the root `[workspace] members` list
    ///    ([`project_layout::add_workspace_member`], idempotent + sorted).
    ///
    /// The commit is scoped: it adds ONLY the root `veil.toml` plus the
    /// `<subpath>/` tree (`git add -A -- veil.toml <sub>`) — never sweeping
    /// sibling projects (mirrors `seed_subpath`'s discipline). Returns the
    /// pushed commit sha, or `Ok(None)` when nothing changed (fully idempotent
    /// re-run: workspace present, subpath seeded, member listed).
    ///
    /// When `init_workspace_root` is false this behaves like [`Self::seed_subpath`]
    /// but ALSO appends the member (the "already a workspace" fast path).
    pub fn init_workspace_and_seed_subpath(
        &self,
        files: &[(String, String)],
        branch: &str,
        init_workspace_root: bool,
    ) -> Result<Option<String>, String> {
        let Some(sub) = self.subpath() else {
            return Err("init_workspace_and_seed_subpath: origin has no subpath binding".into());
        };
        let work = unique_tmp(&format!(
            "veil-ws-seed-{}",
            &self.repo_id[..8.min(self.repo_id.len())]
        ));
        // Materialize the shared repo. An empty remote is fine — we create the
        // first commit (root manifest + subpath) here.
        if let Err(e) = self.checkout(&work, branch, CheckoutMode::ResetHard) {
            init_repo(&work, branch).map_err(|_| e)?;
        }

        // 1) Workspace ROOT manifest (idempotent: no-op if already a workspace).
        if init_workspace_root {
            crate::project_layout::init_workspace_root(&work)?;
        }

        // 2) Seed the subproject scaffold (skip if already populated).
        let proj_root = work.join(&sub);
        if !has_source_files(&proj_root) {
            fs::create_dir_all(&proj_root)
                .map_err(|e| format!("mkdir subpath {}: {e}", proj_root.display()))?;
            for (rel, content) in files {
                let rel = rel.trim_start_matches('/');
                let p = proj_root.join(rel);
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
                }
                fs::write(&p, content).map_err(|e| format!("write seed {}: {e}", p.display()))?;
            }
        }

        // 3) Append the member to the root manifest (idempotent + sorted).
        //    Ensures the members list carries this subpath even on a fast-path
        //    (already-a-workspace) re-run where the root manifest already exists.
        crate::project_layout::add_workspace_member(&work, &sub)?;

        ensure_gitignore(&work)?;
        // Ensure we are on the target branch (fresh repo starts detached/default).
        if current_branch(&work).ok().as_deref() != Some(branch) {
            let _ = git(&work, &["checkout", "-B", branch]);
        }

        // Deterministic, scoped stage: ONLY the root manifest + this subpath.
        // Never sweep sibling projects (mirrors seed_subpath discipline).
        git(&work, &["add", "-A", "--", "veil.toml", &sub])?;

        // If nothing changed after staging root + subpath, this is a no-op
        // re-run — idempotent, no double-commit.
        let root_dirty = status_dirty_under(&work, Some("veil.toml"))?;
        let sub_dirty = status_dirty_under(&work, Some(&sub))?;
        if !root_dirty && !sub_dirty {
            let _ = fs::remove_dir_all(&work);
            return Ok(None);
        }

        git(
            &work,
            &[
                "commit",
                "-m",
                &format!("init workspace + seed {sub}: root veil.toml [workspace] + project scaffold"),
            ],
        )?;
        let sha = self.push(&work, branch);
        let _ = fs::remove_dir_all(&work);
        sha.map(Some)
    }

    /// If origin is missing, import `repos/{id}/{branch}/` (legacy tree) as the first commit.
    pub fn import_legacy_tree(&self, branch: &str) -> Result<Option<String>, String> {
        if self.exists() {
            return Ok(self.remote_tip(branch));
        }
        if fs_store_root().is_some() {
            return Ok(None);
        }
        let tmp = unique_tmp(&format!(
            "veil-git-import-{}",
            &self.repo_id[..8.min(self.repo_id.len())]
        ));
        fs::create_dir_all(&tmp).map_err(|e| format!("mkdir import: {e}"))?;
        let src = format!(
            "s3://{}/{}/{branch}/",
            bucket(),
            format!("repos/{}", self.repo_id)
        );
        let out = aws_base()
            .args([
                "s3",
                "sync",
                &src,
                &tmp.to_string_lossy(),
                "--exact-timestamps",
            ])
            .output()
            .map_err(|e| format!("aws s3 sync import: {e}"))?;
        if !out.status.success() {
            let _ = fs::remove_dir_all(&tmp);
            return Ok(None);
        }
        if !has_source_files(&tmp) {
            let _ = fs::remove_dir_all(&tmp);
            return Ok(None);
        }
        let sha = self.ensure_from_workdir(&tmp, branch);
        let _ = fs::remove_dir_all(&tmp);
        sha.map(Some)
    }

    /// Reconcile the git origin with the current `repos/{id}/{branch}/` source
    /// tree. Unlike `import_legacy_tree` (create-only, bails if the bundle
    /// exists), this UPDATES an existing bundle: it checks out the branch, syncs
    /// the current S3 source over the working tree (deleting stale files), and
    /// commits + pushes a new commit **iff** the tree actually changed. This is
    /// what keeps the origin bundle tracking the source store instead of frozen
    /// at first import. Returns Some(sha) if a new commit was pushed, Ok(None)
    /// if already up to date (no diff), Err on failure. No-op in fs-store /
    /// GitRemote modes (those are the source of truth themselves).
    pub fn reconcile_from_repos(&self, branch: &str) -> Result<Option<String>, String> {
        // GitRemote origins ARE the source of truth; nothing to reconcile.
        if matches!(self.backend, OriginBackend::GitRemote(_)) {
            return Ok(None);
        }
        if fs_store_root().is_some() {
            return Ok(None);
        }
        // If the bundle doesn't exist yet, this is just the initial import.
        if !self.exists() {
            return self.import_legacy_tree(branch);
        }
        let tmp = unique_tmp(&format!(
            "veil-git-reconcile-{}",
            &self.repo_id[..8.min(self.repo_id.len())]
        ));
        // Check out the existing bundle so we commit on top of its history.
        if let Err(e) = self.checkout(&tmp, branch, CheckoutMode::ResetHard) {
            let _ = fs::remove_dir_all(&tmp);
            return Err(format!("reconcile checkout: {e}"));
        }
        // Sync current repos/ source over the checkout, deleting files removed
        // from the source so the git tree matches the source store exactly.
        // Preserve the .git dir (aws s3 sync --exclude).
        let src = format!("s3://{}/repos/{}/{branch}/", bucket(), self.repo_id);
        let out = aws_base()
            .args([
                "s3",
                "sync",
                &src,
                &tmp.to_string_lossy(),
                "--exact-timestamps",
                "--delete",
                "--exclude",
                ".git/*",
            ])
            .output()
            .map_err(|e| format!("aws s3 sync reconcile: {e}"))?;
        if !out.status.success() {
            let _ = fs::remove_dir_all(&tmp);
            return Err(format!(
                "aws s3 sync reconcile failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        if !has_source_files(&tmp) {
            // Source store empty/unreachable — do NOT wipe the bundle.
            let _ = fs::remove_dir_all(&tmp);
            return Ok(None);
        }
        ensure_gitignore(&tmp)?;
        // Stage everything; if nothing changed vs the bundle HEAD, we're done.
        git(&tmp, &["add", "-A"])?;
        if !status_dirty_under(&tmp, None)? {
            let _ = fs::remove_dir_all(&tmp);
            return Ok(None);
        }
        git(
            &tmp,
            &["commit", "-m", "Reconcile origin with source store"],
        )?;
        let sha = self.push(&tmp, branch);
        let _ = fs::remove_dir_all(&tmp);
        sha.map(Some)
    }

    /// Ensure origin exists (import legacy tree or seed workdir).
    pub fn ensure(&self, seed: Option<&Path>, branch: &str) -> Result<(), String> {
        if let OriginBackend::GitRemote(cfg) = &self.backend {
            // The provider repo is the source of truth. It must already exist
            // (created out-of-band or via the config API). We only verify reach.
            return if self.exists() {
                Ok(())
            } else {
                Err(format!(
                    "git remote {} ({}) is unreachable or empty — create/clone it first",
                    cfg.repo,
                    cfg.provider.as_str()
                ))
            };
        }
        if self.exists() {
            return Ok(());
        }
        if let Some(seed) = seed {
            if has_source_files(seed) {
                self.ensure_from_workdir(seed, branch)?;
                return Ok(());
            }
        }
        if self.import_legacy_tree(branch)?.is_some() {
            return Ok(());
        }
        Err(format!(
            "no git origin for {} and no seed/legacy tree to import",
            self.repo_id
        ))
    }

    pub fn checkout(
        &self,
        work: &Path,
        branch: &str,
        mode: CheckoutMode,
    ) -> Result<String, String> {
        if let OriginBackend::GitRemote(cfg) = &self.backend {
            return self.checkout_remote(work, branch, mode, cfg);
        }
        self.ensure(if work.exists() { Some(work) } else { None }, branch)
            .or_else(|_| self.ensure(None, &default_git_branch()))?;
        fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;

        let remote = self
            .download_tip(branch)
            .or_else(|| self.download_tip(&default_git_branch()));
        let Some(remote) = remote else {
            return Err(format!("origin {} has no bundles yet", self.repo_id));
        };

        if !work.join(".git").is_dir() {
            clone_from_bundle(work, &remote.bundle_path, &remote.branch)?;
            if remote.branch != branch {
                git(work, &["checkout", "-B", branch])?;
            }
            let _ = fs::remove_file(&remote.bundle_path);
            return git(work, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
        }

        git(
            work,
            &[
                "fetch",
                &remote.bundle_path.to_string_lossy(),
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )?;
        let _ = fs::remove_file(&remote.bundle_path);

        let local_branch = current_branch(work).unwrap_or_default();
        if local_branch != branch {
            if branch_exists_local(work, branch) || remote_branch_exists(work, branch) {
                git(work, &["checkout", branch])?;
            } else {
                git(work, &["checkout", "-B", branch])?;
            }
        }

        match mode {
            CheckoutMode::ResetHard => {
                let tip = format!("origin/{branch}");
                if ref_exists(work, &tip) {
                    git(work, &["reset", "--hard", &tip])?;
                }
            }
            CheckoutMode::FetchKeepDirty => {
                if !status_dirty(work)? {
                    let tip = format!("origin/{branch}");
                    if ref_exists(work, &tip) {
                        let _ = git(work, &["merge", "--ff-only", &tip]);
                    }
                }
            }
        }
        git(work, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
    }

    pub fn create_branch(&self, work: &Path, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("branch name required".into());
        }
        if !work.join(".git").is_dir() {
            return Err("create_branch: workdir is not a git checkout".into());
        }
        git(work, &["checkout", "-B", name])?;
        Ok(())
    }

    pub fn commit(&self, work: &Path, message: &str) -> Result<CommitInfo, String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("commit message required".into());
        }
        if !work.join(".git").is_dir() {
            return Err("commit: workdir is not a git checkout".into());
        }
        ensure_gitignore(work)?;
        // Subpath projects commit only files under their own project root; a
        // sibling project's dirty files in the same shared checkout must not be
        // swept into this project's commit.
        match self.subpath() {
            Some(sub) => {
                git(work, &["add", "-A", "--", &sub])?;
            }
            None => {
                git(work, &["add", "-A"])?;
            }
        }
        if !status_dirty_under(work, self.subpath().as_deref())? {
            return Err(
                "nothing to commit — working tree clean. Edit with write_source first.".into(),
            );
        }
        let parent = git(work, &["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string());
        git(work, &["commit", "-m", message])?;
        let sha = git(work, &["rev-parse", "HEAD"])?.trim().to_string();
        let branch = current_branch(work)?;
        let files = changed_files(work, parent.as_deref())?;
        Ok(CommitInfo {
            sha,
            message: message.to_string(),
            parent,
            branch,
            files,
        })
    }

    pub fn push(&self, work: &Path, branch: &str) -> Result<String, String> {
        if !work.join(".git").is_dir() {
            return Err("push: workdir is not a git checkout".into());
        }
        if let OriginBackend::GitRemote(cfg) = &self.backend {
            return self.push_remote(work, branch, cfg);
        }
        git(work, &["rev-parse", "--verify", branch])?;
        let sha = git(work, &["rev-parse", branch])?.trim().to_string();
        let bundle = unique_tmp(&format!("veil-{}.bundle", &sha[..8.min(sha.len())]));
        git(
            work,
            &["bundle", "create", &bundle.to_string_lossy(), branch],
        )?;
        let bytes = fs::read(&bundle).map_err(|e| format!("read bundle: {e}"))?;
        let _ = fs::remove_file(&bundle);

        let prefix = origin_prefix(&self.repo_id);
        store_put(&format!("{prefix}FORMAT"), FORMAT_BUNDLE.as_bytes())?;
        let prev = store_get(&format!("{prefix}refs/heads/{branch}/TIP"))
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != &sha);

        store_put(&format!("{prefix}refs/heads/{branch}/{sha}.bundle"), &bytes)?;
        store_put(&format!("{prefix}refs/heads/{branch}/TIP"), sha.as_bytes())?;
        if branch == default_git_branch() || !self.exists_head() {
            store_put(
                &format!("{prefix}HEAD"),
                format!("refs/heads/{branch}").as_bytes(),
            )?;
        }
        if let Some(old) = prev {
            let _ = store_delete(&format!("{prefix}refs/heads/{branch}/{old}.bundle"));
        }
        let _ = self.publish_checkout_cache(work, branch);
        Ok(sha)
    }

    pub fn commit_and_push(
        &self,
        work: &Path,
        message: &str,
        branch: &str,
    ) -> Result<CommitInfo, String> {
        let mut info = self.commit(work, message)?;
        if info.branch != branch && !branch.is_empty() {
            git(work, &["branch", "-M", branch])?;
            info.branch = branch.to_string();
        }
        let sha = self.push(work, &info.branch)?;
        info.sha = sha;
        Ok(info)
    }

    /// Merge `source` into `target` in `work` and push `target`.
    pub fn merge_and_push(
        &self,
        work: &Path,
        source: &str,
        target: &str,
    ) -> Result<String, String> {
        if let OriginBackend::GitRemote(cfg) = &self.backend {
            return self.merge_and_push_remote(work, source, target, cfg);
        }
        if source != target && work.join(".git").is_dir() && branch_exists_local(work, source) {
            let _ = self.push(work, source);
        }
        self.checkout(work, target, CheckoutMode::ResetHard)?;
        if source != target {
            if let Some(remote) = self.download_tip(source) {
                let _ = git(
                    work,
                    &[
                        "fetch",
                        &remote.bundle_path.to_string_lossy(),
                        &format!("+refs/heads/{source}:refs/remotes/origin/{source}"),
                    ],
                );
                let _ = fs::remove_file(&remote.bundle_path);
            }
            let merge_ref = if ref_exists(work, source) {
                source.to_string()
            } else {
                format!("origin/{source}")
            };
            merge_no_ff_or_abort(work, &merge_ref, source, target)?;
        }
        self.push(work, target)
    }

    pub fn remote_tip(&self, branch: &str) -> Option<String> {
        if let OriginBackend::GitRemote(cfg) = &self.backend {
            return git_remote_tip(&cfg.remote_url(), &cfg.auth_args(), branch);
        }
        store_get(&format!(
            "{}refs/heads/{branch}/TIP",
            origin_prefix(&self.repo_id)
        ))
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    }

    /// All branch names known to this origin.
    ///
    /// - `GitRemote`: `git ls-remote --heads` (the provider is source of truth).
    /// - `S3Bundle`: enumerate `git/{repo}/refs/heads/{branch}/TIP` keys.
    ///
    /// Best-effort: returns `Ok(vec![])` rather than erroring when the origin
    /// has no heads yet, so callers (list_files/session_status) can degrade to
    /// "only the current branch is known" instead of failing the whole tool.
    /// The result is sorted, de-duplicated, and always includes the default
    /// branch first when present.
    pub fn list_branches(&self) -> Result<Vec<String>, String> {
        let mut names: Vec<String> = match &self.backend {
            OriginBackend::GitRemote(cfg) => {
                let out = git_ls_remote(&cfg.remote_url(), &cfg.auth_args())?;
                out.lines()
                    .filter_map(|line| {
                        let refname = line.split_whitespace().nth(1)?;
                        refname
                            .strip_prefix("refs/heads/")
                            .map(|s| s.to_string())
                            .filter(|s| !s.is_empty())
                    })
                    .collect()
            }
            OriginBackend::S3Bundle => {
                let prefix = format!("{}refs/heads/", origin_prefix(&self.repo_id));
                store_list_branch_tips(&prefix)
            }
        };
        names.sort();
        names.dedup();
        // Surface the default branch first when it exists.
        let def = default_git_branch();
        if let Some(pos) = names.iter().position(|b| b == &def) {
            names.remove(pos);
            names.insert(0, def);
        }
        Ok(names)
    }

    pub fn log(&self, work: &Path, n: usize) -> Result<Vec<LogEntry>, String> {
        let n = n.max(1).min(100).to_string();
        let mut args = vec![
            "log".to_string(),
            format!("-{n}"),
            "--format=%H%x09%P%x09%cI%x09%an%x09%s".to_string(),
        ];
        // Scope to the subpath (hybrid model) so a subpath project's history
        // shows only commits touching its own files.
        if let Some(sub) = self.subpath() {
            args.push("--".to_string());
            args.push(sub);
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = git(work, &arg_refs)?;
        let mut entries = Vec::new();
        for line in out.lines() {
            let mut parts = line.splitn(5, '\t');
            let sha = parts.next().unwrap_or("").to_string();
            if sha.is_empty() {
                continue;
            }
            let parent = parts
                .next()
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty());
            let created_at = parts.next().unwrap_or("").to_string();
            let author = parts.next().unwrap_or("").to_string();
            let message = parts.next().unwrap_or("").to_string();
            let files = git(
                work,
                &["diff-tree", "--no-commit-id", "--name-only", "-r", &sha],
            )
            .unwrap_or_default()
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
            entries.push(LogEntry {
                sha,
                message,
                author,
                parent,
                created_at,
                files,
            });
        }
        Ok(entries)
    }

    /// `git status --porcelain` in a checkout.
    pub fn status_files(work: &Path) -> Result<Vec<StatusFile>, String> {
        if !work.join(".git").is_dir() {
            return Ok(vec![]);
        }
        let out = git(work, &["status", "--porcelain", "-uall"])?;
        Ok(out
            .lines()
            .filter_map(|line| {
                if line.len() < 4 {
                    return None;
                }
                let status = line[..2].trim().to_string();
                let path = line[3..].trim().replace(" -> ", "/");
                if path.is_empty() {
                    return None;
                }
                Some(StatusFile { path, status })
            })
            .collect())
    }

    /// Working-tree diff vs `HEAD` (`git diff HEAD` + untracked as new files).
    pub fn working_diff(work: &Path) -> Result<String, String> {
        Self::working_diff_under(work, None)
    }

    /// `git status --porcelain` scoped to a subpath (hybrid model). `subpath`
    /// `None` = whole checkout. Paths in the returned `StatusFile.path` are
    /// **repo-relative** (i.e. still prefixed by `<subpath>/`), matching git's
    /// porcelain output.
    pub fn status_files_under(
        work: &Path,
        subpath: Option<&str>,
    ) -> Result<Vec<StatusFile>, String> {
        if !work.join(".git").is_dir() {
            return Ok(vec![]);
        }
        let mut args = vec!["status", "--porcelain", "-uall"];
        let sub = normalize_subpath(subpath);
        if let Some(ref s) = sub {
            args.push("--");
            args.push(s.as_str());
        }
        let out = git(work, &args)?;
        Ok(out
            .lines()
            .filter_map(|line| {
                if line.len() < 4 {
                    return None;
                }
                let status = line[..2].trim().to_string();
                let path = line[3..].trim().replace(" -> ", "/");
                if path.is_empty() {
                    return None;
                }
                Some(StatusFile { path, status })
            })
            .collect())
    }

    /// Working-tree diff vs `HEAD` scoped to `subpath` (hybrid model), with
    /// untracked files under the subpath rendered as new-file adds. `subpath`
    /// `None` = whole checkout.
    pub fn working_diff_under(work: &Path, subpath: Option<&str>) -> Result<String, String> {
        if !work.join(".git").is_dir() {
            return Ok(String::new());
        }
        let sub = normalize_subpath(subpath);
        let mut diff_args = vec!["diff", "--no-color", "HEAD"];
        let mut ls_args = vec!["ls-files", "--others", "--exclude-standard"];
        if let Some(ref s) = sub {
            diff_args.push("--");
            diff_args.push(s.as_str());
            ls_args.push("--");
            ls_args.push(s.as_str());
        }
        let tracked = git(work, &diff_args).unwrap_or_default();
        let untracked = git(work, &ls_args).unwrap_or_default();
        if untracked.trim().is_empty() {
            return Ok(tracked);
        }
        let mut out = tracked;
        for path in untracked.lines().map(str::trim).filter(|s| !s.is_empty()) {
            let body = fs::read_to_string(work.join(path)).unwrap_or_default();
            out.push_str(&format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"
            ));
            for line in body.lines() {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// Unified diff `from...to` using origin bundles (no shared workdir).
    pub fn unified_diff_refs(&self, from: &str, to: &str) -> Result<String, String> {
        let tmp = unique_tmp("diff-refs");
        self.checkout(&tmp, to, CheckoutMode::ResetHard)?;
        if let OriginBackend::GitRemote(cfg) = &self.backend {
            // `origin` already points at the provider (set up by checkout_remote).
            let auth = cfg.auth_args();
            let _ = git_auth(
                &tmp,
                &auth,
                &[
                    "fetch",
                    "origin",
                    &format!("+refs/heads/{from}:refs/remotes/origin/{from}"),
                ],
            );
            let spec = format!("origin/{from}...HEAD");
            let patch = git(&tmp, &["diff", "--no-color", &spec]).unwrap_or_default();
            let _ = fs::remove_dir_all(&tmp);
            return Ok(patch);
        }
        if let Some(remote) = self.download_tip(from) {
            let _ = git(
                &tmp,
                &[
                    "fetch",
                    &remote.bundle_path.to_string_lossy(),
                    &format!("+refs/heads/{from}:refs/remotes/origin/{from}"),
                ],
            );
            let _ = fs::remove_file(&remote.bundle_path);
        }
        let spec = format!("origin/{from}...HEAD");
        let patch = git(&tmp, &["diff", "--no-color", &spec]).unwrap_or_default();
        let _ = fs::remove_dir_all(&tmp);
        Ok(patch)
    }

    /// Checkout `branch` into a fresh temp dir. Caller deletes it.
    pub fn checkout_tmp(&self, branch: &str) -> Result<PathBuf, String> {
        let tmp = unique_tmp(&format!("ref-{branch}"));
        self.checkout(&tmp, branch, CheckoutMode::ResetHard)?;
        Ok(tmp)
    }

    /// Repo-relative paths changed between `from` and `to` (name-only). Used for
    /// subpath attribution: which project subdirs a PR touches. Best-effort —
    /// returns an empty vec on any failure.
    pub fn changed_paths_between(&self, from: &str, to: &str) -> Result<Vec<String>, String> {
        let tmp = unique_tmp("diff-names");
        self.checkout(&tmp, to, CheckoutMode::ResetHard)?;
        let spec = match &self.backend {
            OriginBackend::GitRemote(cfg) => {
                let auth = cfg.auth_args();
                let _ = git_auth(
                    &tmp,
                    &auth,
                    &[
                        "fetch",
                        "origin",
                        &format!("+refs/heads/{from}:refs/remotes/origin/{from}"),
                    ],
                );
                format!("origin/{from}...HEAD")
            }
            OriginBackend::S3Bundle => {
                if let Some(remote) = self.download_tip(from) {
                    let _ = git(
                        &tmp,
                        &[
                            "fetch",
                            &remote.bundle_path.to_string_lossy(),
                            &format!("+refs/heads/{from}:refs/remotes/origin/{from}"),
                        ],
                    );
                    let _ = fs::remove_file(&remote.bundle_path);
                }
                format!("origin/{from}...HEAD")
            }
        };
        let out = git(&tmp, &["diff", "--name-only", &spec]).unwrap_or_default();
        let _ = fs::remove_dir_all(&tmp);
        Ok(out
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub fn list_tree_at(&self, work: &Path) -> Result<Vec<String>, String> {
        let out = git(work, &["ls-tree", "-r", "--name-only", "HEAD"])?;
        Ok(out
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    fn exists_head(&self) -> bool {
        store_get(&format!("{}HEAD", origin_prefix(&self.repo_id))).is_some()
    }

    fn download_tip(&self, branch: &str) -> Option<DownloadedBundle> {
        let sha = self.remote_tip(branch)?;
        let key = format!(
            "{}refs/heads/{branch}/{sha}.bundle",
            origin_prefix(&self.repo_id)
        );
        let bytes = store_get(&key)?;
        let path = unique_tmp(&format!("{sha}.bundle"));
        if fs::write(&path, bytes).is_err() {
            return None;
        }
        Some(DownloadedBundle {
            branch: branch.to_string(),
            sha,
            bundle_path: path,
        })
    }

    /// Mirror the pushed tree to `repos/{id}/{branch}/` for compile/HTTP cache.
    pub fn publish_checkout_cache(&self, work: &Path, branch: &str) -> Result<(), String> {
        if fs_store_root().is_some() {
            return Ok(());
        }
        let dest = format!("s3://{}/repos/{}/{}/", bucket(), self.repo_id, branch);
        let out = aws_base()
            .args([
                "s3",
                "sync",
                &work.to_string_lossy(),
                &dest,
                "--exclude",
                ".git/*",
                "--exclude",
                ".veil-session.json",
                "--exclude",
                "target/*",
                "--exclude",
                "generated/*",
                "--exclude",
                "node_modules/*",
            ])
            .output()
            .map_err(|e| format!("aws s3 sync checkout cache: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "checkout cache sync failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(())
    }

    // ---- GitRemote backend ------------------------------------------------

    /// Clone (or fetch) the provider repo into `work` and check out `branch`.
    fn checkout_remote(
        &self,
        work: &Path,
        branch: &str,
        mode: CheckoutMode,
        cfg: &RemoteConfig,
    ) -> Result<String, String> {
        fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
        let url = cfg.remote_url();
        let auth = cfg.auth_args();

        if !work.join(".git").is_dir() {
            clone_remote(work, &url, &auth, branch)?;
        } else {
            // Tokenless origin URL on disk; creds are supplied per-invocation.
            let _ = git(work, &["remote", "set-url", "origin", &url]);
            let _ = scrub_remote_config(work);
            git_auth(
                work,
                &auth,
                &["fetch", "origin", "+refs/heads/*:refs/remotes/origin/*"],
            )
            .map_err(|e| format!("git fetch origin: {e}"))?;
        }

        // Select/create the branch.
        let local_branch = current_branch(work).unwrap_or_default();
        if local_branch != branch {
            if branch_exists_local(work, branch) {
                git(work, &["checkout", branch])?;
            } else if remote_branch_exists(work, branch) {
                git(
                    work,
                    &["checkout", "-B", branch, &format!("origin/{branch}")],
                )?;
            } else {
                // New feature branch off the current tip.
                git(work, &["checkout", "-B", branch])?;
            }
        }

        match mode {
            CheckoutMode::ResetHard => {
                let tip = format!("origin/{branch}");
                if ref_exists(work, &tip) {
                    git(work, &["reset", "--hard", &tip])?;
                }
            }
            CheckoutMode::FetchKeepDirty => {
                if !status_dirty(work)? {
                    let tip = format!("origin/{branch}");
                    if ref_exists(work, &tip) {
                        let _ = git(work, &["merge", "--ff-only", &tip]);
                    }
                }
            }
        }
        git(work, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
    }

    /// Push `branch` to the real provider remote (auth supplied off-disk).
    fn push_remote(&self, work: &Path, branch: &str, cfg: &RemoteConfig) -> Result<String, String> {
        git(work, &["rev-parse", "--verify", branch])?;
        let sha = git(work, &["rev-parse", branch])?.trim().to_string();
        let url = cfg.remote_url();
        let auth = cfg.auth_args();
        // Ensure origin exists with a TOKENLESS url; creds are passed per-invocation.
        if git(work, &["remote", "get-url", "origin"]).is_err() {
            git(work, &["remote", "add", "origin", &url])?;
        } else {
            let _ = git(work, &["remote", "set-url", "origin", &url]);
        }
        let _ = scrub_remote_config(work);
        git_auth(
            work,
            &auth,
            &["push", "origin", &format!("{branch}:{branch}")],
        )
        .map_err(|e| format!("git push origin {branch}: {e}"))?;
        Ok(sha)
    }

    /// Merge `source` into `target` against the provider remote and push.
    fn merge_and_push_remote(
        &self,
        work: &Path,
        source: &str,
        target: &str,
        cfg: &RemoteConfig,
    ) -> Result<String, String> {
        // Make sure any local `source` work is on the remote first.
        if source != target && work.join(".git").is_dir() && branch_exists_local(work, source) {
            let _ = self.push_remote(work, source, cfg);
        }
        self.checkout_remote(work, target, CheckoutMode::ResetHard, cfg)?;
        if source != target {
            let auth = cfg.auth_args();
            let _ = git_auth(
                work,
                &auth,
                &[
                    "fetch",
                    "origin",
                    &format!("+refs/heads/{source}:refs/remotes/origin/{source}"),
                ],
            );
            let merge_ref = if branch_exists_local(work, source) {
                source.to_string()
            } else {
                format!("origin/{source}")
            };
            merge_no_ff_or_abort(work, &merge_ref, source, target)?;
        }
        self.push_remote(work, target, cfg)
    }

    /// Project root within a checkout, honouring the subpath (hybrid model).
    pub fn project_root(&self, work: &Path) -> PathBuf {
        match self.remote().and_then(|c| c.subpath_norm()) {
            Some(sub) => work.join(sub),
            None => work.to_path_buf(),
        }
    }

    /// The normalised subpath for this origin, if bound to a repo subdirectory.
    pub fn subpath(&self) -> Option<String> {
        self.remote().and_then(|c| c.subpath_norm())
    }
}

struct DownloadedBundle {
    branch: String,
    #[allow(dead_code)]
    sha: String,
    bundle_path: PathBuf,
}

fn init_repo(work: &Path, branch: &str) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    if !work.join(".git").is_dir() {
        git(work, &["init", "-b", branch])?;
    }
    ensure_gitignore(work)?;
    Ok(())
}

/// `git ls-remote <url>` — succeeds iff the remote is reachable + authorized.
/// `auth` carries per-invocation credential `-c` args (never on disk).
fn git_ls_remote(url: &str, auth: &[String]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    for a in auth {
        cmd.arg(a);
    }
    cmd.args(["ls-remote", "--heads", url]);
    let out = cmd.output().map_err(|e| format!("git ls-remote: {e}"))?;
    if !out.status.success() {
        return Err(redact_secrets(&format!(
            "git ls-remote failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Remote tip SHA for a branch via `ls-remote`.
fn git_remote_tip(url: &str, auth: &[String], branch: &str) -> Option<String> {
    let out = git_ls_remote(url, auth).ok()?;
    let want = format!("refs/heads/{branch}");
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let sha = parts.next().unwrap_or("");
        let refname = parts.next().unwrap_or("");
        if refname == want && !sha.is_empty() {
            return Some(sha.to_string());
        }
    }
    None
}

/// Clone a provider remote to `work` and check out `branch` (creating it off the
/// default HEAD if the branch does not exist remotely yet). `auth` carries
/// per-invocation credential `-c` args so the token is never written to the
/// clone's `.git/config`.
fn clone_remote(work: &Path, url: &str, auth: &[String], branch: &str) -> Result<(), String> {
    let parent = work.parent().unwrap_or(Path::new("/tmp"));
    let name = work
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("clone dest name")?;
    fs::create_dir_all(parent).map_err(|e| format!("mkdir clone parent: {e}"))?;
    let mut cmd = Command::new("git");
    cmd.current_dir(parent)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "user.name=VEIL", "-c", "user.email=veil@localhost"]);
    for a in auth {
        cmd.arg(a);
    }
    cmd.args(["clone", url, name]);
    let out = cmd.output().map_err(|e| format!("git clone: {e}"))?;
    if !out.status.success() {
        return Err(redact_secrets(&format!(
            "git clone {url} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // The clone wrote the tokenless `url` to config (auth was passed via -c,
    // not embedded). Defense in depth: scrub any credential material anyway.
    let _ = scrub_remote_config(work);
    // Switch to the requested branch (existing remote branch or a new one).
    if remote_branch_exists(work, branch) {
        let _ = git(
            work,
            &["checkout", "-B", branch, &format!("origin/{branch}")],
        );
    } else if !branch_exists_local(work, branch) {
        let _ = git(work, &["checkout", "-B", branch]);
    }
    ensure_gitignore(work)?;
    Ok(())
}

/// Run a git command in `work` with per-invocation auth `-c` args prepended.
/// Redacts secrets from any error. Used for authenticated fetch/push.
fn git_auth(work: &Path, auth: &[String], args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(work)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", git_author_name())
        .env("GIT_AUTHOR_EMAIL", git_author_email())
        .env("GIT_COMMITTER_NAME", git_author_name())
        .env("GIT_COMMITTER_EMAIL", git_author_email())
        .args(["-c", "user.name=VEIL", "-c", "user.email=veil@localhost"]);
    for a in auth {
        cmd.arg(a);
    }
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(redact_secrets(&format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Remove any credential material from a checkout's on-disk config, and make
/// sure the origin URL is tokenless. Best-effort; ignores missing keys.
fn scrub_remote_config(work: &Path) -> Result<(), String> {
    if !work.join(".git").is_dir() {
        return Ok(());
    }
    // Drop any per-URL extraHeader / credential helper that a prior version may
    // have persisted, and strip embedded userinfo from the origin URL.
    let _ = git(work, &["config", "--unset-all", "credential.helper"]);
    if let Ok(url) = git(work, &["remote", "get-url", "origin"]) {
        let url = url.trim();
        if let Some(clean) = strip_url_userinfo(url) {
            if clean != url {
                let _ = git(work, &["remote", "set-url", "origin", &clean]);
            }
        }
    }
    // Remove any http.<url>.extraheader entries written to disk.
    if let Ok(list) = git(work, &["config", "--local", "--name-only", "--list"]) {
        for key in list.lines() {
            let k = key.trim();
            if k.starts_with("http.") && k.ends_with(".extraheader") {
                let _ = git(work, &["config", "--local", "--unset-all", k]);
            }
        }
    }
    Ok(())
}

/// Strip `user:secret@` userinfo from an https/http URL. Returns `None` if the
/// URL has no userinfo to strip.
fn strip_url_userinfo(url: &str) -> Option<String> {
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            if let Some(at) = rest.find('@') {
                // Only strip if the userinfo is before the first '/'.
                let slash = rest.find('/').unwrap_or(rest.len());
                if at < slash {
                    return Some(format!("{scheme}{}", &rest[at + 1..]));
                }
            }
            return Some(url.to_string());
        }
    }
    None
}

/// `git clone <bundle> <work>` — dest must be missing or empty. If `work` already
/// has files (session marker), clone to a sibling and move `.git` + tracked files.
fn clone_from_bundle(work: &Path, bundle: &Path, branch: &str) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    let empty = fs::read_dir(work)
        .map(|rd| rd.filter_map(|e| e.ok()).count() == 0)
        .unwrap_or(true);
    let dest = if empty {
        work.to_path_buf()
    } else {
        unique_tmp("clone")
    };
    if dest != work {
        fs::create_dir_all(&dest).map_err(|e| format!("mkdir clone: {e}"))?;
    }
    // Clone from the parent so we can pass an absolute dest.
    let parent = dest.parent().unwrap_or(Path::new("/tmp"));
    let name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("clone dest name")?;
    let mut cmd = Command::new("git");
    cmd.current_dir(parent)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=VEIL",
            "-c",
            "user.email=veil@localhost",
            "clone",
            "--branch",
            branch,
            &bundle.to_string_lossy(),
            name,
        ]);
    let out = cmd.output().map_err(|e| format!("git clone bundle: {e}"))?;
    if !out.status.success() {
        // Retry without --branch (bundle may advertise HEAD only).
        let mut cmd = Command::new("git");
        cmd.current_dir(parent)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["clone", &bundle.to_string_lossy(), name]);
        let out2 = cmd.output().map_err(|e| format!("git clone bundle: {e}"))?;
        if !out2.status.success() {
            return Err(format!(
                "git clone bundle failed: {}",
                String::from_utf8_lossy(&out2.stderr).trim()
            ));
        }
    }
    if dest != work {
        // Move checkout contents onto the existing workdir (keep session marker).
        copy_tree(&dest, work)?;
        let _ = fs::remove_dir_all(&dest);
    }
    ensure_gitignore(work)?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fn rec(from: &Path, to: &Path) -> Result<(), String> {
        fs::create_dir_all(to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
        for e in fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
            let e = e.map_err(|e| format!("readdir: {e}"))?;
            let src = e.path();
            let dst = to.join(e.file_name());
            if src.is_dir() {
                rec(&src, &dst)?;
            } else {
                fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
            }
        }
        Ok(())
    }
    rec(from, to)
}

fn ensure_gitignore(work: &Path) -> Result<(), String> {
    let gi = work.join(".gitignore");
    if gi.is_file() {
        let cur = fs::read_to_string(&gi).unwrap_or_default();
        if !cur.lines().any(|l| l.trim() == ".veil-session.json") {
            let mut next = cur;
            if !next.ends_with('\n') && !next.is_empty() {
                next.push('\n');
            }
            next.push_str(".veil-session.json\n");
            fs::write(&gi, next).map_err(|e| format!("write .gitignore: {e}"))?;
        }
        return Ok(());
    }
    fs::write(
        gi,
        ".veil-session.json\ntarget/\ngenerated/\nnode_modules/\ndist/\n",
    )
    .map_err(|e| format!("write .gitignore: {e}"))
}

pub fn status_dirty(work: &Path) -> Result<bool, String> {
    status_dirty_under(work, None)
}

/// Dirty check scoped to a subpath (hybrid model). `None` = whole checkout.
pub fn status_dirty_under(work: &Path, subpath: Option<&str>) -> Result<bool, String> {
    if !work.join(".git").is_dir() {
        return Ok(false);
    }
    let mut args = vec!["status", "--porcelain"];
    let sub = normalize_subpath(subpath);
    if let Some(ref s) = sub {
        args.push("--");
        args.push(s.as_str());
    }
    let out = git(work, &args)?;
    Ok(out.lines().any(|l| !l.trim().is_empty()))
}

fn current_branch(work: &Path) -> Result<String, String> {
    let s = git(work, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(s.trim().to_string())
}

fn branch_exists_local(work: &Path, name: &str) -> bool {
    git(
        work,
        &["rev-parse", "--verify", &format!("refs/heads/{name}")],
    )
    .is_ok()
}

fn remote_branch_exists(work: &Path, name: &str) -> bool {
    git(
        work,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/origin/{name}"),
        ],
    )
    .is_ok()
}

fn ref_exists(work: &Path, name: &str) -> bool {
    git(work, &["rev-parse", "--verify", name]).is_ok()
}

fn changed_files(work: &Path, parent: Option<&str>) -> Result<Vec<String>, String> {
    let out = if let Some(p) = parent {
        git(
            work,
            &[
                "diff-tree",
                "--no-commit-id",
                "--name-only",
                "-r",
                p,
                "HEAD",
            ],
        )?
    } else {
        git(work, &["ls-files"])?
    };
    let mut files: Vec<String> = out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    files.sort();
    Ok(files)
}

pub fn has_source_files(root: &Path) -> bool {
    fn rec(p: &Path) -> bool {
        let Ok(rd) = fs::read_dir(p) else {
            return false;
        };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if matches!(
                    name.as_str(),
                    ".git" | "target" | "generated" | "node_modules" | "dist"
                ) {
                    continue;
                }
                if rec(&path) {
                    return true;
                }
            } else if name.ends_with(".veil")
                || name.ends_with(".layer")
                || name == "veil.toml"
                || name == "MISSION.md"
            {
                return true;
            }
        }
        false
    }
    rec(root)
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", git_author_name())
        .env("GIT_AUTHOR_EMAIL", git_author_email())
        .env("GIT_COMMITTER_NAME", git_author_name())
        .env("GIT_COMMITTER_EMAIL", git_author_email())
        .args(["-c", "user.name=VEIL", "-c", "user.email=veil@localhost"])
        .args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run `git merge --no-ff <merge_ref>` and, on ANY failure, restore the
/// checkout to a clean state before returning.
///
/// The cached checkout under `/tmp/veil-git-work/{repo_id16}` is reused across
/// operations, so a merge that leaves conflict markers / unmerged index entries
/// on disk would poison later reads and pushes (C2). This helper guarantees the
/// working tree is clean on every error path:
///   1. `git merge --abort` (undo the in-progress merge; best-effort).
///   2. If any unmerged paths remain (abort was a no-op, or merge left a
///      half-state), force `git reset --hard HEAD` + `git clean` as a backstop.
///   3. Return a clear, greppable "merge conflict between …" error when the
///      failure was an actual conflict, rather than an opaque git stderr blob.
fn merge_no_ff_or_abort(
    work: &Path,
    merge_ref: &str,
    source: &str,
    target: &str,
) -> Result<(), String> {
    let msg = format!("Merge branch '{source}'");
    let res = git(work, &["merge", "--no-ff", "-m", &msg, merge_ref]);
    if res.is_ok() {
        // A merge can "succeed" (exit 0) yet still leave unmerged entries only
        // under exotic configs; re-check to be certain the tree is clean.
        if !has_unmerged_paths(work) {
            return Ok(());
        }
    }

    // Failure (or a surprising unmerged state): detect a conflict, then ALWAYS
    // restore the checkout to a clean state before returning.
    let conflicted = has_unmerged_paths(work);
    // Best-effort undo of the in-progress merge.
    let _ = git(work, &["merge", "--abort"]);
    // Backstop: if anything is still dirty/unmerged (abort failed or there was
    // no MERGE_HEAD), hard-reset and drop untracked cruft so reuse is safe.
    if has_unmerged_paths(work) || !working_tree_clean(work) {
        let _ = git(work, &["reset", "--hard", "HEAD"]);
        let _ = git(work, &["clean", "-fd"]);
    }

    if conflicted {
        return Err(format!(
            "merge conflict between '{source}' and '{target}': \
             resolve the overlapping changes and retry (checkout was reset clean)"
        ));
    }
    // Non-conflict failure: surface the original git error (redacted).
    Err(res.err().unwrap_or_else(|| {
        format!("git merge --no-ff {merge_ref} failed (checkout was reset clean)")
    }))
}

/// True if `git status --porcelain` reports any unmerged (conflicted) path.
/// Porcelain v1 marks unmerged entries with codes containing `U` (e.g. `UU`,
/// `AA`, `DD`, `AU`, `UA`, `DU`, `UD`).
fn has_unmerged_paths(work: &Path) -> bool {
    match git(work, &["status", "--porcelain"]) {
        Ok(out) => out.lines().any(|line| {
            let code = line.get(0..2).unwrap_or("");
            code == "AA"
                || code == "DD"
                || code.starts_with('U')
                || code.ends_with('U')
        }),
        Err(_) => false,
    }
}

/// True if the working tree + index are clean (no staged, unstaged, or
/// untracked changes).
fn working_tree_clean(work: &Path) -> bool {
    match git(work, &["status", "--porcelain"]) {
        Ok(out) => out.trim().is_empty(),
        Err(_) => false,
    }
}

fn git_author_name() -> String {
    std::env::var("VEIL_GIT_AUTHOR_NAME")
        .or_else(|_| std::env::var("VEIL_DEV_USER"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "VEIL".into())
}

fn git_author_email() -> String {
    std::env::var("VEIL_GIT_AUTHOR_EMAIL").unwrap_or_else(|_| "veil@localhost".into())
}

fn unique_tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("veil-git-origin");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}-{}", uuid::Uuid::new_v4(), name))
}

fn bucket() -> String {
    std::env::var("VEIL_S3_BUCKET")
        .or_else(|_| std::env::var("BUCKET"))
        .unwrap_or_else(|_| "veil-runtime-dev".into())
}

fn aws_base() -> Command {
    let mut c = Command::new("aws");
    if let Ok(p) = std::env::var("AWS_PROFILE") {
        c.env("AWS_PROFILE", p);
    }
    if let Ok(r) = std::env::var("AWS_REGION") {
        c.env("AWS_REGION", r);
    }
    c
}

fn fs_store_root() -> Option<PathBuf> {
    std::env::var("VEIL_GIT_STORE_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn store_put(key: &str, bytes: &[u8]) -> Result<(), String> {
    if let Some(root) = fs_store_root() {
        let path = root.join(key);
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))?;
        }
        return fs::write(path, bytes).map_err(|e| format!("write {key}: {e}"));
    }
    let dest = format!("s3://{}/{key}", bucket());
    let mut child = aws_base()
        .args(["s3", "cp", "-", &dest])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("aws s3 cp: {e}"))?;
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or("aws s3 cp: no stdin")?;
        stdin
            .write_all(bytes)
            .map_err(|e| format!("aws s3 cp write: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("aws s3 cp wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 cp {dest} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

fn store_get(key: &str) -> Option<Vec<u8>> {
    if let Some(root) = fs_store_root() {
        return fs::read(root.join(key)).ok();
    }
    let src = format!("s3://{}/{key}", bucket());
    let out = aws_base().args(["s3", "cp", &src, "-"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(out.stdout)
}

/// Enumerate branch names under a `git/{repo}/refs/heads/` prefix by finding
/// each `{branch}/TIP` marker. Branch names may themselves contain `/`
/// (`feat/foo`), so we strip the trailing `/TIP` rather than splitting.
///
/// - fs store: walk the directory tree.
/// - S3: `aws s3 ls --recursive` under the prefix.
///
/// Returns an empty vec on any failure (best-effort; see `list_branches`).
fn store_list_branch_tips(prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(root) = fs_store_root() {
        let base = root.join(prefix);
        collect_tip_dirs(&base, &base, &mut out);
        return out;
    }
    let s3_prefix = format!("s3://{}/{prefix}", bucket());
    let Ok(cmd_out) = aws_base()
        .args(["s3", "ls", "--recursive", &s3_prefix])
        .output()
    else {
        return out;
    };
    if !cmd_out.status.success() {
        return out;
    }
    // Lines: `2026-01-01 00:00:00       41 git/{repo}/refs/heads/{branch}/TIP`
    for line in String::from_utf8_lossy(&cmd_out.stdout).lines() {
        let Some(key) = line.split_whitespace().last() else {
            continue;
        };
        if let Some(rest) = key.split(&format!("{prefix}")).nth(1) {
            if let Some(branch) = rest.strip_suffix("/TIP") {
                if !branch.is_empty() {
                    out.push(branch.to_string());
                }
            }
        } else if let Some(idx) = key.find("refs/heads/") {
            // Fallback when the ls key is not prefixed exactly as expected.
            let tail = &key[idx + "refs/heads/".len()..];
            if let Some(branch) = tail.strip_suffix("/TIP") {
                if !branch.is_empty() {
                    out.push(branch.to_string());
                }
            }
        }
    }
    out
}

/// Recursively find directories containing a `TIP` file under `base`, emitting
/// the path relative to `base` (slash-joined) as the branch name.
fn collect_tip_dirs(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.join("TIP").is_file() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let name = rel.to_string_lossy().replace('\\', "/");
                    if !name.is_empty() {
                        out.push(name);
                    }
                }
            }
            collect_tip_dirs(base, &p, out);
        }
    }
}

fn store_delete(key: &str) -> Result<(), String> {
    if let Some(root) = fs_store_root() {
        let _ = fs::remove_file(root.join(key));
        return Ok(());
    }
    let out = aws_base()
        .args(["s3", "rm", &format!("s3://{}/{key}", bucket())])
        .output()
        .map_err(|e| format!("aws s3 rm: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 rm {key} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn env_lock() -> &'static Mutex<()> { super::test_env_lock() }

    fn with_store<F: FnOnce(&Path)>(f: F) {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let root = unique_tmp("store");
        fs::create_dir_all(&root).unwrap();
        // SAFETY: tests are serialized by ENV_LOCK; this process is the only writer.
        unsafe {
            std::env::set_var("VEIL_GIT_STORE_ROOT", &root);
            std::env::set_var("VEIL_GIT_ORIGIN", "1");
        }
        f(&root);
        unsafe {
            std::env::remove_var("VEIL_GIT_STORE_ROOT");
            std::env::remove_var("VEIL_GIT_ORIGIN");
        }
        let _ = fs::remove_dir_all(&root);
    }

    fn seed_tree() -> PathBuf {
        let p = unique_tmp("seed");
        fs::create_dir_all(p.join("layers")).unwrap();
        fs::write(p.join("main.veil"), "pkg Shop\n").unwrap();
        fs::write(p.join("MISSION.md"), "# Shop\n").unwrap();
        fs::write(p.join("veil.toml"), "[package]\nname = \"shop\"\n").unwrap();
        fs::write(p.join("layers/main.layer"), "layer Main\n").unwrap();
        p
    }

    #[test]
    fn origin_roundtrip_commit_branch_merge() {
        with_store(|_| {
            let origin = GitOrigin::new("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
            let seed = seed_tree();
            let sha0 = origin.ensure_from_workdir(&seed, "main").unwrap();
            assert_eq!(sha0.len(), 40);
            assert!(origin.exists());

            let work = unique_tmp("sess-a");
            origin
                .checkout(&work, "main", CheckoutMode::ResetHard)
                .unwrap();
            assert!(work.join("main.veil").is_file());
            assert!(work.join(".git").is_dir());

            origin.create_branch(&work, "feat-bus").unwrap();
            fs::write(work.join("main.veil"), "pkg Shop\n  rec Topic\n").unwrap();
            let c = origin
                .commit_and_push(&work, "feat: add Topic", "feat-bus")
                .unwrap();
            assert_eq!(c.branch, "feat-bus");
            assert_ne!(c.sha, sha0);

            let other = unique_tmp("sess-b");
            origin
                .checkout(&other, "feat-bus", CheckoutMode::ResetHard)
                .unwrap();
            let body = fs::read_to_string(other.join("main.veil")).unwrap();
            assert!(body.contains("Topic"));

            let patch = origin.unified_diff_refs("main", "feat-bus").unwrap();
            assert!(
                patch.contains("Topic"),
                "git diff main...feat-bus should contain Topic, got:\n{patch}"
            );

            origin.merge_and_push(&work, "feat-bus", "main").unwrap();
            let mainline = unique_tmp("sess-main");
            origin
                .checkout(&mainline, "main", CheckoutMode::ResetHard)
                .unwrap();
            let merged = fs::read_to_string(mainline.join("main.veil")).unwrap();
            assert!(merged.contains("Topic"));

            let _ = fs::remove_dir_all(&seed);
            let _ = fs::remove_dir_all(&work);
            let _ = fs::remove_dir_all(&other);
            let _ = fs::remove_dir_all(&mainline);
        });
    }

    #[test]
    fn second_session_does_not_see_uncommitted() {
        with_store(|_| {
            let origin = GitOrigin::new("11111111-2222-3333-4444-555555555555");
            let seed = seed_tree();
            origin.ensure_from_workdir(&seed, "main").unwrap();

            let a = unique_tmp("iso-a");
            origin
                .checkout(&a, "main", CheckoutMode::ResetHard)
                .unwrap();
            fs::write(a.join("main.veil"), "pkg Dirty\n").unwrap();

            let b = unique_tmp("iso-b");
            origin
                .checkout(&b, "main", CheckoutMode::ResetHard)
                .unwrap();
            let body = fs::read_to_string(b.join("main.veil")).unwrap();
            assert!(body.contains("Shop"));
            assert!(!body.contains("Dirty"));

            let _ = fs::remove_dir_all(&seed);
            let _ = fs::remove_dir_all(&a);
            let _ = fs::remove_dir_all(&b);
        });
    }

    #[test]
    fn list_branches_enumerates_s3_bundle_refs() {
        with_store(|_| {
            let origin = GitOrigin::new("99999999-8888-7777-6666-555555555555");
            let seed = seed_tree();
            origin.ensure_from_workdir(&seed, "main").unwrap();

            // Only main exists so far.
            let mut branches = origin.list_branches().unwrap();
            assert_eq!(branches, vec!["main".to_string()], "just main initially");

            // Push a feature branch and a slash-named branch.
            let work = unique_tmp("lb-work");
            origin
                .checkout(&work, "main", CheckoutMode::ResetHard)
                .unwrap();
            origin.create_branch(&work, "feat-execution-runtime").unwrap();
            fs::write(work.join("main.veil"), "pkg Shop\n  rec A\n").unwrap();
            origin
                .commit_and_push(&work, "feat: a", "feat-execution-runtime")
                .unwrap();

            origin.create_branch(&work, "work/deadbeef").unwrap();
            fs::write(work.join("main.veil"), "pkg Shop\n  rec B\n").unwrap();
            origin
                .commit_and_push(&work, "wip", "work/deadbeef")
                .unwrap();

            branches = origin.list_branches().unwrap();
            // Default branch is first, others present (slash-branch preserved).
            assert_eq!(branches.first().map(String::as_str), Some("main"));
            assert!(
                branches.contains(&"feat-execution-runtime".to_string()),
                "got {branches:?}"
            );
            assert!(
                branches.contains(&"work/deadbeef".to_string()),
                "slash branch preserved, got {branches:?}"
            );
            assert_eq!(branches.len(), 3, "no dupes: {branches:?}");

            let _ = fs::remove_dir_all(&seed);
            let _ = fs::remove_dir_all(&work);
        });
    }

    /// Regression for `incident-inner-agent-stale-branch`: `main` holds the
    /// full tree (incl. `ui.veil`) while a stale feature branch has only a
    /// subset. Seating a fresh session on the resolved default (`main`) MUST
    /// yield the full tree — not the stale branch's partial view — so the agent
    /// never concludes a package is "absent" when it lives on main.
    #[test]
    fn mainline_checkout_has_ui_stale_feature_branch_does_not() {
        with_store(|_| {
            let origin = GitOrigin::new("deadbeef-0000-1111-2222-333344445555");
            let seed = seed_tree();
            // main additionally carries ui.veil (the /agents UI package).
            fs::write(seed.join("ui.veil"), "pkg Ui\n  page Agents\n").unwrap();
            origin.ensure_from_workdir(&seed, "main").unwrap();

            // Create a stale feature branch WITHOUT ui.veil (5-file view).
            let work = unique_tmp("stale");
            origin
                .checkout(&work, "main", CheckoutMode::ResetHard)
                .unwrap();
            origin
                .create_branch(&work, "feat-execution-runtime")
                .unwrap();
            fs::remove_file(work.join("ui.veil")).unwrap();
            origin
                .commit_and_push(&work, "drop ui", "feat-execution-runtime")
                .unwrap();

            // Fresh session seated on the DEFAULT branch (main).
            let fresh_main = unique_tmp("fresh-main");
            origin
                .checkout(&fresh_main, "main", CheckoutMode::ResetHard)
                .unwrap();
            assert!(
                fresh_main.join("ui.veil").is_file(),
                "mainline seat must include ui.veil (the /agents UI)"
            );

            // The stale branch genuinely lacks it — proving the incident's
            // partial view, and why defaulting to it was the bug.
            let stale = unique_tmp("stale-view");
            origin
                .checkout(&stale, "feat-execution-runtime", CheckoutMode::ResetHard)
                .unwrap();
            assert!(
                !stale.join("ui.veil").is_file(),
                "stale feature branch has no ui.veil"
            );

            // Both branches are enumerable so tools can surface the other.
            let branches = origin.list_branches().unwrap();
            assert!(branches.contains(&"main".to_string()));
            assert!(branches.contains(&"feat-execution-runtime".to_string()));

            let _ = fs::remove_dir_all(&seed);
            let _ = fs::remove_dir_all(&work);
            let _ = fs::remove_dir_all(&fresh_main);
            let _ = fs::remove_dir_all(&stale);
        });
    }


    /// exercising the GitRemote transport: clone → branch → commit → push →
    /// re-checkout in a second workdir → diff → merge → verify on the remote.
    #[test]
    fn git_remote_backend_roundtrip() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());

        // Bare "provider" repo, seeded with an initial commit on main.
        let bare = unique_tmp("provider.git");
        fs::create_dir_all(&bare).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .current_dir(&bare)
                .status()
                .unwrap()
                .success()
        );
        let seed = seed_tree();
        {
            git(&seed, &["init", "-b", "main"]).unwrap();
            git(&seed, &["add", "-A"]).unwrap();
            git(&seed, &["commit", "-m", "seed"]).unwrap();
            git(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
            git(&seed, &["push", "origin", "main:main"]).unwrap();
        }

        // Build a GitOrigin whose remote URL is the bare repo (bypass provider host).
        let cfg = RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "test/provider".into(),
            subpath: None,
            branch: "main".into(),
        };
        // Override remote_url via a fake by pushing/checking out through the same URL:
        let url = bare.to_string_lossy().to_string();
        let origin = GitOrigin::with_remote("00000000-1111-2222-3333-444444444444", cfg);

        // checkout main
        let work = unique_tmp("remote-sess-a");
        clone_remote(&work, &url, &[], "main").unwrap();
        assert!(work.join("main.veil").is_file());

        // feature branch + commit + push (direct helpers with the file:// url)
        git(&work, &["checkout", "-B", "feat-x"]).unwrap();
        fs::write(work.join("main.veil"), "pkg Shop\n  rec Topic\n").unwrap();
        git(&work, &["add", "-A"]).unwrap();
        git(&work, &["commit", "-m", "feat: topic"]).unwrap();
        git(&work, &["remote", "set-url", "origin", &url]).unwrap();
        git(&work, &["push", "origin", "feat-x:feat-x"]).unwrap();

        // Second session sees feat-x from the remote.
        let other = unique_tmp("remote-sess-b");
        clone_remote(&other, &url, &[], "feat-x").unwrap();
        let body = fs::read_to_string(other.join("main.veil")).unwrap();
        assert!(body.contains("Topic"), "feat-x should carry Topic");

        // remote_tip via ls-remote resolves the branch head.
        assert!(git_remote_tip(&url, &[], "feat-x").is_some());
        assert!(git_remote_tip(&url, &[], "does-not-exist").is_none());

        // is_git_remote flag + project_root without subpath.
        assert!(origin.is_git_remote());
        assert_eq!(origin.project_root(&work), work);

        let _ = fs::remove_dir_all(&bare);
        let _ = fs::remove_dir_all(&seed);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&other);
    }

    /// C2: a conflicting `merge_and_push` against a local `file://` provider
    /// remote MUST (a) return a clear "merge conflict" error and (b) leave the
    /// cached checkout CLEAN (no conflict markers, `git status` empty) so a
    /// later op reusing that dir can never push/read conflicted content.
    #[test]
    fn merge_conflict_aborts_and_leaves_checkout_clean() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());

        // Bare "provider" repo, seeded on main.
        let bare = unique_tmp("conflict-provider.git");
        fs::create_dir_all(&bare).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .current_dir(&bare)
                .status()
                .unwrap()
                .success()
        );
        let file_url = format!("file://{}", bare.display());
        unsafe {
            std::env::set_var("VEIL_GITHUB_BASE_URL", &file_url);
            std::env::set_var("VEIL_GITHUB_GH_CLI", "0");
        }

        let cfg = RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "test/conflict".into(),
            subpath: None,
            branch: "main".into(),
        };
        let origin = GitOrigin::with_remote("dddddddd-eeee-ffff-0000-111111111111", cfg);

        // Seed main with a file both branches will edit on the SAME line.
        let seed = unique_tmp("conflict-seed");
        fs::create_dir_all(&seed).unwrap();
        fs::write(seed.join("main.veil"), "pkg Shop\n  rec Base\n").unwrap();
        origin.ensure_from_workdir(&seed, "main").unwrap();

        // Branch A: edit line 2 → "Alpha", push.
        let work = unique_tmp("conflict-work");
        origin
            .checkout(&work, "main", CheckoutMode::ResetHard)
            .unwrap();
        origin.create_branch(&work, "feat-a").unwrap();
        fs::write(work.join("main.veil"), "pkg Shop\n  rec Alpha\n").unwrap();
        origin
            .commit_and_push(&work, "feat: alpha", "feat-a")
            .unwrap();

        // Branch B (from the same main base): edit the SAME line → "Beta", push.
        let work_b = unique_tmp("conflict-work-b");
        origin
            .checkout(&work_b, "main", CheckoutMode::ResetHard)
            .unwrap();
        origin.create_branch(&work_b, "feat-b").unwrap();
        fs::write(work_b.join("main.veil"), "pkg Shop\n  rec Beta\n").unwrap();
        origin
            .commit_and_push(&work_b, "feat: beta", "feat-b")
            .unwrap();

        // Merge feat-a into main first (clean, fast).
        origin.merge_and_push(&work, "feat-a", "main").unwrap();

        // Now merge feat-b into main in the SAME cached checkout: this conflicts
        // with the already-merged Alpha change on the same line.
        let err = origin
            .merge_and_push(&work, "feat-b", "main")
            .expect_err("conflicting merge must fail");
        assert!(
            err.contains("merge conflict"),
            "expected a typed merge-conflict error, got: {err}"
        );
        assert!(
            err.contains("feat-b") && err.contains("main"),
            "conflict error should name source+target, got: {err}"
        );

        // (b) The cached checkout MUST be clean afterwards — no conflict markers,
        // no unmerged index entries, no in-progress merge.
        let status = git(&work, &["status", "--porcelain"]).unwrap();
        assert!(
            status.trim().is_empty(),
            "checkout left dirty after aborted merge:\n{status}"
        );
        assert!(
            !work.join(".git/MERGE_HEAD").exists(),
            "MERGE_HEAD still present — merge was not aborted"
        );
        let body = fs::read_to_string(work.join("main.veil")).unwrap();
        assert!(
            !body.contains("<<<<<<<") && !body.contains(">>>>>>>") && !body.contains("======="),
            "conflict markers left on disk:\n{body}"
        );

        // The checkout must remain usable: a follow-up clean merge target
        // (re-merging feat-a, already in main) is a no-op that still succeeds.
        origin.merge_and_push(&work, "feat-a", "main").unwrap();

        unsafe {
            std::env::remove_var("VEIL_GITHUB_BASE_URL");
        }
        let _ = fs::remove_dir_all(&bare);
        let _ = fs::remove_dir_all(&seed);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&work_b);
    }

    #[test]
    fn subpath_norm_and_project_root() {
        let cfg = RemoteConfig {
            provider: GitProvider::Bitbucket,
            repo: "org/mono".into(),
            subpath: Some("/agent-core/".into()),
            branch: "main".into(),
        };
        assert_eq!(cfg.subpath_norm().as_deref(), Some("agent-core"));
        let origin = GitOrigin::with_remote("id", cfg);
        assert_eq!(
            origin.project_root(Path::new("/tmp/work")),
            Path::new("/tmp/work/agent-core")
        );
        assert_eq!(origin.subpath().as_deref(), Some("agent-core"));
    }

    #[test]
    fn normalize_subpath_rejects_traversal_and_trims() {
        assert_eq!(normalize_subpath(Some("  /dlx-auth/ ")).as_deref(), Some("dlx-auth"));
        assert_eq!(normalize_subpath(Some("a/b/c")).as_deref(), Some("a/b/c"));
        assert_eq!(normalize_subpath(Some("a\\b")).as_deref(), Some("a/b"));
        assert_eq!(normalize_subpath(Some("")), None);
        assert_eq!(normalize_subpath(Some("   ")), None);
        assert_eq!(normalize_subpath(None), None);
        // Traversal is refused (never point outside the checkout).
        assert_eq!(normalize_subpath(Some("../evil")), None);
        assert_eq!(normalize_subpath(Some("a/../b")), None);
        assert_eq!(normalize_subpath(Some("./a")), None);
    }

    #[test]
    fn project_root_under_helper() {
        assert_eq!(
            project_root_under(Path::new("/w"), Some("sub")),
            Path::new("/w/sub")
        );
        assert_eq!(project_root_under(Path::new("/w"), None), Path::new("/w"));
        // Traversal collapses to the checkout root (never escapes).
        assert_eq!(
            project_root_under(Path::new("/w"), Some("../x")),
            Path::new("/w")
        );
    }

    /// Subpath-scoped status/diff/commit: two projects A/ and B/ live in one
    /// checkout. Editing A/ must NOT surface B/ in A's status/diff, and A's
    /// commit must not sweep B's dirty files.
    #[test]
    fn subpath_scoped_status_diff_commit() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // A shared "provider" bare repo seeded with two subpaths.
        let bare = unique_tmp("mono-provider.git");
        fs::create_dir_all(&bare).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .current_dir(&bare)
                .status()
                .unwrap()
                .success()
        );
        let seed = unique_tmp("mono-seed");
        fs::create_dir_all(seed.join("dlx-auth/layers")).unwrap();
        fs::create_dir_all(seed.join("dlx-bus/layers")).unwrap();
        fs::write(seed.join("dlx-auth/main.veil"), "pkg Auth\n").unwrap();
        fs::write(seed.join("dlx-auth/veil.toml"), "[package]\nname=\"auth\"\n").unwrap();
        fs::write(seed.join("dlx-bus/main.veil"), "pkg Bus\n").unwrap();
        fs::write(seed.join("dlx-bus/veil.toml"), "[package]\nname=\"bus\"\n").unwrap();
        git(&seed, &["init", "-b", "main"]).unwrap();
        git(&seed, &["add", "-A"]).unwrap();
        git(&seed, &["commit", "-m", "seed mono"]).unwrap();
        git(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        git(&seed, &["push", "origin", "main:main"]).unwrap();

        let url = bare.to_string_lossy().to_string();
        let file_url = format!("file://{}", bare.display());
        unsafe {
            std::env::set_var("VEIL_GITHUB_BASE_URL", &file_url);
            std::env::set_var("VEIL_GITHUB_GH_CLI", "0");
        }
        let cfg_a = RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "org/mono".into(),
            subpath: Some("dlx-auth".into()),
            branch: "main".into(),
        };
        let origin_a = GitOrigin::with_remote("repo-a", cfg_a);

        // Checkout the whole repo (a subpath project shares the checkout).
        let work = unique_tmp("mono-work");
        clone_remote(&work, &url, &[], "main").unwrap();

        // Dirty BOTH subpaths.
        fs::write(work.join("dlx-auth/main.veil"), "pkg Auth\n  rec Token\n").unwrap();
        fs::write(work.join("dlx-bus/main.veil"), "pkg Bus\n  rec Topic\n").unwrap();

        // A's status/diff must show ONLY dlx-auth files.
        let status_a = GitOrigin::status_files_under(&work, Some("dlx-auth")).unwrap();
        assert!(
            status_a.iter().all(|f| f.path.starts_with("dlx-auth/")),
            "A status leaked non-subpath files: {status_a:?}"
        );
        assert!(
            status_a.iter().any(|f| f.path == "dlx-auth/main.veil"),
            "A status missing its own edit: {status_a:?}"
        );
        let diff_a = GitOrigin::working_diff_under(&work, Some("dlx-auth")).unwrap();
        assert!(diff_a.contains("Token"), "A diff should show Token");
        assert!(!diff_a.contains("Topic"), "A diff must NOT show B's Topic:\n{diff_a}");

        // dirty check is subpath-scoped and true for both.
        assert!(status_dirty_under(&work, Some("dlx-auth")).unwrap());
        assert!(status_dirty_under(&work, Some("dlx-bus")).unwrap());

        // A's commit must include only dlx-auth (git add -A -- dlx-auth).
        origin_a.create_branch(&work, "feat-auth").unwrap();
        let commit = origin_a
            .commit_and_push(&work, "feat: token", "feat-auth")
            .unwrap();
        assert!(
            commit.files.iter().all(|f| f.starts_with("dlx-auth/")),
            "A commit swept non-subpath files: {:?}",
            commit.files
        );
        // B's edit is still uncommitted (not swept into A's commit).
        assert!(
            status_dirty_under(&work, Some("dlx-bus")).unwrap(),
            "B's edit must remain uncommitted after A commits its subpath"
        );

        unsafe {
            std::env::remove_var("VEIL_GITHUB_BASE_URL");
        }
        let _ = fs::remove_dir_all(&bare);
        let _ = fs::remove_dir_all(&seed);
        let _ = fs::remove_dir_all(&work);
    }

    /// §1.3.1: per-repo → per-org/workspace → global provider precedence, and
    /// anonymous (None) when nothing is configured.
    #[test]
    fn credential_precedence_per_repo_org_global() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // Clean slate.
        let clear = || unsafe {
            for k in [
                "VEIL_GITHUB_TOKEN",
                "GITHUB_TOKEN",
                "GH_TOKEN",
                "VEIL_GIT_CRED_GITHUB__DASHLX",
                "VEIL_GIT_CRED_GITHUB__DASHLX_VEIL_PROJECTS",
                "VEIL_GITHUB_BASE_URL",
            ] {
                std::env::remove_var(k);
            }
            // Tests must not pick up the operator's `gh auth token`.
            std::env::set_var("VEIL_GITHUB_GH_CLI", "0");
        };
        clear();

        // Anonymous when nothing set.
        assert!(resolve_credential(GitProvider::GitHub, "dashlx/veil-projects").is_none());

        // Global token → x-access-token user.
        unsafe { std::env::set_var("VEIL_GITHUB_TOKEN", "global-tok") };
        let c = resolve_credential(GitProvider::GitHub, "dashlx/veil-projects").unwrap();
        assert_eq!(c.username, "x-access-token");
        assert_eq!(c.token, "global-tok");

        // Per-org overrides global.
        unsafe { std::env::set_var("VEIL_GIT_CRED_GITHUB__DASHLX", "org-tok") };
        let c = resolve_credential(GitProvider::GitHub, "dashlx/veil-projects").unwrap();
        assert_eq!(c.token, "org-tok");

        // Per-repo overrides per-org, and supports user:token form.
        unsafe { std::env::set_var("VEIL_GIT_CRED_GITHUB__DASHLX_VEIL_PROJECTS", "bot:repo-tok") };
        let c = resolve_credential(GitProvider::GitHub, "dashlx/veil-projects").unwrap();
        assert_eq!(c.username, "bot");
        assert_eq!(c.token, "repo-tok");

        clear();
    }

    /// §1.3.1: auth is carried as an http.extraHeader `-c` arg (off disk) and the
    /// tokenless URL is what would be written to config.
    #[test]
    fn auth_args_are_off_disk_header_and_url_is_tokenless() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("VEIL_GITHUB_BASE_URL");
            std::env::set_var("VEIL_GITHUB_TOKEN", "secrettoken123");
        }
        let cfg = RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "dashlx/priv".into(),
            subpath: None,
            branch: "main".into(),
        };
        let url = cfg.remote_url();
        assert_eq!(url, "https://github.com/dashlx/priv.git");
        assert!(!url.contains("secrettoken123"), "url must be tokenless");

        let args = cfg.auth_args();
        // Expect an http.<url>.extraHeader carrying a Basic header, plus a
        // disabled credential helper.
        let joined = args.join(" ");
        assert!(joined.contains("extraHeader=Authorization: Basic"));
        assert!(joined.contains("credential.helper="));
        // The base64 must decode to x-access-token:secrettoken123 but we at
        // least assert the raw token is not present verbatim.
        assert!(
            !joined.contains("secrettoken123"),
            "token must be base64, not raw"
        );

        unsafe { std::env::remove_var("VEIL_GITHUB_TOKEN") };
    }

    #[test]
    fn redact_secrets_strips_userinfo_and_tokens() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("VEIL_GITHUB_TOKEN", "ghp_supersecretvalue") };
        let s = "fatal: could not read from https://x-access-token:ghp_supersecretvalue@github.com/o/r.git";
        let r = redact_secrets(s);
        assert!(!r.contains("ghp_supersecretvalue"), "raw token leaked: {r}");
        assert!(
            r.contains("x-access-token:***@github.com"),
            "userinfo not redacted: {r}"
        );
        unsafe { std::env::remove_var("VEIL_GITHUB_TOKEN") };
    }

    #[test]
    fn base64_encode_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(
            base64_encode(b"x-access-token:tok"),
            "eC1hY2Nlc3MtdG9rZW46dG9r"
        );
    }

    #[test]
    fn strip_url_userinfo_removes_creds() {
        assert_eq!(
            strip_url_userinfo("https://u:p@github.com/o/r.git").as_deref(),
            Some("https://github.com/o/r.git")
        );
        assert_eq!(
            strip_url_userinfo("https://github.com/o/r.git").as_deref(),
            Some("https://github.com/o/r.git")
        );
        assert_eq!(strip_url_userinfo("file:///tmp/x"), None);
    }

    /// §1.3.1: after clone, the on-disk .git/config carries no token and no
    /// extraheader. Uses a local bare remote (no real creds needed).
    #[test]
    fn cloned_config_has_no_token_on_disk() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let bare = unique_tmp("scrub-provider.git");
        fs::create_dir_all(&bare).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .current_dir(&bare)
                .status()
                .unwrap()
                .success()
        );
        let seed = seed_tree();
        git(&seed, &["init", "-b", "main"]).unwrap();
        git(&seed, &["add", "-A"]).unwrap();
        git(&seed, &["commit", "-m", "seed"]).unwrap();
        git(&seed, &["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        git(&seed, &["push", "origin", "main:main"]).unwrap();

        let work = unique_tmp("scrub-work");
        // Simulate an auth arg being present (harmless header for a file remote).
        let auth = vec![
            "-c".to_string(),
            "http.https://example/.extraHeader=Authorization: Basic Zm9v".to_string(),
        ];
        clone_remote(&work, &bare.to_string_lossy(), &auth, "main").unwrap();

        // Read the on-disk config; it must not contain a token/extraheader.
        let cfg = fs::read_to_string(work.join(".git/config")).unwrap_or_default();
        assert!(
            !cfg.to_lowercase().contains("extraheader"),
            "config leaked header: {cfg}"
        );
        assert!(!cfg.contains("Basic Zm9v"), "config leaked auth: {cfg}");

        let _ = fs::remove_dir_all(&bare);
        let _ = fs::remove_dir_all(&seed);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn remote_config_from_json_git_and_s3() {
        let git = serde_json::json!({
            "kind": "git",
            "provider": "github",
            "repo": "acme/widgets",
            "subpath": "/app/",
            "branch": "main"
        });
        let cfg = remote_config_from_json(Some(&git)).expect("git origin");
        assert_eq!(cfg.provider, GitProvider::GitHub);
        assert_eq!(cfg.repo, "acme/widgets");
        assert_eq!(cfg.subpath.as_deref(), Some("app"));

        let s3 = serde_json::json!({ "kind": "s3" });
        assert!(remote_config_from_json(Some(&s3)).is_none());
    }

    #[test]
    fn for_repo_uses_registered_github_origin() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        clear_origin_cache();
        unsafe {
            std::env::set_var("VEIL_GIT_STORE_ROOT", unique_tmp("reg-store"));
        }
        let id = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
        let cfg = RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "me/app".into(),
            subpath: None,
            branch: "main".into(),
        };
        register_origin(id, Some(cfg));
        let origin = GitOrigin::for_repo(id);
        assert!(origin.is_git_remote());
        register_origin(id, None);
        let origin = GitOrigin::for_repo(id);
        assert!(!origin.is_git_remote());
        clear_origin_cache();
        unsafe {
            std::env::remove_var("VEIL_GIT_STORE_ROOT");
        }
    }

    #[test]
    fn git_remote_seeds_empty_provider_repo() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let bare = unique_tmp("empty-provider.git");
        fs::create_dir_all(&bare).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-b", "main"])
                .current_dir(&bare)
                .status()
                .unwrap()
                .success()
        );
        let file_url = format!("file://{}", bare.display());
        unsafe {
            std::env::set_var("VEIL_GITHUB_BASE_URL", &file_url);
            std::env::set_var("VEIL_GITHUB_GH_CLI", "0");
        }
        let cfg = RemoteConfig {
            provider: GitProvider::GitHub,
            repo: "test/empty".into(),
            subpath: None,
            branch: "main".into(),
        };
        let origin = GitOrigin::with_remote("cccccccc-dddd-eeee-ffff-000000000000", cfg);
        assert!(origin.exists(), "empty bare repo should be reachable");
        assert!(origin.remote_tip("main").is_none());

        let seed = seed_tree();
        let sha = origin
            .ensure_from_workdir(&seed, "main")
            .expect("seed empty remote");
        assert_eq!(sha.len(), 40);

        let work = unique_tmp("from-empty");
        origin
            .checkout(&work, "main", CheckoutMode::ResetHard)
            .expect("checkout seeded remote");
        assert!(work.join("main.veil").is_file());

        unsafe {
            std::env::remove_var("VEIL_GITHUB_BASE_URL");
        }
        let _ = fs::remove_dir_all(&bare);
        let _ = fs::remove_dir_all(&seed);
        let _ = fs::remove_dir_all(&work);
    }
}
