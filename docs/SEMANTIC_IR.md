# Semantic IR sketch (PAR-001)

**Status:** design (no large rewrite required to accept this story)  
**Date:** 2026-07-10

## Goals

Ground multi-target work in **semantic axes**, not Rust AST shape alone.
Incremental migration is OK; backends keep lowering from current AST while
capabilities (CHK-005) fail closed.

## Axes

| Axis | Current | Target IR note |
|------|---------|----------------|
| **Errors / effects** | `Res!` / `?` as sugar over Result-like | Explicit fallible region + effect row later (PAR-003) |
| **Async** | implicit in fn/service for Rust; await expr | Capability `AwaitExpr`; per-target sync/async policy |
| **Ownership / sharing** | implicit owned; Arc at boundaries | Optional share marks (PAR-004); no lifetimes in source |
| **Concurrency** | `par` steps, saga coordinator | Bounds as capabilities, not OS threads in IR |
| **Modules / visibility** | `pkg`, `ctx`, `group`, `+` export | Stable visibility lattice: private / package / export |

## Mapping (incremental)

```
AST Construct(shape)  →  IR Node(kind) + subkind (layer)
AST Expr::*           →  keep; gate by Feature capability per target
Layer declare         →  injected constructs (Bus, Auth, …) — not engine domain
```

Check pipeline already projects capabilities (`veil_codegen::capabilities`).
Semantic IR does **not** replace layers; layers still stamp vocabulary.

## Non-goals (phase N)

- Full Rust `unsafe` / raw pointers  
- Proc-macro / attribute DSLs as VEIL syntax  
- Keyword-for-keyword clones of Swift/Kotlin/TS  
- Lifetime parameters in `.veil` source  

## Review vs MISSION

MISSION “semantic substrate” asks for honesty over demos: capabilities + escape
hatches (CHK-005/006) are the enforcement path until typed effect/async IR
lands (PAR-003+).

## Next code steps (not this story)

1. PAR-002 — document/extend capability matrix (already partially implemented)  
2. PAR-003 — effects as first-class  
3. New backends register `Feature` sets only  
