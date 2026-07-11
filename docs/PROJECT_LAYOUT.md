# Project layout & serve modes

How VEIL discovers packages, layers, and stubs — and how the **runtime-embedded
IDE** opens multiple products without treating the language monorepo or
`examples/` as the product workspace.

Related: [`VCS_MODEL.md`](VCS_MODEL.md), [`STORAGE.md`](STORAGE.md),
[`SERVER.md`](SERVER.md), [`runtime/README.md`](../runtime/README.md).

---

## Decisions (locked)

| Decision | Choice |
|----------|--------|
| **Source of truth** | On-disk files in a **project root** (git repo) |
| **Not default** | Full source trees in SQLite / object store |
| **Core platform layers** | Toolchain / `VEIL_LAYERS_DIR` — not user project files |
| **User / family layers** | Project-local `layers/` only |
| **`examples/`** | Syntax demos + CI only — **not** the IDE default workspace |
| **Runtime local** | Configured **projects directory**; each product is an **independent git repo** |
| **Config** | `~/.veil/config.json` (`projects_dir`, …); first-run prompt; env overrides |
| **IDE + runtime API** | **One kernel** (`veil-server`); runtime (VEIL) embeds it — see [`IDE_RUNTIME.md`](IDE_RUNTIME.md) |
| **Multi-project** | **Single server process**, request-scoped `/api/p/{project}/…` (not N× processes) |

---

## One project root (single product)

Layout for an application or library the user owns:

```text
my-app/                    # git repository root
  veil.toml                # optional: name, members, path overrides
  *.veil                   # packages (flat at root is fine to start)
  layers/                  # this project’s layers only (family / client DSL)
  stubs/                   # external crate stubs for this project
  generated/               # codegen output (IDE readonly)
```

- `use ddd` / `use rust` resolve from the **installed** core layers, not from
  copies inside the project.
- File picker lists **packages + project layers** (+ stubs as non-editable).
- Core layers are editable only in **language** mode (VEIL monorepo / core devs).

CLI (target shape):

```bash
cd my-app && veil serve .          # mode: project
# or: veil serve --project .
```

---

## Runtime local: projects directory

When the **platform runtime** runs locally, it is configured with a single
**projects directory** (workspace of products), not a flat dump of every
`.veil` in the monorepo.

```text
~/veil-projects/                 # configured projects root (env / settings)
  onboarding/                    # independent git repo
    *.veil
    layers/
    stubs/
  billing/                       # independent git repo
    …
  dlx_core/                      # independent git repo
    …
```

| Setting | Meaning |
|---------|---------|
| Projects directory | `config.projects_dir` or `VEIL_PROJECTS_DIR` (default `~/veil-projects`) |
| New project (UX/CLI) | Create subdirectory + **`git init`** + scaffold (`veil projects create`) |
| Open project | Viewer/runtime selects project on the **shared** multi-project server |
| Multi-open | Concurrent requests with different `{project}` ids on **one** port |

### Why independent git repos

- Clear ownership and CI per product.
- Clone / fork / PR workflows stay normal.
- No “mixed multiproduct soup” in one working tree unless the user chooses a monorepo workspace later.
- Runtime can list repos under the projects directory without parsing a giant composite tree.

### Runtime + IDE (one process)

```text
┌─ Runtime UX (VEIL) ──── embeds veil-server ──────────────┐
│  config: ~/.veil/config.json  projects_dir=…             │
│  [onboarding] [billing] [dlx_core]  [+ New]              │
│       │                                                  │
│       └─ Open IDE view ──► same host :port               │
│            /api/p/billing/ir  (request-scoped project)   │
└──────────────────────────────────────────────────────────┘
```

- CLI single-project `veil serve <path>` remains for demos/dev convenience.
- Product path: **runtime host** runs multi-project kernel (see `IDE_RUNTIME.md`).
- CLI: `veil projects list|create|dir|path`

---

## Serve / load modes

| Mode | Who | File list | Notes |
|------|-----|-----------|--------|
| **`project`** | App team | Packages + `layers/` + `stubs/` under one project root | `veil serve <path>` / `make serve PROJECT=` |
| **`projects` hub** | Runtime / CLI | **Index** of git repos under `VEIL_PROJECTS_DIR` | `veil projects list` — not multi-tab IDE |
| **`workspace`** | Optional monorepo | Members from `veil.toml` `[workspace]` | Still FS + git; later |
| **`language`** | VEIL core devs | Workspace `layers/` editable; optional test packages | Core platform DSL |
| **`runtime`** | Platform packages | e.g. `runtime/src/*.veil` as the platform’s own project | Separate from user products |
| **`remote`** | Distributed IDE | Proxied packages (`VEIL_REMOTE_URL`) | Existing remote provider |
| **`demo`** | Docs / CI | `examples/` | Never product default |

`make serve` for **language development** may use `demo` or a dedicated
playground; product and runtime docs should not treat `examples/` as home.

---

## Core vs userland layers

| Kind | Location | In file picker |
|------|----------|----------------|
| Core platform (`ddd`, `base`, `di`, `rust`, …) | Install / monorepo `layers/` / `VEIL_LAYERS_DIR` | **No** (unless language mode / `--show-core-layers`) |
| Family / client (`wear_test`, `crm`, …) | **Project** `layers/` | **Yes** |
| Stubs | Project `stubs/` (or package-adjacent) | Browse / palette only |

Registry resolution for `use <name>` always walks: project → install layers →
ancestors; editing is a separate concern from resolution.

---

## What we are not doing (default)

- Serving the entire VEIL monorepo as one IDE workspace.
- Storing live source trees primarily in SQLite or S3.
- Auto-creating multiple products as folders *without* git (UX always creates a repo).
- Flattening all open projects into one file selector (tabs isolate context).

Platform object store + meta DB remain for **artifacts, deploy, multi-tenant
runtime** — see [`STORAGE.md`](STORAGE.md) and [`VCS_MODEL.md`](VCS_MODEL.md).

---

## Implementation status

1. Document modes — done.
2. Strict project scan (`collect_project_files`) — no monorepo layers in file list.
3. `veil projects {dir,list,create,path}` + `VEIL_PROJECTS_DIR`.
4. `make serve PROJECT=…` / `make serve-examples` / `make projects`.
5. API: `GET /api/project`, `GET|POST /api/projects` (hub; runtime UI later).
6. Viewer: project name badge (single session).

---

## Env / config

| Variable / key | Purpose |
|----------------|---------|
| `~/.veil/config.json` | Durable prefs (`projects_dir`, `layers_dir`, …) |
| `VEIL_DATA_DIR` | Root for config + local storage (default `~/.veil`) |
| `VEIL_PROJECTS_DIR` | Session override of `projects_dir` |
| `VEIL_LAYERS_DIR` | Core platform layers (install path) |
| `VEIL_SHOW_CORE_LAYERS` | Language-dev: list core layers in the editor |
| Project `veil.toml` | Name, paths, optional workspace members |
