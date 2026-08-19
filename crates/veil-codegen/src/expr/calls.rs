use std::collections::HashSet;
use veil_ir::ast::*;
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::*;

/// Determine the call suffix for a method invoked on a chained receiver.
///
/// - Fluent `.send()` / `.send_with()` are async + Result → `.await?`
/// - Stub methods marked async+fallible (BoxFuture / executor param) → `.await.map_err…?`
/// - Other stub methods marked `Res!` are sync Result → `map_err…?`
/// - Trait methods (ports) are async_trait + Result → `.await?`
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
            } else if ctx.stub_type_crate.contains_key(name) {
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
            || ctx.stub_type_crate.contains_key(bare.as_str())
            || ctx.stub_type_crate.contains_key(ty.as_str())
        {
            if method == "send"
                || method == "send_with"
                || ctx.async_fallible_methods.contains(method)
            {
                // send!() → unwrap Result; bare send() keeps Result so .is_ok()/.is_err() work.
                if has_bang {
                    return ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string();
                } else {
                    return ".await".to_string();
                }
            }
            if ctx.fallible_methods.contains(method) {
                let suffix = if should_own_str_result(ctx, Some(ty.as_str()), method) {
                    map_err_domain_own_str()
                } else {
                    map_err_domain()
                };
                // Only apply fallible suffix if this specific type has the method as fallible.
                // Use type_fallible_methods: (Type, method) set for precision.
                if ctx.type_fallible_methods.contains(&(bare.clone(), method.to_string())) {
                    return suffix.to_string();
                }
                // If the method is ONLY fallible (not ambiguous), apply it.
                if !ctx.non_fallible_methods.contains(method) {
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
                    .type_fallible_methods
                    .contains(&(bare.clone(), method.to_string()))
                || ctx
                    .type_fallible_methods
                    .contains(&(ty.clone(), method.to_string()));
            return if fallible {
                ".await?".to_string()
            } else {
                ".await".to_string()
            };
        }
    }

    // Fluent SDK send / async fallible stubs (untyped receivers).
    if method == "send"
        || method == "send_with"
        || ctx.async_fallible_methods.contains(method)
    {
        // send!() → unwrap; bare send() keeps Result so .is_ok()/.is_err() work.
        if has_bang {
            return ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string();
        } else {
            return ".await".to_string();
        }
    }
    // Untyped receiver: method name appears on a port trait → async_trait.
    // If a stub/struct also has the same method name (e.g. `delete`), do not
    // force await — that would break reqwest Client.delete. List elements of
    // trait objects are handled via Index + peel above (SagaStep.action).
    let is_trait_method = ctx.method_returns.keys().any(|(ty, m)| {
        m == method && ctx.name_to_shape.get(ty) == Some(&Shape::Trait)
    });
    let is_stub_or_struct_method = ctx.method_returns.keys().any(|(ty, m)| {
        m == method
            && (ctx.stub_type_crate.contains_key(ty)
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
    let is_ambiguous = ctx.non_fallible_methods.contains(method);
    if ctx.fallible_methods.contains(method) && !recv_is_chain && !is_ambiguous {
        let own = should_own_str_result(ctx, recv_type_name.as_deref(), method);
        return if own {
            map_err_domain_own_str()
        } else {
            map_err_domain()
        }
        .to_string();
    }
    // Terminal builder `.build!()` is fallible (BuildError) even on chains.
    if has_bang && method == "build" {
        return map_err_domain().to_string();
    }
    // Fallback: if the method has a bang (!) and nothing else matched,
    // treat it as an async fallible call (common for SDK methods like collect!,
    // execute!, etc. on receivers whose type isn't in our stub system).
    if has_bang {
        return ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string();
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
    } else if let Some(rest) = t.strip_prefix("dyn ") {
        rest
    } else {
        return None;
    };
    let name = after_dyn.split(|c: char| c == '+' || c == '>' || c == ' ').next()?;
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
pub fn json_message(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> String {
    let mut parts = vec![format!("\"type\": \"{}\"", name)];
    for (k, v) in fields {
        parts.push(format!("\"{}\": {}", k, to_json_arg(v, ctx)));
    }
    format!("serde_json::json!({{ {} }})", parts.join(", "))
}

/// Build a JSON envelope for a cross-boundary call routed through a routing
/// trait: `{ "target": T, "method": m, "args": [ ... ] }`. Positional args are
/// rendered as JSON values so the receiving side can decode them.
pub fn json_envelope(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let arg_vals = args.iter().map(|a| to_json_arg(a, ctx)).collect::<Vec<_>>().join(", ");
    format!(
        "serde_json::json!({{ \"target\": \"{}\", \"method\": \"{}\", \"args\": [{}] }})",
        target, method, arg_vals
    )
}

/// Render call args, cloning value-bearing locals/state so passing them into a
/// by-value parameter doesn't move them out of the caller. Skips the routing
/// reference and Copy scalars (which don't move).
pub fn clone_args(args: &[Expr], ctx: &GenCtx) -> String {
    args.iter()
        .map(|a| match a {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => format!("state[\"{}\"].clone()", n),
            // The routing reference and Copy scalars are passed as-is.
            Expr::Ident(n) if !ctx.routing_ref.is_empty() && *n == ctx.routing_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if is_ref_local(n, ctx) => n.clone(),
            // sqlx Executor is implemented for `&Pool`, not `Pool`.
            Expr::Ident(n) if n == "pool" => "&self.pool".to_string(),
            Expr::Ident(n) if ctx.is_local(n) && should_clone_ident(n, ctx) => {
                format!("{n}.clone()")
            }
            Expr::FieldAccess(base, field)
                if field == "pool"
                    && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                "&self.pool".to_string()
            }
            _ => expr_to_rust(a, ctx),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Like `clone_args` but applies method-specific argument shaping (e.g. reqwest
/// `basic_auth` takes `Option` password).
pub fn clone_args_for_method(method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    clone_args_for_typed_method(None, method, args, ctx)
}

/// Clone/ref args for a method call, with optional receiver type for ref-param resolution.
pub fn clone_args_for_typed_method(recv_type: Option<&str>, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let method = method.trim_end_matches(['!', '?']);

    // Check ref_params for this specific (type, method) combination.
    // If found, emit &arg for ref positions instead of arg.clone().
    if let Some(type_name) = recv_type {
        if let Some(ref_flags) = ctx.ref_params.get(&(type_name.to_string(), method.to_string())) {
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
    if method == "unwrap_or" && args.len() == 1 {
        if let Expr::StringLit(s) = &args[0] {
            return format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        }
    }
    if method == "basic_auth" && args.len() >= 2 {
        let user = clone_args(&args[..1], ctx);
        let pass = expr_to_rust(&args[1], ctx);
        // reqwest: basic_auth(user, Option<password>)
        return format!("{user}, Some({pass})");
    }
    // sqlx bind: Uuid needs the `uuid` feature; bind as text to stay feature-light.
    if method == "bind" && args.len() == 1 {
        if let Expr::Ident(n) = &args[0] {
            if ctx.local_type(n) == Some("Uuid")
                || n == "id"
                || n.ends_with("_id")
                || n.ends_with("Id")
            {
                return format!("{n}.to_string()");
            }
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

pub fn list_elem_is_cloneable(base: &Expr, ctx: &GenCtx) -> bool {
    let ty = match base {
        Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
        _ => infer_expr_type(base, ctx),
    };
    // Check the container type first — `element_type_of` peels `Box<dyn Trait>`
    // down to `Trait`, which would look cloneable.
    if let Some(ref t) = ty {
        if t.contains("dyn ") {
            return false;
        }
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
    } else {
        format!("({idx_rust}) as usize")
    };
    let recv = base_rust
        .strip_suffix(".clone()")
        .unwrap_or(base_rust);
    if list_elem_is_cloneable(base, ctx) {
        format!("{recv}.get({idx}).cloned().ok_or(DomainError::NotFound)?")
    } else {
        format!("{recv}.get({idx}).ok_or(DomainError::NotFound)?")
    }
}

pub fn list_first_rust(base_rust: &str, base: &Expr, ctx: &GenCtx) -> String {
    if list_elem_is_cloneable(base, ctx) {
        format!("{base_rust}.first().cloned().ok_or(DomainError::NotFound)?")
    } else {
        format!("{base_rust}.first().ok_or(DomainError::NotFound)?")
    }
}

/// Lower `local.field.nested` — struct fields stay `.field`; once a field is Json,
/// remaining segments become `["key"]` indexes.
pub fn lower_dotted_local_path(target: &str, ctx: &GenCtx) -> String {
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
        if let Some(t) = ty.as_deref() {
            if let Some(ft) = ctx
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
            if let Expr::Ident(name) = base.as_ref() {
                if let Some(type_name) = ctx.local_type(name) {
                    if ctx
                        .field_type(type_name, field)
                        .or_else(|| ctx.field_type(type_name, &to_snake(field)))
                        .is_some_and(is_json_type_name)
                    {
                        return true;
                    }
                }
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

pub fn is_hashmap_param(ty: &str) -> bool {
    ty.contains("HashMap") || ty.starts_with("Map<")
}

pub fn is_option_param(ty: &str) -> bool {
    ty.starts_with("Option<") || ty.starts_with("Opt<")
}

/// Peel `Arc<dyn Port + Send + Sync>` / `Option<Foo>` down to the type name
/// used as a `method_params` key.
pub fn peel_type_key(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.split("dyn ").nth(1) {
        return rest
            .split(|c: char| c == '+' || c == '<' || c == '>' || c == ',')
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
        if let Some(p) = ctx.method_params.get(k) {
            return p.clone();
        }
    }
    // Prefer a unique Map-bearing signature for this method when the
    // receiver key missed (dep field vs stub fluent of the same name).
    let map_hits: Vec<&Vec<String>> = ctx
        .method_params
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
    if let Some(first) = map_hits.first() {
        if map_hits.iter().all(|h| *h == *first) {
            return (*first).clone();
        }
    }
    let hits: Vec<&Vec<String>> = ctx
        .method_params
        .iter()
        .filter(|((_, m), _)| *m == method || *m == bare)
        .map(|(_, v)| v)
        .collect();
    if hits.len() == 1 {
        return hits[0].clone();
    }
    if let Some(first) = hits.first() {
        if hits.iter().all(|h| *h == *first) {
            return (*first).clone();
        }
    }
    Vec::new()
}

pub fn map_literal_to_hashmap(fields: &[(String, Expr)], ctx: &GenCtx) -> String {
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

pub fn arg_looks_optional(arg: &Expr, rust: &str, ctx: &GenCtx) -> bool {
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

pub fn arg_to_rust(arg: &Expr, param_ty: Option<&str>, ctx: &GenCtx) -> String {
    let mut rust = if let (Some(ty), Expr::StructLit(name, fields)) = (param_ty, arg) {
        if name.is_empty() && is_hashmap_param(ty) {
            map_literal_to_hashmap(fields, ctx)
        } else {
            expr_to_rust(arg, ctx)
        }
    } else {
        match arg {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => {
                format!("state[\"{n}\"].clone()")
            }
            Expr::Ident(n) if !ctx.routing_ref.is_empty() && *n == ctx.routing_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if is_ref_local(n, ctx) => n.clone(),
            Expr::Ident(n) if n == "pool" => "&self.pool".to_string(),
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
                if field == "pool" && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                "&self.pool".to_string()
            }
            _ => expr_to_rust(arg, ctx),
        }
    };
    if let Some(ty) = param_ty {
        if is_option_param(ty) && !arg_looks_optional(arg, &rust, ctx) {
            rust = format!("Some({rust})");
        }
    }
    rust
}

/// Receiver name used to look up port/stub param types.
/// Ident and `self.field` / `deps.field` last segments all count.
pub fn call_recv_lookup_name(call: &CallExpr) -> Option<String> {
    if !call.target.is_empty() {
        return Some(call.target.clone());
    }
    match call.receiver.as_deref() {
        Some(Expr::Ident(n)) => Some(n.clone()),
        Some(Expr::FieldAccess(_, field)) => Some(field.clone()),
        Some(Expr::Call(inner)) => call_recv_lookup_name(inner),
        _ => None,
    }
}

pub fn call_args_to_rust(call: &CallExpr, ctx: &GenCtx) -> String {
    let recv_owned = call_recv_lookup_name(call);
    let recv = recv_owned.as_deref();
    let tys = param_types_for(recv, &call.method, ctx);
    call.args
        .iter()
        .enumerate()
        .map(|(i, a)| arg_to_rust(a, tys.get(i).map(|s| s.as_str()), ctx))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Translate a Call expression with shape-aware name resolution.
pub fn translate_call(call: &CallExpr, ctx: &GenCtx) -> String {
    let args_str = call_args_to_rust(call, ctx);

    // Built-in List methods: `.get(i)` → indexing (`[i as usize]`), `.len()` →
    // `.len() as i64`. The receiver/target is the list expression.
    // Only treat `.get` as slice index when the arg is index-like (int lit /
    // int-typed local) OR the receiver is known to be a Vec/slice (saga
    // coordinators: `steps: List<SagaStep>` → `&[Box<dyn …>]`). `client.get(url)`
    // and `map.get(key)` must stay method calls.
    let list_base = if let Some(recv) = &call.receiver {
        Some(expr_to_rust(recv, ctx))
    } else if !call.target.is_empty()
        && !ctx.is_trait_target(&call.target)
        && (call.method == "get"
            || call.method == "len"
            || call.method == "first"
            || call.method == "first!")
        && ctx.local_type(&call.target) != Some("serde_json::Value")
    {
        Some(call.target.clone())
    } else {
        None
    };
    if let Some(base) = list_base {
        if call.method == "get" && call.args.len() == 1 {
            // String args (HashMap key lookup) stay as .get("key") — fall through.
            let is_string_arg = matches!(&call.args[0], Expr::StringLit(_));
            let arg_is_index_like = match &call.args[0] {
                Expr::IntLit(_) => true,
                Expr::Ident(n) => matches!(
                    ctx.local_type(n),
                    Some("i64")
                        | Some("i32")
                        | Some("u64")
                        | Some("u32")
                        | Some("usize")
                        | Some("isize")
                ) || is_copy_local(n, ctx),
                _ => false,
            };
            // Receiver known as Vec / slice → index even if local types lag
            // (e.g. `mut i = upto` before Ident inference is wired).
            let base_is_list = if !call.target.is_empty() {
                ctx.local_type(&call.target)
                    .map(|t| {
                        t.starts_with("Vec<")
                            || t.starts_with("&[")
                            || t.starts_with("&mut [")
                    })
                    .unwrap_or(false)
            } else if let Some(recv) = &call.receiver {
                if let Expr::Ident(n) = recv.as_ref() {
                    ctx.local_type(n)
                        .map(|t| {
                            t.starts_with("Vec<")
                                || t.starts_with("&[")
                                || t.starts_with("&mut [")
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };
            if !is_string_arg && (arg_is_index_like || base_is_list) {
                let idx = expr_to_rust(&call.args[0], ctx);
                let fallback = Expr::Ident(call.target.clone());
                let recv_expr = call.receiver.as_deref().unwrap_or(&fallback);
                return list_index_get_rust(&base, &format!("({idx})"), recv_expr, ctx);
            }
        }
        if (call.method == "first" || call.method == "first!") && call.args.is_empty() {
            let fallback = Expr::Ident(call.target.clone());
            let recv_expr = call.receiver.as_deref().unwrap_or(&fallback);
            return list_first_rust(&base, recv_expr, ctx);
        }
        if call.method == "len" && call.args.is_empty() {
            return format!("({}.len() as i64)", base);
        }
    }

    // Chained method call: `<receiver>.method(args)` (e.g. `.collect()` in
    // `items.map(f).collect()`). The receiver carries the left side of the chain.
    if let Some(recv) = &call.receiver {
        let mut recv_str = expr_to_rust(recv, ctx);

        // Auto-unwrap Option<T> locals for method calls: when the receiver is a
        // local typed as Option<T> and the method is NOT an Option method, unwrap
        // first so that domain-type methods can be called directly.
        if let Expr::Ident(name) = recv.as_ref() {
            if let Some(ty) = ctx.local_type(name) {
                if ty.starts_with("Option<") {
                    let bare_method = call.method.trim_end_matches(['!', '?']);
                    let option_methods = [
                        "is_some", "is_none", "unwrap", "unwrap_or", "unwrap_or_else",
                        "unwrap_or_default", "map", "and_then", "or_else", "ok_or",
                        "ok_or_else", "as_ref", "as_mut", "take", "replace", "clone",
                        "expect", "filter", "flatten", "zip",
                    ];
                    if !option_methods.contains(&bare_method) {
                        recv_str = format!(
                            "{}.clone().ok_or(DomainError::NotFound)?",
                            recv_str
                        );
                    } else {
                        // Consuming Option methods (and_then, map, unwrap, filter, etc.)
                        // move self — clone to allow reuse of the local variable.
                        let non_consuming = ["is_some", "is_none", "as_ref", "as_mut", "clone"];
                        if !non_consuming.contains(&bare_method) {
                            recv_str = format!("{}.clone()", recv_str);
                        }
                    }
                }
            }
        }

        let bare_conv = call.method.trim_end_matches(['!', '?']);
        // Json / Value: as_str / as_s / to_string extract a string, never bytes.
        if expr_is_json(recv, ctx)
            && call.args.is_empty()
            && matches!(bare_conv, "as_str" | "as_s" | "to_str" | "to_string")
        {
            return format!("{recv_str}.as_str().map(|s| s.to_string())");
        }
        if matches!(bare_conv, "to_str" | "as_str" | "to_string") && call.args.is_empty() {
            let recv_is_string = matches!(recv.as_ref(), Expr::Ident(n) if ctx.local_type(n) == Some("String"));
            if !recv_is_string {
                return format!("String::from_utf8_lossy({recv_str}.as_ref()).to_string()");
            }
        }
        // Stub/`Str` as_ref is a bytes view in Rust (`&[u8]`). Honor VEIL Str.
        if matches!(bare_conv, "as_ref") && call.args.is_empty() && should_decode_as_ref_to_str(recv, ctx)
        {
            return format!("String::from_utf8_lossy({recv_str}.as_ref()).to_string()");
        }
        if matches!(bare_conv, "as_bytes" | "to_bytes" | "into_bytes") && call.args.is_empty() {
            return format!("{recv_str}.as_ref().to_vec()");
        }

        // Phase 2, Issue 1: Redundant .unwrap() elision.
        // When the receiver is itself a Call whose codegen already unwraps the value
        // (as_s, as_n → .map_err()?  /  get("lit") → .ok_or_else()?), a following
        // .unwrap() is redundant and would error (String/&AV has no .unwrap()).
        if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
            if let Expr::Call(inner_call) = recv.as_ref() {
                let inner_bare = inner_call.method.trim_end_matches(['!', '?']);
                // as_s / as_n already produce a fully-unwrapped String
                if inner_bare == "as_s" || inner_bare == "as_n" || (inner_bare.starts_with("as_") && inner_bare != "as_str") {
                    return recv_str;
                }
                // .get("key") already produces .ok_or_else(...)? — value is extracted
                if inner_bare == "get" && inner_call.args.len() == 1 {
                    if matches!(&inner_call.args[0], Expr::StringLit(_)) {
                        return recv_str;
                    }
                }
            }
            // Also catch: recv_str ends with `)?` or `.unwrap()` — redundant unwrap
            let trimmed = recv_str.trim();
            if trimmed.ends_with(")?") || trimmed.ends_with(".unwrap()") {
                return recv_str;
            }
        }

        // Map/HashMap .get("lit") → &str key (not String) on any receiver chain.
        // Match local-target lowering: unwrap Option for immediate .as_s() chains.
        if call.method == "get" && call.args.len() == 1 {
            if let Expr::StringLit(key) = &call.args[0] {
                // Issue 6: never panic on missing map keys in adapter bodies.
                return format!(
                    "{}.get(\"{}\").ok_or_else(|| DomainError::External(\"missing {}\".into()))?",
                    recv_str, key, key
                );
            }
        }
        // serde_json::Value::as_str → Option<String> (owned) for assigns/unwrap.
        if call.method == "as_str" && call.args.is_empty() {
            return format!("{}.as_str().map(|s| s.to_string())", recv_str);
        }
        // Stub `Res!<Str>` getters (`as_s` / typed as_*): Rust is
        // usually `Result<&str, E>` with E: Debug + !Display. Own a String.
        // `as_n` is the numeric extractor — parse to i64.
        if call.args.is_empty() && method_bare(&call.method) == "as_n" {
            return format!(
                "{recv_str}.as_n(){}{}",
                map_err_domain_own_str(),
                parse_i64_suffix()
            );
        }
        if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
            return format!("{recv_str}{}", parse_i64_suffix());
        }
        if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
            return format!("serde_json::from_str::<serde_json::Value>(&{recv_str})?");
        }
        if call.args.is_empty() {
            let recv_ty = infer_expr_type(recv, ctx);
            if should_own_str_result(ctx, recv_ty.as_deref(), &call.method) {
                let m = method_bare(&call.method);
                return format!("{recv_str}.{m}(){}", map_err_domain_own_str());
            }
        }
        // A trait method invoked on a chained receiver is async + fallible.
        let suffix = receiver_call_suffix(recv, &call.method, ctx);
        let m = rust_method_name(&call.method);
        let bare_m = call.method.trim_end_matches(['!', '?']);
        // .trim() on a String returns &str — own it for return/assign contexts.
        if bare_m == "trim" && call.args.is_empty() {
            return format!("{}.trim().to_string()", recv_str);
        }
        if (bare_m == "unwrap_or" || bare_m == "unwrap_or_else") && call.args.len() == 1 {
            if let Expr::StringLit(s) = &call.args[0] {
                let lit = s.replace('\\', "\\\\").replace('"', "\\\"");
                // Option<String> (after .map(|s| s.to_string()) / .clone() / .as_str().map(...)
                // / .and_then(|c| c.field)):
                // need owned default. Option<&str> (AWS getters): bare &str.
                // VEIL Str always maps to Rust String, so .and_then() / .map()
                // chains on domain types produce Option<String>. Only explicit
                // AWS getter patterns (handled via as_str() → map) stay &str.
                let owned_default = recv_str.contains("to_string()")
                    || recv_str.contains("as_str().map")
                    || recv_str.ends_with(".clone()")
                    || recv_str.contains(".and_then(")
                    || recv_str.contains(".map(");
                if owned_default {
                    return format!("{}.{m}(\"{lit}\".to_string()){suffix}", recv_str);
                }
                return format!("{}.{m}(\"{lit}\"){suffix}", recv_str);
            }
        }
        // Phase 2, Issue 3: S3 .body() takes ByteStream, not Vec<u8>.
        // Append .into() when the arg is a local typed as Vec<u8>/Bytes.
        if bare_m == "body" && call.args.len() == 1 {
            if let Expr::Ident(name) = &call.args[0] {
                let ty = ctx.local_type(name).unwrap_or("");
                if ty == "Vec<u8>" || ty.contains("Bytes") || ty.contains("Vec<u8>") {
                    return format!("{}.body({}.into()){}", recv_str, name, suffix);
                }
            }
        }
        // Phase 2, Issue 4: DDB .limit() takes i32, VEIL Int is i64.
        // Insert `as i32` cast for the argument.
        if bare_m == "limit" && call.args.len() == 1 {
            let arg = expr_to_rust(&call.args[0], ctx);
            // Only cast if the arg could be i64 (ident or int lit, not already cast)
            if !arg.contains("as i32") {
                return format!("{}.limit(({}) as i32){}", recv_str, arg, suffix);
            }
        }
        // Look up param types by receiver *name* (port/dep field), not the
        // inferred Rust local type. `local_type("sns_client")` is None; the
        // port is registered as `(sns_client, publish)`. Falling back to
        // "any method named publish" collides with stub fluent `publish()`.
        let recv_lookup = match recv.as_ref() {
            Expr::Ident(name) => Some(name.as_str()),
            Expr::FieldAccess(_, field) => Some(field.as_str()),
            _ => None,
        };
        return format!(
            "{}.{}({}){}",
            recv_str,
            m,
            clone_args_for_typed_method(recv_lookup, &call.method, &call.args, ctx),
            suffix
        );
    }

    // Trait-shaped target → deps.<field>.method(args).await?
    // Field name comes from dep_fields (shared with harness / Deps struct).
    if ctx.is_trait_target(&call.target) {
        let dep_name = ctx.deps_field_for(&call.target);
        let method = if call.method.is_empty() { "call" } else { &call.method };
        // Desugared routing-port calls (layer statement sugar) carry a StructLit
        // payload; build a JSON message tagged with its type.
        let final_args = if call.sugar.is_some() {
            match call.args.first() {
                Some(Expr::StructLit(name, fields)) => json_message(name, fields, ctx),
                Some(Expr::Ident(evt)) => format!("serde_json::json!({{ \"type\": \"{}\" }})", evt),
                _ => json_envelope(&call.target, method, &call.args, ctx),
            }
        } else {
            // Direct routing-trait call — clone args to avoid move issues.
            // Auto-unwrap Option<T> only when the port param is T (not Option<T>).
            let method_key = method.trim_end_matches(['!', '?']);
            let param_tys = param_types_for(Some(call.target.as_str()), method_key, ctx);
            call.args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let expected = param_tys.get(i).map(|s| s.as_str());
                    let s = arg_to_rust(a, expected, ctx);
                    match a {
                        Expr::Ident(name) if ctx.local_type(name) == Some("serde_json::Value") => {
                            format!("{}.clone()", name)
                        }
                        Expr::Ident(name)
                            if ctx
                                .local_type(name)
                                .map(|t| t.starts_with("Option<"))
                                .unwrap_or(false) =>
                        {
                            let expects_opt = expected
                                .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                                .unwrap_or(false);
                            if expects_opt {
                                format!("{}.clone()", name)
                            } else {
                                format!("{}.clone().ok_or(DomainError::NotFound)?", name)
                            }
                        }
                        _ => s,
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        // Routing traits use `routing_ref` (`deps.<trait>` in a flow, injected
        // param inside a step impl); other trait deps come from `deps`.
        if ctx.routing_traits.contains(&call.target) {
            let rref = if ctx.routing_ref.is_empty() {
                format!("deps.{}", dep_name)
            } else {
                ctx.routing_ref.clone()
            };
            let bare = to_snake(method);
            let call_expr = format!("{}.{}({}).await?", rref, bare, final_args);
            // Typed bus decode: when sugar carries a message type with a known
            // domain return, deserialize instead of leaving serde_json::Value.
            if matches!(bare.as_str(), "invoke" | "request") {
                if let Some(msg) = bus_message_name_from_args(&call.args) {
                    if let Some(ret) = ctx.bus_returns.get(&msg) {
                        // Only decode types this crate can name (local domain /
                        // primitives). Cross-context domain types (e.g. tools
                        // invoking storage CreateRepo → Repo) stay as Value.
                        if bus_return_type_in_scope(ctx, ret) {
                            return format!(
                                "serde_json::from_value::<{ret}>({call_expr})\
                                 .map_err(|e| DomainError::External(e.to_string()))?"
                            );
                        }
                    }
                }
            }
            return call_expr;
        }
        // Bang on ports means fallible/async (Result), not "unwrap Opt".
        // Keep Option so callers can use .is_some() / .is_none() / .unwrap().
        let method_key = method.trim_end_matches(['!', '?']);
        // Port methods that return non-Result types (e.g. Bool, plain Str)
        // should NOT have `?` appended — they are async but not fallible.
        // However, bang (`!`) on the call site always means fallible — the
        // method wraps its return in Result even if the inner type is `()`.
        let has_bang = method.ends_with('!');
        let ret_type = ctx.return_type_of(&call.target, method)
            .or_else(|| {
                // Also try the PascalCase trait name via dep_fields reverse lookup
                ctx.dep_fields.iter()
                    .find(|(_, v)| *v == &call.target)
                    .and_then(|(trait_name, _)| ctx.return_type_of(trait_name, method))
            });
        let is_fallible = if has_bang {
            true // Bang always means Result-wrapped (fallible)
        } else {
            match ret_type {
                Some("bool") | Some("Bool") | Some("i64") | Some("f64")
                | Some("String") | Some("()") | Some("") => false,
                Some(t) if t.starts_with("Option<") || t.starts_with("Opt<") => false,
                _ => true,
            }
        };
        let suffix = if is_fallible { ".await?" } else { ".await" };
        return format!(
            "deps.{}.{}({}){}",
            dep_name,
            to_snake(method_key),
            final_args,
            suffix,
        );
    }

    // Envelope routing: cross-boundary calls (struct construction, foreign
    // methods, etc.) go through the primary routing trait with a typed JSON
    // envelope — the caller crate cannot see the target's concrete types.
    // Language primitives (Json, Map, Dt, etc.) are excluded — they resolve locally.
    // Locals with known types (esp. serde_json::Value) are also excluded —
    // they are calling methods on data, not cross-boundary invocations.
    let is_lang_target = matches!(
        call.target.as_str(),
        "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "Int" | "UUID"
    );
    let is_typed_local = ctx.is_local(&call.target) && ctx.local_type(&call.target).is_some();
    if ctx.envelope_routing && !is_lang_target && !is_typed_local
        && !ctx.stub_pkg_crate.contains_key(&call.target)
        && (ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty()) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        let rref = if ctx.routing_ref.is_empty() {
            "deps".to_string() // should not happen when envelope_routing is set
        } else {
            ctx.routing_ref.clone()
        };
        return format!(
            "{}.invoke({}).await?",
            rref,
            json_envelope(&call.target, method, &call.args, ctx)
        );
    }

    // Language primitives win over stub names (e.g. gix.stub `struct Id`,
    // axum.stub `Json` — IR Json is not axum::Json).
    if !call.method.is_empty() {
        let lang = match (call.target.as_str(), call.method.as_str()) {
            ("Id", "new") | ("Id", "new_v4") | ("UUID", "new") | ("UUID", "new_v4") | ("Uuid", "new")
                => Some("Uuid::new_v4()".to_string()),
            ("Dt", "now") => Some("Utc::now()".to_string()),
            ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
                => Some(now_iso8601_rust()),
            ("Int", "now_unix") | ("Int", "now") => Some("Utc::now().timestamp()".to_string()),
            ("Json", "parse") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!(
                    "serde_json::from_str::<serde_json::Value>(&{})?",
                    arg
                ))
            }
            ("Json", "stringify") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::to_string(&{})?", arg))
            }
            ("Json", "null") => Some("serde_json::Value::Null".to_string()),
            ("Json", "object") => Some("serde_json::Value::Object(serde_json::Map::new())".to_string()),
            ("Json", "array") => Some("serde_json::Value::Array(Vec::new())".to_string()),
            _ => None,
        };
        if let Some(result) = lang {
            return result;
        }
    }

    // Built-in type-level method translations.
    // These are VEIL's short type names with associated methods that map
    // to Rust idioms. Language primitives always win over stub types that
    // happen to share a name (e.g. sqlx's `Map` must not steal `Map.new()`).
    if !call.method.is_empty() {
        let lang_leaf = lang_type_leaf(&call.target);
        let is_lang_primitive = matches!(
            lang_leaf,
            "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "Int"
                | "Process"
                | "Blob" | "Bytes"
        );
        if is_lang_primitive || !ctx.is_struct_target(&call.target) {
            let method_key = call.method.trim_end_matches(['!', '?']);
            let translated = match (lang_leaf, method_key) {
                ("Dt", "now") => Some("Utc::now()".to_string()),
                ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
                    if call.args.is_empty() =>
                {
                    Some(now_iso8601_rust())
                }
                ("Int", "now_unix") | ("Int", "now") if call.args.is_empty() => {
                    Some("Utc::now().timestamp()".to_string())
                }
                ("Uuid", "new_v4") | ("Id", "new_v4") => Some("Uuid::new_v4()".to_string()),
                ("Map", "new") => Some("HashMap::new()".to_string()),
                ("List", "new") => Some("Vec::new()".to_string()),
                ("Opt", "empty") | ("Opt", "none") => Some("None".to_string()),
                ("Opt", "some") | ("Opt", "of") if call.args.len() == 1 => {
                    Some(format!("Some({})", expr_to_rust(&call.args[0], ctx)))
                }
                ("Env", "get_or") if call.args.len() == 2 => {
                    let var = expr_to_rust(&call.args[0], ctx);
                    // StringLit already becomes `"…".to_string()` — do not double.
                    let default = match &call.args[1] {
                        Expr::StringLit(s) => format!("\"{}\".to_string()", s),
                        other => {
                            let d = expr_to_rust(other, ctx);
                            if d.ends_with(".to_string()") {
                                d
                            } else {
                                format!("{d}.to_string()")
                            }
                        }
                    };
                    Some(format!(
                        "std::env::var({}).unwrap_or_else(|_| {})",
                        var, default
                    ))
                }
                ("Env", "get_opt") if call.args.len() == 1 => {
                    let var = expr_to_rust(&call.args[0], ctx);
                    Some(format!("std::env::var({}).ok()", var))
                }
                ("Json", "parse") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "serde_json::from_str::<serde_json::Value>(&{})?",
                        arg
                    ))
                }
                ("Json", "stringify") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("serde_json::to_string(&{})?", arg))
                }
                ("Json", "null") => Some("serde_json::Value::Null".to_string()),
                ("Json", "object") => Some("serde_json::Value::Object(serde_json::Map::new())".to_string()),
                ("Json", "array") => Some("serde_json::Value::Array(Vec::new())".to_string()),
                ("Str", "from_bytes") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("String::from_utf8({})?", arg))
                }
                // Host process execution (language primitive — not a product facade).
                // Always returns a detail String; non-zero exit → "prog failed: …" (no hard Err)
                // so provision/job steps can record failure without 502. Spawn I/O errors still Err.
                ("Process", "run") if call.args.len() == 3 => {
                    let prog = expr_to_rust(&call.args[0], ctx);
                    let args = expr_to_rust(&call.args[1], ctx);
                    let cwd = expr_to_rust(&call.args[2], ctx);
                    let hard = call.method.ends_with('!');
                    if hard {
                        Some(format!(
                            "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); let __out = std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output().map_err(|e| DomainError::External(format!(\"{{e:?}}\")))?; if !__out.status.success() {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(2000).collect::<String>().chars().rev().collect(); return Err(DomainError::External(format!(\"{{}} failed: {{}}\", __prog, __tail))); }} format!(\"{{}} ok\", __prog) }}"
                        ))
                    } else {
                        Some(format!(
                            "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); match std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output() {{ Ok(__out) => {{ if __out.status.success() {{ format!(\"{{}} ok: {{}}\", __prog, String::from_utf8_lossy(&__out.stdout).chars().take(400).collect::<String>()) }} else {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(1200).collect::<String>().chars().rev().collect(); format!(\"{{}} failed: {{}}\", __prog, __tail) }} }}, Err(e) => format!(\"{{}} spawn failed: {{e}}\", __prog) }} }}"
                        ))
                    }
                }
                // Binary payload: use the loaded stub type path, never a bare Vec<u8>.
                ("Blob", "new") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "{}::new({})",
                        stub_ctor_path(ctx, &call.target),
                        bytes_from_str_expr(&arg)
                    ))
                }
                ("Bytes", "from_str") | ("Bytes", "new") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(bytes_from_str_expr(&arg))
                }
                ("Str", "from_bytes") | ("Str", "from_utf8") if call.args.len() == 1 => {
                    let arg = expr_to_rust(&call.args[0], ctx);
                    Some(format!("String::from_utf8_lossy(&{arg}).to_string()"))
                }
                ("Blob", "to_str") | ("Blob", "as_str") | ("Blob", "to_string")
                    if call.args.is_empty() =>
                {
                    None // handled as receiver method below
                }
                ("Blob", "from_hex") if call.args.len() == 1 => {
                    let hex_expr = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "{}::new({})",
                        stub_ctor_path(ctx, &call.target),
                        bytes_from_hex_expr(&hex_expr)
                    ))
                }
                ("Blob", "from_file") if call.args.len() == 1 => {
                    let path_expr = expr_to_rust(&call.args[0], ctx);
                    Some(format!(
                        "{}::new(std::fs::read(({path_expr}).as_str()).map_err(|e| DomainError::External(e.to_string()))?)",
                        stub_ctor_path(ctx, &call.target)
                    ))
                }
                _ => None,
            };
            if let Some(result) = translated {
                return result;
            }
        }
    }

    // Struct-shaped target with method "new" or empty → Type::new(args)
    // Handle dotted paths: `sqlx.Query` → prefer stub crate matching the prefix
    // so `sqlx.Query` does not resolve to an unrelated SDK type also named Query.
    let (module_prefix, effective_target) = if call.target.contains('.') {
        let mut parts = call.target.splitn(2, '.');
        let m = parts.next().unwrap_or("").to_string();
        let t = parts.next().unwrap_or(&call.target).to_string();
        (Some(m), t)
    } else {
        (None, call.target.clone())
    };
    if ctx.is_struct_target(&effective_target)
        || ctx.stub_type_crate.contains_key(&effective_target)
        || module_prefix
            .as_ref()
            .map(|m| {
                ctx.stub_type_crate.values().any(|(c, _)| {
                    c.replace('-', "_") == *m || c.as_str() == m
                })
            })
            .unwrap_or(false)
    {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        // Qualify with crate path if type is from a stub — prefer prefix match.
        let qualified = if let Some(prefix) = &module_prefix {
            // Prefer `prefix.Type` / `prefix::Type` keys (store rust_type_path).
            // A leaf-name scan misses dotted keys and can steal a same-named
            // type from another crate.
            let dotted = format!("{prefix}.{effective_target}");
            let colon = format!("{prefix}::{effective_target}");
            if let Some((crate_name, path_type)) = stub_type_parts(ctx, &dotted)
                .or_else(|| stub_type_parts(ctx, &colon))
                .or_else(|| stub_type_parts(ctx, &effective_target).filter(|(c, _)| {
                    c.replace('-', "_") == *prefix || *c == prefix.as_str()
                }))
            {
                format!("{crate_name}::{path_type}")
            } else {
                // Unloaded stub or no matching crate: keep author module path.
                format!("{}::{}", prefix, effective_target)
            }
        } else if let Some((crate_name, original_name)) = ctx.stub_type_crate.get(&effective_target) {
            // Never crate-qualify Rust built-in types (String, Vec, etc.) even if a
            // stub happens to declare a struct with the same name (e.g. gix has `struct String`).
            let is_builtin = matches!(effective_target.as_str(),
                "String" | "Vec" | "Option" | "Result" | "Box" | "Arc" | "HashMap" | "HashSet" |
                "Path" | "PathBuf" | "Bytes" | "Duration" | "Instant"
            );
            if is_builtin {
                effective_target.clone()
            } else {
                format!("{}::{}", crate_name, original_name)
            }
        } else {
            effective_target.clone()
        };
        // Clone args to avoid move issues (idents and field access like `repo.slug`)
        let cloned = call.args.iter()
            .map(|a| clone_for_reuse(a, expr_to_rust(a, ctx), ctx))
            .collect::<Vec<_>>().join(", ");
        // `Type.default()` → `Type::default()` (requires Default impl from smart ctor).
        if method == "default" && call.args.is_empty() {
            return format!("{}::default()", qualified);
        }
        if method == "new" {
            // Stub constructors that map to module-level free functions.
            // e.g. crate::Query::new(sql) → crate::query(sql)
            // When the stub declares `typed_variant` and the enclosing method has a
            // domain return type → crate::query_as::<_, T>(sql) (params from stub).
            // Only when the stub says so — a lowercase crate path is not enough
            // (`aws_sdk_lambda.Blob.new` is Type::new, not crate::blob()).
            if let Some(module) = qualified.split("::").next() {
                let is_module_fn = qualified.contains("::")
                    && module.chars().next().map(|c| c.is_lowercase()).unwrap_or(false);
                let type_leaf = qualified.split("::").last().unwrap_or("new");
                if is_module_fn
                    && stub_new_is_module_free_fn(ctx, &effective_target, type_leaf)
                {
                    let fn_name = to_snake(type_leaf);
                    let raw_args = call.args.iter()
                        .map(|a| match a {
                            Expr::StringLit(s) => format!("\"{}\"", s),
                            _ => expr_to_rust(a, ctx),
                        })
                        .collect::<Vec<_>>().join(", ");

                    // Prefer explicit stub metadata; fall back to sibling `TypeAs` heuristic.
                    let typed_meta = ctx
                        .stub_typed_ctors
                        .get(&effective_target)
                        .or_else(|| ctx.stub_typed_ctors.get(type_leaf));

                    // query_as only when fetch_* on this type returns a domain row,
                    // not Opt<Str>/List<Str> (JSON payload columns use plain query +
                    // from_str). Method return type alone is not enough — find() may
                    // return Opt<Entity> while the SQL selects a text payload.
                    let fetch_ret = ctx
                        .method_returns
                        .get(&(type_leaf.to_string(), "fetch_optional".into()))
                        .or_else(|| {
                            ctx.method_returns
                                .get(&(effective_target.clone(), "fetch_optional".into()))
                        })
                        .map(|s| s.as_str());
                    let fetch_is_stringish = fetch_ret.is_some_and(|r| {
                        r.contains("Str")
                            || r.contains("String")
                            || r == "Opt<Str>"
                            || r.starts_with("List<Str")
                    });

                    let domain_type = if fetch_is_stringish {
                        None
                    } else {
                        ctx.expected_return_rust.as_ref().and_then(|ret| {
                            extract_domain_type_from_return(ret, &ctx.name_to_shape)
                        })
                    };

                    if let Some(domain_type) = domain_type {
                        if let Some((typed_fn, param_tmpl)) = typed_meta {
                            let tparams = expand_typed_type_params(param_tmpl, &domain_type);
                            return format!(
                                "{module}::{typed_fn}::<{tparams}>({raw_args})"
                            );
                        }
                        // Heuristic: Query + QueryAs both registered → query_as
                        let typed_struct = format!("{type_leaf}As");
                        let has_sibling = ctx.stub_type_crate.contains_key(&typed_struct)
                            || ctx.name_to_shape.contains_key(&typed_struct);
                        if has_sibling {
                            let typed_fn_name = format!("{fn_name}_as");
                            return format!(
                                "{module}::{typed_fn_name}::<_, {domain_type}>({raw_args})"
                            );
                        }
                    }
                    // JSON-payload adapters: SELECT → query_scalar::<_, String>;
                    // INSERT/UPDATE/DELETE → plain query (has execute, no row type).
                    if fetch_is_stringish && type_leaf == "Query" {
                        let sql_is_select = call.args.first().is_some_and(|a| {
                            matches!(a, Expr::StringLit(s) if s.trim_start().to_ascii_lowercase().starts_with("select"))
                        });
                        if sql_is_select {
                            return format!("{module}::query_scalar::<_, String>({raw_args})");
                        }
                        return format!("{module}::query({raw_args})");
                    }
                    return format!("{module}::{fn_name}({raw_args})");
                }
            }
            // If the struct has an `id` field and the caller doesn't provide it
            // (arg count is one fewer than expected), auto-insert Uuid::new_v4() as first arg.
            let has_id_field = ctx.struct_fields.get(&effective_target)
                .map(|fields| fields.iter().any(|(n, _)| n == "id"))
                .unwrap_or(false);
            let final_args = if has_id_field && !call.args.is_empty() {
                // Check if first arg is already named 'id' — if so, caller is providing it
                let first_is_id = matches!(&call.args[0], Expr::Ident(n) if n == "id");
                if first_is_id {
                    cloned // caller provides id explicitly
                } else {
                    // Prepend auto-generated id
                    format!("Uuid::new_v4(), {}", cloned)
                }
            } else if has_id_field && call.args.is_empty() {
                "Uuid::new_v4()".to_string()
            } else {
                cloned
            };
            // If the constructor returns Result (invariant type), append ? to unwrap
            let returns_result = ctx.method_returns.get(&(effective_target.clone(), "new".to_string()))
                .map(|t| t.starts_with("Result<"))
                .unwrap_or(false);
            let suffix = if returns_result { "?" } else { "" };

            // Zero-arg smart ctors (`Default`): `T.new(a, b, c)` → positional
            // field fill + `..T::default()`. Skips a leading `id: Uuid` so
            // `Greeting.new(message)` still maps onto `message`, not `id`.
            if ctx.defaultable_types.contains(&effective_target) && !call.args.is_empty() {
                if let Some(fields) = ctx.struct_fields.get(&effective_target) {
                    let mut field_iter = fields.iter().peekable();
                    let mut parts: Vec<String> = Vec::new();
                    if let Some((fname, fty)) = field_iter.peek() {
                        if *fname == "id" && (*fty == "Uuid" || *fty == "uuid::Uuid") {
                            parts.push("id: Uuid::new_v4()".to_string());
                            field_iter.next();
                        }
                    }
                    for arg in &call.args {
                        if let Some((fname, _)) = field_iter.next() {
                            parts.push(format!(
                                "{}: {}",
                                to_snake(fname),
                                expr_to_rust(arg, ctx)
                            ));
                        }
                    }
                    parts.push(format!("..{}::default()", qualified));
                    return format!("{} {{ {} }}", qualified, parts.join(", "));
                }
            }

            return format!("{}::{}({}){}", qualified, to_snake(method), final_args, suffix);
        }
        // Language primitives (not product facades):
        // - Blob.from_hex / Blob.from_file for binary payloads
        // - Process.run(program, args, cwd) for host process execution
        let method_bare = method.trim_end_matches(['!', '?']);
        if (effective_target == "Blob" || effective_target.ends_with("Blob"))
            && method_bare == "from_hex"
            && call.args.len() == 1
        {
            let hex_expr = expr_to_rust(&call.args[0], ctx);
            return format!(
                "{}::new({})",
                stub_ctor_path(ctx, "Blob"),
                bytes_from_hex_expr(&hex_expr)
            );
        }
        if (effective_target == "Blob" || effective_target.ends_with("Blob"))
            && method_bare == "from_file"
            && call.args.len() == 1
        {
            let path_expr = expr_to_rust(&call.args[0], ctx);
            return format!(
                "{}::new(std::fs::read({path_expr}.as_str()).map_err(|e| DomainError::External(e.to_string()))?)",
                stub_ctor_path(ctx, "Blob")
            );
        }
        if effective_target == "Process" && method_bare == "run" && call.args.len() == 3 {
            let prog = expr_to_rust(&call.args[0], ctx);
            let args = expr_to_rust(&call.args[1], ctx);
            let cwd = expr_to_rust(&call.args[2], ctx);
            let hard = call.method.ends_with('!');
            // Soft Process.run returns detail String (incl. failed:); Process.run! aborts on non-zero.
            if hard {
                return format!(
                    "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); let __out = std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output().map_err(|e| DomainError::External(format!(\"{{e:?}}\")))?; if !__out.status.success() {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(2000).collect::<String>().chars().rev().collect(); return Err(DomainError::External(format!(\"{{}} failed: {{}}\", __prog, __tail))); }} format!(\"{{}} ok: {{}}\", __prog, String::from_utf8_lossy(&__out.stdout).chars().take(500).collect::<String>()) }}"
                );
            }
            return format!(
                "{{ let __prog: String = ({prog}).to_string(); let __args: String = ({args}).to_string(); let __cwd: String = ({cwd}).to_string(); let __argv: Vec<&str> = __args.split_whitespace().collect(); match std::process::Command::new(&__prog).args(&__argv).current_dir(&__cwd).output() {{ Ok(__out) => {{ if __out.status.success() {{ format!(\"{{}} ok: {{}}\", __prog, String::from_utf8_lossy(&__out.stdout).chars().take(400).collect::<String>()) }} else {{ let __err = String::from_utf8_lossy(&__out.stderr); let __tail: String = __err.chars().rev().take(1200).collect::<String>().chars().rev().collect(); format!(\"{{}} failed: {{}}\", __prog, __tail) }} }}, Err(e) => format!(\"{{}} spawn failed: {{e}}\", __prog) }} }}"
            );
        }
        // Non-new method on a struct: UFCS instance form `Email.validate(email)`
        // → `email.validate()`. Only when the first arg *names* the type
        // (Email/email). Do NOT rewrite for any local — that breaks enum
        // constructors: `AttributeValue.S(name)` must stay `AttributeValue::S(name)`.
        // PascalCase methods are always associated constructors / variants.
        let is_pascal_ctor = method
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if !is_pascal_ctor && !call.args.is_empty() {
            if let Expr::Ident(first_arg) = &call.args[0] {
                if first_arg.eq_ignore_ascii_case(&effective_target) {
                    let rest_args = call.args[1..]
                        .iter()
                        .map(|a| expr_to_rust(a, ctx))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("{}.{}({})", first_arg, to_snake(method), rest_args);
                }
            }
        }
        // Enum variant constructor: AttributeValue.S(pk) → AttributeValue::S(pk)
        // No suffix needed — variant constructors are plain sync calls.
        // Stub enums are stored as Shape::Struct, so also check PascalCase method name
        // which indicates a variant constructor (e.g. AttributeValue.S(pk)).
        if ctx.name_to_shape.get(effective_target.as_str()) == Some(&Shape::Enum) || is_pascal_ctor {
            let m = rust_method_name(method);
            let ctor_args = call
                .args
                .iter()
                .map(|a| match a {
                    Expr::StringLit(s) => rust_string_lit_owned(s),
                    other => clone_for_reuse(other, expr_to_rust(other, ctx), ctx),
                })
                .collect::<Vec<_>>()
                .join(", ");
            return format!("{}::{}({})", qualified, m, ctor_args);
        }
        // Prefer stub-qualified path (aws_sdk_s3::Client) over VEIL alias (S3Client).
        // Keep PascalCase for enum variants: AttributeValue::S(x), not ::s(x).
        // Use `cloned` so idents/field access (e.g. repo.slug) are not moved.
        let m = rust_method_name(method);
        let suffix = receiver_call_suffix(
            &Expr::Ident(effective_target.clone()),
            method,
            ctx,
        );
        return format!("{}::{}({}){}", qualified, m, cloned, suffix);
    }

    // `local.field.method(args)` — parser keeps dotted target "initiative.id".
    // Emit `initiative.id.method(...)`, never `id::method(...)`.
    // When a field is Json (`context.stack.topic_arn`), remaining segs are Value keys.
    if call.target.contains('.') && !call.target.starts_with("self.") {
        let first = call.target.split('.').next().unwrap_or("");
        if ctx.is_local(first) {
            let path = lower_dotted_local_path(&call.target, ctx);
            let method = rust_method_name(&call.method);
            if call.args.is_empty()
                && matches!(method_bare(&call.method), "as_str" | "as_s" | "to_str" | "to_string")
            {
                return format!("{path}.as_str().map(|s| s.to_string())");
            }
            if call.args.is_empty() && method_bare(&call.method) == "first" {
                let recv_expr = Expr::Ident(first.to_string());
                return list_first_rust(&path, &recv_expr, ctx);
            }
            let suffix = receiver_call_suffix(
                &Expr::Ident(first.to_string()),
                &call.method,
                ctx,
            );
            return format!(
                "{}.{}({}){}",
                path,
                method,
                clone_args_for_method(&call.method, &call.args, ctx),
                suffix
            );
        }
    }

    // Self field target (method bodies) → self.target.method(args)
    // Parser may produce target "client" or dotted "self.client".
    if ctx.in_method {
        let field = call
            .target
            .strip_prefix("self.")
            .unwrap_or(call.target.as_str());
        if ctx.self_fields.contains(field)
            || call.target.starts_with("self.")
        {
            if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
                return format!(
                    "self.{}{}",
                    to_snake(field),
                    parse_i64_suffix()
                );
            }
            if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
                return format!(
                    "serde_json::from_str::<serde_json::Value>(&self.{})?",
                    to_snake(field)
                );
            }
            let method = rust_method_name(&call.method);
            let suffix = receiver_call_suffix(
                &Expr::Ident(field.to_string()),
                &call.method,
                ctx,
            );
            // Map/HashMap fields wrapped in RwLock need lock acquisition and
            // reference-passing for key arguments (get/contains_key/remove take &Q).
            let field_type = ctx.self_field_types.get(field).or_else(|| ctx.self_field_types.get(&to_snake(field)));
            let is_map_field = field_type
                .map(|t| t.contains("HashMap") || t.starts_with("std::collections::HashMap"))
                .unwrap_or(false);
            if is_map_field {
                let bare_method = call.method.trim_end_matches(['!', '?']);
                match bare_method {
                    "get" | "contains_key" => {
                        // Read-only access: acquire read lock, pass key by reference.
                        // For `get`, append `.cloned()` so the returned value is owned
                        // and does not borrow the lock guard.
                        let key_arg = if !call.args.is_empty() {
                            let s = expr_to_rust(&call.args[0], ctx);
                            format!("&{}", s)
                        } else {
                            String::new()
                        };
                        let clone_suffix = if bare_method == "get" { ".cloned()" } else { "" };
                        return format!(
                            "self.{}.read().await.{}({}){}",
                            to_snake(field),
                            method,
                            key_arg,
                            clone_suffix,
                        );
                    }
                    "insert" => {
                        // Mutating access: acquire write lock
                        let map_args = call.args.iter()
                            .map(|a| {
                                let s = expr_to_rust(a, ctx);
                                match a {
                                    Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{}.clone()", s),
                                    _ => s,
                                }
                            }).collect::<Vec<_>>().join(", ");
                        return format!(
                            "self.{}.write().await.insert({})",
                            to_snake(field),
                            map_args,
                        );
                    }
                    "remove" => {
                        // Mutating access: acquire write lock, pass key by reference
                        let key_arg = if !call.args.is_empty() {
                            let s = expr_to_rust(&call.args[0], ctx);
                            format!("&{}", s)
                        } else {
                            String::new()
                        };
                        return format!(
                            "self.{}.write().await.remove({})",
                            to_snake(field),
                            key_arg,
                        );
                    }
                    "values" | "keys" | "iter" | "len" | "is_empty" => {
                        // Read-only access, no key arg
                        return format!(
                            "self.{}.read().await.{}({})",
                            to_snake(field),
                            method,
                            clone_args_for_method(&call.method, &call.args, ctx),
                        );
                    }
                    _ => {
                        // Other methods: default to write lock (safe fallback)
                        return format!(
                            "self.{}.write().await.{}({})",
                            to_snake(field),
                            method,
                            clone_args_for_method(&call.method, &call.args, ctx),
                        );
                    }
                }
            }
            return format!(
                "self.{}.{}({}){}",
                to_snake(field),
                method,
                clone_args_for_method(&call.method, &call.args, ctx),
                suffix
            );
        }
    }

    // Local variable target → target.method(args)?
    if ctx.is_local(&call.target) {
        // Always strip VEIL `!`/`?` fallible/query suffixes (typecheck sugar only).
        let method = rust_method_name(&call.method);

        // Blob / Bytes / unknown locals: `.to_str()` is utf-8 decode, not a
        // rustc method on the stub type. Leave `.to_string()` alone.
        if call.args.is_empty()
            && matches!(
                call.method.trim_end_matches(['!', '?']),
                "to_str" | "as_str"
            )
        {
            let ty = ctx.local_type(&call.target).unwrap_or("");
            if ty != "String" {
                return format!(
                    "String::from_utf8_lossy({}.as_ref()).to_string()",
                    call.target
                );
            }
        }
        if call.args.is_empty() && method_bare(&call.method) == "as_ref" {
            let recv_ident = Expr::Ident(call.target.clone());
            if should_decode_as_ref_to_str(&recv_ident, ctx) {
                return format!(
                    "String::from_utf8_lossy({}.as_ref()).to_string()",
                    call.target
                );
            }
        }
        if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
            return format!("{}{}", call.target, parse_i64_suffix());
        }
        if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
            return format!(
                "serde_json::from_str::<serde_json::Value>(&{})?",
                call.target
            );
        }
        if call.args.is_empty() && method_bare(&call.method) == "as_n" {
            return format!(
                "{}.as_n(){}{}",
                call.target,
                map_err_domain_own_str(),
                parse_i64_suffix()
            );
        }

        // HashMap/DynamoDB item .get("key") — never panic (review Issue 6).
        if call.method == "get" && call.args.len() == 1 {
            if let Expr::StringLit(key) = &call.args[0] {
                return format!(
                    "{}.get(\"{}\").ok_or_else(|| DomainError::External(\"missing {}\".into()))?",
                    call.target, key, key
                );
            }
        }
        // Option.unwrap() → ok_or; Result.unwrap() → map_err to DomainError.
        // Clone Option first so the local can be reused after is_some()/unwrap.
        if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
            let ty = ctx.local_type(&call.target);
            if ty.map(|t| t.starts_with("Result<")).unwrap_or(false) {
                return format!(
                    "{}.map_err(|e| DomainError::External(format!(\"{{e}}\")))?",
                    call.target
                );
            }
            let is_option = ty
                .map(|t| t.starts_with("Option<"))
                .unwrap_or(true); // default to true if type unknown
            if is_option {
                // When the enclosing function returns Option<T>, use `?` directly
                // on the Option (returns None early) instead of converting to Result.
                let enclosing_returns_option = ctx.expected_return_rust.as_ref()
                    .map(|r| r.starts_with("Option<"))
                    .unwrap_or(false);
                if enclosing_returns_option {
                    return format!("{}.clone()?", call.target);
                }
                return format!(
                    "{}.clone().ok_or(DomainError::NotFound)?",
                    call.target
                );
            } else {
                // Already unwrapped — just use the value
                return call.target.clone();
            }
        }
        // local.ok_or(...) when local is NOT Option → skip, just use the local.
        if call.method == "ok_or" && ctx.is_local(&call.target) {
            let is_option = ctx.local_type(&call.target)
                .map(|t| t.starts_with("Option<"))
                .unwrap_or(true);
            if !is_option {
                return call.target.clone();
            }
        }
        if let Some(type_name) = ctx.local_type(&call.target) {
            // JSON value locals: translate common methods to serde_json equivalents.
            if type_name == "serde_json::Value" {
                match call.method.as_str() {
                    "len" => return format!("{}.as_array().map(|a| a.len() as i64).unwrap_or(0)", call.target),
                    "is_empty" => return format!("{}.as_array().map(|a| a.is_empty()).unwrap_or(true)", call.target),
                    "to_string" | "to_str" => return format!("{}.as_str().unwrap_or(\"\").to_string()", call.target),
                    _ => {}
                }
            }
            // If the local's type is a known trait, methods are async. Only
            // apply `?` when the port method is fallible (Res! / bang).
            if ctx.name_to_shape.get(type_name) == Some(&Shape::Trait) {
                let bare_ty = peel_dyn_trait_name(type_name).unwrap_or_else(|| type_name.to_string());
                let fallible = call.method.ends_with('!')
                    || ctx
                        .type_fallible_methods
                        .contains(&(bare_ty, method.clone()))
                    || ctx
                        .type_fallible_methods
                        .contains(&(type_name.to_string(), method.clone()));
                let suffix = if fallible { ".await?" } else { ".await" };
                return format!("{}.{}({}){}", call.target, method, args_str, suffix);
            }
            // Auto-unwrap Option<T> locals when calling a method that belongs to T.
            // This handles the common pattern: `provider = repo.find!(id)` then
            // `provider.get_endpoint(...)` where provider is Option<ApiProvider>.
            if type_name.starts_with("Option<") {
                let bare_method = call.method.trim_end_matches(['!', '?']);
                let option_methods = [
                    "is_some", "is_none", "unwrap", "unwrap_or", "unwrap_or_else",
                    "unwrap_or_default", "map", "and_then", "or_else", "ok_or",
                    "ok_or_else", "as_ref", "as_mut", "take", "replace", "clone",
                    "expect", "filter", "flatten", "zip",
                ];
                if !option_methods.contains(&bare_method) {
                    let cloned_args = clone_args_for_method(&call.method, &call.args, ctx);
                    return format!(
                        "{}.clone().ok_or(DomainError::NotFound)?.{}({})",
                        call.target, method, cloned_args
                    );
                }
                // Consuming Option methods (and_then, map, unwrap, filter, etc.)
                // move self — clone to allow reuse of the local variable.
                let non_consuming = ["is_some", "is_none", "as_ref", "as_mut", "clone"];
                if !non_consuming.contains(&bare_method) {
                    let cloned_args = clone_args_for_method(&call.method, &call.args, ctx);
                    let suffix = receiver_call_suffix(
                        &Expr::Ident(call.target.clone()),
                        &call.method,
                        ctx,
                    );
                    // unwrap_or with a string lit on Option<String> needs .to_string()
                    if (bare_method == "unwrap_or" || bare_method == "unwrap_or_else")
                        && call.args.len() == 1
                    {
                        if let Expr::StringLit(s) = &call.args[0] {
                            let lit = s.replace('\\', "\\\\").replace('"', "\\\"");
                            return format!(
                                "{}.clone().{}(\"{}\".to_string()){}",
                                call.target, method, lit, suffix
                            );
                        }
                    }
                    return format!(
                        "{}.clone().{}({}){}",
                        call.target, method, cloned_args, suffix
                    );
                }
            }
            // Known concrete method (e.g. aggregate fn) — call with ?
            if ctx.method_returns.contains_key(&(type_name.to_string(), call.method.clone()))
                || ctx.method_returns.contains_key(&(
                    type_name.to_string(),
                    call.method.trim_end_matches(['!', '?']).to_string(),
                ))
            {
                let cloned_args = clone_args_for_typed_method(Some(&type_name), &call.method, &call.args, ctx);
                let suffix = receiver_call_suffix(
                    &Expr::Ident(call.target.clone()),
                    &call.method,
                    ctx,
                );
                return format!("{}.{}({}){}", call.target, method, cloned_args, suffix);
            }
        }
        // Stub getters that return Result<&str, _> (e.g. enum as_s): own a String.
        // Method may be written `as_s!` — lookup is on the bare name.
        if call.args.is_empty() {
            let recv_ty = ctx.local_type(&call.target).map(|s| s.to_string());
            if should_own_str_result(ctx, recv_ty.as_deref(), &call.method) {
                let m = method_bare(&call.method);
                return format!("{}.{m}(){}", call.target, map_err_domain_own_str());
            }
        }
        // serde_json::Value::as_str → Option<String> so assigns/unwrap are owned.
        if call.method == "as_str" && call.args.is_empty() {
            return format!("{}.as_str().map(|s| s.to_string())", call.target);
        }
        // Unknown method on local — clone args to avoid move issues.
        // Collection predicate methods need .iter() prefix in Rust.
        let iter_methods = ["any", "all", "find", "filter", "map", "for_each", "count", "flat_map"];
        if iter_methods.contains(&method.as_str()) {
            return format!(
                "{}.iter().{}({})",
                call.target,
                method,
                clone_args_for_method(&call.method, &call.args, ctx)
            );
        }
        let suffix = receiver_call_suffix(
            &Expr::Ident(call.target.clone()),
            &call.method,
            ctx,
        );
        // unwrap_or on Option<String> needs owned default; Option<&str> (e.g. after
        // `.as_str()`) needs a bare str. Prefer owned — callers of as_str use the
        // chained-receiver path below.
        let bare_m = call.method.trim_end_matches(['!', '?']);
        if (bare_m == "unwrap_or" || bare_m == "unwrap_or_else") && call.args.len() == 1 {
            if let Expr::StringLit(s) = &call.args[0] {
                return format!(
                    "{}.{}(\"{}\".to_string()){}",
                    call.target, method, s, suffix
                );
            }
        }
        // Auto-unwrap Option<T> args passed to container methods like push/insert.
        // When an Option<T> local is pushed into a Vec<T>, unwrap it first.
        if (bare_m == "push" || bare_m == "insert" || bare_m == "extend") && !call.args.is_empty() {
            if let Some(Expr::Ident(arg_name)) = call.args.first() {
                if let Some(ty) = ctx.local_type(arg_name) {
                    if ty.starts_with("Option<") {
                        let rest_args = if call.args.len() > 1 {
                            format!(", {}", clone_args_for_method(&call.method, &call.args[1..], ctx))
                        } else {
                            String::new()
                        };
                        return format!(
                            "{}.{}({}.clone().ok_or(DomainError::NotFound)?{}){}",
                            call.target, method, arg_name, rest_args, suffix
                        );
                    }
                }
            }
        }
        // Resolve target type for ref-param passing
        let target_type: Option<&str> = ctx.local_type(&call.target);
        return format!(
            "{}.{}({}){}",
            call.target,
            method,
            clone_args_for_typed_method(target_type, &call.method, &call.args, ctx),
            suffix
        );
    }
    if call.method.is_empty() {
        // Bare call: now() → Utc::now(), others → as-is (cloning value args so
        // passing locals/state into a by-value param doesn't move them).
        // Bang form `name!(args)` stores target as `name!` — strip for symbol, keep `?`.
        let bare_target = call.target.trim_end_matches(['!', '?']);
        match bare_target {
            "now" => "Utc::now()".to_string(),
            "drop" => {
                // Rust builtin drop() — pass through without cloning.
                let args_str = call.args.iter()
                    .map(|a| expr_to_rust(a, ctx))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("drop({})", args_str)
            }
            _ => {
                // Bare dep-method resolution: `authenticate!()` → `deps.auth.authenticate().await?`
                // when `authenticate` matches a method on an in-scope dep. Two strategies:
                // 1. Exact method match in method_returns (formally declared dep methods)
                // 2. Dep field name prefix match (e.g. dep "auth" → call "authenticate")
                let dep_method_match = ctx.dep_fields.iter().find_map(|(trait_name, field_name)| {
                    // Strategy 1: bare_target is a registered method on this trait
                    let key = (trait_name.clone(), bare_target.to_string());
                    if ctx.method_returns.contains_key(&key) {
                        return Some(field_name.clone());
                    }
                    let key2 = (field_name.clone(), bare_target.to_string());
                    if ctx.method_returns.contains_key(&key2) {
                        return Some(field_name.clone());
                    }
                    // Strategy 2: bare_target starts with the dep field name
                    // (e.g. dep "auth" → call "authenticate", dep "check_scope" → call "check_scope")
                    if bare_target.starts_with(field_name.as_str())
                        && (bare_target.len() == field_name.len()
                            || bare_target.as_bytes().get(field_name.len()) == Some(&b'_')
                            || bare_target[field_name.len()..].chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false))
                    {
                        return Some(field_name.clone());
                    }
                    None
                });
                if let Some(dep_field) = dep_method_match {
                    let args_str = clone_args(&call.args, ctx);
                    return format!(
                        "deps.{}.{}({}).await?",
                        dep_field,
                        to_snake(bare_target),
                        args_str
                    );
                }
                let base = format!(
                    "{}({})",
                    to_snake(bare_target),
                    clone_args(&call.args, ctx)
                );
                let is_bang = call.target.ends_with('!');
                // Layer-declared async functions (e.g. unwind, run_saga) need .await?
                if ctx.async_fns.contains(bare_target) || ctx.async_fns.contains(&call.target)
                {
                    format!("{}.await?", base)
                } else if is_bang {
                    format!("{}?", base)
                } else {
                    base
                }
            }
        }
    } else if ctx.is_local(&call.target) || ctx.name_to_shape.contains_key(&call.target) {
        // Known local/construct method call (already handled above, but be safe).
        format!("{}.{}({})", call.target, to_snake(&call.method), args_str)
    } else {
        // Unknown target with a method (e.g. `http.post(...)`): an external
        // effect. Route it to a generated runtime hook `<target>_<method>(...)`
        // so the code compiles without inventing domain knowledge. The set of
        // hooks is emitted at the bottom of the module.
        //
        // If target has dots (e.g. `sqlx.Query`), the last segment is the
        // struct name — emit `Struct::method(args)` (Rust path syntax).
        // Skip `self.field` — already handled above when in_method.
        if call.target.contains('.') && !call.target.starts_with("self.") {
            let parts: Vec<&str> = call.target.split('.').collect();
            let struct_name = parts.last().unwrap_or(&"");
            // Qualify via stub map when present
            let qualified = if let Some((crate_name, original_name)) =
                ctx.stub_type_crate.get(*struct_name).or_else(|| {
                    // case-insensitive match for Client vs client
                    ctx.stub_type_crate
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(struct_name))
                        .map(|(_, v)| v)
                }) {
                format!("{}::{}", crate_name, original_name)
            } else {
                (*struct_name).to_string()
            };
            let m = rust_method_name(&call.method);
            let bare = call.method.trim_end_matches(['!', '?']);
            let suffix = if bare == "send"
                || bare == "send_with"
                || ctx.async_fallible_methods.contains(bare)
            {
                ".await.map_err(|e| DomainError::External(format!(\"{e:?}\")))?"
            } else if ctx.fallible_methods.contains(bare) {
                "?"
            } else {
                ""
            };
            return format!("{}::{}({}){}", qualified, m, args_str, suffix);
        }
        // Recognize Rust module-qualified calls: serde_json.from_str, std.fs.read, etc.
        // These are lowercase targets with no dots that map to Rust crate paths using `::`.
        let known_modules = [
            "serde_json", "serde", "tokio", "tracing", "uuid", "chrono",
            "std", "aws_sdk_dynamodb", "aws_sdk_s3", "aws_config",
        ];
        let target_snake = to_snake(&call.target);
        if known_modules.contains(&target_snake.as_str()) {
            let m = to_snake(&call.method);
            let suffix = if ctx.fallible_methods.contains(&call.method)
                || call.method == "from_str"
                || call.method == "to_string"
                || call.method == "parse"
            {
                "?"
            } else {
                ""
            };
            // serde_json.from_str → serde_json::from_str(&arg)?
            // serde_json.to_string → serde_json::to_string(&arg)?
            let needs_ref = m == "from_str" || m == "to_string" || m == "to_vec";
            let final_args = if needs_ref && call.args.len() == 1 {
                format!("&{}", expr_to_rust(&call.args[0], ctx))
            } else {
                args_str.clone()
            };
            // from_str needs a turbofish when the enclosing method return type
            // names a concrete domain type (else inference fails with `?`).
            if target_snake == "serde_json" && m == "from_str" {
                if let Some(ty) = from_str_turbofish_type(ctx) {
                    return format!(
                        "serde_json::from_str::<{ty}>({final_args}){suffix}"
                    );
                }
            }
            return format!("{}::{}({}){}", target_snake, m, final_args, suffix);
        }
        // Stub package free functions: `crypto.hmac_sha256_hex(s, m)` or
        // `relay_crypto.aes_gcm_encrypt!(k, p)` → `relay_crypto::fn(&…)` (+ `?` if Res!).
        if let Some(rust_crate) = ctx
            .stub_pkg_crate
            .get(&call.target)
            .or_else(|| ctx.stub_pkg_crate.get(&target_snake))
        {
            let bare = call.method.trim_end_matches(['!', '?']);
            if let Some(&fallible) = ctx
                .stub_free_fns
                .get(&(rust_crate.clone(), bare.to_string()))
            {
                let m = to_snake(bare);
                // Helper crates typically take &str / shared refs.
                let final_args = call
                    .args
                    .iter()
                    .map(|a| {
                        let s = expr_to_rust(a, ctx);
                        match a {
                            Expr::StringLit(_) => format!("&{s}"),
                            Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("&{s}"),
                            _ => {
                                if s.starts_with('&') {
                                    s
                                } else {
                                    format!("&({s})")
                                }
                            }
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let suffix = if fallible {
                    ".map_err(|e| DomainError::External(e.to_string()))?"
                } else {
                    ""
                };
                return format!("{rust_crate}::{m}({final_args}){suffix}");
            }
        }
        // Last resort: target is not a known local, construct, self-field,
        // module, or stub. It is either (a) an external-effect target (e.g.
        // `http.post(...)`) that should be flattened to `http_post(args)` to
        // match the generated runtime-hook stubs, or (b) a closure/iterator
        // parameter calling a method. Closure params are now properly tracked
        // in ctx.locals (see Closure branch above), so anything reaching here
        // IS an external effect — emit the flattened hook form.
        let m_clean = call.method.trim_end_matches(['!', '?']);
        let target_is_var_like = call.target.chars().next()
            .map(|c| c.is_lowercase())
            .unwrap_or(false)
            && !call.target.contains('.');
        if target_is_var_like {
            // Phase 2, Issue 2: .get("key") on closure params — emit bare &str,
            // not .to_string(). Also emit .ok_or_else(...)? to unwrap the Option.
            if m_clean == "get" && call.args.len() == 1 {
                if let Expr::StringLit(key) = &call.args[0] {
                    return format!(
                        "{}.get(\"{}\").ok_or_else(|| DomainError::External(\"missing {}\".into()))?",
                        call.target, key, key
                    );
                }
            }
            // Phase 2, Issue 1: .unwrap() on closure params that are already extracted
            if (m_clean == "unwrap" || m_clean == "unwrap!") && call.args.is_empty() {
                // In closure contexts the value is typically already unwrapped — just return target
                // This handles cases where the closure param was already unwrapped by the chain
                return call.target.clone();
            }
            if call.args.is_empty() && m_clean == "as_n" {
                return format!(
                    "{}.as_n(){}{}",
                    call.target,
                    map_err_domain_own_str(),
                    parse_i64_suffix()
                );
            }
            if call.args.is_empty() && m_clean == "parse_int" {
                return format!("{}{}", call.target, parse_i64_suffix());
            }
            if call.args.is_empty() && m_clean == "parse_json" {
                return format!(
                    "serde_json::from_str::<serde_json::Value>(&{})?",
                    call.target
                );
            }
            // Phase 2: as_s on closure params (DDB AttributeValue)
            if call.args.is_empty()
                && should_own_str_result(ctx, ctx.local_type(&call.target), &call.method)
            {
                return format!(
                    "{}.{}(){}",
                    call.target,
                    m_clean,
                    map_err_domain_own_str()
                );
            }
        }
        // Not on a stub type / construct / local. Do not invent a crate or a
        // no-op hook — the .stub is the only third-party contract.
        format!(
            "{{ compile_error!(\"unstubbed external `{}.{}` — install a .stub and call its types (@field + stub methods)\"); }}",
            call.target,
            m_clean
        )
    }
}

