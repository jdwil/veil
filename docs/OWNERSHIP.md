# Ownership capabilities (PAR-004)

## Policy

VEIL source is **not** full of Rust lifetimes. Values are **implicitly owned**
unless a sharing mark is required at a boundary.

| Mode | Source syntax (MVP) | Rust lowering | TS / GC targets |
|------|---------------------|---------------|-----------------|
| Owned (default) | `T` | `T` / move | plain `T` |
| Shared | optional `@shared` / layer policy later | `Arc<T>` or clone at boundary | ignored |
| Borrow | not required in `.veil` | inserted by Rust backend only | n/a |

## MVP status

- **No lifetime parameters** in `.veil` (enforced by language non-goal).
- Rust backend already inserts ownership at infrastructure boundaries (Bus
  handlers, shared deps) without author-written `Arc` / `'a`.
- Explicit `@shared` field annotations are **optional future syntax**; until
  then, layers / DI inject shared services and codegen decides clone vs Arc.
- TypeScript / Swift / Kotlin spikes ignore ownership marks.

## Design rules

1. Prefer owned values in domain structs.
2. Sharing is a **capability**, not a default — only where concurrent or
   multi-handler access needs it.
3. Check must not invent lifetime errors for portable packages (PAR-008).

## Migration

When `@shared` lands:

1. Add parse + IR metadata flag on fields/params.
2. Rust: `Arc<T>` + clone-on-send policy.
3. Other targets: no-op or document equivalent (e.g. Kotlin by-ref objects).
