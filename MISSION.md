# VEIL — Visual Engineering Intermediate Language

## Purpose

VEIL is a token-efficient, indentation-based DSL designed for AI-generated applications. It compiles to Rust and renders as an interactive visual editor in Svelte. The viewer IS the editor — users never see raw code. They interact with nodes, drag/drop, property panels, and tabs.

## Core Architecture

VEIL has three layers:

1. **Core Language** — every Rust primitive: struct, enum, fn, trait, impl, mod,
   if/else, match, for, while, loop, break, continue, closures, let/mut, return,
   await, try(?), cast, index, range, arrays, tuples, operators, and literals.
2. **Abstraction Layers** (`.layer` files) — teach the system new domain-specific constructs
3. **Application Code** (`.veil` files) — written using vocabulary from the referenced layers

Additionally:
- **`.stub` files** declare external Rust crate APIs so adapters can call them
  with full type inference. Generated automatically via `veil stub-gen <crate>`.
- The **viewer IS the editor** — every expression, type, and construct is
  editable through composable visual form components. No raw code editing.

## How Layers Work

A `.layer` file defines constructs that map to core primitives. For example, `ddd.layer` defines domain-driven design constructs:

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

## The Critical Invariant

**The VEIL system (lexer, parser, IR builder, codegen, viewer) must contain ZERO domain-specific knowledge.** All domain concepts come exclusively from `.layer` files loaded at runtime.

This means:
- The parser does NOT know what "ctx", "agg", "port" mean — it reads the layer file to learn that "ctx" maps to "mod" (a module-shaped construct with children), "agg" maps to "struct" (a struct-shaped construct with fields), etc.
- The builder does NOT hardcode subkind strings like "Context" or "Aggregate" — it reads the construct name from the layer schema
- The codegen does NOT have DDD-specific generation — it generates code based on the `mt` category (mod → module, struct → struct, trait → trait, etc.)
- The viewer does NOT hardcode icons, colors, or labels — it reads visual metadata from the layer file via an API

If someone creates a `crud.layer` or `ecs.layer` or `microservices.layer`, the system should work WITHOUT changing any Rust code or viewer code.

## Construct Categories

Every layer construct maps to exactly one core primitive via `mt`:

| maps_to | Parse shape | Contains |
|---------|-------------|----------|
| `mod`   | Block of child constructs and groups | Other constructs, groups |
| `struct` | Named type with fields | Fields (name: type), nested `fn` methods, named sub-blocks |
| `enum` | Named set of variants, optionally with transitions | Variants; `A -> B` records state transitions |
| `trait` | Interface with method signatures | Methods |
| `impl` | Implementation binding to a trait | Method implementations (expression bodies) |
| `fn` | Flow/function with inputs and steps, or a code function with an expression body | Input block, step/par blocks, or a raw body |
| `group` | Visual/organizational container | Child constructs |

The parser only needs to understand these **7 shapes**. When it encounters a
keyword, it looks up which shape to use from the loaded layer schema. A
`mt` may also name another construct (by keyword or name), and shapes
resolve transitively — see Layer Stacking.

The full language reference (every core keyword, operator, type form, and the
`.layer`/`.stub` formats) lives in [`docs/LANGUAGE.md`](docs/LANGUAGE.md).

Architecture decisions, CQRS patterns, and deployment model are in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Statement Types (inside `fn`-mapped constructs)

The engine knows only **2 statement shapes**: `call` (an invocation) and `if`
(a conditional/guard). Every domain statement is layer-defined and maps to one
of them. In `ddd.layer`, for example:

- Bare invocations — `Target.method(args)` (no keyword needed)
- `dispatch` — `maps_to Bus.dispatch` (fire-and-forget event)
- `invoke` — `maps_to Bus.invoke` (command)
- `request` — `maps_to Bus.request` (inter-context query)
- `emit` — aggregate-local event collection (`maps_to call`)
- `guard` — `maps_to if` (validation/precondition)

A statement whose `mt` names `Port.method` **desugars** at parse time into
a call on that port, so `dispatch Evt{...}` becomes a `Bus.dispatch(...)`
call while the source and viewer keep the `dispatch` sugar (via
`CallExpr.sugar`). None of these keywords are hardcoded in the engine.

## Visual Metadata

Each construct in a `.layer` file declares its visual representation:

```
visual
  icon "📦"
  color "#8b5cf6"
  label "Bounded Context"
```

The viewer reads this from the `/api/palette` endpoint (which parses the layer
files) and uses it for:
- Node styling (background color, icon)
- Palette sidebar (what constructs are available to drag onto the canvas)
- Property editor labels **and available annotations** (declared per construct
  in the layer's `ann` block)

## Layer-Driven Codegen Templates

Code generation in VEIL uses a **hybrid architecture**: each language target
has a compiler backend (`lang.rs`) for expression translation, type mapping,
and project layout, plus an emission policy layer (`lang.layer`) for
target-specific opinions and conventions.

Domain layers (`di.layer`, `ddd.layer`, etc.) add their own `codegen <target>`
blocks that **augment** the compiler backend's output with pattern-specific
code — they don't replace it.

### The Split

| Component | What it does | Changes when... |
|-----------|--------------|-----------------|
| `rust.rs` (engine) | Expressions, types, project layout, builtins | Rust language evolves |
| `rust.layer` (layer) | Derives, conventions, smart constructors | Project preferences change |
| `di.layer` (layer) | `@dep` constructors, `@main` composition | DI approach changes |

### How Templates Work

A `.layer` file includes `codegen <target>` blocks that declare templates:

```
layer di
  codegen rust
    match struct where has_annotation("dep")
      emit """
        impl {{name}} {
            pub fn new({{for field in dep_fields}}{{field.name}}: {{field.type}}{{sep ", "}}{{end}}) -> Self {
                Self { {{for field in dep_fields}}{{field.name}}{{sep ", "}}{{end}} }
            }
        }
      """

    match fn where has_annotation("main")
      emit_to "main" priority 50
      emit """
        {{for step in steps}}{{for action in step.actions}}
        {{emit_action(action)}}
        {{end}}{{end}}
      """
```

### Key Concepts

| Concept | Purpose |
|---------|---------|
| `codegen <target>` | Block scoped to a target language (rust, typescript, swift, kotlin) |
| `match <kind> where <condition>` | Pattern matching against the IR |
| `emit """..."""` | Template literal with interpolation |
| `{{for x in collection}}` | Iteration over IR elements |
| `{{name}}`, `{{field.type}}` | Access to node properties |
| `emit_to "section"` | Contribute output to a named composable section |
| `priority <n>` | Ordering when multiple templates target the same section |
| `emit_action(action)` | Call into base codegen's built-in emitters |

### Multi-Target Support

The same layer can provide templates for multiple targets:

```
layer ui
  codegen swift
    match struct where subkind == "Screen"
      emit """
        struct {{name}}: View { ... }
      """

  codegen kotlin
    match struct where subkind == "Screen"
      emit """
        @Composable fun {{name}}() { ... }
      """
```

This enables: write domain logic once in VEIL, layers define the mapping,
same source produces Swift, Kotlin, Rust, or TypeScript output depending on
the target.

### Composition

Multiple layers can contribute to the same output sections. For example,
`di.layer` contributes dependency wiring to "main", while `http.layer`
contributes server startup. The engine composes them by priority order.

If a programmer writes their own `main` fn in the `.veil` source, the
auto-generated main is suppressed. Diagnostics warn if required `@main`
contributors are not called in a custom main.

### The Pipeline

```
.veil source + .layer files
       |
   Parse (Package AST)
       |
   Build IR (nodes, edges, metadata)
       |
   Analyze (diagnostics, dep graph resolution)
       |
   Codegen:
     1. lang.rs compiler backend (expressions, types, layout)
     2. Layer templates execute against IR (augment output)
     3. Sections compose (multiple @main contributors → one main())
       |
   Output (target files + manifest.json for runtime)
```

The codegen phase receives the full AST and `LayerRegistry`. The
compiler backend (`rust.rs`) handles core shape emission and project structure.
Layer templates then execute, adding domain-specific code (DI wiring, pattern
implementations). Section contributions are composed by priority. The result
is the final target files plus a `manifest.json` that describes the module's
wiring requirements for veil-runtime (deps, handlers, env vars).

## File Structure

```
docs/
  LANGUAGE.md            — Complete language reference (keywords, types, formats)

examples/
  base.layer             — Core primitives layer
  ddd.layer              — DDD abstraction layer (Bus, SagaStep + run_saga, ...)
  crm.layer              — Stacks on ddd.layer (proof of composability)
  functional.layer       — FP abstractions (pure, adt, pipe, typeclasses, effects)
  customer_onboarding.veil — Example app using the DDD layer
  sales_crm.veil         — Example app using the CRM layer
  reqwest.stub           — Example external-crate stub

stories/                 — User stories for UX features

crates/
  veil-parser/           — Lexer + Parser (comprehensive Rust expression coverage)
  veil-ir/               — AST, IR graph, builder, serializer, validator,
                           layer registry (with .stub parsing)
  veil-codegen/          — Multi-target code generation:
                             rust.rs       — IR → Rust (primary target)
                             typescript.rs — IR → TypeScript
                             expr.rs       — Rust expression translator
  veil-cli/              — CLI (lex, parse, check, gen --target, emit, stub-gen, serve)

veil-viewer/             — Svelte visual editor
  src/lib/editors/       — Composable expression editing components:
    ExprEditor.svelte    — Recursive editor for all 34 expression kinds
    BlockEditor.svelte   — Reusable expression list (bodies, arms, etc.)
    ExprPicker.svelte    — Searchable dropdown to add expressions
    TypeEditor.svelte    — Recursive type annotation editor
    EnumEditor.svelte    — Variant + transition editor
    ConstructEditor.svelte — Unified construct editor (dispatches by shape)
    AnnotationEditor.svelte — @annotation toggle + param editor
    expr-types.ts        — Full Expr/TypeExpr type system for the UI
    ir-convert.ts        — IR nodes → Expr trees for editing
    expr-serialize.ts    — Expr trees → VEIL source text
```

## Transpilation Design

VEIL core expressions are **universal programming primitives** — they map to
both Rust and TypeScript (and potentially other targets). The AST is
target-agnostic; only the codegen backend decides how to lower:

| VEIL | Rust | TypeScript |
|------|------|-----------|
| `Res!<T>` | `Result<T, DomainError>` | `Promise<T>` |
| `mut x = 0` | `let mut x = 0;` | `let x = 0;` |
| `mut x: Int = 0` | `let mut x: i64 = 0;` | `let x: number = 0;` |
| `await expr` | `expr.await` | `await expr` |
| `expr?` | `expr?` | `await expr` (throws) |
| `.clone()` | emitted as needed | skipped |
| `List<T>` | `Vec<T>` | `T[]` |
| `Opt<T>` | `Option<T>` | `T \| null` |
| `Map<K,V>` | `HashMap<K, V>` | `Map<K, V>` |
| struct | `pub struct` | `export interface` |
| trait | `#[async_trait] pub trait` | `export interface` (async methods) |
| enum (data) | `pub enum { Variant(T) }` | discriminated union |

Rust-specific artifacts (Arc, Box, lifetimes, .await placement) are codegen
concerns, not source-level. The TypeScript backend reads the same AST and
produces typed interfaces, async service functions, and project scaffolding
(`package.json`, `tsconfig.json`).

**CLI usage:**
```
veil gen app.veil -o ./out            # default: Rust
veil gen app.veil -o ./out -t ts      # TypeScript
```

## Layer Stacking

Layers compose: a construct's `mt` may name a core shape OR another
construct from any loaded layer (by keyword or name). A layer declares its
dependencies with `use` lines, and the `LayerRegistry` resolves `mt`
chains transitively at load time:

```
# crm.layer
pkg crm v1
  use ddd

  construct Lead
    kw lead
    mt agg          # lead -> agg -> struct
```

Constraint checks (`only Saga`, `has` allow-lists) follow the same
chain via an is-a relation, so a crm `Playbook` (playbook → saga) is
accepted wherever a ddd `Saga` is allowed. Statements stack the same way
(`notify` → `dispatch` → `call`).

## Current State

The zero-domain-knowledge invariant HOLDS across the whole pipeline —
lexer, parser, IR builder, **codegen** (both Rust and TypeScript), and
**viewer**. All example workspaces generate Rust that compiles cleanly.

**File types:** The only top-level keyword is `pkg`. The `sol` keyword is
accepted as a deprecated alias (produces identical output). There is no
separate "solution" vs "package" distinction — `pkg` is the unit of domain
modeling, and deployment topology is determined by the manifest + runtime.

Implementation map:

- `veil-ir/src/layer.rs` — `LayerRegistry`: parses `.layer` files, resolves
  `mt` transitively, exposes constructs/statements/visuals/annotations.
  The 7 core shapes (`mod`, `struct`, `enum`, `trait`, `impl`, `fn`, `group`)
  and 2 statement shapes (`call`, `if`) are the ONLY vocabulary the engine
  knows. `routing_traits()` identifies the Bus-like ports generically.
- Lexer: layer keywords all lex as `Ident`; only core language/file/flow
  keywords are TokenKinds. Flow-modeling words (`step`, `par`) are NOT
  reserved — they lex as identifiers and are recognized contextually, so
  they can be used as variable names.
- Parser: one parse function per core shape, dispatched by registry lookup.
  Named sub-blocks (`root`, `state`) come from `has` entries of the
  form `keyword: shape`. Layer statements parse into a generic `ActionExpr`,
  and `Port.method` statements desugar into `call`s. Rich enum variants
  (`Variant(Type)`, struct variants) parsed into `EnumVariant`. Destructuring
  patterns (`(a, b) = expr`) parsed into `LetPattern` with structured `Pattern`.
  Match guards (`pattern if condition -> body`) supported.
- AST: a single generic `Construct` stamped with its shape + layer subkind,
  plus a top-level `Function` (for layer-declared code). 34 expression
  variants covering all Rust expression forms. `Pattern` enum for structured
  destructuring. `EnumVariant` for data-carrying enum variants. Optional type
  annotations on let bindings. Generic type parameters on constructs.
- Builder/serializer/codegen: switch on shape only; subkind is metadata.
  Codegen emits **real behavior**, not stubs — aggregate methods, adapter
  impls, guards that enforce, and a JSON message Bus (see below).
- Validation: generic constraint grammar (`only X`, `deny X`,
  `must_have <block>`, `requires_groups`); unknown constraint words are
  semantic hints, skipped by the structural validator.
- Viewer: fully layer-driven. `NODE_STYLES` covers core shapes only; all
  layer visuals AND the available annotations arrive at runtime via
  `/api/palette`. It is an **editor**: `POST /api/edit` applies structured
  edits to the AST, re-serializes, validates, writes the file, and regenerates
  — the "viewer IS the editor" loop. Dark/light mode toggle.

### Codegen decisions (all keep the invariant)

- **`@dep` annotation routing.** When a `fn`-shaped construct's input field
  carries an annotation whose name is `dep` (defined by a layer, e.g.
  `di.layer`), the engine excludes it from the generated function's parameter
  list. Instead, all `@dep`-annotated fields are collected into a generated
  `Deps` struct and calls to those fields route through `deps.field.method().await?`.
  The engine recognizes this pattern generically via the field's annotation +
  its type resolving to `Shape::Trait`.

- **Smart constructors for struct-shaped constructs.** The codegen's `new()`
  generator applies generic heuristics to determine which fields become
  parameters vs auto-defaulted:
  - Fields whose type is `Optional` or `Opt<T>` default to `None`
  - Fields named after common timestamps (`created_on`, `updated_on`, etc.) default to `Utc::now()` or `None` if optional
  - Fields typed `Int`, `Bool`, `F64` get scalar defaults (0, false, 0.0)
  - Fields typed `Json` default to `{}`
  - The `id` field is always a parameter; the expression translator auto-inserts
    `Uuid::new_v4()` when the caller omits it
  - Constructs with `@invariant` annotations return `Result<Self, ValidationError>`
  These heuristics are engine-level (driven by type names and field names),
  not domain-specific. Any layer's struct-shaped constructs benefit from them.

- **JSON message Bus.** Cross-context calls route through a `Bus` whose payloads
  are `Json` (`serde_json::Value`), so an orchestrator crate never depends on
  another context's concrete types. `Bus`, `DomainError`, and shared traits
  live in a single generated `veil_shared` crate that every context re-exports.
- **Sagas are defined in the layer, not the engine.** A layer construct may
  declare a `runtime` binding (`runtime run_saga SagaStep` +
  `compensate -> compensate`). Codegen then lowers each authored `step` into a
  generated `struct` + `impl SagaStep` (action from the body, compensate from
  the sub-block, Bus injected, inputs captured as fields), collects them into a
  `Vec<Box<dyn SagaStep>>`, and calls the layer-declared coordinator. The
  coordinator (`run_saga`/`unwind` — forward-run, then reverse-unwind on
  failure, threading a shared JSON `state` so later steps see earlier results)
  is written in **VEIL in `ddd.layer`'s `declare` block**. `rust.rs` contains
  zero saga control flow.
- **Layer-declared code.** A `fn` with a body in a layer's `declare` block
  generates a real function in `veil_shared`. This — plus first-class trait
  objects (`List<Trait>` → `Vec<Box<dyn Trait>>`) — is what lets the saga
  runtime be authored entirely in VEIL.

- Proof of composability: `examples/crm.layer` stacks on `ddd.layer`
  (`pipeline→ctx→mod`, `lead→agg→struct`, `notify→dispatch→call`) and
  `examples/sales_crm.veil` parses, validates, and generates compiling Rust
  without any engine changes.
