# Viewer Restructure & Navigation Stories

Mission: humans restructure topology; canvas mutations must be real.

---

## UX-010: `pkg` files are editable

**Status:** Done · **Priority:** P0  
**As a** human using `veil serve`  
**I want** normal `pkg …` sources to be editable  
**So that** nearly all real VEIL files are not read-only

**Acceptance criteria:**

- Editability is not `!source.starts_with("pkg ")`
- Policy: all application `.veil` files editable unless explicitly marked
  read-only (e.g. generated or lock)
- Layer files may be read-only in MVP or editable — document choice
- Examples and `runtime/src/*.veil` open editable in local serve
- Regression test or assert in server unit test

**Touch:** `crates/veil-cli/src/main.rs` (and/or filesystem provider)

**Done notes:** `is_veil_source_editable` — `.veil` editable; readonly via
`# veil:readonly` or `generated/` path. Layers/stubs not loaded as serve
targets. Unit tests in `veil-cli`.

**Mission impact:** Human restructure loop is broken without this.

---

## UX-011: Multi-file select sets active file

**Status:** Done · **Priority:** P0  
**As a** user with multiple `.veil` files  
**I want** selecting a file to make it the active IR/source/edit target  
**So that** subsequent GETs and edits apply to the right package

**Acceptance criteria:**

- `POST /api/files/select` calls provider `set_active`
- `GET /api/files` returns `{ name, path, editable, active, … }` matching client
- Client `selectFile` refreshes IR, source, diagnostics, palette, generated
- Remove schema drift between `veil-server` and legacy `serve.rs` (prefer one)

**Touch:** `crates/veil-server/src/api.rs`, `provider/filesystem.rs`, `store.ts`

**Done notes:** `SourceProvider::set_active`; FileInfo includes index/active;
client refreshes source/palette/generated/presentation/check.

---

## UX-012: Palette drop persists constructs

**Status:** Done · **Priority:** P1  
**As a** human building structure  
**I want** dropping a palette item to create a real construct in source  
**So that** the graph is not a ephemeral sketch

**Acceptance criteria:**

- Drop → `create_construct` (or equivalent) via `/api/edit`
- Parent/group context respected (allowed_in / layer constraints)
- Failure surfaces diagnostic (e.g. wrong parent shape)
- Undo optional later

**Done notes:** Implemented with LAY-008 (`handleDrop` → `saveEdits` create_construct;
placement via `createPlacement.ts`). Palette filtering still uses `allowed_in` / group.

---

## UX-013: Connect / wiring persistence (MVP policy)

**Status:** Done · **Priority:** P2  
**As a** human  
**I want** clarity on whether edges are editable wiring or derived  
**So that** I do not draw edges that vanish on reload

**Acceptance criteria:**

- Document: edges today are derived from IR (calls/refs/implements)
- Either: disable freeform connect, **or** implement persisted wiring ops
- If freeform remains, mark edges as “local only” visually until saved

**Done notes:** Freeform connect kept for sketching; edges use dashed amber
style + `local only` label and `local-…` ids. Real edges from IR on reload.
Policy documented in `docs/SERVER.md`.

---

## UX-014: Outline / search jump

**Status:** Done · **Priority:** P1  
**As a** reviewer of a large package  
**I want** search and an outline tree  
**So that** I can jump to constructs without pan-zoom archaeology

**Acceptance criteria:**

- Cmd/Ctrl-K or search box: fuzzy find construct by name/subkind
- Optional left outline: modules → groups → constructs
- Jump selects node and drills breadcrumb as needed

**Done notes:** `OutlinePanel.svelte` — Ctrl/Cmd-K opens search; jump uses
`focusDiagnostic` breadcrumb path + selection.

---

## UX-015: Stub palette section

**Status:** Done · **Priority:** P2  
**As a** human writing adapters  
**I want** loaded stubs visible in the palette/browser  
**So that** external APIs are discoverable

**Acceptance criteria:**

- `/api/stubs` data rendered (section exists in CSS historically; wire markup)
- Read-only browse of types/methods
- Does not imply stubs are instantiable constructs unless layer says so

**Related legacy:** UX-006

**Done notes:** Palette “External (stubs)” lists crates, structs, impls with
method counts/tooltips; not draggable.

---

## UX-016: Statement palette section

**Status:** Done · **Priority:** P2  
**As a** human editing a step body  
**I want** layer statements (dispatch, guard, …) listed separately  
**So that** verbs are discoverable without memorizing the layer

**Acceptance criteria:**

- Palette section “Statements” from layer registry
- Insert path: either body template helper or documented “edit in body”
- Icons/labels from layer visual metadata

**Related legacy:** UX-001 partial

**Done notes:** `entry_type === 'statement'` section; hint “edit in body”;
not draggable.

---

## UX-017: Layer-provided constructs in graph

**Status:** Done · **Priority:** P2  
**As a** reviewer  
**I want** declared/layer-provided infrastructure visible but distinct  
**So that** I see Bus and friends without confusing them with user code

**Acceptance criteria:**

- Toggle (exists) works; default hidden or dimmed — document default
- Selecting shows methods/docs from declare
- Not re-serialized into user source (already true — keep tests)

**Related legacy:** UX-002

**Done notes:** Default **hidden** (`showLayerProvided = false`). When shown:
dimmed dashed node + `infra` badge. Properties/methods via existing PE.
Documented in `docs/SERVER.md`.

---

## UX-018: Retire dual serve implementations

**Status:** Done · **Priority:** P2  
**As a** maintainer  
**I want** one server implementation  
**So that** API behavior does not drift

**Acceptance criteria:**

- `veil serve` uses only `veil-server`
- Delete or thin-wrap legacy `veil-cli/src/serve.rs`
- All endpoints documented in one place (short `docs/SERVER.md` or README section)

**Done notes:** CLI already used `veil_server::build_router`; deleted orphan
`serve.rs`. Endpoints + policy in `docs/SERVER.md`.
