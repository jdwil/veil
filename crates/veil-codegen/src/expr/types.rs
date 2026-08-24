use std::collections::{HashMap, HashSet};
use veil_ir::ast::*;
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::*;

/// Whether a stored struct field (Rust type string) can be filled without a
/// `new(...)` argument. Mirrors smart-ctor defaults in rust.rs.
pub fn rust_field_is_defaultable(
    field_name: &str,
    rust_ty: &str,
    ctor_pol: &veil_ir::layer::ConstructorPolicy,
    defaultable: &HashSet<String>,
) -> bool {
    if ctor_pol.is_auto_field(field_name) {
        return true;
    }
    // Field-name string defaults (constructor_policy-adjacent conventions).
    if field_name == "authorization_header_string" {
        return true;
    }
    let t = rust_ty.trim();
    if t.starts_with("Option<")
        || t.starts_with("Vec<")
        || t.contains("HashMap")
        || t.contains("HashSet")
    {
        return true;
    }
    // INV-002 scalar type defaults (policy table, not domain words).
    for (veil_ty, _) in &ctor_pol.type_defaults {
        let rust = rust_type_for_named(veil_ty);
        if t == rust {
            return true;
        }
    }
    // Nested type already known defaultable.
    if defaultable.contains(t) {
        return true;
    }
    // Unit enums and domain types that implement Default appear as bare names.
    // Only treat as defaultable once registered (fixpoint / unit-enum pass).
    false
}

/// Record a unit variant → enum type. Ambiguous names (two enums, same variant)
/// are dropped so we never invent the wrong qualifier.
pub fn register_enum_variant(ctx: &mut GenCtx, variant: &str, enum_name: &str) {
    if variant.is_empty()
        || matches!(
            variant,
            "Ok" | "Err" | "Some" | "None" | "true" | "false" | "_" | "null" | "noop"
        )
    {
        return;
    }
    match ctx.enum_variants.get(variant) {
        Some(existing) if existing != enum_name => {
            ctx.enum_variants.remove(variant);
        }
        Some(_) => {}
        None => {
            ctx.enum_variants
                .insert(variant.to_string(), enum_name.to_string());
        }
    }
}

/// Qualified constructor for a stub type (`example_sdk::primitives::Blob`).
/// Accepts a bare name (`Blob`) or a crate-qualified VEIL path
/// (`aws_sdk_lambda.Blob` / `aws_sdk_lambda::Blob`). Falls back to the
/// leaf name so rustc names the missing type instead of emitting `Vec<u8>`.
pub fn stub_ctor_path(ctx: &GenCtx, type_name: &str) -> String {
    if let Some((c, p)) = stub_type_parts(ctx, type_name) {
        return format!("{c}::{p}");
    }
    lang_type_leaf(type_name).to_string()
}

/// `(crate, rust_type_path)` for a stub type. Tries the written name, then
/// `crate.Leaf` / `crate::Leaf`, then a unique bare leaf. Never invents a
/// module — `rust_type_path` on the stub is the only source of `types::` /
/// `primitives::`.
pub fn stub_type_parts<'a>(ctx: &'a GenCtx, type_name: &str) -> Option<(&'a str, &'a str)> {
    if let Some((c, p)) = ctx.stubs.stub_type_crate.get(type_name) {
        return Some((c.as_str(), p.as_str()));
    }
    let leaf = lang_type_leaf(type_name);
    let crate_guess = type_name
        .split(['.', ':'])
        .next()
        .unwrap_or("")
        .replace('-', "_");
    if !crate_guess.is_empty() && leaf != type_name {
        for key in [
            format!("{crate_guess}.{leaf}"),
            format!("{crate_guess}::{leaf}"),
        ] {
            if let Some((c, p)) = ctx.stubs.stub_type_crate.get(&key) {
                return Some((c.as_str(), p.as_str()));
            }
        }
    }
    if leaf != type_name
        && let Some((c, p)) = ctx.stubs.stub_type_crate.get(leaf) {
            return Some((c.as_str(), p.as_str()));
        }
    None
}

/// Last path segment of a VEIL type (`aws_sdk_lambda.Blob` → `Blob`).
pub fn lang_type_leaf(target: &str) -> &str {
    target
        .rsplit(['.', ':'])
        .find(|s| !s.is_empty())
        .unwrap_or(target)
}

pub fn method_bare(method: &str) -> &str {
    method.trim_end_matches(['!', '?'])
}

/// SDK / stub `Res!` errors are often `&T` or types with Debug but not Display.
/// Never use `e.to_string()` for unknown E.
pub fn map_err_domain(em: &super::context::ErrorModel) -> String {
    format!(".map_err(|e| {}(format!(\"{{e:?}}\")))?", em.external_path())
}

/// `Res!<Str>` on the Rust side is usually `Result<&str, E>`. VEIL `Str` is
/// owned `String`, so own the payload and map the error via Debug.
pub fn map_err_domain_own_str(em: &super::context::ErrorModel) -> String {
    format!(".map(|s| s.to_string()).map_err(|e| {}(format!(\"{{e:?}}\")))?", em.external_path())
}

/// `.await.map_err(...)` suffix for async+fallible methods, parameterized by error model.
pub fn map_err_await_domain(em: &super::context::ErrorModel) -> String {
    format!(
        ".await.map_err(|e| {}::{}(format!(\"{{e:?}}\")))?",
        em.type_name, em.external
    )
}

pub fn is_str_like_return(ty: &str) -> bool {
    let t = ty.trim();
    matches!(
        t,
        "Str" | "String" | "&str" | "&String" | "Res!<Str>" | "Opt<Str>"
    ) || t.starts_with("Result<String")
        || t.starts_with("Result<&str")
        || t.starts_with("Result<&String")
}

/// True when a stub method's success type is VEIL `Str` (own a `String`).
/// Name fallback is only `as_s` / `as_n` — other `as_*` extractors return
/// maps, lists, bools, bytes.
pub fn should_own_str_result(ctx: &GenCtx, recv_ty: Option<&str>, method: &str) -> bool {
    let bare = method_bare(method);
    // `as_n` is a number in VEIL even when the stub says Res!<Str>.
    if bare == "as_n" {
        return false;
    }
    if let Some(ty) = recv_ty {
        let leaf = lang_type_leaf(ty);
        for key in [ty, leaf] {
            if let Some(ret) = ctx.return_type_of(key, bare) {
                return is_str_like_return(ret);
            }
        }
    }
    matches!(bare, "as_s")
}

pub fn parse_i64_suffix(em: &super::context::ErrorModel) -> String {
    format!(".parse::<i64>().map_err(|e| {}(format!(\"{{e:?}}\")))?", em.external_path())
}

pub fn peel_option_rust(ty: &str) -> Option<&str> {
    let t = ty.trim();
    t.strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .or_else(|| t.strip_prefix("Opt<").and_then(|s| s.strip_suffix('>')))
}

pub fn rust_ty_is_stringish(ty: &str) -> bool {
    matches!(
        ty.trim(),
        "String" | "Str" | "&str" | "&String" | "&'static str"
    )
}

pub fn rust_ty_is_numeric(ty: &str) -> bool {
    matches!(
        ty.trim(),
        "i64" | "i32" | "u64" | "u32" | "usize" | "isize" | "f64" | "f32" | "Int" | "F64"
    )
}

pub fn rust_ty_is_copy(ty: &str) -> bool {
    matches!(
        ty.trim(),
        "i64" | "i32" | "i16" | "i8"
            | "u64" | "u32" | "u16" | "u8"
            | "usize" | "isize"
            | "f64" | "f32"
            | "bool"
            | "Int" | "F64" | "Bool"
    )
}

pub fn rust_already_owned(s: &str) -> bool {
    let t = s.trim();
    t.ends_with(".clone()")
        || t.ends_with(".to_string()")
        || t.ends_with(".to_owned()")
        || t.ends_with(".cloned()")
        || t.contains(".map(|s| s.to_string())")
}

pub fn rust_string_lit(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn rust_string_lit_owned(s: &str) -> String {
    format!("{}.to_string()", rust_string_lit(s))
}

/// VEIL `Str` *values* are owned `String`. Bare lits stay for `==` and
/// match *patterns*. Use this for match/if arm values and other value positions.
pub fn expr_to_rust_value(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::StringLit(s) => rust_string_lit_owned(s),
        other => expr_to_rust(other, ctx),
    }
}

pub fn rust_ty_is_unit_enum(ty: &str, ctx: &GenCtx) -> bool {
    let leaf = lang_type_leaf(ty);
    ctx.unit_enums.contains(leaf) || ctx.unit_enums.contains(ty.trim())
}

pub fn rust_is_copy_value(expr: &Expr, rust: &str, ctx: &GenCtx) -> bool {
    if rust.parse::<i64>().is_ok() || rust == "true" || rust == "false" {
        return true;
    }
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::BoolLit(_) => true,
        Expr::Ident(n) => is_copy_local(n, ctx) || is_unit_enum_variant(n, ctx),
        Expr::FieldAccess(base, field) => {
            if let Expr::Ident(ty) = base.as_ref()
                && ctx.unit_enums.contains(ty) {
                    return true;
                }
            field_access_is_copy(base, field, ctx)
        }
        _ => infer_expr_type(expr, ctx).as_deref().is_some_and(|t| {
            rust_ty_is_copy(t) || rust_ty_is_unit_enum(t, ctx)
        }),
    }
}

pub fn is_unit_enum_variant(name: &str, ctx: &GenCtx) -> bool {
    ctx.enum_variants
        .get(name)
        .is_some_and(|enum_ty| rust_ty_is_unit_enum(enum_ty, ctx))
}

pub fn should_clone_ident(name: &str, ctx: &GenCtx) -> bool {
    if is_copy_local(name, ctx) || is_unit_enum_variant(name, ctx) {
        return false;
    }
    // Shared-ref loop element (`for x in &xs`) is `&T`. Owned slots need `.clone()`.
    if ctx.ownership.ref_elem_locals.contains(name) {
        return true;
    }
    if is_ref_local(name, ctx) {
        return false;
    }
    // Unknown count → clone (safe). Count of 1 → last/only use → move.
    ctx.ownership.ident_uses.get(name).copied().unwrap_or(2) > 1
}

pub fn rust_success_is_str(ty: &str) -> bool {
    if is_str_like_return(ty) {
        return true;
    }
    let t = ty.trim();
    if let Some(inner) = t.strip_prefix("Result<") {
        let success = inner
            .rsplit_once(',')
            .map(|(a, _)| a.trim())
            .unwrap_or(inner);
        return is_str_like_return(success)
            || success == "Option<String>"
            || success == "Option<Str>";
    }
    false
}

pub fn rust_ty_is_option_or_result(ty: &str) -> bool {
    let t = ty.trim();
    t.starts_with("Option<")
        || t.starts_with("Opt<")
        || t.starts_with("Result<")
        || t.starts_with("Res!")
}

/// Whether a type string represents an Option type.
pub fn is_option_type(ty: &str) -> bool {
    ty.starts_with("Option<") || ty.starts_with("Opt<")
}

/// Whether a type string represents a Result type.
pub fn is_result_type(ty: &str) -> bool {
    ty.starts_with("Result<")
}

pub fn rust_ty_is_bytes_like(ty: &str) -> bool {
    let leaf = lang_type_leaf(ty);
    leaf == "Blob"
        || leaf == "Bytes"
        || ty == "Vec<u8>"
        || ty.ends_with("::Blob")
        || ty.contains("Blob")
}

/// VEIL values are reusable. A field read is a copy of the field, not a move.
pub fn field_access_is_copy(base: &Expr, field: &str, ctx: &GenCtx) -> bool {
    let base_ty = match base {
        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
        _ => infer_expr_type(base, ctx),
    };
    let Some(base_ty) = base_ty else {
        return false;
    };
    let peeled = peel_option_rust(&base_ty).unwrap_or(base_ty.as_str());
    let leaf = lang_type_leaf(peeled);
    for key in [peeled, leaf] {
        if let Some(ft) = ctx
            .field_type(key, field)
            .or_else(|| ctx.field_type(key, &to_snake(field)))
        {
            return rust_ty_is_copy(ft) || rust_ty_is_unit_enum(ft, ctx);
        }
    }
    false
}

pub fn recv_rust_type(recv: &Expr, ctx: &GenCtx) -> Option<String> {
    match recv {
        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
        _ => infer_expr_type(recv, ctx),
    }
}

/// `as_ref` / bytes view used where VEIL wants `Str` → utf-8 decode.
/// Never rewrite `Option`/`Result`/`String` `.as_ref()`.
pub fn should_decode_as_ref_to_str(recv: &Expr, ctx: &GenCtx) -> bool {
    let recv_ty = recv_rust_type(recv, ctx);
    if let Some(ty) = recv_ty.as_deref() {
        if rust_ty_is_option_or_result(ty) || rust_ty_is_stringish(ty) {
            return false;
        }
        if should_own_str_result(ctx, Some(ty), "as_ref") || rust_ty_is_bytes_like(ty) {
            return true;
        }
    }
    if ctx
        .expected_return_rust
        .as_deref()
        .is_some_and(rust_success_is_str)
    {
        return true;
    }
    ctx.types.method_returns.iter().any(|((ty, method), ret)| {
        method_bare(method) == "as_ref"
            && is_str_like_return(ret)
            && !rust_ty_is_option_or_result(ty)
    })
}

pub fn now_iso8601_rust() -> String {
    "Utc::now().to_rfc3339()".to_string()
}

pub fn expr_is_stringish(expr: &Expr, ctx: &GenCtx) -> bool {
    match expr {
        Expr::StringLit(_) | Expr::StringInterp(_) => true,
        Expr::Ident(n) => ctx.local_type(n).is_some_and(rust_ty_is_stringish),
        _ => infer_expr_type(expr, ctx)
            .as_deref()
            .is_some_and(rust_ty_is_stringish),
    }
}

pub fn expr_is_numeric(expr: &Expr, ctx: &GenCtx) -> bool {
    match expr {
        Expr::IntLit(_) | Expr::FloatLit(_) => true,
        Expr::Ident(n) => ctx.local_type(n).is_some_and(rust_ty_is_numeric),
        _ => infer_expr_type(expr, ctx)
            .as_deref()
            .is_some_and(rust_ty_is_numeric),
    }
}

/// Flatten `a + b + c` into leaf operands so one `format!` can own the chain.
pub fn flatten_str_add_chain(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::BinaryOp(op) if matches!(op.op, veil_ir::ast::BinOp::Add) => {
            let mut out = flatten_str_add_chain(&op.left);
            out.extend(flatten_str_add_chain(&op.right));
            out
        }
        _ => vec![expr],
    }
}

/// `format!("{}{}", ident, field)` must not move locals reused later.
pub fn clone_if_named_value(expr: &Expr, rust: String) -> String {
    if rust_already_owned(&rust) || rust.starts_with('"') || rust.starts_with("format!(") {
        return rust;
    }
    match expr {
        Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{rust}.clone()"),
        _ => rust,
    }
}

/// Drop a trailing try-suffix so a `match` can consume a `Result` directly.
/// String-pattern matches must **not** use this — they need the unwrapped value.
pub fn strip_try_suffix(raw: String) -> String {
    // Generic pattern: strip .await.map_err(...)? or .map_err(...)? or .await? or ?
    // The exact error type name doesn't matter — we just need to strip the try suffix.
    if let Some(pos) = raw.rfind(".await.map_err(") {
        if raw.ends_with(")?") {
            return format!("{}.await", &raw[..pos]);
        }
    }
    if let Some(stripped) = raw.strip_suffix(".await?") {
        return format!("{stripped}.await");
    }
    if let Some(pos) = raw.rfind(".map(|s| s.to_string()).map_err(") {
        if raw.ends_with(")?") {
            return raw[..pos].to_string();
        }
    }
    if let Some(pos) = raw.rfind(".map_err(") {
        if raw.ends_with(")?") {
            return raw[..pos].to_string();
        }
    }
    if let Some(stripped) = raw.strip_suffix('?') {
        return stripped.to_string();
    }
    raw
}

pub fn expr_handles_option_wrap(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Match(_, _) | Expr::IfExpr(_) | Expr::Return(_) | Expr::IfLet { .. }
    )
}

/// `null` / `()` → `None`; already-Option locals stay as-is; else `Some(val)`.
pub fn wrap_as_option_value(expr: &Expr, rust: String, ctx: &GenCtx) -> String {
    let t = rust.trim();
    if t == "None" || t == "()" {
        return "None".to_string();
    }
    if t.starts_with("Some(") || t.starts_with("return ") {
        return rust;
    }
    if let Expr::Ident(n) = expr
        && ctx
            .local_type(n)
            .is_some_and(|ty| is_option_type(ty))
        {
            return rust;
        }
    format!("Some({rust})")
}

/// True when `Type.new` is a module free-fn (`sqlx::query`), not `Type::new`.
/// Stub metadata only — never a type-name special case (`Query` is also a
/// DynamoDB rustdoc type with `fn new()`).
pub fn stub_new_is_module_free_fn(ctx: &GenCtx, effective_target: &str, type_leaf: &str) -> bool {
    ctx.stubs.stub_typed_ctors.contains_key(effective_target)
        || ctx.stubs.stub_typed_ctors.contains_key(type_leaf)
        || ctx
            .stubs.stub_type_crate
            .contains_key(&format!("{type_leaf}As"))
        || ctx.name_to_shape.contains_key(&format!("{type_leaf}As"))
}

pub fn bytes_from_str_expr(arg: &str) -> String {
    format!("{{ let __s = ({arg}).to_string(); __s.into_bytes() }}")
}

pub fn bytes_from_hex_expr(hex_expr: &str) -> String {
    format!(
        "{{ let __h: String = ({hex_expr}).to_string(); let __h = __h.as_str(); let mut __b = Vec::with_capacity(__h.len() / 2); let mut __i = 0usize; while __i + 1 < __h.len() {{ if let Ok(__v) = u8::from_str_radix(&__h[__i..__i + 2], 16) {{ __b.push(__v); }} __i += 2; }} __b }}"
    )
}

/// Extract the inner type from a TypeExpr (unwrapping Result/Optional).
pub fn extract_inner_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Result(Some(inner)) => type_name_simple(inner),
        TypeExpr::Result(None) => "()".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_name_simple(inner)),
        _ => type_name_simple(ty),
    }
}

/// Get a simple type name string from a TypeExpr.
/// Extract the inner domain struct type from a return type string.
/// e.g., `Result<Option<Widget>, DomainError>` → Some("Widget")
/// e.g., `Result<Vec<Cohort>, DomainError>` → Some("Cohort")
/// Only returns Some when the extracted type is a known struct in name_to_shape
/// AND all its fields are primitive types that a DB row can decode directly.
pub fn extract_domain_type_from_return(
    ret: &str,
    name_to_shape: &HashMap<String, Shape>,
) -> Option<String> {
    // Strip Result<..., E> wrapper (any error type)
    let inner = ret
        .strip_prefix("Result<")
        .and_then(|s| s.strip_suffix('>'))
        .and_then(|s| s.rsplit_once(", "))
        .map(|(inner, _)| inner)
        .unwrap_or(ret);
    // Strip Option<...> / Vec<...>
    let type_name = inner
        .strip_prefix("Option<").and_then(|s| s.strip_suffix('>'))
        .or_else(|| inner.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')))
        .unwrap_or(inner);
    // Check if it's a known struct
    if name_to_shape.get(type_name) == Some(&Shape::Struct) {
        Some(type_name.to_string())
    } else {
        None
    }
}

/// Expand stub `typed_type_params` template (`_, return_type` → `_, CohortDTO`).
pub fn expand_typed_type_params(template: &str, domain_type: &str) -> String {
    template
        .split(',')
        .map(|p| {
            let t = p.trim();
            if t == "return_type" || t == "$ret" {
                domain_type.to_string()
            } else {
                t.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn type_name_simple(ty: &TypeExpr) -> String {
    match ty {
        // Map VEIL builtins to their Rust form so inferred return types /
        // method_returns can be pasted into signatures (Json → serde_json::Value).
        TypeExpr::Named(n) => rust_type_for_named(n),
        TypeExpr::Generic(n, args) => {
            // Keep domain generics (EntityRepo<T>) by name; map List/Map/etc.
            match n.as_str() {
                "List" | "Vec" if args.len() == 1 => {
                    format!("Vec<{}>", type_name_simple(&args[0]))
                }
                "Opt" | "Option" if args.len() == 1 => {
                    format!("Option<{}>", type_name_simple(&args[0]))
                }
                "Map" | "HashMap" if args.len() == 2 => {
                    format!(
                        "HashMap<{}, {}>",
                        type_name_simple(&args[0]),
                        type_name_simple(&args[1])
                    )
                }
                "Set" | "HashSet" if args.len() == 1 => {
                    format!("HashSet<{}>", type_name_simple(&args[0]))
                }
                _ => n.clone(),
            }
        }
        TypeExpr::Result(Some(inner)) => type_name_simple(inner),
        TypeExpr::Result(None) => "()".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_name_simple(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", type_name_simple(inner)),
        TypeExpr::Map(k, v) => format!("HashMap<{}, {}>", type_name_simple(k), type_name_simple(v)),
        TypeExpr::Set(inner) => format!("HashSet<{}>", type_name_simple(inner)),
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_name_simple).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}; {}]", type_name_simple(inner), size),
        TypeExpr::Ref(inner, _) => type_name_simple(inner),
        TypeExpr::Dyn(inner) => format!("dyn {}", type_name_simple(inner)),
        TypeExpr::ImplTrait(inner) => format!("impl {}", type_name_simple(inner)),
        TypeExpr::FnPtr(_, _) => "fn()".to_string(),
        TypeExpr::LitStr(_) => "str".to_string(),
    }
}
