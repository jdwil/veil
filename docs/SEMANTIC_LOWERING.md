# Semantic lowering (codegen honesty)

How VEIL IR becomes target code **without lying**.  
Companion to [BANG_CONTRACT.md](./BANG_CONTRACT.md) and [ENGINE.md](./ENGINE.md).

---

## Goal

Agents must trust:

1. **What typecheck says** matches **what codegen emits**
2. **Green compile** is not purchased by constant-folding or dropping statements
3. **Cross-context** `invoke`/`request` resolve to real handlers when the package defines them

If a fix reintroduces “emit `true` so it typechecks,” it is a regression.

---

## Invariants (pin these)

**Bang law:** ACS-010 portable is the **current engine default** (see
[BANG_CONTRACT.md](./BANG_CONTRACT.md)). Force-present is `require` / `.unwrap()`
/ layer policy — not silent NotFound on every `!`.

| ID | Rule | Where enforced |
|----|------|----------------|
| **SL-001** | Bang strips `Res!` only; `Opt` stays `Opt` | typecheck + rust codegen |
| **SL-002** | `if cond then expr` does not require `else` | parser |
| **SL-003** | `ret null` on `Opt` → `Ok(None)`; on non-Opt → `Err(NotFound)` | codegen |
| **SL-004** | Bus `null` fields are JSON null, not the string `"null"` | `to_json_arg` |
| **SL-005** | Known `invoke Msg` decodes to domain type when return is known | codegen `bus_returns` |
| **SL-006** | `invoke`/`request` target must name a svc/tool in the package | typecheck `missing_handler` |
| **SL-007** | REST paths unique across multi-context harness (collision → `/api/{crate}/…`) | bin codegen |
| **SL-008** | `is_some`/`is_none` on non-`Opt` → error | typecheck `opt_method_on_non_opt` |
| **SL-009** | `require` force-presents one Opt **and** one Res. Bang already tries Res; leftover `Opt` still unwraps | rust codegen `Expr::Require` |
| **SL-010** | `Res!<Str>` getters (`as_s` / stub-typed) own a `String` and `map_err` via Debug, never Display | rust codegen |
| **SL-011** | String-pattern `match` keeps the try-unwrap, then `.as_str()` — never `.as_str()` on `Result` | rust codegen |
| **SL-012** | `Str + Str` / `"lit" + field` → `format!("{}{}", …)`, not Rust `+` | rust codegen |
| **SL-013** | `pkg.Type` uses stub `rust_type_path` (`types_module` / per-struct `path`). Product stubs inherit unset policy from the system stub | rust codegen + `fill_stub_gaps_from_system` |
| **SL-014** | `Str.now_iso8601()` / `Dt.now_iso8601()` is the current UTC instant as an ISO-8601 `Str` (`Utc::now().to_rfc3339()`). Not an unstubbed external | rust codegen + typecheck |
| **SL-015** | Bytes-view `as_ref` in a `Str` slot (`stub → Str`, Blob/Bytes return, or enclosing `Str`) decodes utf-8. Never emit raw `&[u8]` as `String`. `Option`/`Result`/`String` `.as_ref()` is unchanged | rust codegen + typecheck |
| **SL-016** | VEIL field reads are reusable. Non-Copy `x.field` clones; do not move a field into the first call and fail the second | rust codegen |
| **SL-017** | `Int.now_unix()` is the current UTC unix timestamp (`i64`). `s.parse_int()` parses a `Str` to `Int` | rust codegen + typecheck |
| **SL-018** | `as_n` is the numeric extractor: Debug-map_err, own the text, then `parse::<i64>()`. `as_s` stays `Str` | rust codegen |
| **SL-019** | rustdoc stub params named with a single uppercase letter (`U`, `T`, `B`) are type parameters (`impl IntoUrl`), not constructable types. They accept any VEIL value (typically `Str`) | typecheck |
| **SL-020** | Sibling `match` / `if` arms each get their own first-bind set. The same name bound independently in two arms is `let`, not `let mut`. A name bound *before* the fork and reassigned in an arm is still `mut` | rust codegen `analyze_mut_locals` |
| **SL-021** | `Str + Str + …` flattens to one `format!("{}{}…", …)`, not nested `format!("{}{}", format!(…), …)` | rust codegen |
| **SL-022** | `tests Target` / `it` / `stub Port.method` / `given` / `then` emit `crates/{crate}/src/tests.rs` that calls `application::{target}` with port test-doubles. Smoke is `cargo check --tests` | rust codegen + host smoke |
| **SL-023** | `hook` (`role:deploy_hook`) emits `crates/veil_hooks` and is **absent** from `HANDLER_NAMES`. Host runs the bin after compile, before `deploy_code`, deps-first, fail closed. Engine dumps a **typed** `DeployContext.constructs` list; it does not interpret product annotation names | rust codegen + `crates/deploy` |
| **SL-024** | Layer-declared types (`DeployContext`, …) re-export from `veil_shared`. Never emit `pub type Name = String` for them — that shadows the real struct | rust codegen `gen_types` |
| **SL-025** | `Json.parse(s)` / `s.parse_json()` is Str→`serde_json::Value`. `Json.stringify(x)` is the inverse. Do not `use serde_json` as a stub | rust codegen + typecheck |
| **SL-026** | `require json.field` extracts a Str (`as_str().map.ok_or?`). `as_s`/`as_str` on Json never go through `as_ref`/bytes. `list[i]` / `list.first()` / `list.get(i)` own the element (`.cloned()`) | rust codegen |
| **SL-027** | Layer-provided types are not package escape debt. Product constructs must not reuse `declare` names (`shadows_layer_declare` rejects before rustc). Codegen never emits a local struct that shadows `veil_shared`. Teaching roots = package `use` + R21 implicit primary layer. Empty session ≠ parse error; scope tools append Tier-1 teaching. `require` on a Json field infers `String` so later struct fields are not coerced with `as_str().unwrap_or` on a String | check + codegen + host preamble |
| **SL-028** | Generated Rust is idiomatic **and rustc-clean**: never `.clone().clone()`; string `==` uses a bare lit; unit-only enums derive `Copy` and are not cloned (including fields and variants); `list[i]` is `.get(0)` not `.get(0 as usize)`; Json fields are indexed in place (`stack["k"]`); last/only ident uses move; struct shorthand does not force `.clone()`; for-loops iterate Vec/List **fields** by shared ref; **do not** prefix `&` on Calls (`result.items()` already returns `&[T]`); match/if arm `Str` values are owned (`"x".to_string()`) so they typecheck as `String` | rust codegen |

---

## Deploy hooks (SL-023)

`hook ConfigureX` (layer keyword; engine matches **`role:deploy_hook` only**) lowers to
an application fn plus `crates/veil_hooks`. The provisioner runs that binary after
`compile` and before `deploy_code`. Transitive `[dependencies]` run **deps first**
with the **consumer** `DeployContext` JSON (`VEIL_DEPLOY_CONTEXT`).

`DeployContext` is layer-declared: `service_name`, `environment`, `resource_prefix`,
`stack: Json`, `units: Json`, `constructs: List<DeployedConstruct>`
(`annotations: List<{name, args, roles}>`). Hooks iterate `context.constructs`.
They do **not** parse a JSON string and they do **not** get `pub type DeployContext = String`.

`Json.parse(s)` / `s.parse_json()` is the language primitive for leftover JSON
strings. Do not stub-gen `serde_json`.

Hooks are not bus handlers and are not uploaded as Lambda code.

`veil_hooks` instantiates every adapter (nested `@dep` / `@field` still need them)
but fills `application::Deps` with **only** the ports handlers/hooks `@dep`.
Every `@env` on an adapter becomes a field (not just the first annotation).

---

## Bus invoke typing

Desugar:

```
plan = invoke Reconcile{desired, environment, new_artifact_hash}
```

→ `Bus.invoke(Reconcile{…})` → JSON envelope →  
`serde_json::from_value::<ReconcileResult>(deps.bus.invoke(...).await?)?`  
when `Reconcile` is registered with return type `ReconcileResult`.

If the service returns `Json` (or unknown), leave `serde_json::Value`.

Handler registration in `veil_bin` must still implement the message name list
(`register_handlers` + real `bus.register` closures).

---

## Regression harness

| Test | Purpose |
|------|---------|
| `veil-ir` bang_* tests | Portable Opt law |
| `codegen_tests` `adapter_if_then_without_else_*` | Empty Opt returns |
| `codegen_tests` `rest_routes_pluralize_*` | branch → branches |
| `codegen_tests` `flow_return_type_from_bang_*` | bang keeps Option |
| `codegen_tests` `runtime_semantic_snapshots` | runtime.veil lowering pins |

Run:

```bash
cargo test -p veil-ir --lib bang_
cargo test -p veil-codegen --test codegen_tests
make pure-runtime-build   # if present: gen + cargo check runtime/generated
```

---

## Anti-patterns (do not reintroduce)

1. Constant-folding `is_some` → `true` because the local was force-unwrapped  
2. Dropping `if then` statements when `else` is missing  
3. Emitting handler **names** without registering closures  
4. Papering over type errors with `Default::default()` for table/bucket without env  
5. Hand-editing `runtime/generated` instead of engine + re-gen  

---

## When adding a fix

1. Name the invariant (or add one to this table)  
2. Typecheck (if it is a VEIL-level rule)  
3. Codegen  
4. Unit or snapshot test that fails if the lie returns  
5. Update this doc or BANG_CONTRACT if the law changes  
