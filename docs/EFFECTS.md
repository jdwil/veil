# Effects / error model (PAR-003)

Fallibility is a **first-class type axis**, not a Rust-only `Result` keyword.

## Representation (today)

| VEIL surface | IR / AST | Meaning |
|--------------|----------|---------|
| `Res!` | `TypeExpr::Result(None)` | fallible, unit success |
| `Res!<T>` | `TypeExpr::Result(Some(T))` | fallible, carrying `T` |
| `?` | try / early-return sugar | propagates fallible |
| plain `T` | non-`Result` `TypeExpr` | non-fallible |

Parser + check already treat `Res!` / `?` as part of the language (see
`docs/LANGUAGE.md`). Semantic IR (PAR-001) keeps this axis independent of
backend keywords.

## Lowerings per target

| Target | `Res!` / `Res!<T>` | `?` | Notes |
|--------|--------------------|-----|-------|
| **Rust** | `Result<(), DomainError>` / `Result<T, DomainError>` | Rust `?` | Production path |
| **TypeScript** | `Promise` / thrown or `Result`-like union (capability-gated) | early return / throw | Prefer honesty over faking monads |
| **Swift** (spike) | `Result<Void, Error>` / `Result<T, Error>` | body not lowered | Capability: `TryOperator` claimed; bodies stub |
| **Kotlin** (spike) | `Result<Unit>` / `Result<T>` | body not lowered | Same honesty bar |

Unsupported combinations fail **closed** via `veil check -t <target>`
(PAR-002 / CHK-005).

## Tests / verification

```bash
# Non-fallible pure lib
veil check examples/pure_lib.veil --json

# Fallible services (existing examples with Res! / ?)
veil check examples/local_run.veil -t rust
```

Unit coverage for type mapping lives in backend modules (`type_to_swift` /
`type_to_kotlin` / Rust codegen). Expanding IR-level effect rows is **future**
(typed effect systems); this story freezes the honest multi-target contract.

## Non-goals

- Algebraic effect handlers in source
- Per-error-code enums auto-generated for every target
- Claiming Swift/Kotlin bodies lower `?` until codegen does

## Phase N delta (PAR-016)

When multi-target `?` starts to diverge beyond `TypeExpr::Result`:

1. Add optional **effect row** metadata on fns (`effects: [fallibility]`).
2. Keep `Res!` as surface sugar that sets the same axis.
3. Backends consume one IR axis (Rust `Result`, Swift `throws`/`Result`,
   Kotlin `Result` / exceptions policy).
4. Do **not** invent algebraic handlers until two backends need it.

Until then, PAR-003 lowerings + capabilities remain the contract.
`TypeExpr::Result` is the stable representation.
