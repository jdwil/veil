//! Typed intermediate representation for Rust expressions.
//!
//! `RustExpr` sits between VEIL AST lowering and final Rust text emission.
//! It preserves type and ownership information so downstream transforms
//! (clone insertion, `?` suppression in closures, etc.) operate on structure
//! rather than rendered strings.
//!
//! ## Migration strategy
//!
//! `RustExpr::Raw` wraps the legacy `expr_to_rust` output for unmigrated
//! expression forms. Expressions are migrated one category at a time:
//! literals → idents → field access → method calls → control flow.
//! At every step, `emit()` must produce byte-identical output to the old path.

use veil_ir::ast::Expr;
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::context::GenCtx;
use super::translate::expr_to_rust;
use super::inference::infer_expr_type;
use super::types::rust_string_lit;
use super::calls::{resolve_self_field_name, is_json_rooted_expr, is_json_type_name};

// ─── RustType ────────────────────────────────────────────────────────────────

/// Simplified Rust type representation for ownership decisions.
/// Not a full type system — just enough to decide clone vs move vs borrow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustType {
    /// Named type: "Customer", "i64", "String", "DomainError"
    Named(String),
    /// Option<T>
    Option(Box<RustType>),
    /// Result<T, DomainError>
    Result(Box<RustType>),
    /// Vec<T>
    Vec(Box<RustType>),
    /// &T (shared reference)
    Ref(Box<RustType>),
    /// ()
    Unit,
    /// serde_json::Value
    Json,
}

impl RustType {
    /// Whether this type is `Copy` (primitives, unit enums).
    pub fn is_copy(&self) -> bool {
        match self {
            RustType::Named(n) => matches!(
                n.as_str(),
                "i8" | "i16" | "i32" | "i64" | "i128"
                    | "u8" | "u16" | "u32" | "u64" | "u128"
                    | "f32" | "f64"
                    | "bool" | "char" | "()" | "usize" | "isize"
            ),
            RustType::Unit => true,
            RustType::Ref(_) => true,
            _ => false,
        }
    }

    /// Parse a simple type string into a RustType. Best-effort — complex
    /// generics fall back to Named.
    pub fn parse(s: &str) -> RustType {
        let s = s.trim();
        if s == "()" {
            return RustType::Unit;
        }
        if s == "serde_json::Value" || s == "Value" {
            return RustType::Json;
        }
        if let Some(inner) = s.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
            return RustType::Option(Box::new(RustType::parse(inner)));
        }
        if let Some(inner) = s.strip_prefix("Result<").and_then(|r| r.strip_suffix('>')) {
            // Result<T, E> — take T (first type param)
            let inner_ty = inner.split(',').next().unwrap_or(inner).trim();
            return RustType::Result(Box::new(RustType::parse(inner_ty)));
        }
        if let Some(inner) = s.strip_prefix("Vec<").and_then(|r| r.strip_suffix('>')) {
            return RustType::Vec(Box::new(RustType::parse(inner)));
        }
        if let Some(inner) = s.strip_prefix('&') {
            return RustType::Ref(Box::new(RustType::parse(inner)));
        }
        RustType::Named(s.to_string())
    }
}

// ─── RustExpr ────────────────────────────────────────────────────────────────

/// Typed intermediate representation of a Rust expression.
/// Carries enough information for ownership analysis and final emission
/// without re-parsing rendered strings.
#[derive(Debug, Clone)]
pub enum RustExpr {
    /// Raw pre-rendered string (escape hatch for unmigrated expressions).
    /// Carries the inferred Rust type if known.
    Raw { text: String, ty: Option<RustType> },

    /// Identifier reference.
    Ident { name: String, ty: Option<RustType> },

    /// String literal: `"hello"`
    StringLit(String),

    /// Integer literal: `42`
    IntLit(i64),

    /// Float literal: `3.14`
    FloatLit(f64),

    /// Boolean literal: `true` / `false`
    BoolLit(bool),

    /// Field access: `base.field`
    FieldAccess {
        base: Box<RustExpr>,
        field: String,
        ty: Option<RustType>,
    },

    /// Method call: `receiver.method(args)`
    MethodCall {
        receiver: Box<RustExpr>,
        method: String,
        args: Vec<RustExpr>,
        ty: Option<RustType>,
        is_async: bool,
        is_fallible: bool,
    },

    /// Free function call: `path::function(args)`
    FnCall {
        path: String,
        args: Vec<RustExpr>,
        ty: Option<RustType>,
    },

    /// Clone wrapper: `expr.clone()`
    Clone(Box<RustExpr>),

    /// Borrow: `&expr` or `&mut expr`
    Borrow { inner: Box<RustExpr>, mutable: bool },

    /// Await: `expr.await`
    Await(Box<RustExpr>),

    /// Try operator: `expr?`
    Try(Box<RustExpr>),

    /// `.map_err(|e| DomainError::External(format!("{e:?}")))`
    MapErr { inner: Box<RustExpr>, variant: String },

    /// `format!(...)` expression
    Format { template: String, args: Vec<RustExpr> },

    /// Block expression: `{ stmts; value }`
    Block {
        stmts: Vec<RustExpr>,
        value: Option<Box<RustExpr>>,
    },

    /// If expression
    If {
        condition: Box<RustExpr>,
        then_body: Box<RustExpr>,
        else_body: Option<Box<RustExpr>>,
    },

    /// Match expression
    Match {
        scrutinee: Box<RustExpr>,
        arms: Vec<(String, RustExpr)>,
    },

    /// Let binding: `let [mut] name [: Type] = value;`
    Let {
        name: String,
        mutable: bool,
        ty: Option<String>,
        value: Box<RustExpr>,
    },
}

// ─── emit() ──────────────────────────────────────────────────────────────────

/// Render a `RustExpr` to its final Rust source string.
///
/// This MUST produce byte-identical output to the old `expr_to_rust` for
/// every migrated expression category. Non-migrated expressions go through
/// `RustExpr::Raw` which is already a rendered string.
pub fn emit(expr: &RustExpr) -> String {
    match expr {
        RustExpr::Raw { text, .. } => text.clone(),
        RustExpr::Ident { name, .. } => name.clone(),
        RustExpr::StringLit(s) => rust_string_lit(s),
        RustExpr::IntLit(n) => n.to_string(),
        RustExpr::FloatLit(f) => f.to_string(),
        RustExpr::BoolLit(b) => b.to_string(),
        RustExpr::FieldAccess { base, field, .. } => {
            format!("{}.{}", emit(base), field)
        }
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            is_async,
            is_fallible,
            ..
        } => {
            let recv = emit(receiver);
            let arg_strs: Vec<String> = args.iter().map(emit).collect();
            let call = format!("{}.{}({})", recv, method, arg_strs.join(", "));
            let with_await = if *is_async {
                format!("{}.await", call)
            } else {
                call
            };
            if *is_fallible {
                format!("{}?", with_await)
            } else {
                with_await
            }
        }
        RustExpr::FnCall { path, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(emit).collect();
            format!("{}({})", path, arg_strs.join(", "))
        }
        RustExpr::Clone(inner) => format!("{}.clone()", emit(inner)),
        RustExpr::Borrow { inner, mutable } => {
            if *mutable {
                format!("&mut {}", emit(inner))
            } else {
                format!("&{}", emit(inner))
            }
        }
        RustExpr::Await(inner) => format!("{}.await", emit(inner)),
        RustExpr::Try(inner) => format!("{}?", emit(inner)),
        RustExpr::MapErr { inner, variant } => {
            format!(
                "{}.map_err(|e| {}(format!(\"{{e:?}}\")))?",
                emit(inner),
                variant
            )
        }
        RustExpr::Format { template, args } => {
            if args.is_empty() {
                format!("format!(\"{}\")", template)
            } else {
                let arg_strs: Vec<String> = args.iter().map(emit).collect();
                format!("format!(\"{}\", {})", template, arg_strs.join(", "))
            }
        }
        RustExpr::Block { stmts, value } => {
            let mut parts: Vec<String> = stmts.iter().map(emit).collect();
            if let Some(val) = value {
                parts.push(emit(val));
            }
            format!("{{ {} }}", parts.join("; "))
        }
        RustExpr::If {
            condition,
            then_body,
            else_body,
        } => {
            let cond = emit(condition);
            let then_str = emit(then_body);
            match else_body {
                Some(eb) => format!("if {} {{ {} }} else {{ {} }}", cond, then_str, emit(eb)),
                None => format!("if {} {{ {} }}", cond, then_str),
            }
        }
        RustExpr::Match { scrutinee, arms } => {
            let scrut = emit(scrutinee);
            let arms_str: Vec<String> = arms
                .iter()
                .map(|(pat, body)| format!("    {} => {}", pat, emit(body)))
                .collect();
            format!("match {} {{\n{}\n}}", scrut, arms_str.join(",\n"))
        }
        RustExpr::Let {
            name,
            mutable,
            ty,
            value,
        } => {
            let mut_kw = if *mutable { "mut " } else { "" };
            let ty_ann = ty
                .as_ref()
                .map(|t| format!(": {}", t))
                .unwrap_or_default();
            format!("let {}{}{} = {}", mut_kw, name, ty_ann, emit(value))
        }
    }
}

// ─── lower_to_rust (bridge) ──────────────────────────────────────────────────

/// Lower a VEIL expression to the typed `RustExpr` intermediate.
///
/// This is the new entry point that progressively replaces `expr_to_rust`.
/// Currently handles: literals, idents, field access.
/// Everything else falls through to `RustExpr::Raw` wrapping `expr_to_rust`.
pub fn lower_to_rust(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        // ── Migrated: literals ───────────────────────────────────────────
        Expr::StringLit(s) => RustExpr::StringLit(s.clone()),
        Expr::IntLit(n) => RustExpr::IntLit(*n),
        Expr::FloatLit(f) => RustExpr::FloatLit(*f),
        Expr::BoolLit(b) => RustExpr::BoolLit(*b),

        // ── Migrated: identifiers ────────────────────────────────────────
        Expr::Ident(name) => lower_ident(name, expr, ctx),

        // ── Migrated: field access ───────────────────────────────────────
        Expr::FieldAccess(base, field) => lower_field_access(base, field, expr, ctx),

        // ── Everything else: fallback to old path ────────────────────────
        _ => RustExpr::Raw {
            text: expr_to_rust(expr, ctx),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },
    }
}

/// Lower `Expr::Ident` to structured `RustExpr`.
///
/// Handles: null→None, noop→{}, state locals, self field resolution,
/// enum variant qualification. Edge cases (inline ternary fstrings,
/// unwrap_or rewrites) fall through to Raw since they're not proper idents.
fn lower_ident(name: &str, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    // VEIL null → Rust None
    if name == "null" {
        return RustExpr::Ident {
            name: "None".to_string(),
            ty: None,
        };
    }
    // VEIL noop → Rust empty block (no-op)
    if name == "noop" {
        return RustExpr::Raw {
            text: "{}".to_string(),
            ty: Some(RustType::Unit),
        };
    }
    // Edge case: inline ternary with nested f-strings from parse_fstring_parts.
    // Not a proper ident — fall through to old path.
    if name.contains(" then ") && (name.contains("f\"") || name.contains("f'")) {
        return RustExpr::Raw {
            text: expr_to_rust(expr, ctx),
            ty: None,
        };
    }
    // Edge case: unwrap_or rewrite from fstring parsing.
    if name.contains(".unwrap_or(\"") && name.ends_with("\")") {
        return RustExpr::Raw {
            text: expr_to_rust(expr, ctx),
            ty: None,
        };
    }
    // Threaded step state: read from the shared JSON bag.
    if ctx.state_locals.contains(name) {
        return RustExpr::Raw {
            text: format!("state[\"{}\"]", name),
            ty: Some(RustType::Json),
        };
    }
    // Inside a method body: resolve self fields and enum variants.
    if ctx.in_method && !ctx.locals.contains(name) {
        if let Some(rf) = resolve_self_field_name(ctx, name) {
            if rf == "pool" {
                return RustExpr::Raw {
                    text: "&self.pool".to_string(),
                    ty: None,
                };
            }
            // self.field.clone() — produces a clone of the field access
            return RustExpr::Clone(Box::new(RustExpr::FieldAccess {
                base: Box::new(RustExpr::Ident {
                    name: "self".to_string(),
                    ty: None,
                }),
                field: rf,
                ty: None,
            }));
        }
        // Uppercase non-local: try enum variant qualification
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            if let Some(enum_ty) = ctx.enum_variants.get(name) {
                return RustExpr::Ident {
                    name: format!("{enum_ty}::{name}"),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
            return RustExpr::Ident {
                name: name.to_string(),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
        }
        return RustExpr::Ident {
            name: name.to_string(),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        };
    }
    // Not in a method, or name is a local:
    // Uppercase non-local: try enum variant qualification
    if !ctx.locals.contains(name)
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    {
        if let Some(enum_ty) = ctx.enum_variants.get(name) {
            return RustExpr::Ident {
                name: format!("{enum_ty}::{name}"),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
        }
    }
    // Plain identifier
    RustExpr::Ident {
        name: name.to_string(),
        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
    }
}

/// Lower `Expr::FieldAccess` to structured `RustExpr`.
///
/// Handles: is_some/is_none predicates, state locals, self.field, enum variants,
/// stub enum variants, JSON indexing, Option auto-unwrap, and copy/clone analysis.
fn lower_field_access(base: &Expr, field: &str, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    // `opt.is_some` / `opt.is_none` (no call) → method call syntax
    if field == "is_some" || field == "is_none" {
        let base_lowered = lower_to_rust(base, ctx);
        return RustExpr::MethodCall {
            receiver: Box::new(base_lowered),
            method: field.to_string(),
            args: vec![],
            ty: Some(RustType::Named("bool".to_string())),
            is_async: false,
            is_fallible: false,
        };
    }

    // State local: index into the threaded JSON state.
    if let Expr::Ident(name) = base {
        if ctx.state_locals.contains(name.as_str()) {
            return RustExpr::Raw {
                text: format!("state[\"{}\"][\"{}\"]", name, field),
                ty: Some(RustType::Json),
            };
        }
    }

    // Method body: self.field handling
    if let Expr::Ident(name) = base {
        if name == "self" && ctx.in_method {
            let f = resolve_self_field_name(ctx, field).unwrap_or_else(|| to_snake(field));
            if f == "pool" {
                return RustExpr::Raw {
                    text: "&self.pool".to_string(),
                    ty: None,
                };
            }
            if ctx.self_fields.contains(field)
                || ctx.self_fields.contains(&f)
                || ctx.self_field_types.contains_key(&f)
            {
                return RustExpr::Clone(Box::new(RustExpr::FieldAccess {
                    base: Box::new(RustExpr::Ident {
                        name: "self".to_string(),
                        ty: None,
                    }),
                    field: f,
                    ty: None,
                }));
            }
            return RustExpr::FieldAccess {
                base: Box::new(RustExpr::Ident {
                    name: "self".to_string(),
                    ty: None,
                }),
                field: f,
                ty: None,
            };
        }
    }

    // Enum variant access: EnumName.Variant → EnumName::Variant
    if let Expr::Ident(name) = base {
        let field_is_variant = field.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);

        if matches!(ctx.name_to_shape.get(name.as_str()), Some(Shape::Enum)) {
            let variant = if field_is_variant {
                field.to_string()
            } else {
                field.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
                    + &field[1..]
            };
            return RustExpr::Ident {
                name: format!("{}::{}", name, variant),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
        }

        // Stub enums: PascalCase field access means a unit variant
        if field_is_variant {
            if let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
                return RustExpr::Ident {
                    name: format!("{}::{}::{}", crate_name, path_type, field),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
        }
        // Lowercase variant on a stub-known type (snake_case → PascalCase)
        if !field_is_variant {
            if let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
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
                return RustExpr::Ident {
                    name: format!("{}::{}::{}", crate_name, path_type, variant),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
        }
    }

    // JSON local: field access on a JSON-typed local → index syntax
    if let Expr::Ident(name) = base {
        if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
            return RustExpr::Raw {
                text: format!("{}[\"{}\"]", name, field),
                ty: Some(RustType::Json),
            };
        }
    }

    // Nested field access on a JSON value at any depth
    if is_json_rooted_expr(base, ctx) {
        let base_str = expr_to_rust(base, ctx);
        return RustExpr::Raw {
            text: format!("{}[\"{}\"]", base_str, field),
            ty: Some(RustType::Json),
        };
    }

    // Option auto-unwrap: local has type Option<X> → unwrap on field access
    if let Expr::Ident(name) = base {
        if let Some(ty) = ctx.local_type(name) {
            if ty.starts_with("Option<") {
                let base_str = expr_to_rust(base, ctx);
                let enclosing_returns_option = ctx.expected_return_rust.as_ref()
                    .map(|r| r.starts_with("Option<"))
                    .unwrap_or(false);
                if enclosing_returns_option {
                    return RustExpr::Raw {
                        text: format!("{}.clone()?.{}", base_str, to_snake(field)),
                        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                    };
                }
                return RustExpr::Raw {
                    text: format!(
                        "{}.clone().ok_or({})?.{}",
                        base_str,
                        ctx.error_model.not_found_path(),
                        to_snake(field)
                    ),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
        }
    }

    // Standard field access with copy/clone analysis
    let base_lowered = lower_to_rust(base, ctx);
    let field_snake = to_snake(field);
    let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));

    let fa = RustExpr::FieldAccess {
        base: Box::new(base_lowered),
        field: field_snake,
        ty: ty.clone(),
    };

    // Check if the field access result needs cloning
    // Json / Value is indexed in place — do not clone
    if infer_expr_type(expr, ctx).as_deref().is_some_and(is_json_type_name) {
        return fa;
    }
    // Copy types don't need clone
    if super::types::field_access_is_copy(base, field, ctx) {
        return fa;
    }
    // Already-cloned base (emitted as self.field.clone() above) doesn't need double-clone
    // For other field accesses, VEIL values are reusable: clone non-Copy fields
    RustExpr::Clone(Box::new(fa))
}

// ─── apply_ownership ─────────────────────────────────────────────────────────

/// Apply ownership semantics to a `RustExpr`: wrap in `Clone` when the value
/// is non-Copy, multi-use, and not already owned.
///
/// This is the IR-level equivalent of the old string-based `clone_for_reuse`.
/// It operates on structure rather than rendered text, making the decision
/// composable with later transforms (borrow insertion, move elision, etc.).
///
/// Call this on expressions in argument positions or assignment RHS where VEIL's
/// "values are reusable" semantics require ownership transfer / cloning.
pub fn apply_ownership(expr: RustExpr, ctx: &GenCtx) -> RustExpr {
    // Already owned / already a clone — no double-clone
    if is_already_owned(&expr) {
        return expr;
    }
    // Copy types don't need cloning
    if is_expr_copy(&expr, ctx) {
        return expr;
    }
    match &expr {
        RustExpr::Ident { name, .. } => {
            if should_clone_ident_ir(name, ctx) {
                RustExpr::Clone(Box::new(expr))
            } else {
                expr
            }
        }
        RustExpr::FieldAccess { .. } => {
            // Field accesses from lower_field_access already have Clone applied
            // where needed. But if someone calls apply_ownership on a bare
            // FieldAccess (e.g. from a different lowering path), clone it.
            RustExpr::Clone(Box::new(expr))
        }
        RustExpr::Raw { text, .. } => {
            // Already-owned strings from the old path
            if super::types::rust_already_owned(text) {
                return expr;
            }
            RustExpr::Clone(Box::new(expr))
        }
        // Literals, FnCalls, MethodCalls produce owned values — no clone needed
        _ => expr,
    }
}

/// Whether a `RustExpr` is already an owned value (clone, literal, call result).
fn is_already_owned(expr: &RustExpr) -> bool {
    matches!(
        expr,
        RustExpr::Clone(_)
            | RustExpr::StringLit(_)
            | RustExpr::IntLit(_)
            | RustExpr::FloatLit(_)
            | RustExpr::BoolLit(_)
            | RustExpr::FnCall { .. }
            | RustExpr::MethodCall { .. }
            | RustExpr::Format { .. }
            | RustExpr::Block { .. }
            | RustExpr::If { .. }
            | RustExpr::Match { .. }
    )
}

/// Whether the expression's type is Copy (primitives, unit enums, refs).
fn is_expr_copy(expr: &RustExpr, ctx: &GenCtx) -> bool {
    match expr {
        RustExpr::IntLit(_) | RustExpr::FloatLit(_) | RustExpr::BoolLit(_) => true,
        RustExpr::Ident { name, ty } => {
            // Check type annotation first
            if let Some(t) = ty {
                if t.is_copy() {
                    return true;
                }
            }
            // Check context: local type or unit enum variant
            if super::calls::is_copy_local(name, ctx) {
                return true;
            }
            super::types::is_unit_enum_variant(name, ctx)
        }
        RustExpr::FieldAccess { ty, .. } => {
            ty.as_ref().is_some_and(|t| t.is_copy())
        }
        RustExpr::Raw { ty, text, .. } => {
            if let Some(t) = ty {
                if t.is_copy() {
                    return true;
                }
            }
            // Fallback: check if the raw text is a literal
            text.parse::<i64>().is_ok() || text == "true" || text == "false"
        }
        RustExpr::Borrow { .. } => true, // refs are Copy
        _ => false,
    }
}

/// IR-level equivalent of `should_clone_ident`: decides whether an ident
/// needs cloning based on usage count, ref status, and copy type.
fn should_clone_ident_ir(name: &str, ctx: &GenCtx) -> bool {
    if super::calls::is_copy_local(name, ctx) || super::types::is_unit_enum_variant(name, ctx) {
        return false;
    }
    // Shared-ref loop element (`for x in &xs`) is `&T`. Owned slots need `.clone()`.
    if ctx.ref_elem_locals.contains(name) {
        return true;
    }
    if super::calls::is_ref_local(name, ctx) {
        return false;
    }
    // Unknown count → clone (safe). Count of 1 → last/only use → move.
    ctx.ident_uses.get(name).copied().unwrap_or(2) > 1
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_string_lit() {
        let expr = RustExpr::StringLit("hello".to_string());
        assert_eq!(emit(&expr), r#""hello""#);
    }

    #[test]
    fn emit_string_lit_escaped() {
        let expr = RustExpr::StringLit(r#"say "hi""#.to_string());
        assert_eq!(emit(&expr), r#""say \"hi\"""#);
    }

    #[test]
    fn emit_string_lit_backslash() {
        let expr = RustExpr::StringLit(r"path\to\file".to_string());
        assert_eq!(emit(&expr), r#""path\\to\\file""#);
    }

    #[test]
    fn emit_int_lit() {
        let expr = RustExpr::IntLit(42);
        assert_eq!(emit(&expr), "42");
    }

    #[test]
    fn emit_int_lit_negative() {
        let expr = RustExpr::IntLit(-7);
        assert_eq!(emit(&expr), "-7");
    }

    #[test]
    fn emit_float_lit() {
        let expr = RustExpr::FloatLit(3.14);
        assert_eq!(emit(&expr), "3.14");
    }

    #[test]
    fn emit_bool_lit() {
        assert_eq!(emit(&RustExpr::BoolLit(true)), "true");
        assert_eq!(emit(&RustExpr::BoolLit(false)), "false");
    }

    #[test]
    fn emit_raw_passthrough() {
        let expr = RustExpr::Raw {
            text: "some_complex_expr.await?".to_string(),
            ty: None,
        };
        assert_eq!(emit(&expr), "some_complex_expr.await?");
    }

    #[test]
    fn emit_ident() {
        let expr = RustExpr::Ident {
            name: "my_var".to_string(),
            ty: None,
        };
        assert_eq!(emit(&expr), "my_var");
    }

    #[test]
    fn emit_clone() {
        let expr = RustExpr::Clone(Box::new(RustExpr::Ident {
            name: "x".to_string(),
            ty: None,
        }));
        assert_eq!(emit(&expr), "x.clone()");
    }

    #[test]
    fn emit_borrow() {
        let expr = RustExpr::Borrow {
            inner: Box::new(RustExpr::Ident {
                name: "x".to_string(),
                ty: None,
            }),
            mutable: false,
        };
        assert_eq!(emit(&expr), "&x");

        let mut_expr = RustExpr::Borrow {
            inner: Box::new(RustExpr::Ident {
                name: "y".to_string(),
                ty: None,
            }),
            mutable: true,
        };
        assert_eq!(emit(&mut_expr), "&mut y");
    }

    #[test]
    fn emit_await() {
        let expr = RustExpr::Await(Box::new(RustExpr::Ident {
            name: "future".to_string(),
            ty: None,
        }));
        assert_eq!(emit(&expr), "future.await");
    }

    #[test]
    fn emit_try() {
        let expr = RustExpr::Try(Box::new(RustExpr::Ident {
            name: "result".to_string(),
            ty: None,
        }));
        assert_eq!(emit(&expr), "result?");
    }

    #[test]
    fn emit_method_call_simple() {
        let expr = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident {
                name: "self.repo".to_string(),
                ty: None,
            }),
            method: "find".to_string(),
            args: vec![RustExpr::Ident {
                name: "id".to_string(),
                ty: None,
            }],
            ty: None,
            is_async: false,
            is_fallible: false,
        };
        assert_eq!(emit(&expr), "self.repo.find(id)");
    }

    #[test]
    fn emit_method_call_async_fallible() {
        let expr = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident {
                name: "deps.repo".to_string(),
                ty: None,
            }),
            method: "save".to_string(),
            args: vec![RustExpr::Ident {
                name: "entity".to_string(),
                ty: None,
            }],
            ty: None,
            is_async: true,
            is_fallible: true,
        };
        assert_eq!(emit(&expr), "deps.repo.save(entity).await?");
    }

    #[test]
    fn emit_fn_call() {
        let expr = RustExpr::FnCall {
            path: "serde_json::from_str".to_string(),
            args: vec![RustExpr::Ident {
                name: "input".to_string(),
                ty: None,
            }],
            ty: None,
        };
        assert_eq!(emit(&expr), "serde_json::from_str(input)");
    }

    #[test]
    fn emit_let_binding() {
        let expr = RustExpr::Let {
            name: "x".to_string(),
            mutable: false,
            ty: None,
            value: Box::new(RustExpr::IntLit(42)),
        };
        assert_eq!(emit(&expr), "let x = 42");
    }

    #[test]
    fn emit_let_mut_typed() {
        let expr = RustExpr::Let {
            name: "count".to_string(),
            mutable: true,
            ty: Some("i64".to_string()),
            value: Box::new(RustExpr::IntLit(0)),
        };
        assert_eq!(emit(&expr), "let mut count: i64 = 0");
    }

    #[test]
    fn rust_type_from_str_basic() {
        assert_eq!(RustType::parse("i64"), RustType::Named("i64".to_string()));
        assert_eq!(RustType::parse("()"), RustType::Unit);
        assert_eq!(RustType::parse("serde_json::Value"), RustType::Json);
    }

    #[test]
    fn rust_type_from_str_option() {
        assert_eq!(
            RustType::parse("Option<String>"),
            RustType::Option(Box::new(RustType::Named("String".to_string())))
        );
    }

    #[test]
    fn rust_type_from_str_vec() {
        assert_eq!(
            RustType::parse("Vec<Customer>"),
            RustType::Vec(Box::new(RustType::Named("Customer".to_string())))
        );
    }

    #[test]
    fn rust_type_is_copy() {
        assert!(RustType::Named("i64".to_string()).is_copy());
        assert!(RustType::Named("bool".to_string()).is_copy());
        assert!(RustType::Unit.is_copy());
        assert!(!RustType::Named("String".to_string()).is_copy());
        assert!(!RustType::Named("Customer".to_string()).is_copy());
    }

    // ─── apply_ownership tests ───────────────────────────────────────

    fn make_ctx_with_uses(name: &str, uses: usize) -> GenCtx {
        use std::collections::HashMap;
        let mut ctx = GenCtx::new(HashMap::new());
        ctx.ident_uses.insert(name.to_string(), uses);
        ctx
    }

    #[test]
    fn ownership_clone_not_needed_for_literals() {
        let ctx = GenCtx::new(std::collections::HashMap::new());
        let expr = RustExpr::StringLit("hello".to_string());
        let result = apply_ownership(expr.clone(), &ctx);
        assert_eq!(emit(&result), emit(&expr)); // unchanged
    }

    #[test]
    fn ownership_clone_not_needed_for_copy_ident() {
        let mut ctx = make_ctx_with_uses("count", 3);
        ctx.local_types.insert("count".to_string(), "i64".to_string());
        let expr = RustExpr::Ident {
            name: "count".to_string(),
            ty: Some(RustType::Named("i64".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "count"); // no clone
    }

    #[test]
    fn ownership_clone_multi_use_ident() {
        let ctx = make_ctx_with_uses("name", 2);
        let expr = RustExpr::Ident {
            name: "name".to_string(),
            ty: Some(RustType::Named("String".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "name.clone()");
    }

    #[test]
    fn ownership_no_clone_single_use_ident() {
        let ctx = make_ctx_with_uses("name", 1);
        let expr = RustExpr::Ident {
            name: "name".to_string(),
            ty: Some(RustType::Named("String".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "name"); // last use, move
    }

    #[test]
    fn ownership_no_double_clone() {
        let ctx = make_ctx_with_uses("x", 3);
        let expr = RustExpr::Clone(Box::new(RustExpr::Ident {
            name: "x".to_string(),
            ty: None,
        }));
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "x.clone()"); // not x.clone().clone()
    }

    #[test]
    fn ownership_ref_elem_always_clones() {
        let mut ctx = make_ctx_with_uses("item", 1);
        ctx.ref_elem_locals.insert("item".to_string());
        let expr = RustExpr::Ident {
            name: "item".to_string(),
            ty: Some(RustType::Named("String".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "item.clone()");
    }

    #[test]
    fn ownership_ref_local_no_clone() {
        let mut ctx = make_ctx_with_uses("data", 3);
        ctx.local_types.insert("data".to_string(), "&str".to_string());
        let expr = RustExpr::Ident {
            name: "data".to_string(),
            ty: Some(RustType::Ref(Box::new(RustType::Named("str".to_string())))),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "data"); // refs are copy
    }
}
