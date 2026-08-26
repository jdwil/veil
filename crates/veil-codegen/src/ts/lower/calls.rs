//! Call and action lowering — VEIL `Expr::Call` and `Expr::Action` → `TsExpr`.
//!
//! Extracted from `lower.rs` to keep individual files under 1000 lines.

use veil_ir::ast::{ActionExpr, CallExpr, Expr};
use veil_ir::layer::StmtShape;
use crate::expr::GenCtx;
use super::super::expr::{TsBinOp, TsExpr, TsType, TsUnaryOp};
use super::{lower_to_ts, to_camel_case};

// ─── Batch 9: Calls ─────────────────────────────────────────────────────────

/// Lower a VEIL `Call` expression to TsExpr.
///
/// Resolution priority:
/// 1. Hardcoded builtins (Id.new, Json.parse, etc.)
/// 2. Trait dependency (port) calls → `await deps.field.method(args)`
/// 3. Struct constructors → object literal
/// 4. Local/receiver method calls → `recv.method(args)` with Rust idiom stripping
/// 5. Fallback → Raw
pub(super) fn lower_call(call: &CallExpr, ctx: &GenCtx) -> TsExpr {
    // ── 1. Hardcoded builtins ────────────────────────────────────────────
    if let Some(builtin) = lower_builtin_call(call, ctx) {
        return builtin;
    }

    // ── 2. Trait dependency (port) calls ─────────────────────────────────
    if ctx.is_trait_target(&call.target) {
        return lower_trait_dep_call(call, ctx);
    }

    // ── 3. Struct constructor calls ──────────────────────────────────────
    if ctx.is_struct_target(&call.target)
        && (call.method.is_empty() || call.method == "new")
    {
        return lower_struct_ctor(call, ctx);
    }

    // ── 4. Receiver method calls ─────────────────────────────────────────
    if let Some(recv) = &call.receiver {
        return lower_receiver_method(recv, &call.method, &call.args, ctx);
    }

    // ── 5. Local/target method calls ─────────────────────────────────────
    if !call.target.is_empty() && !call.method.is_empty() {
        return lower_target_method(&call.target, &call.method, &call.args, ctx);
    }

    // ── 6. Free function fallback ────────────────────────────────────────
    if !call.target.is_empty() && call.method.is_empty() {
        let args: Vec<TsExpr> = call.args.iter().map(|a| lower_to_ts(a, ctx)).collect();
        return TsExpr::FnCall {
            name: to_camel_case(&call.target),
            args,
            ty: None,
        };
    }

    // ── 7. Bare method call (target empty, no receiver) ─────────────────
    if !call.method.is_empty() {
        let args: Vec<TsExpr> = call.args.iter().map(|a| lower_to_ts(a, ctx)).collect();
        return TsExpr::FnCall {
            name: to_camel_case(&call.method),
            args,
            ty: None,
        };
    }

    // Unreachable in practice: target empty, method empty, no receiver.
    // Emit as undefined with a TODO comment for visibility.
    TsExpr::LayerEmit("/* TODO: empty call expression */ undefined".to_string())
}

/// Lower a trait dependency call: `Target.method!(args)` → `await deps.field.method(args)`
fn lower_trait_dep_call(call: &CallExpr, ctx: &GenCtx) -> TsExpr {
    let dep_name = ctx.deps_field_for(&call.target);
    let method = if call.method.is_empty() { "call" } else { &call.method };
    let method_clean = method.trim_end_matches(['!', '?']);

    let receiver = TsExpr::FieldAccess {
        base: Box::new(TsExpr::Ident { name: "deps".to_string(), ty: None }),
        field: to_camel_case(&dep_name),
        ty: None,
    };

    let args: Vec<TsExpr> = call.args.iter().map(|a| lower_to_ts(a, ctx)).collect();

    // is_async: true on MethodCall emits `await` — no extra Await wrapper needed
    TsExpr::MethodCall {
        receiver: Box::new(receiver),
        method: to_camel_case(method_clean),
        args,
        ty: None,
        is_async: true,
    }
}

/// Lower struct constructor: `Type.new(a, b, c)` → `{ field1: a, field2: b, field3: c }`
fn lower_struct_ctor(call: &CallExpr, ctx: &GenCtx) -> TsExpr {
    let args: Vec<TsExpr> = call.args.iter().map(|a| lower_to_ts(a, ctx)).collect();

    if let Some(struct_fields) = ctx.types.struct_fields.get(&call.target) {
        let ts_fields: Vec<(String, TsExpr)> = struct_fields
            .iter()
            .zip(args.into_iter())
            .map(|((field_name, _field_type), value)| (to_camel_case(field_name), value))
            .collect();
        TsExpr::ObjectLit {
            fields: ts_fields,
            ty: Some(TsType::Named(call.target.clone())),
        }
    } else {
        let ts_fields: Vec<(String, TsExpr)> = args
            .into_iter()
            .enumerate()
            .map(|(i, v)| (format!("field{}", i), v))
            .collect();
        TsExpr::ObjectLit {
            fields: ts_fields,
            ty: Some(TsType::Named(call.target.clone())),
        }
    }
}

/// Lower a method call on a receiver expression.
///
/// Handles Rust-idiom stripping (.clone(), .is_some(), .unwrap(), etc.)
fn lower_receiver_method(recv: &Expr, method: &str, args: &[Expr], ctx: &GenCtx) -> TsExpr {
    let method_clean = method.trim_end_matches(['!', '?']);

    // ── Strip ownership methods ──────────────────────────────────────────
    match method_clean {
        "clone" | "to_owned" => {
            return lower_to_ts(recv, ctx);
        }
        "is_some" => {
            return TsExpr::BinOp {
                left: Box::new(lower_to_ts(recv, ctx)),
                op: TsBinOp::NotEq,
                right: Box::new(TsExpr::NullLit),
                ty: Some(TsType::Boolean),
            };
        }
        "is_none" => {
            return TsExpr::BinOp {
                left: Box::new(lower_to_ts(recv, ctx)),
                op: TsBinOp::Eq,
                right: Box::new(TsExpr::NullLit),
                ty: Some(TsType::Boolean),
            };
        }
        "unwrap" | "unwrap_or_default" => {
            return TsExpr::NonNullAssertion(Box::new(lower_to_ts(recv, ctx)));
        }
        "to_string" => {
            return lower_to_ts(recv, ctx);
        }
        "len" | "length" => {
            return TsExpr::FieldAccess {
                base: Box::new(lower_to_ts(recv, ctx)),
                field: "length".to_string(),
                ty: Some(TsType::Number),
            };
        }
        "contains" | "includes" => {
            let lowered_args: Vec<TsExpr> = args.iter().map(|a| lower_to_ts(a, ctx)).collect();
            return TsExpr::MethodCall {
                receiver: Box::new(lower_to_ts(recv, ctx)),
                method: "includes".to_string(),
                args: lowered_args,
                ty: Some(TsType::Boolean),
                is_async: false,
            };
        }
        "push" => {
            let lowered_args: Vec<TsExpr> = args.iter().map(|a| lower_to_ts(a, ctx)).collect();
            return TsExpr::MethodCall {
                receiver: Box::new(lower_to_ts(recv, ctx)),
                method: "push".to_string(),
                args: lowered_args,
                ty: None,
                is_async: false,
            };
        }
        "unwrap_or" if args.len() == 1 => {
            return TsExpr::NullishCoalesce {
                left: Box::new(lower_to_ts(recv, ctx)),
                right: Box::new(lower_to_ts(&args[0], ctx)),
            };
        }
        _ => {}
    }

    // ── General method call ──────────────────────────────────────────────
    let lowered_args: Vec<TsExpr> = args.iter().map(|a| lower_to_ts(a, ctx)).collect();
    let is_async = ctx.is_stub_method_async_global(method_clean)
        || ctx.is_stub_method_fallible_global(method_clean);

    // is_async: true on MethodCall emits `await` — no extra Await wrapper needed
    TsExpr::MethodCall {
        receiver: Box::new(lower_to_ts(recv, ctx)),
        method: to_camel_case(method_clean),
        args: lowered_args,
        ty: None,
        is_async,
    }
}

/// Lower a method call on a named target (not a receiver expression).
fn lower_target_method(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> TsExpr {
    let recv_expr = Expr::Ident(target.to_string());
    lower_receiver_method(&recv_expr, method, args, ctx)
}

// ─── Hardcoded Builtins ─────────────────────────────────────────────────────

/// Try to translate a call as a known language builtin.
/// Returns `None` if this isn't a recognized builtin.
///
/// Resolution order:
/// 1. Layer-declared method templates (`method_lowers_to` from declare blocks)
/// 2. Language primitives (Id, Json, Dt, Int, Map, List)
fn lower_builtin_call(call: &CallExpr, ctx: &GenCtx) -> Option<TsExpr> {
    let method = call.method.trim_end_matches(['!', '?']);

    // ── 1. Layer-declared method templates ───────────────────────────────
    if let Some(template) = ctx.method_lowers_to
        .get(&(call.target.clone(), method.to_string()))
        .and_then(|targets| targets.get("typescript"))
    {
        let rendered = interpolate_call_template(template, call, ctx);
        return Some(TsExpr::LayerEmit(rendered));
    }

    // Also check for free-function calls (target used as fn name, empty method)
    if method.is_empty() && !call.target.is_empty() {
        if let Some(template) = ctx.method_lowers_to
            .get(&(call.target.clone(), String::new()))
            .and_then(|targets| targets.get("typescript"))
        {
            let rendered = interpolate_call_template(template, call, ctx);
            return Some(TsExpr::LayerEmit(rendered));
        }
    }

    // ── 2. Language primitives ───────────────────────────────────────────
    match (call.target.as_str(), method) {
        ("Id", "new") | ("Id", "new_v4") | ("UUID", "new") | ("UUID", "new_v4")
        | ("Uuid", "new") | ("Uuid", "new_v4") => {
            Some(TsExpr::FnCall {
                name: "crypto.randomUUID".to_string(),
                args: vec![],
                ty: Some(TsType::String),
            })
        }

        ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
            if call.args.is_empty() =>
        {
            Some(TsExpr::MethodCall {
                receiver: Box::new(TsExpr::NewCall {
                    class: "Date".to_string(),
                    args: vec![],
                    ty: Some(TsType::Named("Date".to_string())),
                }),
                method: "toISOString".to_string(),
                args: vec![],
                ty: Some(TsType::String),
                is_async: false,
            })
        }

        ("Dt", "now") | ("DateTime", "now") if call.args.is_empty() => {
            Some(TsExpr::NewCall {
                class: "Date".to_string(),
                args: vec![],
                ty: Some(TsType::Named("Date".to_string())),
            })
        }

        ("Int", "now_unix") | ("Int", "now") if call.args.is_empty() => {
            Some(TsExpr::FnCall {
                name: "Math.floor".to_string(),
                args: vec![TsExpr::BinOp {
                    left: Box::new(TsExpr::FnCall {
                        name: "Date.now".to_string(),
                        args: vec![],
                        ty: Some(TsType::Number),
                    }),
                    op: TsBinOp::Div,
                    right: Box::new(TsExpr::IntLit(1000)),
                    ty: Some(TsType::Number),
                }],
                ty: Some(TsType::Number),
            })
        }

        ("Json", "parse") if call.args.len() == 1 => {
            let arg = lower_to_ts(&call.args[0], ctx);
            Some(TsExpr::FnCall {
                name: "JSON.parse".to_string(),
                args: vec![arg],
                ty: None,
            })
        }

        ("Json", "stringify") if call.args.len() == 1 => {
            let arg = lower_to_ts(&call.args[0], ctx);
            Some(TsExpr::FnCall {
                name: "JSON.stringify".to_string(),
                args: vec![arg],
                ty: Some(TsType::String),
            })
        }

        ("Json", "null") => Some(TsExpr::NullLit),
        ("Json", "object") => Some(TsExpr::ObjectLit { fields: vec![], ty: None }),
        ("Json", "array") => Some(TsExpr::ArrayLit { items: vec![], ty: None }),

        ("Map", "new") if call.args.is_empty() => {
            Some(TsExpr::NewCall {
                class: "Map".to_string(),
                args: vec![],
                ty: None,
            })
        }

        ("List", "new") if call.args.is_empty() => {
            Some(TsExpr::ArrayLit { items: vec![], ty: None })
        }

        _ => None,
    }
}

/// Interpolate a layer template for a Call expression.
/// Variables: `{arg0}`, `{arg1}`, ..., `{args}` (all args comma-separated).
fn interpolate_call_template(template: &str, call: &CallExpr, ctx: &GenCtx) -> String {
    let mut result = template.to_string();

    // {dep} — the deps field name for the call target
    let dep_name = ctx.deps_field_for(&call.target);
    result = result.replace("{dep}", &dep_name);

    // {arg0}, {arg1}, ...
    for (i, arg) in call.args.iter().enumerate() {
        let lowered = crate::ts::emit::emit_ts(&lower_to_ts(arg, ctx));
        result = result.replace(&format!("{{arg{i}}}"), &lowered);
    }

    // {args} — all args comma-separated
    let all_args: String = call.args.iter()
        .map(|a| crate::ts::emit::emit_ts(&lower_to_ts(a, ctx)))
        .collect::<Vec<_>>()
        .join(", ");
    result = result.replace("{args}", &all_args);

    result
}

// ─── Actions ────────────────────────────────────────────────────────────────

/// Lower a layer-defined ActionExpr to TsExpr.
pub(super) fn lower_action(action: &ActionExpr, ctx: &GenCtx) -> TsExpr {
    // Check if layer provides a `lowers_to { typescript: "..." }` template
    if let Some(spec) = ctx.statement_specs.get(&action.keyword) {
        if let Some(template) = spec.lowers_to.get("typescript") {
            let rendered = interpolate_action_template(template, action, spec, ctx);
            let core = TsExpr::LayerEmit(rendered);
            return wrap_action_binding(action, core);
        }
        // Port.method fallback from spec
        if let (Some(port), Some(method)) = (&spec.port_target, &spec.port_method) {
            let receiver = TsExpr::FieldAccess {
                base: Box::new(TsExpr::Ident { name: "deps".to_string(), ty: None }),
                field: to_camel_case(port),
                ty: None,
            };
            let args = lower_action_args(action, ctx);
            let core = TsExpr::MethodCall {
                receiver: Box::new(receiver),
                method: to_camel_case(method),
                args,
                ty: None,
                is_async: true,
            };
            return wrap_action_binding(action, core);
        }
    }

    // Shape-specific defaults
    match action.shape {
        StmtShape::If => lower_guard_action(action, ctx),
        StmtShape::Call | StmtShape::Assign => lower_dispatch_action(action, ctx),
        _ => {
            let fallback = format!(
                "/* TODO: lower action '{}' */ undefined",
                action.keyword
            );
            let core = TsExpr::LayerEmit(fallback);
            wrap_action_binding(action, core)
        }
    }
}

/// Guard action: `guard condition, "message"` → `if (!(cond)) throw new Error("msg")`
fn lower_guard_action(action: &ActionExpr, ctx: &GenCtx) -> TsExpr {
    let condition = action
        .condition
        .as_ref()
        .map(|c| lower_to_ts(c, ctx))
        .or_else(|| action.args.first().map(|c| lower_to_ts(c, ctx)))
        .unwrap_or(TsExpr::BoolLit(true));

    let message = action
        .message
        .clone()
        .or_else(|| {
            action.args.get(1).and_then(|e| match e {
                Expr::StringLit(s) => Some(s.clone()),
                _ => None,
            })
        })
        .unwrap_or_else(|| "precondition failed".to_string());

    // Structural negation — emit parenthesizes complex operands automatically
    let negated_condition = TsExpr::UnaryOp {
        op: TsUnaryOp::Not,
        expr: Box::new(condition),
    };

    TsExpr::If {
        condition: Box::new(negated_condition),
        then_body: vec![TsExpr::Throw {
            message: Box::new(TsExpr::NewCall {
                class: "Error".to_string(),
                args: vec![TsExpr::StringLit(message)],
                ty: None,
            }),
        }],
        else_body: None,
    }
}

/// Dispatch/emit action: → `await deps.bus.dispatch(...)` or event emission
fn lower_dispatch_action(action: &ActionExpr, ctx: &GenCtx) -> TsExpr {
    let bus_receiver = TsExpr::FieldAccess {
        base: Box::new(TsExpr::Ident { name: "deps".to_string(), ty: None }),
        field: "bus".to_string(),
        ty: None,
    };

    let method_name = to_camel_case(&action.keyword);
    let mut args = lower_action_args(action, ctx);

    if !action.target.is_empty() {
        args.insert(0, TsExpr::StringLit(action.target.clone()));
    }

    let core = TsExpr::MethodCall {
        receiver: Box::new(bus_receiver),
        method: method_name,
        args,
        ty: None,
        is_async: true,
    };

    wrap_action_binding(action, core)
}

/// Lower action args (positional or named) to TsExpr vec.
fn lower_action_args(action: &ActionExpr, ctx: &GenCtx) -> Vec<TsExpr> {
    if !action.named_args.is_empty() {
        let fields: Vec<(String, TsExpr)> = action
            .named_args
            .iter()
            .map(|(k, v)| (to_camel_case(k), lower_to_ts(v, ctx)))
            .collect();
        vec![TsExpr::ObjectLit { fields, ty: None }]
    } else if !action.args.is_empty() {
        action.args.iter().map(|a| lower_to_ts(a, ctx)).collect()
    } else {
        vec![]
    }
}

/// Wrap an action core expression in a binding if `result_binding` is set.
fn wrap_action_binding(action: &ActionExpr, core: TsExpr) -> TsExpr {
    if let Some(binding) = &action.result_binding {
        TsExpr::Const {
            name: to_camel_case(binding),
            ty: None,
            value: Box::new(core),
        }
    } else {
        core
    }
}

/// Interpolate a layer template with action values.
fn interpolate_action_template(
    template: &str,
    action: &ActionExpr,
    spec: &veil_ir::layer::StatementSpec,
    ctx: &GenCtx,
) -> String {
    let mut result = template.to_string();

    // {args}
    let args_str = if !action.named_args.is_empty() {
        let fields: Vec<String> = action
            .named_args
            .iter()
            .map(|(k, v)| {
                let val = emit_lower(v, ctx);
                let key = to_camel_case(k);
                if key == val { key } else { format!("{}: {}", key, val) }
            })
            .collect();
        if action.target.is_empty() {
            format!("{{ {} }}", fields.join(", "))
        } else {
            format!("{{ type: \"{}\", {} }}", action.target, fields.join(", "))
        }
    } else if !action.args.is_empty() {
        action.args.iter().map(|a| emit_lower(a, ctx)).collect::<Vec<_>>().join(", ")
    } else if !action.target.is_empty() {
        action.target.clone()
    } else {
        String::new()
    };
    result = result.replace("{args}", &args_str);

    // {arg0}, {arg1}, ...
    for (i, arg) in action.args.iter().enumerate() {
        result = result.replace(&format!("{{arg{i}}}"), &emit_lower(arg, ctx));
    }

    // {dep}
    if let Some(dep_type) = &spec.requires_dep {
        result = result.replace("{dep}", &to_camel_case(dep_type));
    } else if let Some(port) = &spec.port_target {
        result = result.replace("{dep}", &to_camel_case(port));
    }

    // {self} → "this" in TS
    result = result.replace("{self}", "this");

    // {named.key}
    for (key, val) in &action.named_args {
        result = result.replace(&format!("{{named.{key}}}"), &emit_lower(val, ctx));
    }

    // {body}
    if result.contains("{body}") {
        let body = action
            .body
            .iter()
            .map(|e| emit_lower(e, ctx))
            .collect::<Vec<_>>()
            .join("; ");
        result = result.replace("{body}", &body);
    }

    result
}

/// Helper: lower an expr and emit to string (for template interpolation).
fn emit_lower(expr: &Expr, ctx: &GenCtx) -> String {
    crate::ts::emit::emit_ts(&lower_to_ts(expr, ctx))
}
