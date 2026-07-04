# VEIL — Visual Engineering Intermediate Language

## Purpose

VEIL is a token-efficient, indentation-based DSL designed for AI-generated applications. It compiles to Rust and renders as an interactive visual editor in Svelte. The viewer IS the editor — users never see raw code. They interact with nodes, drag/drop, property panels, and tabs.

## Core Architecture

VEIL has three layers:

1. **Core Language** — language primitives only (struct, enum, fn, trait, impl, let, mod, if, match, ret, expressions)
2. **Abstraction Layers** (`.layer` files) — teach the system new domain-specific constructs
3. **Application Code** (`.veil` files) — written using vocabulary from the referenced layers

## How Layers Work

A `.layer` file defines constructs that map to core primitives. For example, `ddd.layer` defines domain-driven design constructs:

```
layer ddd

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
use ddd

sol MyApp
  ctx Identity
    agg Customer
      root
        id: UUID
        email: Email
    port CustomerRepo
      save(customer: Customer)
      find(id: UUID) -> Opt!<Customer>
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
| `struct` | Named type with fields | Fields (name: type) |
| `trait` | Interface with method signatures | Methods |
| `impl` | Implementation binding to a trait | Method implementations |
| `fn` | Flow with inputs and steps | Input block, step blocks with expressions |

The parser only needs to understand these 5 shapes. When it encounters a keyword, it looks up which shape to use from the loaded layer schema.

## Statement Types (inside `fn`-mapped constructs)

Steps within flows/sagas contain statements with DDD-style semantics. These are also layer-defined:

- `call` — direct dependency call
- `dispatch` — event bus (fire-and-forget)
- `invoke` — command bus (request processing)
- `request` — inter-context query
- `guard` — validation/precondition check

These keywords should also be layer-defined, not hardcoded.

## Visual Metadata

Each construct in a `.layer` file declares its visual representation:

```
visual
  icon "📦"
  color "#8b5cf6"
  label "Bounded Context"
```

The viewer reads this from the `/api/palette` endpoint (which parses the layer files) and uses it for:
- Node styling (background color, icon)
- Palette sidebar (what constructs are available to drag onto the canvas)
- Property editor labels

## File Structure

```
examples/
  ddd.layer              — DDD abstraction layer
  base.layer             — Core primitives layer
  customer_onboarding.veil — Example app using DDD layer

crates/
  veil-parser/           — Lexer + Parser
  veil-ir/               — AST, IR graph, builder, serializer, validator
  veil-codegen/          — IR → Rust code generation
  veil-cli/              — CLI commands (lex, parse, check, gen, emit, serve)

veil-viewer/             — Svelte visual editor
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

The zero-domain-knowledge invariant HOLDS. Implementation map:

- `veil-ir/src/layer.rs` — `LayerRegistry`: parses `.layer` files, resolves
  `maps_to` transitively, exposes constructs/statements/visuals. The 7 core
  shapes (`mod`, `struct`, `enum`, `trait`, `impl`, `fn`, `group`) and 2
  statement shapes (`call`, `if`) are the ONLY vocabulary the engine knows.
- Lexer: layer keywords all lex as `Ident`; only core language/file/flow
  keywords are TokenKinds.
- Parser: one parse function per core shape, dispatched by registry lookup.
  Named sub-blocks (`root`, `state`) come from `contains` entries of the
  form `keyword: shape`. Layer statements parse into a generic `ActionExpr`.
- AST: a single generic `Construct` stamped with its shape + layer subkind.
  No typed DDD structs, no DDD expression variants.
- Builder/serializer/codegen: switch on shape only; subkind is metadata.
- Validation: generic constraint grammar (`only X`, `deny X`,
  `must_have <block>`, `requires_groups`); unknown constraint words are
  semantic hints, skipped by the structural validator.
- Viewer: `NODE_STYLES` covers core shapes only; all layer visuals arrive at
  runtime via `/api/palette` and register through `setPaletteStyles()`.
- Proof of composability: `examples/crm.layer` stacks on `ddd.layer`
  (`pipeline→ctx→mod`, `lead→agg→struct`, `notify→dispatch→call`) and
  `examples/sales_crm.veil` parses, validates, and generates compiling Rust
  without any engine changes.
