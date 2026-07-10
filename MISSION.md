# VEIL — Visual Engineering Intermediate Language

## Purpose

VEIL is a **token-efficient, indentation-based intermediate language** for
software that agents author and humans oversee. It compiles to real target
languages (Rust today; TypeScript/Svelte and others on the roadmap) and
presents as an interactive structural viewer/editor.

VEIL is not low-code and not “just another LLM prompt format.” It is a stable
IR with:

- a small, fixed core grammar
- **layers** that teach domain and platform vocabulary at runtime
- **codegen backends** that lower to real projects
- a **visual surface** for human review of structure and critical logic

The long-term aim is **expressiveness parity**: any program expressible in
major application languages (Rust, TypeScript/Svelte, Swift, Kotlin, …) can
be represented in VEIL and lowered with preserved semantics. That is
*expressiveness* parity — not keyword-for-keyword clones of each host
language.

## Product Intent

| Role | Responsibility |
|------|----------------|
| **Agents** (primary authors) | Write `.veil` quickly and cheaply in tokens |
| **Humans** (primary reviewers) | Approve structure and critical bodies without line-by-line LoC review |
| **Engine** | Parse, check, lower — with **zero domain knowledge** |
| **Layers** | Domain/platform vocabulary, visuals, prompts, codegen opinions |
| **Runtime** | Wire generated artifacts (Bus, adapters, deploy topology) |

### Dual feedback loops (both first-class)

| Loop | Actor | Must be fast and honest |
|------|-------|-------------------------|
| **Machine** | Agent | `parse → check (types, constraints, target capabilities) → codegen → (optional) target compile` |
| **Human** | Reviewer | Topology graph + critical expression bodies (guards, saga steps, adapters, risky paths) |

Graphics alone do not speed agents. **Diagnostics and deterministic codegen**
are half the product. Canvas review is the other half.

### Human review depth

Humans should typically review:

1. **Topology** — packages, modules/contexts, groups, constructs, ports, wiring, expose contracts, annotations
2. **Critical bodies** — guards, orchestration steps, adapter implementations, other high-risk expressions

They should **not** need to read every expression or all generated target code
for routine approval. Generated code remains available when drilling down
(performance, odd bugs, distrust). Success is *rarely* needing it, not
*never*.

Viewer UX prioritizes **read, navigate, restructure, and diff** of topology
and critical bodies. Dense expression editing may stay text or hybrid;
full click-to-build of every expression kind is not the primary human path.

## Expressiveness Parity

| Meaning | Status |
|---------|--------|
| **Expressiveness parity** — any program representable in core IR + layers; backends preserve semantics | **Mission** |
| **Surface syntax parity** — every host-language keyword has a VEIL twin | **Rejected** (explodes the core; kills the small-engine story) |
| **Idiomatic output parity** — generated code always looks hand-written | **Per-target quality bar**, not a blocker |

Escape hatches (raw template/style blocks, untyped `Json` boundaries, stub-only
calls, FFI) are **temporary debt** with a retirement plan — not a permanent
second language. Agents will dump complexity into them unless the system
surfaces that debt in diagnostics and review.

Platform and framework APIs (Svelte, SwiftUI, AWS, …) live in **layers and
stubs**, never in the engine core.

### Semantic substrate (direction)

Today the core is a rich expression AST with Rust as the primary lowering
target. Full multi-target parity requires an honest **semantic IR** backends
interpret, including axes such as:

- errors / effects (`Res!`, throws, result types)
- async model
- ownership / sharing (capabilities, not forced Rust lifetimes in source)
- concurrency bounds
- modules, packages, visibility

Each backend should declare a **capability matrix**. Unsupported constructs
fail at **check time** with actionable diagnostics — never silent wrong
codegen.

## Core Architecture

VEIL has three authoring layers:

1. **Core language** — fixed primitives: the 7 construct shapes, 2 statement
   shapes, and universal expression forms (control flow, calls, match,
   closures, await, try, casts, collections, operators, literals, …).
2. **Abstraction layers** (`.layer` files) — teach domain- or
   platform-specific constructs, statements, visuals, prompts, and codegen.
3. **Application code** (`.veil` files) — written with vocabulary from
   referenced layers.

Additionally:

- **`.stub` files** declare external crate/SDK APIs for type inference and
  codegen deps (`veil stub-gen <crate>`).
- The **viewer is the structural editor** — layer-driven palette, node graph,
  property panels; source text remains the agent-native authoring form.

### How layers work

A `.layer` file defines constructs that map to core primitives:

```
pkg ddd v1

  construct Context
    kw ctx
    mt mod
    visual
      icon "📦"
      color "#8b5cf6"
      label "Bounded Context"

  construct Aggregate
    kw agg
    mt struct
    visual
      icon "🏛️"
      color "#0891b2"
      label "Aggregate Root"

  construct Port
    kw port
    mt trait
    visual
      icon "🔌"
      color "#059669"
      label "Port"
```

A `.veil` file references layers via `use`:

```
pkg MyApp
  use ddd
  ctx Identity
    group domain
      agg Customer
        root
          id: UUID
          email: Email
      port CustomerRepo
        save(customer: Customer) -> Res!
        find(id: UUID) -> Res!<Opt<Customer>>
```

Opinionated stacks (e.g. DDD + Bus + CQRS in `ddd.layer`) are **blessed
paths**, not core law. The engine stays integration-agnostic; other layers
(plain HTTP, local libraries, UI frameworks) must remain first-class.

## The Critical Invariant

**The VEIL engine (lexer, parser, IR builder, check, codegen, viewer chrome)
must contain ZERO domain-specific knowledge.** All domain and platform
concepts come exclusively from `.layer` files loaded at runtime.

This means:

- The parser does NOT know what `ctx`, `agg`, or `port` mean — it looks up
  the layer schema (`mt`, `has`, …).
- The builder does NOT hardcode subkind strings like `"Aggregate"`.
- Codegen does NOT special-case DDD — it lowers by **shape** and executes
  **layer-declared** templates/policies.
- The viewer does NOT hardcode icons, colors, labels, or available
  annotations — those arrive via `/api/palette` from layers.

If someone creates `crud.layer`, `ecs.layer`, or `swiftui.layer`, the system
works **without** engine or viewer code changes.

### Invariant hygiene

Engine-level heuristics that encode policy by magic names (e.g. treating
annotation `dep` specially, field-name smart-constructor defaults, Bus-shaped
routing assumptions) are **invariant debt**. Prefer declaring them in
`di.layer` / `rust.layer` (or equivalent) so the engine only *executes*
rules. Do not grow new magic as targets and frameworks expand.

## Construct Categories

Every layer construct maps to exactly one core primitive via `mt`:

| maps_to | Parse shape | Contains |
|---------|-------------|----------|
| `mod`   | Block of child constructs and groups | Other constructs, groups |
| `struct` | Named type with fields | Fields, nested `fn` methods, named sub-blocks |
| `enum` | Variants, optionally with transitions | Variants; `A -> B` state transitions |
| `trait` | Interface with method signatures | Methods |
| `impl` | Implementation binding to a trait | Method bodies |
| `fn` | Flow/function with inputs and steps, or expression body | `input`, `step`/`par`, or raw body |
| `group` | Visual/organizational container | Child constructs |

The parser understands these **7 shapes** only. A `mt` may name another
construct; shapes resolve transitively (see Layer Stacking). Full parity
deepens the *semantics* of these shapes — it does not add a new shape per
paradigm.

Language reference: [`docs/LANGUAGE.md`](docs/LANGUAGE.md).  
Architecture / deploy: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).  
Codegen templates: [`docs/CODEGEN_TEMPLATES.md`](docs/CODEGEN_TEMPLATES.md).  
Layer prompts for agents: [`docs/LAYER_PROMPTS.md`](docs/LAYER_PROMPTS.md).

## Statement Types (inside `fn`-mapped constructs)

The engine knows only **2 statement shapes**: `call` and `if`. Every domain
verb is layer-defined and maps to one of them. Example from `ddd.layer`:

- Bare invocations — `Target.method(args)`
- `dispatch` — `maps_to Bus.dispatch`
- `invoke` — `maps_to Bus.invoke`
- `request` — `maps_to Bus.request`
- `emit` — aggregate-local event collection (`maps_to call`)
- `guard` — `maps_to if`

A statement whose `mt` names `Port.method` **desugars** at parse time into a
call on that port (`dispatch Evt{...}` → `Bus.dispatch(...)`) while source
and viewer keep the sugar via `CallExpr.sugar`. None of these keywords are
hardcoded in the engine.

## Visual Metadata and Review Surface

Each construct declares presentation in its layer:

```
visual
  icon "📦"
  color "#8b5cf6"
  label "Bounded Context"
```

The viewer uses `/api/palette` for:

- Node styling (color, icon)
- Palette of available constructs/statements
- Property labels and **layer-declared annotations**

Edits go through structured APIs (`POST /api/edit`): update AST →
re-serialize → validate → write → regenerate. Round-trips must stay
deterministic; noisy pretty-print churn breaks trust for agents and humans.

## Layer-Driven Codegen (Hybrid Model)

| Component | Responsibility |
|-----------|----------------|
| `lang.rs` (engine backend) | Expressions, types, project layout, builtins |
| `lang.layer` (emission policy) | Derives, conventions, target opinions |
| Domain/platform layers | Pattern-specific augmentation (`@dep`, `@main`, UI emit, …) |

The engine **may** know target-language reality (that is its job). It
**must not** know what `@dep`, `ctx`, or `dispatch` mean — those come from
layers.

```
.veil source + .layer files
       |
   Parse (Package AST)
       |
   Build IR
       |
   Analyze / check (diagnostics, constraints, capabilities)
       |
   Codegen:
     1. lang.rs backend (core shapes → target)
     2. Layer templates (augment)
     3. Section composition (e.g. multiple @main contributors)
       |
   Output (target files + manifest.json for runtime)
```

Templates use `codegen <target>`, `match`/`where`, `emit`, `emit_to`, and
`priority`. Prefer declarative hooks and strong builtins over turning the
template DSL into a third programming language. Details:
[`docs/CODEGEN_TEMPLATES.md`](docs/CODEGEN_TEMPLATES.md).

### Multi-target

The same VEIL program can lower to multiple backends. Today Rust is primary
and TypeScript is available; Swift/Kotlin and richer Svelte emission are
roadmap. Idiomatic quality is pursued per target; **semantic honesty**
(capability checks) always outranks pretty output.

```
veil gen app.veil -o ./out            # default: Rust
veil gen app.veil -o ./out -t ts      # TypeScript
```

Illustrative mappings (not the full semantic model):

| VEIL | Rust | TypeScript |
|------|------|------------|
| `Res!<T>` | `Result<T, DomainError>` | `Promise<T>` |
| `await expr` | `expr.await` | `await expr` |
| `expr?` | `expr?` | `await expr` (throws) |
| `List<T>` | `Vec<T>` | `T[]` |
| `Opt<T>` | `Option<T>` | `T \| null` |
| struct / trait / enum | `struct` / `trait` / `enum` | interfaces / unions |

Target-specific artifacts (Arc, Box, lifetimes, package layout) are codegen
concerns, not VEIL source concerns.

## Layer Stacking

Layers compose: `mt` may name a core shape or another construct. Dependencies
use `use`; `LayerRegistry` resolves chains transitively:

```
# crm.layer
pkg crm v1
  use ddd

  construct Lead
    kw lead
    mt agg          # lead -> agg -> struct
```

Constraints (`only Saga`, `has` allow-lists) follow the same is-a chain.
Statements stack (`notify` → `dispatch` → `call`).

## Design Laws

1. **Zero domain knowledge in the engine** — permanent.
2. **Agents author; humans review topology + critical bodies.**
3. **Dual loops** — machine check and human structure are both product
   requirements.
4. **Expressiveness parity** — semantic, not keyword cloning.
5. **Token efficiency** — terse forms are the standard; verbose forms are
   compatibility only.
6. **Terseness never outranks diagnostics** — bare-field inference and sugar
   must not produce silent wrongness; strict check is the agent default.
7. **Escape hatches are debt** — visible in review/diagnostics; burn down
   over time.
8. **Layers own vocabulary, visuals, prompts, and pattern codegen.**
9. **Blessed paths ≠ core** — `ddd`/`di` are defaults for service apps, not
   the only legal architecture.
10. **No silent miscompile** — unsupported target features fail at check.

## File Structure

```
docs/
  LANGUAGE.md            — Complete language reference
  ARCHITECTURE.md        — Packages, CQRS, Bus, manifest, deploy
  CODEGEN_TEMPLATES.md   — Template DSL and hybrid codegen
  LAYER_PROMPTS.md       — How to write layer prompt sections for agents

examples/                — Layers, apps, stubs (composability proofs)
layers/                  — System layers (base, ddd, di, rust, svelte5, …)
stories/                 — Living backlog (dual-loop, invariant debt, runtime, parity)

crates/
  veil-parser/           — Lexer + parser
  veil-ir/               — AST, IR, builder, serializer, validator, layers, stubs
  veil-codegen/          — Multi-target generation (rust, typescript, templates)
  veil-cli/              — lex, parse, check, gen, emit, stub-gen, serve
  veil-server/           — Editor/API server

veil-viewer/             — Svelte structural viewer/editor
runtime/                 — Runtime harness and larger self-hosted examples
```

## Current State

The zero-domain-knowledge invariant **holds** across the pipeline in spirit
and for layer vocabulary; watch invariant debt (engine heuristics) as the
system grows. Example workspaces generate Rust that compiles cleanly.
TypeScript generation and a Svelte 5 layer exist; full UI/structure parity
and additional backends are incomplete.

**File types:** top-level unit is `pkg` (`sol` is a deprecated alias).
Deployment topology is manifest + runtime, not a separate “solution” kind.

Implementation map (summary):

- `LayerRegistry` — parse layers, transitive `mt`, constructs/statements/
  visuals/annotations/prompts/stubs; engine vocabulary is 7 shapes + 2
  statement shapes only.
- Lexer — layer words are `Ident`; only core keywords are reserved. `step` /
  `par` are contextual, not reserved.
- Parser — one function per core shape; named sub-blocks from `has`;
  layer statements → `ActionExpr` with Port.method desugar; rich enums,
  patterns, match guards.
- AST — generic `Construct` (shape + subkind); ~34 expression variants;
  patterns; optional type annotations; generics on constructs.
- Check/validate — generic constraint grammar; expand toward types,
  unresolved calls, and target capabilities (agent loop).
- Codegen — shape-only switches; real behavior (not empty stubs); layer
  templates augment; `manifest.json` for runtime wiring.
- Viewer — layer-driven palette and styles; structured edit API; dual-mode
  chrome. Invest next in topology/critical-body **review** quality.

### Notable codegen behaviors (keep invariant pressure high)

- **`@dep` routing** — fields annotated `dep` (layer-defined) collect into a
  generated `Deps` struct; calls route through deps. Prefer making this
  fully layer-policy-driven over engine magic.
- **Smart constructors** — defaults from types/names (`Opt` → `None`,
  timestamps, scalars, `id`). Treat name heuristics as policy debt to move
  into layers where possible.
- **JSON message Bus** — cross-context payloads as `Json` so crates do not
  share domain types; `veil_shared` holds Bus/error shared surface when
  using that pattern.
- **Sagas in the layer** — `runtime` bindings + `declare`d coordinators in
  VEIL; engine has zero saga control-flow knowledge.
- **Layer-declared code** — `declare` blocks inject real shared functions/
  traits (e.g. saga runner) authored in VEIL.
- **Composability proof** — `examples/crm.layer` on `ddd.layer` and
  `examples/sales_crm.veil` generate compiling Rust with no engine changes.

## Strategic Sequencing

Not a sprint plan — product order of operations:

1. **Dual-loop excellence on the current surface** — world-class `check` +
   deterministic codegen; topology and critical-body review UX; Rust primary,
   TS/Svelte secondary with honest capabilities.
2. **Semantic IR hardening** — effects, errors, async, ownership
   capabilities; purge engine domain heuristics into layers.
3. **Parity by program class** — portable application logic → services/
   adapters → structured UI (retire raw templates) → more backends →
   library-quality modules.
4. **Escape-hatch debt burn-down** — measure and reduce raw/stub/untyped
   surface in real trees (`examples/`, `runtime/`).

## Success Measures

- Agent tokens (or steps) per feature vs raw target languages
- Human time-to-approve a structural change without opening generated LoC
- Share of reviews completed at topology + critical bodies only
- Agent fix-cycle time under `veil check` / compile feedback
- Compile/success rate of agent-authored VEIL
- Escape-hatch surface area trend (should fall over time)
- Target capability violations caught at check (not in production)
