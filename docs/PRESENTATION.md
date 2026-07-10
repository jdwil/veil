# VEIL Layer Presentation Model

**Status:** Normative (LAY-001 grammar + semantics).  
**Implementation:**

- **LAY-002:** layer loader + **`GET /api/presentation`**
- **LAY-003:** viewer fetches presentation IR, view switcher, `projectView()`
  in `veil-viewer/src/lib/presentation.ts` (tabs / tree / flat / flow)
- **LAY-006:** MVP layouts locked + tested (`veil-ir/src/project.rs` + viewer
  `resolveLayout` fallback)
- **LAY-007:** Nest when predicates, orphan policies, cycle/ambiguity rules;
  `implements` edge; type membership deferred

**Mission rule:** The engine and viewer contain **zero domain knowledge**.
Paradigms (DDD, functional, Svelte UI, …) teach the IDE **how to look** via
this model — never via hardcoded `agg` / `ctx` / `saga` strings in UI code.

---

## 1. Purpose

Layers already define:

| Concern | Mechanism |
|---------|-----------|
| Vocabulary | `construct` / `statement` |
| Shape | `mt` → core shape |
| Parse structure | `has`, named sub-blocks |
| Placement | `in`, `group`, `dg` |
| Chrome | `visual` |
| Constraints | `cst` |
| Codegen | templates / `lang` policy |

**Presentation** adds: named **views**, **nest rules**, **layouts**, **orphan
policy**, and **lenses** so the IDE can project the same AST as a hierarchy,
tabbed groups, flow, or other layouts without knowing what “Aggregate” means.

### 1.1 Non-goals

- Rewriting `.veil` source when the user switches views (views are **projections**)
- Replacing source-level `group domain` blocks (those remain author structure)
- Encoding DDD (or any paradigm) inside `veil-viewer` or the parser
- Full force-directed / custom layout plugins in MVP
- Click-to-build expression chrome (separate UX stories)

### 1.2 Product principle

> Switching a view **re-projects** IR nodes for display and navigation.  
> It does **not** change parse trees, spans, or on-disk source.

---

## 2. Concepts

| Term | Meaning |
|------|---------|
| **View** | Named projection of children under a **host** construct type (e.g. under every `Context`) |
| **Host** | Construct **name** (layer construct id, e.g. `Context`, not keyword `ctx`) that owns a set of views |
| **Layout** | How projected nodes are arranged (`flat`, `tabs`, `tree`, `flow`, …) |
| **Nest rule** | How a child construct type attaches under a parent type **in a view** |
| **Role** | Presentation role of a construct type (`container`, `leaf`, …) |
| **Lens** | Tag for review filters (e.g. `critical`) — optional metadata |
| **Projection** | Ordered forest of IR nodes derived from AST + presentation rules |

Construct identity in rules is always the layer **construct name**
(`Aggregate`, `Port`), never the surface keyword (`agg`, `port`), so stacking
and aliases stay stable.

---

## 3. Surface grammar (`.layer`)

Presentation is declared with a `present` block. Two attachment sites:

1. **On a construct** — host views and/or roles for that construct type  
2. **Package-level** (optional, future) — defaults for the layer; **not required for MVP**

### 3.1 Full grammar (line-oriented, indent blocks)

```
present
  # --- views (typically on a mod-shaped host construct) ---
  view <view_id>
    label "<human label>"
    layout <layout_id>
    # optional:
    default                          # this view is selected when drilling into host
    members <member_mode>            # how the initial candidate set is chosen
    roots <ConstructName> [, <ConstructName>...]   # tree roots (layout tree)
    nest <ChildName> under <ParentName> [when <nest_when>]
    orphan_policy <policy>
    # layout-specific (tabs):
    tabs <name> [, <name>...]        # explicit tab keys; else from requires_groups / groups present
    # layout-specific (bipartite / ports-style) — deferred past MVP if needed:
    left <ConstructName> [, ...]
    right <ConstructName> [, ...]
    edge <edge_kind>                 # implements | calls | references | ...

  # --- roles (on any construct) ---
  role <role_id>                     # container | leaf | edge_endpoint
  default_view <view_id>             # when this type is the drill host, pick this view
  nestable_in <view_id> as root
  nestable_in <view_id> under <ParentName>
  lens <lens_id>                     # critical | integration | ...
```

**Lexical notes:**

- `<view_id>`, `<layout_id>`, `<role_id>`, `<lens_id>`, `<member_mode>`,
  `<policy>`, `<nest_when>`, `<edge_kind>` are **identifiers** (or a fixed enum
  listed in §4–§5). Unknown **layout** / **member_mode** / **orphan_policy** /
  **nest_when** values are **errors** at layer load (strict).
- `<ConstructName>` matches a `construct <Name>` in this layer or a dependency
  layer. Forward references within the same file are allowed; unresolved names
  are **errors** at end of layer load.
- `label` values are quoted strings.
- Comments `# ...` allowed as in the rest of the layer format.
- Multiple `nest` lines per view; order is significance for ambiguous parents
  (first matching rule wins — §6.3).

### 3.2 Minimal valid examples

**Host with two views:**

```
construct Context
  kw ctx
  mt mod
  ...
  present
    view groups
      label "Layers"
      layout tabs
      members by_source_group
      default
    view model
      label "Domain model"
      layout tree
      members by_host_children
      roots Aggregate
      nest Event under Aggregate when declared_in_parent
      nest Command under Aggregate when declared_in_parent
      nest Entity under Aggregate when declared_in_parent
      nest ValueObject under Aggregate when declared_in_parent
      orphan_policy list
```

**Container / leaf roles:**

```
construct Aggregate
  ...
  present
    role container
    nestable_in model as root

construct Event
  ...
  present
    role leaf
    nestable_in model under Aggregate
    lens critical
```

### 3.3 Abbreviation: roles imply nestable hints

If a view lists `roots Aggregate` and `nest Event under Aggregate`, the
`nestable_in` lines on Aggregate/Event are **optional sugar** for documentation
and palette tooling. The **view’s** `roots` / `nest` lines are authoritative
for projection. Loaders may warn if they disagree.

---

## 4. Enumerations (MVP)

### 4.1 Layouts (LAY-006 — implemented)

| Id | Behavior | Status |
|----|----------|--------|
| `flat` | All projected candidates as siblings (type-column placement) | **Implemented** |
| `tabs` | Partition by source group / `tabs` list; one tab active | **Implemented** |
| `tree` | Roots + nest rules; nested nodes omitted at host level (drill into containers) | **Implemented** |
| `flow` | Sibling candidates + ELK layered sequence (`LR` by default) | **Implemented** |
| `bipartite` | Two columns + edge kind | Deferred; load accepted, **runtime falls back to `flat`** |

**Load time (strict):** unknown layout id → **layer load error** (`validate_presentations`).

**Runtime (viewer):** if an older viewer meets a newer layout id, or `bipartite`,
`resolveLayout` maps to **`flat`** and sets `layoutFallback: true` (no crash).

**Engine tests:** `veil-ir::project` — `layout_flat_*`, `layout_tabs_*`,
`layout_tree_*`, `layout_flow_*`, `unknown_layout_falls_back_to_flat`.

**Viewer:** uses only `projected.layout` / `flowDirection` for placement — no
per-construct layout hardcoding.

### 4.2 Member modes (`members`)

How the **candidate set** is chosen before nest/tabs partitioning:

| Id | Candidates |
|----|------------|
| `by_host_children` | IR children of the host instance (Contains), excluding pure infrastructure if viewer filter says so |
| `by_source_group` | Same, then partition by construct’s layer `group` / instance group block |
| `by_construct` | Filter to construct names listed in `roots` and all `nest` child/parent names (union) |
| `all_descendants` | Host’s full descendant construct set (use sparingly) |

Default if omitted:

- `layout tabs` → `by_source_group`
- `layout tree` → `by_host_children`
- `layout flat` / `flow` → `by_host_children`

Unknown `members` → **layer load error**.

### 4.3 Nest `when` predicates (LAY-007 — implemented)

| Id | Meaning |
|----|---------|
| `declared_in_parent` | An **AST ancestor** of the child is a candidate of construct `ParentName` |
| `in_parent_type` | Alias of `declared_in_parent` |
| `same_source_group` | Child and parent share the same nearest **Group** ancestor name |
| `always` | Attach under a parent-type candidate (deterministic pick — §6.3) |
| `implements` | IR edge `Implements` between child and parent (either direction) |

Default if `when` omitted: `declared_in_parent`.

Unknown `when` → **layer load error**.

**Not implemented (no IR yet):** field-type / annotation **type membership** links.
When the IR gains typed field references, add e.g. `when field_type` without
viewer domain knowledge. Until then use `declared_in_parent` or `implements`.

### 4.4 Orphan policy (LAY-007)

Nodes in the candidate set that are not roots and not nested:

| Id | Behavior |
|----|----------|
| `list` | Show as top-level siblings alongside roots (default for `tree`) |
| `hide` | Omit from top-level; still recorded in `orphan_ids` |
| `bucket` | Synthetic folder labeled `"Other"`; orphans linked under it (not editable) |
| `bucket:Name` / `bucket Name` | Same with custom label |

Unknown policy → **layer load error**.

### 4.5 Roles

| Id | Meaning |
|----|---------|
| `container` | May be drilled; tree layout treats as expandable |
| `leaf` | Default; no child projection in tree beyond nest rules |
| `edge_endpoint` | Hint for bipartite/ports views |

### 4.6 Lenses

Free identifier tags for review filters (LAY-009 / UX-022). MVP well-known:

- `critical` — high-priority review
- `integration` — ports, adapters, external boundaries

Layers may define additional lens ids; the viewer shows only lenses it knows
**or** lists unknown lenses generically (“lens: foo”). Prefer not hardcoding
paradigm names—only lens ids from presentation IR.

---

## 5. Semantics

### 5.1 When views apply

Given the user has drilled into IR node *H* whose construct **subkind/name** is
`HostName` (from layer construct name stamped on the IR):

1. Collect all `view` blocks declared on `construct HostName` from loaded layers
   (see §7 merge).
2. If none: **fallback** (§5.5).
3. Else show a view switcher; active view = `default` view if any, else first
   declared view, else last-used per session (viewer preference).

### 5.2 Projection algorithm (normative outline)

Inputs: host IR node *H*, active view *V*, IR graph *G*, presentation model *P*.

1. **Candidates** = apply `V.members` to *H*/*G*.
2. Filter out layer-provided infrastructure if the user toggle hides them
   (existing viewer behavior — not presentation-specific).
3. **Partition / nest:**
   - `flat`: candidates as siblings, stable sort (§5.4).
   - `tabs`: partition by source group key; tab list = `V.tabs` if set, else
     union of keys present + host `requires_groups` / expected groups.
   - `tree`:  
     a. Roots = candidates whose construct name ∈ `V.roots`  
        (if `roots` empty, every candidate is a root).  
     b. For each nest rule in order, attach matching candidates under matching
        parents (§6).  
     c. Apply `orphan_policy` to unattached non-roots.  
   - `flow`: candidates as siblings; layout engine uses flow/ELK LR|TB.
4. Output: forest + optional tab key → used only for display/navigation.

### 5.3 Spans and edits

Projection never invents AST nodes. Synthetic orphan buckets have **no**
`span_start` and are not editable targets. All real nodes keep IR ids/spans
from the graph. Edits still key by **AST span start** (SER-005/006).

### 5.4 Stable ordering

Within a sibling list:

1. Source order (span start ascending) when available  
2. Else construct name, then display name  

View switch must not reshuffle source order randomly.

### 5.5 Fallback (no `present` on host)

Preserve today’s behavior:

1. If host has `requires_groups` / `expected_groups` → behave as  
   `view groups` + `layout tabs` + `members by_source_group` + those tab names.
2. Else → `layout flat` + `members by_host_children`.

No view switcher required when only the implicit single view exists.

### 5.6 Interaction with source `group` blocks

- Source `group domain` under a `ctx` remains the **authoring** partition.
- `members by_source_group` / `layout tabs` **read** those groups.
- `layout tree` may **cross** group boundaries only if candidates include those
  nodes (usually `by_host_children` still sees group children as host children
  in IR — implementation must use the same Contains edges the viewer uses today).

Presentation does **not** require authors to stop using groups.

---

## 6. Nest rules

### 6.1 Form

```
nest <ChildConstructName> under <ParentConstructName> [when <predicate>]
```

### 6.2 Matching

A candidate node *C* (construct name = Child) attaches under parent node *P*
(construct name = Parent) when:

1. `when` predicate holds for (*C*, *P*), and  
2. *P* is in the candidate set or is a chosen root, and  
3. No earlier nest rule already attached *C*.

### 6.3 Ambiguity (LAY-007)

If multiple parents satisfy the predicate:

1. Prefer the parent that is an **AST ancestor** of the child (when applicable).  
2. Else **lowest node id** (stable, deterministic).  
3. First matching **nest rule** wins for a given child (later rules skip).  
4. Refuse attach if it would create a **cycle** in the nest forest (child stays orphan).

Never attach under two parents (tree, not DAG) in MVP.

### 6.4 Type membership (deferred)

Field-type / annotation links are **not** available as first-class IR edges yet.
Do not invent viewer heuristics. Future: `when field_type` once IR carries them.

### 6.5 Display-only

Nesting in a view **must not** imply a source move. Creating a child under a
container in the IDE (LAY-008) may use nest rules to choose `parent_span`; that
is an edit policy story, not automatic rewrites on view switch.

---

## 7. Layer stacking and merge

Layers stack via `use` (e.g. `crm` uses `ddd`).

### 7.1 Views

- Key: `(HostConstructName, view_id)`.
- Later layer **overrides** a view with the same id on the same host (replace
  entire view block).
- Later layer may **add** new view ids.
- Order of views in the switcher = declaration order after merge (dependency
  layers first, then overriding layer’s order for new ids; overrides keep
  original position if id existed).

### 7.2 Roles / lens / nestable_in

- Later layer overrides same field on the same construct name.
- Lenses **union** across layers unless explicitly cleared (MVP: union only).

### 7.3 Conflict with `requires_groups`

If both `requires_groups` and an explicit `view` with `layout tabs` exist:

- Explicit `present` views win for the switcher list.  
- If no `view` has `layout tabs`, synthesize fallback from `requires_groups`
  (§5.5).

---

## 8. Machine IR (API shape)

Serializable form for `GET /api/presentation` or embedded in palette payload
(LAY-002 chooses one endpoint; **prefer single payload** under palette or a
sibling `presentation` field on the same response).

```json
{
  "version": 1,
  "hosts": {
    "Context": {
      "default_view": "groups",
      "views": [
        {
          "id": "groups",
          "label": "Layers",
          "layout": "tabs",
          "members": "by_source_group",
          "tabs": ["domain", "application", "infrastructure", "presentation"],
          "roots": [],
          "nest_rules": [],
          "orphan_policy": "list"
        },
        {
          "id": "model",
          "label": "Domain model",
          "layout": "tree",
          "members": "by_host_children",
          "tabs": [],
          "roots": ["Aggregate"],
          "nest_rules": [
            {
              "child": "Event",
              "parent": "Aggregate",
              "when": "declared_in_parent"
            },
            {
              "child": "Command",
              "parent": "Aggregate",
              "when": "declared_in_parent"
            },
            {
              "child": "Entity",
              "parent": "Aggregate",
              "when": "declared_in_parent"
            },
            {
              "child": "ValueObject",
              "parent": "Aggregate",
              "when": "declared_in_parent"
            }
          ],
          "orphan_policy": "list"
        }
      ]
    }
  },
  "constructs": {
    "Aggregate": {
      "role": "container",
      "lenses": [],
      "default_view": null
    },
    "Event": {
      "role": "leaf",
      "lenses": ["critical"],
      "default_view": null
    }
  }
}
```

**Viewer rules:**

1. Never branch on keyword strings (`agg`, `ctx`) — only on this IR + IR node
   `subkind` / construct name fields already supplied by the engine.  
2. Missing host entry → fallback §5.5.  
3. Unknown layout at runtime (old viewer / new layer) → treat as `flat` and
   surface a diagnostic once (forward compatibility); **loaders** stay strict.

---

## 9. Migration path

| Today | After LAY-002/003 |
|-------|-------------------|
| `requires_groups a, b, c` | Implicit tabs view **or** explicit `view groups` / `layout tabs` / `tabs a, b, c` |
| `group domain` on construct | Still used for palette + `by_source_group` membership |
| `dg infrastructure` | Unchanged create default; LAY-008 may also consult active view |
| `visual` | Unchanged |
| No hierarchy | Add `view model` + `nest` on the host construct |

**Authoring recommendation:** New paradigm layers should declare `present`
explicitly. Existing layers keep working via §5.5 until updated (LAY-004 for
ddd).

**Compatibility (LAY-002):** `present` is parsed and stored. Unknown
layout/members/when/orphan_policy and unresolved construct names **fail layer
load**. Layers without `present` are unchanged (API returns empty `hosts`).

---

## 10. Worked example — DDD (`Context`)

```
construct Context
  kw ctx
  mt mod
  au
  cst
    requires_groups domain, application, infrastructure, presentation
  visual
    icon "📦"
    color "#8b5cf6"
    label "Bounded Context"
  in top
  present
    view groups
      label "Layers"
      layout tabs
      members by_source_group
      tabs domain, application, infrastructure, presentation
      default
    view model
      label "Domain model"
      layout tree
      members by_host_children
      roots Aggregate
      nest Entity under Aggregate when declared_in_parent
      nest ValueObject under Aggregate when declared_in_parent
      nest Event under Aggregate when declared_in_parent
      nest Command under Aggregate when declared_in_parent
      orphan_policy list

construct Aggregate
  ...
  group domain
  present
    role container
    nestable_in model as root

construct Event
  ...
  group domain
  in Aggregate
  present
    role leaf
    nestable_in model under Aggregate
    lens critical
```

**User experience (once implemented):**

1. Drill into a `ctx` → switcher: **Layers | Domain model**.  
2. **Layers** → existing four tabs.  
3. **Domain model** → aggregates as roots; events/commands nested under the
   aggregate that owns them in source; free-floating domain types listed as
   orphans.

---

## 11. Worked example — non-DDD (Svelte app)

```
construct App
  kw app
  mt mod
  au
  cst
    requires_groups pages, components, stores
  visual
    icon "🔶"
    color "#ff3e00"
    label "Svelte App"
  in top
  present
    view groups
      label "Folders"
      layout tabs
      members by_source_group
      tabs pages, components, stores
      default
    view routes
      label "Route tree"
      layout tree
      members by_host_children
      roots Layout, Page
      nest Page under Layout when declared_in_parent
      nest Component under Page when declared_in_parent
      orphan_policy list

construct Layout
  ...
  group pages
  present
    role container
    nestable_in routes as root

construct Page
  ...
  group pages
  present
    role container
    nestable_in routes as root
    nestable_in routes under Layout

construct Component
  ...
  group components
  present
    role leaf
    nestable_in routes under Page
```

**Why this matters:** Same viewer code path as DDD; only `svelte5.layer` text
differs. No `if (layer === 'svelte5')` in the IDE.

---

## 12. Worked example — functional module (sketch)

```
construct Module
  kw module
  mt mod
  present
    view members
      label "Members"
      layout flat
      members by_host_children
      default
    view types
      label "Types"
      layout tree
      members by_construct
      roots ADT, Record, Typeclass
      nest Instance under Typeclass when declared_in_parent
      orphan_policy list
```

### Adding a paradigm = layer only

Ship a new programming style without a viewer PR:

1. Write/extend a `.layer` with `construct` vocabulary + **`present` views**.
2. Optionally add an example `.veil` that uses it.
3. Point `veil serve` at the example — the same view switcher and
   `projectView` path (LAY-003) apply. No `if (layer === "…")` in the IDE.

**Proofs in-tree:**

| Paradigm | Layer | Host views | Demo |
|----------|-------|------------|------|
| DDD | `layers/ddd.layer` | Context: Layers + Domain model | any `use ddd` package |
| Svelte UI | `layers/svelte5.layer` | App: Folders + Route tree | `examples/svelte_present_demo.veil` |

---

## 13. Review checklist (zero domain knowledge)

Before merging presentation-related code, answer **no** to all:

1. Does the viewer match on strings like `agg`, `ctx`, `saga`, `port`?  
2. Does layout code special-case “Aggregate” outside presentation IR?  
3. Does switching views write source or reshuffle AST?  
4. Can a new paradigm ship **only** as a `.layer` (+ examples) without a
   viewer PR?  
5. Are construct references in rules **construct names**, not keywords?

If any answer is yes, the design is violated.

---

## 14. Implementation roadmap (out of scope for LAY-001 lock, but ordered)

| Story | Deliverable |
|-------|-------------|
| **LAY-001** | This document (normative grammar + semantics) |
| **LAY-002** | Parse `present` → registry + API JSON |
| **LAY-006/007** | Layout + nest projection library (engine or viewer-shared) |
| **LAY-003** | View switcher UI |
| **LAY-004** | Real `present` blocks in `ddd.layer` |
| **LAY-005** | Real `present` in second paradigm layer |
| **LAY-008+** | Create placement, lenses, agent export |

---

## 15. Open decisions (closed for MVP)

| Question | Decision |
|----------|----------|
| Construct vs package-level `present`? | **Construct-level only** for MVP; package-level deferred |
| Stacked layers? | **Override by (host, view_id)**; lenses union (§7) |
| Nest rules rewrite source? | **Never** on view switch; create policy is LAY-008 |
| Human dump CLI? | Optional later (`veil present`); API JSON is primary machine form |
| Strict unknown keys? | **Yes** for layout/members/when/orphan_policy at load |

---

## 16. Related docs

- [`LANGUAGE.md`](./LANGUAGE.md) — layer construct format (`visual`, `group`, …)
- [`MISSION.md`](../MISSION.md) — zero domain knowledge, human topology review
- [`stories/35-layer-presentation.md`](../stories/35-layer-presentation.md) — epic
