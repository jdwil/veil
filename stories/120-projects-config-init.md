# Projects config, first-run, and `veil init`

Mission: a developer (or runtime host) can go from **zero machine state** to a
**working projects hub + product package** without reading tribal Makefile lore.
Durable prefs live under `~/.veil/`; each product is a git repo under a
configured projects directory. The IDE/runtime dual-loop uses one
`veil-server` kernel ([`docs/IDE_RUNTIME.md`](../docs/IDE_RUNTIME.md),
[`docs/PROJECT_LAYOUT.md`](../docs/PROJECT_LAYOUT.md)).

**Personas**

| Persona | Need |
|---------|------|
| New VEIL user | First command asks where projects live; dirs appear automatically |
| Product author | `veil init` (or serve) scaffolds a full product tree |
| Runtime (VEIL) | Same config + hub APIs; no second config format |
| CI / non-TTY | Sensible defaults; no hung stdin |

**Product principles**

> **No silent empty state.** Missing `~/.veil/config.json` → first-run setup
> (prompt or non-interactive default), then **create** config + projects dir.

> **`veil init` is the explicit scaffold.** Serve/runtime may **ensure** missing
> directories exist when paths are already known, but must not invent a new
> product name without the user (or an explicit create API).

> **One kernel.** Config and project layout live in `veil-server` (or shared
> lib used by CLI); runtime embeds them — do not reimplement in VEIL source.

---

## Related docs

| Doc | Role |
|-----|------|
| [`PROJECT_LAYOUT.md`](../docs/PROJECT_LAYOUT.md) | Project root shape, modes |
| [`IDE_RUNTIME.md`](../docs/IDE_RUNTIME.md) | Multi-project single process, shared API |
| [`SERVER.md`](../docs/SERVER.md) | HTTP surface |
| [`STORAGE.md`](../docs/STORAGE.md) | `VEIL_DATA_DIR` / `~/.veil` for objects |

**Groundwork (partial — not acceptance):** `veil-server` has a config module and
`veil projects create`; first-run and `veil init` must meet the stories below
end-to-end (TTY, non-TTY, serve ensure, docs).

---

## CFG — User config & first-run

### CFG-001: Config path and schema — Done

**Mission impact:** Durable prefs without env soup.

**Acceptance**

- [ ] Canonical path: `$VEIL_DATA_DIR/config.json` if `VEIL_DATA_DIR` set, else
      `~/.veil/config.json`.
- [ ] Schema (v1) at minimum:

```json
{
  "version": 1,
  "projects_dir": "/absolute/or/~/path",
  "layers_dir": null,
  "show_core_layers": false,
  "configured": true
}
```

- [ ] `projects_dir` supports `~/…` expansion on load.
- [ ] Load/save are pure library functions (CLI and runtime host share them).
- [ ] Invalid JSON → clear error (do not overwrite without asking).

---

### CFG-002: First-run when config is missing — Done

**Mission impact:** First `veil` command on a new machine is guided.

**Acceptance**

- [ ] **Any** of these when `config.json` is absent triggers first-run:
  - `veil projects …`
  - `veil serve …`
  - `veil init …` (if it needs projects hub context)
  - Runtime host start (same library entrypoint)
- [ ] **Interactive (TTY, not CI):**
  1. Print short explanation of “projects directory.”
  2. Prompt: `Projects directory [default]: `
  3. Default suggestion:
     - `~/dev/veil-projects` if `~/dev` exists
     - else `~/veil-projects`
  4. Empty input → accept default.
  5. Expand `~/`, create **projects directory** if missing
     (`create_dir_all`).
  6. Write `config.json` with `configured: true`.
  7. Print where config was written.
- [ ] **Non-interactive** (`CI=1`, no TTY, or `--yes` / `--non-interactive`):
  - Write defaults without prompting (same default path rules).
  - Log one line: config path + projects_dir.
- [ ] **Idempotent:** if config already exists, never re-prompt.
- [ ] After first-run, `veil projects dir` prints the configured absolute path.

---

### CFG-003: Ensure directories after config resolve — Done

**Mission impact:** Broken paths are fixed, not fatal-by-default.

**Acceptance**

- [ ] After resolving `projects_dir` (env override or config), if the directory
      does not exist → **create it** (`create_dir_all`) and log once.
- [ ] If `~/.veil` (or `VEIL_DATA_DIR`) is missing when saving config → create it.
- [ ] Env `VEIL_PROJECTS_DIR` overrides config for the process but does **not**
      rewrite `config.json` unless user runs an explicit “save” / reconfigure
      command (see CFG-005).
- [ ] Creating projects_dir never deletes existing contents.

---

### CFG-004: `GET /api/config` — Done

**Mission impact:** Viewer/runtime UX can show hub path without shell.

**Acceptance**

- [ ] Public JSON: `projects_dir` (absolute), `config_path`, `veil_home`,
      `show_core_layers`, `configured`, `version`.
- [ ] No secrets in response.
- [ ] Documented in `docs/SERVER.md`.

---

### CFG-005: Reconfigure projects dir (optional polish) — Done

**Mission impact:** Change hub without hand-editing JSON.

**Acceptance**

- [ ] `veil projects dir --set <path>` **or** `veil config set projects_dir <path>`:
  - Expand path, create dir if missing, update config, print confirmation.
- [ ] Does not move existing product repos (document: user moves trees manually).

---

## INIT — Product project scaffold

### INIT-001: `veil init` command — Done

**Mission impact:** One command creates a product-ready tree.

**Acceptance**

```text
veil init [PATH] [options]
```

| Arg / flag | Meaning |
|------------|---------|
| `PATH` | Directory to initialize (default: `.`) |
| `--name <name>` | Product name (default: directory basename, validated) |
| `--in-hub` | Create under configured `projects_dir/<name>` (ignore PATH or use as name) |
| `--git` / `--no-git` | Default: run `git init` when git available |
| `--force` | Allow non-empty directory only if safe (or refuse without force) |

**Scaffold layout (minimum):**

```text
<project>/
  veil.toml              # name = "…"
  <name>.veil            # minimal pkg (use ddd optional / documented)
  layers/                # empty (project layers)
  stubs/                 # empty
  .gitignore             # generated/, target/, .veil-dev/, OS junk
```

- [ ] Name validation: `[a-zA-Z0-9_-]+` (same as `projects create`).
- [ ] Refuse to clobber existing `*.veil` / `veil.toml` without `--force`.
- [ ] `git init` on success when git exists; warn and continue if not.
- [ ] Print next steps: `veil serve <path>`, `veil check <path>/*.veil`.
- [ ] Library function `init_project(path, opts)` used by CLI (and later API).

---

### INIT-002: Align `veil projects create` with `veil init` — Done

**Mission impact:** Hub create and init are one scaffold, two entrypoints.

**Acceptance**

- [ ] `veil projects create <name>` ≡ `veil init --in-hub --name <name>`
      (or shared implementation).
- [ ] Same files, same git behavior, same validation.
- [ ] `projects create` always under configured projects_dir.
- [ ] Docs / help text cross-link the two commands.

---

### INIT-003: Serve ensures project shape (auto, non-destructive) — Done

**Mission impact:** Opening a half-ready directory still works.

**When:** `veil serve <dir>` and `<dir>` is a directory.

**Acceptance**

- [ ] If config missing → CFG-002 first (before serve bind).
- [ ] If `projects_dir` missing → CFG-003 create.
- [ ] If serve root is **missing** → error with hint to `veil init` / `projects create`
      (do **not** invent a project name from thin air).
- [ ] If serve root **exists** but lacks `layers/` or `stubs/` → create empty
      dirs (log once).
- [ ] If serve root has **no** `*.veil` and no `veil.toml`:
  - Interactive: offer “Run scaffold here? [y/N]” or print `veil init .`
  - Non-interactive: exit non-zero with clear message (do not auto-write a
    package without consent).
- [ ] Never pull monorepo `layers/` into the editable file list (existing
      `collect_project_files` rules).

---

### INIT-004: `.gitignore` and readonly generated — Done

**Mission impact:** Codegen output stays out of VCS by default.

**Acceptance**

- [ ] `veil init` writes a `.gitignore` including at least:
      `generated/`, `target/`, `.veil-dev/`, `.DS_Store`.
- [ ] Existing editability rules: paths under `generated/` remain non-editable
      in IDE ([UX-010](40-viewer-restructure.md) spirit).

---

## HUB — Projects directory CLI (complete gaps)

### HUB-001: `veil projects` surface — Done

**Mission impact:** Runtime and humans share one hub CLI.

| Subcommand | Behavior | Status target |
|------------|----------|---------------|
| `projects dir` | Print absolute projects_dir (+ config path on stderr or `--verbose`) | Keep + CFG-003 |
| `projects list` | List products (name, path, package count, git) | Keep |
| `projects create <name>` | INIT-002 scaffold under hub | Align with init |
| `projects path <name>` | Print absolute path or error | Keep |
| `projects dir --set` | CFG-005 | New |

- [ ] First-run (CFG-002) runs before list/create when config missing.
- [ ] Help text mentions `~/.veil/config.json` and `veil init`.

---

### HUB-002: HTTP hub APIs stay in sync — Done

**Mission impact:** Runtime UX does not shell out for create/list only.

**Acceptance**

- [ ] `GET /api/projects` — list + `projects_dir` + `config_path`.
- [ ] `POST /api/projects` `{ "name" }` — same scaffold as `projects create`.
- [ ] `GET /api/config` — CFG-004.
- [ ] Create ensures projects_dir exists (CFG-003).
- [ ] Documented in `SERVER.md`.

---

## MP — Multi-project single server (follow-on; design locked)

Design: [`IDE_RUNTIME.md`](../docs/IDE_RUNTIME.md). Implement after CFG/INIT so
hub paths are real.

### MP-001: Extract shared IDE routes — Todo

- [ ] `ide_routes()` from `build_router` with no duplicated handlers.
- [ ] Single-project `veil serve <path>` keeps un-prefixed `/api/…`.

### MP-002: `ProjectsHub` + `/api/p/{project}/…` — Todo

- [ ] Lazy per-project `FilesystemProvider` sessions.
- [ ] Request-scoped project id (path segment).
- [ ] Concurrent different projects on one port.

### MP-003: Viewer project prefix — Todo

- [ ] `?project=` or path drives `/api/p/{project}/…`.
- [ ] No multi-process spawn required for multi-open.

### MP-004: Runtime embeds kernel — Todo

- [ ] VEIL runtime host links `veil-server` (or thin host crate).
- [ ] No reimplementation of edit/check/agent in `.veil`.

---

## Sequencing

| Order | Stories | Why |
|-------|---------|-----|
| 1 | **CFG-001–003** | Config + prompt + auto-create dirs |
| 2 | **INIT-001–002** | `veil init` + align `projects create` |
| 3 | **INIT-003–004**, **HUB-001–002**, **CFG-004–005** | Serve ensure, gitignore, API parity |
| 4 | **MP-001–004** | Multi-project one process (runtime product path) |

**P1** for CFG + INIT (daily driver onboarding).  
**P2** for MP (platform multi-open).

---

## Out of scope

- Moving/renaming product repos on disk when `projects_dir` changes.
- SQLite as source of truth for packages.
- Auto-cloning remote git URLs in `init` (later: `veil init --from`).
- Full runtime-ui chrome (depends on MP + runtime host).

---

## Acceptance demo (human)

```bash
rm -rf /tmp/veil-home-test && VEIL_DATA_DIR=/tmp/veil-home-test
# Interactive or CI:
veil projects list
# → config created, projects_dir exists, empty list

veil projects create demo-app
# → $projects_dir/demo-app with veil.toml, demo-app.veil, layers/, stubs/, git

cd /tmp && veil init my-scratch --name my_scratch
# → ./my-scratch scaffolded

veil serve "$(veil projects path demo-app)" -p 3001
# → IDE for that product; GET /api/config shows projects_dir
```

---

## Status summary

| ID | Title | Status |
|----|--------|--------|
| CFG-001 | Config path and schema | Done |
| CFG-002 | First-run prompt + write config | Done |
| CFG-003 | Auto-create projects_dir / .veil | Done |
| CFG-004 | GET /api/config | Done |
| CFG-005 | Reconfigure projects_dir | Done |
| INIT-001 | `veil init` | Done |
| INIT-002 | Align `projects create` | Done |
| INIT-003 | Serve ensures dirs / empty-root UX | Done |
| INIT-004 | .gitignore in scaffold | Done |
| HUB-001 | projects CLI complete | Done |
| HUB-002 | HTTP hub parity | Done |
| MP-001–004 | Multi-project kernel | Todo (P2) |
