use veil_ir::ast::*;
mod translate;
pub use translate::*;

use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::*;
use super::rust_ir::{
    self, borrow_of, clone_of, field, fn_call, ident, lower_to_rust, lower_value,
    map_err_to_string, method, ok_or_not_found, owned_str, some_of, to_string_of, CallFinish,
    RustExpr, RustType,
};

/// Finish a method call from **receiver type + stub/layer metadata**.
///
/// Never keys off a hardcoded method name (`send`, `put_item`, `build`, …).
/// Async/fallible come from `(Type, method)` registered when the stub was
/// loaded. VEIL `!` on the call site is the only name-independent override.
///
/// Walks Call chains so `self.client.put_item().item(…).send()` sees the
/// builder type of `send`, not an untyped receiver.
pub fn receiver_call_finish(recv: &Expr, method: &str, ctx: &GenCtx) -> CallFinish {
    let has_bang = method.ends_with('!');
    let method = method.trim_end_matches(['!', '?']);
    let recv_ty = infer_receiver_type(recv, ctx);

    if let Some(ref ty) = recv_ty {
        let keys = type_lookup_keys(ty);
        let is_trait = keys.iter().any(|k| ctx.name_to_shape.get(k.as_str()) == Some(&Shape::Trait));
        let is_struct = keys.iter().any(|k| {
            ctx.name_to_shape.get(k.as_str()) == Some(&Shape::Struct)
                || ctx.stubs.stub_type_crate.contains_key(k.as_str())
        });
        let typed_async = keys.iter().any(|k| {
            ctx.is_method_async_fallible(k, method)
        });
        let typed_fall = keys.iter().any(|k| {
            ctx.is_method_fallible(k, method)
        });

        if is_trait {
            return if has_bang || typed_fall {
                CallFinish::AwaitTry
            } else {
                CallFinish::Await
            };
        }
        if is_struct || typed_async || typed_fall {
            if typed_async {
                return if has_bang {
                    CallFinish::AwaitMapErr
                } else {
                    CallFinish::Await
                };
            }
            if typed_fall {
                return if should_own_str_result(ctx, Some(ty.as_str()), method) {
                    CallFinish::MapErrOwnStr
                } else {
                    CallFinish::MapErrDebug
                };
            }
            return if has_bang {
                CallFinish::AwaitMapErr
            } else {
                CallFinish::Bare
            };
        }
    }

    // Call-site `!` is VEIL fallible sugar, not a method-name special case.
    if has_bang {
        CallFinish::AwaitMapErr
    } else {
        CallFinish::Bare
    }
}

/// Type names to probe in stub maps: full path, dyn peel, leaf after `::`.
fn type_lookup_keys(ty: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let push = |keys: &mut Vec<String>, s: String| {
        if !s.is_empty() && !keys.iter().any(|k| k == &s) {
            keys.push(s);
        }
    };
    push(&mut keys, ty.to_string());
    if let Some(p) = peel_dyn_trait_name(ty) {
        push(&mut keys, p);
    }
    if let Some(leaf) = ty.rsplit("::").next() {
        push(&mut keys, leaf.to_string());
    }
    keys
}

fn field_type_of(owner: &str, field: &str, ctx: &GenCtx) -> Option<String> {
    for key in type_lookup_keys(owner) {
        if let Some(ft) = ctx
            .field_type(&key, field)
            .or_else(|| ctx.field_type(&key, &to_snake(field)))
        {
            return Some(ft.to_string());
        }
    }
    None
}

fn dotted_recv_type(target: &str, ctx: &GenCtx) -> Option<String> {
    let mut parts = target.split('.');
    let first = parts.next()?;
    let mut ty = if first == "self" {
        let field = parts.next()?;
        ident_recv_type(field, ctx)?
    } else {
        ident_recv_type(first, ctx)?
    };
    for seg in parts {
        ty = field_type_of(&ty, seg, ctx)?;
    }
    Some(ty)
}

fn ident_recv_type(name: &str, ctx: &GenCtx) -> Option<String> {
    if let Some(rest) = name.strip_prefix("self.") {
        return ident_recv_type(rest, ctx);
    }
    if ctx.is_struct_target(name) || ctx.is_trait_target(name) {
        return Some(name.to_string());
    }
    if let Some(t) = ctx.local_type(name) {
        return Some(peel_dyn_trait_name(t).unwrap_or_else(|| t.to_string()));
    }
    if let Some(t) = ctx
        .self_field_types
        .get(name)
        .or_else(|| ctx.self_field_types.get(&to_snake(name)))
        .or_else(|| {
            resolve_self_field_name(ctx, name).and_then(|rf| ctx.self_field_types.get(&rf))
        })
    {
        return Some(peel_dyn_trait_name(t).unwrap_or_else(|| t.clone()));
    }
    if ctx.stubs.stub_type_crate.contains_key(name) {
        return Some(name.to_string());
    }
    None
}

/// Infer the static type of a receiver, walking Call/FieldAccess chains.
pub fn infer_receiver_type(recv: &Expr, ctx: &GenCtx) -> Option<String> {
    match recv {
        Expr::Ident(name) => ident_recv_type(name, ctx),
        Expr::FieldAccess(base, field) => {
            if let Expr::Ident(n) = base.as_ref()
                && n == "self"
            {
                return ctx
                    .self_field_types
                    .get(field)
                    .or_else(|| ctx.self_field_types.get(&to_snake(field)))
                    .cloned()
                    .map(|t| peel_dyn_trait_name(&t).unwrap_or(t));
            }
            let bt = infer_receiver_type(base, ctx)?;
            for key in type_lookup_keys(&bt) {
                if let Some(ft) = ctx
                    .field_type(&key, field)
                    .or_else(|| ctx.field_type(&key, &to_snake(field)))
                {
                    return Some(ft.to_string());
                }
            }
            None
        }
        Expr::Index(base, _) => infer_receiver_type(base, ctx)
            .and_then(|t| extract_box_dyn_trait(&t).or_else(|| extract_vec_elem(&t))),
        Expr::Call(inner)
            if (inner.method == "get" || inner.method == "get!") && inner.args.len() == 1 =>
        {
            let base = if !inner.target.is_empty() {
                Some(Expr::Ident(inner.target.clone()))
            } else {
                inner.receiver.as_deref().cloned()
            };
            base.as_ref()
                .and_then(|b| infer_receiver_type(b, ctx))
                .and_then(|t| extract_box_dyn_trait(&t).or_else(|| extract_vec_elem(&t)))
        }
        Expr::Call(inner) => {
            let recv_ty = if let Some(r) = &inner.receiver {
                infer_receiver_type(r, ctx)
            } else if inner.target.contains('.') {
                dotted_recv_type(&inner.target, ctx)
            } else if !inner.target.is_empty() {
                ident_recv_type(&inner.target, ctx)
            } else {
                None
            };
            let method = inner.method.trim_end_matches(['!', '?']);
            let ret = recv_ty.as_ref().and_then(|ty| {
                type_lookup_keys(ty).into_iter().find_map(|k| {
                    ctx.return_type_of(&k, method).map(|s| s.to_string())
                })
            });
            match ret.as_deref() {
                Some("Self") | Some("self") | Some("&Self") | Some("&mut Self") => recv_ty,
                Some(t) => Some(peel_dyn_trait_name(t).unwrap_or_else(|| t.to_string())),
                None => recv_ty,
            }
        }
        _ => None,
    }
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

pub fn clone_args_for_method(method: &str, args: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    clone_args_ir(None, method, args, ctx)
}

/// Clone/ref args for a method call, with optional receiver type for ref-param resolution.
pub fn clone_args_ir(recv_type: Option<&str>, method: &str, args: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    let method = method.trim_end_matches(['!', '?']);

    if let Some(type_name) = recv_type
        && let Some(ref_flags) = ctx
            .types
            .ref_params
            .get(&(type_name.to_string(), method.to_string()))
    {
        return args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let is_ref = ref_flags.get(i).copied().unwrap_or(false);
                if is_ref {
                    match a {
                        Expr::StringLit(lit) => RustExpr::StringLit(lit.clone()),
                        Expr::Ident(n) if ctx.is_local(n) => borrow_of(RustExpr::UnaryOp {
                            op: "*".to_string(),
                            expr: Box::new(ident(n.clone())),
                            ty: None,
                        }),
                        other => {
                            let node = lower_value(other, ctx);
                            if matches!(node, RustExpr::Borrow { .. }) {
                                node
                            } else {
                                borrow_of(node)
                            }
                        }
                    }
                } else {
                    match a {
                        Expr::Ident(n) if ctx.is_local(n) && should_clone_ident(n, ctx) => {
                            clone_of(ident(n.clone()))
                        }
                        Expr::StringLit(s) => owned_str(s),
                        _ => lower_value(a, ctx),
                    }
                }
            })
            .collect();
    }
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
                Expr::StringLit(s) => RustExpr::StringLit(s.clone()),
                Expr::Ident(n) => borrow_of(ident(n.clone())),
                other => {
                    let node = lower_value(other, ctx);
                    if matches!(node, RustExpr::Borrow { .. }) {
                        node
                    } else {
                        borrow_of(node)
                    }
                }
            })
            .collect();
    }
    if method == "unwrap_or" && args.len() == 1
        && let Expr::StringLit(s) = &args[0]
    {
        return vec![RustExpr::StringLit(s.clone())];
    }
    if method == "basic_auth" && args.len() >= 2 {
        let mut out = clone_args_ir(recv_type, method, &args[..1], ctx);
        out.push(some_of(lower_value(&args[1], ctx)));
        return out;
    }
    if method == "bind" && args.len() == 1 {
        if let Expr::Ident(n) = &args[0]
            && (ctx.local_type(n) == Some("Uuid")
                || n == "id"
                || n.ends_with("_id")
                || n.ends_with("Id"))
        {
            return vec![to_string_of(ident(n.clone()))];
        }
        if let Expr::FieldAccess(base, field) = &args[0] {
            let f = to_snake(field);
            if f == "id" || f.ends_with("_id") {
                return vec![to_string_of(rust_ir::field(lower_value(base, ctx), f))];
            }
        }
    }
    let param_tys = param_types_for(recv_type, method, ctx);
    args.iter()
        .enumerate()
        .map(|(i, a)| arg_to_ir(a, param_tys.get(i).map(|s| s.as_str()), ctx))
        .collect()
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

pub fn list_index_get_ir(base: RustExpr, idx: RustExpr, base_expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let recv = match base {
        RustExpr::Clone(inner) => *inner,
        other => other,
    };
    let idx = match &idx {
        RustExpr::IntLit(_) => idx,
        _ => rust_ir::cast(idx, "usize"),
    };
    let got = method(recv, "get", vec![idx]);
    if list_elem_is_cloneable(base_expr, ctx) {
        ok_or_not_found(method(got, "cloned", vec![]), ctx)
    } else {
        ok_or_not_found(got, ctx)
    }
}

pub(super) fn list_first_ir(base: RustExpr, base_expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let got = method(base, "first", vec![]);
    if list_elem_is_cloneable(base_expr, ctx) {
        ok_or_not_found(method(got, "cloned", vec![]), ctx)
    } else {
        ok_or_not_found(got, ctx)
    }
}

/// Lower `local.field.nested` — struct fields stay `.field`; once a field is Json,
/// remaining segments become `["key"]` indexes.
pub(super) fn lower_dotted_local_path_ir(target: &str, ctx: &GenCtx) -> RustExpr {
    let mut parts = target.split('.');
    let Some(first) = parts.next() else {
        return ident(target);
    };
    let mut node = ident(first);
    let mut ty = ctx.local_type(first).map(|s| s.to_string());
    let mut json_mode = ty.as_deref().is_some_and(is_json_type_name);
    for seg in parts {
        let field_snake = to_snake(seg);
        if json_mode {
            node = RustExpr::Index {
                base: Box::new(node),
                index: Box::new(RustExpr::StringLit(seg.to_string())),
                ty: Some(RustType::Json),
            };
            continue;
        }
        if let Some(t) = ty.as_deref()
            && let Some(ft) = ctx
                .field_type(t, seg)
                .or_else(|| ctx.field_type(t, &field_snake))
        {
            node = clone_of(field(node, field_snake));
            if is_json_type_name(ft) {
                json_mode = true;
            }
            ty = Some(ft.to_string());
            continue;
        }
        node = field(node, field_snake);
    }
    node
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

fn map_literal_to_hashmap_ir(fields: &[(String, Expr)], ctx: &GenCtx) -> RustExpr {
    if fields.is_empty() {
        return fn_call("std::collections::HashMap::new", vec![]);
    }
    let mut stmts: Vec<RustExpr> = vec![RustExpr::Let {
        name: "__m".to_string(),
        mutable: true,
        ty: None,
        value: Box::new(fn_call("std::collections::HashMap::new", vec![])),
    }];
    for (k, v) in fields {
        stmts.push(method(
            ident("__m"),
            "insert",
            vec![owned_str(k), lower_value(v, ctx)],
        ));
    }
    RustExpr::Block {
        stmts,
        value: Some(Box::new(ident("__m"))),
    }
}

fn arg_looks_optional_ir(arg: &Expr, node: &RustExpr, ctx: &GenCtx) -> bool {
    match node {
        RustExpr::FnCall { path, .. } if path == "Some" => true,
        RustExpr::Ident { name, .. } if name == "None" || name.starts_with("None::<") => true,
        _ => match arg {
            Expr::Ident(n) => ctx
                .local_type(n)
                .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                .unwrap_or(false),
            _ => false,
        },
    }
}

fn is_state_index(node: &RustExpr) -> bool {
    match node {
        RustExpr::Clone(inner) | RustExpr::Borrow { inner, .. } => is_state_index(inner),
        RustExpr::Index { base, .. } => {
            matches!(base.as_ref(), RustExpr::Ident { name, .. } if name == "state")
                || is_state_index(base)
        }
        _ => false,
    }
}

pub fn arg_to_ir(arg: &Expr, param_ty: Option<&str>, ctx: &GenCtx) -> RustExpr {
    let mut node = if let (Some(ty), Expr::StructLit(name, fields)) = (param_ty, arg) {
        if name.is_empty() && is_hashmap_param(ty) {
            map_literal_to_hashmap_ir(fields, ctx)
        } else if is_json_type_name(ty) {
            json_message_ir(name, fields, ctx)
        } else {
            lower_value(arg, ctx)
        }
    } else {
        match arg {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => {
                let indexed = clone_of(RustExpr::Index {
                    base: Box::new(ident("state")),
                    index: Box::new(RustExpr::StringLit(n.clone())),
                    ty: Some(RustType::Json),
                });
                if param_ty.is_some_and(|t| !is_json_type_name(t) && t != "()" && !t.is_empty()) {
                    let ty = param_ty.unwrap();
                    map_err_to_string(
                        fn_call(format!("serde_json::from_value::<{ty}>"), vec![indexed]),
                        ctx.error_model.external_path(),
                    )
                } else {
                    indexed
                }
            }
            Expr::Ident(n) if is_copy_local(n, ctx) || is_ref_local(n, ctx) => ident(n.clone()),
            Expr::Ident(n) if ctx.ownership.borrow_fields.contains(n.as_str()) => {
                borrow_of(field(ident("self"), n.clone()))
            }
            Expr::Ident(n) if ctx.is_local(n) && should_clone_ident(n, ctx) => {
                clone_of(ident(n.clone()))
            }
            Expr::StringLit(s)
                if param_ty.is_some_and(|t| {
                    rust_ty_is_stringish(t) && !t.starts_with('&') && t != "str"
                }) =>
            {
                owned_str(s)
            }
            Expr::FieldAccess(base, field_name)
                if ctx.ownership.borrow_fields.contains(field_name.as_str())
                    && matches!(base.as_ref(), Expr::Ident(n) if n == "self") =>
            {
                borrow_of(field(ident("self"), field_name.clone()))
            }
            _ => lower_value(arg, ctx),
        }
    };
    if let Some(ty) = param_ty {
        if is_option_param(ty) && !arg_looks_optional_ir(arg, &node, ctx) {
            node = some_of(node);
        }
        if !is_json_type_name(ty)
            && ty != "()"
            && !ty.is_empty()
            && is_state_index(&node)
            && !matches!(&node, RustExpr::FnCall { path, .. } if path.contains("from_value"))
        {
            node = map_err_to_string(
                fn_call(
                    format!("serde_json::from_value::<{ty}>"),
                    vec![clone_of(node)],
                ),
                ctx.error_model.external_path(),
            );
        }
    }
    node
}

pub(super) fn clone_args(args: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    args.iter().map(|a| arg_to_ir(a, None, ctx)).collect()
}

pub fn json_message_ir(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> RustExpr {
    let mut entries: Vec<(String, RustExpr)> = Vec::with_capacity(fields.len() + 1);
    entries.push(("type".to_string(), RustExpr::StringLit(name.to_string())));
    for (k, v) in fields {
        entries.push((k.clone(), to_json_arg_ir(v, ctx)));
    }
    RustExpr::JsonMacro { entries }
}

pub fn json_envelope_ir(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> RustExpr {
    let arg_vals: Vec<RustExpr> = args.iter().map(|a| to_json_arg_ir(a, ctx)).collect();
    RustExpr::JsonMacro {
        entries: vec![
            (
                "target".to_string(),
                RustExpr::StringLit(target.to_string()),
            ),
            (
                "method".to_string(),
                RustExpr::StringLit(method.to_string()),
            ),
            ("args".to_string(), RustExpr::VecMacro(arg_vals)),
        ],
    }
}

pub fn to_json_arg_ir(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::Ident(name) => {
            if name == "null" {
                return RustExpr::JsonNull;
            }
            if ctx.state_locals.contains(name.as_str()) {
                return clone_of(RustExpr::Index {
                    base: Box::new(ident("state")),
                    index: Box::new(RustExpr::StringLit(name.clone())),
                    ty: Some(RustType::Json),
                });
            }
            if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                return clone_of(field(ident("self"), to_snake(name)));
            }
            if ctx.is_local(name) {
                return clone_of(ident(name.clone()));
            }
            RustExpr::StringLit(name.clone())
        }
        Expr::FieldAccess(base, field_name) => {
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return clone_of(RustExpr::Index {
                        base: Box::new(RustExpr::Index {
                            base: Box::new(ident("state")),
                            index: Box::new(RustExpr::StringLit(name.clone())),
                            ty: Some(RustType::Json),
                        }),
                        index: Box::new(RustExpr::StringLit(field_name.clone())),
                        ty: Some(RustType::Json),
                    });
                }
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return clone_of(RustExpr::Index {
                        base: Box::new(ident(name.clone())),
                        index: Box::new(RustExpr::StringLit(field_name.clone())),
                        ty: Some(RustType::Json),
                    });
                }
            }
            clone_of(RustExpr::Index {
                base: Box::new(RustExpr::JsonValue(Box::new(to_json_arg_ir(base, ctx)))),
                index: Box::new(RustExpr::StringLit(field_name.clone())),
                ty: Some(RustType::Json),
            })
        }
        Expr::ArrayLit(items) if items.is_empty() => RustExpr::JsonEmptyArray,
        Expr::ArrayLit(items) => {
            RustExpr::VecMacro(items.iter().map(|e| to_json_arg_ir(e, ctx)).collect())
        }
        _ => lower_to_rust(expr, ctx),
    }
}

