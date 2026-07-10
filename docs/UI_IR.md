# Structured UI IR (PAR-007)

## Problem

Raw `template """..."""` traps critical UI in opaque strings — hard to check,
diff, or multi-target lower.

## Target model

Layer constructs for **framework-agnostic view trees**:

```text
view LoginForm
  el form
    el input @bind email: Str
    el button @on click -> submit()
      text "Sign in"
  when loading
    el spinner
  list items as item
    el row
      text item.name
```

| Construct | Role |
|-----------|------|
| `view` / `ui` | Root presentable unit |
| `el` | Element node (tag + attrs + children) |
| `when` / `else` | Conditional branch |
| `list … as` | Collection map |
| `text` | Text child |
| `template """…"""` | **Escape hatch** — debt-flagged (CHK-006) |

## Codegen (first target: Svelte)

- Structured tree → Svelte markup + script bindings.
- Migration: existing `template` blocks remain valid; check emits escape-hatch
  debt so UI can be rewritten incrementally (depends GEN-004 templates).

## MVP slice (this story)

- Design locked here.
- Layer: `layers/ui.layer` — `view` / `el` / `text` constructs + Svelte template
  skeleton for `view` (codegen target `svelte`).
- Escape hatch remains; raw `template` still debt-flagged (CHK-006).
- Full el/when/list → Svelte emit is incremental; start with `view` shells.
- Presentation layers (`docs/PRESENTATION.md`) continue for **IDE views**;
  UI IR is for **product UI** in packages.

## Non-goals (phase N)

- Pixel-perfect multi-framework parity in one shot
- Replacing all HTML strings on day one
