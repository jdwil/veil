# veil-contract-git-shaped-sessions

**Type:** Concept  
**Summary:** Agent coding uses git-shaped branch → edit → veil_check → commit → **open PR**. Autosave ≠ commit. Humans merge after review (PR Wizard). Scoreboard is err/warn counts. Agent decides branch/commit; never auto-merges.

## Contract

- **Branch before multi-step product work.** Prefer an isolated work line (`create_branch` / `branch_name`), not silent rewrites on shared main when doing a fix campaign.
- **Autosave is durable S3 write-through** — work is saved on every successful `write_source`. That is **not** a commit.
- **Commit** = named checkpoint with a **message** (`session_commit` or `POST /api/sessions/{id}/commits`). Create after a meaningful slice of progress. Message should name the slice **and why** (rationale for human review).
- **Scoreboard:** `veil_check` **error_count** / **warning_count** after each meaningful edit. Do not claim "fixed" without a post-edit check and lower counts (or explicit block reason).
- **Same-turn diagnostics:** If a post-edit check shows **new** errors/warnings the agent introduced, **fix them on the same turn** before claiming done or opening a PR.
- **One diagnostic class per commit** when fixing large lists.
- **End of agent task = open a PR, not merge.**
  - `create_change` with title + description (per-slice `## Construct` rationales for the PR Wizard)
  - `submit_change` so the operator reviews in the IDE **PR Wizard** (top bar Review / Changes → PR Wizard)
  - Operator walks each structural DiffItem: approve or feedback (send now / queue). History stored as PR comments.
  - **FORBIDDEN:** `merge_branch` / `merge_change` unless the operator **explicitly** says merge/land
- **IDE Changes** = **Uncommitted** (working tree constructs) + **History** (named commits). Not structural IR "vs baseline."
- **Agent decides** branch / commit; **human decides** merge after visual review. Do not require heavy operator guidance for every branch/commit step.
- **Bang law (ACS-010):** `find!` → `Opt<T>`; force with `require` / `.unwrap()`. See `veil-contract-bang-opt-res`.

## Agent loop (mandatory shape)

```text
1. session_status — know branch / uncommitted
2. Multi-step? create_branch (e.g. fix-type-mismatch)
3. veil_check → note error_count / warning_count (baseline)
4. Fix ONE class of issues
5. write_source → smoke
6. veil_check → report new counts; if you introduced new err/warn → fix same turn
7. session_commit with message (slice + why)
8. Repeat 4–7 until task complete or blocked
9. create_change(title, description with rationales) → submit_change
10. merge_branch / merge_change ONLY if operator explicitly asked to land
```

## Tools

| Tool | Role |
|------|------|
| `session_status` | Branch, uncommitted, head_commit |
| `create_branch` | Isolated feature branch; becomes active work line |
| `session_commit` | Named checkpoint (+ rationale in message) |
| `list_commits` | History |
| `create_change` / `submit_change` | Open PR for human review (default landing) |
| `merge_branch` / `merge_change` | Operator gate only — never auto |
| `switch_main` | Return to sticky mainline |

## Forbidden

- "Fix all errors" in one unbounded rewrite without intermediate check + commit
- Treating change-list size as progress / errors fixed
- Claiming success without post-edit `veil_check` counts
- Ending a turn with new diagnostics you introduced (unless hard-blocked and reported)
- Auto-merging when a task finishes (`merge_branch` / `merge_change` without explicit operator request)
- Asking the operator for every branch/commit decision
- Assuming bang forces Opt→T (obsolete ACS-001)

## APIs

```
POST /api/sessions              { slug, branch_name? }
POST /api/sessions/{id}/commits { message }
GET  /api/sessions/{id}/commits
POST /api/sessions/{id}/merge   # operator gate
POST /api/change_requests       # create PR
POST /api/change_requests/{id}/submit
```

**Source of truth:** `runtime/docs/DURABLE_SESSIONS.md` · palace `decision-durable-coding-sessions` · SOP `veil-agent-git-shaped-coding` · UX `veil-sdlc-ux-design`
