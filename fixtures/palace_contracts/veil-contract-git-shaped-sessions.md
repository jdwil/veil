# veil-contract-git-shaped-sessions

**Type:** Concept  
**Summary:** Agent coding uses git-shaped branch → edit → veil_check → commit → **open pull request**. Autosave ≠ commit. Humans merge after review (PR Wizard). Host gates + orchestrator enforce cadence. Scoreboard is err/warn counts. Never auto-merges.

## Contract

- **Branch before multi-step product work.** Prefer an isolated work line (`create_branch` / `branch_name`), not silent rewrites on shared main when doing a fix campaign.
- **Autosave is durable S3 write-through** — work is saved on every successful `write_source`. That is **not** a commit.
- **Commit** = named checkpoint with a **message** (`session_commit` or `POST /api/sessions/{id}/commits`). Host **rejects empty** commits. Message should name the slice **and why**.
- **Scoreboard:** host `veil_check` / `host_check` **error_count** / **warning_count**. Trust host severity — do not claim clean if HOST_CHECK_SEVERITY=errors.
- **Same-turn diagnostics:** If a post-edit check shows **new** errors/warnings the agent introduced, **fix them on the same turn** before claiming done or opening a PR.
- **One diagnostic class per commit** when fixing large lists.
- **Start of coding:** `resolve_coding_target` or `run_coding_plan` matches **open unmerged PRs** by scope (never Merged/Closed). Auto-bind, Present modal if ambiguous, or new work line.
- **End of agent task = open a pull request, not merge.**
  - `run_coding_plan` `coding.finish_task` **or** `create_pr` + `submit_pr`
  - `create_pr` reuses session `active_pr_id` unless `force_new`
  - Operator walks PR Wizard DiffItems. **FORBIDDEN:** merge unless operator explicitly says merge/land
- **Terminology:** product name is **pull request (PR)**. “Change Request” reserved for future ticket systems. API paths may still say `pull_requests`.
- **IDE Changes** = **Uncommitted** + **History**. Agent decides branch/commit; human decides merge.
- **Bang law (ACS-010):** `find!` → `Opt<T>`; force with `require` / `.unwrap()`. See `veil-contract-bang-opt-res`.

## Agent loop (mandatory shape)

```text
1. run_coding_plan(coding.fix_diagnostics) or resolve_coding_target
2. session_status — know branch / uncommitted / host_check
3. Multi-step? create_branch (e.g. fix-type-mismatch)
4. veil_check → note host baseline
5. Fix ONE class of issues
6. write_source → smoke
7. veil_check → host_check; if you introduced new err/warn → fix same turn
8. session_commit with message (slice + why)  # host rejects if clean
9. Repeat 5–8 until task complete or blocked
10. run_coding_plan(coding.finish_task)  # or create_pr → submit_pr
11. merge ONLY if operator explicitly asked to land
```

## Tools

| Tool | Role |
|------|------|
| `resolve_coding_target` | Match open unmerged PRs / Present choose / new |
| `run_coding_plan` | Host plans: slice, fix_diagnostics, finish_task |
| `session_status` | Branch, uncommitted, host_check, head_commit |
| `create_branch` | Isolated feature branch; becomes active work line |
| `session_commit` | Named checkpoint (empty tree rejected) |
| `list_commits` | History |
| `create_pr` / `submit_pr` | Open/submit **pull request** (default landing) |
| `merge_branch` / `merge_pr` | Operator gate only — never auto |
| `switch_main` | Return to sticky mainline |

## Forbidden

- "Fix all errors" in one unbounded rewrite without intermediate check + commit
- Treating change-list size as progress / errors fixed
- Claiming success without post-edit `veil_check` counts
- Ending a turn with new diagnostics you introduced (unless hard-blocked and reported)
- Auto-merging when a task finishes (`merge_branch` / `merge_pr` without explicit operator request)
- Asking the operator for every branch/commit decision
- Assuming bang forces Opt→T (obsolete ACS-001)

## APIs

```
POST /api/sessions              { slug, branch_name? }
POST /api/sessions/{id}/commits { message }
GET  /api/sessions/{id}/commits
POST /api/sessions/{id}/merge   # operator gate
POST /api/pull_requests       # create PR
POST /api/pull_requests/{id}/submit
```

**Source of truth:** `runtime/docs/DURABLE_SESSIONS.md` · palace `decision-durable-coding-sessions` · SOP `veil-agent-git-shaped-coding` · UX `veil-sdlc-ux-design`
