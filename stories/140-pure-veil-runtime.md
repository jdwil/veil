# Pure VEIL runtime — front and back, full product

**Goal (definition of done):** A developer runs **one** runtime binary built
from **VEIL sources** for product domain **and** shell UI. All **intended
platform functionality** works end-to-end on local defaults. Dual-loop IDE for
customer products is available in-process. Handwritten bootstrap is gone or
reduced to a **tiny, boring trampoline** that only exists until VEIL can emit
the host binary itself.

**Status:** Spec only (Todo)  
**Depends on:** [120](120-projects-config-init.md) (Done), [130](130-runtime-ux-audit.md)
partial host, [70](70-runtime-harness.md), [80](80-runtime-platform.md)  
**Docs:** [`IDE_RUNTIME.md`](../docs/IDE_RUNTIME.md), [`HARNESS.md`](../docs/HARNESS.md),
[`PROJECT_LAYOUT.md`](../docs/PROJECT_LAYOUT.md)

---

## Vision vs today’s honest baseline

| Layer | Target (“pure VEIL”) | Today (2026-07) |
|-------|----------------------|-----------------|
| Platform domain (repos, files, compile, deploy, agents) | `runtime.veil` → generated crates, **wired and live** | VEIL sources exist; host mostly hub List/Create + stubs |
| Runtime shell UI | `runtime-ui.veil` → generated Svelte/TS, **served as the product UI** | Checks/gen work; **live shell is static HTML** |
| Dual-loop IDE for products | Same UI embeds IDE; API is multi-project kernel | Works via `veil-server` + separate **veil-viewer** |
| Host process | VEIL `@main` harness (or one-line trampoline) | Handwritten `runtime/bootstrap` |
| IDE engine (parse/check/edit/agent) | **Not** rewritten in VEIL — **Rust `veil-server` linked as platform capability** | Same |

### Residual non-VEIL (explicitly allowed forever)

These are **platform engine**, not “runtime product code.” Calling the runtime
“pure VEIL” means **product + shell + domain services** are VEIL-authored, not
that the compiler/IDE engine is rewritten in VEIL.

| Component | Language | Role |
|-----------|----------|------|
| `veil-parser` / `veil-ir` / `veil-codegen` | Rust | Language engine |
| `veil-server` | Rust | Dual-loop IDE HTTP kernel (multi-project) |
| `veil-local` adapters | Rust (or thin VEIL ports later) | FS/S3/meta ports |
| Optional trampoline `main` | ≤50 lines Rust **or** generated from `@main` | Process entry until full self-host |

**Forbidden forever as “the product”:** growing handwritten dashboard, project
list, storage business logic, or IDE chrome outside VEIL sources.

---

## Definition of done (release gate)

Ship only when **all** of the following pass on a clean machine:

### D0 — Build & run

1. `make pure-runtime` (or documented one-liner) from a clean clone:
   - generates backend from `runtime/src/runtime.veil` (+ deps/layers/stubs)
   - generates frontend from `runtime/src/runtime-ui.veil`
   - builds one runnable host
2. `http://127.0.0.1:$PORT/` serves the **generated** shell (not hand HTML as primary).
3. No separate “start multi on 3001 + Vite” required for basic use (viewer may
   be bundled/served from the same origin).

### D1 — Backend functionality (live, not echo)

| Capability | Acceptance |
|------------|------------|
| List products | UI + `GET /api/projects` + Bus `ListRepos` agree on hub contents |
| Create product | UI + API + Bus `CreateRepo` create git project under `projects_dir` |
| Read/write product files | Bus or package APIs read/write under project root (real FS) |
| Branches / log | At least list default branch + stub commit log **or** honest “git status” from disk |
| Compile | `Compile` runs `veil check`/`veil gen` (or cargo on generated) for a project and returns status |
| Deploy local | Local “deploy” = register artifact path / run harness binary (not AWS-required) |
| Config | Load/save runtime config via `~/.veil/config.json` + shell Config page |
| Health | `/health` reports version, projects_dir, mode |

`VEIL_RUNTIME_STUB=1` may exist for CI **unit** tests of routing only; **default
local and release builds must not use echo stubs** for List/Create/Read/Write.

### D2 — Frontend functionality (generated UI)

| Capability | Acceptance |
|------------|------------|
| Dashboard | Stats (project count, …) from live API |
| Projects | List, create, open IDE |
| IDE embed | Full dual-loop for selected product (`/api/p/{name}/…`) |
| Deploy / Registry / Bus / Agents / Config pages | Present and wired to real or clearly gated endpoints (no dead mock that pretends success) |
| Nav | Sidebar from VEIL UI; routes match pages in `runtime-ui.veil` |

### D3 — “Pure VEIL” authorship gate

1. **No** product UI logic in `runtime/bootstrap/static/*.html` except optional
   redirect stub (`index.html` → generated app).
2. **No** product domain logic in bootstrap beyond optional `@main` glue.
3. `runtime.veil` + `runtime-ui.veil` (+ layers/stubs they `use`) are the **source
   of truth**; regenerate + rebuild is the release path.
4. Code review checklist: “Is this change in `.veil` / `.layer` / `.stub`?”
   Handwritten host changes require an ADR + story.

### D4 — Dual-loop for customer products

1. From shell → Open IDE → graph, edit, check, agent work for that product.
2. Multi-project concurrent: two projects open (two tabs/windows) on **one** host.
3. First-run config still works ([120](120-projects-config-init.md)).

---

## Architecture target

```text
                    ┌─────────────────────────────────────────┐
                    │  Host (generated @main or ≤trampoline)  │
                    │  axum Router merge:                     │
                    │    · platform HTTP (from runtime.veil)  │
                    │    · shell static/SPA (from runtime-ui) │
                    │    · veil_server::build_multi_router    │
                    └──────────────────┬──────────────────────┘
                                       │
          ┌────────────────────────────┼────────────────────────────┐
          ▼                            ▼                            ▼
   runtime.veil                 runtime-ui.veil              veil-server
   Storage/Tools/               app RuntimeUI                (Rust kernel)
   Daemon/Exec                  pages + comps                /api/p/{proj}/…
   Bus handlers                 Svelte/TS emit
```

**Integration rule:** Platform packages **call** IDE kernel only via HTTP or a
thin `provided_by: runtime` port (`IdeKernel`), never by reimplementing IR.

---

## Story map (implement in this order)

Priority: **P1** = pure-runtime critical path · **P2** = depth · **P3** = polish.

### Phase 0 — Contract & inventory

#### PVR-000: Pure-runtime definition of done locked in docs — Todo · P1

**Acceptance**

- [ ] This file is the release gate; `MISSION.md` / `runtime/README.md` point here.
- [ ] Explicit residual non-VEIL list (engine + optional trampoline) published.
- [ ] “Done” demo script listed in D0–D4 (human-runnable).

---

#### PVR-001: Capability matrix (runtime.veil vs live host) — Todo · P1

**Acceptance**

- [ ] Table of every public Bus/command/query in `runtime.veil` with status:
      `live` | `stub` | `missing` | `ui-only`.
- [ ] Matrix checked into `docs/RUNTIME_CAPABILITIES.md` and updated per PR.
- [ ] Gate: no `live` claim without an automated test.

---

### Phase 1 — VEIL backend becomes the real platform

#### PVR-010: Host is generated from VEIL `@main` — Todo · P1

**Acceptance**

- [ ] `runtime.veil` (or `runtime/src/host.veil`) declares `@main` that:
  - loads config
  - builds Bus + registers handlers from **generated** packages
  - mounts multi IDE router (via port / `external` / codegen hook)
  - serves shell assets
  - listens on `VEIL_PORT`
- [ ] `veil gen runtime/...` + `cargo run -p <bin>` is the primary path.
- [ ] Handwritten `bootstrap/src/main.rs` deleted **or** ≤50 lines that only
      `include!` / call generated `run()`.
- [ ] Cross-link RT-021 (bin crate layout) closed if still open.

**Mission impact:** Backend process ownership is VEIL.

---

#### PVR-011: Wire Storage handlers to real impls — Todo · P1

**Acceptance**

- [ ] `ListRepos`, `CreateRepo`, `WriteFile`, `ReadFile`, `ListFiles` invoke
      generated `storage` application code (or equivalent VEIL services).
- [ ] Default local: project roots under `projects_dir` **or** object/meta store
      under `VEIL_DATA_DIR` — documented which model wins.
- [ ] Integration tests (temp dir): create → list → write → read.
- [ ] Failures are structured errors, never silent success.

---

#### PVR-012: Git-backed branch/diff/log — Todo · P1

**Acceptance**

- [ ] `CreateBranch`, `ListBranches`, `GetDiff`, `GetCommitLog` work against
      real git in the product repo (gix or `git` CLI with honest errors).
- [ ] Empty repo / no git → clear error, not panic.

---

#### PVR-013: Compile pipeline for a product — Todo · P1

**Acceptance**

- [ ] `Compile` runs check/gen for a selected project (primary target rust).
- [ ] Returns artifact id/path or diagnostic list compatible with UI.
- [ ] Timeout and cancel policy documented.

---

#### PVR-014: Local deploy / run harness — Todo · P2

**Acceptance**

- [ ] `Deploy` local mode runs or registers the generated binary/harness.
- [ ] Cloud deploy remains adapter-gated (RT-024/025); local path does not
      require AWS.
- [ ] Deployment status queryable from UI.

---

#### PVR-015: Tools context delegates to Storage/Exec — Todo · P1

**Acceptance**

- [ ] `*Tool` handlers are thin facades over Storage/Compile/Deploy (no second
      business logic).
- [ ] Agent tool surface (if exposed) matches tools package.

---

#### PVR-016: Daemon / agent message path — Todo · P2

**Acceptance**

- [ ] `HandleAgentMessage` / `HandleToolCall` call real tools + optional
      veil-server agent turn **or** documented external ACP.
- [ ] Agents page in UI can complete one full turn against a product.

---

#### PVR-017: Remove default stub mode — Todo · P1

**Acceptance**

- [ ] Default binary: no echo handlers for List/Create/Read/Write.
- [ ] `VEIL_RUNTIME_STUB=1` only for unit tests; CI release build asserts stub off.
- [ ] `/health` reports `bus_mode: live | stub`.

---

### Phase 2 — VEIL frontend is the real shell

#### PVR-020: Page/layout raw templates parse for svelte5 — Todo · P1

**Acceptance**

- [ ] `page` / `layout` accept `template """…"""` / `style """…"""` same as `comp`
      (parser/layer fix — root cause of earlier runtime-ui failures).
- [ ] Round-trip test in suite using a minimal page+layout with raw template.
- [ ] `runtime-ui.veil` uses full templates on pages again (not stripped).

---

#### PVR-021: `runtime-ui.veil` is the only product shell source — Todo · P1

**Acceptance**

- [ ] All dashboard/projects/deploy/registry/bus/agents/config screens defined
      only in `runtime-ui.veil` (or VEIL packages it imports).
- [ ] `veil gen -t typescript` (or svelte target) produces a runnable app.
- [ ] Build step copies/bundled output into host static root automatically.

---

#### PVR-022: Shell consumes live multi API (no mock success) — Todo · P1

**Acceptance**

- [ ] Dashboard stats from `GET /api/projects` (and compile/deploy counts when live).
- [ ] Projects create uses `POST /api/projects`.
- [ ] Config page reads/writes `GET/PATCH /api/config` (add PATCH if missing).
- [ ] Bus page invokes `/bus/invoke` with real handlers; shows errors honestly.
- [ ] Agents page performs one live turn or disables with “not configured”.

---

#### PVR-023: Serve generated shell from host origin — Todo · P1

**Acceptance**

- [ ] `GET /` serves generated SPA (or SvelteKit adapter output).
- [ ] Static HTML in bootstrap is redirect-only or deleted.
- [ ] Same origin for shell + `/api/*` (no CORS required for happy path).

---

#### PVR-024: IDE embed is first-class in VEIL UI — Todo · P1

**Acceptance**

- [ ] Route `/projects/{name}/ide` (or page in runtime-ui) embeds dual-loop.
- [ ] Options (pick one, document):
  - **A.** Bundle/serve `veil-viewer` build from host, or
  - **B.** Emit IDE shell pages from VEIL that call multi API (longer-term)
- [ ] MVP gate: **A** is acceptable if viewer is a **built asset** of the
      runtime release, not a second dev server requirement.
- [ ] API base is always same origin `/api/p/{name}/…`.

---

#### PVR-025: Project switcher in shell + IDE — Todo · P1

**Acceptance**

- [ ] Shell and embedded IDE share project list from hub.
- [ ] Switching project updates IDE without full process restart.
- [ ] Create project available from both surfaces.

---

### Phase 3 — One binary, one origin, kill dual-process DX

#### PVR-030: Single-port product experience — Todo · P1

**Acceptance**

- [ ] One port serves: shell SPA + multi IDE API + bus + health.
- [ ] Docs primary path is `make pure-runtime` / `veil-runtime` only.
- [ ] `veil serve --multi` remains supported for engine dev, not product default.

---

#### PVR-031: `make pure-runtime` / release packaging — Todo · P1

**Acceptance**

- [ ] Makefile (or script) runs: gen backend, gen UI, build host, optional
      package viewer assets, print URL.
- [ ] Works offline after first dependency fetch.
- [ ] CI job: pure-runtime build + smoke (health, list projects, open IR).

---

#### PVR-032: Delete or quarantine handwritten shell — Todo · P1

**Acceptance**

- [ ] `runtime/bootstrap/static/index.html` and `ide.html` removed or moved to
      `legacy/` with CI fail if referenced by default binary.
- [ ] Bootstrap main ≤ trampoline or fully generated.

---

### Phase 4 — Full functionality depth (platform complete)

#### PVR-040: Registry (layers/stubs) management UI + API — Todo · P2

**Acceptance**

- [ ] List installed layers/stubs; install from path; show dependents.
- [ ] Align with layer IDE ([110](110-layer-dsl-ide.md)) where overlap exists.

---

#### PVR-041: Artifacts browser — Todo · P2

**Acceptance**

- [ ] List compile artifacts; download/open path; retention policy documented.

---

#### PVR-042: Multi-tenant / remote later (explicit non-goal for pure-local) — Todo · P3

**Acceptance**

- [ ] Spec only until local pure-runtime Done: auth, remote SourceStore
      (AGT-016/018) as follow-on, not blocking D0–D4.

---

## Sequencing (logical implementation order)

```text
PVR-000 · PVR-001          inventory & lock DoD
    ↓
PVR-020                    parser: page/layout raw templates
    ↓
PVR-010                    VEIL @main host
    ↓
PVR-011 · PVR-015 · PVR-017  storage live + tools + no stub default
    ↓
PVR-021 · PVR-022 · PVR-023  generated UI is the shell
    ↓
PVR-024 · PVR-025 · PVR-030  IDE embed + switcher + one port
    ↓
PVR-012 · PVR-013 · PVR-014  git / compile / local deploy
    ↓
PVR-016 · PVR-040 · PVR-041  agents, registry, artifacts
    ↓
PVR-031 · PVR-032            packaging + delete legacy HTML
```

**Parallelizable:** PVR-020 (language) || PVR-011 (storage wiring) once PVR-001 matrix exists.

---

## Test & demo plan

### Automated

| Test | Covers |
|------|--------|
| `veil check runtime/src/runtime.veil` | Backend typecheck |
| `veil check runtime/src/runtime-ui.veil` | Frontend typecheck |
| `veil gen` both packages | Codegen |
| Integration: temp `VEIL_DATA_DIR` + projects | Create/list/write/read |
| HTTP smoke: health, projects, IR for fixture project | Host |
| UI e2e (optional): create project → open IDE → check 0 errors | Shell |

### Human demo script (release)

```bash
make pure-runtime
# browser → /
# create project "demo"
# open IDE → add a ctx/agg → check clean
# Bus page → ListRepos shows demo
# Compile from UI → success or diagnostics
```

---

## Non-goals (for this epic)

- Rewriting `veil-server` / parser in VEIL
- Cloud multi-region HA
- Replacing git with sqlite-as-source-of-truth
- Pixel-perfect design system polish (function first)

---

## Status summary

| ID | Title | Status |
|----|--------|--------|
| PVR-000 | Docs lock pure-runtime DoD | Todo |
| PVR-001 | Capability matrix | Todo |
| PVR-010 | VEIL `@main` host | Todo |
| PVR-011 | Storage handlers live | Todo |
| PVR-012 | Git branch/diff/log | Todo |
| PVR-013 | Compile pipeline | Todo |
| PVR-014 | Local deploy | Todo |
| PVR-015 | Tools facades | Todo |
| PVR-016 | Agent message path | Todo |
| PVR-017 | No default stub bus | Todo |
| PVR-020 | page/layout raw templates | Todo |
| PVR-021 | runtime-ui sole shell source | Todo |
| PVR-022 | Shell on live APIs | Todo |
| PVR-023 | Serve generated shell | Todo |
| PVR-024 | IDE embed first-class | Todo |
| PVR-025 | Project switcher | Todo |
| PVR-030 | Single-port product | Todo |
| PVR-031 | make pure-runtime + CI | Todo |
| PVR-032 | Delete handwritten shell | Todo |
| PVR-040 | Registry UI/API | Todo |
| PVR-041 | Artifacts browser | Todo |
| PVR-042 | Remote multi-tenant (later) | Todo |

---

## Relation to existing RTU / RT stories

| Existing | Role after this epic |
|----------|----------------------|
| RTU-001–009 | Scaffold / audit fixes; **superseded** when PVR DoD met |
| RT-000–023 | Harness foundations; PVR-010 closes remaining host gap |
| RT-010–015 | Adapters remain; PVR-011+ consumes them |
| MP / 120 | Multi-project kernel stays; product host **is** the multi server |

When pure-runtime ships, mark RTU epic **Superseded** with pointer here and keep
PVR statuses as the live backlog.
