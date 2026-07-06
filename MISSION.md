# VEIL — Visual Engineering Intermediate Language

## Purpose

VEIL is a token-efficient, indentation-based DSL designed for AI-generated applications. It compiles to Rust and renders as an interactive visual editor in Svelte. The viewer IS the editor — users never see raw code. They interact with nodes, drag/drop, property panels, and tabs.

## Core Architecture

VEIL has three layers:

1. **Core Language** — language primitives only (struct, enum, fn, trait, impl, mod, group, match, for, while, ret, call, and expressions)
2. **Abstraction Layers** (`.layer` files) — teach the system new domain-specific constructs
3. **Application Code** (`.veil` files) — written using vocabulary from the referenced layers

## How Layers Work

A `.layer` file defines constructs that map to core primitives. For example, `ddd.layer` defines domain-driven design constructs:

```
pkg ddd v1

  construct Context
    keyword ctx
    maps_to mod
    visual
      icon "📦"
      color "#8b5cf6"
      label "Bounded Context"

  construct Aggregate
    keyword agg
    maps_to struct
    visual
      icon "🏛️"
      color "#0891b2"
      label "Aggregate Root"

  construct Port
    keyword port
    maps_to trait
    visual
      icon "🔌"
      color "#059669"
      label "Port"
```

A `.veil` file references layers via `use`:

```
sol MyApp
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
- The codegen does NOT have DDD-specific generation — it generates code based on the `maps_to` category (mod → module, struct → struct, trait → trait, etc.)
- The viewer does NOT hardcode icons, colors, or labels — it reads visual metadata from the layer file via an API

If someone creates a `crud.layer` or `ecs.layer` or `microservices.layer`, the system should work WITHOUT changing any Rust code or viewer code.

## Construct Categories

Every layer construct maps to exactly one core primitive via `maps_to`:

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
`maps_to` may also name another construct (by keyword or name), and shapes
resolve transitively — see Layer Stacking.

The full language reference (every core keyword, operator, type form, and the
`.layer`/`.stub` formats) lives in [`docs/LANGUAGE.md`](docs/LANGUAGE.md).

## Statement Types (inside `fn`-mapped constructs)

The engine knows only **2 statement shapes**: `call` (an invocation) and `if`
(a conditional/guard). Every domain statement is layer-defined and maps to one
of them. In `ddd.layer`, for example:

- `call` — direct dependency call (core)
- `dispatch` — `maps_to Bus.dispatch` (fire-and-forget event)
- `invoke` — `maps_to Bus.invoke` (command)
- `request` — `maps_to Bus.request` (inter-context query)
- `emit` — aggregate-local event collection (`maps_to call`)
- `guard` — `maps_to if` (validation/precondition)

A statement whose `maps_to` names `Port.method` **desugars** at parse time into
a `call` on that port, so `dispatch Evt{...}` becomes a `Bus.dispatch(...)`
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
  in the layer's `annotations` block)

## File Structure

```
docs/
  LANGUAGE.md            — Complete language reference (keywords, types, formats)

examples/
  base.layer             — Core primitives layer
  ddd.layer              — DDD abstraction layer (Bus, SagaStep + run_saga, ...)
  crm.layer              — Stacks on ddd.layer (proof of composability)
  customer_onboarding.veil — Example app using the DDD layer
  sales_crm.veil         — Example app using the CRM layer
  reqwest.stub           — Example external-crate stub

crates/
  veil-parser/           — Lexer + Parser
  veil-ir/               — AST, IR graph, builder, serializer, validator,
                           layer registry, structured edits
  veil-codegen/          — IR → Rust code generation (+ veil_shared crate)
  veil-cli/              — CLI (lex, parse, check, gen, emit, stub-gen, serve)

veil-viewer/             — Svelte visual editor (reads /api/palette,
                           /api/ir, /api/generated, /api/stubs; writes /api/edit)
```

## Layer Stacking

Layers compose: a construct's `maps_to` may name a core shape OR another
construct from any loaded layer (by keyword or name). A layer declares its
dependencies with `use` lines, and the `LayerRegistry` resolves `maps_to`
chains transitively at load time:

```
# crm.layer
pkg crm v1
  use ddd

  construct Lead
    keyword lead
    maps_to agg          # lead -> agg -> struct
```

Constraint checks (`only Saga`, `contains` allow-lists) follow the same
chain via an is-a relation, so a crm `Playbook` (playbook → saga) is
accepted wherever a ddd `Saga` is allowed. Statements stack the same way
(`notify` → `dispatch` → `call`).

## Current State

The zero-domain-knowledge invariant HOLDS across the whole pipeline —
lexer, parser, IR builder, **codegen**, and **viewer**. Both example
workspaces generate Rust that compiles. Implementation map:

- `veil-ir/src/layer.rs` — `LayerRegistry`: parses `.layer` files, resolves
  `maps_to` transitively, exposes constructs/statements/visuals/annotations.
  The 7 core shapes (`mod`, `struct`, `enum`, `trait`, `impl`, `fn`, `group`)
  and 2 statement shapes (`call`, `if`) are the ONLY vocabulary the engine
  knows.
- Lexer: layer keywords all lex as `Ident`; only core language/file/flow
  keywords are TokenKinds. Flow-modeling words (`step`, `par`) are NOT
  reserved — they lex as identifiers and are recognized contextually, so
  they can be used as variable names.
- Parser: one parse function per core shape, dispatched by registry lookup.
  Named sub-blocks (`root`, `state`) come from `contains` entries of the
  form `keyword: shape`. Layer statements parse into a generic `ActionExpr`,
  and `Port.method` statements desugar into `call`s.
- AST: a single generic `Construct` stamped with its shape + layer subkind,
  plus a top-level `Function` (for layer-declared code). No typed DDD structs,
  no DDD expression variants.
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
  — the "viewer IS the editor" loop.

### Codegen decisions (all keep the invariant)

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
