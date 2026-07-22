# veil-presentation-model

**Type:** Concept  
**Summary:** Layer-driven IDE views — how layers declare named views, nest rules, layouts (flat/tabs/tree/flow), orphan policies, and lenses so the viewer projects the same AST differently per paradigm without domain knowledge in the engine.  
**Links:** veil-project-index, veil-language-core, veil-ddd-layer

## Purpose

Layers already define vocabulary, shapes, and chrome (visual). **Presentation** adds:
- Named **views** per host construct type
- **Nest rules** (child → parent relationships in a view)
- **Layouts** (flat, tabs, tree, flow)
- **Orphan policies** (how unattached nodes display)
- **Lenses** (review filter tags like `critical`)

The viewer projects the same AST differently based on which view is active — **no source rewrite on view switch**.

**Zero domain knowledge:** Switching paradigm (DDD → Svelte → Functional) = different `.layer` file. Same viewer code path.

## Grammar

Declared in a `present` block on constructs:

```
construct Context
  kw ctx
  mt mod
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
      nest Event under Aggregate when declared_in_parent
      nest Command under Aggregate when declared_in_parent
      orphan_policy list
```

Roles on constructs:
```
construct Aggregate
  present
    role container
    nestable_in model as root
    lens critical
```

## Layouts

| Layout | Behavior |
|--------|----------|
| `flat` | All candidates as siblings |
| `tabs` | Partition by source group / explicit tab list |
| `tree` | Roots + nest rules; nested nodes omit from top level |
| `flow` | Siblings + ELK layered sequence (LR default) |
| `bipartite` | Two columns + edges (deferred; falls back to flat) |

Unknown layout id → layer load error (strict).

## Member Modes & Nest Predicates

**Member modes** (how candidate set is chosen):
- `by_host_children` — IR children of host (default for tree/flat/flow)
- `by_source_group` — partition by construct's group (default for tabs)
- `by_construct` — filter to names in roots + nest rules
- `all_descendants` — full descendant set (use sparingly)

**Nest `when` predicates:**
- `declared_in_parent` — AST ancestor relationship (default)
- `same_source_group` — share nearest Group ancestor
- `always` — deterministic attach
- `implements` — IR Implements edge
- `references` — IR References edge (FK ownership)

**Orphan policies:** `list` (default, show as siblings), `hide`, `bucket` (synthetic folder), `bucket:Name`

## Projection Algorithm

Given host node H, active view V, IR graph G:
1. **Candidates** = apply V.members to H/G
2. Filter infrastructure if user toggle hides them
3. **Partition/nest:**
   - flat: siblings, stable sort
   - tabs: partition by group key
   - tree: roots from V.roots, apply nest rules, orphan policy for remainder
   - flow: siblings + layout engine
4. Output: forest + optional tab key

**Stable ordering:** source order (span start) when available, else name.
**Never rewrites source** — views are display-only projections.

## Agent Usage

`GET /api/context` includes presentation model + outline + layer prompts.

Agents should describe structure using view language (e.g. "in Domain model under Customer aggregate") when the active host has multiple views.

Always use construct **names** (Aggregate, Event) not keywords (agg, evt) in nest rules — keywords may alias across layers but names are stable.
