# Codegen Execution Plan — Bus-as-Port + Real Rust Generation

Status: **ready to implement**. This is a code-grounded expansion of
`CODEGEN_PLAN.md`, written after reading the full engine. Read `MISSION.md`
first (the zero-domain-knowledge invariant), then `CODEGEN_PLAN.md` (the
agreed design). This file adds the concrete, file-by-file steps and the exact
current-code anchors so the work can be executed directly.

The engine is fully generic and 40 tests pass. Codegen emits real declarative
code (structs/enums/traits/workspace) but stubs ALL behavior. This plan makes
behavior real without adding any domain knowledge to the engine.

## Ground rules (do not violate)

- **Zero domain knowledge.** No DDD strings ("Aggregate", "Bus", "dispatch")
  hardcoded in parser/builder/codegen Rust. Everything keys off `Shape`,
  `StmtShape`, and layer-declared names. The words `Bus`/`dispatch` live in
  `.layer` files only.
- **Keep the 40 tests green** at every step; add tests as you go.
- Commit the current refactor FIRST (working tree has ~25 files uncommitted
  plus a stray deleted `.claude/settings.local.json.tmp...` — see `git status`).
  Actually verify with `git status` and `cargo test` before starting.

## Key file map (verified line anchors as of this writing)

- `crates/veil-ir/src/layer.rs` (639 lines) — `Shape`, `StmtShape`,
  `ConstructSpec`, `StatementSpec`, `LayerRegistry`, `parse_layer_file`
  (line 378), `merge_and_resolve` (280), `resolve_statement_shape` (349).
  Statement parsing currently reads `maps_to`, `desc`, `semantics` only
  (lines 515-522). No `declare` handling yet.
- `crates/veil-parser/src/parser.rs` (2066 lines) — `parse_action` (1678)
  builds `ActionExpr`; `parse_expr` (1643) dispatches statements via
  `registry.statement()` at 1655. `parse_call_stmt` (1789) is the core call.
- `crates/veil-ir/src/ast.rs` (459 lines) — generic `Construct` (116),
  `Expr` (355), `ActionExpr` (390), `CallExpr` (453).
- `crates/veil-codegen/src/rust.rs` (702 lines) — THE STUBS. `gen_application`
  (569) emits `tracing::info!` + `// TODO` per step; `gen_impls` (525) emits
  commented-out impl; aggregate `c.fns` are NOT emitted anywhere (gen_struct at
  348 emits only fields/enums/invariant-new); saga `sub_blocks` (compensate)
  ignored.
- `crates/veil-ir/src/builder.rs`, `serialize.rs`, `validate.rs` — generic,
  shape-switched. `expr_to_veil`/`expr_to_display` handle all `Expr` variants.
- `examples/ddd.layer`, `examples/crm.layer`, `examples/customer_onboarding.veil`,
  `examples/sales_crm.veil`.

## Data model facts that matter for codegen

- Aggregate business logic lives in `Construct.fns: Vec<FnDef>` (ast.rs:135,
  221). Each `FnDef` has `params`, `return_type`, `annotations`, `body: Vec<Expr>`.
  In `customer_onboarding.veil` the `agg Customer` has `fn verify`/`fn reject`
  with `@invariant(...)`, an `Assign` (`status = Verified`), and an
  `emit ...{}` action.
- Events/commands are nested `children` of the struct-shaped aggregate, grouped
  by subkind. `gen_child_types` (rust.rs:423) already builds `CustomerEvent`
  wrapper enum + per-message structs in `messages.rs`. Reuse this for `emit`.
- Steps carry `body: Vec<Expr>`, `refs: Vec<RefLine>` (e.g. `ctx Identity`),
  and `sub_blocks: Vec<SubBlock>` (e.g. `compensate`). `ActionExpr` with
  `StmtShape::Call` has `target`, `method`, `args`, `named_args`; with
  `StmtShape::If` (guard) has `condition`, `message`.
- `emit`, `dispatch`, `invoke`, `request`, `notify` all currently resolve to
  `StmtShape::Call`. `guard` resolves to `StmtShape::If`.

---

## Step 1 — Commit the refactor

```
git add -A && git commit   # message: the generic-engine refactor
```
(Already has commit `ab46eef` — confirm the tree is actually dirty first; the
memory says uncommitted but `git log` shows the refactor landed. If clean, skip.)

## Step 2 — `declare` sections in `.layer` files

**Goal:** a layer can declare concrete constructs injected into every solution
that uses it. `ddd.layer` gains:

```
declare
  port Bus
    dispatch(evt: Event) -> Res!
    invoke(cmd: Command) -> Res!<Any>
    request(qry: Query) -> Res!<Any>
```

**layer.rs changes:**
- Add `pub declared: Vec<String>` (raw source lines) OR better, parse into a
  reusable structure. Simplest robust approach: store the raw text block of the
  `declare` section per layer and re-parse it with the veil parser at
  registry-build time. But parser depends on veil-ir, and veil-ir cannot depend
  on veil-parser (cycle). So DO NOT parse in layer.rs.
- Instead: `LayerRegistry` stores `pub declarations: Vec<String>` — each entry
  is the dedented source text of one declared construct (e.g. the `port Bus`
  block). `parse_layer_file` recognizes a top-level `declare` section (sibling
  of `construct`/`statement`) and accumulates the raw indented lines under it,
  splitting into one string per top-level declared construct.
- Expose `registry.declarations()`.

**Injection point (parser side):** in `parse.rs` / `parse_solution`, after the
solution's own items are parsed, parse each declaration string using the SAME
registry and append the resulting `Construct`s as top-level `TopLevelItem`s
(or into a synthetic module). Because declarations use core/ddd keywords
(`port` → trait), the existing shape parsers handle them. Guard against
double-injection when multiple layers declare (dedupe by construct name).

Alternative cleaner injection: do it in the CLI/`parse_with_registry` wrapper so
both `parse` and `serve` get it. Recommendation: add a helper
`inject_declarations(&mut Solution, &LayerRegistry)` in veil-parser that lexes
+ parses each declaration block and pushes unique constructs. Call it at the
end of `parse_solution`/`parse_with_registry`.

**OPEN QUESTION (from CODEGEN_PLAN.md):** what do `Event`/`Command`/`Query`
mean as Bus param types? Decision needed. Pragmatic answer: treat them as
opaque marker types generated once (e.g. `pub struct Event;` or a blanket
`enum`/trait). Simplest: emit `pub trait Event {}` / type aliases, OR make Bus
generic-ish by lowering these params to `String`/`serde_json::Value` via the
existing stub-type path (`type_to_rust` unknown → passes name through; the
stub-type generator in `gen_types` at rust.rs:281 turns undefined types into
`pub type X = String;`). Least-effort correct: let Event/Command/Query fall
through as stub types. Revisit once flows actually call `Bus.dispatch`.

**Tests:** registry test that `declarations()` contains a `Bus` block; parser
test that a solution using ddd has a `Bus` trait-shaped construct injected.

## Step 3 — Statement desugaring to port calls

**Goal:** `dispatch X{...}` becomes a plain `CallExpr { target: "Bus",
method: "dispatch", args: [X{...}] }` at parse time. Statements whose
`maps_to` names `port.method` desugar; `guard` (maps_to `if`) stays an Action.

**layer.rs:** `StatementSpec.maps_to` may now be `Bus.dispatch`. Update
`resolve_statement_shape` (349) so a `maps_to` containing a `.` resolves by
looking at the LEFT side's ultimate... no — the shape is still `Call`. Keep
shape resolution: if `maps_to` has a dot, treat as `Call` shape and record the
port target+method. Add fields to `StatementSpec`:
`pub port_target: Option<String>`, `pub port_method: Option<String>`.
Populate when `maps_to` matches `Ident.Ident`. For chained statements
(crm `notify -> dispatch -> Bus.dispatch`), resolve transitively: follow
`notify.maps_to = dispatch` to the `dispatch` statement, inherit its
`port_target`/`port_method`. Do this in `merge_and_resolve` after shape
resolution.

**Ddd/crm layer edits:**
```
statement dispatch
  maps_to Bus.dispatch
statement invoke
  maps_to Bus.invoke
statement request
  maps_to Bus.request
statement emit          # emit is aggregate-local, NOT a bus call — keep maps_to call
  maps_to call
statement guard
  maps_to if
# crm.layer
statement notify
  maps_to dispatch      # chains → Bus.dispatch
```
Note: `emit` should stay a plain Call (it collects events inside an aggregate),
NOT a Bus call. Confirm with the aggregate-fn design in Step 5.

**parser.rs:** in `parse_action` (1678), when the resolved statement has
`port_target`/`port_method`, build `Expr::Call(CallExpr{ target, method, args })`
instead of `Expr::Action`. The named-args form `dispatch Evt{a,b}` must lower
to a single constructor arg: `Bus.dispatch(Evt{a: .., b: ..})`. Since `Expr`
has no struct-literal variant, represent the event construction as
`Expr::Call(CallExpr{ target: "Evt", method: "new"/"", args: [...] })` OR add a
`StructLit` expr variant. **Recommendation:** add `Expr::StructLit(String,
Vec<(String, Expr)>)` to ast.rs — cleaner than faking a call, and codegen can
emit `Evt { a, b }`. Update `expr_to_veil`, `expr_to_display`, builder,
serializer for the new variant (all have exhaustive matches — compiler will
flag them).

**Keep viewer icons:** the palette still lists `dispatch`/`notify` as statements
(they remain in `registry.statements`), so viewer icons survive. Desugaring is
purely at the AST level.

**Tests:** update `test_layer_statements_parse_as_actions` and
`test_stacked_layer_resolves_transitively` — `dispatch`/`notify` now produce
`Expr::Call` (or StructLit-bearing call) targeting Bus, not `Expr::Action`.
Guard still an Action. Round-trip (`emit`) test: serializer must reproduce
`dispatch Evt{...}` from the desugared form — this is the tricky part. If
round-trip fidelity matters, keep enough info on the Call to re-emit the sugar
(e.g. stash original keyword). **Decision:** add `CallExpr.sugar: Option<String>`
= original statement keyword, so serializer emits `dispatch Evt{...}` when
present and codegen emits `self.bus.dispatch(...)`. This preserves the
token-efficient source and the viewer.

## Step 4 — `expr.rs` translator + Deps injection + flow bodies

Create `crates/veil-codegen/src/expr.rs`. Pure `Expr -> String` (Rust), with a
small context for name resolution.

**Translator (`fn expr_to_rust(e: &Expr, ctx: &GenCtx) -> String`):**
- `Ident(n)` → `n` (snake if it's a known local? keep as-is; VEIL idents are
  already lower for locals). Field access → `a.b`.
- `IntLit/FloatLit/BoolLit/StringLit` → literal.
- `BinaryOp/UnaryOp` → obvious.
- `Assign(name, rhs)` → `let name = <rhs>;` (first assignment) — but repeated
  assignment to `status` in aggregate fns is a mutation, not a let. In flow
  bodies, `c = call ...` is a let-binding. **Rule:** at statement position in a
  step/fn body, `Assign` → `let <name> = <rhs>;` UNLESS `name` is a known field
  of the enclosing struct (aggregate fn), where it becomes `self.<name> = <rhs>;`.
  Pass the enclosing struct's field set in `GenCtx`.
- `Call(CallExpr)` name resolution (the core of the design):
  - target is a **trait-shaped construct** (port/repo/integration/Bus) → it's an
    injected dependency: `self.deps.<snake target>.<method>(args).await?`
    (or `ctx.<dep>` — see Deps below). Look up shape via a name→shape map built
    from the solution's constructs.
  - target is a **struct-shaped construct** with method `new` → `Type::new(args)`
    (or `Type::new(args)?` if returns Result).
  - target is a **local variable** (was let-bound earlier in the body, or the
    aggregate `self`) → `target.method(args)` — e.g. `c.verify(code)` →
    `c.verify(code)?`.
  - unknown target (e.g. `http`, `now`) → runtime hook/stub: emit
    `veil_runtime::<target>_<method>(args)` or a local stub fn. Keep a set of
    emitted stubs. For `now()` specifically emit `chrono::Utc::now()`. Simplest:
    unknown bare call `now()` → `Utc::now()`; unknown `http.post(...)` → a
    generated `http_post(...)` stub returning `Ok(Default::default())`.
  - `.await?` handling: trait methods are `async fn ... -> Result<..>` (traits
    generated at rust.rs:497 are `#[async_trait]`, methods `async`). So port
    calls need `.await` and `?`. Track whether the call is fallible (target
    method returns `Res!`). Cheap heuristic: all port/repo methods return
    Result in these examples → always `.await?` for trait-target calls.
- `StructLit(name, fields)` → `name { field: expr, ... }`.
- Guard is an `Action` (If shape), handled in the step emitter, not here:
  `guard cond, "msg"` → `if !(cond) { return Err(DomainError::Validation("msg".into())); }`.

**GenCtx** carries: `name_to_shape: HashMap<String, Shape>` (all constructs in
the solution, by name), `locals: HashSet<String>` (accumulated let-bindings),
`self_fields: HashSet<String>` (when inside an aggregate fn), `deps: &Deps`
field name for the module. Build `name_to_shape` once per `generate()`.

**Deps struct per module (rust.rs `gen_application`):** scan all flow/svc/saga
step bodies (and aggregate fns? no — aggregates don't hold deps) for `Call`
targets that resolve to trait-shaped constructs. Emit:
```
pub struct Deps {
    pub customer_repo: std::sync::Arc<dyn CustomerRepo>,
    pub notifier: std::sync::Arc<dyn Notifier>,
    pub bus: std::sync::Arc<dyn Bus>,
    ...
}
```
Flows become `pub async fn create_customer(deps: &Deps, <inputs>) ->
Result<..., DomainError>`. Port calls become `deps.customer_repo.save(c).await?`.

**gen_application rewrite (rust.rs:569-648):** replace the per-step
`tracing::info!` stub loop with:
```
for step in steps {
    // emit `// step: name` comment (keep for readability)
    for expr in &step.body { emit translated stmt }
    // compensate sub_blocks handled in saga runner (Step 6)
}
// return: if flow has return_expr, translate it; else Ok(())
```
Return type: currently hardcoded `Result<Uuid, DomainError>` returning
`Ok(Uuid::new_v4())`. Change to honor the fn/flow return. For `svc
CreateCustomerService` the `ret c.id` → `Ok(c.id)` with return type
`Result<Uuid, DomainError>` (id is Uuid). Deriving the exact return type
generically is hard; acceptable v1: if `return_expr` present, emit `Ok(<expr>)`
and set return type to `Result<Uuid, DomainError>` when the returned expr looks
like an id, else `Result<(), DomainError>`. Better: infer from svc's declared
return if available. Keep pragmatic; the goal is `cargo build` success.

**Wire module:** add `pub mod expr;` to `crates/veil-codegen/src/lib.rs`
(currently 5 lines — just `pub mod rust;` presumably; check).

**Verify:** `cargo run -p veil-cli -- gen examples/customer_onboarding.veil -o
/tmp/co && cd /tmp/co && cargo build`. Iterate until it compiles.

## Step 5 — Aggregate fn bodies

Aggregate `c.fns` are currently dropped. In `gen_struct` (rust.rs:348), after
emitting the struct + enum blocks, emit an `impl <Name>` with the fns:
```
impl Customer {
    pub fn verify(&mut self, code: String) -> Result<Vec<CustomerEvent>, DomainError> {
        // @invariant(status == Pending) → guard
        if !(self.status == CustomerStatus::Pending) {
            return Err(DomainError::Validation("invariant".into()));
        }
        let mut events = Vec::new();
        self.status = CustomerStatus::Verified;     // Assign to a field → self.
        events.push(CustomerEvent::CustomerVerified(CustomerVerified { id: self.id, verified_at: Utc::now() }));  // emit
        Ok(events)
    }
}
```
Rules:
- Return type: `Result<Vec<<Parent>Event>, DomainError>` — the wrapper enum
  name is `format!("{}{}", parent.name, "Event")` matching `gen_child_types`
  (rust.rs:446). If the aggregate emits multiple message subkinds, this is only
  Events; commands aren't emitted. Fine.
- `@invariant(expr)` annotation on the fn (FnDef.annotations) → guard/early
  return, same lowering as `guard`.
- `Assign(field, rhs)` where field ∈ struct fields → `self.field = rhs;`.
  Enum-valued assignments: `status = Verified` → `self.status =
  CustomerStatus::Verified;`. Need to qualify the variant with its enum type.
  Look up which enum block declares the variant `Verified` (the `state
  CustomerStatus` block's variants) and emit `CustomerStatus::Verified`. This is
  generic: search `c.blocks` (enum shape) for a variant match.
- `emit X{...}` (an `Action` Call with keyword `emit`, OR a desugared call —
  decide in Step 3 to keep `emit` as an Action so it's distinguishable) →
  `events.push(<ParentEvent>::<X>(<X> { ...named_args... }));`. Bare fields
  like `emit CustomerVerified{id, now()}` map `id` → `self.id` if id is a
  self field, `now()` → `Utc::now()`.
- Method takes `&mut self` because it mutates and returns collected events.
  Callers (`c.verify(result.code)` in the saga) then get
  `let events = c.verify(...)?;` — but the saga also treats `c` as needing to
  exist. This cross-fn interaction is where it gets hairy; for v1, ensure the
  aggregate impl compiles standalone even if the saga's use of it is loose.

**Test:** unit test on codegen output string contains `impl Customer` with
`pub fn verify(&mut self`. Plus the /tmp build.

## Step 6 — Saga runner with compensation

Saga = fn-shaped construct whose steps have `sub_blocks` with keyword
`compensate`. Generic lowering (no DDD knowledge — keys off presence of a
`compensate` sub_block):
```
pub async fn onboard(deps: &Deps, <inputs>) -> Result<(), DomainError> {
    let mut compensations: Vec<Box<dyn FnOnce() -> ...>> = Vec::new();
    // step create_customer
    <translated body>
    compensations.push(Box::new(move || { <translated compensate body> }));
    // ... on any `?` failure we should unwind. Since `?` early-returns, wrap:
}
```
The clean async way is hard with closures capturing async. Pragmatic v1: emit a
sequential block where each step is a labeled inner async block, and on error
run the accumulated compensations in reverse. Simplest that compiles:
- Translate each step body into a `let step_result: Result<(), DomainError> =
  async { ...; Ok(()) }.await;` then `if step_result.is_err() { <run comps in
  reverse>; return Err(...); }` before pushing this step's compensation.
- Compensations: store as a `Vec` of already-evaluated async calls is
  impossible; instead emit explicit reverse-order calls inline in the error
  path. Because compensation bodies are just port calls (e.g.
  `CustomerRepo.delete(c.id)`), you can codegen a nested if-error ladder.

Given complexity, **v1 acceptable output:** emit steps sequentially with `?`,
and emit compensation logic as commented reverse-unwind PLUS a working
best-effort: wrap the whole saga body in an inner closure returning Result and,
on Err, call each compensation in reverse (compensations that only do port
calls can be re-emitted as direct `deps.x.y().await` in the catch block). Keep
it compiling; note the limitation in a doc comment. Don't over-engineer — the
MISSION values a compiling, shape-driven runner over a perfect one.

## Step 7 — Adapter impl bodies

`gen_impls` (rust.rs:525) currently emits a struct + commented-out impl. Make it
emit a real `#[async_trait] impl <Target> for <Name>` with each `MethodImpl`
body translated via expr.rs. Adapter bodies call `http.post(...)` etc. — these
are unknown targets → runtime hook/stub (`http_post(url, body)` returning
`Ok(Default::default())` or `Ok(...)` matching the trait method's return type).
The `@env(...)` annotation already becomes struct fields (rust.rs:540). Ensure
the generated impl's method signatures match the trait (params from the trait
method, not the impl's bare param names — impl params are just names; zip with
the trait's typed params by position). Look up the target trait's method by
name to get the real signature.

**Test + build.**

## Step 8 — End-to-end verify

```
cargo test                                   # 40+ green
cargo run -p veil-cli -- gen examples/customer_onboarding.veil -o /tmp/co
(cd /tmp/co && cargo build)                  # MUST compile
cargo run -p veil-cli -- gen examples/sales_crm.veil -o /tmp/crm
(cd /tmp/crm && cargo build)                 # MUST compile — proves genericity
```
Then a smoke test: hand-write (or codegen) a `main` that constructs `Deps` with
mock `Arc<dyn Bus>` + mock adapters and drives one flow, assert Ok. Put it in a
generated `examples/`-style bin or a `#[cfg(test)]` in the generated crate. The
real proof is that BOTH workspaces build with zero engine changes between them.

## Ordering / dependencies

2 → 3 (desugaring needs declared Bus + statement port targets).
3 → 4 (flow bodies translate the desugared calls).
4 → 5, 6, 7 (all reuse expr.rs).
5,6,7 → 8.
Steps 5, 6, 7 are independent of each other; can be done in any order after 4.

## Decisions locked in this plan (were open in CODEGEN_PLAN.md)

1. **Event/Command/Query Bus param types** → let them fall through as stub
   types (`pub type Event = String;` via existing undefined-type path) for v1.
2. **Struct literals** → add `Expr::StructLit(String, Vec<(String, Expr)>)` to
   the AST rather than faking a call. Update all exhaustive matches.
3. **Preserve statement sugar** → add `CallExpr.sugar: Option<String>` so the
   serializer re-emits `dispatch Evt{...}` and the viewer palette is unaffected,
   while codegen emits the real `self.bus.dispatch(...)`.
4. **`emit` stays an Action** (not desugared to Bus) — it's aggregate-local
   event collection, lowered inside aggregate fn bodies in Step 5.
5. **Saga compensation** → best-effort compiling reverse-unwind for v1,
   limitation documented; not a perfect transactional runner.

## Watch-outs

- Every `Expr` match in the codebase is exhaustive (builder.rs, serialize.rs,
  and the display fns). Adding `StructLit` / new `CallExpr` field → the compiler
  will point you at every site. Good.
- `type_to_rust` (rust.rs:663) maps `Str→String, Int→i64, UUID→Uuid`, etc.
- Undefined referenced types become `pub type X = String;` (rust.rs:281) — this
  is your safety net for `Any`, `Plan`, `KYCResult`, `ExtId`, `Company`, etc.
- Trait methods are `#[async_trait]` + `async fn` (rust.rs:497), so all
  port/adapter calls are `.await` and fallible.
- `to_snake` (rust.rs:652) for field/dep/fn names.
- Keep the invariant: if you're about to type a domain word in Rust, stop — it
  belongs in a `.layer` file.
