use veil_ir::ast::*;
mod translate;
pub use translate::*;

use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::*;

/// Determine the call suffix for a method invoked on a chained receiver.
///
/// - Fluent `.send()` / `.send_with()` are async + Result → `.await?`
/// - Stub methods marked async+fallible (BoxFuture / executor param) → `.await.map_err…?`
/// - Other stub methods marked `Res!` are sync Result → `map_err…?`
/// - Trait methods (trait deps) are async_trait + Result → `.await?`
///
/// **Receiver shape wins over bare method name.** The same identifier can name a
/// *receiver's* Shape (Struct vs Trait) when known, not a global method-name scan.
///
/// Method names may carry VEIL bang/query suffixes (`fetch_all!`); strip before lookup.
pub fn receiver_call_suffix(recv: &Expr, method: &str, ctx: &GenCtx) -> String {
    let has_bang = method.ends_with('!');
    let method = method.trim_end_matches(['!', '?']);

    // Resolve the static type of the receiver when we can (UFCS / local / self field).
    // Index into List/slice of trait objects also yields a trait receiver
    // (e.g. `steps[i].action(...)` for `List<SagaStep>`).
    let recv_type_name: Option<String> = match recv {
        Expr::Ident(name) => {
            if ctx.is_struct_target(name) || ctx.is_trait_target(name) {
                Some(name.clone())
            } else if let Some(t) = ctx.local_type(name) {
                Some(t.to_string())
            } else if let Some(t) = ctx
                .self_field_types
                .get(name)
                .or_else(|| ctx.self_field_types.get(&to_snake(name)))
            {
                Some(
                    peel_dyn_trait_name(t)
                        .unwrap_or_else(|| t.clone()),
                )
            } else if ctx.stubs.stub_type_crate.contains_key(name) {
                Some(name.clone())
            } else {
                None
            }
        }
        Expr::Index(base, _) => {
            // List/slice element: peel Vec/slice and Box<dyn Trait>
            if let Expr::Ident(name) = base.as_ref() {
                ctx.local_type(name)
                    .and_then(|t| extract_box_dyn_trait(t).or_else(|| extract_vec_elem(t)))
            } else {
                None
            }
        }
        // AST still has `.get(i)` before list-index lowering; treat as element access.
        Expr::Call(inner)
            if (inner.method == "get" || inner.method == "get!") && inner.args.len() == 1 =>
        {
            let base_name = if !inner.target.is_empty() {
                Some(inner.target.as_str())
            } else if let Some(r) = &inner.receiver {
                match r.as_ref() {
                    Expr::Ident(n) => Some(n.as_str()),
                    _ => None,
                }
            } else {
                None
            };
            base_name.and_then(|n| {
                ctx.local_type(n)
                    .and_then(|t| extract_box_dyn_trait(t).or_else(|| extract_vec_elem(t)))
            })
        }
        _ => None,
    };

    // Known struct / stub type: use stub fallibility metadata only (not trait scan).
    if let Some(ref ty) = recv_type_name {
        // Peel Box<dyn Trait + …> / bare trait names stored in local_types
        let bare = peel_dyn_trait_name(ty).unwrap_or_else(|| ty.clone());
        if ctx.name_to_shape.get(bare.as_str()) == Some(&Shape::Struct)
            || ctx.stubs.stub_type_crate.contains_key(bare.as_str())
            || ctx.stubs.stub_type_crate.contains_key(ty.as_str())
        {
            if ctx.stubs.async_fallible_methods.contains(method)
            {
                // async+fallible → unwrap Result; bare send() keeps Result so .is_ok()/.is_err() work.
                if has_bang {
                    return map_err_await_domain(&ctx.error_model);
                } else {
                    return ".await".to_string();
                }
            }
            if ctx.stubs.fallible_methods.contains(method) {
                let suffix = if should_own_str_result(ctx, Some(ty.as_str()), method) {
                    map_err_domain_own_str(&ctx.error_model)
                } else {
                    map_err_domain(&ctx.error_model)
                };
                // Only apply fallible suffix if this specific type has the method as fallible.
                // Use type_fallible_methods: (Type, method) set for precision.
                if ctx.stubs.type_fallible_methods.contains(&(bare.clone(), method.to_string())) {
                    return suffix.to_string();
                }
                // If the method is ONLY fallible (not ambiguous), apply it.
                if !ctx.stubs.non_fallible_methods.contains(method) {
                    return suffix.to_string();
                }
                // Ambiguous and not confirmed fallible on this type: no suffix.
            }
            return String::new();
        }
        if ctx.name_to_shape.get(bare.as_str()) == Some(&Shape::Trait)
            || ctx.name_to_shape.get(ty.as_str()) == Some(&Shape::Trait)
        {
            let fallible = has_bang
                || ctx
                    .stubs.type_fallible_methods
                    .contains(&(bare.clone(), method.to_string()))
                || ctx
                    .stubs.type_fallible_methods
                    .contains(&(ty.clone(), method.to_string()));
            return if fallible {
                ".await?".to_string()
            } else {
                ".await".to_string()
            };
        }
    }

    // Fluent SDK send / async fallible stubs (untyped receivers).
    if ctx.stubs.async_fallible_methods.contains(method)
    {
        // async+fallible → unwrap; bare send() keeps Result so .is_ok()/.is_err() work.
        if has_bang {
            return map_err_await_domain(&ctx.error_model);
        } else {
            return ".await".to_string();
        }
    }
    // Untyped receiver: method name appears on a trait dep → async_trait.
    // If a stub/struct also has the same method name (e.g. `delete`), do not
    // force await — that would break reqwest Client.delete. List elements of
    // trait objects are handled via Index + peel above (SagaStep.action).
    let is_trait_method = ctx.types.method_returns.keys().any(|(ty, m)| {
        m == method && ctx.name_to_shape.get(ty) == Some(&Shape::Trait)
    });
    let is_stub_or_struct_method = ctx.types.method_returns.keys().any(|(ty, m)| {
        m == method
            && (ctx.stubs.stub_type_crate.contains_key(ty)
                || ctx.name_to_shape.get(ty) == Some(&Shape::Struct))
    });
    if is_trait_method && !is_stub_or_struct_method {
        return if has_bang {
            ".await?".to_string()
        } else {
            ".await".to_string()
        };
    }
    // Sync Res! stub methods: map any Error into DomainError.
    // Only apply when the receiver is NOT a chained Call — intermediate methods
    // in builder chains (returning Self) should not get fallible suffixes even if
    // a method of the same name is fallible on a different type (Issue 5/global
    // name collision: e.g. gix.prefix() is Res! but S3 builder.prefix() is not).
    // Also skip if the method is ambiguous — exists as both fallible and non-fallible
    // across different stub types (e.g. gix Id.detach() is non-fallible but
    // Pathspec.detach() is fallible).
    let recv_is_chain = matches!(recv, Expr::Call(_));
    let is_ambiguous = ctx.stubs.non_fallible_methods.contains(method);
    if ctx.stubs.fallible_methods.contains(method) && !recv_is_chain && !is_ambiguous {
        let own = should_own_str_result(ctx, recv_type_name.as_deref(), method);
        return if own {
            map_err_domain_own_str(&ctx.error_model)
        } else {
            map_err_domain(&ctx.error_model)
        }
        .to_string();
    }
    // Terminal builder `.build!()` is fallible (BuildError) even on chains.
    if has_bang && method == "build" {
        return map_err_domain(&ctx.error_model).to_string();
    }
    // Fallback: if the method has a bang (!) and nothing else matched,
    // treat it as an async fallible call (common for SDK methods like collect!,
    // execute!, etc. on receivers whose type isn't in our stub system).
    if has_bang {
        return map_err_await_domain(&ctx.error_model);
    }
    String::new()
}

/// `Box<dyn SagaStep + Send + Sync>` / `Arc<dyn SnsClient + Send + Sync>` → trait name
pub fn peel_dyn_trait_name(ty: &str) -> Option<String> {
    let t = ty.trim();
    let after_dyn = if let Some(rest) = t.strip_prefix("Box<dyn ") {
        rest
    } else if let Some(rest) = t.strip_prefix("Arc<dyn ") {
        rest
    } else if let Some(rest) = t.strip_prefix("std::sync::Arc<dyn ") {
        rest
    } else { t.strip_prefix("dyn ")? };
    let name = after_dyn.split(['+', '>', ' ']).next()?;
    if !name.is_empty() {
        Some(name.to_string())
    } else {
        None
    }
}

/// Map a VEIL `self.X` / bare env ident onto the rust adapter field name.
/// `@env(TABLE_NAME)` is `table_name`; last-segment (`table` / `name`) and
/// the original `TABLE_NAME` still resolve to that field.
pub fn resolve_self_field_name(ctx: &GenCtx, field: &str) -> Option<String> {
    let snake = to_snake(field);
    let lower = field.to_ascii_lowercase();
    if ctx.self_field_types.contains_key(&snake) {
        return Some(snake);
    }
    if ctx.self_field_types.contains_key(&lower) {
        return Some(lower);
    }
    let mut best: Option<String> = None;
    let consider = |known: &str, needle: &str, best: &mut Option<String>| {
        if known == needle || known.rsplit('_').next() == Some(needle) {
            match best {
                None => *best = Some(known.to_string()),
                Some(b) if known.len() > b.len() => *best = Some(known.to_string()),
                _ => {}
            }
        }
    };
    for known in ctx
        .self_field_types
        .keys()
        .chain(ctx.self_fields.iter())
    {
        consider(known, &snake, &mut best);
        consider(known, &lower, &mut best);
    }
    best.filter(|b| ctx.self_fields.contains(b) || ctx.self_field_types.contains_key(b))
}

/// `Vec<Box<dyn T + …>>` / `&[Box<dyn T + …>]` → element type string
pub fn extract_vec_elem(ty: &str) -> Option<String> {
    let t = ty.trim();
    if let Some(inner) = t.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        return Some(inner.trim().to_string());
    }
    if let Some(inner) = t.strip_prefix("&[").and_then(|s| s.strip_suffix(']')) {
        return Some(inner.trim().to_string());
    }
    if let Some(inner) = t.strip_prefix("&mut [").and_then(|s| s.strip_suffix(']')) {
        return Some(inner.trim().to_string());
    }
    None
}

pub fn extract_box_dyn_trait(ty: &str) -> Option<String> {
    if let Some(elem) = extract_vec_elem(ty) {
        return peel_dyn_trait_name(&elem).or(Some(elem));
    }
    peel_dyn_trait_name(ty)
}

/// Rust method/path segment for a call: keep PascalCase for enum variants /
/// associated constructors (`AttributeValue::S`); snake_case for normal methods.
/// Strip VEIL fallible/query suffixes (`!` / `?`) — those are typecheck sugar only.
pub fn rust_method_name(method: &str) -> String {
    let method = method.trim_end_matches(['!', '?']);
    if method
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        method.to_string()
    } else {
        to_snake(method)
    }
}

/// Build a `serde_json::json!` object for a message with a `"type"` tag plus
/// its named fields — the wire form for a JSON envelope payload.
pub(super) fn json_message(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> String {
    let mut parts = vec![format!("\"type\": \"{}\"", name)];
    for (k, v) in fields {
        parts.push(format!("\"{}\": {}", k, to_json_arg(v, ctx)));
    }
    format!("serde_json::json!({{ {} }})", parts.join(", "))
}

/// Build a JSON envelope for a cross-boundary call routed through a routing
/// trait: `{ "target": T, "method": m, "args": [ ... ] }`. Positional args are
/// rendered as JSON values so the receiving side can decode them.
pub(super) fn json_envelope(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let arg_vals = args.iter().map(|a| to_json_arg(a, ctx)).collect::<Vec<_>>().join(", ");
    format!(
        "serde_json::json!({{ \"target\": \"{}\", \"method\": \"{}\", \"args\": [{}] }})",
        target, method, arg_vals
    )
}

/// Render call args, cloning value-bearing locals/state so passing them into a
/// by-value parameter doesn't move them out of the caller. Skips the routing
/// reference and Copy scalars (which don't move).
pub(super) fn clone_args(args: &[Expr], ctx: &GenCtx) -> String {
    args.iter()
        .map(|a| match a {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => format!("state[\"{}\"].clone()", n),
            // The routing reference and Copy scalars are passed as-is.
            Expr::Ident(n) if !ctx.routing.routing_ref.is_empty() && *n == ctx.routing.routing_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if is_ref_local(n, ctx) => n.clone(),
            // Stub-declared borrow fields (e.g. sqlx Executor requires &Pool).
            Expr::Ident(n) if ctx.ownership.borrow_fields.contains(n.as_str()) => format!("&self.{n}"),
            Expr::Ident(n) if ctx.is_local(n) && should_clone_ident(n, ctx) => {
                format!("{n}.clone()")
            }
            Expr::FieldAccess(base, field)
                if ctx.ownership.borrow_fields.contains(field.as_str())
                    && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                format!("&self.{field}")
            }
            _ => expr_to_rust(a, ctx),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Like `clone_args` but applies method-specific argument shaping (e.g. reqwest
/// `basic_auth` takes `Option` password).
pub(super) fn clone_args_for_method(method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    clone_args_for_typed_method(None, method, args, ctx)
}

/// Clone/ref args for a method call, with optional receiver type for ref-param resolution.
pub fn clone_args_for_typed_method(recv_type: Option<&str>, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let method = method.trim_end_matches(['!', '?']);

    // Check ref_params for this specific (type, method) combination.
    // If found, emit &arg for ref positions instead of arg.clone().
    if let Some(type_name) = recv_type
        && let Some(ref_flags) = ctx.types.ref_params.get(&(type_name.to_string(), method.to_string())) {
            return args.iter().enumerate().map(|(i, a)| {
                let is_ref = ref_flags.get(i).copied().unwrap_or(false);
                if is_ref {
                    let s = expr_to_rust(a, ctx);
                    if s.starts_with('&') {
                        s
                    } else if matches!(a, Expr::Ident(n) if ctx.is_local(n)) {
                        // Deref to &str for String locals — avoids &String which
                        // doesn't satisfy generic bounds like TryInto<FullName>.
                        format!("&*{s}")
                    } else if let Expr::StringLit(lit) = a {
                        // ref params expecting &str: emit bare string literal
                        format!("\"{}\"", lit.replace('\\', "\\\\").replace('"', "\\\""))
                    } else {
                        format!("&{s}")
                    }
                } else {
                    // Normal clone behavior for non-ref params
                    match a {
                        Expr::Ident(n) if ctx.is_local(n) && should_clone_ident(n, ctx) => {
                            format!("{n}.clone()")
                        }
                        Expr::StringLit(s) => rust_string_lit_owned(s),
                        _ => expr_to_rust(a, ctx),
                    }
                }
            }).collect::<Vec<_>>().join(", ");
        }
    // str::starts_with / contains / ends_with / replace take Pattern / &str —
    // string lits as &str, not owned String (Pattern not implemented for String).
    if matches!(
        method,
        "starts_with"
            | "contains"
            | "ends_with"
            | "strip_prefix"
            | "strip_suffix"
            | "replace"
            | "replacen"
            | "split"
    ) {
        return args
            .iter()
            .map(|a| match a {
                Expr::StringLit(s) => {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                }
                _ => {
                    let s = expr_to_rust(a, ctx);
                    // Owned String locals: borrow for Pattern / &str
                    if matches!(a, Expr::Ident(_)) {
                        format!("&{s}")
                    } else if s.starts_with('&') {
                        s
                    } else {
                        format!("&({s})")
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
    }
    // Option<&str>.unwrap_or("") — keep bare &str, not String
    if method == "unwrap_or" && args.len() == 1
        && let Expr::StringLit(s) = &args[0] {
            return format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        }
    if method == "basic_auth" && args.len() >= 2 {
        let user = clone_args(&args[..1], ctx);
        let pass = expr_to_rust(&args[1], ctx);
        // reqwest: basic_auth(user, Option<password>)
        return format!("{user}, Some({pass})");
    }
    // sqlx bind: Uuid needs the `uuid` feature; bind as text to stay feature-light.
    if method == "bind" && args.len() == 1 {
        if let Expr::Ident(n) = &args[0]
            && (ctx.local_type(n) == Some("Uuid")
                || n == "id"
                || n.ends_with("_id")
                || n.ends_with("Id"))
            {
                return format!("{n}.to_string()");
            }
        if let Expr::FieldAccess(base, field) = &args[0] {
            let f = to_snake(field);
            if f == "id" || f.ends_with("_id") {
                let b = expr_to_rust(base, ctx);
                // self.x.clone().id → already cloned base
                if b.ends_with(".clone()") {
                    return format!("{b}.{f}.to_string()");
                }
                return format!("{b}.{f}.to_string()");
            }
        }
    }
    let param_tys = param_types_for(recv_type, method, ctx);
    args.iter()
        .enumerate()
        .map(|(i, a)| arg_to_rust(a, param_tys.get(i).map(|s| s.as_str()), ctx))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn is_json_type_name(ty: &str) -> bool {
    let t = ty.trim();
    t == "serde_json::Value" || t == "Json" || t.ends_with("::Value") && t.contains("serde_json")
}

pub fn expr_is_json(expr: &Expr, ctx: &GenCtx) -> bool {
    is_json_rooted_expr(expr, ctx)
        || infer_expr_type(expr, ctx)
            .as_deref()
            .is_some_and(is_json_type_name)
}

pub(super) fn list_elem_is_cloneable(base: &Expr, ctx: &GenCtx) -> bool {
    let ty = match base {
        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
        _ => infer_expr_type(base, ctx),
    };
    // Check the container type first — `element_type_of` peels `Box<dyn Trait>`
    // down to `Trait`, which would look cloneable.
    if let Some(ref t) = ty
        && t.contains("dyn ") {
            return false;
        }
    if let Some(elem) = element_type_of(base, ctx) {
        if elem.contains("dyn ") {
            return false;
        }
        if ctx.name_to_shape.get(&elem) == Some(&Shape::Trait) {
            return false;
        }
    }
    true
}

pub fn list_index_get_rust(base_rust: &str, idx_rust: &str, base: &Expr, ctx: &GenCtx) -> String {
    // Integer literals are already `usize`-compatible for `get`.
    let idx = if idx_rust.chars().all(|c| c.is_ascii_digit()) {
        idx_rust.to_string()
    } else if idx_rust.chars().all(|c| c.is_alphanumeric() || c == '_') {
        // Simple identifier — no parens needed for `as usize`.
        format!("{idx_rust} as usize")
    } else {
        format!("({idx_rust}) as usize")
    };
    let recv = base_rust
        .strip_suffix(".clone()")
        .unwrap_or(base_rust);
    if list_elem_is_cloneable(base, ctx) {
        format!("{recv}.get({idx}).cloned().ok_or({})?", ctx.error_model.not_found_path())
    } else {
        format!("{recv}.get({idx}).ok_or({})?", ctx.error_model.not_found_path())
    }
}

pub(super) fn list_first_rust(base_rust: &str, base: &Expr, ctx: &GenCtx) -> String {
    if list_elem_is_cloneable(base, ctx) {
        format!("{base_rust}.first().cloned().ok_or({})?", ctx.error_model.not_found_path())
    } else {
        format!("{base_rust}.first().ok_or({})?", ctx.error_model.not_found_path())
    }
}

/// Lower `local.field.nested` — struct fields stay `.field`; once a field is Json,
/// remaining segments become `["key"]` indexes.
pub(super) fn lower_dotted_local_path(target: &str, ctx: &GenCtx) -> String {
    let mut parts = target.split('.');
    let Some(first) = parts.next() else {
        return target.to_string();
    };
    let mut rust = first.to_string();
    let mut ty = ctx.local_type(first).map(|s| s.to_string());
    let mut json_mode = ty.as_deref().is_some_and(is_json_type_name);
    for seg in parts {
        let field = to_snake(seg);
        if json_mode {
            rust = format!("{rust}[\"{seg}\"]");
            continue;
        }
        if let Some(t) = ty.as_deref()
            && let Some(ft) = ctx
                .field_type(t, seg)
                .or_else(|| ctx.field_type(t, &field))
            {
                if is_json_type_name(ft) {
                    rust = format!("{rust}.{field}.clone()");
                    json_mode = true;
                    ty = Some(ft.to_string());
                    continue;
                }
                rust = format!("{rust}.{field}.clone()");
                ty = Some(ft.to_string());
                continue;
            }
        rust = format!("{rust}.{field}");
    }
    rust
}

/// Check if an expression is rooted in a Json / serde_json::Value.
/// Handles field access on typed structs whose field is Json (`context.stack.topic_arn`).
pub fn is_json_rooted_expr(expr: &Expr, ctx: &GenCtx) -> bool {
    match expr {
        Expr::Ident(name) => ctx.is_local(name) && ctx.local_type(name).is_some_and(is_json_type_name),
        Expr::FieldAccess(base, field) => {
            if is_json_rooted_expr(base, ctx) {
                return true;
            }
            if let Expr::Ident(name) = base.as_ref()
                && let Some(type_name) = ctx.local_type(name)
                    && ctx
                        .field_type(type_name, field)
                        .or_else(|| ctx.field_type(type_name, &to_snake(field)))
                        .is_some_and(is_json_type_name)
                    {
                        return true;
                    }
            false
        }
        _ => false,
    }
}

/// A local whose inferred type is a Copy scalar (int/bool/float) — no clone.
pub fn is_copy_local(name: &str, ctx: &GenCtx) -> bool {
    ctx.local_type(name).is_some_and(|t| {
        rust_ty_is_copy(t) || rust_ty_is_unit_enum(t, ctx)
    })
}

/// Locals that are already references / trait objects / slices — `.clone()` is a no-op.
pub fn is_ref_local(name: &str, ctx: &GenCtx) -> bool {
    let Some(ty) = ctx.local_type(name) else {
        return false;
    };
    ty.starts_with('&')
        || ty.contains("dyn ")
        || ty.starts_with('[')
        || ty.contains("&[")
}

fn is_hashmap_param(ty: &str) -> bool {
    ty.contains("HashMap") || ty.starts_with("Map<")
}

fn is_option_param(ty: &str) -> bool {
    ty.starts_with("Option<") || ty.starts_with("Opt<")
}

/// Peel `Arc<dyn Port + Send + Sync>` / `Option<Foo>` down to the type name
/// used as a `method_params` key.
fn peel_type_key(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.split("dyn ").nth(1) {
        return rest
            .split(['+', '<', '>', ','])
            .next()
            .unwrap_or(rest)
            .trim()
            .to_string();
    }
    s.split('<')
        .next()
        .unwrap_or(s)
        .trim()
        .trim_start_matches('&')
        .to_string()
}

pub fn param_types_for(recv: Option<&str>, method: &str, ctx: &GenCtx) -> Vec<String> {
    let bare = method.trim_end_matches(['!', '?']).to_string();
    let mut keys: Vec<(String, String)> = match recv {
        Some(r) => {
            let snake = to_snake(r);
            vec![
                (r.to_string(), method.to_string()),
                (r.to_string(), bare.clone()),
                (snake.clone(), method.to_string()),
                (snake, bare.clone()),
            ]
        }
        None => Vec::new(),
    };
    if let Some(r) = recv {
        // @dep field → trait (`sns_client` → `SnsClient`)
        if let Some((trait_name, _)) = ctx
            .dep_fields
            .iter()
            .find(|(_, f)| f.as_str() == r || f.as_str() == to_snake(r))
        {
            keys.push((trait_name.clone(), method.to_string()));
            keys.push((trait_name.clone(), bare.clone()));
            keys.push((to_snake(trait_name), method.to_string()));
            keys.push((to_snake(trait_name), bare.clone()));
        }
        // Adapter field rust type (`Arc<dyn SnsClient + …>`) → trait key
        if let Some(fty) = ctx
            .self_field_types
            .get(r)
            .or_else(|| ctx.self_field_types.get(&to_snake(r)))
        {
            let peeled = peel_type_key(fty);
            if !peeled.is_empty() {
                keys.push((peeled.clone(), method.to_string()));
                keys.push((peeled.clone(), bare.clone()));
                keys.push((to_snake(&peeled), method.to_string()));
                keys.push((to_snake(&peeled), bare.clone()));
            }
        }
    }
    for k in &keys {
        if let Some(p) = ctx.types.method_params.get(k) {
            return p.clone();
        }
    }
    // Prefer a unique Map-bearing signature for this method when the
    // receiver key missed (dep field vs stub fluent of the same name).
    let map_hits: Vec<&Vec<String>> = ctx
        .types.method_params
        .iter()
        .filter(|((_, m), tys)| {
            (*m == method || *m == bare)
                && tys
                    .iter()
                    .any(|t| is_hashmap_param(t))
        })
        .map(|(_, v)| v)
        .collect();
    if map_hits.len() == 1 {
        return map_hits[0].clone();
    }
    if let Some(first) = map_hits.first()
        && map_hits.iter().all(|h| *h == *first) {
            return (*first).clone();
        }
    let hits: Vec<&Vec<String>> = ctx
        .types.method_params
        .iter()
        .filter(|((_, m), _)| *m == method || *m == bare)
        .map(|(_, v)| v)
        .collect();
    if hits.len() == 1 {
        return hits[0].clone();
    }
    if let Some(first) = hits.first()
        && hits.iter().all(|h| *h == *first) {
            return (*first).clone();
        }
    Vec::new()
}

fn map_literal_to_hashmap(fields: &[(String, Expr)], ctx: &GenCtx) -> String {
    if fields.is_empty() {
        return "std::collections::HashMap::new()".to_string();
    }
    let inserts: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            let val = expr_to_rust(v, ctx);
            format!("__m.insert(\"{k}\".to_string(), {val})")
        })
        .collect();
    format!(
        "{{ let mut __m = std::collections::HashMap::new(); {}; __m }}",
        inserts.join("; ")
    )
}

fn arg_looks_optional(arg: &Expr, rust: &str, ctx: &GenCtx) -> bool {
    rust.starts_with("Some(")
        || rust == "None"
        || rust.starts_with("None::<")
        || match arg {
            Expr::Ident(n) => ctx
                .local_type(n)
                .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                .unwrap_or(false),
            _ => false,
        }
}

pub(super) fn arg_to_rust(arg: &Expr, param_ty: Option<&str>, ctx: &GenCtx) -> String {
    let mut rust = if let (Some(ty), Expr::StructLit(name, fields)) = (param_ty, arg) {
        if name.is_empty() && is_hashmap_param(ty) {
            map_literal_to_hashmap(fields, ctx)
        } else if is_json_type_name(ty) {
            // Struct lit passed to a Json-typed param → serialize as JSON message
            // with a "type" tag (the wire form for bus dispatch/invoke payloads).
            json_message(name, fields, ctx)
        } else {
            expr_to_rust(arg, ctx)
        }
    } else {
        match arg {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => {
                // State locals are serde_json::Value. Deserialize when the
                // target param expects a concrete type.
                if param_ty.is_some_and(|t| !is_json_type_name(t) && t != "()" && !t.is_empty()) {
                    let ty = param_ty.unwrap();
                    format!("serde_json::from_value::<{ty}>(state[\"{n}\"].clone()).map_err(|e| {}(e.to_string()))?", ctx.error_model.external_path())
                } else {
                    format!("state[\"{n}\"].clone()")
                }
            }
            Expr::Ident(n) if !ctx.routing.routing_ref.is_empty() && *n == ctx.routing.routing_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if is_ref_local(n, ctx) => n.clone(),
            Expr::Ident(n) if ctx.ownership.borrow_fields.contains(n.as_str()) => format!("&self.{n}"),
            Expr::Ident(n) if ctx.is_local(n) && should_clone_ident(n, ctx) => {
                format!("{n}.clone()")
            }
            Expr::StringLit(s)
                if param_ty.is_some_and(|t| {
                    rust_ty_is_stringish(t) && !t.starts_with('&') && t != "str"
                }) =>
            {
                rust_string_lit_owned(s)
            }
            Expr::FieldAccess(base, field)
                if ctx.ownership.borrow_fields.contains(field.as_str()) && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                format!("&self.{field}")
            }
            _ => expr_to_rust(arg, ctx),
        }
    };
    if let Some(ty) = param_ty {
        if is_option_param(ty) && !arg_looks_optional(arg, &rust, ctx) {
            rust = format!("Some({rust})");
        }
        // State-local field access produces serde_json::Value — deserialize when
        // the target param expects a concrete type.
        if !is_json_type_name(ty) && ty != "()" && !ty.is_empty()
            && rust.starts_with("state[\"") && !rust.contains("from_value")
        {
            rust = format!("serde_json::from_value::<{ty}>({rust}.clone()).map_err(|e| {}(e.to_string()))?", ctx.error_model.external_path());
        }
    }
    rust
}

