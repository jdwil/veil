# DESIGN: Scenario "Movies" Subsystem

> **Status: design only.** No feature code exists yet. `scene` / `reel` / `movie`
> are net-new (verified: `grep -w '(scene|reel|movie)'` over `crates/` returns
> zero matches). This document is the careful, phased plan the team wants
> *before* any implementation. Mind Palace: `decision-scenario-movies-architecture`,
> `veil-scenario-movies-brainstorm`, `veil-review-visual-surface-brainstorm`.

## 0. One-paragraph thesis

A **movie** is behavior as replay, not a test log. It is one isolated, stub-only,
deterministic run of **one construct** against a fixture: clock/RNG/IO are stubbed,
the inbound event is an `expose` input (or, later, a UI event for a `view`), and
the outbound is events / `ret` / port calls / (for UI) pixels. The recording is a
**trace**; the human watches its deterministic replay. Each construct carries three
reels — **Happy / Fault / Odd** — authored in VEIL as `scene` blocks that reuse the
existing `flow`/`step`/`ret` engine, executed inside the *same* binary `veil gen`
already emits, and rendered on the delta-on-map review surface (swimlane for domain
services, widget+play for `view`). Sign-off can mean "I watched `fault_duplicate`."

The whole design rests on one rule: **a movie that needs the world is not a movie.**
`veil check` fails any scene that reaches a live port. That single gate is what
separates this from "an integration test wearing a costume."

---

## 1. `scene` grammar

### 1.1 Shape and placement

A `scene` is a **flow-shaped construct** that lives next to its target construct in
layer vocabulary. It reuses the exact AST the engine already has for behavior:
`Flow` / `FlowStep` / `StepDef` (`crates/veil-ir/src/ast.rs:543-590`). A scene is
authored with three ordered sections that lower to typed steps:

- `given` — fixture / precondition seeding (repo state, clock, tokens).
- `when` — the stimulus: an `expose` input value (or a `× N in Tms` burst).
- `then` — assertions over the outbound channel: `ret`, `emit`, port-call counts.

`given`/`when`/`then` are **typed step kinds** (`StepDef.kind`), not new engine
primitives. The engine already carries `kind: Option<String>` on `StepDef`
(`ast.rs:574`) and layer-defined step keywords with a `has` schema
(`StepFieldSpec`, `layer.rs:137`). So `scene` introduces **no new control-flow
concept** — it is a `Flow` whose steps are of kinds `given`, `when`, `then`.

### 1.2 Worked case — `CreateCustomer` (the three reels)

Taken directly from `veil-scenario-movies-brainstorm`, expanded to all three reels
as concrete, parseable-looking VEIL:

```veil
svc CreateCustomer
  expose
    node CreateCustomer
      input email: Email
      output customer_id: UUID

  # ── Reel 1: HAPPY ────────────────────────────────────────────
  scene happy_new_email
    reel happy
    given no Customer with email "a@b.co"
    when CreateCustomer{email: "a@b.co"}
    then ret Ok
    then emit CustomerCreated
    then CustomerRepo.save called 1

  # ── Reel 2: FAULT (each error channel the Res!/expose contract admits) ──
  scene fault_duplicate
    reel fault
    given Customer exists with email "a@b.co"
    when CreateCustomer{email: "a@b.co"}
    then ret Err(Duplicate)
    then CustomerRepo.save called 0
    then emit none

  # ── Reel 3: ODD (abuse the boundary) ─────────────────────────
  scene odd_quad_submit
    reel odd
    given no Customer with email "a@b.co"
    when CreateCustomer{email: "a@b.co"} × 4 in 50ms
    then CustomerRepo.save called 1
    then later calls ret Err(InFlight)
```

Additional odd-reel idioms the grammar must admit (all simulated, never slept):

```veil
  scene odd_empty_batch
    when CreateCustomerBatch{emails: []}
    then ret Ok(count: 0)
    then CustomerRepo.save called 0

  scene odd_clock_jump
    given clock at "2026-01-01T00:00:00Z"
    when CreateCustomer{email: "a@b.co"} ; clock advance 400d ; RenewCustomer{...}
    then ret Err(Expired)

  scene odd_stale_token
    given token issued at "t0" ; clock advance 25h
    when CreateCustomer{email: "a@b.co", token: "t0"}
    then ret Err(TokenExpired)

  scene odd_replayed_command
    when CreateCustomer{email: "a@b.co", idempotency_key: "k1"} × 2
    then CustomerRepo.save called 1
    then second call ret Ok(replayed: true)
```

### 1.3 `then` assertion vocabulary (outbound-only)

`then` assertions may only observe the **outbound channel** — this keeps a movie a
black-box replay of contract, not a white-box unit test:

| `then` form | Observes |
|---|---|
| `ret Ok` / `ret Ok(field: v)` | the `ret` value/shape |
| `ret Err(Variant)` | error channel (`Res!` / `Err`, see §1.4) |
| `emit EventName` / `emit none` | events on the bus port |
| `Port.method called N` | stubbed port call count |
| `later calls ret Err(...)` / `second call ...` | ordinal assertions over a burst |

### 1.4 Error channels (`Res!` / `Err`)

The `then ret Err(Variant)` form binds to the construct's existing error model.
The engine already lowers `Res!` / bang-contracts (`docs/BANG_CONTRACT.md`) and
carries an `error_boundary` on `Flow` (`ast.rs:549`). Scene fault reels **enumerate
the error variants the `expose`/`Res!` contract admits**; `veil check` can warn when
a declared error variant has no fault reel (a coverage smell, §5).

### 1.5 Where the parser / IR changes land

| Concern | File | Change (design) |
|---|---|---|
| Lex `scene` / `reel` / `given` / `when` / `then` / `×` burst | `crates/veil-parser/src/lexer.rs` | New keyword tokens; `×`/`x N in Tms` burst literal |
| Parse `scene` block into a `Flow` with typed `given`/`when`/`then` steps | `crates/veil-parser/src/parser.rs` | Reuse flow/step parsing; map sections to `StepDef.kind` |
| AST node for a scene | `crates/veil-ir/src/ast.rs` | Prefer **no new struct**: a `scene` is a `Flow` tagged `role:scene` on a child construct. Add `Scene`-flavored accessors only if flow reuse proves insufficient. |
| Scene as layer vocabulary (keyword + roles + `has` schema for reel/given/when/then) | `layers/*.layer` (new `scenes.layer`) | Declare `scene` keyword, `reel`/`given`/`when`/`then` step kinds via existing `StepFieldSpec` schema |
| IR lowering / validation entry | `crates/veil-ir/src/ir.rs` + new `crates/veil-ir/src/scene_check.rs` | Lower scene flows; run isolation gate (§2) |

**Design bias:** reuse `Flow`/`StepDef` and layer `declare`/`has` schemas so `scene`
is *vocabulary*, not a second engine. Add a dedicated `Scene` AST struct only if the
flow shape genuinely cannot carry `reel` tagging + burst semantics — decide in the
first slice, not up front.

---

## 2. Isolation contract ("no world, only stubs")

### 2.1 The rule

> A scene that calls a **live port** fails `veil check`. Movies that need the world
> are not movies.

Ports in VEIL are trait-shaped dependencies wired by `compose`
(`harness.rs:has_declared_harness`, `lower_compose`, `WireKind::{Adapter, ProvidedRuntime}`).
For a scene, **every** dependency the target construct reaches must resolve to a
**stub**, never to a `ProvidedRuntime` wire or a real adapter that opens a socket.

### 2.2 How stubs bind

The engine already has stub machinery this maps onto cleanly:

- **Stub crates / catalog:** `crates/veil-ir/src/stub_catalog.rs`, `StubCrate`
  (`harness_fields`, `structs`, `free_fns`), and the codegen resolvers
  `stub_type_path` / `stub_harness_field_expr` (`crates/veil-codegen/src/rust/harness.rs`).
- **`stub_gen` / `stub_install`:** the same path that gives `veil check` its offline
  fixtures. A scene's `given` seeds an in-memory stub (e.g. `InMemoryCustomerRepo`),
  and clock/RNG/bus are bound to deterministic stubs (fixed `now()`, seeded RNG,
  a recording bus that captures `emit` instead of publishing).

The binding is the **same isolation `veil check` + the harness already use**, just
*filmed*: instead of asserting once and discarding, the run records a trace.

### 2.3 The `veil check` gate (new `scene_check.rs`)

`check` walks each `scene`'s target construct's reachable calls (reuse
`expr_mentions_trait_dep`, already in `crates/veil-codegen/src/rust/harness.rs`) and:

1. Resolves each reached port to its scene-time wire.
2. Emits `scene_live_port` (**error**) if any wire is `ProvidedRuntime`, a real
   network adapter, or an unstubbed trait.
3. Emits `scene_unstubbed_clock` / `scene_unstubbed_rng` (**error**) when a scene
   reads wall-clock or RNG without a `given clock …` / seeded RNG stub.
4. Emits `scene_no_then` (**error**) — a movie with no outbound assertion is not a movie.

These join the existing harness diagnostics family (`harness_*` in
`crates/veil-ir/src/harness_check.rs`), same `Diagnostic` type and severity model.

---

## 3. Execution + replay model

### 3.1 Same binary, no second runtime

Scenes execute inside the **same binary `veil gen` already emits** — no new runtime,
no ECS, no ProductHost. Codegen emits a `#[cfg(test)]`-style **scene harness module**
next to the generated construct (a sibling of the emitted `harness` main), wiring the
construct to stub deps and driving the `when` stimulus. This reuses the Rust harness
emission path (`crates/veil-codegen/src/rust/harness.rs`, `harness_template.rs`); it
adds a **record wrapper** around port calls, not a parallel execution engine.

### 3.2 Record → replay split

- **Record (once, at `veil gen` + `veil scene run`):** execute the scene against
  stubs, capturing an ordered `TraceEvent` stream. Deterministic inputs (fixed clock,
  seeded RNG, simulated bursts) make the trace reproducible.
- **Replay (what the human watches):** the UI plays the recorded `TraceEvent` stream
  at human speed with scrub/step transport. Replay reads the trace only — it never
  re-executes, so watching is cheap and identical every time.

### 3.3 Trace event schema

Designed to be a **superset-compatible sibling of Spec A's review data** (§4) so the
movie pane and the delta-map share one source. JSON, `#[serde(tag)]`-style like
`EditOp`:

```jsonc
// TraceEvent — one recorded step of a reel replay
{
  "seq": 0,                      // ordinal within the reel
  "t_ms": 0,                     // simulated wall-clock offset (NOT real sleep)
  "span_start": 1234,            // AST span of the emitting node (same key as EditOp)
  "kind": "input" | "guard" | "step" | "port_call" | "emit" | "ret" | "error",
  "label": "CreateCustomer",     // human label for the swimlane lane
  "detail": { /* kind-specific */ }
}
```

Kind-specific `detail`:

| `kind` | `detail` fields |
|---|---|
| `input` | `{ construct, args }` — the `when` stimulus |
| `guard` | `{ expr, passed: bool }` — an invariant/guard evaluation |
| `step` | `{ name, step_kind }` — a `StepDef` boundary |
| `port_call` | `{ port, method, args_redacted, ordinal }` — stubbed call (secrets redacted via `veil_redact_secrets`) |
| `emit` | `{ event, payload_redacted }` |
| `ret` | `{ ok: bool, value }` |
| `error` | `{ variant, message }` — the `Res!`/`Err` channel |

The `span_start` key is deliberately the **same identity** `EditOp` uses
(`edit.rs`: "keyed by AST span start"), so a trace event can be joined to the edit
that touched that node.

### 3.4 Time cap

Each reel has a **wall-clock cap in the hundreds of ms**. Bursts (`× 4 in 50ms`),
clock jumps, and stale-token windows are **simulated on the stub clock**, never slept.
A reel exceeding the cap fails with `scene_time_cap` — this prevents a "movie" from
smuggling in a slow real dependency.

---

## 4. Rendering model

### 4.1 Pixels vs swimlane

- **`view` / UI constructs → real pixels** (later slice). The reel renders the
  component and replays UI events (mid-quad-click preview per mock `tiVBy.jpg`).
- **Domain services → swimlane trace.** A horizontal lane per stage:
  `INPUT → GUARD → PERSIST(port) → CALL → RET`, with OK/ERROR chips and a scrubbable
  timeline carrying click/emit markers. This is a **pure function of the TraceEvent
  stream** from §3.3 — no pixels, no headless browser.

### 4.2 Data the UI consumes

The movie pane consumes exactly two things, both already-defined shapes:

1. The **`TraceEvent[]`** per reel (§3.3).
2. The reel's **status** (`passed` / `failed` / `new`) plus the `span_start` of its
   target construct (to place the film pip on the map node).

### 4.3 Alignment with Spec A's `EditRecord` (auto-queue)

Spec A records edits as `EditOp` + `EditAnnotation { category, criticality }`
(`crates/veil-ir/src/edit.rs`). The movie subsystem hooks that model:

- A **`SetBody` on a guard** (`EditOp::SetBody`, which `infer_criticality` already
  scores `High`, `edit.rs:infer_criticality`) **auto-queues the fault + odd reels**
  for that construct — the changed logic must be re-watched under adversity.
- Because trace events and edits share the `span_start` key, the delta-map can paint
  "this guard changed **and** its fault reel now fails" on one node without a join
  table.

---

## 5. Review-surface integration

Integrates into the four-zoom delta-on-map surface
(`veil-review-visual-surface-brainstorm`), **not** a separate Storybook:

- **Film pip on map nodes.** A node with scenes gets a pip showing reel count; the pip
  goes **red if any reel failed after this change set** (criticality-style surfacing,
  same idea as the orange High pip).
- **Click node → change card + preview stage.** The card is the `EditAnnotation`
  (level 2); the preview stage adds the reel tabs (Happy / Fault / Odd) + transport.
- **Outstanding change set includes "reels that broke or are new."** This extends the
  existing `ChangeSet` / `OutstandingItem` model in `crates/veil-server/src/review.rs`
  — a broken/new reel is just another kind of outstanding item.
- **Partial sign-off per reel.** Reuse the existing partial sign-off (`ids[]` →
  `/api/review/sign_off`, `veil-visible-agency-sign-off`). "Approve happy+fault, reject
  odd_quad_submit, send the agent back at that scene only" maps to signing a subset of
  reel item ids.
- **Review smell.** A **changed `persist` with no added/updated scene** is a review
  smell of the same class as a missing `@invariant`: `check` emits
  `scene_missing_for_changed_persist` (**warning**). This is the movie analogue of the
  edit-annotation coverage the delta-map already surfaces.

---

## 6. Phased plan

Each slice is independently shippable. Do **not** boil the ocean — the first slice is
deliberately tiny.

### Slice 1 — "one keyword, three reels, stub-only, services only" (FIRST)

Scope, and nothing more:

1. `scene` keyword + `reel happy|fault|odd` + `given`/`when`/`then` typed steps,
   parsed into a `Flow` (parser + IR; new `scenes.layer`).
2. Stub-only execution in the generated harness for **`expose` nodes on domain
   services** (no `view`, no pixels).
3. The `veil check` isolation gate (§2.3): `scene_live_port`, `scene_no_then`.
4. `veil scene run` records the `TraceEvent` stream (§3.3) to a JSON artifact.
5. Preview stage = **swimlane trace** for services (no widget, no pixels yet).

Explicitly **out** of slice 1: real pixels, `view` constructs, odd-reel *generators*,
auto-queue, failed-reel-as-diagnostic jumping, partial per-reel sign-off UI.

### Slice 2 — review-surface wiring

Film pip on map nodes; reel tabs + transport in the preview stage; broken/new reels
in the outstanding change set; partial per-reel sign-off via existing `ids[]`.

### Slice 3 — `view` constructs with real pixels

Render the component; replay simulated UI events (quad-click); mid-event widget
preview (mock `tiVBy.jpg`).

### Slice 4 — odd-reel generators + auto-queue

Generate boundary-abuse reels (double-submit, empty batch, clock jump, stale token,
replayed command) from the construct's contract; auto-queue fault+odd on guard
`SetBody` (§4.3).

### Slice 5 — failed reel as jumpable diagnostic

A failed reel becomes a first-class diagnostic on the construct, jumpable like a
`UX-023` finding — not a CI afterthought.

---

## 7. Risks + open questions

- **Determinism of replay.** Any hidden nondeterminism (map iteration order, unseeded
  RNG in a stub, real `now()` leaking past the gate) makes replays diverge. Mitigation:
  the §2.3 gate must be strict, and stubs must expose a fixed clock + seeded RNG by
  contract. *Open:* do we hash the trace to detect drift between record runs?
- **UI event-simulation fidelity (Slice 3).** Quad-click "simulated not slept" must
  match real component behavior closely enough to be trustworthy. *Open:* headless
  render vs. a VEIL-level event model — which is the source of truth for `view` reels?
- **Fixture management.** `given` seeds can sprawl. *Open:* shared fixtures per layer
  vocabulary vs. per-scene inline; how do fixtures version with the construct?
- **Cost / time caps.** Hundreds-of-ms per reel × many constructs × every turn could
  get expensive. *Open:* record reels only for changed constructs (delta-driven), or
  all reels on demand?
- **How a scene references layer vocabulary.** `given no Customer with email` uses
  domain nouns from the loaded layers. *Open:* does the parser resolve those against
  layer `declare` types at parse time, or defer to `check`?
- **Codegen-target interaction.** Slice 1 targets Rust. *Open:* is the scene harness
  emitted per target (TS backend needs its own record wrapper), or is the trace a
  target-independent artifact produced only by the Rust harness for now? Recommend:
  **trace artifact is target-independent; only the Rust harness records it in v1.**
- **`scene` AST reuse vs. dedicated struct.** Decide in Slice 1 whether `Flow` +
  `role:scene` is sufficient or a `Scene` struct is warranted (§1.5).

---

## Appendix A — grounding citations

| Claim | Evidence in repo |
|---|---|
| `scene`/`reel`/`movie` are net-new | `grep -w '(scene\|reel\|movie)' crates/` → 0 matches |
| flow/step/ret engine exists | `crates/veil-ir/src/ast.rs:543` (`Flow`), `:555` (`FlowStep`), `:563` (`StepDef`, incl. `kind`, `fields`, `edges`) |
| typed step kinds already supported | `StepDef.kind: Option<String>` (`ast.rs:574`), `StepFieldSpec` (`layer.rs:137`) |
| harness / stub machinery exists | `crates/veil-ir/src/harness.rs`, `harness_check.rs`; `crates/veil-codegen/src/rust/harness.rs`, `harness_template.rs`; `stub_type_path` / `stub_harness_field_expr` / `StubCrate` |
| port wiring model | `harness.rs` `lower_compose`, `WireKind::{Adapter, ProvidedRuntime}` |
| reachable-dep detection reusable for the gate | `expr_mentions_trait_dep` (`crates/veil-codegen/src/rust/harness.rs`) |
| edit model to align trace with (Spec A) | `crates/veil-ir/src/edit.rs`: `EditOp`, `EditAnnotation`, `EditCategory`, `Criticality`, `infer_criticality` (SetBody→High) |
| secret redaction for trace payloads | `veil_redact_secrets` (`crates/veil-codegen/src/rust/harness.rs`) |
| sign-off / change set to extend | `crates/veil-server/src/review.rs`; `veil-visible-agency-sign-off` |
| structural diff for delta paint | `crates/veil-ir/src/struct_diff.rs` |
