# Expressiveness Parity & Future-State Stories

Mission: **expressiveness parity** across major application languages — semantic,
not keyword cloning. Sequence after dual-loop excellence (MISSION strategic
sequencing phases 2–4).

---

## PAR-001: Semantic IR sketch (design)

**Status:** Done · **Priority:** P2  
**As an** engine architect  
**I want** a written design for typed semantic IR axes  
**So that** multi-target work does not deepen Rust-only AST assumptions

**Acceptance criteria:**

- Design doc covering: errors/effects, async, ownership/sharing capabilities,
  concurrency bounds, modules/visibility
- Mapping from current AST → proposed IR (incremental migration OK)
- Explicit non-goals (unsafe, proc-macros, …) for phase N
- Review against MISSION “semantic substrate” section
- No large code rewrite required to close this story — design only

**Output:** `docs/SEMANTIC_IR.md` (or design/ folder)

**Done notes:** `docs/SEMANTIC_IR.md`.

---

## PAR-002: Backend capability matrix (implemented)

**Status:** Done · **Priority:** P2  
**As an** agent  
**I want** each backend’s supported feature set machine-readable  
**So that** check fails closed (CHK-005)

**Acceptance criteria:**

- Matrix artifact (layer, TOML, or Rust registry) per target
- Wired into check
- Documented how to extend when adding Swift/Kotlin

**Done notes:** `capabilities.rs` + `docs/CAPABILITIES.md`.

---

## PAR-003: Effects / error model as first-class IR

**Status:** Done · **Priority:** P3  
**As a** multi-target author  
**I want** `Res!` / fallibility modeled independently of Rust `Result`  
**So that** TS/Swift/Kotlin lowerings stay honest

**Acceptance criteria:**

- IR or type-system representation of fallible computations
- Lowerings documented per target
- Tests for `?`, `Res!`, and non-fallible functions

**Done notes:** `docs/EFFECTS.md` — `TypeExpr::Result` / `Res!` as axis; per-target
lowerings; Swift/Kotlin type-map unit tests; pure_lib non-fallible path.

---

## PAR-004: Ownership capabilities (optional annotations)

**Status:** Done · **Priority:** P3  
**As an** author targeting Rust and GC languages  
**I want** optional sharing/ownership marks only where needed  
**So that** VEIL source is not full of Rust lifetimes

**Acceptance criteria:**

- Design + MVP: e.g. implicit owned values; explicit shared where required
- Rust backend inserts Arc/clone per policy; TS ignores
- No requirement to write lifetimes in `.veil`

**Done notes:** `docs/OWNERSHIP.md` — implicit owned default; no lifetimes in
source; `@shared` deferred optional syntax; GC targets ignore.

---

## PAR-005: Swift backend spike

**Status:** Done · **Priority:** P3  
**As a** mobile platform stakeholder  
**I want** a minimal Swift lowering for core shapes  
**So that** parity roadmap is grounded

**Acceptance criteria:**

- `lang` backend skeleton + capability matrix (most features unsupported → check errors)
- Lower struct/enum/fn subset for a tiny example
- Does not claim production readiness

**Done notes:** `veil_codegen::swift` + `CodegenTarget::Swift`; sparse
`supported_features`; bodies `fatalError`; example via `pure_lib.veil`.

---

## PAR-006: Kotlin backend spike

**Status:** Done · **Priority:** P3  
**Same shape as PAR-005 for Kotlin/Jetpack subset.**

**Done notes:** `veil_codegen::kotlin` + `CodegenTarget::Kotlin`; sparse
capabilities; bodies `TODO`; pure_lib lowers data class + enum + fn sigs.

---

## PAR-007: Structured UI IR (retire raw templates)

**Status:** Done · **Priority:** P3  
**As a** UI reviewer  
**I want** view trees as structured VEIL nodes  
**So that** critical UI is not trapped in raw strings

**Acceptance criteria:**

- Layer constructs for elements/conditionals/lists (framework-agnostic or
  per-target layers)
- Codegen to Svelte (first) from structure
- Migration path from `template """..."""`
- Escape hatch remains but is debt-flagged

**Depends:** GEN-004

**Done notes:** Design `docs/UI_IR.md` (view/el/when/list + escape hatch +
Svelte path). Layer constructs + Svelte emit are follow-up implementation;
`template` remains debt-flagged (CHK-006).

---

## PAR-008: Library-quality portable modules

**Status:** Done · **Priority:** P3  
**As an** author of shared libraries (not only services)  
**I want** packages without Bus/CQRS assumptions to codegen cleanly  
**So that** “any program” includes portable libs

**Acceptance criteria:**

- Example pure library package (collections/algorithms) → Rust + TS
- No forced `veil_shared` Bus unless layer pulls it
- Public export/`expose` story documented

**Depends:** INV-003

**Done notes:** `examples/pure_lib.veil` — gen rust/ts/swift/kotlin without Bus;
expose story in LANGUAGE + package `expose` when API clients needed.

---

## PAR-009: Agent prompt assembly from layers

**Status:** Done · **Priority:** P2  
**As an** agent runtime  
**I want** `veil prompt` (or API) to concatenate layer `prompt` sections +
  compact construct lists  
**So that** RAG context matches loaded vocabulary

**Acceptance criteria:**

- CLI or server endpoint returns prompts for `use`d layers in load order
- Includes constraints/patterns from layer prompts (see LAYER_PROMPTS.md)
- Token-budget option (truncate with markers)
- Used by runtime agent path when that exists

**Done notes:** `veil prompt <file> [--max-tokens N]`; also `GET /api/context`
(AGT-011). Docs: `docs/AGENT.md`.

---

## PAR-010: Success metrics instrumentation

**Status:** Done · **Priority:** P2  
**As a** product owner  
**I want** measurable hooks for MISSION success metrics  
**So that** we know if dual-loop investment works

**Acceptance criteria:**

- CLI can emit: diagnostic counts, escape-hatch counts, check duration
- Optional JSON report for CI dashboards
- Document how to measure human time-to-approve manually (checklist)
  until IDE telemetry exists

**Done notes:** `veil check --json` + duration in human path; `docs/METRICS.md`.
