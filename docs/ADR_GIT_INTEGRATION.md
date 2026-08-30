# ADR: Git integration — real provider repos + review-as-merge-gate

**Status:** accepted (2026-08-28)
**Related:** [`ADR_GIT_ORIGIN_S3.md`](./ADR_GIT_ORIGIN_S3.md), [`SOURCE_STORE.md`](./SOURCE_STORE.md), [`VCS_MODEL.md`](./VCS_MODEL.md)
**Spec:** `git-integration.md`

## Mission

A VEIL project can live in a **real GitHub / Bitbucket repository** instead of
only the S3 bundle store. The hybrid model: a project is either its own repo or
a **subdirectory** of a shared repo (`subpath`). Git holds code + history + PRs;
**VEIL's visual/structural review is the merge gate**, surfaced to the provider
as a required `veil/review` status check.

The S3 bundle backend stays the **default**. Git is additive.

## Backend abstraction (Phase 1)

`GitOrigin` (`crates/veil-server/src/git_origin.rs`) gained an `OriginBackend`:

- `S3Bundle` — the existing behavior (default; unchanged).
- `GitRemote { provider, repo: "org/name", subpath, branch }` — a real remote.

Only the **transport** methods dispatch on the backend: `exists`, `ensure`,
`checkout` → `checkout_remote`, `push` → `push_remote`, `merge_and_push` →
`merge_and_push_remote`, `remote_tip` → `git_ls_remote`, and `unified_diff_refs`.
The local git machinery (commit, create_branch, log, diff, status) is
provider-agnostic and untouched.

`GitRemote` uses native git over an authenticated https URL: `git clone` /
`git fetch` / `git push origin`. When `subpath` is set the VEIL project root is
`{workdir}/{subpath}` (`GitOrigin::project_root`).

## Project → repo binding

The DDB `Repo` record (`storage::domain::types::Repo`) has an optional
`origin: Option<OriginBinding>`:

```json
{ "origin": { "kind": "git", "provider": "github",
              "repo": "dashlx/veil-projects", "subpath": "agent-core",
              "branch": "main" } }
```

Absent / `kind:"s3"` → S3 backend. `#[serde(default)]` keeps pre-existing
records deserializable. `veil-runtime/src/origin_resolve.rs::git_origin_for`
translates the binding into a `GitOrigin` backend (kept in the runtime to avoid
a `storage → veil-server` cycle).

## Provider auth (config, never in the engine)

Tokens come from runtime env, resolved at clone/push/API time (most specific
first: per-repo → per-org/workspace → global):

| Scope             | GitHub                                                 | Bitbucket                          |
|-------------------|--------------------------------------------------------|------------------------------------|
| per-repo          | `VEIL_GIT_CRED_GITHUB__<ORG>_<NAME>`                   | `VEIL_GIT_CRED_BITBUCKET__<ORG>_<NAME>` |
| per-org/workspace | `VEIL_GIT_CRED_GITHUB__<ORG>`                          | `VEIL_GIT_CRED_BITBUCKET__<ORG>`   |
| global            | `VEIL_GITHUB_TOKEN`, `GITHUB_TOKEN`, `GH_TOKEN`        | `VEIL_BITBUCKET_TOKEN`, `BITBUCKET_TOKEN` |

Per-repo/org values may be `token` or `user:token`. No match → anonymous
(public read). See "Private repositories" below for the full model.

Base-URL overrides for enterprise / self-hosted / testing:
`VEIL_GITHUB_BASE_URL` / `VEIL_BITBUCKET_BASE_URL` (clone/push) and
`VEIL_GITHUB_API_BASE` / `VEIL_BITBUCKET_API_BASE` (REST). A `file://` clone base
is used directly with no auth (local test remotes).

Commit author = the authenticated VEIL user (via `VEIL_GIT_AUTHOR_*`);
committer = the runtime service identity.

## Private repositories (§1.3.1 — public + private from day one)

Private is the default posture; public repos simply resolve to no credential
(anonymous clone).

- **Per-repo / per-workspace credential mapping** — `resolve_credential(provider,
  "org/name")` resolves most-specific-first:
  1. per-repo `VEIL_GIT_CRED_<PROVIDER>__<ORG>_<NAME>`
  2. per-org/workspace `VEIL_GIT_CRED_<PROVIDER>__<ORG>`
  3. global `VEIL_<PROVIDER>_TOKEN` (+ aliases)
  `None` → anonymous. Values may be `token` or `user:token`. One runtime can thus
  serve public GitHub repos (anonymous) and private DLX Bitbucket repos
  (workspace token) simultaneously.
- **Authenticated on every read.** `clone`, `fetch`, `checkout`, and `ls-remote`
  all present the credential for a `GitRemote`; public repos are anonymous.
- **Token never on disk.** The remote URL written to `.git/config` is
  **tokenless**. Credentials are injected per-invocation as
  `git -c http.<url>.extraHeader="Authorization: Basic <b64>" -c credential.helper=`,
  so the secret never lands in the checked-out repo's config. `scrub_remote_config`
  strips any userinfo from the origin URL and removes `http.*.extraheader` /
  `credential.helper` on disk (defense in depth) after clone/fetch/push.
- **Never logged.** `redact_secrets` strips `user:secret@` from URLs and
  blanket-replaces known tokens; it is applied to all git stderr errors and all
  provider REST errors. `Credential`'s `Debug` never renders the secret.
- **GitHub private**: GitHub App installation token (fine-grained, rotating) is
  preferred; a PAT with `repo` scope works for a quick start. Presented as
  `x-access-token:<token>` (Basic header) for git and Bearer for REST.
- **Bitbucket private**: workspace/repo access token or app password. The
  deployment **variant** matters — set `VEIL_BITBUCKET_VARIANT`:
  - `cloud` (default): `api.bitbucket.org/2.0`, Basic auth
    (`x-token-auth:<token>` or `user:app_password`); PRs at
    `/repositories/{repo}/pullrequests`, build status at
    `/commit/{sha}/statuses/build`.
  - `server` / `datacenter` / `dc`: self-hosted; **operators MUST set
    `VEIL_BITBUCKET_API_BASE`**. HTTP access token as Bearer; PRs at
    `/rest/api/1.0/projects/{PROJECT}/repos/{slug}/pull-requests` (repo given as
    `PROJECT/slug`), build status at `/rest/build-status/1.0/commits/{sha}`.

## File I/O for git-backed projects

`veil-runtime/src/git_files.rs` operates on the checked-out working tree at
`project_root/subpath` (cache dir `/tmp/veil-git-work/{repo_id16}`): reads from a
fresh checkout, writes commit + push the branch. `platform_http` routes
`read_file` / `write_file` / `list_files` to this path when the repo is
git-backed; S3-backed projects are unchanged.

## Config API

- `POST /api/repos/{id}/origin` `{ kind, provider, repo, subpath, branch }` —
  bind (or reset with `kind:"s3"`). Records the binding and reports
  `remote_reachable`.
- `GET /api/repos/{id}/origin` — report the current binding.
- `POST /api/list-files` — list files (git or S3).

## Review-as-merge-gate (Phase 2)

`crates/veil-server/src/git_provider.rs` is a synchronous
(`reqwest::blocking`) REST client for GitHub + Bitbucket Cloud:

1. **Author change → PR.** On `create_pull_request`, git-backed projects also
   open a provider PR (feature branch → default) and post the initial
   `veil/review` = **pending** status on the PR head.
2. **VEIL review is the gate.** VEIL's existing `/review` surface renders the
   structural/visual IR diff of the branch (unchanged UX). Sign-off is recorded
   as today (`veil_server::review::may_ship`).
3. **Sign-off → required check + merge.** On merge, once `may_ship` passes,
   the runtime posts `veil/review` = **success** to the PR head, then (unless
   `VEIL_GIT_AUTO_MERGE` is off) drives the provider merge.

### Coexistence contract (operators MUST read)

- Provider PR review (line diffs, comments) remains available but is **NOT** the
  gate.
- **VEIL visual review IS the gate**, surfaced to the provider as the required
  `veil/review` status check.
- Operators MUST configure **branch protection** on the default branch to
  **require the `veil/review` status check** before merge. Without it the
  provider will not block merge on VEIL sign-off.
- Do NOT replace VEIL's visual review with the provider's line-diff review.

## Out of scope

Layer registry / central catalog (future). Version pinning already lives in
`use` statements. GitHub App installation-token minting (a PAT / installation
token supplied via env works today).

## Verification

- `crates/veil-server/src/git_origin.rs` tests: `git_remote_backend_roundtrip`
  (file:// bare repo as provider), `subpath_norm_and_project_root`, plus the S3
  regression tests — all pass.
- `crates/veil-runtime/tests/git_backend_roundtrip.rs`: write into a subpath →
  push → read from a fresh checkout; file outside the subpath is invisible.
- `crates/veil-server/tests/git_provider_contract.rs`: create PR / post
  `veil/review=success` / merge shape the correct GitHub requests (mock server).
