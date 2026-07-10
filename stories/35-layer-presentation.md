# Layer-Driven Presentation & View Model

Mission: the engine and viewer keep **zero domain knowledge**. Layers already
define vocabulary, shapes, constraints, and codegen policy. They must also
define **how the IDE presents** that vocabulary — hierarchy, multiple views,
grouping, layout, and review lenses — so paradigms (DDD, functional, UI, …)
get first-class visualization without Svelte hardcoding `agg` / `ctx` / etc.

## Problem

Today layers can express:

| Capability | Example | Viewer effect |
|------------|---------|----------------|
| Visual chrome | `icon`, `color`, `label` | Node look |
| Bucket placement | `group domain`, `dg infrastructure` | Palette section / default create group |
| Parent tabs | `requires_groups domain, application, …` | Tabs on mod-shaped parents |
| Drop rules | `in Context` | Allowed parent |
| Nested parse shapes | `has root: struct`, `fn[]` | AST structure, not canvas hierarchy |

What layers **cannot** express:

- Hierarchical **domain-model** view (Aggregate → root / entities / events / commands)
- Multiple named **views** of the same package/context (groups vs model vs ports)
- Layout policy (tree vs flow vs flat) per view
- Paradigm-specific nesting that is **not** the same as source `group` blocks
- Review lenses (“critical”) declared by layer rather than viewer heuristics

Result: DDD has organizational groups, but the canvas stays mostly flat inside
each group. Functional / other paradigms face the same ceiling. Any hierarchy
hardcoded for DDD would violate the mission.

## Product principle

> Layers teach the IDE **how to look**, not only **what words mean**.

The viewer interprets a small, stable **presentation IR** (views, containers,
membership rules, layout hints). All paradigm content lives in `.layer` files.

## Non-goals (this epic)

- Click-to-build full expression editors (separate UX stories)
- Hardcoding Aggregate nesting in `veil-viewer`
- Replacing source-level `group` blocks (presentation is orthogonal; groups remain source structure)
- Perfect auto-layout of every edge kind (wiring policy remains UX-013)

## Normative design

**Locked in LAY-001:** [`docs/PRESENTATION.md`](../docs/PRESENTATION.md)

Summary:

- `present` / `view` / `nest` / `layout` / `members` / `orphan_policy` / `lens`
- Construct **names** (not keywords) in all rules
- Views are **display projections** only (no source rewrite)
- Fallback: `requires_groups` → implicit tabs; else flat
- MVP layouts: `flat`, `tabs`, `tree`, `flow`
- Worked examples: DDD `Context`, Svelte `App`, functional `Module`
- Machine IR JSON shape for LAY-002
- Zero-domain-knowledge review checklist

---

## LAY-001: Presentation grammar + docs

**Status:** Done · **Priority:** P0  
**As a** layer author  
**I want** a documented way to declare views, nest rules, and layout hints  
**So that** paradigm authors can drive the IDE without engine patches

**Acceptance criteria:**

- Design doc (or section in `docs/LANGUAGE.md` / new `docs/PRESENTATION.md`) with:
  - Grammar for presentation declarations in `.layer`
  - Semantics: views, nest rules, layouts, orphan policy, fallback when absent
  - At least two worked examples: **DDD Context** and **one non-DDD** (e.g. functional module or svelte app)
- Explicit non-goals and migration path from `requires_groups` / `group` / `dg`
- Review checklist: “would this force domain knowledge into the viewer?” → no
- No requirement to fully implement the runtime in this story — grammar + semantics lock

**Done notes:** `docs/PRESENTATION.md` is normative; `LANGUAGE.md` + `MISSION.md`
cross-link. Runtime parse/API = LAY-002 (do not put `present` in ddd.layer until
loader accepts it, unless LAY-002 lands in the same change).

**Mission impact:** Without a locked surface, UX hierarchy work will hardcode paradigms.

---

## LAY-002: Parse presentation into registry + API

**Status:** Done · **Priority:** P0  
**As a** viewer or agent client  
**I want** presentation metadata via API (with palette or dedicated endpoint)  
**So that** the IDE loads view definitions with the layer vocabulary

**Acceptance criteria:**

- Layer loader parses presentation blocks (per LAY-001) into `LayerRegistry`
- Serializable DTO (e.g. `PresentationModel` / extended palette payload)
- `GET /api/palette` **or** `GET /api/presentation` returns views + roles
  (document choice; prefer one payload to avoid dual source of truth)
- Unknown presentation keys fail layer load with a clear error (strict) **or**
  warn and ignore (document); prefer strict for unknown **layout** names
- Unit tests: ddd.layer (or fixture) loads expected views; invalid block errors

**Depends:** LAY-001  
**Touch:** `veil-ir/layer.rs`, server API, docs

**Done notes:**

- Module `crates/veil-ir/src/presentation.rs` — types, validation, `presentation_from_registry`
- `ConstructSpec.presentation` filled by `present` section in layer loader
- Strict validation: layouts, members, nest when, orphan_policy, construct name refs
- **`GET /api/presentation`** (separate from palette; empty hosts if no `present`)
- Fixture tests; `ddd.layer` still without live `present` until LAY-004

**Mission impact:** Delivery path for layer → IDE without viewer knowledge of DDD.

---

## LAY-003: Viewer consumes presentation IR (generic)

**Status:** Done · **Priority:** P0  
**As a** human reviewing a package  
**I want** the canvas to offer layer-declared views and nest by presentation rules  
**So that** hierarchy is paradigm-correct without frontend special cases

**Acceptance criteria:**

- When current parent’s construct type has multiple views, UI shows a **view switcher**
  (tabs/segmented control) labeled from presentation IR
- Selecting a view re-projects children:
  - `tabs` / `by_source_group` — current group-tab behavior (may re-use `requires_groups`)
  - `tree` — roots + nested children per nest rules; drill into containers
  - `flat` — current default
- No string-match on `agg`, `ctx`, `saga`, etc. in viewer code paths for layout
- Fallback: no presentation → today’s flat / requires_groups behavior
- Breadcrumb + selection still work after view switch
- Golden or component test with a **fixture layer** (not only ddd) proving genericity

**Depends:** LAY-002  
**Touch:** `veil-viewer` (`+page.svelte` / layout / store), possibly IR projection helper

**Done notes:**

- `veil-viewer/src/lib/presentation.ts` — pure projection (`projectView`, members,
  nest, tabs/tree/flat/flow); uses construct **names**/subkinds only
- Fetch `GET /api/presentation` into `presentationModel` store
- View switcher UI when host has ≥2 views; group tabs still nest under tabs layout
- Fallback path unchanged when presentation empty (pre-LAY-004 ddd.layer)
- Genericity check: `veil-viewer/scripts/check-presentation.mjs` (Host/RootType/ChildType)

**Mission impact:** Makes multi-view topology real for any layer, not only DDD.

---

## LAY-004: DDD layer presentation (Context model view)

**Status:** Done · **Priority:** P1  
**As a** DDD author/reviewer  
**I want** a domain-model view that nests under Aggregates  
**So that** BCs are reviewable as hierarchical models, not only group buckets

**Acceptance criteria:**

- `ddd.layer` declares at least:
  - **groups** view (domain / application / infrastructure / presentation)
  - **model** view: Aggregates as roots; Events/Commands (and Entities/VOs when
    nested in source or via declared rules) shown under their Aggregate
- Application / infrastructure groups remain sensible in groups view
- Existing examples (`customer_onboarding`, `hello_world`, `sales_crm`) open and
  render without error; model view non-empty where aggs exist
- Document in layer `prompt` or desc how authors should nest for best model view
- No viewer changes that mention Aggregate by English hardcoding — only layer data

**Depends:** LAY-003  
**Touch:** `layers/ddd.layer`, optional example tweaks

**Done notes:**

- `layers/ddd.layer` (+ `examples/ddd.layer`): Context **Layers** + **Domain model**
  views; Orchestrator Layers; Aggregate container + Event/Command/Entity/VO roles
- Prompt section documents IDE views and nesting guidance for agents/authors
- Test: `ddd_layer_context_has_groups_and_model_views`
- Viewer still has zero DDD hardcoding (LAY-003 projection)

**Mission impact:** Proves the epic on the primary paradigm.

---

## LAY-005: Second-paradigm proof (non-DDD)

**Status:** Done · **Priority:** P1  
**As a** maintainer  
**I want** a non-DDD layer to declare a different view structure  
**So that** we prove presentation is not a DDD feature in disguise

**Acceptance criteria:**

- One of: `functional.layer`, `svelte5.layer`, or a small fixture layer declares
  ≥2 views with different layouts/nesting than DDD
- Example or fixture `.veil` demonstrates the views in the IDE
- Same viewer code path as LAY-003 (no `if layer == ddd`)
- Short note in PRESENTATION.md: “adding a paradigm = layer only”

**Depends:** LAY-003  
**Done notes:**

- `layers/svelte5.layer`: App views **Folders** (tabs pages/components/stores) +
  **Route tree** (Layout/Page roots; nest Page under Layout, Component under Page/Layout)
- Demo: `examples/svelte_present_demo.veil` — `veil serve examples/svelte_present_demo.veil`
- Test: `svelte5_layer_app_has_folders_and_routes_views` (asserts not DDD model/Context)
- PRESENTATION.md §12: “Adding a paradigm = layer only” + proof table

**Mission impact:** Guards the zero-domain-knowledge invariant.

---

## LAY-006: Layout kinds (MVP set)

**Status:** Done · **Priority:** P1  
**As a** layer author  
**I want** a documented MVP set of layout algorithms  
**So that** I can choose presentation without inventing engine features per app

**Acceptance criteria:**

MVP layouts (implement + document):

| Layout | Behavior |
|--------|----------|
| `flat` | All projected nodes as siblings (current default) |
| `tabs` | Partition by source group / declared tab keys |
| `tree` | Hierarchical expand/drill from roots + nest rules |
| `flow` | Optional: LR/TB sequence for fn/flow-shaped parents (may map existing ELK flow) |

- Unknown layout name → error or fallback `flat` (document)
- Viewer uses layout field only; no per-construct layout hardcoding
- At least `flat`, `tabs`, `tree` wired end-to-end in tests

**Depends:** LAY-001, LAY-003  
**Done notes:**

- `crates/veil-ir/src/project.rs` — pure projection + unit tests for flat/tabs/tree/flow
  and unknown→flat fallback; `bipartite` runtime-falls-back to flat
- Load still strict (unknown layout fails layer load)
- Viewer: `resolveLayout`, `flowDirection`; canvas placement keyed only by
  `projected.layout` (not host kind hardcoding for presentation path)
- PRESENTATION.md §4.1 updated with implementation status

**Mission impact:** Stops ad-hoc layout growth inside the Svelte app.

---

## LAY-007: Nest rules & containment edges

**Status:** Open · **Priority:** P1  
**As a** layer author  
**I want** nest rules that use containment and/or IR relationships  
**So that** hierarchy can follow parse tree *or* semantic edges

**Acceptance criteria:**

- Support at least two rule kinds (names per LAY-001):
  1. **AST / declared-in-parent** — child construct’s source parent is the container
  2. **Type membership** — e.g. field type / annotation link (if already in IR);
     if not available, document as follow-up and ship (1) only
- Orphan policy: `list` | `hide` | `bucket:<name>` for nodes that match a view
  filter but have no parent
- Cycles / ambiguous parents: deterministic rule (document + test)
- Unit tests with synthetic IR graphs

**Depends:** LAY-001, LAY-002  
**Mission impact:** Hierarchical DDD model view is only as good as nest rules.

---

## LAY-008: Create / palette respect presentation

**Status:** Open · **Priority:** P2  
**As a** human adding structure  
**I want** create/drop placement to follow view + `dg` / nest rules  
**So that** new constructs land in the right group *and* hierarchical parent

**Acceptance criteria:**

- Creating from palette while in a view uses presentation + `dg` / `allowed_in`
- If current selection is a container (e.g. Aggregate in model view), optional
  “create child” uses nest rules for parent_span
- Invalid placement returns diagnostic (existing edit errors OK)
- Document interaction with UX-012 (palette drop persist)

**Depends:** LAY-003, UX-012  
**Mission impact:** Presentation stays honest when editing, not only when viewing.

---

## LAY-009: Layer-declared review lenses

**Status:** Open · **Priority:** P2  
**As a** reviewer  
**I want** criticality / focus lenses driven by layer tags  
**So that** “what matters” is paradigm-defined, not hardcoded in the viewer

**Acceptance criteria:**

- Presentation or construct metadata can mark roles: e.g. `lens critical`,
  `lens integration`, or annotation classes
- Viewer “Critical” filter (UX-022) consumes lens membership from IR/presentation
  plus diagnostics (escape hatches) — not keyword lists like `saga`/`guard`
- ddd.layer tags a sensible default set (guards, compensate, ports, adapters)
- functional/other layer can choose a different set

**Depends:** LAY-002, related UX-022  
**Mission impact:** Dual-loop review stays layer-honest.

---

## LAY-010: Agent/context export of presentation

**Status:** Open · **Priority:** P2  
**As an** in-IDE agent  
**I want** presentation/view metadata in context tools  
**So that** I describe topology the way humans see it

**Acceptance criteria:**

- Agent tool or IR summary includes active view id + projected tree outline
- `get_palette` / context bundle documents views (ties to AGT stories)
- Agent docs: prefer speaking in view terms (“under Aggregate X in model view”)

**Depends:** LAY-002, AGT context tools  
**Mission impact:** Agents and humans share one structural vocabulary.

---

## Suggested implementation order

| Order | Story | Why |
|-------|-------|-----|
| 1 | **LAY-001** | Lock grammar before code |
| 2 | **LAY-002** | Registry + API |
| 3 | **LAY-006** (MVP layouts) + **LAY-007** (nest rules) | Projection engine |
| 4 | **LAY-003** | Viewer switcher + project |
| 5 | **LAY-004** | DDD proof |
| 6 | **LAY-005** | Non-DDD proof |
| 7 | **LAY-008** … **LAY-010** | Edit + lenses + agent |

**Before deep UX chrome** that assumes hierarchy (parts of UX-024/025), prefer
LAY-001–004 so those stories can consume presentation rather than invent DDD UI.

## Relationship to other epics

| Epic | Relationship |
|------|----------------|
| [30-viewer-review](30-viewer-review.md) | Review panes show selection; **what is selectable/nested** comes from LAY |
| [40-viewer-restructure](40-viewer-restructure.md) | Edits persist AST; presentation only re-projects |
| [50-invariant-debt](50-invariant-debt.md) | Forbids viewer DDD special cases — LAY is the compliant path |
| [100-ide-agent](100-ide-agent.md) | Agents need the same projection (LAY-010) |

## Open questions (resolved in LAY-001)

See `docs/PRESENTATION.md` §15:

1. **Construct-level only** for MVP (package-level deferred)
2. Stacked layers: **override** view by `(host, view_id)`; lenses **union**
3. Nest rules are **display-only** on view switch; create policy is LAY-008
4. **API JSON** is primary machine form; CLI dump optional later
