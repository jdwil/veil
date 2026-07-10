# ADR: Viewer expression-editor chrome (UX-027)

## Status

Accepted (2026-07-10)

## Context

The viewer accumulated half-wired visual editors (`ConstructEditor`,
`FieldsEditor`, `MethodEditor`, `EnumEditor`, `AnnotationEditor`, deep
`ExprEditor` / `ExprPicker` trees) before the review-first surfaces
(UX-020 source dock, UX-021 structural diff, UX-023 diagnostics jump)
were complete. Shipping “click-to-build all 34 expr kinds” pretends full
visual parity that does not exist and steals focus from topology + VEIL
body review.

## Decision

1. **Primary review path:** topology canvas + VEIL source dock + structural
   diff + check diagnostics. Property panel is secondary.
2. **Wired for review use cases only:**
   - `BlockEditor` (via PropertyEditor) for step/method body list read/edit
   - IR-driven methods list + `@invariant` (UX-025)
   - Annotation checkboxes already on PropertyEditor (layer palette defs)
3. **Quarantined (keep source, do not mount from routes):**
   - `ConstructEditor.svelte`
   - `FieldsEditor.svelte`, `MethodEditor.svelte`
   - `EnumEditor.svelte`, `AnnotationEditor.svelte` (standalone)
   - Full `ExprPicker` invent-all-kinds flow remains available only as
     nested implementation detail of `BlockEditor`, not as a product claim
4. **Do not expand** freeform expression click-to-build until UX-020–023
   remain Done and agent edit loop is the authoring default.

## Consequences

- PropertyEditor no longer imports unused editors (dead import cleanup).
- Files stay in tree for potential future wiring; they are not deleted so
  partial work is not lost.
- Agents and humans edit structure via `/api/edit` and review in VEIL source.
