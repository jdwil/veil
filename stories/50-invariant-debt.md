# Invariant Debt Stories

Mission law: **zero domain knowledge in the engine**.  
Engine executes shapes + layer-declared rules; it does not invent `@dep`, Bus,
or field-name policy.

---

## INV-001: Remove magic `"dep"` from engine

**Status:** Open · **Priority:** P1  
**As a** layer author  
**I want** DI routing driven by layer policy, not the string `"dep"`  
**So that** alternate DI vocabularies work without engine edits

**Acceptance criteria:**

- Engine does not special-case annotation name `"dep"` in builder/codegen/
  templates as hardwired policy
- Mechanism: annotation metadata (e.g. `role: dependency`) or explicit
  codegen template hooks in `di.layer` that the engine executes generically
- Existing `di.layer` + examples keep working
- Grep gate test: engine sources must not match `\b"dep"\b` for policy
  (allow parsing generic annotation names)

**Touch:** `builder.rs`, `rust.rs`, `template.rs`, `layers/di.layer`

---

## INV-002: Smart constructors as layer/target policy

**Status:** Open · **Priority:** P1  
**As a** target/policy author  
**I want** defaulting rules (id, timestamps, scalars) in `rust.layer` (or
  equivalent), not hardcoded arrays in `rust.rs`  
**So that** teams can change conventions without forking the compiler

**Acceptance criteria:**

- Field-name lists and type-default tables leave `rust.rs` / `typescript.rs`
- Implemented via layer codegen rules or declarative default policy tables
- Document how to customize
- Examples still generate compiling constructors

---

## INV-003: Bus / orchestrator policy out of expr core

**Status:** Open · **Priority:** P1  
**As a** non-DDD app author  
**I want** plain packages without JSON-Bus orchestration assumptions  
**So that** “blessed path ≠ core” holds

**Acceptance criteria:**

- JSON envelope routing and orchestrator `Value` typing are opt-in via layer
  runtime/routing metadata (or codegen templates), not unconditional for
  multi-module packages
- Packages without routing traits generate direct calls / normal types
- `ddd.layer` continues to provide Bus orchestration for service apps
- Tests: hello_world / non-bus package does not force Bus JSON path;
  onboarding still does

**Touch:** `expr.rs`, `rust.rs`, layer `routing` / `runtime` metadata

---

## INV-004: Genericize DDD constraint algorithms

**Status:** Open · **Priority:** P2  
**As a** layer author  
**I want** constraint words to map to generic engines or layer scripts  
**So that** `crud_for_aggregate` is not permanently special-cased Rust

**Acceptance criteria:**

- Inventory engine-encoded constraints in `validate.rs`
- Either: express via generic primitives (`must_have_methods find|save|…`,
  `child_subblock compensate`, …) declared in the layer  
  or: pluggable constraint handlers registered from layers
- Unknown constraints warn once (not silent skip) until implemented
- `crud_for_aggregate`, `spans_contexts`, `steps_have_compensation` keep
  behavior via layer definitions

---

## INV-005: No subkind string switches in backends

**Status:** Open · **Priority:** P2  
**As a** UI layer author  
**I want** Svelte/React emission keyed on layer tags, not `"Component"`  
**So that** renaming constructs does not break codegen

**Acceptance criteria:**

- TypeScript/Svelte backend uses layer flags / `codegen` match rules /
  visual or emit tags — not hardcoded subkind name sets
- `svelte5.layer` updated accordingly
- Test with an alias construct name mapping to the same shape

**Touch:** `typescript.rs`, `svelte5.layer`

---

## INV-006: Identity / FK heuristics as policy

**Status:** Open · **Priority:** P2  
**As a** layer author  
**I want** `id` / `*_id` identity and reference edges configurable  
**So that** domains without UUID identity still work

**Acceptance criteria:**

- Builder FK edge inference (`*_id` → References) is layer-configurable or
  off-by-default with layer opt-in
- `equality_by_value` / `has_identity` do not hardcode field `"id"` without
  layer schema
- Document default for `ddd.layer`

---

## INV-007: Invariant hygiene CI gate

**Status:** Open · **Priority:** P2  
**As a** maintainer  
**I want** CI to fail on new engine domain literals  
**So that** invariant debt does not grow

**Acceptance criteria:**

- Script or test scans engine crates for denylist tokens (e.g. Aggregate,
  BoundedContext, sqlx feature special-case, hardcoded annotation policy)
- Allowlist file for known residual debt with ticket IDs
- New hits fail CI until allowlisted with justification
