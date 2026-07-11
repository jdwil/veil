# Runtime UX audit (2026-07-11)

Live pass of what actually runs today as “the runtime” and the multi-project IDE
path. Gaps become stories below. Related: [120](120-projects-config-init.md),
[`docs/IDE_RUNTIME.md`](../docs/IDE_RUNTIME.md), [`runtime/README.md`](../runtime/README.md).

---

## What was run

| Process | How | Port | Result |
|---------|-----|------|--------|
| **veil-runtime** (bootstrap) | `cargo run --release` in `runtime/bootstrap` | **8080** | Starts; **echo Bus only** |
| **veil serve --multi** | `./target/release/veil serve --multi -p 3001` | **3001** | Hub + `/api/p/{name}/…` work |
| **veil-viewer** | existing Vite on 5173 | **5173** | Loads IDE; multi via `?project=` |
| **runtime-ui.veil** | `veil gen runtime/src/runtime-ui.veil -t typescript` | — | **Parse fails** |

---

## Audit findings

### A. Bootstrap “runtime server” is not a product UX shell

`runtime/bootstrap` → binary `veil-runtime` listens with:

| Route | Behavior |
|-------|----------|
| `GET /health` | `{ status: healthy, service: veil-runtime }` |
| `POST /bus/invoke` | Echo stub: `{ handler, status: ok, received: N }` |
| `POST /bus/request` | Same |
| `POST /bus/dispatch` | Accepted |

Handlers (`ListRepos`, `CreateRepo`, …) are **placeholders** registered by name;
they do **not** call generated storage crates, object store, or git. There is
**no HTML UI**, no projects list, no IDE, no static assets.

**Verdict:** Bus smoke host only — not the runtime UX described in product docs
(projects hub, embedded IDE, dashboard).

### B. VEIL-authored runtime UI does not compile

`runtime/src/runtime-ui.veil` (sidebar, dashboard, project cards, pages) fails
codegen:

```text
parse error: unexpected token StringLit in 'layout' body
parse error: unexpected token StringLit in 'page' body
```

(multiple errors). The intended Svelte runtime shell is **not shippable**.

### C. Multi-project host (`veil serve --multi`) works as API kernel

Verified:

- `GET /api/projects` → projects under configured hub (`projects_dir`)
- `GET /api/config` → `~/.veil/config.json` fields
- `POST /api/projects` `{ "name": "audit-demo" }` → creates git scaffold
- `GET /api/p/hubby/files` → package list
- `GET /api/p/hubby/ir` → IR graph
- `GET /api/ir` without project → **404** (correct for multi mode)
- `GET /api/p/nope/ir` → **500** (should be **404** with clear body — bug)

### D. Viewer UX against multi-serve

Opened `http://127.0.0.1:5173/?project=hubby` with multi on :3001.

**Works**

- Project badge shows `hubby`
- Loads scaffold package source (`pkg Hubby` / `use ddd`)
- Outline, Review dock, Agent pane, palette (Bounded Context, etc.)
- Theme toggle, infrastructure/critical filters present

**Missing / weak for “runtime shell”**

| Gap | Detail |
|-----|--------|
| No project hub UI | No list/create/switch projects inside the viewer |
| `?project=` only | No project picker; raw query param is the only multi switch |
| No runtime chrome | No sidebar (Dashboard / Projects / Deploys) from runtime-ui |
| Empty product | Scaffold has no domain nodes beyond package root — OK for init, poor demo |
| Not embedded | Viewer is separate Vite app; runtime does not serve or embed it |
| Agent dock | Present; not audited end-to-end ACP turn in this pass |

### E. Wiring gaps (design vs code)

| Intended ([IDE_RUNTIME.md](../docs/IDE_RUNTIME.md)) | Actual |
|-----------------------------------------------------|--------|
| Runtime embeds `veil-server` / `build_multi_router` | Bootstrap has **own** axum router; **no** `veil-server` link |
| One process: hub + IDE | Multi is **CLI** `veil serve --multi`, not runtime binary |
| Runtime UI (VEIL/Svelte) | **Does not parse** |
| Real ListRepos / storage | Echo handlers only |

---

## Stories

### RTU — Runtime host UX (product shell)

#### RTU-001: Embed multi-project IDE kernel in veil-runtime — Done

**Mission impact:** One binary for hub + IDE; stop dual-process confusion.

**Acceptance**

- [ ] `veil-runtime` (or generated host) links `veil-server` and mounts
      `build_multi_router(ProjectsHub)` (or equivalent nest).
- [ ] Same routes as `veil serve --multi`: `/api/projects`, `/api/config`,
      `/api/p/{project}/…`.
- [ ] Config first-run / `~/.veil/config.json` used on startup.
- [ ] Bootstrap bus routes either remain under `/bus/…` or migrate to real
      generated handlers without breaking health.
- [ ] Docs: `make runtime` / `cargo run -p veil-runtime` is the local product
      entry; `veil serve --multi` remains CLI convenience.

---

#### RTU-002: Fix `runtime-ui.veil` parse + generate — Done

**Mission impact:** Runtime shell authored in VEIL, not handwritten React forever.

**Acceptance**

- [ ] `veil check` / `veil gen -t typescript` (or svelte target) succeeds on
      `runtime/src/runtime-ui.veil`.
- [ ] Parse errors for layout/page string bodies resolved (language or source).
- [ ] Generated UI can load against multi API (repos/projects from hub).

---

#### RTU-003: Runtime shell pages (dashboard + projects) — Done

**Mission impact:** Users see products without memorizing query params.

**Acceptance**

- [ ] Dashboard: project count, recent projects (from `GET /api/projects`).
- [ ] Projects page: list, create (POST), open IDE for a project.
- [ ] “Open in IDE” navigates to viewer/embed with project scope set
      (path or `?project=`).
- [ ] Empty hub: CTA to create first project + first-run if no config.

---

#### RTU-004: Embed or serve IDE in runtime shell — Done

**Mission impact:** One window, not “start multi + Vite separately.”

**Acceptance**

- [ ] Runtime serves or embeds the IDE (iframe or integrated routes) for
      `/projects/{name}/ide` (or equivalent).
- [ ] IDE API base automatically `/api/p/{name}/…`.
- [ ] Dev mode may still use Vite HMR; production/runtime uses one origin.
- [ ] Document ports: default runtime port (8080) vs legacy 3001/5173.

---

#### RTU-005: Project switcher in IDE chrome — Done

**Mission impact:** Multi-open without editing the URL bar.

**Acceptance**

- [ ] With multi host: dropdown/tabs of hub projects in viewer top bar.
- [ ] Switching project reloads IR/files (same as changing `?project=`).
- [ ] Create project action (optional) calls hub API then opens new project.
- [ ] Single-project `veil serve <path>` hides switcher (or shows one name only).

---

#### RTU-006: Multi-project error hygiene — Done

**Mission impact:** Trustable HTTP for agents and UI.

**Acceptance**

- [ ] Unknown `{project}` → **404** JSON `{ "error": "project not found", "name": "…" }`
      (not 500).
- [ ] Empty/invalid project name → **400**.
- [ ] Session open failures (no packages) → clear **404/422** message + hint
      `veil init`.

---

#### RTU-007: Wire real Bus handlers (not echo) — Done

**Mission impact:** Runtime storage/tools match `runtime.veil` domain model.

**Acceptance**

- [ ] Bootstrap (or generated host) registers **generated** crate handlers for
      at least `ListRepos`, `CreateRepo`, `WriteFile`, `ReadFile`.
- [ ] Local default: filesystem / `VEIL_DATA_DIR` meta+objects (or projects_dir
      mapping) — honest errors if not configured.
- [ ] Echo placeholders removed or gated behind `VEIL_RUNTIME_STUB=1`.
- [ ] Integration test: create repo via bus → appears in list.

---

#### RTU-008: `make runtime-serve` (or equivalent) — Done

**Mission impact:** One documented command to try the product stack.

**Acceptance**

- [ ] Makefile target builds runtime + starts multi kernel (+ optional UI).
- [ ] Prints URLs: health, projects hub, example `?project=`.
- [ ] Uses config projects_dir; creates sample project if hub empty (opt-in flag).

---

### Viewer / multi polish (IDE side)

#### RTU-009: Hub API client + empty multi state — Done

**Acceptance**

- [ ] Viewer can call `GET /api/projects` (hub base, not under `/api/p/…`).
- [ ] If multi host and no `?project=`, show hub picker instead of silent empty
      IR failure.
- [ ] Connection error copy mentions `veil serve --multi` / runtime port.

---

## Severity / priority

| ID | Severity | Band |
|----|----------|------|
| RTU-001 | P0 product architecture | P1 |
| RTU-002 | P0 UI blocked | P1 |
| RTU-003 | P1 shell | P1 |
| RTU-004 | P1 single window | P1 |
| RTU-005 | P1 multi UX | P1 |
| RTU-006 | P1 API honesty | P1 |
| RTU-007 | P1 platform truth | P2 |
| RTU-008 | P2 DX | P2 |
| RTU-009 | P1 multi viewer | P1 |

**Suggested order:** RTU-006 (quick) → RTU-001 → RTU-009 → RTU-005 → RTU-002 →
RTU-003 → RTU-004 → RTU-007 → RTU-008.

---

## What works today (do not regress)

- `veil serve --multi` + `?project=<name>` dual-loop IDE for hub products
- `veil projects` / `veil init` / config first-run ([120](120-projects-config-init.md))
- Bootstrap `GET /health` and bus route shape
- Single-project `veil serve <path>` still valid

---

## Status summary

| ID | Title | Status |
|----|--------|--------|
| RTU-001 | Embed multi kernel in veil-runtime | Done |
| RTU-002 | Fix runtime-ui.veil parse/gen | Done |
| RTU-003 | Dashboard + projects pages | Done |
| RTU-004 | Embed/serve IDE from runtime | Done |
| RTU-005 | Project switcher in IDE | Done |
| RTU-006 | 404 hygiene for missing project | Done |
| RTU-007 | Real Bus handlers | Done |
| RTU-008 | make runtime-serve | Done |
| RTU-009 | Viewer hub empty / multi state | Done |
