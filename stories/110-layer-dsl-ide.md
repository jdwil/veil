# Layer & Team-DSL IDE (Language Designer Loop)

Mission: layers are **shippable team languages** (product-family DSLs). Designers
iterate many `.layer` files with the **same dual-loop product surface** as
packages — open, graph/outline, source, diagnostics, restructure, agent, live
reload — so language work is not second-class to application packages.

**Domain framing (DashLX / product stack)**

| Ring | Artifact | Example |
|------|----------|---------|
| Platform capability | `.veil` package | `dlx_core` (IAAA), providers, signals |
| Product-family language | `.layer` | `wear_test`, `loyalty` — vocabulary + rails |
| Client product program | `.veil` package | `brooks_wear_spring`, initiative-specific logic |
| Soft knobs | data | dates, segment ids, copy, thresholds |

Layers are **not** a configurable workflow engine and **not** where client
if/else lives. They define **words, structure, presentation, and agent teaching**
so product teams assemble **programs** on top of platform packages.

**Personas**

| Persona | Primary files | Needs |
|---------|---------------|--------|
| Language designer | many `.layer` (+ reference packages) | Full IDE loop; high iteration |
| Product implementer | `.veil` using team layers | Constrained palette (downstream of this epic) |
| Agent | both | Tools for package *and* layer with allowlist |

**Product principle**

> Treat `.layer` as a first-class project file kind. Prefer **one IDE shell**
> with kind-aware adapters (IR builder, check, edit ops, chrome) over a forever
> “text-only sidecar.” Divergence is allowed where formats differ; capability
> parity is the goal.

**Related**

- Presentation grammar: [35-layer-presentation.md](35-layer-presentation.md)
- Package multi-file / editability: [40-viewer-restructure.md](40-viewer-restructure.md) (UX-010 deferred layers)
- Human review chrome: [30-viewer-review.md](30-viewer-review.md)
- Agent tools: [100-ide-agent.md](100-ide-agent.md)
- Layer format: [docs/LANGUAGE.md](../docs/LANGUAGE.md) §9

**Non-goals (this epic)**

- Replacing IAAA / signals with layer config documents
- Multi-target debt spam by default (primary target only stays for packages)
- Forcing team members to edit layers day-to-day
- Perfect semantic IR for layers identical to package IR (adapters OK)

**Parity matrix (target state)**

| Capability | Packages (`.veil`) | Layers (`.layer`) | Notes |
|------------|--------------------|-------------------|--------|
| List in serve / file switcher | yes | **DSL-001** | `kind` on FileInfo |
| Active file + source GET/POST | yes | **DSL-002** | |
| Diagnostics / check | dual-loop | **DSL-003** layer validate | |
| Live refresh (SSE) | yes | **DSL-004** + registry reload | |
| Outline / topology | IR graph | **DSL-005** layer IR or outline graph | |
| Source review pane | UX-020 | same dock | |
| Structured create/edit/delete | EditOp / palette | **DSL-006–008** | format-preserving |
| Property editor | yes | **DSL-007** | construct fields |
| Presentation / prompts | via registry | **DSL-009** edit in place | |
| Diff vs baseline | UX-021 | **DSL-010** | |
| Agent tools | rename/check/… | **DSL-011** | |
| Multi-file project | directory serve | layers/ + packages | |
| Team-constrained palette | show infra toggle | **DSL-012** consumer mode | later |
| Scaffold new file | informal | **DSL-013** | |

---

## DSL-001: Serve loads layers as first-class files

**Status:** Done · **Priority:** P0  
**As a** language designer  
**I want** `veil serve` to list `.layer` files next to packages  
**So that** I can open any DSL without leaving the stack

**Acceptance criteria:**

- Directory serve includes `**/*.layer` (at least `layers/` + configurable roots)
- `GET /api/files` entries include `kind: "package" | "layer" | "stub"` (stub optional MVP)
- File switcher groups or labels by kind; selection sets active file
- Opening a layer does not 500 the IR/check endpoints (kind-aware handlers)
- Docs: discovery rules for `make serve` / project layout

**Touch:** `veil-cli` serve collection, `FileInfo`, `FilesystemProvider`, viewer selector

**Mission impact:** Invisible layers cannot be iterated in the daily driver.

---

## DSL-002: Layer source edit + persist (same path as packages)

**Status:** Done · **Priority:** P0  
**As a** language designer  
**I want** to edit and save layer text through the same source API as packages  
**So that** the write path is one mental model

**Acceptance criteria:**

- Active layer: `GET/POST /api/source` works
- Editable by default; honor `# veil:readonly` and path policy
- Replace UX-010 policy “non-`.veil` ⇒ read-only” with kind-aware rules + tests
- Writes via SourceProvider → disk; revision bus / SSE fires
- No package AST serialize round-trip for layers

**Touch:** editability helper, provider, viewer source dock

**Mission impact:** Text save is the minimum full-loop write.

---

## DSL-003: Layer check / diagnostics pipeline

**Status:** Done · **Priority:** P0  
**As a** language designer  
**I want** check diagnostics for layers  
**So that** the machine loop works while I iterate vocabulary

**Acceptance criteria:**

- CLI: `veil check path.layer` (auto-detect kind) or `veil check-layer`
- API: `/api/check` on active layer returns structured diagnostics
- Covers: parse errors, unknown `mt` chains, bad `in`/`has`, presentation block errors, dependency `use` resolution where applicable
- Viewer diagnostics badge + list work in layer mode (reuse UX-023 patterns)
- Failed check does not apply broken registry to consumers (see DSL-004)

**Touch:** layer parse error mapping, check routing by kind, DiagnosticsPanel

**Mission impact:** Dual-loop without check is just a text editor.

---

## DSL-004: Hot reload registry after layer write

**Status:** Done · **Priority:** P0  
**As a** designer with packages open that `use` my layer  
**I want** palette, presentation, and agent context to refresh on save  
**So that** I never restart serve to see construct changes

**Acceptance criteria:**

- Successful layer write reloads that layer into registries for dependents in the serve set
- SSE revision triggers client soft refresh (palette, presentation, context; package IR if needed)
- Last-good registry retained if new layer text fails parse/validate; errors surface on layer file
- Document dependent refresh scope (active package only vs all loaded packages)

**Touch:** provider reload, LayerRegistry load paths, viewer SSE

**Mission impact:** Many layers × high iteration requires zero-restart feedback.

---

## DSL-005: Layer topology / outline canvas (parity with package navigation)

**Status:** Done · **Priority:** P0  
**As a** language designer  
**I want** a navigable structure view of a layer (constructs, statements, groups, present)  
**So that** large DSLs are reviewable like package topology

**Acceptance criteria:**

- Selecting a layer builds a **layer graph or outline IR** (nodes for constructs/statements/sections)
- Canvas or outline panel: click → selection → source span focus (same as package UX-020 behavior)
- Groups (`group domain`, etc.) visible as structure
- Empty/broken parse: show errors, partial outline if possible
- Not required: package-style sequence edges between “steps” unless natural

**Touch:** layer→IR builder (new or extend), viewer computeView branch on kind

**Mission impact:** “Same capabilities” includes structural review, not only text.

---

## DSL-006: Palette create / delete constructs on layers

**Status:** Done · **Priority:** P1  
**As a** language designer  
**I want** to add and remove constructs from the palette or outline actions  
**So that** growing a DSL matches growing a package structure

**Acceptance criteria:**

- Create construct (name, kw, mt defaults) appends a valid block; format-preserving
- Delete construct with confirm; updates source + outline
- Undo optional later; failures return diagnostics
- Agent-equivalent ops exist or share server helpers (DSL-011)

**Touch:** edit API for layer ops, palette mode when `kind=layer`

**Mission impact:** Restructure loop for languages.

---

## DSL-007: Property editor for layer constructs

**Status:** Done · **Priority:** P1  
**As a** language designer  
**I want** to edit construct metadata in the property panel  
**So that** iteration matches package property editing muscle memory

**Acceptance criteria:**

- Selection of a layer construct opens property editor: `kw`, `mt`, `in`, `desc`, `group`, `dg`, visual (icon/color/label), annotation defs, contains/has lines (MVP subset OK if complete path exists for text)
- Saves patch source without rewriting unrelated file regions
- Invalid values blocked with diagnostics
- Shared chrome with package PropertyEditor where practical; kind-specific fields OK

**Touch:** PropertyEditor branch, layer patch helpers

**Mission impact:** High-volume field tweaks without pure text.

---

## DSL-008: Structured layer EditOp (or equivalent) honesty

**Status:** Done · **Priority:** P1  
**As a** platform  
**I want** structured edits for layers with the same integrity bar as package EditOp  
**So that** IDE and agent do not corrupt layer files

**Acceptance criteria:**

- Documented op set: e.g. rename construct, set field, set visual, set contains, set ann, replace prompt section, replace present block
- Server applies ops → validate → write; reject on validation failure (configurable)
- Format-preserving patches preferred over full pretty-print
- Round-trip tests on real layers (`ddd.layer`, `ui.layer`, a product-family sample)

**Touch:** new module or extend edit pipeline, tests

**Mission impact:** SER/edit honesty for the second file kind.

---

## DSL-009: Edit presentation + layer prompts in IDE

**Status:** Done · **Priority:** P1  
**As a** language designer  
**I want** to edit `present` / views and prompt sections in the designer  
**So that** team IDE layout and agent teaching ship with the DSL

**Acceptance criteria:**

- Navigate presentation and prompt sections from outline
- Edit in source focus and/or structured form
- After save + reload: `GET /api/presentation` and agent Tier-1 prompts reflect changes
- Ties to LAY-* presentation model (no hardcoded paradigms)

**Touch:** designer UI, presentation parse already in registry

**Mission impact:** “Simpler IDE for teams” is presentation + prompts, not keywords alone.

---

## DSL-010: Diff for layer files

**Status:** Done · **Priority:** P1  
**As a** language designer  
**I want** structural or text diff vs git baseline for layers  
**So that** review of vocabulary changes matches package review

**Acceptance criteria:**

- Diff panel works when active file is a layer (text diff MVP acceptable; structural construct-level better)
- Same entry points as package diff (UX-021)

**Touch:** DiffPanel, baseline_source for layer paths

**Mission impact:** Human review of language changes.

---

## DSL-011: Agent tools for layers (parity with package tools)

**Status:** Done · **Priority:** P1  
**As an** agent  
**I want** layer list/read/check/edit tools  
**So that** DSL generation is tool-driven like package edits

**Acceptance criteria:**

| Tool | Behavior |
|------|----------|
| `list_files` / kind filter | includes layers |
| `read_source` | works for active layer |
| `veil_check` | kind-aware |
| `layer_outline` | constructs/keywords/groups |
| `rename_construct` / layer structured ops | format-preserving where applicable |
| `write`/`patch` layer | validate + reload |

- Allowlist includes layer paths (AGT-013)
- `/api/agent/tools` documents layer tools
- ACP disk edits picked up via `reload_from_disk` + registry refresh

**Touch:** rig_tools, agent docs, safety allowlist

**Mission impact:** Agent-first applies to language design.

---

## DSL-012: Team consumer mode (package side, driven by layers)

**Status:** Done · **Priority:** P2  
**As a** product implementer  
**I want** the package IDE constrained by the team DSL  
**So that** layers deliver a simpler IDE without editing layers myself

**Acceptance criteria:**

- When a package `use`s product-family layers, palette prioritizes that vocabulary
- Infrastructure / foreign constructs hidden or behind existing toggles by default
- Docs: “Shipping a team DSL” — layer version, reference package, presentation, prompts, platform `use`s
- Optional: project marker or serve hint for “primary DSL layer”

**Touch:** palette filtering, docs, example reference package layout

**Mission impact:** Closes the loop from designer → team without requiring layer editing for users.

---

## DSL-013: Scaffold new layer (+ optional reference package)

**Status:** Done · **Priority:** P2  
**As a** language designer  
**I want** to create a new DSL from a template  
**So that** greenfield family languages start consistent

**Acceptance criteria:**

- IDE or CLI: scaffold `layers/<name>.layer` with pkg header, desc, starter construct, empty `present`, prompt stub
- Optional companion `examples/<name>_ref.veil` that `use`s the layer and platform packages
- Appears in file list without process restart

**Mission impact:** Many layers need a fast on-ramp.

---

## DSL-014: Impact view — dependents of this layer

**Status:** Done · **Priority:** P2  
**As a** language designer  
**I want** to see packages in the serve set that `use` this layer  
**So that** I know blast radius before breaking changes

**Acceptance criteria:**

- API or client-derived list of dependents
- Designer chrome shows impact list
- Action: re-check dependents (package check) after layer change

**Mission impact:** Safe evolution of family languages.

---

## DSL-015: Multi-layer workspace UX polish

**Status:** Done · **Priority:** P3  
**As a** designer juggling many DSLs  
**I want** search, filter, and kind-aware navigation across dozens of layers  
**So that** scale does not collapse the file picker

**Acceptance criteria:**

- Filter files by kind / name
- Optional favorites or “recent layers”
- Performance OK with 50+ layers in list

**Mission impact:** Matches “we will have many of them.”

---

## Implementation order (recommended)

| Slice | Stories | Outcome |
|-------|---------|---------|
| **A — File kind MVP** | DSL-001 … DSL-004 | **Done** |
| **B — Structure parity** | DSL-005 … DSL-008 | **Done** |
| **C — Language product** | DSL-009 … DSL-011 | **Done** |
| **D — Ship to teams** | DSL-012 … DSL-014 | **Done** |
| **E — Scale** | DSL-015 | **Done** |

**Vertical slice for first PR:** A (001–004) on `layers/` + one product-family sample (e.g. extend `examples/wear_test.layer`).

---

## Mission impact summary

| Loop | Language designer | Product implementer |
|------|-------------------|---------------------|
| Machine | Layer check + dependent package check | Package check under DSL rails |
| Human | Topology + source + props + diff on `.layer` | Topology + critical bodies on `.veil` |
| Agent | Layer + package tools | Package tools; vocabulary from layers |

This epic makes **language authoring** as real as **program authoring**, which is required when many team DSLs are iterated weekly and client products are programs on top of platform packages — not config rows in a generic engine.

---

## Implementation notes (2026-07-10)

Shipped in one capability pass (logical order A→E):

| Area | Implementation |
|------|----------------|
| File kind | `FileKind` on `FileInfo`; serve collects `.veil` + `layers/*.layer` |
| Edit | Layers editable; `POST /api/source` validates via `check_layer` |
| Check | CLI + `/api/check` kind-aware; `veil_ir::check_layer` / `build_layer_ir` |
| Hot reload | Layer write rebuilds dependent package registries + SSE |
| Topology | Layer IR graph (constructs/groups/statements/prompt) |
| Structured edit | `layer_edit` + `/api/edit` for create/rename on layers |
| Scaffold | `POST /api/layer/scaffold` |
| Impact | `GET /api/layer/dependents?layer=` |
| Viewer | Kind badge, kind-labeled file list, `activeFileKind` |
| Docs | `docs/LAYERS_DSL.md` |

