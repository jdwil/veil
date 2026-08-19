use veil_ir::ast::*;
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::*;

/// Translate a VEIL expression to a Rust expression string (no trailing semicolon).
pub fn expr_to_rust(expr: &Expr, ctx: &GenCtx) -> String {
    if ctx.option_value_wrap && !expr_handles_option_wrap(expr) {
        let mut inner_ctx = ctx.clone_for_inference();
        inner_ctx.option_value_wrap = false;
        let inner = expr_to_rust(expr, &inner_ctx);
        return wrap_as_option_value(expr, inner, ctx);
    }
    match expr {
        Expr::Ident(name) => {
            // VEIL null → Rust None
            if name == "null" {
                return "None".to_string();
            }
            // VEIL noop → Rust empty block (no-op)
            if name == "noop" {
                return "{}".to_string();
            }
            // Issue 5: Handle inline ternary with nested f-strings from parse_fstring_parts.
            // These arrive as raw text like: `if x.is_some() then f" in {x.unwrap()}" else ""`
            if name.contains(" then ") && (name.contains("f\"") || name.contains("f'")) {
                return translate_inline_ternary_fstring(name);
            }
            // Raw method call idents from fstring parsing: x.unwrap_or("literal")
            // String literal defaults in unwrap_or need .to_string() for Option<String>.
            if name.contains(".unwrap_or(\"") && name.ends_with("\")") {
                // Transform: x.unwrap_or("text") → x.unwrap_or("text".to_string())
                let converted = name.replace(".unwrap_or(\"", ".unwrap_or(\"")
                    .replacen("\")", "\".to_string())", 1);
                return converted;
            }
            if ctx.state_locals.contains(name.as_str()) {
                // Threaded step state: read from the shared JSON bag.
                format!("state[\"{}\"]", name)
            } else if ctx.in_method && !ctx.locals.contains(name.as_str()) {
                if let Some(rf) = resolve_self_field_name(ctx, name) {
                    if rf == "pool" {
                        "&self.pool".to_string()
                    } else {
                        format!("self.{rf}.clone()")
                    }
                } else if name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    if let Some(enum_ty) = ctx.enum_variants.get(name) {
                        format!("{enum_ty}::{name}")
                    } else {
                        name.clone()
                    }
                } else {
                    name.clone()
                }
            } else if !ctx.locals.contains(name)
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                if let Some(enum_ty) = ctx.enum_variants.get(name) {
                    format!("{enum_ty}::{name}")
                } else {
                    name.clone()
                }
            } else {
                name.clone()
            }
        }
        Expr::FieldAccess(base, field) => {
            // `opt.is_some` (no call) is the same predicate as `opt.is_some()`.
            if field == "is_some" || field == "is_none" {
                return format!("{}.{field}()", expr_to_rust(base, ctx));
            }
            // A field of a state-local: index into the threaded JSON state.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"]", name, field);
                }
                // Method body: `self.table` → clone so `&self` methods compile.
                // `self.pool` stays uncloned — sqlx `Executor` is for `&Pool`.
                if name == "self" && ctx.in_method {
                    let f = resolve_self_field_name(ctx, field).unwrap_or_else(|| to_snake(field));
                    if f == "pool" {
                        return "&self.pool".to_string();
                    }
                    if ctx.self_fields.contains(field.as_str())
                        || ctx.self_fields.contains(&f)
                        || ctx.self_field_types.contains_key(&f)
                    {
                        return format!("self.{}.clone()", f);
                    }
                    return format!("self.{}", f);
                }
                // Enum variant access: EnumName.Variant → EnumName::Variant
                // Keep PascalCase field names (S, Hash, PayPerRequest) as variant ids.
                let field_is_variant = field
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if matches!(ctx.name_to_shape.get(name.as_str()), Some(Shape::Enum)) {
                    let variant = if field_is_variant {
                        field.clone()
                    } else {
                        field.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
                            + &field[1..]
                    };
                    return format!("{}::{}", name, variant);
                }
                // Stub enums are registered as Struct shapes; PascalCase field access
                // still means a unit variant (ScalarAttributeType.S, Runtime.Nodejs20x).
                if field_is_variant {
                    if let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
                        // path_type is e.g. `types::ScalarAttributeType` (no crate prefix)
                        return format!("{}::{}::{}", crate_name, path_type, field);
                    }
                }
                // Lowercase variant on a stub-known type (e.g. BillingMode.pay_per_request
                // → aws_sdk_dynamodb::types::BillingMode::PayPerRequest).
                if !field_is_variant {
                    if let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
                        // Convert snake_case variant to PascalCase
                        let variant: String = field
                            .split('_')
                            .map(|seg| {
                                let mut chars = seg.chars();
                                match chars.next() {
                                    Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                                    None => String::new(),
                                }
                            })
                            .collect();
                        return format!("{}::{}::{}", crate_name, path_type, variant);
                    }
                }
            }
            // Envelope routing: a field of a routing-returned local is a JSON
            // index (`result["code"]`). Envelope results are serde_json::Value.
            // Issue 2: Also applies to bus invoke results outside envelope routing.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return format!("{}[\"{}\"]", name, field);
                }
            }
            // Nested field access on a JSON value at any depth: result["a"]["b"]["c"]
            // When base resolves to a chain rooted in a JSON local, chain indexing.
            if is_json_rooted_expr(base, ctx) {
                let base_str = expr_to_rust(base, ctx);
                return format!("{}[\"{}\"]", base_str, field);
            }
            let base_str = expr_to_rust(base, ctx);
            // Auto-unwrap Option<T> locals on field access: if a local has type
            // `Option<X>`, field access implies the value is expected to be present.
            // Emit `.clone().ok_or(DomainError::NotFound)?.field` so the Option is
            // unwrapped at point of use.  This handles the common pattern where a
            // port method returns `Opt<T>` and the VEIL code accesses fields directly.
            // When the enclosing function returns Option<T>, use `?` directly
            // (returns None early) instead of converting to Result.
            if let Expr::Ident(name) = base.as_ref() {
                if let Some(ty) = ctx.local_type(name) {
                    if ty.starts_with("Option<") {
                        let enclosing_returns_option = ctx.expected_return_rust.as_ref()
                            .map(|r| r.starts_with("Option<"))
                            .unwrap_or(false);
                        if enclosing_returns_option {
                            return format!(
                                "{}.clone()?.{}",
                                base_str,
                                to_snake(field)
                            );
                        }
                        return format!(
                            "{}.clone().ok_or({})?.{}",
                            base_str,
                            ctx.error_model.not_found_path(),
                            to_snake(field)
                        );
                    }
                }
            }
            // TODO: enum field access — when `base` is an enum instance (e.g.
            // `version.hash` where version is MetaFunctionVersion::Pinned { hash }),
            // Rust requires `if let Enum::Variant { field, .. } = base { field }`.
            // Without type info at codegen time we emit direct field access which
            // only works for structs. May need if-let destructuring when type info
            // is available.
            // VEIL field reads are reusable (not Rust moves). Clone non-Copy fields.
            // Json / Value is indexed in place (`stack["k"]`) — do not clone the bag.
            let rust = format!("{}.{}", base_str, to_snake(field));
            if rust.ends_with(".clone()")
                || field_access_is_copy(base, field, ctx)
                || infer_expr_type(
                    &Expr::FieldAccess(base.clone(), field.clone()),
                    ctx,
                )
                .as_deref()
                .is_some_and(is_json_type_name)
            {
                rust
            } else {
                format!("{rust}.clone()")
            }
        }
        Expr::Call(call) => translate_call(call, ctx),
        Expr::BinaryOp(op) => {
            let l = expr_to_rust(&op.left, ctx);
            let r = expr_to_rust(&op.right, ctx);
            // Special case: x != None → x.is_some(), x == None → x.is_none()
            if r == "None" {
                return match op.op {
                    veil_ir::ast::BinOp::NotEq => format!("{}.is_some()", l),
                    veil_ir::ast::BinOp::Eq => format!("{}.is_none()", l),
                    _ => format!("{} {} {}", l, binop_to_rust(&op.op), r),
                };
            }
            if l == "None" {
                return match op.op {
                    veil_ir::ast::BinOp::NotEq => format!("{}.is_some()", r),
                    veil_ir::ast::BinOp::Eq => format!("{}.is_none()", r),
                    _ => format!("{} {} {}", l, binop_to_rust(&op.op), r),
                };
            }
            // List append: `out + [x]` / `out + vec` → extend into owned Vec
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (r.starts_with("vec![") || l.starts_with("vec!["))
            {
                return format!(
                    "{{ let mut __v = {l}; __v.extend({r}); __v }}"
                );
            }
            // String concat: Rust `String` has no `+ &String` / `+ &str` mix that
            // typechecks for every operand shape. `format!` is the portable
            // lowering for VEIL `Str + Str` (and `"lit" + field`). Flatten a
            // chain (`a + b + c`) into one `format!` (SL-021).
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (expr_is_stringish(&op.left, &l, ctx) || expr_is_stringish(&op.right, &r, ctx))
                && !(expr_is_numeric(&op.left, ctx) && expr_is_numeric(&op.right, ctx))
            {
                let parts = flatten_str_add_chain(expr);
                if parts.len() >= 2 {
                    let rendered: Vec<String> = parts
                        .into_iter()
                        .map(|p| {
                            let s = expr_to_rust(p, ctx);
                            clone_if_named_value(p, s)
                        })
                        .collect();
                    let holes = vec!["{}"; rendered.len()].join("");
                    return format!("format!(\"{holes}\", {})", rendered.join(", "));
                }
                let l = clone_if_named_value(&op.left, l);
                let r = clone_if_named_value(&op.right, r);
                return format!("format!(\"{{}}{{}}\", {l}, {r})");
            }
            format!("{} {} {}", l, binop_to_rust(&op.op), r)
        }
        Expr::UnaryOp(op) => {
            let inner = expr_to_rust(&op.expr, ctx);
            format!("{}{}", unaryop_to_rust(&op.op), inner)
        }
        Expr::IfExpr(ie) => {
            let mut cond_ctx = ctx.clone_for_inference();
            cond_ctx.option_value_wrap = false;
            let cond = expr_to_rust(&ie.condition, &cond_ctx);
            // Auto-coerce serde_json::Value → bool for if conditions
            let cond = if let Expr::Ident(name) = ie.condition.as_ref() {
                if ctx.local_type(name) == Some("serde_json::Value") {
                    format!("{}.as_bool().unwrap_or(false)", name)
                } else { cond }
            } else { cond };
            // Single-expression if/else: emit as value expression (no semicolons)
            // Assign / `let _ = …` is a statement — do not drop the semicolon.
            let then_is_stmt = matches!(
                ie.then_body.first(),
                Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _))
            );
            let else_is_stmt = ie.else_body.as_ref().is_some_and(|b| {
                matches!(b.first(), Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _)))
            });
            if ie.then_body.len() == 1
                && ie.else_body.as_ref().map_or(false, |b| b.len() == 1)
                && !then_is_stmt
                && !else_is_stmt
            {
                let then_expr = expr_to_rust_value(&ie.then_body[0], ctx);
                let else_expr = expr_to_rust_value(&ie.else_body.as_ref().unwrap()[0], ctx);
                return format!("if {} {{ {} }} else {{ {} }}", cond, then_expr, else_expr);
            }
            if ctx.option_value_wrap {
                let then_body = emit_value_block(&ie.then_body, ctx, "    ");
                if let Some(else_body) = &ie.else_body {
                    let else_stmts = emit_value_block(else_body, ctx, "    ");
                    return format!(
                        "if {} {{\n{}\n}} else {{\n{}\n}}",
                        cond, then_body, else_stmts
                    );
                }
                return format!("if {} {{\n{}\n}} else {{\n    None\n}}", cond, then_body);
            }
            let then_body = emit_tracked_block(&ie.then_body, ctx, "    ");
            if let Some(else_body) = &ie.else_body {
                let else_stmts = emit_tracked_block(else_body, ctx, "    ");
                format!("if {} {{\n{}\n}} else {{\n{}\n}}", cond, then_body, else_stmts)
            } else {
                format!("if {} {{\n{}\n}}", cond, then_body)
            }
        }
        Expr::Assign(name, rhs, ty_ann) => {
            // List append sugar: `out = out + [x]` → `out.push(x)` when the
            // left is the same local and the right is a single-element list.
            if let Expr::BinaryOp(bin) = rhs.as_ref() {
                if matches!(bin.op, veil_ir::ast::BinOp::Add) {
                    if let (Expr::Ident(left), Expr::ArrayLit(items)) =
                        (bin.left.as_ref(), bin.right.as_ref())
                    {
                        if left == name && items.len() == 1 {
                            let item = expr_to_rust(&items[0], ctx);
                            // Auto-unwrap Option<T> items pushed into a list: if
                            // the item is a local with Option<T> type, unwrap it
                            // since the list expects T elements.
                            if let Expr::Ident(item_name) = &items[0] {
                                if let Some(ty) = ctx.local_type(item_name) {
                                    if ty.starts_with("Option<") {
                                        return format!(
                                            "{}.push({}.clone().ok_or({})?)",
                                            name, item, ctx.error_model.not_found_path()
                                        );
                                    }
                                }
                            }
                            return format!("{}.push({})", name, item);
                        }
                    }
                }
            }
            // List concat sugar: `x = x.concat([items])` → `x.extend(vec![items])`
            // when target == LHS name and arg is an array literal.
            if let Expr::Call(call) = rhs.as_ref() {
                let bare_m = call.method.trim_end_matches('!');
                if bare_m == "concat" && call.target == *name && !call.args.is_empty() {
                    if let Some(Expr::ArrayLit(items)) = call.args.first() {
                        let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                        if items.len() == 1 {
                            return format!("{}.push({})", name, item_strs[0]);
                        } else {
                            return format!("{}.extend(vec![{}])", name, item_strs.join(", "));
                        }
                    }
                }
            }
            let rhs_str = match rhs.as_ref() {
                Expr::StringLit(s) => rust_string_lit_owned(s),
                _ => expr_to_rust(rhs, ctx),
            };
            // Field assignment: `wt.name = x` stored as Assign("wt.name", …)
            // Emit path with snake_case fields; never introduce a `let` binding.
            if name.contains('.') {
                let parts: Vec<&str> = name.splitn(2, '.').collect();
                let base_name = parts[0];
                let field_path = parts[1];
                // Auto-unwrap Option<T> locals on field assignment: if the base
                // local is Option<T>, we need to unwrap it first. Use
                // `as_mut().ok_or(DomainError::NotFound)?.field = val` pattern.
                if let Some(ty) = ctx.local_type(base_name) {
                    if ty.starts_with("Option<") {
                        let field_snake = field_path
                            .split('.')
                            .map(|s| to_snake(s))
                            .collect::<Vec<_>>()
                            .join(".");
                        return format!(
                            "{}.as_mut().ok_or({})?.{} = {}",
                            base_name, ctx.error_model.not_found_path(), field_snake, rhs_str
                        );
                    }
                }
                let path = name
                    .split('.')
                    .enumerate()
                    .map(|(i, seg)| {
                        if i == 0 {
                            seg.to_string()
                        } else {
                            to_snake(seg)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                return format!("{} = {}", path, rhs_str);
            }
            if ctx.state_locals.contains(name.as_str()) {
                // Write the result into the threaded step state as JSON.
                format!("state[\"{}\"] = serde_json::json!({})", name, rhs_str)
            } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                format!("self.{} = {}", to_snake(name), rhs_str)
            } else if ctx.is_local(name) {
                // Already-declared local (e.g. a `mut` var) → reassignment, no `let`.
                format!("{} = {}", name, rhs_str)
            } else {
                let mut_kw = if ctx.mut_locals.contains(name.as_str()) {
                    "mut "
                } else {
                    ""
                };
                if let Some(ty) = ty_ann {
                    format!(
                        "let {}{}: {} = {}",
                        mut_kw,
                        name,
                        crate::rust::type_to_rust(ty),
                        rhs_str
                    )
                } else {
                    format!("let {}{} = {}", mut_kw, name, rhs_str)
                }
            }
        }
        Expr::MutAssign(name, rhs, ty_ann) => {
            // List concat sugar: `x = x.concat([items])` → `x.extend(vec![items])`
            if let Expr::Call(call) = rhs.as_ref() {
                let bare_m = call.method.trim_end_matches('!');
                if bare_m == "concat" && call.target == *name && !call.args.is_empty() {
                    if let Some(Expr::ArrayLit(items)) = call.args.first() {
                        let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                        if items.len() == 1 {
                            return format!("{}.push({})", name, item_strs[0]);
                        } else {
                            return format!("{}.extend(vec![{}])", name, item_strs.join(", "));
                        }
                    }
                }
            }
            let rhs_str = match rhs.as_ref() {
                Expr::StringLit(s) => rust_string_lit_owned(s),
                _ => expr_to_rust(rhs, ctx),
            };
            // Reassignment of an already-bound local (e.g. `mut req` inside while).
            if ctx.is_local(name) {
                return format!("{} = {}", name, rhs_str);
            }
            match ty_ann {
                Some(ty) => format!("let mut {}: {} = {}", name, crate::rust::type_to_rust(ty), rhs_str),
                None => format!("let mut {} = {}", name, rhs_str),
            }
        }
        Expr::StringLit(s) => rust_string_lit(s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => {
            // `ret Ok` / `ret Err e` construct the Result directly; anything
            // else is the success value and gets wrapped in `Ok(..)`.
            match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => "return Ok(())".to_string(),
                Expr::Ident(n) if n == "Err" => {
                    format!("return Err({}(\"error\".to_string()))", ctx.error_model.external_path())
                }
                // `ret Err e` parses as a call `Err(e)` or ident chain; handle a
                // call whose target is Err.
                Expr::Call(c) if c.target == "Err" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust_value(e, ctx)).collect::<Vec<_>>().join(", ");
                    let err_type = &ctx.error_model.type_name;
                    if a.is_empty() {
                        format!("return Err({}(\"error\".to_string()))", ctx.error_model.validation_path())
                    } else if a.starts_with(&format!("{err_type}::")) {
                        // Already a domain error variant
                        format!("return Err({})", a)
                    } else {
                        // Check if the argument is a simple identifier (likely a caught error variable)
                        let is_simple_ident = c.args.len() == 1 && matches!(&c.args[0], Expr::Ident(_));
                        if is_simple_ident {
                            // Bare variable from a match arm — likely already domain error
                            format!("return Err({})", a)
                        } else if matches!(c.args.first(), Some(Expr::StringLit(_))) {
                            // ret Err "msg" → External (adapter fail-closed, not validation)
                            format!("return Err({}({}))", ctx.error_model.external_path(), a)
                        } else {
                            // format! / computed messages (upstream HTTP, DB) → External → 502
                            // User-facing validation uses `guard`, not `ret Err`.
                            format!("return Err({}({}))", ctx.error_model.external_path(), a)
                        }
                    }
                }
                Expr::Call(c) if c.target == "Ok" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    format!("return Ok({})", if a.is_empty() { "()".to_string() } else { a })
                }
                _ => {
                    let val = expr_to_rust_value(inner, ctx);
                    // Check if the function returns Result<...> — if so, wrap in Ok().
                    let returns_result = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(|t| t.starts_with("Result<"))
                        .unwrap_or(true); // default to Result wrapping
                    let returns_option = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(|t| t.contains("Option<"))
                        .unwrap_or(false);
                    if !returns_result {
                        // Direct return (not Result-wrapped)
                        if val == "None" {
                            if returns_option {
                                "return None".to_string()
                            } else {
                                // Non-Option API with null → treat as missing resource
                                "return /* null */ unreachable!(\"null return on non-Option\")"
                                    .to_string()
                            }
                        } else if returns_option && !val.starts_with("Some(") {
                            format!("return Some({})", val)
                        } else {
                            format!("return {}", val)
                        }
                    } else if val == "None" || val == "()" {
                        // `ret null` / `ret ()`: Option APIs → Ok(None); otherwise NotFound / unit Ok.
                        if returns_option {
                            "return Ok(None)".to_string()
                        } else if val == "()" {
                            "return Ok(())".to_string()
                        } else {
                            format!("return Err({})", ctx.error_model.not_found_path())
                        }
                    } else if returns_option && !val.starts_with("Some(") {
                        // If the value is already Option<T> (from a local typed as such),
                        // don't double-wrap in Some(). Just return Ok(val).
                        if let Expr::Ident(name) = inner.as_ref() {
                            if ctx.local_type(name).map(|t| t.starts_with("Option<")).unwrap_or(false) {
                                return format!("return Ok({})", val);
                            }
                        }
                        format!("return Ok(Some({}))", val)
                    } else {
                        format!("return Ok({})", val)
                    }
                }
            }
        }
        Expr::Await(inner) => {
            let inner_str = expr_to_rust(inner, ctx);
            format!("{}.await", inner_str)
        }
        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),
        Expr::Index(base, idx) => {
            let b = expr_to_rust(base, ctx);
            // HashMap / Dynamo item: `.get("key").cloned().ok_or(NotFound)?`
            // so subsequent `.as_s()` is on AttributeValue, not Option.
            match idx.as_ref() {
                Expr::StringLit(s) => format!(
                    "{b}.get(\"{s}\").cloned().ok_or({})?", ctx.error_model.not_found_path()
                ),
                Expr::IntLit(n) => list_index_get_rust(&b, &n.to_string(), base, ctx),
                // Dynamic key (e.g. params[p.name] on serde_json::Value)
                other => {
                    let i = expr_to_rust(other, ctx);
                    let base_ty = match base.as_ref() {
                        Expr::Ident(n) => ctx.local_type(n).unwrap_or(""),
                        _ => "",
                    };
                    // Integer / usize indices → owned element, never a borrow.
                    let idx_is_int = matches!(other, Expr::IntLit(_))
                        || matches!(
                            other,
                            Expr::Ident(n) if matches!(
                                ctx.local_type(n),
                                Some("i64")
                                    | Some("i32")
                                    | Some("u64")
                                    | Some("u32")
                                    | Some("usize")
                                    | Some("isize")
                            )
                        );
                    if idx_is_int {
                        list_index_get_rust(&b, &format!("({i})"), base, ctx)
                    } else if base_ty.contains("Value") || base_ty == "Json" || base_ty.is_empty()
                    {
                        // String-keyed JSON map access.
                        format!(
                            "{b}.get({i}.as_str()).cloned().unwrap_or(serde_json::Value::Null)"
                        )
                    } else {
                        format!("{b}[{i}]")
                    }
                }
            }
        }
        Expr::ArrayLit(items) => { let s = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", "); format!("vec![{}]", s) }
        Expr::Range { start, end, inclusive } => { let s = start.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let e = end.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let op = if *inclusive { "..=" } else { ".." }; format!("{}{}{}", s, op, e) }
        Expr::Loop(body) => { let b = body.iter().map(|e| format!("    {};", expr_to_rust(e, ctx))).collect::<Vec<_>>().join("\n"); format!("loop {{\n{}\n}}", b) }
        Expr::DoBlock(body) => {
            if body.is_empty() {
                "{}".to_string()
            } else {
                // Child scope for type tracking — locals don't leak out of the block
                let mut block_ctx = ctx.clone_for_inference();
                let mut lines = Vec::new();
                for (i, e) in body.iter().enumerate() {
                    let rust = expr_to_rust(e, &block_ctx);
                    // Track local types so subsequent lines resolve receiver types
                    if let Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) = e {
                        if !name.contains('.') {
                            block_ctx.locals.insert(name.clone());
                            if let Some(ty) = ty_ann {
                                block_ctx.local_types.insert(name.clone(), crate::rust::type_to_rust(ty));
                            } else if let Some(t) = infer_expr_type(rhs, &block_ctx) {
                                block_ctx.local_types.insert(name.clone(), t);
                            }
                        }
                    }
                    if i == body.len() - 1 {
                        // Last expression: no semicolon (block return value)
                        lines.push(format!("    {}", rust));
                    } else {
                        lines.push(format!("    {};", rust));
                    }
                }
                format!("{{\n{}\n}}", lines.join("\n"))
            }
        }
        Expr::Cast(expr, ty) => format!("{} as {}", expr_to_rust(expr, ctx), ty),
        Expr::Try(expr) => format!("{}?", expr_to_rust(expr, ctx)),
        Expr::Require(inner) => {
            let s = expr_to_rust(inner, ctx);
            // ACS-010: require force-presents one Opt layer *and* one Res layer.
            // Bang already emits try (`?` / `.await?`) for Res. If the success
            // type is still Option, we must unwrap that too — do not treat a
            // trailing `?` as "already fully present".
            let ty = infer_expr_type(inner, ctx);
            if expr_is_json(inner, ctx)
                || ty.as_deref().is_some_and(is_json_type_name)
            {
                // `require context.stack.topic_arn` → present JSON string.
                format!(
                    "{s}.as_str().map(|s| s.to_string()).ok_or({})?", ctx.error_model.not_found_path()
                )
            } else {
                let still_option = ty.as_deref().is_some_and(|t| peel_option_rust(t).is_some());
                if still_option {
                    format!("{s}.ok_or({})?", ctx.error_model.not_found_path())
                } else if s.trim_end().ends_with('?') {
                    s
                } else if ty.as_deref().is_some_and(|t| {
                    rust_ty_is_stringish(t) || t == "i64" || t == "bool" || t.starts_with("Vec<")
                }) {
                    // Already a present value (e.g. `a.args[0]` after cloned get).
                    s
                } else {
                    format!("{s}.ok_or({})?", ctx.error_model.not_found_path())
                }
            }
        },
        Expr::StructUpdate { name, fields, base } => { let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx))).collect::<Vec<_>>().join(", "); format!("{} {{ {}, ..{} }}", name, fs, expr_to_rust(base, ctx)) }
        Expr::IfLet { pattern, expr, then_body, else_body } => {
            let e = expr_to_rust(expr, ctx);
            let then_str = then_body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            let else_str = else_body.as_ref().map(|eb| { let s = eb.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n"); format!(" else {{\n{}\n}}", s) }).unwrap_or_default();
            format!("if let {} = {} {{\n{}\n}}{}", pattern, e, then_str, else_str)
        }
        Expr::WhileLet { pattern, expr, body } => {
            let e = expr_to_rust(expr, ctx);
            let body_str = body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            format!("while let {} = {} {{\n{}\n}}", pattern, e, body_str)
        }
        Expr::LetPattern(pattern, expr, ty_ann) => {
            let pat_str = pattern_to_rust(pattern);
            let e = expr_to_rust(expr, ctx);
            match ty_ann {
                Some(ty) => format!("let {}: {} = {}", pat_str, crate::rust::type_to_rust(ty), e),
                None => format!("let {} = {}", pat_str, e),
            }
        }
        Expr::Action(a) => translate_action(a, ctx),
        Expr::StructLit(name, fields) if name.is_empty() => {
            // Anonymous record/map literal (`{}` or `{ key: value, ... }`) → a
            // JSON object value.
            if fields.is_empty() {
                "serde_json::json!({})".to_string()
            } else {
                let pairs = fields.iter().map(|(k, v)| {
                    format!("\"{}\": {}", k, to_json_arg(v, ctx))
                }).collect::<Vec<_>>().join(", ");
                format!("serde_json::json!({{ {} }})", pairs)
            }
        }
        Expr::StructLit(name, fields) => {
            let fs = fields.iter().map(|(k, v)| {
                let v_str = expr_to_rust(v, ctx);
                // Clone ident and field access values to prevent move issues.
                // Skip copy/null/bools so we don't emit `None.clone()`.
                let cloned = match v {
                    Expr::StringLit(s) => rust_string_lit_owned(s),
                    _ => clone_for_reuse(v, v_str.clone(), ctx),
                };
                // Type-aware coercion: when a field value is serde_json::Value
                // but the target struct field expects a typed value, auto-convert.
                // Also handle the reverse: typed values going into Json/Option<Json> fields.
                let coerced = if let Some(field_ty) = ctx.field_type(name, k) {
                    let val_ty = match v {
                        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
                        _ => infer_expr_type(v, ctx),
                    };
                    if val_ty.as_deref() == Some("serde_json::Value") {
                        match field_ty {
                            "String" => format!(
                                "{}.as_str().map(|s| s.to_string()).unwrap_or_default()",
                                cloned.trim_end_matches(".clone()")
                            ),
                            "bool" => format!("{}.as_bool().unwrap_or(false)", cloned.trim_end_matches(".clone()")),
                            "i64" => format!("{}.as_i64().unwrap_or(0)", cloned.trim_end_matches(".clone()")),
                            "f64" => format!("{}.as_f64().unwrap_or(0.0)", cloned.trim_end_matches(".clone()")),
                            t if t.starts_with("Option<") => format!("Some({})", cloned),
                            _ => cloned,
                        }
                    } else if field_ty == "serde_json::Value" || field_ty == "Option<serde_json::Value>" {
                        // Non-JSON value going into a Json field → wrap with json!()
                        if field_ty.starts_with("Option") {
                            // null → None (not Some(json!(None)))
                            if cloned == "None" {
                                "None".to_string()
                            } else {
                                format!("Some(serde_json::json!({}))", cloned)
                            }
                        } else {
                            format!("serde_json::json!({})", cloned)
                        }
                    } else {
                        cloned
                    }
                } else {
                    cloned
                };
                // Field-init shorthand when the value is already the field name
                // (`status` not `status: status.clone()`). Never force `.clone()`
                // here — `clone_for_reuse` already decided Copy / last-use.
                let field = to_snake(k);
                if coerced == field || coerced == *k {
                    coerced
                } else {
                    format!("{field}: {coerced}")
                }
            }).collect::<Vec<_>>().join(", ");
            format!("{} {{ {} }}", name, fs)
        }
        Expr::Match(scrutinee, arms) => {
            // The match consumes the scrutinee's Result directly, so a fallible
            // call scrutinee must NOT auto-propagate with `?`.
            // Never Some-wrap the scrutinee — only arm values.
            let mut scrut_ctx = ctx.clone_for_inference();
            scrut_ctx.option_value_wrap = false;
            let raw = expr_to_rust(scrutinee, &scrut_ctx);
            // String-literal arms match `&str`. Keep the try-unwrap so we do
            // not call `.as_str()` on a `Result`. Result/enum arms strip `?`
            // so the match can consume Ok/Err or the domain value.
            let has_string_patterns = arms.iter().any(|a| a.pattern.starts_with('"'));
            let scrutinee_str = if has_string_patterns {
                raw.clone()
            } else {
                strip_try_suffix(raw)
            };
            // If the scrutinee is a serde_json::Value local but arms use typed
            // enum/struct patterns, deserialize first.
            let scrutinee_str = if let Expr::Ident(name) = scrutinee.as_ref() {
                if ctx.local_type(name) == Some("serde_json::Value") {
                    // Detect enum type from first arm's pattern (e.g. "ReconcileResult.InSync")
                    let first_pat = arms.first().map(|a| &a.pattern).cloned().unwrap_or_default();
                    let has_enum_pat = first_pat.contains("::")
                        || first_pat.contains('.')
                        || first_pat.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
                    if has_enum_pat && !first_pat.starts_with('"') && first_pat != "_" {
                        // Extract enum type: "ReconcileResult.InSync" → "ReconcileResult"
                        // or "ReconcileResult::InSync" → "ReconcileResult"
                        let enum_type = first_pat.split(|c| c == '.' || c == ':')
                            .next().unwrap_or(&first_pat)
                            .split('{').next().unwrap_or(&first_pat).trim();
                        format!("serde_json::from_value::<{}>({}.clone()).unwrap()", enum_type, name)
                    } else {
                        scrutinee_str
                    }
                } else {
                    scrutinee_str
                }
            } else {
                scrutinee_str
            };
            // String-literal arms need `&str`. Scrutinee is already unwrapped
            // (see above) so this is String / &str, never Result.
            let scrutinee_final = if has_string_patterns {
                let t = scrutinee_str.trim();
                if t.ends_with(".as_str()") || t.ends_with(".as_str().trim()") {
                    scrutinee_str
                } else {
                    format!("{scrutinee_str}.as_str()")
                }
            } else {
                scrutinee_str
            };
            // When the scrutinee is a local variable matched against enum
            // patterns with field destructuring, clone it so the variable is
            // not moved and can be reused in subsequent match expressions or
            // later statements. Pattern bindings stay owned (no ref issues).
            let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
            let scrutinee_is_local_ident = if let Expr::Ident(name) = scrutinee.as_ref() {
                ctx.is_local(name) && !has_string_patterns
            } else {
                false
            };
            let scrutinee_final = if scrutinee_is_local_ident && has_enum_patterns {
                format!("{}.clone()", scrutinee_final)
            } else {
                scrutinee_final
            };
            let mut out = format!("match {} {{\n", scrutinee_final);
            for arm in arms {
                // Use structured pattern if available, fall back to string normalization
                let pattern = if let Some(rich) = &arm.rich_pattern {
                    pattern_to_rust_qualified(rich, Some(&ctx.enum_variants))
                } else {
                    normalize_match_pattern(&arm.pattern, ctx)
                };
                let guard_str = match &arm.guard {
                    Some(g) => format!(" if {}", expr_to_rust(g, &scrut_ctx)),
                    None => String::new(),
                };
                // Match arm bodies get their own local set (bindings + assigns).
                let mut arm_ctx = ctx.clone_for_inference();
                // Bind pattern idents as locals (Some(item) → item)
                for name in pattern_binding_names(&arm.pattern) {
                    arm_ctx.locals.insert(name);
                }
                arm_ctx.mut_locals.extend(analyze_mut_locals(&arm.body));
                let body_str = if arm.body.len() == 1 {
                    expr_to_rust_value(&arm.body[0], &arm_ctx)
                } else {
                    format!(
                        "{{\n{}\n    }}",
                        emit_value_block(&arm.body, &arm_ctx, "        ")
                    )
                };
                out.push_str(&format!("        {}{} => {},\n", pattern, guard_str, body_str));
            }
            // Add wildcard arm for enum matches to ensure exhaustiveness
            let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
            let has_wildcard = arms.iter().any(|a| a.pattern == "_" || a.pattern == "else" || a.pattern.starts_with('_'));
            if has_enum_patterns && !has_wildcard {
                out.push_str("        _ => unreachable!()\n");
            }
            out.push_str("    }");
            out
        }
        Expr::ForLoop { binding, index, iterable, body } => {
            let mut iter_str = expr_to_rust(iterable, ctx);
            // Iterate Vec/List fields by shared ref — no clone of the collection.
            // Do NOT prefix `&` on Calls: `result.items()` already returns `&[T]`.
            // `&result.items()` is `&&[T]` and is not IntoIterator (SL-028).
            let elem_copy = element_type_of(iterable, ctx)
                .as_deref()
                .is_some_and(|t| rust_ty_is_copy(t) || rust_ty_is_unit_enum(t, ctx));
            let iterable_is_call = matches!(iterable.as_ref(), Expr::Call(_));
            if !elem_copy
                && !iterable_is_call
                && !iter_str.starts_with('&')
                && !iter_str.ends_with(".iter()")
                && !iter_str.ends_with(".into_iter()")
            {
                let base = iter_str
                    .strip_suffix(".clone()")
                    .unwrap_or(iter_str.as_str());
                iter_str = format!("&{base}");
            } else if matches!(iterable.as_ref(), Expr::FieldAccess(_, _))
                && !iter_str.ends_with(".clone()")
                && !iter_str.ends_with(".iter()")
            {
                iter_str = format!("{iter_str}.clone()");
            }
            let bind = if let Some(idx) = index {
                format!("({}, {})", idx, binding)
            } else {
                binding.clone()
            };
            // The loop variable is a local within the body. Infer its element
            // type from the iterable so method calls on it resolve (e.g. a
            // `List<SagaStep>` yields `SagaStep` elements).
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.locals.insert(binding.clone());
            if let Some(elem) = element_type_of(iterable, ctx) {
                body_ctx.local_types.insert(binding.clone(), elem);
            }
            // Shared-ref iteration: the binding is `&T`. Last-use must not move it.
            if !elem_copy && iter_str.starts_with('&') {
                body_ctx.ref_elem_locals.insert(binding.clone());
            }
            if let Some(idx) = index {
                body_ctx.locals.insert(idx.clone());
            }
            body_ctx.mut_locals.extend(analyze_mut_locals(body));
            let body_str = emit_tracked_block(body, &body_ctx, "    ");
            let enumerate = if index.is_some() { ".enumerate()" } else { "" };
            // If the iterable type is Option<_>, unwrap to empty default; else as-is.
            let iter_expr = if let Expr::Ident(name) = iterable.as_ref() {
                if ctx
                    .local_type(name)
                    .map(|t| t.starts_with("Option<"))
                    .unwrap_or(false)
                {
                    format!("{iter_str}.unwrap_or_default()")
                } else {
                    iter_str
                }
            } else {
                iter_str
            };
            format!("for {bind} in {iter_expr}{enumerate} {{\n{body_str}\n}}")
        }
        Expr::WhileLoop { condition, body } => {
            let cond_str = expr_to_rust(condition, ctx);
            // Track locals across the loop body so `mut req = …` then `req = …`
            // reassigns (adapters / retries) instead of shadowing or free fns.
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.mut_locals.extend(analyze_mut_locals(body));
            let mut lines = Vec::new();
            for e in body {
                let line = expr_to_rust(e, &body_ctx);
                if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = e {
                    if !name.contains('.') {
                        body_ctx.locals.insert(name.clone());
                    }
                }
                lines.push(format!("        {};", line));
            }
            format!("while {} {{\n{}\n    }}", cond_str, lines.join("\n"))
        }
        Expr::Tuple(items) => {
            let parts = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        Expr::StringInterp(parts) => {
            use veil_ir::ast::StringPart;
            let mut fmt = String::new();
            let mut args = Vec::new();
            for p in parts {
                match p {
                    // Escape `{`/`}` so literal braces survive `format!` (e.g. path `{id}`).
                    StringPart::Literal(l) => {
                        for ch in l.chars() {
                            match ch {
                                '{' => fmt.push_str("{{"),
                                '}' => fmt.push_str("}}"),
                                _ => fmt.push(ch),
                            }
                        }
                    }
                    StringPart::Expr(e) => {
                        fmt.push_str("{}");
                        args.push(expr_to_rust(e, ctx));
                    }
                }
            }
            if args.is_empty() {
                // Still a format-free string; unescape was only for format! — rebuild raw.
                let raw: String = parts
                    .iter()
                    .filter_map(|p| match p {
                        StringPart::Literal(l) => Some(l.as_str()),
                        _ => None,
                    })
                    .collect();
                format!("\"{}\".to_string()", raw.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                format!("format!(\"{}\", {})", fmt, args.join(", "))
            }
        }
        Expr::Closure { .. } => {
            // Delegate to the structured IR path which applies
            // suppress_try_in_closure for ? → .unwrap() conversion.
            use super::rust_ir::{emit, lower_to_rust};
            emit(&lower_to_rust(expr, ctx))
        }
        // Expanded by adapt merge before codegen — should never remain.
        Expr::Stock => {
            "/* error: stock not expanded */ ()".to_string()
        }
    }
}

/// Render an expression for embedding inside a `json!` payload. Values are
/// Issue 5: Translate inline VEIL ternary expressions with nested f-strings.
/// Input: `if x.is_some() then f" in {x.unwrap()}" else ""`
/// Output: `if x.is_some() { format!(" in {}", x.unwrap()) } else { "".to_string() }`
pub fn translate_inline_ternary_fstring(raw: &str) -> String {
    // Parse: `if <cond> then <then_expr> else <else_expr>`
    let Some(then_idx) = raw.find(" then ") else {
        return raw.to_string();
    };
    let cond = &raw[3..then_idx]; // skip "if "
    let after_then = &raw[then_idx + 6..]; // skip " then "

    // Find the `else` boundary — must handle nested quotes
    let (then_expr, else_expr) = if let Some(else_idx) = find_top_level_else(after_then) {
        (&after_then[..else_idx], after_then[else_idx + 5..].trim()) // skip " else "
    } else {
        (after_then, "\"\"")
    };

    let then_rust = translate_fstring_value(then_expr.trim());
    let else_rust = translate_fstring_value(else_expr.trim());

    format!("if {} {{ {} }} else {{ {} }}", cond, then_rust, else_rust)
}

/// Find top-level " else " that's not inside quotes.
pub fn find_top_level_else(s: &str) -> Option<usize> {
    let mut in_quote = false;
    let mut quote_char = '"';
    let bytes = s.as_bytes();
    let else_pat = b" else ";
    for i in 0..s.len().saturating_sub(5) {
        let ch = bytes[i] as char;
        if !in_quote && (ch == '"' || ch == '\'') {
            in_quote = true;
            quote_char = ch;
        } else if in_quote && ch == quote_char && (i == 0 || bytes[i - 1] != b'\\') {
            in_quote = false;
        } else if !in_quote && i + 6 <= s.len() && &bytes[i..i + 6] == else_pat {
            return Some(i);
        }
    }
    None
}

/// Translate a value that may be an f-string or a plain string literal.
pub fn translate_fstring_value(val: &str) -> String {
    // f"..." or f'...' → format!(...)
    if (val.starts_with("f\"") && val.ends_with('"')) ||
       (val.starts_with("f'") && val.ends_with('\'')) {
        let inner = &val[2..val.len() - 1];
        // Convert {expr} interpolations to format! args
        let mut fmt = String::new();
        let mut args = Vec::new();
        let mut chars = inner.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut depth = 1;
                let mut expr_text = String::new();
                while let Some(c) = chars.next() {
                    if c == '{' { depth += 1; }
                    if c == '}' { depth -= 1; if depth == 0 { break; } }
                    expr_text.push(c);
                }
                fmt.push_str("{}");
                args.push(expr_text);
            } else {
                fmt.push(ch);
            }
        }
        if args.is_empty() {
            format!("\"{}\".to_string()", fmt)
        } else {
            format!("format!(\"{}\", {})", fmt, args.join(", "))
        }
    } else if val.starts_with('"') && val.ends_with('"') {
        // Plain string literal
        format!("{}.to_string()", val)
    } else if val.starts_with('\'') && val.ends_with('\'') {
        let inner = &val[1..val.len() - 1];
        format!("\"{}\".to_string()", inner)
    } else {
        val.to_string()
    }
}

/// Serialize field-access and method-call expressions to JSON-safe values for use as
/// cloned to avoid moving locals that are reused across bus calls; bare
/// non-local identifiers (e.g. enum variants like `FreeTier`) become JSON
/// strings; field access uses JSON indexing on the serialized base so it works
/// regardless of the (opaque) source type.
pub fn to_json_arg(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            // VEIL null in JSON envelopes must be JSON null, not the string "null".
            if name == "null" {
                return "serde_json::Value::Null".to_string();
            }
            // A shared step-state value → read from the threaded state.
            if ctx.state_locals.contains(name.as_str()) {
                format!("state[\"{}\"].clone()", name)
            } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                // A struct-captured input (step impl) → self.<field>.
                format!("self.{}.clone()", to_snake(name))
            } else if ctx.is_local(name) {
                format!("{}.clone()", name)
            } else {
                // Non-local bare ident in a payload → symbolic string (enum variant, marker).
                format!("\"{}\"", name)
            }
        }
        Expr::FieldAccess(base, field) => {
            // A field of a state-local → index into the threaded state.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"].clone()", name, field);
                }
            }
            // If the base is already a serde_json::Value local, index it directly.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return format!("{}[\"{}\"].clone()", name, field);
                }
            }
            // Otherwise serialize the base then index (works for opaque stub types;
            // Index yields Null on mismatch rather than panicking).
            format!("serde_json::json!({})[\"{}\"].clone()", to_json_arg(base, ctx), field)
        }
        // Empty arrays in json! context need explicit typing
        Expr::ArrayLit(items) if items.is_empty() => {
            "serde_json::Value::Array(vec![])".to_string()
        }
        Expr::ArrayLit(items) => {
            let vals: Vec<String> = items.iter().map(|e| to_json_arg(e, ctx)).collect();
            format!("vec![{}]", vals.join(", "))
        }
        _ => expr_to_rust(expr, ctx),
    }
}

