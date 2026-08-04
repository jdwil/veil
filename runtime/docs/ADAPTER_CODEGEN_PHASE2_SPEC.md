# VEIL Codegen Fixes Phase 2 — Type Coercion & Chain Semantics

## Overview

After the adapter expression fixes (Phase 1), the generated storage, meta_execution,
and deploy crates compile with 48 remaining errors. These fall into 5 categories,
all related to type coercion gaps and chain-method semantics in adapter bodies.

This spec covers the fixes needed to bring error count to zero.

## How to Reproduce

```bash
cd /home/jd/dev/jd/veil
cargo build -p veil-cli --release
./target/release/veil gen runtime/src/runtime.veil -o runtime/generated -t rust
cd runtime/generated
cargo build -p storage -p meta_execution -p deploy
```

## Error Summary

| Category | Count | Root Cause |
|----------|-------|-----------|
| `.unwrap()` on `String` | 14 | Redundant `.unwrap()` after `as_s()` already unwrapped the Result |
| `.unwrap()` on `&AttributeValue` | 9 | Redundant `.unwrap()` after `.get("key")` already unwrapped via `ok_or_else` |
| `&_` expected, `String` found | 14 | `.get("key".to_string())` passes owned String; HashMap::get expects `&str` |
| `ByteStream` expected, `Vec<u8>` found | 5 | S3 `.body(data)` needs `.into()` for ByteStream conversion |
| `i32` expected, `i64` found | 4 | DDB `.limit(n)` takes i32, VEIL Int is i64 |
| VEIL source bugs | 2 | `edge.repo_id` (field doesn't exist), `f"{record.target}"` (enum not Display) |

---

## Issue 1: Redundant `.unwrap()` After `as_s()` Chain

### Symptom
```rust
error[E0599]: no method named `unwrap` found for struct `std::string::String`
```

### Generated Code
```rust
i.get("data".to_string())
    .unwrap()           // ← unwraps Option<&AV> (correct)
    .as_s()
    .map(|s| s.to_string())
    .unwrap()           // ← unwraps Result from as_s() → gives String
    .unwrap()           // ← ERROR: String has no .unwrap()
```

### VEIL Source
```veil
i.get("data").unwrap().as_s().unwrap()
```

### Root Cause

The codegen translates `as_s()` into a 3-part chain:
`.as_s().map(|s| s.to_string()).map_err(|e| ...)?` (or `.unwrap()` in closures).

This already "unwraps" the Result. The VEIL `.unwrap()` that FOLLOWS `as_s()` is
then applied again — producing a redundant `.unwrap()` on the already-extracted String.

The same issue applies outside closures where the chain is:
`.as_s().map(|s| s.to_string()).map_err(|e| DomainError::External(...))?`
followed by `.unwrap()` → error on String.

### Fix Direction

In `expr_to_rust`, the receiver-chain handling for `.as_s()` (and `.as_n()`)
already produces a fully-unwrapped value. When the NEXT chained method is
`.unwrap()` and the receiver expression ENDS with `.as_s()...` handling, the
`.unwrap()` should be elided (it's a no-op — the value is already extracted).

**Implementation:** In the receiver-based call handling in `translate_call` (the
`if let Some(recv) = &call.receiver` branch), when `call.method` is `unwrap` and
the receiver expression (`recv_str`) already contains the `as_s()` unwrapping
pattern (ends with `.map(|s| s.to_string()).map_err(...)? ` or `.unwrap()`), skip
appending another `.unwrap()`.

A simpler approach: detect that the receiver is itself a Call with method `as_s`
(or `as_n`). If so, the `.unwrap()` is redundant — just return `recv_str` as-is.

```rust
// In the receiver-chain handling, before the general suffix logic:
if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
    if let Some(inner_call) = call.receiver.as_ref().and_then(|r| match r.as_ref() {
        Expr::Call(c) => Some(c),
        _ => None,
    }) {
        let bare = inner_call.method.trim_end_matches(['!', '?']);
        if bare == "as_s" || bare == "as_n" || bare.starts_with("as_") {
            // as_s() codegen already unwraps — skip redundant .unwrap()
            return recv_str;
        }
    }
}
```

### Also applies to `.ok_or_else()?.unwrap()` pattern

The `.get("key")` on a HashMap-local is emitted as:
```rust
.get("data").ok_or_else(|| DomainError::External("missing data".into()))?
```

This already extracts the value from the Option. A subsequent `.unwrap()` from
the VEIL source is redundant and produces an error on `&AttributeValue`.

**Same fix location**: when `call.method == "unwrap"` and the receiver ends with
`ok_or_else(...)? `, skip the `.unwrap()`.

Alternatively, detect: if `recv_str` ends with `)?` or `.unwrap()`, a following
`.unwrap()` is always redundant.

---

## Issue 2: `.get("key".to_string())` — String Where `&str` Expected

### Symptom
```rust
error[E0308]: mismatched types — expected `&_`, found `String`
```

### Generated Code
```rust
i.get("data".to_string())
```

### Expected
```rust
i.get("data")
```

### Root Cause

When `.get("string_lit")` is called on a receiver inside a closure (where the
receiver is a closure param, not in `ctx.locals`), the codegen falls through to
the generic receiver-chain handling:

```rust
return format!(
    "{}.{}({}){}", recv_str, m,
    clone_args_for_method(&call.method, &call.args, ctx), suffix
);
```

The `clone_args_for_method` function, or the default `args_str` at the top of
`translate_call`, converts `StringLit("data")` to `"data".to_string()`. But
`HashMap::get()` takes `&Q where K: Borrow<Q>` — passing `"data"` (a `&str`)
works; passing `"data".to_string()` (an owned String) does not.

### Fix Direction

In the receiver-chain branch of `translate_call`, when `call.method == "get"` and
the argument is a `StringLit`, emit the argument as a bare string literal (`"data"`)
without `.to_string()`. The existing special-case for `.get("key")` on locals
already does this — it needs to also apply in the generic receiver path.

**Implementation:** The receiver-chain `.get("lit")` handling already exists at
~line 1780:
```rust
if call.method == "get" && call.args.len() == 1 {
    if let Expr::StringLit(key) = &call.args[0] {
        return format!(
            "{}.get(\"{}\").ok_or_else(|| ...)?", recv_str, key, key
        );
    }
}
```

This only fires for `StringLit`. When called inside a closure, this DOES fire
and returns `.ok_or_else(...)? `. But looking at the generated code:
`i.get("data".to_string())` — this means the special case is NOT firing.

Check: is the arg parsed as `StringLit("data")` or as something else when inside
a closure arg position? If the string literal is being parsed differently in
nested contexts, fix the parser. If `clone_args_for_method` is transforming it
before the check runs, reorder the checks.

Actually — the issue may be that `clone_args_for_method` is used in the generic
`format!("{}.{}({}){}", ...)` branch, and `StringLit` gets converted to
`"data".to_string()` by `expr_to_rust`. The `.get("key")` special case fires
BEFORE that generic branch, so if it's NOT firing, the issue is that the
`call.method` is not exactly `"get"` (maybe it has a bang suffix or the closure
fixup changed it).

**Verify**: print/log what `call.method` is when the arg is a StringLit in the
closure context. The fix may be as simple as also matching `"get!"`.

---

## Issue 3: `Vec<u8>` Where `ByteStream` Expected

### Symptom
```rust
error[E0308]: mismatched types — expected `ByteStream`, found `Vec<u8>`
```

### VEIL Source
```veil
client.put_object().bucket(bucket).key(key).body(data).send!()
```

### Root Cause

The S3 `.body()` method takes `ByteStream`, not `Vec<u8>`. When the VEIL source
passes a `Bytes` (`Vec<u8>` in Rust) argument, the codegen should emit `.into()`
to convert.

### Fix Direction

In the receiver-chain builder method handling, when the method is `body` and the
argument resolves to a `Vec<u8>` type (check `ctx.local_type` on the arg ident),
append `.into()` to the argument expression.

**Implementation:** In the generic receiver-chain format at the end:
```rust
format!("{}.{}({}){}", recv_str, m, args, suffix)
```

Add a special case: if `method == "body"` and the arg is an Ident whose type is
`Vec<u8>` or `Bytes`:
```rust
if bare_m == "body" && call.args.len() == 1 {
    if let Expr::Ident(name) = &call.args[0] {
        let ty = ctx.local_type(name).unwrap_or("");
        if ty == "Vec<u8>" || ty.contains("Bytes") || ty.contains("Vec<u8>") {
            return format!("{}.body({}.into()){}", recv_str, name, suffix);
        }
    }
}
```

Alternatively, check the stub's method parameter type: if the stub declares
`.body(input: ByteStream)` and the arg is typed `Vec<u8>`, auto-coerce with
`.into()`. This is more general but requires cross-referencing stub parameter
types at call sites.

**Simpler approach:** The S3 stub declares `fn body(input: ByteStream) -> Self`.
When the codegen sees a builder method whose stub param type differs from the
arg's inferred type, and the target type implements `From<source_type>`, emit
`.into()`. For now, hardcode `ByteStream` as a known conversion target from
`Vec<u8>`.

---

## Issue 4: `i64` Where `i32` Expected (`.limit()`)

### Symptom
```rust
error[E0308]: mismatched types — expected `i32`, found `i64`
```

### VEIL Source
```veil
...query()...limit(limit).send!()
```

### Root Cause

VEIL's `Int` type maps to `i64` in Rust. The DynamoDB `.limit()` method takes
`i32`. The codegen doesn't insert narrowing conversions.

### Fix Direction

When a builder method's stub-declared parameter type is `Int` but the real Rust
SDK expects `i32`, emit `arg as i32`. The DDB stub declares:
```
fn limit(input: Int) -> Self
```

The stub type `Int` maps to `i64` in VEIL, but the real SDK uses `i32`. The fix:

**Option A (targeted):** In the receiver-chain builder args, when the method is
`limit` on a known DDB builder type, cast the arg: `(arg) as i32`.

**Option B (general):** Add a `narrowing_methods` set (or annotation in the stub)
for methods where Int args need `as i32` coercion. The stub could declare:
```
fn limit(input: I32) -> Self
```
And the codegen maps `I32` → `i32` and inserts `as i32` when the arg is `i64`.

**Option C (simplest):** In the stub, change `Int` to a new type `I32` for these
specific parameters. Map `I32` → `i32` in Rust codegen. When an i64 local is
passed to an i32 param, emit `(val) as i32`.

For now, Option A is sufficient — just add `as i32` for `.limit()` args on DDB
builders. There are only 4 occurrences.

---

## Issue 5: VEIL Source Bugs

### 5a. `edge.repo_id` — Field Does Not Exist

**File:** `runtime/src/runtime.veil`, line 430

```veil
client.put_item()...item("PK", AttributeValue.S(f"DEP#{edge.repo_id.value}"))...
```

**Fix:** `DependencyEdge` has field `dependent: RepoId`, not `repo_id`. Change to:
```veil
client.put_item()...item("PK", AttributeValue.S(f"DEP#{edge.dependent.value}"))...
```

### 5b. `f"TARGET#{record.target}"` — Enum Not Display

**File:** `runtime/src/runtime.veil`, line 405

```veil
client.put_item()...item("SK", AttributeValue.S(f"TARGET#{record.target}"))...
```

`record.target` is a `DeployTarget` enum which doesn't implement `Display`.

**Fix options:**
1. Serialize with Json: `AttributeValue.S(f"TARGET#{Json.stringify(record.target)}")`
2. Add `@derive(Display)` to the `DeployTarget` enum definition
3. Use a match to convert to string

Recommended: Option 2 — add a Display derive (or `@derive(ToString)` annotation)
to the `DeployTarget` enum. The codegen should emit `#[derive(strum::Display)]`
or a manual `impl Display` for enums with the annotation.

Alternatively, the simplest VEIL source fix:
```veil
target_str = match record.target
  DeployTarget.Lambda => "lambda"
  DeployTarget.Ecs => "ecs"
  DeployTarget.S3 => "s3"
  else => "unknown"
client.put_item()...item("SK", AttributeValue.S(f"TARGET#{target_str}"))...
```

---

## Priority Order

1. **Issue 1** (redundant unwrap) — 23 errors, most impactful
2. **Issue 2** (String vs &str in get) — 14 errors
3. **Issue 3** (ByteStream) — 5 errors
4. **Issue 4** (i32 vs i64) — 4 errors
5. **Issue 5** (source bugs) — 2 errors

## Files to Modify

- `crates/veil-codegen/src/expr.rs` — Issues 1, 2, 3, 4
- `runtime/src/runtime.veil` — Issue 5

## Test Commands

```bash
# Parser tests (should stay green)
cargo test -p veil-parser --lib

# Codegen tests
cargo test -p veil-codegen --lib

# Integration — target is zero errors:
cd runtime/generated && cargo build -p storage -p meta_execution -p deploy
```

## Context

These errors persist after the Phase 1 adapter expression fixes which resolved
closures, enum variant constructors, .send() vs .send!(), and builder chain suffix
collisions. The remaining issues are all type-level: the codegen correctly
structures the expression tree but doesn't handle Rust's borrowing/sizing rules
for specific SDK method signatures.

The `.unwrap()` redundancy (Issue 1) is the most impactful — it accounts for
23 of the 48 errors. The pattern is always the same: VEIL source says
`.unwrap()` after a method that the codegen already unwraps (via `.map_err()?`
or `.ok_or_else()?`). The fix should detect and elide these redundant unwraps
rather than modifying the VEIL source (which correctly expresses intent to unwrap).
