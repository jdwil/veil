//! VEIL AST → TsExpr lowering.
//!
//! `lower_to_ts` converts a VEIL `Expr` into the typed `TsExpr` IR.
//! Unhandled expressions fall back to `TsExpr::Raw(expr_to_ts(...))` wrapping
//! the existing string-based codegen path during migration.

use veil_ir::ast::{BinOp, Expr, Pattern, StringPart, TypeExpr, UnaryOp};
use crate::expr::GenCtx;
use crate::typescript::expr_to_ts;
use super::expr::{TsBinOp, TsExpr, TsPattern, TsTemplatePart, TsType, TsUnaryOp};

// ─── Public Entry Point ──────────────────────────────────────────────────────

/// Lower a VEIL expression to a TypeScript IR node.
///
/// Handles literals, identifiers, field access, operators, bindings, and
/// simple wrappers (return, await, try, require, break, continue).
/// All other expressions fall through to the `Raw` escape hatch.
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
                Expr::Ident(n) if n == "Ok" => TsExpr::Raw("return".to_string()),
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

        // ── Fallback: delegate to old string-based codegen ───────────────
        _ => TsExpr::Raw(expr_to_ts(expr, 0)),
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
        "noop" => TsExpr::Raw(String::new()),
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

fn lower_require(inner: &Expr, _ctx: &GenCtx) -> TsExpr {
    // `require expr` → null check: if (expr == null) throw new Error("NotFound"); expr
    // Emit as the IIFE pattern: ((v) ?? (() => { throw new Error("NotFound"); })())
    // Use the raw pattern matching the old codegen for compatibility:
    let raw = expr_to_ts(&Expr::Require(Box::new(inner.clone())), 0);
    TsExpr::Raw(raw)
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
