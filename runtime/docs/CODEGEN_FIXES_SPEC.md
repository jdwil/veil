# VEIL Codegen Fixes — Application Layer Type Awareness

## Overview

The VEIL codegen produces correct domain types, port traits, and adapter signatures.
However, the **application layer** (service/tool step bodies) has systematic issues
because `expr_to_rust()` in `crates/veil-codegen/src/expr.rs` is largely type-unaware —
it translates AST nodes to Rust syntax without knowing the concrete types of expressions.

This spec covers the remaining compilation errors in the generated `meta_execution`
and `deploy` crates (from `runtime/src/runtime.veil`), plus pre-existing issues in
the `storage` crate that share the same root causes.

## How to Reproduce

```bash
cd /home/jd/dev/jd/veil
make pure-runtime-build                    # runs gen + cargo build
# Or individually:
./target/release/veil gen runtime/src/runtime.veil -o runtime/generated -t rust
sed -i '/^aws-config = "1\.8"$/d' runtime/generated/Cargo.toml
cd runtime/generated
cargo build -p meta_execution -p deploy    # see errors
cargo build -p storage                     # pre-existing errors (same root causes)
```

## Issue 1: Nested Struct Literals Lose Field Values in Bus Invoke Context

### Symptom
```rust
// Generated (WRONG):
new();
new();
let timeout = timeout_ms.unwrap_or(30000);
unwrap_or(serde_json::Value::Null);
```

### VEIL Source
```veil
subprocess_input = SubprocessInput{input, capabilities: ResolvedCapabilities{services: Map.new(), storage: Map.new(), bus_emit: []}}
output = SubprocessOutput{success: true, output: input, error: null, emitted_events: []}
ret ExecutionResult{output: output.output.unwrap_or(Json.null()), ...}
```

### Root Cause
In `expr.rs`, when a service invokes another service within the same context, the
codegen uses the **Bus invoke pattern** (JSON serialization). Struct literal field
values that are complex expressions (method calls, nested struct literals) get
decomposed into separate statements instead of remaining as field initializers.

The issue is in the `Expr::StructLit` → `json_message()` path (around line 1632)
and in how `Expr::Call` targets like `Map.new()` are handled when they appear as
values inside a `serde_json::json!({...})` macro invocation.

### Fix Direction
In `json_message()` (search for `fn json_message`), when serializing struct fields
into a `serde_json::json!({...})` call:
- Field values that are `Expr::Call` with lang-primitive targets (Map, List, etc.)
  should be translated inline: `Map.new()` → `serde_json::json!({})`
- Field values that are nested `Expr::StructLit` should recursively serialize
- Field values that are method calls on other expressions should be translated
  inline as well

Also check `to_json_arg()` (called from the anonymous struct literal `Expr::StructLit`
handler at ~line 948) — it may need similar treatment.

---

## Issue 2: Bus Invoke Returns `serde_json::Value` — Typed Field Access Fails

### Symptom
```rust
error[E0609]: no field `state` on type `serde_json::Value`
error[E0609]: no field `version` on type `serde_json::Value`
error[E0425]: cannot find value `metadata` in this scope
```

### VEIL Source
```veil
result = invoke GetDeploymentStatus{environment, unit_name}
ret {state: result.state, recent_events: result.recent_events, ...}
```

### Root Cause
`invoke ServiceName{...}` generates a Bus call that returns `serde_json::Value`.
When the VEIL code then accesses fields on the result (`result.state`,
`result.version`), the codegen emits direct Rust field access which doesn't work
on `serde_json::Value`.

### Fix Direction
When generating field access (`Expr::FieldAccess`) where the base expression is
known to come from a Bus invoke (check if the local was assigned from a
`deps.bus.invoke(...)` call), emit JSON indexing instead:

```rust
// Instead of: result.state
// Generate:   result["state"].clone()
```

Implementation approach:
1. In `GenCtx`, track which locals are "bus results" (assigned from bus invoke)
2. In `Expr::FieldAccess` handling (~line 636), check if base is a bus-result local
3. If so, emit `base["field_name"].clone()` instead of `base.field_name`

Alternatively (simpler but less precise): when accessing a field on an expression
whose type is unknown/unresolved, always emit JSON indexing.

---

## Issue 3: Opt<T> Not Always Lowered to Option<T>

### Symptom
```rust
error[E0599]: no method named `is_some` found for struct `std::string::String`
error[E0599]: no method named `ok_or` found for struct `domain::types::DeploymentState`
```

### Root Cause
Port method return types declared as `-> Opt<DeploymentState>` should generate
`Result<Option<DeploymentState>, DomainError>`. When the codegen generates the
calling code, it correctly calls `.await?` to unwrap the Result, but the inner
`Option` wrapping is sometimes lost — the local gets typed as the inner type
directly.

This happens specifically in the trait method signatures vs. the generated
application code. The trait has:
```rust
async fn get_current(...) -> Result<Option<DeploymentState>, DomainError>;
```
But the calling code treats the result as `DeploymentState` directly.

### Fix Direction
In the application code generation (look for where port method calls are generated,
around `translate_call` and the port-call path), ensure that:
1. When a port method's return type is `Opt<T>`, the generated call result is
   typed as `Option<T>` (after `?` unwraps the Result)
2. Track this in `GenCtx.local_types` so subsequent `.is_some()`, `.unwrap()`,
   `.is_none()` calls are valid

The port method return types ARE available in `ctx.method_returns` (populated from
the IR). The codegen just isn't consulting them when determining the local's type
after assignment.

---

## Issue 4: DDB/S3 Adapter Stub Lowering

### Symptom (69 errors in storage crate, similar in deploy)
```rust
error[E0061]: this function takes 1 argument but 2 arguments were supplied
   aws_sdk_dynamodb::Client::put_item("repos", metadata).execute(&self.client);
error[E0599]: no method named `fetch_one` found for struct `HeadObjectFluentBuilder`
error[E0599]: no method named `begins_with` found for struct `QueryFluentBuilder`
```

### VEIL Source (adapter patterns)
```veil
adapter DdbDeploymentStore for DeploymentStateStore
  @field(client: DdbClient)
  impl get_current(environment, unit_name)
    pk = f"DEPLOY#{environment}#{unit_name}"
    DdbClient.query(pk).key("CURRENT").fetch_optional!(client)
  impl save_current(state)
    pk = f"DEPLOY#{state.environment}#{state.unit_name}"
    DdbClient.put_item(pk, "CURRENT", state).execute!(client)
```

### Root Cause
The codegen translates `DdbClient.query(pk)` as a static method call:
`aws_sdk_dynamodb::Client::query(pk)` — but the real AWS SDK uses a builder pattern:
`client.query().table_name(TABLE).key_condition_expression(...)`.

The VEIL stub for `aws_sdk_dynamodb` doesn't provide enough information for the
codegen to know how to map `.query(pk).key("CURRENT").fetch_optional!()` to the
actual SDK builder chain.

### Fix Direction
This requires **stub-aware method chain translation**. Options:

**Option A (stub method mapping):** Extend `.stub` files to include method chain
mappings that tell the codegen how to translate each method:
```
DdbClient.query(pk) → client.query().table_name(&self.table).key_condition_expression("PK = :pk").expression_attribute_values(":pk", AttributeValue::S(pk))
DdbClient.query(pk).key(sk) → ... .key_condition_expression("PK = :pk AND SK = :sk") ...
.fetch_optional!(client) → .send().await?.items.and_then(|i| i.into_iter().next())
.fetch_all!(client) → .send().await?.items.unwrap_or_default()
.execute!(client) → .send().await?
```

**Option B (runtime helper crate):** Generate calls to a helper crate that wraps
the SDK with the VEIL-shaped API:
```rust
// Generated:
veil_ddb::query(&self.client, &self.table, pk).key(sk).fetch_optional().await?
```
This crate (`veil_ddb`) would live at `runtime/ddb_helper/` and implement the
builder methods that VEIL expects.

Option B is simpler to implement and doesn't require codegen changes — just a new
Rust crate that provides the exact API the current codegen already emits.

---

## Issue 5: F-Strings in Nested Contexts (Match Arms, Format Args)

### Symptom
```rust
error: prefix `f` is unknown
   format!("...{}", if env.is_some() then f" in {env.unwrap()}" else "")
```

### VEIL Source
```veil
ret {count, summary: f"Found {count} deployments{if environment.is_some() then f' in {environment.unwrap()}' else ''}."}
```

### Root Cause
The f-string parser (`parse_fstring_parts`) handles top-level `{expr}` interpolation,
but when the expression INSIDE the interpolation is itself an inline ternary that
contains nested f-strings (`f'...'`), those inner f-strings aren't recursively
translated to `format!()`.

The `Expr::Ident(expr_text)` fallback path (line ~3940 of parser.rs) captures the
raw expression text including `f'...'` as-is, and the codegen emits it literally.

### Fix Direction
In `parse_fstring_parts`, when the expression text contains `f"..."` or `f'...'`
patterns, recursively process them:

1. Detect nested f-strings in the captured expression text
2. Recursively call `parse_fstring_parts` or transform to `format!()` inline
3. Or: parse the full expression text through the main expression parser (create
   a sub-parser with lexer for the expression text) instead of using the simplified
   dot-split / call-detect heuristics

The ideal long-term fix: replace the regex-style `parse_fstring_parts` with a proper
sub-parse that creates a mini-lexer for the expression text and calls `parse_expr()`
on it. This would handle ALL expression forms correctly, not just simple field access
and calls.

---

## Issue 6: Enum Variant Field Access

### Symptom
```rust
error[E0609]: no field `hash` on type `domain::types::MetaFunctionVersion`
```

### VEIL Source
```veil
ret function_id.version.hash
```
Where `version` is `MetaFunctionVersion` enum with variant `Pinned { hash: Str }`.

### Root Cause
Rust enums don't support direct field access. You need pattern matching:
```rust
if let MetaFunctionVersion::Pinned { hash, .. } = &function_id.version {
    return Ok(hash.clone());
} else {
    panic!("expected Pinned variant");
}
```

### Fix Direction
This requires type information at codegen time. The codegen would need to:
1. Know that `function_id.version` is of type `MetaFunctionVersion` (an enum)
2. Know which variant has a `hash` field
3. Generate an `if let` destructuring

Implementation:
- In `GenCtx`, track the types of locals (already partially done via `local_types`)
- In `Expr::FieldAccess` handling, when the base type is a known enum, check if
  any variant has the accessed field
- If so, generate `if let EnumType::Variant { field, .. } = &base { field.clone() }`

This is a significant lift. A simpler interim: if the field access is on an enum
type AND used in a return position, generate an `unreachable!()` fallback:
```rust
match &function_id.version {
    MetaFunctionVersion::Pinned { hash, .. } => hash.clone(),
    _ => unreachable!("expected Pinned variant"),
}
```

---

## Issue 7: Duplicate `aws-config` in Generated Workspace Cargo.toml

### Symptom
```
error: duplicate key
  --> runtime/generated/Cargo.toml:37:1
```

### Root Cause
The workspace `[workspace.dependencies]` section in `gen_workspace_toml()` (rust.rs)
emits `aws-config` twice — once from the `aws_config` import and once from the
`cfg` alias resolution.

### Fix Direction
In `rust.rs`, find `gen_workspace_toml()` or wherever `[workspace.dependencies]` is
built. Add deduplication: use a `BTreeSet` or `HashMap` keyed by crate name to
prevent duplicate entries. The Makefile currently patches this with
`sed -i '/^aws-config = "1\.8"$/d'` but the fix belongs in the codegen.

Search for `workspace.dependencies` or `gen_workspace` in rust.rs.

---

## Priority Order

1. **Issue 7** (duplicate aws-config) — trivial fix, blocks all builds
2. **Issue 4** (DDB/S3 stubs) — Option B (helper crate) unblocks 69+ errors
3. **Issue 3** (Opt<T> lowering) — medium effort, unblocks many type errors
4. **Issue 2** (Bus invoke field access) — medium effort, unblocks tool returns
5. **Issue 1** (nested struct in bus context) — medium effort
6. **Issue 5** (nested f-strings) — requires parser refactor
7. **Issue 6** (enum field access) — requires type inference infrastructure

## Files to Modify

- `crates/veil-codegen/src/expr.rs` — Issues 1, 2, 3, 6
- `crates/veil-codegen/src/rust.rs` — Issues 4, 7
- `crates/veil-parser/src/parser.rs` — Issue 5
- New crate `runtime/ddb_helper/` — Issue 4 (Option B)

## Test Commands

```bash
# Parser tests (should stay green)
cargo test -p veil-parser --lib

# Codegen tests
cargo test -p veil-codegen --lib

# Full integration
cargo build -p veil-cli --release
./target/release/veil gen runtime/src/runtime.veil -o runtime/generated -t rust
sed -i '/^aws-config = "1\.8"$/d' runtime/generated/Cargo.toml
cd runtime/generated && cargo build -p meta_execution -p deploy

# Pre-existing storage (same root causes)
cd runtime/generated && cargo build -p storage
```
