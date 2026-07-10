# Codegen & Multi-Target Stories

Mission: hybrid codegen; semantic honesty over pretty demos; escape hatches measured.

---

## GEN-001: Surface adapter `todo!` as check debt

**Status:** Done · **Priority:** P1  
**As an** agent/reviewer  
**I want** generated `todo!` / stub method bodies reported  
**So that** “compiles” is not confused with “implements”

**Acceptance criteria:**

- When codegen would emit `todo!` / empty `// TODO: implement`, check or
  codegen emits a diagnostic (warning or error under strict mode)
- Message names the construct, method, and reason (missing body / external SDK)
- Ties to CHK-006

**Touch:** `rust.rs` adapter paths

**Done notes:** CHK-006 `escape_empty_adapter` already flags empty adapter
methods (“codegen may emit todo!()”); `--deny-escape-hatches` promotes to
error. Message names construct + method.

---

## GEN-002: AWS / external SDK adapter lowering

**Status:** Open · **Priority:** P1  
**As a** runtime platform author  
**I want** adapters that call stubbed SDKs to lower to real calls when bodies
  are authored, not `todo!("SQL: …")`  
**So that** S3/DDB adapters in `runtime.veil` can work

**Acceptance criteria:**

- Diagnose why current adapters become `todo!("SQL: …")` (misclassified as SQL?)
- Fix classification: HTTP/SDK/stub calls lower via expr translator
- At least one S3 and one DynamoDB method in runtime generates non-todo Rust
  that typechecks against real or mocked SDK
- Document how adapter bodies should be written in VEIL

**Mission impact:** Blocks runtime platform (RT-010+).

---

## GEN-003: TypeScript backend fidelity + tests

**Status:** Open · **Priority:** P2  
**As an** agent targeting TS  
**I want** TS codegen covered by tests and free of silent placeholders  
**So that** `-t ts` is a real target, not a demo

**Acceptance criteria:**

- Tests for types, services, enums, and Svelte emit paths in use
- Ranges and empty services either implemented or capability-gated (CHK-005)
- `customer_portal.veil` (or successor) generates a project that `tsc` accepts
  or fails check with clear gaps
- No `// TODO: implement` without diagnostic

---

## GEN-004: Svelte structured emit (begin raw-template retirement)

**Status:** Open · **Priority:** P2  
**As a** UI author  
**I want** props/state/derived/effects to drive generated Svelte without
  requiring all logic in raw strings  
**So that** critical UI structure is reviewable in VEIL

**Acceptance criteria:**

- Codegen from `svelte5.layer` constructs emits `$props` / `$state` /
  `$derived` / `$effect` scaffolding from structured fields
- `template` / `style` raw blocks still allowed but flagged as escape hatch
- Example component with zero raw template still produces valid `.svelte`
  shell (placeholder markup OK if explicit)

**Mission impact:** Escape-hatch debt burn-down for UI.

---

## GEN-005: Template engine completeness (pragmatic)

**Status:** Open · **Priority:** P2  
**As a** layer author  
**I want** templates to walk nested constructs and call real builtins  
**So that** pattern codegen does not require engine PRs

**Acceptance criteria:**

- `execute_templates` visits nested constructs (not only top-level items)
- Documented builtins that work: `emit_action`, and either implement or remove
  claimed `emit_struct` / `emit_fn` from docs
- Docs match implementation (`CODEGEN_TEMPLATES.md`)
- Prefer declarative templates; do not invent a third general-purpose language

---

## GEN-006: sqlx / crate special-cases leave engine

**Status:** Open · **Priority:** P2  
**As a** stub/layer author  
**I want** Cargo features and deps driven by stubs + layer metadata  
**So that** the engine does not special-case `sqlx`

**Acceptance criteria:**

- Remove hardwired sqlx feature logic from `rust.rs`
- Stub or layer declares package features/deps
- Postgres adapter examples still generate correct Cargo.toml

---

## GEN-007: Manifest contract tests

**Status:** Open · **Priority:** P1  
**As a** runtime implementer  
**I want** golden tests for `manifest.json` shape  
**So that** the compiler↔runtime handoff stays stable

**Acceptance criteria:**

- Assert handlers, deps (`adapter` vs `provided_by: runtime`), env lists
- Cover onboarding + at least one multi-context example
- Document versioning if fields evolve

**Touch:** codegen tests, `docs/ARCHITECTURE.md` if fields change
