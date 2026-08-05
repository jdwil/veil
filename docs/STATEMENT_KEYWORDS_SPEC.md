# Layer-Declared Statement Keywords — Implementation Spec

## Status: READY FOR IMPLEMENTATION

## Overview

VEIL already has partial support for layer-declared statement keywords. This spec describes how to **extend** the existing mechanism to support custom lowering templates, additional statement shapes, and return-value binding.

### What Already Exists

The infrastructure is partially in place:

- **IR:** `Expr::Action(ActionExpr)` variant in `crates/veil-ir/src/ast.rs:686`
- **Layer spec:** `StatementSpec` struct in `crates/veil-ir/src/layer.rs:282` with `keyword`, `maps_to`, `shape`, `port_target`, `port_method`
- **Parser:** Statement keyword detection and parsing at `crates/veil-parser/src/parser.rs:3223` via `parse_layer_statement()`
- **StmtShape enum:** `Call` and `If` shapes in `crates/veil-ir/src/layer.rs:67`
- **DDD layer example:** `layers/ddd.layer:295` declares `dispatch`, `invoke`, `emit`, `request`, `guard` keywords
- **Codegen:** `Expr::Action` handling in `crates/veil-codegen/src/rust.rs:4766` (monomorphize) and expression translation

### Layer Declaration Syntax (Current)

```
statement dispatch
  mt Bus.dispatch           # maps_to: Port.method pattern
  desc "Fire event through event bus"
  sem event_bus_fire_and_forget
  visual
    icon "📡"
    color "#f59e0b"
    label "Dispatch"

statement guard
  mt if                     # maps_to: core shape name
  desc "Precondition check"
  sem precondition_fail_step
  visual
    icon "🛡️"
    color "#ef4444"
    label "Guard"
```

## What Needs to Be Added

### 1. Custom Lowering Templates (per-target)

Currently, `maps_to` is either a `Port.method` or a shape name (`call`/`if`). The codegen hardcodes how to lower these. We need **explicit lowering templates per target** so layer authors control the output.

**Layer syntax extension:**

```
statement dispatch
  mt call
  desc "Fire event through event bus"
  sem event_bus_fire_and_forget
  requires_dep EventBus
  lowers_to
    rust: "self.{dep}.dispatch({args}).await.map_err(|e| DomainError::External(e.to_string()))?"
    typescript: "await this.{dep}.dispatch({args})"
  visual
    icon "📡"
    color "#f59e0b"
    label "Dispatch"
```

**Template variables:**
- `{args}` — comma-separated positional args, codegen'd to target syntax
- `{arg0}`, `{arg1}`, ... — specific positional args
- `{dep}` — the dep field name that satisfies `requires_dep`
- `{self}` — the adapter/handler self reference
- `{named.key}` — named argument by key

**Implementation:**

1. Add `lowers_to: HashMap<String, String>` to `StatementSpec` in `crates/veil-ir/src/layer.rs`
2. Add `requires_dep: Option<String>` to `StatementSpec`
3. Extend the layer parser (`crates/veil-ir/src/layer.rs` or wherever layers are parsed) to parse the `lowers_to` and `requires_dep` sub-blocks
4. In codegen (`crates/veil-codegen/src/rust.rs`), when emitting `Expr::Action`:
   - Look up the `StatementSpec` from the registry
   - If `lowers_to` has an entry for the current target, use template interpolation
   - Otherwise fall back to the current `Port.method` call pattern

### 2. Additional Statement Shapes

Add new `StmtShape` variants for common patterns:

```rust
pub enum StmtShape {
    Call,          // existing: kw Target.method(args)
    If,           // existing: kw <condition>, "message"
    Assign,       // NEW: result = kw args (binds return value)
    Block,        // NEW: kw args do ... end (has a body)
    Infix,        // existing partial: expr |> expr
}
```

**Assign shape** allows:
```
result = invoke ProcessOrder{order_id: id}
response = request GetUser{user_id: uid}
```

This is critical — currently you can't bind the return value of a statement keyword to a variable. The parser needs to recognize `ident = keyword ...` as an assignment where the RHS is a layer statement.

**Implementation:**

1. Add `Assign` and `Block` to `StmtShape` in `crates/veil-ir/src/layer.rs:67`
2. In parser `parse_body_expr()` (or wherever assignments are parsed): when an assignment's RHS starts with a known layer statement keyword, parse it as a statement keyword with the assignment wrapping it
3. Add `result_binding: Option<String>` to `ActionExpr` in `crates/veil-ir/src/ast.rs:849`
4. In codegen: if `result_binding` is set, emit `let {binding} = {lowered_statement};`

### 3. Scope Validation (`requires_dep`)

When `requires_dep` is specified, the checker should verify that the enclosing construct (service/handler/adapter) has a `@dep` annotation matching the required port type.

**Implementation:**

1. In `crates/veil-ir/src/check.rs` or `validate.rs`: when visiting `Expr::Action`, look up the `StatementSpec` and check `requires_dep` against the enclosing construct's deps
2. Emit a diagnostic if the dep is missing: `error[missing_dep]: statement 'dispatch' requires @dep of type EventBus`

### 4. IDE Integration

Statement keywords should appear in the IDE palette and autocomplete:

1. In the presentation model: statement keywords from loaded layers should appear as autocomplete suggestions when typing in a body context
2. The `visual` block (icon/color/label) is already parsed — ensure it's surfaced in the IDE's palette API
3. Statement keywords should appear in the outline (currently they're just expressions — they should have semantic meaning in the IR graph)

## File Locations

| File | What to Change |
|------|---------------|
| `crates/veil-ir/src/layer.rs` | Add `lowers_to`, `requires_dep` to `StatementSpec`; add `Assign`/`Block` to `StmtShape` |
| `crates/veil-ir/src/ast.rs` | Add `result_binding: Option<String>` to `ActionExpr` |
| `crates/veil-parser/src/parser.rs:3223` | Extend `parse_layer_statement()` for Assign shape; handle `ident = keyword ...` |
| `crates/veil-codegen/src/rust.rs` | Template interpolation for `lowers_to`; handle `result_binding` |
| `crates/veil-codegen/src/typescript.rs` | Same for TS target |
| `crates/veil-ir/src/check.rs` or `validate.rs` | Add `requires_dep` validation |
| `layers/ddd.layer` | Add `lowers_to` blocks to existing statement declarations |
| Layer parser (wherever `statement` blocks are parsed from `.layer` files) | Parse new sub-blocks |

## Parsing Logic (Detailed)

### Current flow (`parse_layer_statement` at parser.rs:3223):

1. Parser encounters an Ident token at statement position
2. Checks if it's a registered layer statement keyword (via `LayerRegistry`)
3. If yes, looks at the `StmtShape`:
   - `Call` → parse `Target.method(args)` or just `(args)`
   - `If` → parse `<condition>, "message"`
4. Emits `Expr::Action(ActionExpr { ... })`

### New flow:

1. Same detection
2. Additional shapes:
   - `Assign` → detected when parser sees `ident = <keyword> ...` — the keyword is the RHS
   - `Block` → parse args, then expect indented body (like `if`/`for`)
3. For `Assign`: set `action.result_binding = Some(ident)`
4. For existing `Call` shape with `Port.method` maps_to: current behavior preserved as fallback when no `lowers_to` template exists

### Assignment detection:

In `parse_body_expr()` (wherever `Assign`/`MutAssign` is parsed):
- When parsing `ident = expr`, check if the first token of `expr` is a registered statement keyword
- If so, delegate to `parse_layer_statement()` and wrap the result with the binding

## Codegen Logic (Detailed)

### Template interpolation:

```rust
fn emit_action_with_template(
    action: &ActionExpr,
    spec: &StatementSpec,
    target: &str,
    ctx: &GenCtx,
) -> String {
    let template = spec.lowers_to.get(target)
        .expect("no lowering for target");
    
    let mut result = template.clone();
    
    // Replace {args} with comma-separated codegen'd args
    let args_str = action.args.iter()
        .map(|a| translate_expr(a, ctx))
        .collect::<Vec<_>>()
        .join(", ");
    result = result.replace("{args}", &args_str);
    
    // Replace {arg0}, {arg1}, etc.
    for (i, arg) in action.args.iter().enumerate() {
        result = result.replace(
            &format!("{{arg{i}}}"),
            &translate_expr(arg, ctx),
        );
    }
    
    // Replace {dep} with the resolved dep field name
    if let Some(dep_type) = &spec.requires_dep {
        let dep_field = ctx.find_dep_field(dep_type);
        result = result.replace("{dep}", &dep_field);
    }
    
    // Replace {named.key}
    for (key, val) in &action.named_args {
        result = result.replace(
            &format!("{{named.{key}}}"),
            &translate_expr(val, ctx),
        );
    }
    
    // Wrap with binding if present
    if let Some(binding) = &action.result_binding {
        format!("let {binding} = {result};")
    } else {
        format!("{result};")
    }
}
```

### Fallback (no template):

If `lowers_to` is empty, fall back to current behavior:
- `mt Bus.dispatch` → `self.bus.dispatch({args}).await?`
- `mt if` → `if !({condition}) { return Err(DomainError::ValidationFailed({message})); }`

## Testing Requirements

### Parser tests (`crates/veil-parser/src/parser_tests.rs`):

1. `test_parse_layer_statement_call` — existing behavior preserved
2. `test_parse_layer_statement_assign` — `result = dispatch Event{}`
3. `test_parse_layer_statement_in_adapter` — works in impl bodies
4. `test_parse_layer_statement_in_svc_step` — works in service steps
5. `test_parse_unknown_keyword_errors` — non-layer idents still error correctly

### Codegen tests:

1. `test_action_template_interpolation` — template with `{args}`, `{dep}` produces correct Rust
2. `test_action_fallback_no_template` — existing Port.method behavior when no lowers_to
3. `test_action_assign_binding` — `let x = ...` wrapping
4. `test_action_typescript_target` — TS lowering works

### Integration tests:

1. A `.veil` file using `dispatch`, `invoke`, `guard` with the ddd layer → compiles and runs
2. A custom layer declaring a new statement keyword → parses, generates, compiles

## Example: Complete Workflow Layer Statements

After this feature is implemented, the workflow layer can declare:

```
statement await_review
  mt call
  desc "Pause execution for human review"
  requires_dep HitlPort
  lowers_to
    rust: "self.{dep}.request_review({arg0}, {arg1}).await?"
  visual
    icon "👁️"
    color "#ec4899"
    label "Await Review"

statement call_agent
  mt call
  desc "Invoke an LLM agent and bind the response"
  requires_dep LlmPort
  lowers_to
    rust: "self.{dep}.invoke({arg0}, {arg1}, {arg2}, {arg3}, {arg4}).await?"
  visual
    icon "🤖"
    color "#8b5cf6"
    label "Call Agent"
```

Usage:
```
svc ProcessDocument
  @dep(llm: LlmPort)
  @dep(hitl: HitlPort)
  step analyze
    summary = call_agent "You are a document analyst", document.content, "anthropic.claude-sonnet-4", 4096, null
    feedback = await_review summary, 72
    ret {summary: summary, feedback: feedback}
```

## Acceptance Criteria

1. ✅ Existing `dispatch`/`invoke`/`guard` keywords continue to work exactly as before (backward compatible)
2. Layer authors can declare `lowers_to` blocks with per-target templates
3. `requires_dep` emits a clear error if the dep is missing from the enclosing scope
4. `result = keyword args` syntax works (Assign shape)
5. Template interpolation handles `{args}`, `{arg0}..{argN}`, `{dep}`, `{named.key}`
6. At least Rust and TypeScript targets are supported in `lowers_to`
7. All existing tests pass
8. New tests cover the added functionality
