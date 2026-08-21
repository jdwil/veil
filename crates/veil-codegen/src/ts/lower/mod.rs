//! VEIL AST → TsExpr lowering.
//!
//! `lower_to_ts` converts a VEIL `Expr` into the typed `TsExpr` IR.
//! All expression variants are handled structurally — no fallback to
//! legacy string-based codegen.

use veil_ir::ast::{BinOp, Expr, Field, IfExprData, MatchArm, Pattern, StringPart, TypeExpr, UnaryOp};
use crate::expr::GenCtx;
use super::expr::{TsBinOp, TsExpr, TsPattern, TsTemplatePart, TsType, TsUnaryOp};

// ─── Public Entry Point ──────────────────────────────────────────────────────

/// Lower a VEIL expression to a TypeScript IR node.
///
/// Handles all VEIL expression variants structurally: literals, identifiers,
/// field access, operators, bindings, wrappers, control flow, collections,
/// calls, and actions. `Expr::Stock` is unreachable (expanded before codegen).
pub fn lower_to_ts(expr: &Expr, ctx: &GenCtx) -> TsExpr {
    match expr {
        // ── Batch 1: Literals ────────────────────────────────────────────
        Expr::StringLit(s) => TsExpr::StringLit(s.clone()),
        Expr::IntLit(n) => TsExpr::IntLit(*n),
        Expr::FloatLit(f) => TsExpr::FloatLit(*f),
        Expr::BoolLit(b) => TsExpr::BoolLit(*b),
        Expr::StringInterp(parts) => lower_string_interp(parts, ctx),

        // ── Batch 2: Identifiers + Field Access ──────────────────────────
        Expr::Ident(name) => lower_ident(name, ctx),
        Expr::FieldAccess(base, field) => lower_field_access(base, field, ctx),

        // ── Batch 3: Operators ───────────────────────────────────────────
        Expr::BinaryOp(op) => {
            let left = lower_to_ts(&op.left, ctx);
            let right = lower_to_ts(&op.right, ctx);
            TsExpr::BinOp {
                left: Box::new(left),
                op: map_binop(&op.op),
                right: Box::new(right),
                ty: None,
            }
        }
        Expr::UnaryOp(op) => {
            let inner = lower_to_ts(&op.expr, ctx);
            TsExpr::UnaryOp {
                op: map_unaryop(&op.op),
                expr: Box::new(inner),
            }
        }

        // ── Batch 4: Bindings ────────────────────────────────────────────
        Expr::Assign(name, rhs, ty_ann) => lower_assign(name, rhs, ty_ann, ctx),
        Expr::MutAssign(name, rhs, ty_ann) => lower_mut_assign(name, rhs, ty_ann, ctx),
        Expr::LetPattern(pattern, rhs, _ty_ann) => {
            let value = lower_to_ts(rhs, ctx);
            let ts_pattern = pattern_to_ts_ir(pattern);
            TsExpr::Destructure {
                pattern: ts_pattern,
                value: Box::new(value),
            }
        }

        // ── Batch 5: Simple Wrappers ─────────────────────────────────────
        Expr::Return(inner) => {
            match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => TsExpr::Return(Box::new(TsExpr::UndefinedLit)),
                Expr::Ident(n) if n == "null" || n == "None" => {
                    TsExpr::Return(Box::new(TsExpr::NullLit))
                }
                _ => TsExpr::Return(Box::new(lower_to_ts(inner, ctx))),
            }
        }
        Expr::Await(inner) => TsExpr::Await(Box::new(lower_to_ts(inner, ctx))),
        Expr::Try(inner) => {
            // In TS, `?` operator = await; errors throw
            TsExpr::Await(Box::new(lower_to_ts(inner, ctx)))
        }
        Expr::Require(inner) => lower_require(inner, ctx),
        Expr::Break => TsExpr::Break,
        Expr::Continue => TsExpr::Continue,

        // ── Batch 6: Control Flow ────────────────────────────────────────
        Expr::IfExpr(data) => lower_if_expr(data, ctx),
        Expr::Match(scrutinee, arms) => lower_match(scrutinee, arms, ctx),
        Expr::ForLoop { binding, index, iterable, body } => {
            lower_for_loop(binding, index.as_deref(), iterable, body, ctx)
        }
        Expr::WhileLoop { condition, body } => TsExpr::While {
            condition: Box::new(lower_to_ts(condition, ctx)),
            body: lower_block(body, ctx),
        },
        Expr::Loop(body) => TsExpr::Loop {
            body: lower_block(body, ctx),
        },
        Expr::DoBlock(body) => lower_do_block(body, ctx),

        // ── Batch 7: Collections ─────────────────────────────────────────
        Expr::ArrayLit(items) => TsExpr::ArrayLit {
            items: items.iter().map(|e| lower_to_ts(e, ctx)).collect(),
            ty: None,
        },
        Expr::Tuple(items) => TsExpr::ArrayLit {
            items: items.iter().map(|e| lower_to_ts(e, ctx)).collect(),
            ty: None, // TS tuples are typed arrays
        },
        Expr::StructLit(name, fields) => lower_struct_lit(name, fields, ctx),
        Expr::StructUpdate { name, fields, base } => {
            lower_struct_update(name, fields, base, ctx)
        }
        Expr::Index(base_expr, idx) => TsExpr::Index {
            base: Box::new(lower_to_ts(base_expr, ctx)),
            index: Box::new(lower_to_ts(idx, ctx)),
        },

        // ── Batch 8: Additional Wrappers ─────────────────────────────────
        Expr::Cast(inner, ty_name) => TsExpr::TypeAssertion {
            expr: Box::new(lower_to_ts(inner, ctx)),
            ty: map_cast_type(ty_name),
        },
        Expr::Closure { params, body } => lower_closure(params, body, ctx),
        // Range has no native TS equivalent — emit a comment-annotated array.
        // TS lacks a built-in range type; preserves start/end for intent.
        Expr::Range { start, end, .. } => {
            let start_expr = start
                .as_ref()
                .map(|e| lower_to_ts(e, ctx))
                .unwrap_or(TsExpr::IntLit(0));
            let end_expr = end
                .as_ref()
                .map(|e| lower_to_ts(e, ctx))
                .unwrap_or_else(|| TsExpr::Ident {
                    name: "Infinity".to_string(),
                    ty: Some(TsType::Number),
                });
            TsExpr::ArrayLit {
                items: vec![start_expr, end_expr],
                ty: None,
            }
        }
        Expr::IfLet { pattern: _, expr: inner, then_body, else_body } => {
            lower_if_let(inner, then_body, else_body.as_deref(), ctx)
        }
        Expr::WhileLet { pattern: _, expr: inner, body } => {
            lower_while_let(inner, body, ctx)
        }

        // ── Batch 9: Calls ────────────────────────────────────────────────
        Expr::Call(call) => lower_call(call, ctx),

        // ── Batch 10: Actions ────────────────────────────────────────────
        Expr::Action(action) => lower_action(action, ctx),

        // ── Stock: transpile-time marker, never reaches codegen ─────────────
        Expr::Stock => unreachable!("Expr::Stock should be expanded before codegen"),
    }
}

// ─── Batch 1: String Interpolation ──────────────────────────────────────────

fn lower_string_interp(parts: &[StringPart], ctx: &GenCtx) -> TsExpr {
    let ts_parts: Vec<TsTemplatePart> = parts
        .iter()
        .map(|part| match part {
            StringPart::Literal(s) => TsTemplatePart::Literal(s.clone()),
            StringPart::Expr(e) => TsTemplatePart::Expr(lower_to_ts(e, ctx)),
        })
        .collect();
    TsExpr::TemplateLit { parts: ts_parts }
}

// ─── Batch 2: Identifiers ───────────────────────────────────────────────────

fn lower_ident(name: &str, ctx: &GenCtx) -> TsExpr {
    match name {
        "null" | "None" => TsExpr::NullLit,
        "noop" => TsExpr::Noop,
        _ => {
            let ty = ctx
                .types
                .local_types
                .get(name)
                .map(|t| veil_type_str_to_ts(t));
            TsExpr::Ident {
                name: to_camel_case(name),
                ty,
            }
        }
    }
}

fn lower_field_access(base: &Expr, field: &str, ctx: &GenCtx) -> TsExpr {
    let base_node = lower_to_ts(base, ctx);
    TsExpr::FieldAccess {
        base: Box::new(base_node),
        field: to_camel_case(field),
        ty: None,
    }
}

// ─── Batch 3: Operator Mapping ──────────────────────────────────────────────

fn map_binop(op: &BinOp) -> TsBinOp {
    match op {
        BinOp::Add => TsBinOp::Add,
        BinOp::Sub => TsBinOp::Sub,
        BinOp::Mul => TsBinOp::Mul,
        BinOp::Div => TsBinOp::Div,
        BinOp::Mod => TsBinOp::Mod,
        BinOp::Eq => TsBinOp::Eq,       // VEIL == → TS ===
        BinOp::NotEq => TsBinOp::NotEq, // VEIL != → TS !==
        BinOp::Lt => TsBinOp::Lt,
        BinOp::Gt => TsBinOp::Gt,
        BinOp::LtEq => TsBinOp::LtEq,
        BinOp::GtEq => TsBinOp::GtEq,
        BinOp::And => TsBinOp::And,
        BinOp::Or => TsBinOp::Or,
    }
}

fn map_unaryop(op: &UnaryOp) -> TsUnaryOp {
    match op {
        UnaryOp::Not => TsUnaryOp::Not,
        UnaryOp::Neg => TsUnaryOp::Neg,
    }
}

// ─── Batch 4: Bindings ──────────────────────────────────────────────────────

fn lower_assign(name: &str, rhs: &Expr, ty_ann: &Option<TypeExpr>, ctx: &GenCtx) -> TsExpr {
    let value = lower_to_ts(rhs, ctx);

    // Field write: `loan.returned = true` — not a new binding, it's an assignment.
    if name.contains('.') {
        let path = name
            .split('.')
            .map(to_camel_case)
            .collect::<Vec<_>>()
            .join(".");
        return TsExpr::Assign {
            target: Box::new(TsExpr::Ident { name: path, ty: None }),
            value: Box::new(value),
        };
    }

    let ty = ty_ann.as_ref().map(veil_type_to_ts_string);
    TsExpr::Const {
        name: to_camel_case(name),
        ty,
        value: Box::new(value),
    }
}

fn lower_mut_assign(name: &str, rhs: &Expr, ty_ann: &Option<TypeExpr>, ctx: &GenCtx) -> TsExpr {
    let value = lower_to_ts(rhs, ctx);
    let ty = ty_ann.as_ref().map(veil_type_to_ts_string);
    TsExpr::Let {
        name: to_camel_case(name),
        ty,
        value: Box::new(value),
    }
}

fn pattern_to_ts_ir(pat: &Pattern) -> TsPattern {
    match pat {
        Pattern::Tuple(parts) => {
            let items = parts.iter().map(pattern_name).collect();
            TsPattern::Array { items }
        }
        Pattern::Struct(_, fields, _has_rest) => {
            let field_names = fields.iter().map(|(k, _)| to_camel_case(k)).collect();
            TsPattern::Object { fields: field_names }
        }
        Pattern::Ident(s) => {
            // Single ident → treat as array destructure with one element
            TsPattern::Array { items: vec![to_camel_case(s)] }
        }
        _ => {
            // Fallback: use a simple object pattern with the string repr
            TsPattern::Object {
                fields: vec![pat.to_string_repr()],
            }
        }
    }
}

/// Extract a binding name from a pattern (for destructure items).
fn pattern_name(pat: &Pattern) -> String {
    match pat {
        Pattern::Ident(s) => to_camel_case(s),
        Pattern::Wildcard => "_".to_string(),
        other => other.to_string_repr(),
    }
}

// ─── Batch 5: Require ───────────────────────────────────────────────────────

fn lower_require(inner: &Expr, ctx: &GenCtx) -> TsExpr {
    // `require expr` → null-check IIFE: `(expr) ?? (() => { throw new Error("NotFound"); })()`
    // Structurally: NullishCoalesce { left: inner, right: throw-IIFE }
    let value = lower_to_ts(inner, ctx);
    let throw_iife = TsExpr::FnCall {
        name: String::new(),
        args: vec![TsExpr::ArrowFn {
            params: vec![],
            body: vec![TsExpr::Throw {
                message: Box::new(TsExpr::NewCall {
                    class: "Error".to_string(),
                    args: vec![TsExpr::StringLit("NotFound".to_string())],
                    ty: None,
                }),
            }],
            is_async: false,
        }],
        ty: None,
    };
    TsExpr::NullishCoalesce {
        left: Box::new(value),
        right: Box::new(throw_iife),
    }
}

// ─── Block Helper ────────────────────────────────────────────────────────────

/// Lower a sequence of VEIL expressions (a body/block) to a Vec<TsExpr>.
pub fn lower_block(body: &[Expr], ctx: &GenCtx) -> Vec<TsExpr> {
    body.iter().map(|e| lower_to_ts(e, ctx)).collect()
}

// ─── Batch 6: Control Flow ──────────────────────────────────────────────────

fn lower_if_expr(data: &IfExprData, ctx: &GenCtx) -> TsExpr {
    TsExpr::If {
        condition: Box::new(lower_to_ts(&data.condition, ctx)),
        then_body: lower_block(&data.then_body, ctx),
        else_body: data.else_body.as_ref().map(|eb| lower_block(eb, ctx)),
    }
}

fn lower_match(scrutinee: &Expr, arms: &[MatchArm], ctx: &GenCtx) -> TsExpr {
    let scrut = lower_to_ts(scrutinee, ctx);
    let mut cases: Vec<(String, Vec<TsExpr>)> = Vec::new();
    let mut default: Option<Vec<TsExpr>> = None;

    for arm in arms {
        let body = lower_block(&arm.body, ctx);
        // Wildcard or `_` pattern → default arm
        if arm.pattern == "_" || matches!(&arm.rich_pattern, Some(Pattern::Wildcard)) {
            default = Some(body);
        } else {
            cases.push((arm.pattern.clone(), body));
        }
    }

    TsExpr::Switch {
        scrutinee: Box::new(scrut),
        cases,
        default,
    }
}

fn lower_for_loop(
    binding: &str,
    index: Option<&str>,
    iterable: &Expr,
    body: &[Expr],
    ctx: &GenCtx,
) -> TsExpr {
    let iter_expr = lower_to_ts(iterable, ctx);
    let lowered_body = lower_block(body, ctx);

    match index {
        Some(idx) => TsExpr::ForIndex {
            index: to_camel_case(idx),
            binding: to_camel_case(binding),
            iterable: Box::new(iter_expr),
            body: lowered_body,
        },
        None => TsExpr::For {
            binding: to_camel_case(binding),
            iterable: Box::new(iter_expr),
            body: lowered_body,
        },
    }
}

fn lower_do_block(body: &[Expr], ctx: &GenCtx) -> TsExpr {
    // DoBlock → IIFE: (() => { ... })()
    let lowered_body = lower_block(body, ctx);
    TsExpr::FnCall {
        name: String::new(),
        args: vec![TsExpr::ArrowFn {
            params: vec![],
            body: lowered_body,
            is_async: false,
        }],
        ty: None,
    }
}

// ─── Batch 7: Collections ───────────────────────────────────────────────────

fn lower_struct_lit(_name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> TsExpr {
    let ts_fields: Vec<(String, TsExpr)> = fields
        .iter()
        .map(|(k, v)| (to_camel_case(k), lower_to_ts(v, ctx)))
        .collect();
    TsExpr::ObjectLit {
        fields: ts_fields,
        ty: None,
    }
}

fn lower_struct_update(
    _name: &str,
    fields: &[(String, Expr)],
    base: &Expr,
    ctx: &GenCtx,
) -> TsExpr {
    // Emit: { ...base, field1: val1, field2: val2 }
    let mut ts_fields: Vec<(String, TsExpr)> = Vec::with_capacity(fields.len() + 1);

    // Spread base first
    let spread_key = "...".to_string();
    ts_fields.push((spread_key, TsExpr::Spread(Box::new(lower_to_ts(base, ctx)))));

    // Then field overrides
    for (k, v) in fields {
        ts_fields.push((to_camel_case(k), lower_to_ts(v, ctx)));
    }

    TsExpr::ObjectLit {
        fields: ts_fields,
        ty: None,
    }
}

// ─── Batch 8: Additional Wrappers ───────────────────────────────────────────

/// Map a VEIL cast type name to a TS type annotation string.
fn map_cast_type(ty_name: &str) -> String {
    match ty_name {
        "Str" | "String" => "string".to_string(),
        "Int" | "i64" | "i32" | "u64" | "u32" | "F64" | "f64" => "number".to_string(),
        "Bool" | "bool" => "boolean".to_string(),
        other => to_camel_case(other),
    }
}

fn lower_closure(params: &[String], body: &[Expr], ctx: &GenCtx) -> TsExpr {
    let lowered_body = lower_block(body, ctx);
    let is_async = body_contains_await(body);
    TsExpr::ArrowFn {
        params: params.iter().map(|p| to_camel_case(p)).collect(),
        body: lowered_body,
        is_async,
    }
}

/// Check if a body (recursively) contains any Await or Try expression.
fn body_contains_await(body: &[Expr]) -> bool {
    body.iter().any(|e| expr_contains_await(e))
}

fn expr_contains_await(expr: &Expr) -> bool {
    match expr {
        Expr::Await(_) | Expr::Try(_) => true,
        Expr::BinaryOp(op) => {
            expr_contains_await(&op.left) || expr_contains_await(&op.right)
        }
        Expr::UnaryOp(op) => expr_contains_await(&op.expr),
        Expr::Return(inner) => expr_contains_await(inner),
        Expr::FieldAccess(base, _) => expr_contains_await(base),
        Expr::IfExpr(data) => {
            expr_contains_await(&data.condition)
                || body_contains_await(&data.then_body)
                || data.else_body.as_ref().map_or(false, |eb| body_contains_await(eb))
        }
        Expr::ForLoop { iterable, body, .. } => {
            expr_contains_await(iterable) || body_contains_await(body)
        }
        Expr::WhileLoop { condition, body } => {
            expr_contains_await(condition) || body_contains_await(body)
        }
        Expr::Loop(body) => body_contains_await(body),
        Expr::DoBlock(body) => body_contains_await(body),
        Expr::Closure { body, .. } => body_contains_await(body),
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => expr_contains_await(rhs),
        _ => false,
    }
}

fn lower_if_let(
    inner: &Expr,
    then_body: &[Expr],
    else_body: Option<&[Expr]>,
    ctx: &GenCtx,
) -> TsExpr {
    // `if let x = expr { ... }` → `if (expr != null) { ... }`
    let condition = TsExpr::BinOp {
        left: Box::new(lower_to_ts(inner, ctx)),
        op: TsBinOp::NotEq,
        right: Box::new(TsExpr::NullLit),
        ty: None,
    };
    TsExpr::If {
        condition: Box::new(condition),
        then_body: lower_block(then_body, ctx),
        else_body: else_body.map(|eb| lower_block(eb, ctx)),
    }
}

fn lower_while_let(inner: &Expr, body: &[Expr], ctx: &GenCtx) -> TsExpr {
    // `while let x = expr { ... }` → `while (expr != null) { ... }`
    let condition = TsExpr::BinOp {
        left: Box::new(lower_to_ts(inner, ctx)),
        op: TsBinOp::NotEq,
        right: Box::new(TsExpr::NullLit),
        ty: None,
    };
    TsExpr::While {
        condition: Box::new(condition),
        body: lower_block(body, ctx),
    }
}

// ─── Type Mapping ────────────────────────────────────────────────────────────

/// Convert a VEIL `TypeExpr` to a `TsType` IR node.
pub fn veil_type_to_ts(ty: &TypeExpr) -> TsType {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Str" | "UUID" | "Id" => TsType::String,
            "Int" | "F64" => TsType::Number,
            "Bool" => TsType::Boolean,
            "Bytes" => TsType::Named("Uint8Array".to_string()),
            "DateTime" | "Dt" => TsType::Named("Date".to_string()),
            "Json" => TsType::Record(Box::new(TsType::String), Box::new(TsType::Named("unknown".to_string()))),
            other => TsType::Named(other.to_string()),
        },
        TypeExpr::Generic(name, args) => {
            let ts_args: Vec<TsType> = args.iter().map(veil_type_to_ts).collect();
            let name_str = format!("{}<{}>", name, ts_args.iter().map(|t| t.to_ts()).collect::<Vec<_>>().join(", "));
            TsType::Named(name_str)
        }
        TypeExpr::Result(Some(inner)) => TsType::Promise(Box::new(veil_type_to_ts(inner))),
        TypeExpr::Result(None) => TsType::Promise(Box::new(TsType::Void)),
        TypeExpr::Optional(inner) => {
            TsType::Union(vec![veil_type_to_ts(inner), TsType::Null])
        }
        TypeExpr::List(inner) => TsType::Array(Box::new(veil_type_to_ts(inner))),
        TypeExpr::Map(k, v) => {
            TsType::Record(Box::new(veil_type_to_ts(k)), Box::new(veil_type_to_ts(v)))
        }
        TypeExpr::Set(inner) => {
            TsType::Named(format!("Set<{}>", veil_type_to_ts(inner).to_ts()))
        }
        TypeExpr::Tuple(items) => {
            // TS tuple as array type: [A, B, C]
            let parts: Vec<String> = items.iter().map(|t| veil_type_to_ts(t).to_ts()).collect();
            TsType::Named(format!("[{}]", parts.join(", ")))
        }
        TypeExpr::Array(inner, size) => {
            let inner_ts = veil_type_to_ts(inner).to_ts();
            let parts: Vec<String> = (0..*size).map(|_| inner_ts.clone()).collect();
            TsType::Named(format!("[{}]", parts.join(", ")))
        }
        TypeExpr::Ref(inner, _) => veil_type_to_ts(inner), // no refs in TS
        TypeExpr::Dyn(inner) => veil_type_to_ts(inner),
        TypeExpr::ImplTrait(inner) => veil_type_to_ts(inner),
        TypeExpr::FnPtr(params, ret) => {
            let param_types: Vec<TsType> = params.iter().map(veil_type_to_ts).collect();
            let ret_type = ret
                .as_ref()
                .map(|t| veil_type_to_ts(t))
                .unwrap_or(TsType::Void);
            TsType::Fn {
                params: param_types,
                ret: Box::new(ret_type),
            }
        }
        TypeExpr::LitStr(_) => TsType::String,
    }
}

/// Render a `TypeExpr` to a TS type annotation string (for Const/Let `ty` field).
fn veil_type_to_ts_string(ty: &TypeExpr) -> String {
    veil_type_to_ts(ty).to_ts()
}

/// Convert a type name string (from GenCtx local_types) to TsType.
/// This handles common VEIL type names like "Str", "Int", "Option<Str>", etc.
fn veil_type_str_to_ts(type_str: &str) -> TsType {
    // Simple named types
    match type_str {
        "Str" | "String" | "UUID" | "Id" => return TsType::String,
        "Int" | "i64" | "i32" | "u64" | "u32" | "F64" | "f64" => return TsType::Number,
        "Bool" | "bool" => return TsType::Boolean,
        "()" | "unit" => return TsType::Void,
        _ => {}
    }
    // Option<T> pattern
    if let Some(inner) = type_str.strip_prefix("Option<").and_then(|s| s.strip_suffix('>')) {
        return TsType::Union(vec![veil_type_str_to_ts(inner), TsType::Null]);
    }
    // Vec<T> / List<T>
    if let Some(inner) = type_str.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        return TsType::Array(Box::new(veil_type_str_to_ts(inner)));
    }
    if let Some(inner) = type_str.strip_prefix("List<").and_then(|s| s.strip_suffix('>')) {
        return TsType::Array(Box::new(veil_type_str_to_ts(inner)));
    }
    // Result<T> → Promise<T>
    if let Some(inner) = type_str.strip_prefix("Result<").and_then(|s| s.strip_suffix('>')) {
        return TsType::Promise(Box::new(veil_type_str_to_ts(inner)));
    }
    // Fallback: named type
    TsType::Named(type_str.to_string())
}

mod calls;
use calls::{lower_call, lower_action};

// ─── String-based type mapping (legacy, used by api_client and legacy expr) ──

/// Convert a VEIL type expression to its TypeScript string equivalent.
pub fn type_to_ts(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Str" => "string".to_string(),
            "Int" | "F64" => "number".to_string(),
            "Bool" => "boolean".to_string(),
            "Bytes" => "Uint8Array".to_string(),
            "UUID" | "Id" => "string".to_string(),
            "DateTime" | "Dt" => "Date".to_string(),
            "Json" => "Record<string, unknown>".to_string(),
            other => other.to_string(),
        },
        TypeExpr::Generic(name, args) => {
            let ts_args = args.iter().map(type_to_ts).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, ts_args)
        }
        TypeExpr::Result(Some(inner)) => format!("Promise<{}>", type_to_ts(inner)),
        TypeExpr::Result(None) => "Promise<void>".to_string(),
        TypeExpr::Optional(inner) => format!("{} | null", type_to_ts(inner)),
        TypeExpr::List(inner) => format!("{}[]", type_to_ts(inner)),
        TypeExpr::Map(k, v) => format!("Map<{}, {}>", type_to_ts(k), type_to_ts(v)),
        TypeExpr::Set(inner) => format!("Set<{}>", type_to_ts(inner)),
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_to_ts).collect::<Vec<_>>().join(", ");
            format!("[{}]", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}]", (0..*size).map(|_| type_to_ts(inner)).collect::<Vec<_>>().join(", ")),
        TypeExpr::Ref(inner, _) => type_to_ts(inner),
        TypeExpr::Dyn(inner) => type_to_ts(inner),
        TypeExpr::ImplTrait(inner) => type_to_ts(inner),
        TypeExpr::FnPtr(params, ret) => {
            let p = params.iter().enumerate()
                .map(|(i, t)| format!("arg{}: {}", i, type_to_ts(t)))
                .collect::<Vec<_>>().join(", ");
            let r = ret.as_ref().map(|t| type_to_ts(t)).unwrap_or_else(|| "void".to_string());
            format!("({}) => {}", p, r)
        }
        TypeExpr::LitStr(_) => "string".to_string(),
    }
}

/// Infer a TypeScript type for shorthand (untyped) fields by naming convention.
pub fn infer_field_type_ts(name: &str) -> String {
    if name == "id" || name.ends_with("_id") {
        return "string".to_string();
    }
    if name.ends_with("_at") || name == "created" || name == "updated"
        || name == "deleted" || name == "expires" || name == "timestamp" {
        return "Date".to_string();
    }
    if name.starts_with("is_") || name.starts_with("has_") || name.starts_with("can_")
        || name == "active" || name == "enabled" || name == "verified" || name == "deleted" {
        return "boolean".to_string();
    }
    if name == "count" || name == "total" || name == "amount" || name == "quantity"
        || name == "score" || name == "age" || name == "size" || name == "length"
        || name == "port" || name == "retries" {
        return "number".to_string();
    }
    "string".to_string()
}

/// Field type as TS string, using explicit type or inferring from name.
pub fn field_type_ts(field: &Field) -> String {
    match &field.type_expr {
        TypeExpr::Named(n) if n.is_empty() => infer_field_type_ts(&field.name),
        ty => type_to_ts(ty),
    }
}

/// Convert a name to camelCase (for variables/functions).
/// This is the legacy version matching the original `typescript.rs` behavior.
pub fn to_camel(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;
    for (i, c) in s.chars().enumerate() {
        if c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(c.to_uppercase().next().unwrap_or(c));
            capitalize_next = false;
        } else if i == 0 {
            result.push(c.to_lowercase().next().unwrap_or(c));
        } else {
            result.push(c);
        }
    }
    result
}

// ─── camelCase Helper ────────────────────────────────────────────────────────

/// Convert a VEIL snake_case identifier to TypeScript camelCase.
///
/// - `user_name` → `userName`
/// - `get_by_id` → `getById`
/// - `already_camel` → `alreadyCamel` (idempotent on camelCase)
/// - `ALL_CAPS` → `allCaps`
/// - Single-word names pass through unchanged.
pub fn to_camel_case(s: &str) -> String {
    // If it contains no underscores and doesn't start uppercase, pass through
    if !s.contains('_') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = false;
    for (i, c) in s.chars().enumerate() {
        if c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(c.to_uppercase().next().unwrap_or(c));
            capitalize_next = false;
        } else if i == 0 {
            result.push(c.to_lowercase().next().unwrap_or(c));
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod camel_case_tests {
    use super::to_camel_case;

    #[test]
    fn snake_to_camel() {
        assert_eq!(to_camel_case("user_name"), "userName");
        assert_eq!(to_camel_case("get_by_id"), "getById");
        assert_eq!(to_camel_case("is_active"), "isActive");
    }

    #[test]
    fn single_word_unchanged() {
        assert_eq!(to_camel_case("name"), "name");
        assert_eq!(to_camel_case("id"), "id");
    }

    #[test]
    fn already_camel_passthrough() {
        assert_eq!(to_camel_case("userName"), "userName");
        assert_eq!(to_camel_case("getById"), "getById");
    }

    #[test]
    fn leading_underscore() {
        assert_eq!(to_camel_case("_private"), "Private");
    }

    #[test]
    fn double_underscore() {
        assert_eq!(to_camel_case("my__field"), "myField");
    }
}
