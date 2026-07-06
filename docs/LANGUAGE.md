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

### `sol` — Solution
The root of an application. `sol <Name>` followed by an indented body.
```
sol CustomerOnboarding
  use ddd
  ctx Identity
    ...
```
The body accepts `use`, `lang`, `type`, `const`, `flow`, `fn`, `allow`/`deny`,
and any construct from the loaded layers.

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

### `lang` — Glossary
A block of `term: definition` lines documenting domain terms. Metadata only;
does not affect codegen.
```
lang
  KYC: Know Your Customer verification
  Onboarding: process from signup to active customer
```

### `export` — Visibility modifier
A prefix on a construct (`export saga Onboard`) marking it as part of the
package's public surface. Not a construct itself.

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
- **`constraints`** — free-text constraint lines within `expose`.

```
expose
  node CreateCustomer
    desc "Register a new customer"
    input
      email: Email
    output
      customer_id: UUID
  constraints
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

### `call` — invocation
The one core invocation primitive: `call Target[.method]([args] | {named})`.
Supports method chaining (`call a.b(x).c(y)`).
```
c = call Customer.new(email, phone)
call CustomerRepo.save(c)
```

### `mut` — mutable binding
`mut total = 0`. A subsequent `total = total + 1` is a reassignment. (Plain
`name = expr` is an assignment expression; there is no separate `let` — see the
note below.)

### `match` — pattern match
`match <scrutinee>` with indented `pattern -> body` arms. Multi-statement arms
use a deeper indent.
```
match call step.action(bus, state)
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
`await call fetch(url)` → `fetch(url).await` in Rust.

### `true` / `false` — boolean literals

### Reserved but not (yet) wired
`let`, `alt` are recognized by the lexer but have **no parser behavior** — do
not use them.

### Additional core expressions (fully wired)

- **`if` / `else`** — standard conditional: `if cond` + body, optional `else` + body. Supports `else if` chaining and `if let pattern = expr`.
- **`loop`** — infinite loop: `loop` + body. Use `break` to exit.
- **`break`** — exit current loop.
- **`continue`** — skip to next iteration.
- **`expr?`** — try operator (propagate errors).
- **`expr as Type`** — type cast.
- **`expr[index]`** — index access.
- **`[a, b, c]`** — array literal.
- **`start..end`** / **`start..=end`** — range expressions.
- **`if let pattern = expr`** — pattern-matching conditional.
- **`while let pattern = expr`** — pattern-matching loop.

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
> *named sub-blocks* a layer declares via `contains` (`root: struct`,
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

Any other capitalized name is a user/domain type and passes through by name.

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
| Access/grouping | `.` `:` `,` `( )` `{ }` `< >` |
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

A layer is a `pkg` whose body declares `construct`s and `statement`s. This is a
separate, line-oriented mini-format (not the expression grammar above).

### `construct <Name>` — a new noun
```
construct Aggregate
  keyword agg                 # the source keyword users write
  maps_to struct              # resolves to a core shape (or another construct)
  allowed_in Context          # which container it may appear in
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

- **`keyword`** — the word users write (defaults to the construct name).
- **`maps_to`** — the core shape (`mod`/`struct`/…) or a parent construct this
  resolves to. Chains resolve transitively (`lead → agg → struct`). The special
  value `primitive` means "I *am* the core shape named by my keyword."
- **`allowed_in`** — legal parent container (`top` = solution level).
- **`contains`** — allowed child constructs; a `keyword: shape` line declares a
  named sub-block (like `root`/`state`); `[]` marks a repeatable child.
- **`constraints`** — advisory semantic rules (e.g. `must_have root`).
- **`visual`** → `icon` / `color` / `label` — how the viewer renders it.
- **`annotations`** — `name: "desc" p1, p2` lines; offered in the property
  editor.
- **`group`** — a palette-grouping label.
- **`runtime <coordinator> <step_trait>`** — for delegated fn-shaped constructs
  (e.g. `saga`): steps are lowered to `impl <step_trait>` blocks and handed to
  the coordinator function. Nested `sub_block -> method` lines map a step's
  sub-blocks (e.g. `compensate`) to trait methods.

### `statement <kw>` — a new verb
```
statement dispatch
  maps_to Bus.dispatch        # `Port.method` → desugars to a bus call
  desc "Fire a domain event"
```
- **`maps_to`** — a core statement shape (`call`/`if`), another statement, or a
  `Port.method` that the statement desugars into.
- **`semantics`** — describes runtime meaning.

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

### `use` (in a layer)
Loads a dependency layer, so layers can stack (`crm.layer` builds on
`ddd.layer`).

---

## 10. The `.stub` format — external crate APIs

A `.stub` file declares a third-party Rust crate's surface so VEIL adapters can
call it. Keywords: `stub <name> <version>`, then `struct` / `impl` / `fn`
signature lines.
```
stub reqwest 0.12
  struct Response
    fn status() -> StatusCode
    fn text() -> Res!<Str>
```

---

## Appendix — complete core keyword list

The definitive list of words the **lexer** reserves (everything else is an
identifier / layer vocabulary):

`struct` `enum` `fn` `trait` `let`* `mod` `if` `else` `match` `ret` `true`
`false` `impl` `sol` `pkg` `use` `lang` `expose` `node` `flow` `alt`* `loop`
`err` `call` `input` `fallback` `for` `while` `mut` `type` `const` `await`
`break` `continue` `static` `boundary` `as` `desc` `output` `constraints`
`group` `allow` `deny` `export`

`*` = reserved by the lexer but not wired into the parser; do not use.

### Source of truth
- Core token list: `crates/veil-parser/src/lexer.rs` — `keyword_lookup`.
- Parse behavior: `crates/veil-parser/src/parser.rs`.
- Shapes, layer format, type mapping: `crates/veil-ir/src/layer.rs`,
  `crates/veil-codegen/src/rust.rs` (`type_to_rust`).
