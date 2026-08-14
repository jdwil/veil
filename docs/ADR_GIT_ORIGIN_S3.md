# ADR: Git origin on S3 (real git, not a facsimile)

**Status:** accepted (2026-08-14)  
**Supersedes:** the “git-shaped” session store in [`DURABLE_SESSIONS.md`](./DURABLE_SESSIONS.md)
(S3 tree snapshots + DDB `COMMIT#` + `change_management` SHA pointers under `git/{slug}/refs`).  
**Related:** [`SOURCE_STORE.md`](./SOURCE_STORE.md), [`VCS_MODEL.md`](./VCS_MODEL.md).

## Mission

The ProductHost bucket (`veil-runtime-dev`) is the **primary remote** for each
product repository. A coding session is a **local checkout** so two sessions
never share a working tree. Persist work with **normal git**:

```
main → branch → edit / veil_check / commit loop → push branch → PR → merge to main
```

No parallel “session commit” snapshot protocol. No draft-prefix working tree
as the source of truth. No 40-byte SHA files pretending to be a forge.

## Why the previous model failed

`session_commit` synced the workdir to `repos/{id}/commits/{session}/{short}/`
and wrote a DDB row. `create_branch` isolated writes under
`repos/{id}/drafts/{session}/`. `change_management` stored only ref *pointers*
at `git/{slug}/refs/heads/…` while objects lived in ephemeral
`/tmp/veil-git-cache`. `log()` / `diff_files()` returned `[]`.

That is not git. Agents and the IDE could not share one history, PRs could not
see real branches, and the bucket did not contain a recoverable repository.

## Transport

The library this maps to is **[git-remote-object-store](https://github.com/dekobon/git-remote-object-store)**
(`gix` + S3 as the remote object store). Its default **bundle** engine stores
one git bundle per ref tip — not a raw `.git/` directory — and that is fine.

We do **not** take that crate as a workspace dependency today: it wants
`gix 0.83` and we are on `gix 0.70` (`change_management` / `storage`). The
on-bucket layout is the same so we can swap the transport later.

| Key | Purpose |
|-----|---------|
| `git/{repo_id}/FORMAT` | `bundle` |
| `git/{repo_id}/HEAD` | default branch (`refs/heads/main`) |
| `git/{repo_id}/refs/heads/{branch}/TIP` | tip SHA |
| `git/{repo_id}/refs/heads/{branch}/{sha}.bundle` | full git bundle of that ref |

Local operations use native **git** (checkout, commit, branch, merge, bundle
create/fetch). Session workdirs have a real `.git`.

`repos/{repo_id}/{branch}/` remains a **materialized checkout cache** for
compile / `veil gen` / existing HTTP readers. It is updated on push/merge.
It is not the origin.

## Session = checkout

| Event | Host |
|-------|------|
| `create_project` | DDB META + scaffold + **initial commit pushed to origin** |
| Open / attach session | `git fetch` origin into `{VEIL_WS_ROOT}/{user}/{session}/{slug}/` |
| `create_branch` | `git checkout -b` in a **new** session workdir (clone of base) |
| `write_source` | local working tree (session-isolated) |
| `session_commit` | `git commit` + **push branch bundle** to origin |
| `create_pr` / publish | push the feature branch (already on origin after commits) |
| `merge_pr` | `git merge` into `main` + push `main` |
| End of session | nothing extra if commits were pushed; uncommitted work is session-local |

Two sessions on the same product do not share files. They share **origin**.

## Git does the VCS work

Do not reimplement these. Call git (via `GitOrigin` / the workdir):

| Question | Answer |
|----------|--------|
| What changed in this session? | `git status` / `git diff HEAD` |
| What is history? | `git log` |
| What is this PR? | `git diff main...branch` |
| Land it | `git merge` + push `main` |

DDB `SESSION#` is **which checkout** (user, workdir, active PR id).  
DDB `PR#` is **review metadata** (approvals, comments, SOC 2) pointing at
git branch names + SHAs. Neither is a second commit graph.

VEIL structural IR walk (PR Wizard construct list) is a **view** over the
same two git trees — not a substitute for `git diff`.

## What is forbidden

- Treating `repos/{id}/drafts/…` or `repos/{id}/commits/…` as history.
- Writing `git/{slug}/refs` SHA stubs without objects.
- `aws s3 sync` of a workdir onto `main` as a “merge”.
- Inventing a second commit graph in DDB (`COMMIT#` is not history).
- IDE “Changes / History” backed by anything other than `git status` / `git log`.

## Flag

`VEIL_GIT_ORIGIN=auto` (default): on when durable sessions are on
(`VEIL_SOURCE_MODE≠disk`). `1` / `0` force.

Tests use `VEIL_GIT_STORE_ROOT` as a local bucket (no AWS).
