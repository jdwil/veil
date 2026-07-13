# Package adapt — specialize stock products without forking

**Goal:** Client / regional product packages can **adapt** a stock VEIL package
(source merge + path patches), producing one flattened IR and one generated
binary. Distinct from **`use`** (API dependency, possibly remote Bus).

**Status:** Done · P1  
**Design contract:** [`docs/ADAPT.md`](../docs/ADAPT.md)  
**Depends on:** package parse/serialize ([20](20-serialize-edit.md)), check
([10](10-check-loop.md)), gen ([60](60-codegen-targets.md))  
**Mission impact:** Enables Wear Test / Loyalty **product lines** and client
variants without copy-paste or OOP “extends.” Dual-loop can review patches and
flattened result.

---

## Vocabulary (locked)

| Term | Meaning |
|------|---------|
| **`use`** | Depend on boundary (layer / expose / Bus) — not source rewrite |
| **`adapt`** | Specialize base package **in this compile unit** |
| **`ins`** | Insert sub-component into existing construct (method, step, …) |
| **`rfn`** | Refine body; may splice **`stock`** (inlined ancestor) |
| **`rpl`** | Replace body; no `stock` |
| **`omit`** | Remove base symbol or step from surface |
| **`ren`** | Rename base symbol; rewrite references in merged IR |
| **`stock`** | Transpile-time placeholder for prior body (not runtime call) |

New top-level constructs: **ordinary syntax** (no `add` keyword).

---

## Epic outcomes

1. Gold-standard grammar + AST for adapt and all patch ops (including **`ren`**).
2. Multi-level adapt chain fully **inlined** before check/codegen.
3. `veil check` / `veil gen` / IDE understand adapted packages.
4. Fixture: stock package + client adapter that renames, inserts, refines with `stock`.
5. Docs: `LANGUAGE.md` + `ADAPT.md`; no “extends” in user-facing copy.

---

## Stories

### ADP-000: Design locked in docs — Done · P0

**I want** the adapt contract written once so implementers don’t invent OOP  
**So that** AI and humans share one gold standard

**Acceptance**

- [x] [`docs/ADAPT.md`](../docs/ADAPT.md) describes use vs adapt, ops, merge, check, non-goals
- [x] `LANGUAGE.md` indexes `adapt` / `ins` / `rfn` / `rpl` / `omit` / `ren` / `stock`
- [x] `stories/README.md` lists this epic

**Done notes:** Design + docs + story board locked (implementation Todo from ADP-001).

---

### ADP-001: Lexer + core keywords — Done · P0

**I want** the lexer to reserve adapt-related keywords  
**So that** packages can parse patches

**Acceptance**

- [x] Keywords (or unambiguous parse of): `adapt`, `ins`, `rfn`, `rpl`, `omit`, `ren`, `stock`
- [x] Not layer-only vocabulary — core package grammar
- [x] Lexer tests for keyword recognition
- [x] No collision with existing idents in examples (rename if needed)

**Mission impact:** Surface syntax exists.

---

### ADP-002: Parse `adapt` and resolve base package sources — Done · P0

**I want** `adapt wear_test` to load the base package AST  
**So that** specialization has a real base

**Acceptance**

- [x] Package body accepts `adapt <name>` (optional `as` only if needed — default no)
- [x] Resolver finds base `.veil` package sources (search paths: same dir, project, hub, examples, config)
- [x] Error if base missing or is a layer/stub only
- [x] Error if adapting denylisted platform packages (`dlx_core` at minimum)
- [x] AST field e.g. `Package.adapts: Vec<AdaptDecl>`
- [x] Serialize/round-trip `adapt` lines

**Mission impact:** Base product is loadable as source.

---

### ADP-003: Adapt chain, cycles, multi-level order — Done · P0

**I want** multi-level `adapt` (Acme → Regional → stock)  
**So that** product lines compose without diamonds by default

**Acceptance**

- [x] Build ordered chain root → leaf
- [x] Cycle → hard error with path
- [x] Diamond (two bases) → error **or** require explicit `adapt a, b order a then b` (document choice in ADAPT.md; implement one)
- [x] Unit tests: 3-level chain; cycle; forbidden platform adapt

**Mission impact:** Gold multi-level model, not single-level MVP.

---

### ADP-004: Path addressing for patches — Done · P0

**I want** patches to target `CreateInitiative.step persist` style paths  
**So that** we don’t rewrite whole packages

**Acceptance**

- [x] Path grammar: dotted / stepped (`X`, `X.fn y`, `X.step name`, extend as needed)
- [x] Resolve path against merged IR
- [x] Clear diagnostic when path missing
- [x] Tests for service, step, aggregate method paths

**Mission impact:** Surgical specialization.

---

### ADP-005: `ins` — insert sub-components — Done · P0

**I want** to insert methods and steps into existing base constructs  
**So that** clients extend structure without forking

**Acceptance**

- [x] `ins <path>` block body parses as construct members / steps
- [x] Step position: `before` / `after` / `at start` / `at end` (default end)
- [x] Insert method on aggregate from base
- [x] Insert step on service from base
- [x] New top-level `svc` / `agg` without `ins` still works (implicit add)
- [x] Round-trip serialize `ins`

**Mission impact:** “Add a method to Initiative” without `add` keyword for types.

---

### ADP-006: `rpl` — replace body — Done · P0

**I want** to fully replace a base service/fn body  
**So that** clients can discard stock logic when needed

**Acceptance**

- [x] `rpl <path>` replaces body of target
- [x] `stock` inside `rpl` → error
- [x] Signature (inputs/outputs) must match base for `svc`/`fn` (ADP-C8)
- [x] Tests: replace service; stock-in-rpl fails check

**Mission impact:** Full override without keep-base.

---

### ADP-007: `rfn` + `stock` hygienic inline — Done · P0

**I want** to refine a base body by splicing the prior implementation  
**So that** clients wrap stock behavior without runtime `super`

**Acceptance**

- [x] `rfn <path>` with body containing `stock` (statement and/or expression form)
- [x] `stock` expands to prior body AST; **no** residual stock after merge
- [x] Expression form: `x = stock` binds return / last value of prior body
- [x] Hygienic local rename on collision
- [x] Multi-level: outer `stock` sees already-inlined inner refine
- [x] Generated Rust for refined service is a **single** function (no parent call)
- [x] Tests: wrap CreateX; 3-level refine; hygiene collision

**Mission impact:** Gold-standard specialization without OOP call stack.

---

### ADP-008: `omit` — remove base surface — Done · P1

**I want** to drop base services/steps from a client product  
**So that** legacy or unwanted stock features disappear

**Acceptance**

- [x] `omit <path>` removes symbol or step
- [x] References to omitted symbol elsewhere in merge → error (or document auto-break)
- [x] Tests: omit service; omit step

**Mission impact:** Client product surface control.

---

### ADP-009: `ren` — rename base symbol — Done · P1

**I want** to rename a base construct or service for branding or clarity  
**So that** clients don’t fork whole files for naming

**Acceptance**

- [x] `ren <path> <new_name>` (or `ren <path> as <new_name>` — pick one in grammar, document)
- [x] Updates definition and **internal references** in merged IR (same package merge)
- [x] Collision with existing name → error
- [x] Works with subsequent `rfn`/`ins` on **new** name (patches after ren use new path, or ren last — **define order: ren before other patches that target new name; document that ren is applied in source order**)
- [x] Expose/API rename if base had expose entry for that name
- [x] Serialize/round-trip
- [x] Tests: ren ListInitiatives → ListPrograms; rfn after ren; collision

**Mission impact:** Feature-complete naming for product lines (small, high leverage).

---

### ADP-010: Merge pipeline + `veil check` on flattened IR — Done · P0

**I want** check to run on the fully merged package  
**So that** adapters can’t ship broken merges

**Acceptance**

- [x] Library API: `merge_adapted_package(leaf) -> Solution` (name flexible)
- [x] `veil check acme.veil` loads adapt chain and checks merge
- [x] Diagnostics cite leaf patch spans when possible
- [x] Provenance available for tooling (symbol → [packages])
- [x] Fixture package under `examples/adapt_*/` or `examples/acme_adapt/`

**Mission impact:** Machine loop trusted on adapted products.

---

### ADP-011: Codegen from merged IR only — Done · P0

**I want** `veil gen` to emit one workspace from the flattened IR  
**So that** runtime has no adapt machinery

**Acceptance**

- [x] `veil gen` adapter package → same backends as today on merged Solution
- [x] No generated “call parent package” helpers for `stock`
- [x] Integration: gen + `cargo check` on adapt fixture (Rust target)
- [x] Manifest/handler names reflect `ren` results

**Mission impact:** Transpiler-faithful deploy.

---

### ADP-012: Serialize, edit ops, IDE dual-loop — Done · P1

**I want** the dual-loop IDE to show and edit adapt packages  
**So that** humans review specialization structurally

**Acceptance**

- [x] Serialize all adapt syntax (canonical form)
- [x] EditOps or source edit for patches (minimum: source dock + reload)
- [x] IDE: badge or project meta “Adapts: a → b → this”
- [x] Optional: flattened source preview (read-only) — gold; may ship after graph badge
- [x] Palette/IR: adapted symbols appear in graph after merge (when viewing leaf as product)
- [x] Agent tools: reload after external edit still works ([files/reload](../crates/veil-server/src/api.rs))

**Mission impact:** Human loop for product lines.

---

### ADP-013: Docs + example product line — Done · P1

**I want** a canonical example and language reference  
**So that** Wear Test / ACME stories have a template

**Acceptance**

- [x] `examples/adapt_stock.veil` + `examples/adapt_client.veil` (or under `examples/adapt/`)
- [x] Client: `adapt stock`, `ren`, `ins` step or method, `rfn` with `stock`, optional `omit`
- [x] `docs/LANGUAGE.md` section for adapt ops
- [x] Cross-link from wear-test / engagement design notes when those exist
- [x] `veil check` + `veil gen -t rust` green on example

**Mission impact:** Teachable gold standard.

---

## Implementation order (recommended)

```text
ADP-000 (docs index)
    → ADP-001 lexer
    → ADP-002 parse adapt + resolve
    → ADP-003 chain rules
    → ADP-004 paths
    → ADP-005 ins
    → ADP-006 rpl
    → ADP-007 rfn + stock   ⎫
    → ADP-008 omit          ⎬ can parallel after paths
    → ADP-009 ren           ⎭
    → ADP-010 merge + check
    → ADP-011 gen
    → ADP-012 IDE
    → ADP-013 example polish
```

**Critical path:** 001 → 002 → 003 → 004 → 007 → 010 → 011  
**ren/ins/omit** complete the surface before 013.

---

## Acceptance demo (epic done)

```bash
# Stock + client
veil check examples/adapt/stock.veil
veil check examples/adapt/client.veil    # adapt stock; ren; ins; rfn with stock
veil gen examples/adapt/client.veil -t rust -o /tmp/adapt_out
cd /tmp/adapt_out && cargo check -p veil_bin   # or relevant crate
```

Human: open `client.veil` in IDE → see adapt badge → open flattened or graph →
confirm one `CreateInitiative`-equivalent body includes stock steps + client step.

---

## Out of scope (explicit)

| Item | Why |
|------|-----|
| Runtime dynamic adapt | Transpiler model |
| `adapt` on `.layer` files | Language via `use`; different epic |
| Per-initiative-row packages as default | DB + Reaction binding |
| Diamond multi-base without order | Complexity; chain is enough for product lines |
| Silent AOP | Dual-loop opacity |

---

## Status board

| ID | Title | Status |
|----|--------|--------|
| ADP-000 | Design locked in docs | **Done** |
| ADP-001 | Lexer keywords | **Done** |
| ADP-002 | Parse adapt + resolve base | **Done** |
| ADP-003 | Chain / cycle / order | **Done** |
| ADP-004 | Path addressing | **Done** |
| ADP-005 | `ins` | **Done** |
| ADP-006 | `rpl` | **Done** |
| ADP-007 | `rfn` + `stock` inline | **Done** |
| ADP-008 | `omit` | **Done** |
| ADP-009 | `ren` | **Done** |
| ADP-010 | Merge + check | **Done** |
| ADP-011 | Gen flattened | **Done** |
| ADP-012 | Serialize + IDE | **Done** |
| ADP-013 | Example + LANGUAGE | **Done** |
