# IDE API + runtime: one kernel, multi-project

How the **dev server**, **runtime** (authored in VEIL), and **viewer** share one
API surface without forking processes per product and without duplicating
handlers.

Related: [`PROJECT_LAYOUT.md`](PROJECT_LAYOUT.md), [`SERVER.md`](SERVER.md),
[`STORAGE.md`](STORAGE.md).

---

## Decisions

| Topic | Decision |
|-------|----------|
| Config | `~/.veil/config.json` (override root with `VEIL_DATA_DIR`) |
| Projects dir | `config.projects_dir` (env `VEIL_PROJECTS_DIR` overrides for a session) |
| First launch | Interactive prompt (CLI/runtime); non-interactive → defaults |
| HTTP API | **One implementation**: `veil-server` (`build_router` + providers) |
| Runtime | **VEIL-authored** platform that **embeds / links** `veil-server` — does not reimplement IR/edit/agent |
| Multi-project | **Single process**, request-scoped project id — **not** N× `veil serve` |
| CLI `veil serve <path>` | Thin **single-project** mode of the same kernel (dev convenience) |

---

## Config file

Path: **`$VEIL_DATA_DIR/config.json`** or **`~/.veil/config.json`**.

```json
{
  "version": 1,
  "projects_dir": "/home/you/dev/veil-projects",
  "layers_dir": null,
  "show_core_layers": false,
  "configured": true
}
```

| Field | Meaning |
|-------|---------|
| `projects_dir` | Parent of product git repos |
| `layers_dir` | Optional pin for core platform `.layer` files |
| `show_core_layers` | Language-dev: list core layers in IDE pickers |
| `configured` | First-run completed |

**Precedence for projects directory:**

1. `VEIL_PROJECTS_DIR` (session)
2. `config.json` → `projects_dir`
3. `~/veil-projects`

First-run (when config file missing): ask for projects directory; suggest
`~/dev/veil-projects` if `~/dev` exists, else `~/veil-projects`; create dir + write config.

---

## Anti-duplication: shared kernel

```text
                    ┌─────────────────────────────────┐
                    │  veil-server (Rust library)       │
                    │  build_router · SourceProvider    │
                    │  edit · check · agent · SSE       │
                    │  project_layout · config          │
                    └────────────▲────────────▲────────┘
                                 │            │
              ┌──────────────────┘            └──────────────────┐
              │                                                    │
   ┌──────────┴──────────┐                          ┌─────────────┴────────────┐
   │ veil-cli serve      │                          │ Runtime (VEIL → codegen) │
   │ single- or multi-   │                          │ host process embeds      │
   │ project provider    │                          │ same router / lib        │
   └─────────────────────┘                          └──────────────────────────┘
              │                                                    │
              └────────────────────┬───────────────────────────────┘
                                   ▼
                         ┌─────────────────┐
                         │ veil-viewer     │
                         │ project in URL  │
                         └─────────────────┘
```

**Do not:**

- Copy API handlers into `runtime.veil` as a second implementation.
- Spawn one HTTP server process per open product for normal local use.

**Do:**

- Keep all dual-loop HTTP in `veil-server`.
- Runtime depends on that library (Rust link from generated host / bootstrap), or mounts the same `Router`.
- VEIL runtime package models **products, deploy, storage, UX**; it **calls** the kernel for IDE operations.

---

## Multi-project in one process

### Problem

Today handlers use `State<Arc<P: SourceProvider>>` with an **implicit active file**.
A global “active project” breaks concurrent clients (two browser windows, two products).

### Solution: request-scoped project + session cache

```text
GET  /api/projects
POST /api/projects                  { "name": "billing" }

GET  /api/p/{project}/ir
GET  /api/p/{project}/files
POST /api/p/{project}/edit
… all existing IDE routes nested under /api/p/{project}/ …

GET  /api/project                   optional: last-selected / default for shell
```

**Compatibility:** single-project `veil serve ./foo` keeps un-prefixed `/api/ir` …
(provider locked to one root). Multi mode and runtime use `/api/p/{name}/…`.

### Provider layering (no handler duplication)

```text
ProjectsHub
  projects_dir: PathBuf
  sessions: Mutex<HashMap<String, Arc<FilesystemProvider>>>  // lazy

  fn session(&self, name: &str) -> Arc<FilesystemProvider>
      // load collect_project_files(root), cache

Router:
  nest("/api/p/{project}", ide_routes())
    // middleware: resolve {project} → Arc<dyn SourceProvider> in extensions
    // handlers: extract provider from extensions (same code as today)
```

Refactor shape (incremental):

1. Extract `ide_routes<P: SourceProvider>() -> Router<Arc<P>>` from `build_router`.
2. `build_router(provider)` = single-project (current URLs).
3. `build_multi_router(hub)` = hub routes + nest per-project ide_routes via a
   **type-erased** or **hub-backed** provider that reads project id from request
   extensions set by middleware.

Handlers stay one set; only state extraction changes.

### Viewer

- Runtime shell: project list from `/api/projects`.
- Open product → navigate viewer to `?project=billing` or path `/p/billing`.
- All `fetch('http://localhost:3001/api/…')` become `…/api/p/billing/…`.
- Multiple browser windows can hit different projects on the **same port**.

### Agent / ACP

- Working directory = project root for that request’s project.
- Context pack layers/registry from that project’s `FilesystemProvider`.

---

## Runtime in VEIL

`runtime/src/runtime.veil` owns **platform domain** (repos, artifacts, deploy,
daemon UX). It should **not** re-specify parse/check/edit.

Bridge options (pick when wiring host):

| Approach | Notes |
|----------|--------|
| **A. Generated host links `veil-server`** | Bootstrap/main builds `Router` from hub; VEIL ports call local HTTP or in-process tools | **Preferred** |
| **B. VEIL `external_call` into Rust shims** | Thin `veil_runtime_host` crate exposes `serve_ide()`, `list_projects()` | Good if axum stays outside VEIL |
| **C. Runtime only reverse-proxies to `veil serve`** | Duplicates processes — **rejected** for local multi-project |

First-run + config belong in the **host** (Rust CLI/runtime entry) so VEIL
packages read config via a small port (`GetConfig`, `ListProjects`).

---

## Migration path

| Step | Deliverable |
|------|-------------|
| 1 | `~/.veil/config.json` + first-run + `resolve_projects_dir()` — **this change** |
| 2 | Extract `ide_routes`; document `/api/p/{project}` |
| 3 | `ProjectsHub` + `build_multi_router`; `veil serve --multi` or runtime default |
| 4 | Viewer base URL prefix from `?project=` |
| 5 | Runtime host embeds multi router; remove multi-process spawn story |

CLI single-project mode remains for language monorepo demos (`examples/`).

---

## Env vs config

| Variable | Role |
|----------|------|
| `VEIL_DATA_DIR` | Root for `config.json`, objects, meta (default `~/.veil`) |
| `VEIL_PROJECTS_DIR` | Session override of `projects_dir` |
| `VEIL_LAYERS_DIR` | Session override of core layers path |
| `VEIL_SHOW_CORE_LAYERS` | Session override of `show_core_layers` |
| `VEIL_REMOTE_URL` | IDE as remote client of another kernel (unchanged) |

Prefer **config file for durable prefs**; env for CI and one-off overrides.
