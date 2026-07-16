# VEIL Language Reference

This is the complete reference for the VEIL language: every keyword, operator,
type form, and file kind. It is grounded in the actual lexer, parser, and layer
loader — see the *source* pointers at the end of each section.

> **The zero-domain-knowledge rule.** The VEIL engine understands only a small
> fixed grammar. Every *domain* word you'll see in `.veil` files — `ctx`, `agg`,
> `port`, `saga`, `dispatch`, `guard`, … — is **not** built into the language.
> Those come from `.layer` files loaded at parse time. The parser knows just
> **7 construct shapes** (`mod`, `struct`, `enum`, `trait`, `impl`, `fn`,
> `group`) and **2 statement shapes** (`call`, `if`). A layer keyword lexes as a
> plain identifier and is given meaning by mapping to one of those shapes.
>
> So this document has two halves: the **core language** (fixed keywords the
> engine reserves) and the **layer format** (how `.layer` files teach new
> vocabulary). If you only write applications, you mostly use layer vocabulary;
> the core keywords below are the scaffolding underneath it.

---

## 1. File kinds

VEIL source comes in three related formats, chosen by the first keyword:

| Extension | Top keyword | Purpose |
|-----------|-------------|---------|
| `.veil`   | `sol` or `pkg` | An application (`sol`) or a reusable package (`pkg`). |
| `.veil`   | `use` (first token) | A *composition* — imports and wires other packages. |
| `.layer`  | `pkg` | A layer: defines domain vocabulary (see §7). |
| `.stub`   | `stub` | Declares an external Rust crate's public API (see §8). |

Indentation is significant. Blocks are opened by indenting; `#` begins a
comment to end of line.

---

## 2. Core keywords — file & package level

### `pkg` — Package
The root of an application. `pkg <Name>` followed by an indented body.
(`sol` is accepted as a deprecated alias and produces identical output.)
```
pkg CustomerOnboarding
  use ddd
  ctx Identity
    ...
```
The body accepts `use`, `link`, `adapt`, `lang`, `type`, `const`, `flow`,
`fn`, `allow`/`deny`, adapt patches (`ins`/`rfn`/`rpl`/`omit`/`ren`),
`stock` (inside `rfn` only), and any construct from the loaded layers.
See [`ADAPT.md`](ADAPT.md) for product specialization.

### `pkg` — Package
A reusable, versioned unit: `pkg <Name> [<version>]`. Same body as `sol` plus
metadata lines and an `expose` block. Layer files are themselves packages.
```
pkg ddd v1
  desc "Domain-Driven Design abstraction layer"
  author "VEIL"
```

### `use` — Import / layer reference
`use <name> [as <alias>]`. In a `.veil` file, `use ddd` loads the vocabulary
from `ddd.layer` (or `ddd.stub`) so its constructs become available. As the
*first* token of a file it starts a composition. `as` aliases the import.
```
use ddd
use billing as bill
```

### `adapt` — Specialize a stock package (source merge)
`adapt <package>` pulls another package’s **sources** into this compile unit and
applies path patches (`ins` / `rfn` / `rpl` / `omit` / `ren`). Distinct from
`use` (API / layer boundary). Multi-level chains fully flatten before check and
codegen. `stock` splices the prior body inside `rfn` only.

```
pkg AcmeWearTest
  use ddd
  adapt wear_test
  ren ListInitiatives ListPrograms
  ins CreateInitiative
    step acme_audit after persist
      ret Ok
  rfn CreateInitiative
    step
      init = stock
      ret init
```

Canonical example: `examples/adapt/stock.veil` + `examples/adapt/client.veil`.
Full contract: [`ADAPT.md`](ADAPT.md).

### `link` — External Cargo crate (CAP-001)
`link <name> [path "…"] [features "a,b"]` declares a **path dependency** that
codegen emits into the generated Rust workspace `Cargo.toml`. Use this so
`@main` / adapters can call monorepo crates such as `veil-server`.

```
pkg VeilRuntimeHost
  use ddd
  link veil_server
  link veil_local path "../../crates/veil-local" features "local"
  link "my-sdk" path "../vendor/my-sdk"
```

| Form | Behavior |
|------|----------|
| `link veil_server` | Allowlisted monorepo crate; default path `../../crates/veil-server` (from gen workspace root) |
| `link name path "rel"` | Explicit relative path (required if not allowlisted) |
| `link name features "a,b"` | Optional Cargo features |

**Security**

- Absolute paths are rejected.
- Non-allowlisted crates **must** supply a relative `path`.
- Allowlist (omit path): `veil_server` / `veil-server`, `veil_local` / `veil-local`,
  `veil_parser`, `veil_ir`, `veil_codegen` (and hyphenated forms).

Codegen writes path deps under `[workspace.dependencies]` and
`name.workspace = true` on `veil_shared`, module crates, and `veil_bin`.
Rust import name uses underscores (`use veil_server::…`).

When a package has `link veil_server` **and** `@main`, rust codegen emits a
**ProductHost** bin (CAP-002/006) instead of the demo InProcessBus harness.

See `crates/veil-codegen/src/links.rs` and story CAP-001–007 in
`stories/141-pure-runtime-capability-gaps.md`.

### `adapt` — Specialize a stock package (planned)

**Design:** [`docs/ADAPT.md`](ADAPT.md) · **Stories:** [`stories/150-package-adapt.md`](../stories/150-package-adapt.md)

| Keyword | Role |
|---------|------|
| **`use`** | Depend on an API / layer boundary (may be remote Bus) — **not** source rewrite |
| **`adapt`** | Pull another package’s **source** into this compile unit and specialize it |

Patch ops (only on symbols that exist on the adapted base):

| Op | Role |
|----|------|
| **`ins`** | Insert sub-components (method, step, …) into an existing construct |
| **`rfn`** | Refine body; may splice **`stock`** (prior body inlined at transpile time) |
| **`rpl`** | Replace body entirely (`stock` illegal) |
| **`omit`** | Remove a base symbol or step from the product surface |
| **`ren`** | Rename a base symbol; rewrite references in the merged IR |

New top-level constructs need no special keyword — define them normally.

**`stock`** is a transpile-time splice of the ancestor body (statement or
expression form), not a runtime “super” call. Multi-level adapt fully flattens
before codegen (one function body, no parent frames).

```veil
pkg AcmeWearTest
  use ddd
  use dlx_core
  adapt wear_test

  ren ListInitiatives ListPrograms

  ins Initiative
    fn mark_vip()
      ...

  ins CreateInitiative
    step acme_audit after persist
      ...

  rfn CreateInitiative
    step
      init = stock
    step
      ret init

  rpl ArchiveInitiative
    step
      ret Ok

  omit SomeLegacyService

  svc AcmeReport
    ...
```

Platform packages (e.g. `dlx_core`) must not be adapted — only `use`d.

### `lang` — Glossary
A block of `term: definition` lines documenting domain terms. Metadata only;
does not affect codegen.
```
lang
  KYC: Know Your Customer verification
  Onboarding: process from signup to active customer
```

### `export` — Visibility modifier
A prefix on a construct (`+saga Onboard`) marking it as part of the
package's public surface. Written as `+` (or the legacy `export` keyword).

### `desc` — Description
A description line taking a string literal, used inside packages and exposed
nodes: `desc "Register a new customer"`.

---

## 3. Core keywords — package public API (`expose`)

Only valid inside a `pkg`. Declares the operations the package offers.

- **`expose`** — opens the public-API block.
- **`node <Name>`** — one exposed operation. Body accepts `desc`, `input`,
  `output`.
- **`input` / `output`** — indented `name: Type` field blocks for the node's
  parameters and results.
- **`cst`** — free-text constraint lines within `expose`.

```
expose
  node CreateCustomer
    desc "Register a new customer"
    input
      email: Email
    output
      customer_id: UUID
  cst
    flow-only
```

### `allow` / `deny`
Recognized in a solution body but currently **parsed-and-discarded** (the
keyword and its block are skipped). Reserved for future capability rules.

---

## 4. Core keywords — construct shapes

Every construct resolves to one of these shapes. The keyword you *write* (e.g.
`agg`) comes from a layer and maps to a shape; the shape keywords below are the
built-in vocabulary a layer maps onto.

### `struct` — a named type with fields
Fields are `name: Type` (or shorthand bare names, whose type is inferred). May
carry a `-> Type` line, nested `fn` methods, layer-declared named sub-blocks
(like `root`/`state`, see below), and nested constructs.
```
struct Customer
  id: UUID
  email: Email
```

### `enum` — variants, optionally a state machine
Variant lines, where `A -> B -> C` records both variants **and** transitions.
```
enum CustomerStatus
  Pending -> Verified -> Active
  Pending -> Rejected
```

Enums may also carry data — tuple variants and struct variants:
```
enum Message
  Text(Str)
  Image(Str, Int, Int)
  Embed
    url: Str
    title: Str
```

- `Variant` alone — unit variant (or state-machine with `->` transitions)
- `Variant(Type1, Type2)` — tuple variant
- `Variant` + indented `field: Type` lines — struct variant

State-machine enums (with transitions) and data-carrying enums are mutually
exclusive per definition.

### `trait` — an interface
A block of method signatures `name(params) -> Ret`.
```
trait Notifier
  send_sms(phone: Phone, msg: Str) -> Res!
```

### `impl` — an implementation of a trait
`impl <Name> for <Target>`, with `impl`-prefixed method bodies inside.
```
impl SmsTwilio for Notifier
  impl send_sms(phone, msg)
    http.post("api.twilio.com/Messages", {To: phone.number, Body: msg})
```

### `mod` — a module / container
Holds child constructs and groups only (no fields). Domain keywords like `ctx`
(bounded context) map to `mod`.

### `group` — a visual/organizational container
A reserved shape (`group <name>`) that holds child constructs. Used to
partition a module (e.g. `group domain`, `group infrastructure`).

### `fn` — a function or flow-shaped construct
Two forms:
1. **Code function** — `fn name(params) -> Type` with an expression body.
   Usable at top level, nested in a struct (aggregate methods), or in a layer's
   `declare` block (e.g. the saga coordinator).
   ```
   fn verify(code: Str) -> Res!
     @invariant(status == Pending)
     status = Verified
     emit CustomerVerified{id, now()}
   ```
2. **Flow-shaped construct** — a `fn`-mapped layer keyword (like `svc` or
   `saga`) with `input`, `step`/`par`, and `ret` in its body (see §6).

### `flow` — a core flow
Like a `fn`-shaped construct but built in: `input`, `step`/`par` blocks, an
`err` boundary, and `ret`.

### `type` — type alias
`type ExternalId = Str`.

### `const` — constant
`const MAX_RETRIES = 3`.

---

## 5. Core keywords — statements & control flow

These are the expressions that appear inside function/step bodies.

### Invocations
`Target.method(args)` or `Target(args)` — a function or method call.
Method chaining: `a.b(x).c(y)`.
```
c = Customer.new(email, phone)
CustomerRepo.save(c)
```

### `mut` — mutable binding
`mut total = 0`. A subsequent `total = total + 1` is a reassignment. (Plain
`name = expr` is an assignment expression; there is no separate `let` — see the
note below.)

### `match` — pattern match
`match <scrutinee>` with indented `pattern -> body` arms. Multi-statement arms
use a deeper indent.
```
match step.action(bus, state)
  Ok next ->
    state = next
  Err e ->
    ret Err(e)
```

### `for` — iteration
`for item in <iterable>` or `for i, item in <iterable>` (with index) + block.
`in` is matched positionally, not reserved.

### `while` — loop
`while <condition>` + block.

### `ret` — return
`ret <expr>`. Bare `ret Ok` / `ret Err(e)` construct the Result directly;
`ret <value>` wraps in `Ok(...)`.

### `await` — await an async expression
`await fetch(url)` → `fetch(url).await` in Rust.

### `true` / `false` — boolean literals

### Reserved but not (yet) wired
`let`, `alt` are recognized by the lexer but have **no parser behavior** — do
not use them.

### Additional core expressions (fully wired)

**Conditionals:**
- **`if` / `else`** — standard conditional: `if cond` + body, optional `else` + body. Supports `else if` chaining.
- **`if let pattern = expr`** — pattern-matching conditional.

**Loops:**
- **`for binding in iterable`** — iteration. Optional index: `for i, item in items`.
- **`while condition`** — conditional loop.
- **`while let pattern = expr`** — pattern-matching loop.
- **`loop`** — infinite loop. Use `break` to exit.
- **`break`** — exit current loop.
- **`continue`** — skip to next iteration.

**Pattern matching:**
- **`match scrutinee`** — with indented `pattern -> body` arms.
- Or-patterns: `A | B -> body`
- Match guards: `x if x > 5 -> body`
- Variant destructuring: `Some(val) -> use(val)`
- Struct destructuring: `Customer { id, email, .. } -> use(id)`
- Tuple destructuring in let: `(a, b) = get_pair()`
- Wildcard: `_ -> default_case`

**Expressions:**
- **`expr?`** — try operator (propagate errors).
- **`expr as Type`** — type cast.
- **`expr[index]`** — index access.
- **`[a, b, c]`** — array literal.
- **`(a, b, c)`** — tuple literal.
- **`start..end`** / **`start..=end`** — range expressions.
- **`|params| body`** — closures (single-line or multi-line).
- **`await expr`** — await an async expression.
- **`f"Hello {name}"`** — string interpolation.
- **`Name { field: val, ..base }`** — struct literal / update.

**Declarations:**
- **`mut name = expr`** — mutable binding.
- **`mut name: Type = expr`** — mutable binding with explicit type annotation.
- **`(a, b) = expr`** — tuple destructuring (let pattern).
- **`Name { field, .. } = expr`** — struct destructuring.
- **`type X = Y`** — type alias.
- **`const NAME = value`** — constant.
- **`static [mut] NAME = value`** — static variable.

---

## 6. Flow bodies — `input`, `step`, `par`, `err`

Inside a `fn`-shaped construct or `flow`:

- **`input`** — an indented `name: Type` block declaring the flow's parameters.
- **`step <name>`** — a named unit of work; its body holds statements,
  reference lines (e.g. `ctx Identity`), and named sub-blocks (e.g.
  `compensate`).
- **`par`** — a block of `step`s intended to run in parallel.
- **`err` (`err boundary`)** — an error-handling boundary; may contain a
  `fallback -> <expr>`.

```
saga Onboard
  input
    email: Email
  step create_customer
    id = invoke CreateCustomer{email}
    compensate
      invoke DeleteCustomer{id}
```

> **`step` and `par` are not reserved words.** They are flow-modeling
> vocabulary recognized *contextually* (`step` followed by a name; `par` alone
> on its line). You may freely use `step` or `par` as ordinary variable names
> elsewhere.
>
> **`root` and `state` are not keywords either.** They are examples of
> *named sub-blocks* a layer declares via `has` (`root: struct`,
> `state: enum`); the parser matches them by name.

---

## 7. Types

Types are written with a compact syntax. Built-in names map to Rust:

| VEIL | Rust | | VEIL | Rust |
|------|------|-|------|------|
| `Str` | `String` | | `UUID` | `Uuid` |
| `Int` | `i64` | | `DateTime` | `DateTime<Utc>` |
| `F64` | `f64` | | `Bytes` | `Vec<u8>` |
| `Bool` | `bool` | | `Json` | `serde_json::Value` |

Type constructors:

| Form | Meaning | Rust |
|------|---------|------|
| `Res!` | fallible, no payload | `Result<(), DomainError>` |
| `Res!<T>` | fallible, carrying `T` | `Result<T, DomainError>` |
| `Opt<T>` | optional | `Option<T>` |
| `List<T>` | list | `Vec<T>` |
| `Set<T>` | set | `HashSet<T>` |
| `Map<K, V>` | map | `HashMap<K, V>` |
| `Name<A, B>` | generic | `Name<A, B>` |
| `(A, B, C)` | tuple | `(A, B, C)` |
| `[T; N]` | fixed-size array | `[T; N]` |
| `&T` | shared reference | `&T` |
| `&mut T` | mutable reference | `&mut T` |
| `dyn Trait` | dynamic dispatch | `dyn Trait` |
| `impl Trait` | opaque return | `impl Trait` |
| `fn(A, B) -> C` | function pointer | `fn(A, B) -> C` |

Any other capitalized name is a user/domain type and passes through by name.

### Generics, type aliases, and monomorphized adapters

```veil
# Generic port + generic adapter with real VEIL method bodies
trait EntityRepo<T>
  find!(id: Id) -> Opt<T>
  save!(entity: T)
  delete!(id: Id)

adapter DynamoJsonRepo<T> for EntityRepo<T>
  @field(client: Client)
  @env(DYNAMO_TABLE)
  impl find(id)
    # … VEIL body; T is a type parameter
    entity = serde_json.from_str(payload)
    ret entity
  # …

# Product monomorphization for DI
type WearTestRepo = EntityRepo<WearTest>

# Empty monomorphized adapter: codegen copies VEIL bodies from DynamoJsonRepo<T>
# substituting T → WearTest (works for any generic class, not Dynamo-specific).
adapter DynamoWearTestRepo for EntityRepo<WearTest>
  @field(client: Client)
  @env(DYNAMO_TABLE)
```

Codegen does **not** invent SDK-specific method bodies. It only substitutes type
parameters and lowers authored VEIL.

> The `!` result marker is what makes a type fallible; `Res` is the
> conventional name written before it (`Res!` / `Res!<T>`). Mechanically the
> parser treats any `Name!` as a unit result and `Name!<T>` as a result
> carrying `T` — the name before `!` is by convention `Res`.

---

## 8. Operators

| Category | Operators |
|----------|-----------|
| Arithmetic | `+` `-` `*` `/` `%` |
| Comparison | `==` `!=` `<` `>` `<=` `>=` |
| Logical | `&&` `\|\|` `!` |
| Assignment | `=` |
| Type/return | `->` (return type / transition), `!` (result marker) |
| Access/grouping | `.` `:` `,` `( )` `{ }` `[ ]` `< >` |
| Range | `..` (exclusive), `..=` (inclusive) |
| Try | `?` (error propagation) |
| Cast | `as` (type coercion) |
| Closure params | `\|params\| body` |

### Annotations
`@name` or `@name(args)` attach metadata to the following construct or method.
Which annotations are available per construct is declared by the layer.
```
@invariant(status == Pending)
@env(TWILIO_SID, TWILIO_TOKEN)
```

### String interpolation
`f"Hello {name}"` interpolates expressions.

---

## 9. The `.layer` format — teaching new vocabulary

Team DSL authoring and IDE parity: see [`docs/LAYERS_DSL.md`](LAYERS_DSL.md) and
stories [`110-layer-dsl-ide.md`](../stories/110-layer-dsl-ide.md).

A layer is a `pkg` whose body declares `construct`s and `statement`s. This is a
separate, line-oriented mini-format (not the expression grammar above).

### `construct <Name>` — a new noun
```
construct Aggregate
  kw agg                 # the source keyword users write
  mt struct              # resolves to a core shape (or another construct)
  in Context          # which container it may appear in
  desc "Aggregate root ..."   # human description
  contains                    # allowed children + named sub-blocks
    root: struct              #   `kw: shape` declares a named sub-block
    state: enum
    fn[]                      #   `[]` = repeatable
    Event[]
  constraints                 # free-text semantic rules (advisory)
    must_have root

Notably **not** reserved (they lex as identifiers): `step`, `par`, `root`,
`state`, and all domain vocabulary (`ctx`, `agg`, `port`, `saga`, `dispatch`,
`guard`, …), which is defined entirely in `.layer` files.
  visual                      # viewer styling
    icon "🧩"
    color "#ec4899"
    label "Aggregate"
  annotations                 # annotations offered in the editor
    invariant: "Domain constraint" expr
  group domain                # palette grouping label
```

Field meanings:

- **`kw`** — the word users write (defaults to the construct name).
- **`mt`** — the core shape (`mod`/`struct`/…) or a parent construct this
  resolves to. Chains resolve transitively (`lead → agg → struct`). The special
  value `primitive` means "I *am* the core shape named by my keyword."
- **`in`** — legal parent container (`top` = solution level).
- **`has`** — allowed child constructs; a `keyword: shape` line declares a
  named sub-block (like `root`/`state`); `[]` marks a repeatable child.
- **`cst`** — advisory semantic rules (e.g. `must_have root`).
- **`visual`** → `icon` / `color` / `label` — how the viewer renders it.
- **`ann`** — `name: "desc" p1, p2` lines; offered in the property
  editor.
- **`group`** — a palette-grouping label (also used by presentation
  `members by_source_group` / tab partitions).
- **`dg`** — default group for create/placement of impl-shaped constructs.
- **`present`** — **layer-driven IDE views** (hierarchy, tabs, layouts, nest
  rules, lenses). Normative grammar and semantics:
  **[`docs/PRESENTATION.md`](./PRESENTATION.md)** (LAY-001). Not yet required
  for layers to load; until LAY-002, treat as the locked design for implementers.
- **`runtime <coordinator> <step_trait>`** — for delegated fn-shaped constructs
  (e.g. `saga`): steps are lowered to `impl <step_trait>` blocks and handed to
  the coordinator function. Nested `sub_block -> method` lines map a step's
  sub-blocks (e.g. `compensate`) to trait methods.

### `statement <kw>` — a new verb
```
statement dispatch
  mt Bus.dispatch        # `Port.method` → desugars to a bus call
  desc "Fire a domain event"
```
- **`mt`** — a core statement shape (`call`/`if`), another statement, or a
  `Port.method` that the statement desugars into.
- **`sem`** — describes runtime meaning.

### `declare` — inject concrete constructs
Raw VEIL blocks that get parsed and injected into every solution using the
layer. Used for shared infrastructure — e.g. `ddd.layer` declares the `Bus`
port and the `SagaStep` trait + `run_saga` coordinator entirely in VEIL:
```
declare
  trait Bus
    dispatch(evt: Json) -> Res!
    invoke(cmd: Json) -> Res!<Json>
```

### `prompt` — LLM guidance for code generation
Free-form text that teaches AI agents how to use the layer. Ignored by the
compiler and codegen toolchain — stored in the `LayerRegistry` for retrieval
by LLM-based code generation systems (RAG context).

```
prompt
  You are writing code using the DDD layer for VEIL.

  ## Structure
  Every pkg using `use ddd` organizes code into bounded contexts...

  ## Key Patterns
  - Dependencies are always injected via @dep
  - Repos return Opt<T> for single lookups, List<T> for collections
  ...
```

The prompt content is indented under the `prompt` keyword. All indented lines
are accumulated as-is (leading indent stripped). Multiple layers' prompts are
concatenated in load order when building context for an LLM.

### `use` (in a layer)
Loads a dependency layer, so layers can stack (`crm.layer` builds on
`ddd.layer`).

---

## 10. The `.stub` format — external crate APIs

A `.stub` file declares a third-party Rust crate's public API so VEIL's type
inference and codegen can use it. The `veil stub-gen <crate_name>` CLI command
generates these automatically from rustdoc JSON (requires nightly).

**Do not hand-edit generated stubs for policy** — re-run `veil stub-gen`. The
generator emits both API shapes and **codegen policy** inferred from the crate
(traits, free functions, type aliases). The engine applies that policy
generically (no `sqlx::…` hardcoding in `rust.rs`).

Keywords: `stub <name> <version>`, crate-level policy lines, then `struct` /
`impl` / `trait` blocks with `fn` signature lines.

```
stub reqwest 0.12
  struct Client

  struct Response
    fn status() -> StatusCode
    fn text() -> Res!<Str>
    fn json() -> Res!<T>

  impl Client
    fn new() -> Client
    fn get(url: Str) -> RequestBuilder
    fn post(url: Str) -> RequestBuilder

  impl RequestBuilder
    fn header(name: Str, value: Str) -> RequestBuilder
    fn send() -> Res!<Response>
```

**Crate-level policy (auto-inferred by stub-gen when possible):**

| Directive | Meaning |
|-----------|---------|
| `cargo_features a, b` | Features for workspace `Cargo.toml` |
| `row_type_derives Path` | Multi-field domain types get these derives (`FromRow` trait present) |
| `wrapper_type_derives Path` | Single-field wrappers get these derives (`Type` trait present) |
| `wrapper_type_attrs inner` | Extra attrs on wrappers (e.g. `crate(transparent)`) |
| `codegen_imports Path` | Extra `use` lines (from type aliases like `PgPool`) |
| `rust_name Veil Rust` | VEIL type → Rust name (`Pool` → `PgPool`) |
| `harness_field Type """…"""` | Local harness construction recipe |

**On a struct (auto when free fns `query` + `query_as` exist for `Query`):**

```
struct Query
  typed_variant query_as
  typed_type_params _, return_type
  fn new(sql: Str) -> Self   # VEIL sugar → free fn `query`
```

**How stubs integrate:**
- Referenced via `use reqwest` in a `.veil` file (loads `reqwest.stub`)
- Type inference learns method return types (e.g. `Client.get()` → `RequestBuilder`)
- Codegen adds the crate to `Cargo.toml` dependencies
- The `veil stub-gen` command creates a temp project, runs `cargo +nightly rustdoc
  --output-format json`, and converts the JSON to `.stub` format automatically

---

## Appendix — complete core keyword list

The definitive list of words the **lexer** reserves (everything else is an
identifier / layer vocabulary):

`struct` `enum` `fn` `trait` `let`* `mod` `if` `else` `match` `ret` `true`
`false` `impl` `sol` `pkg` `use` `link` `adapt` `lang` `expose` `node` `flow` `alt`* `loop`
`err` `call` `input` `fallback` `for` `while` `mut` `type` `const` `await`
`break` `continue` `static` `boundary` `as` `desc` `output` `cst`
`group` `allow` `deny` `export` `ins` `rfn` `rpl` `omit` `ren` `stock`

`*` = reserved by the lexer but not fully wired into the parser; do not use.

Adapt family (`adapt`/`ins`/`rfn`/`rpl`/`omit`/`ren`/`stock`): see [`ADAPT.md`](ADAPT.md).

### Source of truth
- Core token list: `crates/veil-parser/src/lexer.rs` — `keyword_lookup`.
- Parse behavior: `crates/veil-parser/src/parser.rs`.
- Expression types: `crates/veil-ir/src/ast.rs` — `Expr` enum (34 variants).
- Type system: `crates/veil-ir/src/ast.rs` — `TypeExpr` enum (13 variants).
- Shapes, layer format, type mapping: `crates/veil-ir/src/layer.rs`,
  `crates/veil-codegen/src/rust.rs` (`type_to_rust`).
- Stub format: `crates/veil-ir/src/layer.rs` — `parse_stub_file`.
- Visual editors: `veil-viewer/src/lib/editors/` — composable components.
- Layer presentation (views / nest / layout): `docs/PRESENTATION.md`.

---

## 11. Token Efficiency

VEIL is designed to minimize token count. The forms below are the standard
way to write VEIL — use them exclusively.

### Calls are bare expressions
`Target.method(args)` — no keyword prefix needed:
```
c = Customer.new(email, phone)
CustomerRepo.save(c)
```

### `+` marks public/exported constructs
```
+saga Onboard
+svc CreateCustomer
```

### `!` marks fallible methods
A `!` after the method name means fallible (`-> Res!` / wrap return in `Res!<…>`):
```
save!(customer: Customer)
find!(id: Id) -> Opt<Customer>
```

**Full law (decl + call + Opt unwrap policy):** [`BANG_CONTRACT.md`](./BANG_CONTRACT.md).  
Do not call `.unwrap()` / `.is_some()` on the result of a bang call when dual-loop
codegen has already forced `Opt` to `T`.

### Preferred type names
| Use | Meaning |
|-----|---------|
| `Id` | UUID |
| `Dt` | DateTime |
| `Str` | String |
| `Int` | i64 |
| `F64` | f64 |
| `Bool` | boolean |

### Bare field names infer types
Fields without explicit types are inferred by convention:
| Pattern | Type |
|---------|------|
| `id`, `*_id` | Id |
| `created`, `updated`, `*_at` | Dt |
| `is_*`, `has_*`, `can_*`, `active` | Bool |
| `count`, `total`, `amount`, `score` | Int |
| `email`, `url`, `name`, `title` | Str |

```
agg Customer
  root
    id email created
```

### Complete example (minimal token form)
```
+saga Onboard
  step create
    c = Customer.new(email, phone)
    CustomerRepo.save(c)
    dispatch CustomerCreated{c.id, email, c.created}
```
