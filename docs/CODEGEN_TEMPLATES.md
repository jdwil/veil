# Codegen Template DSL

## Overview

The codegen template DSL allows `.layer` files to declare how their patterns
transform into target language code. Templates handle **domain-specific** and
**opinionated** code generation; the core compiler backend handles expression
translation, type mapping, and project layout.

## Architecture: The Hybrid Model

Each language target has two components:

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `lang.rs` | Engine (`veil-codegen/src/`) | Compiler backend: expression translation, type mapping, project/workspace layout, built-in emitters |
| `lang.layer` | Layer file (`layers/`) | Emission policy: derives, conventions, decorators, target-specific templates that call builtins |
| roles / policies | `.layer` blocks | INV-001: engine matches `role:*` and `*_policy` blocks — never annotation spellings; see [`POLICY_ROLES.md`](./POLICY_ROLES.md) |

Additionally, domain layers (`di.layer`, `ddd.layer`, etc.) add their own
`codegen <target>` blocks that augment the output with domain-specific patterns.

### What lives where

| Concern | Location | Reason |
|---------|----------|--------|
| `Expr` → target syntax | `lang.rs` | Complex compilation logic, ~1200 lines of expression routing |
| Type mapping | `lang.rs` | Target-specific type system knowledge |
| Project/workspace layout | `lang.rs` | Structural orchestration (which files, imports, module system) |
| Built-in emitters (`emit_struct`, etc.) | `lang.rs` | Heavy lifting that templates call into |
| `#[derive(...)]` macros on structs | `rust.layer` | Opinionated convention, easily changed |
| Smart constructor patterns | `rust.layer` | Team-specific preference |
| `@dep` constructor generation | `di.layer` | Domain pattern |
| `@main` section composition | `di.layer` | Domain pattern |
| SwiftUI view emission | `swiftui.layer` | Target-specific UI framework |

### The Critical Invariant

The invariant is **zero domain knowledge in the engine**, not zero target
knowledge. The engine CAN know what Rust/TypeScript/Swift syntax looks like
(that's its job as a compiler backend). It CANNOT know what `@dep`, `ctx`,
`agg`, or `dispatch` mean — those come exclusively from layers.

## Motivation

The template DSL enables:
- `rust.layer` defines Rust-specific opinions (derives, async patterns)
- `ddd.layer` defines how DDD patterns emit (adapters, ports, services)
- `di.layer` defines how DI patterns emit (constructors, wiring, main)
- `swiftui.layer` defines how UI patterns emit SwiftUI code
- `jetpack.layer` defines how UI patterns emit Jetpack Compose code
- Community layers can ship their own codegen without engine changes

Adding a new **language target** requires a `lang.rs` (compiler backend) plus
a `lang.layer` (emission policy). Adding a new **domain pattern** requires
only a layer file with `codegen` blocks — no engine changes.

## Syntax

### Block Structure

```
layer <name>
  codegen <target>
    match <shape> where <condition>
      emit """
        <template>
      """
```

### Targets

A target is a language identifier: `rust`, `typescript`, `swift`, `kotlin`, etc.
Each `codegen` block is scoped to one target. A layer can have multiple
`codegen` blocks for different targets.

### Match Patterns

```
match struct                           # all structs
match struct where has_role("dependency") # any annotation with role:dependency (INV-001)
match fn where has_role("main")           # role:main composition contributors
match struct where has_annotation("dep")  # literal name (layer self-reference only)
match impl where target == "Pool"         # impls targeting Pool
match struct where field_typed("Pool")    # structs with a Pool-typed field
```

### Template Interpolation

```
{{name}}                    # construct name
{{field.name}}              # field property access
{{field.type}}              # field type
{{route}}                   # role:http_route first arg (or /name_lower)
{{annotation_value:name}}   # first arg of @name (quotes stripped)
{{annotation_arg:name:N}}   # Nth arg (0-based) of @name
{{annotation_value("name")}}  # same as annotation_value:name
{{annotation_arg("name", N)}} # same as annotation_arg:name:N
{{for field in fields}}...{{end}}   # iteration
{{for step in steps}}...{{end}}     # iterate steps
{{for action in step.actions}}...{{end}}  # nested iteration
{{sep ", "}}                # separator between iterations
{{if condition}}...{{end}}  # conditional
{{emit_action(action)}}     # call base emitter
{{emit_struct(node)}}       # call base emitter
{{emit_fn(node)}}           # call base emitter
```

`emit_file` for the same path **replaces** an earlier scaffold/emit (e.g. bare
`vite.config.ts` scaffold overridden when `@proxy` matches). Prefer
`{{annotation_arg:name:N}}` in layer bodies to avoid nested-quote issues.

**Framework config (Vite proxy, etc.) belongs in layers**, not `typescript.rs` /
`rust.rs` — see MISSION and [IDE_AGENT_PLATFORM.md](./IDE_AGENT_PLATFORM.md).
### Section Composition

```
match fn where has_role("main")
  emit_to "main" priority 50
  emit """..."""
```

Multiple templates can target the same section. The engine collects all
contributions, sorts by priority, and concatenates them into the final
section output. Priority defaults to 100 (lower = earlier).

### Built-in Emitters

The engine provides built-in emitters for base shapes:
- `emit_struct(node)` — emit a struct definition
- `emit_trait(node)` — emit a trait/interface definition
- `emit_impl(node)` — emit an impl block
- `emit_fn(node)` — emit a function
- `emit_action(action)` — emit a single action/expression
- `emit_type(type_expr)` — emit a type annotation

These are available in all templates and handle the boilerplate of
translating core shapes to the target language.

## Execution Model

1. The codegen phase receives the full `Package` (AST) and `LayerRegistry`
2. For the requested target, it collects all `codegen <target>` blocks from
   all loaded layers
3. For each template, it evaluates the `match` condition against every node
   in the IR
4. For matching nodes, it executes the template, interpolating values from
   the node's properties, fields, children, and annotations
5. Output goes either to the node's file (default `emit`) or to a named
   section (`emit_to`)
6. After all templates execute, sections are composed and files are written

## Query Functions

Available in `where` clauses and template expressions:

| Function | Returns | Description |
|----------|---------|-------------|
| `has_annotation(name)` | bool | Node has this annotation |
| `annotation_value(name)` | string | Annotation's argument value |
| `fields` | list | Struct fields |
| `dep_fields` | list | Fields with @dep annotation |
| `methods` | list | Trait/impl methods |
| `steps` | list | Fn steps |
| `inputs` | list | Fn input parameters |
| `return_type` | string | Fn return type |
| `target` | string | Impl target trait name |
| `subkind` | string | Node's layer subkind |
| `children` | list | All child nodes |
| `parent` | node | Parent node |

## Example: di.layer Codegen

```
layer di
  codegen rust
    # Generate constructor for structs with @dep fields
    match struct where has_annotation("dep")
      emit """
        impl {{name}} {
            pub fn new({{for field in dep_fields}}{{field.name}}: {{field.type}}{{sep ", "}}{{end}}) -> Self {
                Self { {{for field in dep_fields}}{{field.name}}{{sep ", "}}{{end}} }
            }
        }
      """

    # Contribute @main fn steps to the main() function
    match fn where has_annotation("main")
      emit_to "main" priority 50
      emit """
        // — {{name}} —
        {{for step in steps}}
        // step: {{step.name}}
        {{for action in step.actions}}
        {{emit_action(action)}}
        {{end}}
        {{end}}
      """

  codegen typescript
    match struct where has_annotation("dep")
      emit """
        export class {{name}} {
          {{for field in dep_fields}}
          private readonly {{field.name}}: {{field.type}};
          {{end}}

          constructor({{for field in dep_fields}}{{field.name}}: {{field.type}}{{sep ", "}}{{end}}) {
            {{for field in dep_fields}}
            this.{{field.name}} = {{field.name}};
            {{end}}
          }
        }
      """
```

## Future Extensions

- **Conditional templates**: `match struct where has_annotation("dep") and field_count > 3`
- **Template inheritance**: layers can override parent layer templates
- **Validation templates**: emit compile-time assertions
- **Test templates**: auto-generate test scaffolding from the IR
