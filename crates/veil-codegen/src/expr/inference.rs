use std::collections::HashSet;
use veil_ir::ast::*;
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::*;

/// Public wrapper for `infer_expr_type`, for the return-type pre-scan.
pub fn infer_expr_type_pub(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    infer_expr_type(expr, ctx)
}

/// Infer the element type of an iterable expression. If it's a local whose
/// tracked type is `Vec<T>` (or a boxed-trait vec), return the inner `T`
/// (unwrapping `Box<dyn T ..>` to `T`) so method calls on the loop var resolve.
pub fn element_type_of(iterable: &Expr, ctx: &GenCtx) -> Option<String> {
    let vec_type = match iterable {
        Expr::Ident(name) => {
            // Self fields in method bodies: `for x in api_endpoints` after bare-field rewrite.
            if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                // Look up via any struct_fields entry that has this field.
                ctx.types.struct_fields.values().find_map(|fields| {
                    fields
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, t)| t.clone())
                })
            } else {
                ctx.local_type(name).map(|s| s.to_string())
            }
        }
        Expr::FieldAccess(base, field) => {
            if let Expr::Ident(base_name) = base.as_ref() {
                if base_name == "self" && ctx.in_method {
                    ctx.types.struct_fields.values().find_map(|fields| {
                        fields
                            .iter()
                            .find(|(n, _)| n == field)
                            .map(|(_, t)| t.clone())
                    })
                } else if let Some(type_name) = ctx.local_type(base_name) {
                    ctx.field_type(type_name, field).map(|s| s.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }?;
    let inner = vec_type
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
        .or_else(|| {
            // Also accept `std::collections::…` forms — take after last `Vec<`.
            vec_type
                .rfind("Vec<")
                .map(|i| &vec_type[i + 4..vec_type.len().saturating_sub(1)])
        })?;
    let inner = inner.trim();
    // Unwrap Box<dyn Trait + Send + Sync> → Trait.
    if let Some(rest) = inner.strip_prefix("Box<dyn ") {
        let name = rest.split([' ', '+', '>']).next().unwrap_or(rest);
        return Some(name.to_string());
    }
    Some(inner.to_string())
}

/// Infer the Rust type of a flow's return expression (`ret <expr>`).
/// Resolves idents and field access against known local/struct-field types.
pub fn infer_return_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::IntLit(_) => Some("i64".to_string()),
        Expr::FloatLit(_) => Some("f64".to_string()),
        Expr::BoolLit(_) => Some("bool".to_string()),
        Expr::StringLit(_) | Expr::StringInterp(_) => Some("String".to_string()),
        Expr::Ident(name) => ctx.local_type(name).map(|s| s.to_string()),
        Expr::FieldAccess(base, field) => {
            // Resolve the base's type, then the field's declared type.
            if let Expr::Ident(name) = base.as_ref()
                && let Some(type_name) = ctx.local_type(name) {
                    if type_name == "serde_json::Value" {
                        // Orchestrator: JSON index — type is Value.
                        return Some("serde_json::Value".to_string());
                    }
                    if let Some(ft) = ctx.field_type(type_name, field) {
                        return Some(rust_type_for_named(ft));
                    }
                }
            None
        }
        Expr::Call(_) => infer_expr_type(expr, ctx),
        _ => None,
    }
}

/// Normalize a VEIL match pattern into Rust form. VEIL writes `Ok _` / `Err e`
/// (space-separated binding); Rust needs `Ok(_)` / `Err(e)`. A bare word or
/// already-parenthesized pattern is left as-is.
pub fn normalize_match_pattern(pattern: &str, ctx: &GenCtx) -> String {
    let p = pattern.trim();
    // Convert dot-separated variant paths to Rust :: syntax
    // e.g. "DeployUnitType.LambdaApi" → "DeployUnitType::LambdaApi"
    if p.contains('.') && p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        let converted = p.replace('.', "::");
        // Check for variant-with-binding after conversion
        if let Some((head, rest)) = converted.split_once(char::is_whitespace) {
            let rest = rest.trim();
            if !rest.is_empty() && !rest.starts_with('(') && head.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                return format!("{}({})", head, rest);
            }
        }
        return converted;
    }
    // Enum-variant-with-binding: `Variant binding` → `Variant(binding)`.
    if let Some((head, rest)) = p.split_once(char::is_whitespace) {
        let rest = rest.trim();
        if !rest.is_empty() && !rest.starts_with('(') {
            // Only treat capitalized heads as variants (Ok, Err, Some, custom).
            if head.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                let head = qualify_variant_name(head, Some(&ctx.enum_variants));
                return format!("{}({})", head, rest);
            }
        }
    }
    qualify_variant_name(p, Some(&ctx.enum_variants))
}

/// Map a VEIL simple type name (as stored in struct_fields) to its Rust form.
pub fn rust_type_for_named(name: &str) -> String {
    match name {
        "Str" => "String".to_string(),
        "Int" => "i64".to_string(),
        "F64" => "f64".to_string(),
        "Bool" => "bool".to_string(),
        "Bytes" => "Vec<u8>".to_string(),
        "UUID" | "Id" => "Uuid".to_string(),
        "DateTime" | "Dt" => "DateTime<Utc>".to_string(),
        "Json" => "serde_json::Value".to_string(),
        other => other.to_string(),
    }
}

/// Default expression for a field type when a positional `Type.new(a, b)` call
/// omits trailing fields. Types are stored as Rust forms from struct_fields.
pub fn field_type_default_expr(rust_ty: &str, field_name: &str) -> String {
    let t = rust_ty.trim();
    if t.starts_with("Option<") {
        return "None".to_string();
    }
    if t.starts_with("Vec<") {
        return "Vec::new()".to_string();
    }
    if t.contains("HashMap") {
        return "std::collections::HashMap::new()".to_string();
    }
    if t.contains("HashSet") {
        return "std::collections::HashSet::new()".to_string();
    }
    match t {
        "String" => {
            // Conventional auth header name used by CreateProvider defaults.
            if field_name == "authorization_header_string" {
                "\"Authorization\".to_string()".to_string()
            } else {
                "String::new()".to_string()
            }
        }
        "i64" | "i32" | "u64" | "u32" | "usize" | "isize" => "0".to_string(),
        "f64" | "f32" => "0.0".to_string(),
        "bool" => "false".to_string(),
        "Uuid" => "Uuid::new_v4()".to_string(),
        "DateTime<Utc>" => "Utc::now()".to_string(),
        "serde_json::Value" => "serde_json::json!({})".to_string(),
        // Nested domain types / enums: prefer Default (emitted for all-defaultable
        // VOs; enums can derive or use first-variant Default later).
        other => format!("{}::default()", other),
    }
}

/// Attempt to infer the type of an expression from context.
pub fn infer_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::Call(call) => {
            if call.receiver.is_none() && call.args.is_empty() {
                let leaf = lang_type_leaf(&call.target);
                let method = method_bare(&call.method);
                if matches!(
                    (leaf, method),
                    ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
                ) {
                    return Some("String".to_string());
                }
                if matches!((leaf, method), ("Int", "now_unix") | ("Int", "now")) {
                    return Some("i64".to_string());
                }
            }
            if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
                return Some("i64".to_string());
            }
            if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
                return Some("serde_json::Value".to_string());
            }
            if call.args.is_empty() && method_bare(&call.method) == "as_n" {
                return Some("i64".to_string());
            }
            // If calling a trait method, return type is known.
            // Bang only unwraps Result (via `.await?`); Opt/Option is preserved.
            if ctx.is_trait_target(&call.target) {
                let method = if call.method.is_empty() {
                    "call"
                } else {
                    &call.method
                };
                return ctx.return_type_of(&call.target, method).map(|s| s.to_string());
            }
            // If calling a struct constructor
            if ctx.is_struct_target(&call.target) {
                let method = if call.method.is_empty() { "new" } else { &call.method };
                return ctx.return_type_of(&call.target, method).map(|s| {
                    // Resolve "Self" to the actual struct name
                    if s == "Self" { call.target.clone() } else { s.to_string() }
                });
            }
            // If calling a method on a local (e.g. @dep wear_test_repo typed as trait via name_to_shape)
            if ctx.is_local(&call.target) || ctx.is_trait_target(&call.target) {
                if let Some(t) = ctx.return_type_of(&call.target, &call.method) {
                    return Some(t.to_string());
                }
                // Resolve through the local's inferred type:
                // e.g. `repo` has type `Repository`, so `repo.write_blob(...)` → look up
                // `Repository.write_blob` return type.
                if let Some(local_ty) = ctx.local_type(&call.target) {
                    let method = call.method.trim_end_matches(['!', '?']);
                    if let Some(t) = ctx.return_type_of(local_ty, method) {
                        return Some(t.to_string());
                    }
                }
            }
            // Adapter `@dep` / `@field` used as a bare ident (`routing_table.get_route!`).
            if let Some(fty) = ctx
                .self_field_types
                .get(&call.target)
                .or_else(|| ctx.self_field_types.get(&to_snake(&call.target)))
            {
                let method = call.method.trim_end_matches(['!', '?']);
                let bare_ty = peel_dyn_trait_name(fty).unwrap_or_else(|| fty.clone());
                if let Some(t) = ctx.return_type_of(&bare_ty, method) {
                    return Some(t.to_string());
                }
                if let Some(t) = ctx.return_type_of(fty, method) {
                    return Some(t.to_string());
                }
            }
            // Stub package free functions: `gix.init_bare(path)` → target is "gix",
            // method is "init_bare". Look up (stub_name, method) in method_returns.
            if ctx.stubs.stub_pkg_crate.contains_key(&call.target) {
                let method = call.method.trim_end_matches(['!', '?']);
                if let Some(t) = ctx.return_type_of(&call.target, method) {
                    return Some(t.to_string());
                }
            }
            // Also handle receiver-based form: `receiver.method(args)` where receiver is a stub pkg ident.
            if let Some(recv) = &call.receiver {
                if let Expr::Ident(recv_name) = recv.as_ref() {
                    // Receiver is a stub package (e.g. `gix.init_bare(...)`)
                    if ctx.stubs.stub_pkg_crate.contains_key(recv_name) {
                        let method = call.method.trim_end_matches(['!', '?']);
                        if let Some(t) = ctx.return_type_of(recv_name, method) {
                            return Some(t.to_string());
                        }
                    }
                    // Receiver is a local variable with a known type
                    if let Some(local_ty) = ctx.local_type(recv_name) {
                        let method = call.method.trim_end_matches(['!', '?']);
                        if let Some(t) = ctx.return_type_of(local_ty, method) {
                            return Some(t.to_string());
                        }
                    }
                    if let Some(fty) = ctx
                        .self_field_types
                        .get(recv_name)
                        .or_else(|| ctx.self_field_types.get(&to_snake(recv_name)))
                    {
                        let method = call.method.trim_end_matches(['!', '?']);
                        let bare_ty = peel_dyn_trait_name(fty).unwrap_or_else(|| fty.clone());
                        if let Some(t) = ctx.return_type_of(&bare_ty, method) {
                            return Some(t.to_string());
                        }
                    }
                }
                // Receiver is a chained call (e.g. `ThreadSafeRepository.open(path).to_thread_local()`)
                // Recursively infer the receiver's type, then look up the method on that type.
                if let Some(recv_type) = infer_expr_type(recv, ctx) {
                    let method = call.method.trim_end_matches(['!', '?']);
                    // "Self" return means same type as receiver
                    if let Some(t) = ctx.return_type_of(&recv_type, method) {
                        if t == "Self" {
                            return Some(recv_type);
                        }
                        return Some(t.to_string());
                    }
                }
            }
            None
        }
        // Empty list `[]` — element unknown until append
        Expr::ArrayLit(items) if items.is_empty() => Some("Vec<()>".to_string()),
        Expr::ArrayLit(items) => items
            .first()
            .and_then(|e| infer_expr_type(e, ctx))
            .map(|t| format!("Vec<{t}>")),
        Expr::BinaryOp(bin) if matches!(bin.op, BinOp::Add) => {
            // options + [x] → keep/upgrade Vec type
            let left = infer_expr_type(&bin.left, ctx);
            let right = infer_expr_type(&bin.right, ctx);
            match (left.as_deref(), right.as_deref()) {
                (Some("Vec<()>"), Some(r)) if r.starts_with("Vec<") => right,
                (Some(l), _) if l.starts_with("Vec<") && l != "Vec<()>" => left,
                (_, Some(r)) if r.starts_with("Vec<") => right,
                (Some(l), _) if rust_ty_is_stringish(l) => Some("String".into()),
                (_, Some(r)) if rust_ty_is_stringish(r) => Some("String".into()),
                _ if matches!(&*bin.left, Expr::StringLit(_))
                    || matches!(&*bin.right, Expr::StringLit(_)) =>
                {
                    Some("String".into())
                }
                _ => left.or(right),
            }
        }
        Expr::StructLit(name, _) => Some(name.clone()),
        Expr::Ident(name) => ctx.local_type(name).map(|s| s.to_string()),
        Expr::FieldAccess(base, field) => {
            if is_json_rooted_expr(
                &Expr::FieldAccess(base.clone(), field.clone()),
                ctx,
            ) {
                return Some("serde_json::Value".to_string());
            }
            if let Expr::Ident(n) = base.as_ref() {
                if n == "self"
                    && let Some(ty) = ctx
                        .self_field_types
                        .get(field)
                        .or_else(|| ctx.self_field_types.get(&to_snake(field)))
                    {
                        return Some(ty.clone());
                    }
                if let Some(base_ty) = ctx.local_type(n) {
                    let leaf = lang_type_leaf(base_ty);
                    if let Some(ft) = ctx
                        .field_type(base_ty, field)
                        .or_else(|| ctx.field_type(base_ty, &to_snake(field)))
                        .or_else(|| ctx.field_type(leaf, field))
                        .or_else(|| ctx.field_type(leaf, &to_snake(field)))
                    {
                        return Some(ft.to_string());
                    }
                }
            }
            None
        }
        Expr::Index(base, idx) => {
            if matches!(idx.as_ref(), Expr::IntLit(_)) {
                return element_type_of(base, ctx);
            }
            if is_json_rooted_expr(base, ctx)
                || infer_expr_type(base, ctx)
                    .as_deref()
                    .is_some_and(is_json_type_name)
            {
                return Some("serde_json::Value".to_string());
            }
            element_type_of(base, ctx)
        }
        Expr::IntLit(_) => Some("i64".to_string()),
        Expr::FloatLit(_) => Some("f64".to_string()),
        Expr::BoolLit(_) => Some("bool".to_string()),
        Expr::StringLit(_) => Some("String".to_string()),
        // Layer actions (invoke, request, etc.) return serde_json::Value
        Expr::Action(_) => Some("serde_json::Value".to_string()),
        Expr::Require(inner) => {
            // `require json.field` lowers to String (as_str + ok_or). Inferring
            // Value here makes later struct fields emit `string.as_str().unwrap_or`
            // which does not compile (SL-027).
            if expr_is_json(inner, ctx) {
                return Some("String".to_string());
            }
            infer_expr_type(inner, ctx).map(|t| {
                peel_option_rust(&t)
                    .map(|s| s.to_string())
                    .unwrap_or(t)
            })
        }
        // ─── Newly inferred expression types ─────────────────────────────
        // BinaryOp: comparisons return bool; arithmetic preserves the operand type.
        Expr::BinaryOp(bin) => {
            match bin.op {
                BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
                | BinOp::And | BinOp::Or => Some("bool".to_string()),
                BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                    // Arithmetic: infer from left operand, fall back to right
                    infer_expr_type(&bin.left, ctx).or_else(|| infer_expr_type(&bin.right, ctx))
                }
                // Add is handled above (Vec concat, string concat)
                _ => None,
            }
        }
        // UnaryOp: Not → bool, Neg preserves the type.
        Expr::UnaryOp(un) => {
            match un.op {
                UnaryOp::Not => Some("bool".to_string()),
                UnaryOp::Neg => infer_expr_type(&un.expr, ctx),
            }
        }
        // IfExpr: type is the type of the then-branch (or else-branch).
        Expr::IfExpr(if_data) => {
            // If there's a then body, infer from the last expression
            if let Some(last) = if_data.then_body.last()
                && let Some(t) = infer_expr_type(last, ctx) {
                    return Some(t);
                }
            // Try else branch
            if let Some(else_body) = &if_data.else_body
                && let Some(last) = else_body.last() {
                    return infer_expr_type(last, ctx);
                }
            None
        }
        // Match: type is the type of the first arm's body.
        Expr::Match(_, arms) => {
            for arm in arms {
                if let Some(last) = arm.body.last()
                    && let Some(t) = infer_expr_type(last, ctx) {
                        return Some(t);
                    }
            }
            None
        }
        // StringInterp (f-strings): always produce String.
        Expr::StringInterp(_) => Some("String".to_string()),
        // Closure: we can't easily infer the full Fn type, but return the body type.
        Expr::Closure { body, .. } => {
            body.last().and_then(|e| infer_expr_type(e, ctx))
        }
        // Await: infer the inner expression type (the future resolves to it).
        Expr::Await(inner) => infer_expr_type(inner, ctx),
        // Try (?): peels Result/Option, returns the inner T.
        Expr::Try(inner) => {
            infer_expr_type(inner, ctx).map(|t| {
                // Peel Result<T, _> → T  or Option<T> → T
                if let Some(inner_t) = peel_option_rust(&t) {
                    return inner_t.to_string();
                }
                if t.starts_with("Result<") {
                    // Extract the success type from Result<T, E>
                    let inner_content = &t["Result<".len()..t.len().saturating_sub(1)];
                    // Split on first comma not inside angle brackets
                    let mut depth = 0;
                    for (i, ch) in inner_content.char_indices() {
                        match ch {
                            '<' => depth += 1,
                            '>' => depth -= 1,
                            ',' if depth == 0 => {
                                return inner_content[..i].trim().to_string();
                            }
                            _ => {}
                        }
                    }
                    return inner_content.to_string();
                }
                t
            })
        }
        // Cast: the result type is the target type name.
        Expr::Cast(_, target_ty) => Some(target_ty.clone()),
        // DoBlock: type is the last expression in the block.
        Expr::DoBlock(body) => {
            body.last().and_then(|e| infer_expr_type(e, ctx))
        }
        // Tuple: we don't track tuple types in the Rust backend currently.
        Expr::Tuple(_) => None,
        // Loops/ForLoop/WhileLoop: they produce () or the break value (unsupported).
        Expr::ForLoop { .. } | Expr::WhileLoop { .. } | Expr::Loop(_) => None,
        // Control flow: no meaningful type.
        Expr::Break | Expr::Continue | Expr::Return(_) => None,
        // Assignments produce () in expression position.
        Expr::Assign(..) | Expr::MutAssign(..) | Expr::LetPattern(..) => None,
        _ => None,
    }
}

/// Message type name from a desugared bus call arg (`invoke Reconcile{…}`).
pub fn bus_message_name_from_args(args: &[Expr]) -> Option<String> {
    match args.first() {
        Some(Expr::StructLit(name, _)) => Some(name.clone()),
        Some(Expr::Ident(name)) => Some(name.clone()),
        _ => None,
    }
}

/// Whether `ret` can be written as a bare path in this crate (local domain type
/// or language primitive). Foreign domain types are left as `serde_json::Value`.
pub fn bus_return_type_in_scope(ctx: &GenCtx, ret: &str) -> bool {
    let ret = ret.trim();
    if ret.is_empty() || ret == "()" || ret == "serde_json::Value" || ret.starts_with("Result<") {
        return false;
    }
    if matches!(
        ret,
        "String" | "bool" | "i64" | "i32" | "f64" | "f32" | "Uuid" | "usize"
    ) {
        return true;
    }
    if let Some(inner) = ret
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return bus_return_type_in_scope(ctx, inner.trim());
    }
    if let Some(inner) = ret
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return bus_return_type_in_scope(ctx, inner.trim());
    }
    // Only domain types defined in *this* crate (set by application codegen).
    ctx.local_domain_types.contains(ret)
}

/// Collect all trait-shaped construct names referenced in flow step bodies.
/// Returns the set of trait names that need to be in the Deps struct.
pub fn collect_deps(steps: &[FlowStep], ctx: &GenCtx) -> HashSet<String> {
    let mut deps = HashSet::new();
    for step in steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                collect_deps_from_expr(expr, ctx, &mut deps);
            }
        }
    }
    deps
}

pub fn collect_deps_from_expr(expr: &Expr, ctx: &GenCtx, deps: &mut HashSet<String>) {
    match expr {
        Expr::Call(call) => {
            if ctx.is_trait_target(&call.target) {
                deps.insert(call.target.clone());
            } else if call.method.ends_with('!') && !call.target.is_empty() {
                // VEIL convention: method! marks trait dep calls. Find matching trait.
                for (name, shape) in &ctx.name_to_shape {
                    if *shape == Shape::Trait {
                        let trait_snake = to_snake(name);
                        // Require exact match or underscore-boundary suffix match
                        // (e.g. "registry" matches "registry" or "acp_session_registry"
                        //  with suffix "_registry", but NOT "extension_registry" matching
                        //  bare "registry" — that's handled by explicit @dep annotations)
                        if trait_snake == call.target
                            || trait_snake.ends_with(&format!("_{}", call.target))
                        {
                            deps.insert(name.clone());
                            break;
                        }
                    }
                }
            }
            if let Some(recv) = &call.receiver {
                collect_deps_from_expr(recv, ctx, deps);
            }
            for arg in &call.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => collect_deps_from_expr(rhs, ctx, deps),
        Expr::Action(a) => {
            for arg in &a.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
            for (_, v) in &a.named_args {
                collect_deps_from_expr(v, ctx, deps);
            }
            if let Some(c) = &a.condition {
                collect_deps_from_expr(c, ctx, deps);
            }
            for e in &a.body {
                collect_deps_from_expr(e, ctx, deps);
            }
            // requires_dep / trait dep targets count as deps
            if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
                if let Some(dep) = &spec.requires_dep {
                    deps.insert(dep.clone());
                } else if let Some(port) = &spec.port_target {
                    deps.insert(port.clone());
                }
            }
        }
        Expr::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_deps_from_expr(v, ctx, deps);
            }
        }
        Expr::Match(scrutinee, arms) => {
            collect_deps_from_expr(scrutinee, ctx, deps);
            for arm in arms {
                for expr in &arm.body {
                    collect_deps_from_expr(expr, ctx, deps);
                }
            }
        }
        Expr::IfExpr(data) => {
            collect_deps_from_expr(&data.condition, ctx, deps);
            for expr in &data.then_body {
                collect_deps_from_expr(expr, ctx, deps);
            }
            if let Some(eb) = &data.else_body {
                for expr in eb {
                    collect_deps_from_expr(expr, ctx, deps);
                }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            collect_deps_from_expr(iterable, ctx, deps);
            for expr in body {
                collect_deps_from_expr(expr, ctx, deps);
            }
        }
        Expr::WhileLoop { condition, body } => {
            collect_deps_from_expr(condition, ctx, deps);
            for expr in body {
                collect_deps_from_expr(expr, ctx, deps);
            }
        }
        Expr::Return(inner) => {
            collect_deps_from_expr(inner, ctx, deps);
        }
        _ => {}
    }
}

/// Generate the Deps struct source for a set of trait dependencies.
pub fn gen_deps_struct(dep_names: &HashSet<String>) -> String {
    if dep_names.is_empty() {
        return String::new();
    }
    let mut out = String::from("/// Injected trait dependencies.\npub struct Deps {\n");
    let mut sorted: Vec<&String> = dep_names.iter().collect();
    sorted.sort();
    for name in sorted {
        out.push_str(&format!(
            "    pub {}: std::sync::Arc<dyn {} + Send + Sync>,\n",
            to_snake(name), name
        ));
    }
    out.push_str("}\n\n");
    out
}

pub fn binop_to_rust(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

pub fn unaryop_to_rust(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}
