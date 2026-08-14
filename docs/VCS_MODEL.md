# VCS model

**Superseded for ProductHost (2026-08-14):** see [`ADR_GIT_ORIGIN_S3.md`](./ADR_GIT_ORIGIN_S3.md).

ProductHost source of truth is a **real git remote on S3** (`git/{repo_id}/`,
gix-compatible bundle engine). Coding sessions are **local checkouts**.
`session_commit` is `git commit` + push. PRs merge git branches.

`VEIL_SOURCE_MODE=disk` still uses a project folder (and may `git init` there).
That path is not the live dashlx_dev runtime.

**Not chosen:** DDB-only source trees; SHA-pointer stubs under `git/{slug}/refs`
without objects; S3 workdir snapshots as “commits”.
