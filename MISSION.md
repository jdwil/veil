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

## Current State vs Goal

### Goal
Zero DDD knowledge in Rust code. The system is a generic language workbench that learns vocabulary from `.layer` files.

### Current Reality
The Rust code still contains DDD-specific:
- Lexer TokenKinds (Ctx, Agg, Ent, Val, Evt, Cmd, Port, Adapter, Saga, Orchestrator, Svc)
- A `token_kind_to_keyword()` function mapping tokens to DDD keyword strings
- A `default_keyword_categories()` fallback map with all DDD keywords hardcoded
- A `capitalize()` function mapping DDD keywords to display names
- Old typed AST structs (ValueObject, Entity, Aggregate, Port, Service, Adapter) still exist and are used internally by the parser before conversion to the generic Construct type
- The parser has separate `parse_value_object()`, `parse_entity()`, `parse_aggregate()`, `parse_port()`, `parse_adapter()`, `parse_saga()`, `parse_domain_service()` functions — one per DDD concept
- The builder's `capitalize()` function hardcodes the keyword→display name mapping
- The viewer's `SUBKIND_STYLES` and `types.ts` hardcode DDD icons/colors
