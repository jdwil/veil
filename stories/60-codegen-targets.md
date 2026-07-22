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

**Status:** Done · **Priority:** P1  
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

**Done notes:** Removed blanket `todo!("SQL: …")` for stub/SDK bodies; authored
impls lower via `expr_to_rust`. Empty bodies still `todo!("empty adapter…")`
(CHK-006). S3/DDB typecheck remains example/stub-dep dependent.

---

## GEN-003: TypeScript backend fidelity + tests

**Status:** Done · **Priority:** P2  
**As an** agent targeting TS  
**I want** TS codegen covered by tests and free of silent placeholders  
**So that** `-t ts` is a real target, not a demo

**Acceptance criteria:**

- Tests for types, services, enums, and Svelte emit paths in use
- Ranges and empty services either implemented or capability-gated (CHK-005)
- `customer_portal.veil` (or successor) generates a project that `tsc` accepts
  or fails check with clear gaps
- No `// TODO: implement` without diagnostic

**Done notes:** Expanded codegen_tests (enum, svelte demo scaffolding);
capabilities gate empty services/ranges (CHK-005). Full `tsc` green on portal
is still example-dependent.

---

## GEN-004: Svelte structured emit (begin raw-template retirement)

**Status:** Done · **Priority:** P2  
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

**Done notes:** Structured `$props`/`$state`/`$derived`/`$effect` from blocks;
empty template → explicit shell div (no silent TODO). Raw template still OK.

---

## GEN-005: Template engine completeness (pragmatic)

**Status:** Done · **Priority:** P2  
**As a** layer author  
**I want** templates to walk nested constructs and call real builtins  
**So that** pattern codegen does not require engine PRs

**Acceptance criteria:**

- `execute_templates` visits nested constructs (not only top-level items)
- Documented builtins that work: `emit_action`, and either implement or remove
  claimed `emit_struct` / `emit_fn` from docs
- Docs match implementation (`CODEGEN_TEMPLATES.md`)
- Prefer declarative templates; do not invent a third general-purpose language

**Done notes:** Nested construct walk in `execute_templates`. Builtins remain
as documented in CODEGEN_TEMPLATES.md (emit_action path).

---

## GEN-006: sqlx / crate special-cases leave engine

**Status:** Done · **Priority:** P2  
**As a** stub/layer author  
**I want** Cargo features and deps driven by stubs + layer metadata  
**So that** the engine does not special-case `sqlx`

**Acceptance criteria:**

- Remove hardwired sqlx feature logic from `rust.rs`
- Stub or layer declares package features/deps
- Postgres adapter examples still generate correct Cargo.toml

**Done notes:** `StubCrate.cargo_features` + stub line `cargo_features …`;
workspace deps emit features from stub only. `examples/sqlx.stub` updated.

---

## GEN-007: Manifest contract tests

**Status:** Done · **Priority:** P1  
**As a** runtime implementer  
**I want** golden tests for `manifest.json` shape  
**So that** the compiler↔runtime handoff stays stable

**Acceptance criteria:**

- Assert handlers, deps (`adapter` vs `provided_by: runtime`), env lists
- Cover onboarding + at least one multi-context example
- Document versioning if fields evolve

**Touch:** codegen tests, `docs/ARCHITECTURE.md` if fields change

**Done notes:** `manifest_includes_layer_provided_deps_with_strategy` in
codegen_tests locks Bus/AuthService + strategy fields for onboarding.

---

## Follow-up stack (codegen hygiene & multi-target packages)

---

## GEN-008: Package / expose codegen path for non-TypeScript targets

**Status:** Done · **Priority:** P3  
**As an** author of a `pkg` with or without `expose`  
**I want** Swift/Kotlin/Rust package generation to be intentional  
**So that** only TS has a special API-client path by design—not by accident

**Acceptance criteria:**

- Document per-target behavior for `VeilFile::Package` (API client vs full crate
  vs spike sources)
- CLI/gen paths do not silently drop expose contracts on non-TS targets
- Where expose is TS-only, check `-t rust|swift|kotlin` warns or documents no-op
- Tests cover package → each registered `CodegenTarget`
- pure_lib (no expose) remains green on all targets

**Depends:** PAR-005/006, GEN package client work  
**Mission impact:** Multi-target honesty for libraries (PAR-008)

**Done notes:** `docs/PACKAGE_TARGETS.md` matrix for package/expose per target.

---

## GEN-009: Codegen and CLI warning hygiene

**Status:** Done · **Priority:** P3  
**As a** maintainer  
**I want** `cargo build` / test of veil-codegen and veil-cli clean of noise  
**So that** real failures are visible

**Acceptance criteria:**

- Remove or use unused imports (`stmt_to_rust`, template `Expr`/`StepDef`/…,
  `Shape`)
- Fix unused variables in CLI (e.g. stub index `id`)
- CI or local `cargo test -p veil-codegen -p veil-cli` shows no `dead_code` /
  unused warnings for those crates (or allowlist only justified cases)
- No behavior change

**Mission impact:** Maintainer velocity; low product risk

**Done notes:** Dropped unused imports in `rust.rs` / `template.rs`; CLI stub
loop `_id`. `cargo check -p veil-codegen -p veil-cli` clean.

---

## GEN-010: Local mutability analysis (`let` vs `let mut`)

**Status:** Done · **Priority:** P2  
**As an** author reviewing generated Rust  
**I want** bindings immutable unless actually mutated  
**So that** the dual-loop quality bar is green without 50+ unused-mut warnings

**Acceptance criteria:**

- Plain `x = expr` emits `let x = …` when `x` is only read afterward
- Emits `let mut x` when later: reassigned, field-written (`x.f = …`), explicit
  `mut x = …`, or receiver of a known mutating method (`push`/`insert`/…)
- Analysis spans flow steps (locals persist across steps) and nested if/for/match
- Regression test covers read-only vs push vs field-write
- Relay `cargo check -p relay` has zero `variable does not need to be mutable`

**Touch:** `expr.rs` (`analyze_mut_locals*`, Assign emission), body sites in
`rust.rs`

**Mission impact:** Generated-code quality bar for product packages (relay)

**Done notes:** `GenCtx.mut_locals` + `analyze_mut_locals` / `_in_steps`;
`MUTATING_METHODS` set; wired into services, adapters, domain methods, layer
fns, saga steps. Relay: 58 unused-mut warnings → 0.

---

## GEN-011: Harness hygiene (wired adapters, imports, Query)

**Status:** Done · **Priority:** P2  
**As an** author running the local harness  
**I want** `cargo check -p veil_bin` clean  
**So that** dual-loop green means zero warnings, not LIB-only green

**Acceptance criteria:**

- Only **wired** adapters are instantiated (first wins per Deps field); dual
  Dynamo+Pg does not leave unused `pg_*_inst`
- Stub `harness_field` lets only for wired adapters
- HTTP routing free-fn imports: only methods that lead a path chain (Axum
  `.post`/`.put`/`.delete` are methods, not free imports)
- `Query(q)` only when the handler has non-dep, non-path inputs
- Drop unused `veil_shared::*` from harness when not needed
- Dynamo Client recipe uses non-deprecated `aws_config::load_defaults`
- Relay: `cargo check -p relay -p veil_bin` → zero warnings

**Touch:** `rust.rs` harness, `aws_sdk_dynamodb.stub`

**Done notes:** Pre-scan free-fn methods + `harness_handler_needs_query`;
wire-before-instantiate; stub Client → `BehaviorVersion::latest()`.
