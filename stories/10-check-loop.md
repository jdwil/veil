# Check Loop Stories (Machine Feedback)

Mission: agents need a fast, honest `parse → check → codegen` loop.  
Success metric: agent fix-cycle time under `veil check` / compile feedback.

---

## CHK-001: Honest `veil check` CLI

**Status:** Done · **Priority:** P0  
**As an** agent (or CI job)  
**I want** `veil check` to validate the program and exit non-zero on errors  
**So that** broken VEIL is never reported as green

**Acceptance criteria:**

- Run structural validation (`validate_solution`) and graph diagnostics (`analyze`)
- Exit code `1` when any **error**-severity diagnostic exists
- Warnings may exit `0` but are printed clearly
- Default output is a **compact diagnostic list** (file, span/node, code, message)
- Do **not** dump full IR JSON by default; put behind `--dump-ir` if needed
- Do **not** print template/codegen side effects during check unless `--emit-templates`
- Document severity levels (error vs warning)

**Mission impact:** Without this, dual-loop “machine half” is fiction.

**Touch:** `crates/veil-cli/src/main.rs`, `veil-ir` validate/diagnostics

---

## CHK-002: Unify validate + diagnostics into one pipeline

**Status:** Done · **Priority:** P0  
**As a** toolchain consumer (CLI, server, viewer)  
**I want** one analysis entry point used everywhere  
**So that** CLI, `/api/diagnostics`, and post-edit validation agree

**Acceptance criteria:**

- Single public API e.g. `veil_ir::check::check_solution(sol, reg) -> Diagnostics`
- Includes today’s `validate_solution` constraints + `analyze` rules
- CLI `check`, server diagnostics, and `POST /api/edit` all use it
- Viewer shows the same set of issues after load and after edit
- Unit tests for at least: `requires_groups`, `must_implement_port`,
  `requires_implementation`, `must_have`, `deny`

**Mission impact:** Humans and agents must not see different truths.

---

## CHK-003: Unresolved name / call detection

**Status:** Done · **Priority:** P1  
**As an** agent  
**I want** unknown identifiers, methods, and ports reported at check time  
**So that** I fix mistakes before codegen invents `todo!` or wrong deps

**Acceptance criteria:**

- Resolve construct names, ports, methods, local bindings, and stub APIs in scope
- Error on call to unknown target/method (with suggestion if close match)
- Error on type names that are neither builtins, aliases, nor defined constructs
- Works across nested groups/modules within a package
- Cross-package references follow `use` / expose rules (or clearly warn if deferred)
- Tests covering happy path + missing port method + typo

**Mission impact:** Core of agent fix-cycle; today errors often only appear in `cargo check`.

---

## CHK-004: Basic type checking for expressions

**Status:** Done · **Priority:** P1  
**As an** agent  
**I want** type mismatches and fallibility mistakes caught in VEIL  
**So that** terseness (bare fields, `!`, sugar) does not create silent wrongness

**Acceptance criteria (MVP):**

- Check assignments and call args against known field/param types
- Check `Res!` / `?` / `await` usage consistency at a practical level
- Check match arm patterns vs scrutinee where types known
- Infer types for bare fields using the same conventions as codegen, but
  **report** ambiguous/unknown rather than guessing wrong
- Errors include node id / span for viewer navigation
- Expand later; document known limitations in diagnostics

**Mission impact:** “Terseness never outranks diagnostics” (MISSION design law).

---

## CHK-005: Target capability checks

**Status:** Done · **Priority:** P1  
**As an** agent generating for `-t rust|ts|…`  
**I want** unsupported constructs to fail at check for that target  
**So that** backends never silently emit placeholders as if complete

**Acceptance criteria:**

- Each backend declares a capability set (or unsupported feature list)
- `veil check app.veil -t ts` reports TS-incapable features as errors
- Features that currently lower to `todo!` / `/* range */` / empty services are
  either implemented or capability-gated
- Default check may use primary target (Rust) plus warn on multi-target debt

**Mission impact:** Full parity ambition requires honesty before breadth.

---

## CHK-006: Escape-hatch debt diagnostics

**Status:** Done · **Priority:** P2  
**As a** human reviewer  
**I want** raw blocks, stub-only calls, and untyped `Json` boundaries flagged  
**So that** agent dumping into escape hatches is visible debt

**Acceptance criteria:**

- Diagnostic codes for: raw template/style surfaces, `todo!`-bound adapters,
  stub-only external calls without body, `Json` at package boundaries
- Severity: warning by default; optional `--deny-escape-hatches` → error
- Viewer can filter/highlight these (ties to UX-022)
- Metric-friendly: CLI can print a count summary

**Mission impact:** Escape hatches are temporary debt (MISSION).

---

## CHK-007: `/api/check` for the IDE

**Status:** Done · **Priority:** P1  
**As a** human or agent plugin using the local server  
**I want** an HTTP check endpoint returning structured diagnostics  
**So that** the viewer dual loop matches the CLI

**Acceptance criteria:**

- `POST` or `GET /api/check` returns JSON diagnostics (same pipeline as CHK-002)
- Viewer refreshes diagnostics on load, file select, and after successful edit
- DiagnosticsPanel navigates to `node_id` on click (ties to UX-023)
- Optional: include target query `?target=rust`

**Mission impact:** Machine loop inside the human review surface.
