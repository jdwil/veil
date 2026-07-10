# Viewer Review Stories (Human Loop)

Mission: humans approve **topology + critical bodies** without reading all
generated LoC. Invest in **read / navigate / restructure / diff** before full
expression click-to-build.

**Related:** Paradigm-specific hierarchy and multi-view topology are **not**
hardcoded here — see [35-layer-presentation.md](35-layer-presentation.md)
(LAY-*). UX-022 criticality should eventually consume layer lenses (LAY-009).

---

## UX-020: VEIL source & body review pane

**Status:** Done · **Priority:** P0  
**As a** human reviewer  
**I want** to read VEIL for the selected construct (and its critical bodies)  
**So that** I review the intermediate language by default

**Acceptance criteria:**

- Panel shows VEIL for selection (construct / step / method), using live source
  or re-serialized fragment
- `veilSource` (or focused extract) is actually rendered
- Syntax highlighting for VEIL is acceptable MVP (plain monospace + basic OK)
- This is the **primary** read surface for review; target source preview is
  secondary (UX-028)

**Done notes:** `VeilSourcePanel.svelte` dock — full package or selection-span
excerpt; monospace MVP.

**Mission impact:** Humans should not be pushed to generated LoC for routine review.

---

## UX-028: Multi-target source preview (navigable, secondary)

**Status:** Done · **Priority:** P1  
**As a** reviewer who wants to verify lowering  
**I want** a **source preview** of the current codegen target (Rust, TS, Svelte, …)  
**So that** I can inspect emitted code without it dominating the IDE

**Product decision:** Demote from primary chrome is fine; must remain **easy to
open and navigate**. Not Rust-only.

**Acceptance criteria:**

- Rename/reframe UI from “Rust / Generated” to **Source preview** (or similar)
- Lists files for the **active target** (`rust`, `ts`, …), not only `.rs`
- Target selector (or uses last `veil gen -t` / server config)
- Navigable: file list + search within file; jump from selected construct to
  best-effort matching generated file/section when possible
- Accessible from toolbar/tab without hunting; does not steal default focus
  from topology + VEIL body (UX-020)
- Live refresh after edit still works
- Default open file is target-appropriate (not hardcoded `application/mod.rs`)

**Done notes:** CodePreview retitled Source preview; lists all generated paths;
smarter default selection. Target selector still follows server gen target.

---

## UX-021: Structural / semantic diff

**Status:** Done · **Priority:** P0  
**As a** human reviewing agent output  
**I want** a structural diff of topology and critical bodies  
**So that** I approve changes without full re-walk or LoC churn

**Acceptance criteria (MVP):**

- Diff two revisions: current file vs previous save, or vs git HEAD if available
- Show added/removed/renamed constructs and changed signatures
- Highlight changed step/method bodies (text or AST-level)
- Entry point from toolbar: “Review changes”
- Later: PR-oriented multi-file; MVP single active file is OK

**Mission impact:** Core success metric — time-to-approve structural change.

**Done notes:** `veil_ir::structural_diff`; `GET /api/diff` vs `git HEAD` via
provider `baseline_source`; `DiffPanel` toolbar “Review changes” with jump.

---

## UX-022: Criticality lens

**Status:** Done · **Priority:** P1  
**As a** reviewer  
**I want** high-risk nodes emphasized  
**So that** I spend time on guards, compensate paths, adapters, and escape hatches

**Acceptance criteria:**

- Toggle or filter: Critical only
- Critical includes (configurable later): guards, compensate sub-blocks,
  adapter impls, constructs with escape-hatch diagnostics, `@invariant` methods
- Node badges/colors for critical without hardcoding DDD keywords where possible
  (use shape + sub-blocks + diagnostics)
- Count in toolbar: “N critical items”

**Done notes:** LAY-009 — presentation `lens critical` + escape/error diags;
toolbar filter + count; node badge. Statement-level guard/compensate still via
diagnostics / compensate badge.

---

## UX-023: Navigable diagnostics

**Status:** Done · **Priority:** P1  
**As a** reviewer or agent operator  
**I want** clicking a diagnostic to select/focus the node  
**So that** the dual loop connects machine and human surfaces

**Acceptance criteria:**

- Diagnostics include `node_id` (and span when available)
- Click → select node, ensure visible (drill breadcrumb if nested)
- Panel shows severity, code, message
- After edit, list refreshes (CHK-007)

**Done notes:** `focusDiagnostic` + DiagnosticsPanel click; check pipeline
already stamps node_id; edit refreshes diagnostics.

---

## UX-024: Step / flow body previews on the canvas

**Status:** Done · **Priority:** P1  
**As a** reviewer  
**I want** step cards to show a short preview of their actions  
**So that** I understand flows without property-panel archaeology

**Acceptance criteria:**

- Each step node shows up to N summary lines (calls, guards, assigns)
- Desugared statements show original sugar keyword when present
- Guard shows condition (+ message if present)
- Click opens full body in review pane (UX-020)
- Empty steps clearly labeled

**Related legacy:** UX-003 (flow body visualization)

**Done notes:** `bodyPreview` on step cards (max 4 + “more”); empty label;
keyword badges; select → VEIL source focus.

---

## UX-025: Aggregate / struct method review

**Status:** Done · **Priority:** P1  
**As a** reviewer  
**I want** struct-shaped constructs to expose methods and bodies  
**So that** invariants and domain mutations are reviewable

**Acceptance criteria:**

- Selected aggregate/struct lists methods: name, params, return, annotations
- Selecting a method shows body in review pane
- `@invariant` visible without opening generated code
- State-machine enums still show transitions (existing enum UI OK)

**Related legacy:** UX-004

**Done notes:** Builder emits struct `fn`s as `InterfaceMethod` children with
body + annotations; PE Methods list + `@invariant` lines; click selects for
VEIL source pane.

---

## UX-026: Compensation and routing visibility

**Status:** Done · **Priority:** P1  
**As a** reviewer of orchestrations  
**I want** compensate sub-blocks and Bus/cross-context calls visible  
**So that** failure paths and integration edges are not hidden

**Acceptance criteria:**

- Steps with compensate (or any layer-declared sub-block) expand/show that body
- Cross-context / Bus calls indicated on edges or step previews (generic:
  routing trait calls or `CallExpr.sugar`)
- No hardcoded “saga” string required beyond layer metadata

**Related legacy:** UX-007

**Done notes:** Sub-blocks emitted as nested Step (`sub_block` ann) with body;
preview lines + drill; `routingTargets` badges from Calls edges.

---

## UX-027: Demote unused expression-editor chrome

**Status:** Open · **Priority:** P2  
**As a** maintainer  
**I want** dead or half-wired editors either wired for review or removed  
**So that** we do not pretend full visual expression parity exists

**Acceptance criteria:**

- Inventory `ConstructEditor`, `FieldsEditor`, `MethodEditor`, `EnumEditor`,
  `AnnotationEditor` usage
- Either wire into PropertyEditor/review flows **for the review use cases**
  or delete/quarantine with a short ADR note
- Do not expand click-to-build of all 34 expr kinds until UX-020–023 are Done

**Mission impact:** Align investment with review-first mission.
