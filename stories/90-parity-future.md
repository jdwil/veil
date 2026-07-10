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

---

## Follow-up stack (post–PAR-010)

Work surfaced while closing PAR-003–010 / spikes. Do **not** claim production
parity until these are Done.

---

## PAR-011: Swift body lowering (beyond signature spike)

**Status:** Open · **Priority:** P3  
**As a** multi-target author  
**I want** core expression/stmt subsets lowered to Swift  
**So that** `veil gen -t swift` is more than stub signatures

**Acceptance criteria:**

- Lower at least: literals, field access, binary ops, `ret`, simple `if`,
  struct construct, `match` on unit enums
- `Res!` / `?` either lower honestly or stay capability-gated with diagnostics
  (no silent `fatalError` for supported shapes)
- Example package (extend `pure_lib` or add `examples/swift_spike.veil`) builds
  with `swiftc` or documented skip
- Capability matrix updated; docs do not claim production readiness until
  integration tests pass
- Bodies that still cannot lower emit **check diagnostics** (or explicit
  escape), not only runtime `fatalError`

**Depends:** PAR-005  
**Mission impact:** Honesty of multi-target gen; avoids demo-only backends

---

## PAR-012: Kotlin body lowering (beyond signature spike)

**Status:** Open · **Priority:** P3  
**As a** multi-target author  
**I want** core expression/stmt subsets lowered to Kotlin  
**So that** `veil gen -t kotlin` is more than stub signatures

**Acceptance criteria:**

- Same expression subset bar as PAR-011 (literals, fields, ops, `ret`, `if`,
  constructs, simple `when`/`match`)
- `Result` / try path honest or capability-gated
- Example compiles with `kotlinc` or documented skip
- Capability matrix + CAPABILITIES.md updated
- Unsupported shapes → check errors, not only `TODO("…")` at runtime

**Depends:** PAR-006  
**Mission impact:** Same as PAR-011 for JVM/Android path

---

## PAR-013: Structured UI IR — layer constructs + Svelte codegen

**Status:** Open · **Priority:** P3  
**As a** UI author  
**I want** view trees as VEIL constructs that codegen to Svelte  
**So that** critical UI is not trapped in raw `template` strings

**Acceptance criteria:**

- Layer (or core) constructs: `view`/`ui`, `el`, `when`/`else`, `list … as`,
  `text` (names may match `docs/UI_IR.md`)
- Codegen path: structured tree → Svelte markup + bindings (first target)
- Migration: existing `template """…"""` still parses; CHK-006 debt remains
- At least one example package with structured UI + check green on rust/ts as
  needed; Svelte emit under generated path
- Escape hatch documented; no claim that all HTML is migrated

**Depends:** PAR-007 (design), GEN-004  
**Mission impact:** Human review of UI structure; multi-target UI honesty

---

## PAR-014: Optional `@shared` / ownership marks in source

**Status:** Open · **Priority:** P3  
**As an** author targeting Rust and GC languages  
**I want** optional sharing marks only where needed  
**So that** I never write lifetimes in `.veil`

**Acceptance criteria:**

- Parse + IR metadata for optional share mark on fields/params (syntax per
  `docs/OWNERSHIP.md`, name may be `@shared` or layer attribute)
- Default remains **owned**; unmarked portable packages unchanged
- Rust backend: `Arc` / clone policy at marked boundaries
- TS / Swift / Kotlin: ignore marks (no errors)
- Check never invents lifetime diagnostics for portable libs
- Docs + one example with a shared service-shaped field

**Depends:** PAR-004  
**Mission impact:** Semantic substrate without Rust-only noise

---

## PAR-015: Spike / partial-backend capability honesty

**Status:** Open · **Priority:** P2  
**As an** agent or CI  
**I want** capability claims to match what codegen actually emits  
**So that** claiming `TryOperator` does not hide stub bodies

**Acceptance criteria:**

- Distinguish **signature-only** vs **body-lowered** support (feature flags,
  severity tiers, or sub-capabilities)
- Swift/Kotlin spikes register honestly until PAR-011/012 land
- `veil check -t swift|kotlin` fails or warns when a supported-looking feature
  would still emit stub body
- CAPABILITIES.md table matches code
- Unit tests lock the honesty contract

**Depends:** PAR-002, PAR-005/006  
**Mission impact:** Fail closed (CHK-005); dual-loop trust

---

## PAR-016: Typed effect rows beyond `Res!` sugar (phase N)

**Status:** Open · **Priority:** P3  
**As an** engine architect  
**I want** optional effect-row IR when multi-target `?` starts to diverge  
**So that** fallibility stays semantic, not per-backend hacks

**Acceptance criteria:**

- Design delta on `docs/EFFECTS.md` / SEMANTIC_IR when work starts
- IR representation of fallible regions independent of Rust `Result` keyword
- At least two backends consume the same IR axis
- Non-goals remain: full algebraic effect handlers
- Can stay deferred if PAR-003 lowerings remain honest without it

**Depends:** PAR-003  
**Mission impact:** Long-term multi-target substrate (only if needed)
