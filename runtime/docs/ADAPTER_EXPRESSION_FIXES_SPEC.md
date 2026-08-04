# VEIL Codegen Fixes — Adapter Body Expression Support

## Overview

With `ddb_helper` eliminated, adapter bodies now call the real AWS SDK builder
API via instance method chains. The codegen correctly translates simple builder
chains (`client.query().table_name(table).send!()`) but fails on several Rust
idioms that appear in the adapter bodies.

This spec covers the remaining parse/codegen gaps that prevent the generated
storage, meta_execution, and deploy crates from compiling.

## How to Reproduce

```bash
cd /home/jd/dev/jd/veil
cargo build -p veil-cli --release
./target/release/veil gen runtime/src/runtime.veil -o runtime/generated -t rust
cd runtime/generated
cargo build -p storage -p meta_execution -p deploy
```

## Issue 1: Iterator/Closure Chains in Adapter Bodies

### Symptom
```rust
error[E0425]: cannot find function `i_get` in this scope
error[E0425]: cannot find function `o_key` in this scope
error[E0425]: cannot find function `k_to_string` in this scope
```

### VEIL Source
```veil
ret resp.items().unwrap_or_default().iter().map(|i| Json.parse(i.get("data").unwrap().as_s().unwrap())).collect()
ret resp.contents().iter().filter_map(|o| o.key().map(|k| k.to_string())).collect()
```

### Root Cause
The f-string expression parser (`parse_fstring_parts`) is NOT involved here —
these are regular expressions in adapter `impl` bodies. The issue is in
`parse_postfix` or `parse_binary_rhs`: when parsing a method chain that includes
a closure argument (`|i| expr`), the parser either:

1. Fails to parse the closure `|i| i.get("data").unwrap().as_s().unwrap()`
   correctly — splitting on dots inside the closure body
2. Treats the `|` as a binary OR operator instead of a closure delimiter

The codegen output shows fragments like `i_get` and `o_key` — suggesting the
dot-separated chain inside the closure is being concatenated with underscores
(the `to_snake` function joining parts).

### Fix Direction
In the expression parser, when inside a method argument position (after `(`),
the `|` token should be recognized as a closure start (not a binary OR).
The closure body should be parsed as a full expression (supporting method
chains, nested calls, etc.).

Check `parse_paren_args()` — closures like `|i| i.get("data")` need to be
parsed as `Expr::Closure { params: ["i"], body: [expr] }` where the body
expression is the full chain.

### Test Case
```veil
impl get_repo(id)
  resp = client.query().table_name(table).key_condition_expression("PK = :pk").expression_attribute_values(":pk", AttributeValue.S(f"REPO#{id.value}")).send!()
  items = resp.items().unwrap_or_default()
  if items.is_empty() then ret null
  ret Json.parse(items[0].get("data").unwrap().as_s().unwrap())
```

Expected Rust:
```rust
let resp = self.client.query()
    .table_name(&self.table)
    .key_condition_expression("PK = :pk")
    .expression_attribute_values(":pk", AttributeValue::S(format!("REPO#{}", id.value)))
    .send().await?;
let items = resp.items().unwrap_or_default();
if items.is_empty() { return Ok(None); }
Ok(serde_json::from_str(items[0].get("data").unwrap().as_s().unwrap())?)
```

---

## Issue 2: `.unwrap_or_default()` on Slice References

### Symptom
```rust
error[E0599]: no method named `unwrap_or_default` found for reference
  `&[HashMap<String, AttributeValue>]` in the current scope
```

### VEIL Source
```veil
items = resp.items().unwrap_or_default()
```

### Root Cause
`QueryOutput::items()` returns `&[HashMap<String, AttributeValue>]` (a slice
reference), NOT `Option<Vec<...>>`. The VEIL code assumes it returns an Option
that can be unwrapped. The actual SDK returns a slice directly (empty if no items).

### Fix Direction
This is a VEIL source fix — change the adapter bodies to use the correct API:

```veil
# Instead of:
items = resp.items().unwrap_or_default()
# Use:
items = resp.items()
```

Since `items()` already returns `&[...]` which can be empty-checked with
`.is_empty()`. The codegen just needs to emit it correctly.

Alternatively, check the stub: if `QueryOutput.items()` is declared as
`-> Opt<List<HashMap<Str, AttributeValue>>>`, the VEIL code is correct
and the issue is that the generated Rust signature doesn't match the stub
declaration. Verify the stub matches the real SDK API.

---

## Issue 3: `.send()` vs `.send!()` — Result vs Direct Value

### Symptom
```rust
error[E0599]: no method named `is_ok` found for struct `HeadObjectOutput`
```

### VEIL Source
```veil
impl exists(key)
  resp = client.head_object().bucket(bucket).key(key).send()
  ret resp.is_ok()
```

### Root Cause
`.send!()` (with bang) means the codegen adds `.await?` — unwrapping the Result
and giving you the Output directly. `.send()` (without bang) should give you the
raw `Result<Output, Error>` so you can call `.is_ok()`.

The codegen currently treats `.send()` (non-bang on a stub-declared fallible
method) by still appending `.await?`, which unwraps the Result. The non-bang
form should only append `.await` (no `?`), preserving the Result wrapper.

### Fix Direction
In `receiver_call_suffix()` (expr.rs), when the method is `send` (non-bang)
on a fluent builder that has `send() -> Res!<T>` in the stub:
- Bang: `.send!()` → `.send().await.map_err(|e| DomainError::External(e.to_string()))?`
- Non-bang: `.send()` → `.send().await` (preserves the Result for `.is_ok()`)

The distinction between bang and non-bang suffix is already used for port calls.
Apply the same logic to chained-receiver stub methods.

---

## Issue 4: Array Indexing Inside Method Chains

### Symptom
```rust
error: `items[0].get("data")` not parsed correctly
```

### VEIL Source
```veil
ret Json.parse(items[0].get("data").unwrap().as_s().unwrap())
```

### Root Cause
`items[0]` is an index expression followed by `.get("data")`. The parser needs
to handle `expr[index].method(args)` as a postfix chain: Index → FieldAccess/Call.

The `parse_postfix` loop already handles `[index]` via `TokenKind::LBracket`,
and continues the loop so subsequent `.method()` can attach. Verify this works
for the specific pattern where the indexed expression is then method-chained.

### Fix Direction
Check that `parse_postfix` correctly produces:
```
Call(
  method: "as_s",
  receiver: Call(
    method: "unwrap",
    receiver: Call(
      method: "get",
      args: ["data"],
      receiver: Index(Ident("items"), IntLit(0))
    )
  )
)
```

If the parser produces this correctly, the codegen should emit:
```rust
items[0].get("data").unwrap().as_s().unwrap()
```

---

## Issue 5: `AttributeValue.S(pk)` — Enum Variant Constructors

### Symptom
```rust
error[E0308]: mismatched types — expected `AttributeValue`, found `String`
```

### VEIL Source
```veil
client.put_item().table_name(table).item("PK", AttributeValue.S(pk)).send!()
```

### Root Cause
`AttributeValue.S(pk)` should emit `AttributeValue::S(pk)`. The codegen already
handles `Enum.Variant(args)` → `Enum::Variant(args)` in many cases. Verify this
path works when:

1. `AttributeValue` is recognized as an enum (from the stub)
2. `.S(pk)` is treated as a variant constructor, not a method call
3. The call appears inside a builder chain argument position

### Fix Direction
In `translate_call`, when `call.target` is a known enum (from `name_to_shape`)
and the method starts with uppercase, emit `Target::Method(args)`.

Check that `AttributeValue` is in `name_to_shape` as `Shape::Enum` — it should
be imported from the DynamoDB stub.

---

## Issue 6: Multi-line Builder Chains in Adapter Bodies

### Symptom
Builder chains like:
```veil
client.put_item().table_name(table).item("PK", AttributeValue.S(f"REPO#{id}")).item("SK", AttributeValue.S("META")).item("data", AttributeValue.S(Json.stringify(metadata))).send!()
```
May fail parsing if the line is too long or if the parser hits limits.

### Root Cause
VEIL's indentation-based syntax means long single-line expressions must stay on
one line. The parser's `parse_brace_args` exits on Newline. Method chains that
exceed comfortable line length can't be broken across lines.

### Fix Direction (future)
Allow continuation: if a line ends with `.` (method chain) or the NEXT line
starts with `.` at greater indentation, treat it as a continuation of the
previous expression. This is NOT required for the current fix — all adapter
bodies fit on single lines. Document as a known limitation.

---

## Issue 7: `Json.parse(expr)` and `Json.stringify(expr)` in Adapter Bodies

### VEIL Source
```veil
ret Json.parse(items[0].get("data").unwrap().as_s().unwrap())
client.put_item()...item("data", AttributeValue.S(Json.stringify(metadata))).send!()
```

### Expected Rust
```rust
serde_json::from_str(items[0].get("data").unwrap().as_s().unwrap())?
serde_json::to_string(&metadata)?
```

### Root Cause
`Json.parse(expr)` and `Json.stringify(expr)` ARE already handled as language
primitives in `translate_call`. Verify they work when:
1. The argument is a complex chained expression (Issue 4)
2. They appear nested inside another method's argument position

---

## Priority Order

1. **Issue 5** (AttributeValue.S) — blocks all DDB adapter methods
2. **Issue 3** (.send vs .send!) — blocks exists/head_object patterns
3. **Issue 2** (unwrap_or_default on slice) — VEIL source fix or stub fix
4. **Issue 1** (closures in chains) — blocks list/map operations
5. **Issue 4** (array indexing + chains) — blocks item extraction
6. **Issue 7** (Json.parse/stringify nesting) — verify working
7. **Issue 6** (multi-line chains) — future enhancement

## Files to Modify

- `crates/veil-codegen/src/expr.rs` — Issues 1, 3, 4, 5
- `crates/veil-parser/src/parser.rs` — Issues 1, 4
- `runtime/src/runtime.veil` — Issue 2 (source fix)
- `runtime/src/stubs/aws_sdk_dynamodb.stub` — Issue 2 (verify stub accuracy)

## Test Commands

```bash
# Parser tests (should stay green)
cargo test -p veil-parser --lib

# Codegen tests
cargo test -p veil-codegen --lib

# Integration — the target state is zero errors on these 3 crates:
cd runtime/generated && cargo build -p storage -p meta_execution -p deploy
```

## Context

The `runtime/ddb_helper/` crate has been eliminated. All adapter bodies now
call the real AWS SDK via instance methods on an injected client field:

```veil
adapter DdbMetadataStore for MetadataStore
  @field(client: DdbClient, table)
  impl get_repo(id)
    resp = client.query().table_name(table)
      .key_condition_expression("PK = :pk AND SK = :sk")
      .expression_attribute_values(":pk", AttributeValue.S(f"REPO#{id.value}"))
      .expression_attribute_values(":sk", AttributeValue.S("META"))
      .send!()
    items = resp.items()
    if items.is_empty() then ret null
    ret Json.parse(items[0].get("data").unwrap().as_s().unwrap())
```

The stub (`aws_sdk_dynamodb.stub`) already correctly declares the Client struct
with its builder methods. The codegen translates builder chains to Rust. The
gaps are in parsing/generating complex expressions within those chains.
