# Codegen Plan — Bus-as-Port + Real Rust Generation

Status: **agreed, not yet implemented**. This is the active work item.
Written 2026-07-04 as a session handoff. Read MISSION.md first for the
architecture and the zero-domain-knowledge invariant (it is up to date and
describes the CURRENT, fully generic engine — the refactor is done,
verified, and uncommitted in the working tree).

## Where things stand

The engine is fully dynamic (see MISSION.md "Current State"): 7 core shapes,
2 statement shapes, all vocabulary from .layer files, stackable layers
(crm.layer on ddd.layer proves it). 40 tests pass; both examples parse,
validate, and generate *compiling* Rust; viewer type-checks clean.
**~25 files of uncommitted changes** — commit before or alongside this work.

BUT: codegen emits real code only for declarative shapes (structs, enums,
traits, workspace). Behavior is stubbed:
- flow/svc/saga step bodies → `tracing::info!` + `// TODO`
- adapter impl bodies → commented-out impl
- aggregate fns (verify/reject) → not emitted at all
- saga compensate blocks → not emitted

The expression AST (`Expr`, `ActionExpr`) is fully parsed and rich enough;
no translator was ever written.

## The agreed design (decided with JD this session)

JD's key insight: layer "statements" (dispatch/invoke/request) were the
wrong abstraction — really they are methods on a **Bus port**. Messaging
should be ordinary port calls; apps supply Bus adapters like any adapter.

1. **`declare` sections in .layer files** — layers can declare concrete
   constructs (not just vocabulary). ddd.layer declares:
   ```
   declare
     port Bus
       dispatch(evt: Event) -> Res!
       invoke(cmd: Command) -> Res!<Any>
       request(qry: Query) -> Res!<Any>
   ```
   Declared constructs are injected into scope of any solution using the
   layer. Parse them with the existing shape parsers.
   OPEN QUESTION: what `Event`/`Command` mean as param types (events are
   app-defined). Candidates: opaque message type, or the per-aggregate
   wrapper enums codegen already makes in `messages.rs`.

2. **Statements become pure sugar over port calls** (JD chose this over
   removing them — keeps token-efficiency + viewer icons). A statement's
   `maps_to` may name a port method:
   ```
   statement dispatch
     maps_to Bus.dispatch
   ```
   The PARSER desugars `dispatch CustomerCreated{...}` into a plain
   `CallExpr` targeting `Bus.dispatch` at parse time. `ActionExpr` likely
   disappears (or shrinks to guard-only). `guard` is different — it's
   control flow (`maps_to if`), stays as-is.
   crm.layer's `notify -> dispatch` must still chain (notify desugars
   through dispatch to Bus.dispatch).

3. **Real codegen** (drops the earlier "VeilRuntime hooks" idea — Bus port
   makes it unnecessary):
   - `veil-codegen/src/expr.rs` — Expr→Rust translator (let-bindings,
     field access, binops, literals; guard → `if !(cond) { return
     Err(DomainError::Validation(...)); }`)
   - Name resolution for call targets: trait-shaped construct → injected
     dep; struct-shaped → `Type::new(...)`; local var → method call;
     unknown (e.g. `http`) → runtime hook / stub
   - Generated `Deps` struct per module: `Arc<dyn Port>` fields derived by
     scanning flow bodies; flows take `&Deps`
   - Aggregate fns → real methods: `pub fn verify(&mut self, ...) ->
     Result<Vec<CustomerEvent>, DomainError>`; `@invariant` → guard;
     `emit` collects events; state assignment checked against enum block
   - Saga runner: sequential steps, compensate blocks pushed as closures,
     unwind in reverse on failure (generic, shape-level)
   - Adapter bodies: translate impl exprs (http.* against a hook/stub)
   - Verify: generated output must `cargo build` + smoke test wiring a
     mock Bus/adapters through a flow

## Suggested order

1. Commit the current refactor first (it's done and verified).
2. `declare` sections in layer.rs parsing + registry (+ scope injection).
3. Statement desugaring in parser (maps_to port.method) + update ddd/crm
   layers + tests.
4. expr.rs translator + Deps injection + flow bodies.
5. Aggregate fn bodies. 6. Saga runner. 7. Adapter bodies.
8. End-to-end: `cargo build` both generated workspaces, smoke test.

## Longer-term context (from this session's discussion)

- Multi-language targets (TypeScript domain types + API clients, later
  Svelte UI) are a goal. The Bus-port design was chosen partly because it
  answers "what does dispatch mean" ONCE, in userland, per target.
  TS backend = another shape-switch emitter; highest-value target is
  shared DTO types + typed client from trait-shaped constructs.
- UI generation should be layer-driven (e.g. a `ui` block like `visual`),
  never hardcoded — same invariant.

## Key files

- `crates/veil-ir/src/layer.rs` — LayerRegistry, shapes, transitive maps_to
- `crates/veil-parser/src/parser.rs` — shape-driven parser (statements → parse_action)
- `crates/veil-ir/src/ast.rs` — generic Construct, ActionExpr (to be desugared away)
- `crates/veil-codegen/src/rust.rs` — shape-driven codegen (the stubs to replace)
- `examples/ddd.layer`, `examples/crm.layer` (stacked), `examples/sales_crm.veil`
