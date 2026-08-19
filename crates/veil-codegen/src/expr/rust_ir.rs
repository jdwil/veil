//! Typed intermediate representation for Rust expressions.
//!
//! `RustExpr` sits between VEIL AST lowering and final Rust text emission.
//! It preserves type and ownership information so downstream transforms
//! (clone insertion, `?` suppression in closures, etc.) operate on structure
//! rather than rendered strings.
//!
//! ## Migration strategy
//!
//! The `Raw` variant wraps the legacy `expr_to_rust` output for unmigrated
//! expression forms. Expressions are migrated one category at a time:
//! literals → idents → field access → method calls → control flow.
//! At every step, `emit()` must produce byte-identical output to the old path.

use veil_ir::ast::{Expr, StringPart};
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::context::GenCtx;
use super::translate::{expr_to_rust, to_json_arg};
use super::inference::{infer_expr_type, binop_to_rust, unaryop_to_rust, normalize_match_pattern, element_type_of};
use super::types::{rust_string_lit, rust_string_lit_owned, expr_is_stringish, expr_is_numeric,
    flatten_str_add_chain, clone_if_named_value, strip_try_suffix,
    peel_option_rust, rust_ty_is_stringish, rust_ty_is_copy, rust_ty_is_unit_enum,
    expr_to_rust_value, field_access_is_copy, rust_already_owned, rust_is_copy_value,
    should_clone_ident, is_option_type, is_result_type};
use super::calls::{resolve_self_field_name, is_json_rooted_expr, is_json_type_name,
    expr_is_json, list_index_get_rust};
use super::patterns::{pattern_to_rust, pattern_to_rust_qualified, pattern_binding_names,
    emit_tracked_block, emit_value_block};
use super::actions::translate_action;
use super::analysis::analyze_mut_locals;

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

    /// `serde_json::json!({ "key": value, ... })` macro invocation.
    /// Entries are key-value pairs rendered inside the json! braces.
    /// Values that are already `RustExpr` get emitted inline (identifiers,
    /// clones, string literals, nested json! calls, vec![...], etc.).
    JsonMacro {
        entries: Vec<(String, RustExpr)>,
    },

    /// `serde_json::Value::Null`
    JsonNull,

    /// `serde_json::Value::Array(vec![])`
    JsonEmptyArray,

    /// `vec![a, b, c]` (for json! array arguments)
    VecMacro(Vec<RustExpr>),

    // ─── Structural nodes added during "complete-the-tree" ───────────

    /// Binary operation: `left op right`
    BinOp {
        left: Box<RustExpr>,
        op: String,
        right: Box<RustExpr>,
        ty: Option<RustType>,
    },

    /// Unary operation: `op expr`
    UnaryOp {
        op: String,
        expr: Box<RustExpr>,
        ty: Option<RustType>,
    },

    /// Array / Vec literal: `vec![items]`
    Array {
        items: Vec<RustExpr>,
        ty: Option<RustType>,
    },

    /// Tuple literal: `(items)`
    Tuple {
        items: Vec<RustExpr>,
        ty: Option<RustType>,
    },

    /// Index expression: `base[index]`
    Index {
        base: Box<RustExpr>,
        index: Box<RustExpr>,
        ty: Option<RustType>,
    },

    /// Struct literal: `Name { field: value, ... }`
    StructLit {
        name: String,
        fields: Vec<(String, RustExpr)>,
        ty: Option<RustType>,
    },

    /// For loop: `for binding in iterable { body }`
    For {
        binding: String,
        iterable: Box<RustExpr>,
        body: Vec<RustExpr>,
        ty: Option<RustType>,
    },

    /// While loop: `while condition { body }`
    While {
        condition: Box<RustExpr>,
        body: Vec<RustExpr>,
        ty: Option<RustType>,
    },

    /// Infinite loop: `loop { body }`
    Loop {
        body: Vec<RustExpr>,
        ty: Option<RustType>,
    },
}

// ─── emit() ──────────────────────────────────────────────────────────────────

/// Render a `RustExpr` to its final Rust source string.
///
/// This MUST produce byte-identical output to the old `expr_to_rust` for
/// every migrated expression category. Non-migrated expressions go through
/// The `Raw` variant which is already a rendered string.
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
        RustExpr::JsonMacro { entries } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("\"{}\": {}", k, emit(v)))
                .collect();
            format!("serde_json::json!({{ {} }})", parts.join(", "))
        }
        RustExpr::JsonNull => "serde_json::Value::Null".to_string(),
        RustExpr::JsonEmptyArray => "serde_json::Value::Array(vec![])".to_string(),
        RustExpr::VecMacro(items) => {
            let vals: Vec<String> = items.iter().map(emit).collect();
            format!("vec![{}]", vals.join(", "))
        }

        // ─── Structural nodes (complete-the-tree) ────────────────────────
        RustExpr::BinOp { left, op, right, .. } => {
            format!("{} {} {}", emit(left), op, emit(right))
        }
        RustExpr::UnaryOp { op, expr, .. } => {
            format!("{}{}", op, emit(expr))
        }
        RustExpr::Array { items, .. } => {
            let vals: Vec<String> = items.iter().map(emit).collect();
            format!("vec![{}]", vals.join(", "))
        }
        RustExpr::Tuple { items, .. } => {
            let parts: Vec<String> = items.iter().map(emit).collect();
            format!("({})", parts.join(", "))
        }
        RustExpr::Index { base, index, .. } => {
            format!("{}[{}]", emit(base), emit(index))
        }
        RustExpr::StructLit { name, fields, .. } => {
            if fields.is_empty() {
                format!("{} {{}}", name)
            } else {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| {
                        let val = emit(v);
                        if *k == val {
                            // Field shorthand: `name` instead of `name: name`
                            k.clone()
                        } else {
                            format!("{}: {}", k, val)
                        }
                    })
                    .collect();
                format!("{} {{ {} }}", name, field_strs.join(", "))
            }
        }
        RustExpr::For { binding, iterable, body, .. } => {
            let body_str = body.iter().map(|e| format!("    {};", emit(e))).collect::<Vec<_>>().join("\n");
            format!("for {} in {} {{\n{}\n}}", binding, emit(iterable), body_str)
        }
        RustExpr::While { condition, body, .. } => {
            let body_str = body.iter().map(|e| format!("        {};", emit(e))).collect::<Vec<_>>().join("\n");
            format!("while {} {{\n{}\n    }}", emit(condition), body_str)
        }
        RustExpr::Loop { body, .. } => {
            let body_str = body.iter().map(|e| format!("    {};", emit(e))).collect::<Vec<_>>().join("\n");
            format!("loop {{\n{}\n}}", body_str)
        }
    }
}

// ─── lower_to_rust (bridge) ──────────────────────────────────────────────────

/// Lower a VEIL expression to the typed `RustExpr` intermediate.
///
/// This is the new entry point that progressively replaces `expr_to_rust`.
/// Currently handles: literals, idents, field access.
/// Everything else falls through to a `Raw` node wrapping `expr_to_rust`.
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

        // ── Migrated: string interpolation ───────────────────────────────
        Expr::StringInterp(parts) => lower_string_interp(parts, ctx),

        // ── Migrated: binary and unary ops ───────────────────────────────
        Expr::BinaryOp(op) => {
            let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
            let l_node = lower_to_rust(&op.left, ctx);
            let r_node = lower_to_rust(&op.right, ctx);
            let l = emit(&l_node);
            let r = emit(&r_node);
            // Special case: x != None → x.is_some(), x == None → x.is_none()
            if r == "None" {
                match op.op {
                    veil_ir::ast::BinOp::NotEq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(l_node),
                            method: "is_some".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    veil_ir::ast::BinOp::Eq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(l_node),
                            method: "is_none".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    _ => {}
                }
            } else if l == "None" {
                match op.op {
                    veil_ir::ast::BinOp::NotEq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(r_node),
                            method: "is_some".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    veil_ir::ast::BinOp::Eq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(r_node),
                            method: "is_none".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    _ => {}
                }
            }
            // Vec concat: x + [items] → { let mut __v = x; __v.extend(items); __v }
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (r.starts_with("vec![") || l.starts_with("vec!["))
            {
                return RustExpr::Raw {
                    text: format!("{{ let mut __v = {l}; __v.extend({r}); __v }}"),
                    ty,
                };
            }
            // String concat: x + y → format!("{}{}", x, y)
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
                    let args = rendered.iter().map(|a| RustExpr::Raw { text: a.clone(), ty: None }).collect();
                    return RustExpr::Format { template: holes, args };
                } else {
                    let l = clone_if_named_value(&op.left, l);
                    let r = clone_if_named_value(&op.right, r);
                    return RustExpr::Format {
                        template: "{}{}".to_string(),
                        args: vec![
                            RustExpr::Raw { text: l, ty: None },
                            RustExpr::Raw { text: r, ty: None },
                        ],
                    };
                }
            }
            // Simple binary op
            RustExpr::BinOp {
                left: Box::new(l_node),
                op: binop_to_rust(&op.op).to_string(),
                right: Box::new(r_node),
                ty,
            }
        }
        Expr::UnaryOp(op) => {
            let inner_node = lower_to_rust(&op.expr, ctx);
            RustExpr::UnaryOp {
                op: unaryop_to_rust(&op.op).to_string(),
                expr: Box::new(inner_node),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: calls ──────────────────────────────────────────────
        Expr::Call(call) => lower_call(call, ctx),

        // ── Migrated: closures ───────────────────────────────────────────
        Expr::Closure { params, body } => lower_closure(params, body, ctx),

        // ── Migrated: assign ─────────────────────────────────────────────
        Expr::Assign(name, rhs, ty_ann) => {
            let text = {
                // List append sugar: `out = out + [x]` → `out.push(x)`
                if let Expr::BinaryOp(bin) = rhs.as_ref()
                    && matches!(bin.op, veil_ir::ast::BinOp::Add)
                        && let (Expr::Ident(left), Expr::ArrayLit(items)) =
                            (bin.left.as_ref(), bin.right.as_ref())
                            && left == name && items.len() == 1 {
                                let item = expr_to_rust(&items[0], ctx);
                                if let Expr::Ident(item_name) = &items[0]
                                    && let Some(ty) = ctx.local_type(item_name)
                                    && is_option_type(ty) {
                                            return RustExpr::MethodCall {
                                                receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                                method: "push".to_string(),
                                                args: vec![RustExpr::Raw {
                                                    text: format!("{}.clone().ok_or({})?", item, ctx.error_model.not_found_path()),
                                                    ty: None,
                                                }],
                                                ty: None,
                                                is_async: false,
                                                is_fallible: false,
                                            };
                                        }
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "push".to_string(),
                                    args: vec![RustExpr::Raw { text: item, ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            }
                // List concat sugar: `x = x.concat([items])` → `x.extend(vec![items])`
                if let Expr::Call(call) = rhs.as_ref() {
                    let bare_m = call.method.trim_end_matches('!');
                    if bare_m == "concat" && call.target == *name && !call.args.is_empty()
                        && let Some(Expr::ArrayLit(items)) = call.args.first() {
                            let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                            if items.len() == 1 {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "push".to_string(),
                                    args: vec![RustExpr::Raw { text: item_strs[0].clone(), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            } else {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "extend".to_string(),
                                    args: vec![RustExpr::Raw { text: format!("vec![{}]", item_strs.join(", ")), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            }
                        }
                }
                let rhs_str = match rhs.as_ref() {
                    Expr::StringLit(s) => rust_string_lit_owned(s),
                    _ => expr_to_rust(rhs, ctx),
                };
                // Field assignment: `wt.name = x`
                if name.contains('.') {
                    let parts: Vec<&str> = name.splitn(2, '.').collect();
                    let base_name = parts[0];
                    let field_path = parts[1];
                    if let Some(ty) = ctx.local_type(base_name)
                        && is_option_type(ty) {
                            let field_snake = field_path
                                .split('.')
                                .map(to_snake)
                                .collect::<Vec<_>>()
                                .join(".");
                            return RustExpr::Raw {
                                text: format!(
                                    "{}.as_mut().ok_or({})?.{} = {}",
                                    base_name, ctx.error_model.not_found_path(), field_snake, rhs_str
                                ),
                                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                            };
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
                    format!("{} = {}", path, rhs_str)
                } else if ctx.state_locals.contains(name.as_str()) {
                    format!("state[\"{}\"] = serde_json::json!({})", name, rhs_str)
                } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                    format!("self.{} = {}", to_snake(name), rhs_str)
                } else if ctx.is_local(name) {
                    // Already-declared local → reassignment
                    if let Expr::BinaryOp(bin) = rhs.as_ref()
                        && let Expr::Ident(left) = bin.left.as_ref()
                            && left == name {
                                let op_str = match bin.op {
                                    veil_ir::ast::BinOp::Add => Some("+="),
                                    veil_ir::ast::BinOp::Sub => Some("-="),
                                    veil_ir::ast::BinOp::Mul => Some("*="),
                                    _ => None,
                                };
                                if let Some(op) = op_str {
                                    let right_str = expr_to_rust(&bin.right, ctx);
                                    return RustExpr::BinOp {
                                        left: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                        op: op.to_string(),
                                        right: Box::new(RustExpr::Raw { text: right_str, ty: None }),
                                        ty: None,
                                    };
                                }
                            }
                    format!("{} = {}", name, rhs_str)
                } else {
                    let is_mutable = ctx.mut_locals.contains(name.as_str());
                    let ty_str = ty_ann.as_ref().map(|t| crate::rust::type_to_rust(t));
                    return RustExpr::Let {
                        name: name.clone(),
                        mutable: is_mutable,
                        ty: ty_str,
                        value: Box::new(RustExpr::Raw { text: rhs_str, ty: None }),
                    };
                }
            };
            RustExpr::Ident {
                name: text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: mut assign ─────────────────────────────────────────
        Expr::MutAssign(name, rhs, ty_ann) => {
            let text = {
                // List concat sugar
                if let Expr::Call(call) = rhs.as_ref() {
                    let bare_m = call.method.trim_end_matches('!');
                    if bare_m == "concat" && call.target == *name && !call.args.is_empty()
                        && let Some(Expr::ArrayLit(items)) = call.args.first() {
                            let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                            if items.len() == 1 {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "push".to_string(),
                                    args: vec![RustExpr::Raw { text: item_strs[0].clone(), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            } else {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "extend".to_string(),
                                    args: vec![RustExpr::Raw { text: format!("vec![{}]", item_strs.join(", ")), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            }
                        }
                }
                let rhs_str = match rhs.as_ref() {
                    Expr::StringLit(s) => rust_string_lit_owned(s),
                    _ => expr_to_rust(rhs, ctx),
                };
                if ctx.is_local(name) {
                    format!("{} = {}", name, rhs_str)
                } else {
                    let ty_str = ty_ann.as_ref().map(|t| crate::rust::type_to_rust(t));
                    return RustExpr::Let {
                        name: name.clone(),
                        mutable: true,
                        ty: ty_str,
                        value: Box::new(RustExpr::Raw { text: rhs_str, ty: None }),
                    };
                }
            };
            RustExpr::Ident {
                name: text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: let pattern ────────────────────────────────────────
        Expr::LetPattern(pattern, inner_expr, ty_ann) => {
            let pat_str = pattern_to_rust(pattern);
            let e = lower_to_rust(inner_expr, ctx);
            RustExpr::Let {
                name: pat_str,
                mutable: false,
                ty: ty_ann.as_ref().map(|t| crate::rust::type_to_rust(t)),
                value: Box::new(e),
            }
        }

        // ── Migrated: if expression ──────────────────────────────────────
        Expr::IfExpr(ie) => {
            let text = {
                let mut cond_ctx = ctx.clone_for_inference();
                cond_ctx.option_value_wrap = false;
                let cond = expr_to_rust(&ie.condition, &cond_ctx);
                let cond = if let Expr::Ident(name) = ie.condition.as_ref() {
                    if ctx.local_type(name) == Some("serde_json::Value") {
                        format!("{}.as_bool().unwrap_or(false)", name)
                    } else { cond }
                } else { cond };
                let then_is_stmt = matches!(
                    ie.then_body.first(),
                    Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _))
                );
                let else_is_stmt = ie.else_body.as_ref().is_some_and(|b| {
                    matches!(b.first(), Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _)))
                });
                if ie.then_body.len() == 1
                    && ie.else_body.as_ref().is_some_and(|b| b.len() == 1)
                    && !then_is_stmt
                    && !else_is_stmt
                {
                    let then_expr = expr_to_rust_value(&ie.then_body[0], ctx);
                    let else_expr = expr_to_rust_value(&ie.else_body.as_ref().unwrap()[0], ctx);
                    format!("if {} {{ {} }} else {{ {} }}", cond, then_expr, else_expr)
                } else if ctx.option_value_wrap {
                    let then_body = emit_value_block(&ie.then_body, ctx, "    ");
                    if let Some(else_body) = &ie.else_body {
                        let else_stmts = emit_value_block(else_body, ctx, "    ");
                        format!(
                            "if {} {{\n{}\n}} else {{\n{}\n}}",
                            cond, then_body, else_stmts
                        )
                    } else {
                        format!("if {} {{\n{}\n}} else {{\n    None\n}}", cond, then_body)
                    }
                } else {
                    let then_body = emit_tracked_block(&ie.then_body, ctx, "    ");
                    if let Some(else_body) = &ie.else_body {
                        let else_stmts = emit_tracked_block(else_body, ctx, "    ");
                        format!("if {} {{\n{}\n}} else {{\n{}\n}}", cond, then_body, else_stmts)
                    } else {
                        format!("if {} {{\n{}\n}}", cond, then_body)
                    }
                }
            };
            RustExpr::Raw {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: match ──────────────────────────────────────────────
        Expr::Match(scrutinee, arms) => {
            let text = {
                let mut scrut_ctx = ctx.clone_for_inference();
                scrut_ctx.option_value_wrap = false;
                let raw = expr_to_rust(scrutinee, &scrut_ctx);
                let has_string_patterns = arms.iter().any(|a| a.pattern.starts_with('"'));
                let scrutinee_str = if has_string_patterns {
                    raw.clone()
                } else {
                    strip_try_suffix(raw)
                };
                let scrutinee_str = if let Expr::Ident(name) = scrutinee.as_ref() {
                    if ctx.local_type(name) == Some("serde_json::Value") {
                        let first_pat = arms.first().map(|a| &a.pattern).cloned().unwrap_or_default();
                        let has_enum_pat = first_pat.contains("::")
                            || first_pat.contains('.')
                            || first_pat.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
                        if has_enum_pat && !first_pat.starts_with('"') && first_pat != "_" {
                            let enum_type = first_pat.split(['.', ':'])
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
                    let pattern = if let Some(rich) = &arm.rich_pattern {
                        pattern_to_rust_qualified(rich, Some(&ctx.enum_variants))
                    } else {
                        normalize_match_pattern(&arm.pattern, ctx)
                    };
                    let guard_str = match &arm.guard {
                        Some(g) => format!(" if {}", expr_to_rust(g, &scrut_ctx)),
                        None => String::new(),
                    };
                    let mut arm_ctx = ctx.clone_for_inference();
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
                let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
                let has_wildcard = arms.iter().any(|a| a.pattern == "_" || a.pattern == "else" || a.pattern.starts_with('_'));
                if has_enum_patterns && !has_wildcard {
                    out.push_str("        _ => compile_error!(\"non-exhaustive match — add missing arm or wildcard\")\n");
                }
                out.push_str("    }");
                out
            };
            RustExpr::Raw {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: for loop ───────────────────────────────────────────
        Expr::ForLoop { binding, index, iterable, body } => {
            let text = {
                let mut iter_str = expr_to_rust(iterable, ctx);
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
                let mut body_ctx = ctx.clone_for_inference();
                body_ctx.locals.insert(binding.clone());
                if let Some(elem) = element_type_of(iterable, ctx) {
                    body_ctx.local_types.insert(binding.clone(), elem);
                }
                if !elem_copy && iter_str.starts_with('&') {
                    body_ctx.ref_elem_locals.insert(binding.clone());
                }
                if let Some(idx) = index {
                    body_ctx.locals.insert(idx.clone());
                }
                body_ctx.mut_locals.extend(analyze_mut_locals(body));
                let body_str = emit_tracked_block(body, &body_ctx, "    ");
                let enumerate = if index.is_some() { ".enumerate()" } else { "" };
                let iter_expr = if let Expr::Ident(name) = iterable.as_ref() {
                    if ctx
                        .local_type(name)
                        .map(is_option_type)
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
            };
            RustExpr::Raw {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: while loop ─────────────────────────────────────────
        Expr::WhileLoop { condition, body } => {
            let text = {
                let cond_str = expr_to_rust(condition, ctx);
                let mut body_ctx = ctx.clone_for_inference();
                body_ctx.mut_locals.extend(analyze_mut_locals(body));
                let mut lines = Vec::new();
                for e in body {
                    let line = expr_to_rust(e, &body_ctx);
                    if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = e
                        && !name.contains('.') {
                            body_ctx.locals.insert(name.clone());
                        }
                    lines.push(format!("        {};", line));
                }
                format!("while {} {{\n{}\n    }}", cond_str, lines.join("\n"))
            };
            RustExpr::Raw {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: loop ───────────────────────────────────────────────
        Expr::Loop(body) => {
            let body_exprs: Vec<RustExpr> = body.iter().map(|e| {
                lower_to_rust(e, ctx)
            }).collect();
            RustExpr::Loop {
                body: body_exprs,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: return ─────────────────────────────────────────────
        Expr::Return(inner) => {
            let text = match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => "return Ok(())".to_string(),
                Expr::Ident(n) if n == "Err" => {
                    format!("return Err({}(\"error\".to_string()))", ctx.error_model.external_path())
                }
                Expr::Call(c) if c.target == "Err" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust_value(e, ctx)).collect::<Vec<_>>().join(", ");
                    let err_type = &ctx.error_model.type_name;
                    if a.is_empty() {
                        format!("return Err({}(\"error\".to_string()))", ctx.error_model.validation_path())
                    } else if a.starts_with(&format!("{err_type}::")) {
                        format!("return Err({})", a)
                    } else {
                        let is_simple_ident = c.args.len() == 1 && matches!(&c.args[0], Expr::Ident(_));
                        if is_simple_ident {
                            format!("return Err({})", a)
                        } else {
                            // String literals, format!, computed messages → External
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
                    let returns_result = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(is_result_type)
                        .unwrap_or(true);
                    let returns_option = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(|t| t.contains("Option<"))
                        .unwrap_or(false);
                    if !returns_result {
                        if val == "None" {
                            if returns_option {
                                "return None".to_string()
                            } else {
                                // Non-Option API with null: should be a check-time diagnostic
                                "compile_error!(\"null return on non-Option function\")"
                                    .to_string()
                            }
                        } else if returns_option && !val.starts_with("Some(") {
                            format!("return Some({})", val)
                        } else {
                            format!("return {}", val)
                        }
                    } else if val == "None" || val == "()" {
                        if returns_option {
                            "return Ok(None)".to_string()
                        } else if val == "()" {
                            "return Ok(())".to_string()
                        } else {
                            format!("return Err({})", ctx.error_model.not_found_path())
                        }
                    } else if returns_option && !val.starts_with("Some(") {
                        if let Expr::Ident(name) = inner.as_ref()
                            && ctx.local_type(name).map(is_option_type).unwrap_or(false) {
                                return RustExpr::Raw {
                                    text: format!("return Ok({})", val),
                                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                                };
                            }
                        format!("return Ok(Some({}))", val)
                    } else {
                        format!("return Ok({})", val)
                    }
                }
            };
            RustExpr::Raw {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: break / continue ───────────────────────────────────
        Expr::Break => RustExpr::Ident {
            name: "break".to_string(),
            ty: Some(RustType::Unit),
        },
        Expr::Continue => RustExpr::Ident {
            name: "continue".to_string(),
            ty: Some(RustType::Unit),
        },

        // ── Migrated: array literal ──────────────────────────────────────
        Expr::ArrayLit(items) => {
            let item_exprs: Vec<RustExpr> = items.iter().map(|e| {
                lower_to_rust(e, ctx)
            }).collect();
            RustExpr::Array {
                items: item_exprs,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: tuple ──────────────────────────────────────────────
        Expr::Tuple(items) => {
            let item_exprs: Vec<RustExpr> = items.iter().map(|e| {
                lower_to_rust(e, ctx)
            }).collect();
            RustExpr::Tuple {
                items: item_exprs,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: await ──────────────────────────────────────────────
        Expr::Await(inner) => {
            RustExpr::Await(Box::new(lower_to_rust(inner, ctx)))
        }

        // ── Migrated: try ────────────────────────────────────────────────
        Expr::Try(inner) => {
            RustExpr::Try(Box::new(lower_to_rust(inner, ctx)))
        }

        // ── Migrated: require ────────────────────────────────────────────
        Expr::Require(inner) => {
            let s = expr_to_rust(inner, ctx);
            let ty = infer_expr_type(inner, ctx);
            let text = if expr_is_json(inner, ctx)
                || ty.as_deref().is_some_and(is_json_type_name)
            {
                format!(
                    "{s}.as_str().map(|s| s.to_string()).ok_or({})?", ctx.error_model.not_found_path()
                )
            } else {
                let still_option = ty.as_deref().is_some_and(|t| peel_option_rust(t).is_some());
                if still_option {
                    format!("{s}.ok_or({})?", ctx.error_model.not_found_path())
                } else if s.trim_end().ends_with('?')
                    || ty.as_deref().is_some_and(|t| {
                        rust_ty_is_stringish(t) || t == "i64" || t == "bool" || t.starts_with("Vec<")
                    })
                {
                    // Already present (trailing ?) or a known-present scalar type
                    s
                } else {
                    format!("{s}.ok_or({})?", ctx.error_model.not_found_path())
                }
            };
            RustExpr::Ident {
                name: text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: index ──────────────────────────────────────────────
        Expr::Index(base, idx) => {
            let b = expr_to_rust(base, ctx);
            let text = match idx.as_ref() {
                Expr::StringLit(s) => format!(
                    "{b}.get(\"{s}\").cloned().ok_or({})?", ctx.error_model.not_found_path()
                ),
                Expr::IntLit(n) => list_index_get_rust(&b, &n.to_string(), base, ctx),
                other => {
                    let i = expr_to_rust(other, ctx);
                    let base_ty = match base.as_ref() {
                        Expr::Ident(n) => ctx.local_type(n).unwrap_or(""),
                        _ => "",
                    };
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
                        format!(
                            "{b}.get({i}.as_str()).cloned().unwrap_or(serde_json::Value::Null)"
                        )
                    } else {
                        format!("{b}[{i}]")
                    }
                }
            };
            RustExpr::Ident {
                name: text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: action ─────────────────────────────────────────────
        Expr::Action(a) => RustExpr::Raw {
            text: translate_action(a, ctx),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },

        // ── Migrated: range ──────────────────────────────────────────────
        Expr::Range { start, end, inclusive } => {
            let s = start.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default();
            let e = end.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default();
            let op = if *inclusive { "..=" } else { ".." };
            RustExpr::Ident {
                name: format!("{}{}{}", s, op, e),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: cast ───────────────────────────────────────────────
        Expr::Cast(inner_expr, ty) => {
            RustExpr::Ident {
                name: format!("{} as {}", expr_to_rust(inner_expr, ctx), ty),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: struct literal ─────────────────────────────────────
        Expr::StructLit(name, fields) => {
            let text = if name.is_empty() {
                // Anonymous record/map literal → JSON object value
                if fields.is_empty() {
                    "serde_json::json!({})".to_string()
                } else {
                    let pairs = fields.iter().map(|(k, v)| {
                        format!("\"{}\": {}", k, to_json_arg(v, ctx))
                    }).collect::<Vec<_>>().join(", ");
                    format!("serde_json::json!({{ {} }})", pairs)
                }
            } else {
                let fs = fields.iter().map(|(k, v)| {
                    let v_str = expr_to_rust(v, ctx);
                    let cloned = match v {
                        Expr::StringLit(s) => rust_string_lit_owned(s),
                        _ => {
                            if rust_already_owned(&v_str) || rust_is_copy_value(v, &v_str, ctx) {
                                v_str.clone()
                            } else {
                                match v {
                                    Expr::Ident(n) if should_clone_ident(n, ctx) => format!("{v_str}.clone()"),
                                    Expr::FieldAccess(_, _) => format!("{v_str}.clone()"),
                                    _ => v_str.clone(),
                                }
                            }
                        }
                    };
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
                                t if is_option_type(t) => format!("Some({})", cloned),
                                _ => cloned,
                            }
                        } else if field_ty == "serde_json::Value" || field_ty == "Option<serde_json::Value>" {
                            if is_option_type(field_ty) {
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
                    let field = to_snake(k);
                    if coerced == field || coerced == *k {
                        coerced
                    } else {
                        format!("{field}: {coerced}")
                    }
                }).collect::<Vec<_>>().join(", ");
                format!("{} {{ {} }}", name, fs)
            };
            RustExpr::Ident {
                name: text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: struct update ──────────────────────────────────────
        Expr::StructUpdate { name, fields, base } => {
            let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx))).collect::<Vec<_>>().join(", ");
            RustExpr::Ident {
                name: format!("{} {{ {}, ..{} }}", name, fs, expr_to_rust(base, ctx)),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: if let ─────────────────────────────────────────────
        Expr::IfLet { pattern, expr: inner_expr, then_body, else_body } => {
            let e = expr_to_rust(inner_expr, ctx);
            let then_str = then_body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            let else_str = else_body.as_ref().map(|eb| { let s = eb.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n"); format!(" else {{\n{}\n}}", s) }).unwrap_or_default();
            RustExpr::Ident {
                name: format!("if let {} = {} {{\n{}\n}}{}", pattern, e, then_str, else_str),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: while let ──────────────────────────────────────────
        Expr::WhileLet { pattern, expr: inner_expr, body } => {
            let e = expr_to_rust(inner_expr, ctx);
            let body_str = body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            RustExpr::Ident {
                name: format!("while let {} = {} {{\n{}\n}}", pattern, e, body_str),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: do block ───────────────────────────────────────────
        Expr::DoBlock(body) => {
            let text = if body.is_empty() {
                "{}".to_string()
            } else {
                let mut block_ctx = ctx.clone_for_inference();
                let mut lines = Vec::new();
                for (i, e) in body.iter().enumerate() {
                    let rust = expr_to_rust(e, &block_ctx);
                    if let Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) = e
                        && !name.contains('.') {
                            block_ctx.locals.insert(name.clone());
                            if let Some(ty) = ty_ann {
                                block_ctx.local_types.insert(name.clone(), crate::rust::type_to_rust(ty));
                            } else if let Some(t) = infer_expr_type(rhs, &block_ctx) {
                                block_ctx.local_types.insert(name.clone(), t);
                            }
                        }
                    if i == body.len() - 1 {
                        lines.push(format!("    {}", rust));
                    } else {
                        lines.push(format!("    {};", rust));
                    }
                }
                format!("{{\n{}\n}}", lines.join("\n"))
            };
            RustExpr::Ident {
                name: text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: stock ──────────────────────────────────────────────
        Expr::Stock => RustExpr::Ident {
            name: "/* error: stock not expanded */ ()".to_string(),
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
        return RustExpr::Ident {
            name: "{}".to_string(),
            ty: Some(RustType::Unit),
        };
    }
    // Edge case: inline ternary with nested f-strings from parse_fstring_parts.
    // Not a proper ident — handled directly here.
    if name.contains(" then ") && (name.contains("f\"") || name.contains("f'")) {
        return RustExpr::Ident {
            name: super::translate::translate_inline_ternary_fstring(name),
            ty: None,
        };
    }
    // Edge case: unwrap_or rewrite from fstring parsing.
    if name.contains(".unwrap_or(\"") && name.ends_with("\")") {
        // Transform: x.unwrap_or("text") → x.unwrap_or("text".to_string())
        let converted = name.replacen("\")", "\".to_string())", 1);
        return RustExpr::Ident {
            name: converted,
            ty: None,
        };
    }
    // Threaded step state: read from the shared JSON bag.
    if ctx.state_locals.contains(name) {
        return RustExpr::Index {
            base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
            index: Box::new(RustExpr::StringLit(name.to_string())),
            ty: Some(RustType::Json),
        };
    }
    // Inside a method body: resolve self fields and enum variants.
    if ctx.in_method && !ctx.locals.contains(name) {
        if let Some(rf) = resolve_self_field_name(ctx, name) {
            if ctx.borrow_fields.contains(rf.as_str()) {
                return RustExpr::Borrow {
                    inner: Box::new(RustExpr::FieldAccess {
                        base: Box::new(RustExpr::Ident { name: "self".to_string(), ty: None }),
                        field: rf,
                        ty: None,
                    }),
                    mutable: false,
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
        && let Some(enum_ty) = ctx.enum_variants.get(name) {
            return RustExpr::Ident {
                name: format!("{enum_ty}::{name}"),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
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
    if let Expr::Ident(name) = base
        && ctx.state_locals.contains(name.as_str()) {
            return RustExpr::Index {
                base: Box::new(RustExpr::Index {
                    base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
                    index: Box::new(RustExpr::StringLit(name.clone())),
                    ty: Some(RustType::Json),
                }),
                index: Box::new(RustExpr::StringLit(field.to_string())),
                ty: Some(RustType::Json),
            };
        }

    // Method body: self.field handling
    if let Expr::Ident(name) = base
        && name == "self" && ctx.in_method {
            let f = resolve_self_field_name(ctx, field).unwrap_or_else(|| to_snake(field));
            if ctx.borrow_fields.contains(f.as_str()) {
                return RustExpr::Borrow {
                    inner: Box::new(RustExpr::FieldAccess {
                        base: Box::new(RustExpr::Ident { name: "self".to_string(), ty: None }),
                        field: f,
                        ty: None,
                    }),
                    mutable: false,
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
        if field_is_variant
            && let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
                return RustExpr::Ident {
                    name: format!("{}::{}::{}", crate_name, path_type, field),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
        // Lowercase variant on a stub-known type (snake_case → PascalCase)
        if !field_is_variant
            && let Some((crate_name, path_type)) = ctx.stub_type_crate.get(name.as_str()) {
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

    // JSON local: field access on a JSON-typed local → index syntax
    if let Expr::Ident(name) = base
        && ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
            return RustExpr::Index {
                base: Box::new(RustExpr::Ident { name: name.clone(), ty: Some(RustType::Json) }),
                index: Box::new(RustExpr::StringLit(field.to_string())),
                ty: Some(RustType::Json),
            };
        }

    // Nested field access on a JSON value at any depth
    if is_json_rooted_expr(base, ctx) {
        let base_node = lower_to_rust(base, ctx);
        return RustExpr::Index {
            base: Box::new(base_node),
            index: Box::new(RustExpr::StringLit(field.to_string())),
            ty: Some(RustType::Json),
        };
    }

    // Option auto-unwrap: local has type Option<X> → unwrap on field access
    if let Expr::Ident(name) = base
        && let Some(ty) = ctx.local_type(name)
            && is_option_type(ty) {
                let base_str = expr_to_rust(base, ctx);
                let enclosing_returns_option = ctx.expected_return_rust.as_deref()
                    .map(is_option_type)
                    .unwrap_or(false);
                if enclosing_returns_option {
                    return RustExpr::Ident {
                        name: format!("{}.clone()?.{}", base_str, to_snake(field)),
                        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                    };
                }
                return RustExpr::Ident {
                    name: format!(
                        "{}.clone().ok_or({})?.{}",
                        base_str,
                        ctx.error_model.not_found_path(),
                        to_snake(field)
                    ),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
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
    if field_access_is_copy(base, field, ctx) {
        return fa;
    }
    // Already-cloned base (emitted as self.field.clone() above) doesn't need double-clone
    // For other field accesses, VEIL values are reusable: clone non-Copy fields
    RustExpr::Clone(Box::new(fa))
}

// ─── lower_string_interp ──────────────────────────────────────────────────────

/// Lower `Expr::StringInterp` (f-strings) to `RustExpr::Format`.
///
/// Mirrors the logic in translate.rs: literal chars get `{`/`}` escaped for
/// `format!`, expression parts become `{}` holes with args.
/// When there are no expression parts, the result is just a string literal.
fn lower_string_interp(parts: &[StringPart], ctx: &GenCtx) -> RustExpr {
    let mut fmt = String::new();
    let mut args: Vec<RustExpr> = Vec::new();
    for p in parts {
        match p {
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
                // Args in format!() are by-reference (Display trait) — do NOT
                // apply ownership analysis here; cloning would be wasteful.
                args.push(lower_to_rust(e, ctx));
            }
        }
    }
    if args.is_empty() {
        // No interpolations — just a string literal that needs .to_string()
        let raw: String = parts
            .iter()
            .filter_map(|p| match p {
                StringPart::Literal(l) => Some(l.as_str()),
                _ => None,
            })
            .collect();
        // Emit as MethodCall on StringLit to produce the `"...".to_string()` form
        RustExpr::MethodCall {
            receiver: Box::new(RustExpr::StringLit(raw)),
            method: "to_string".to_string(),
            args: vec![],
            ty: Some(RustType::Named("String".to_string())),
            is_async: false,
            is_fallible: false,
        }
    } else {
        RustExpr::Format { template: fmt, args }
    }
}

// ─── Bus routing IR (structured envelope/message builders) ───────────────────

/// Convert a VEIL expression to a `RustExpr` for use in JSON argument positions
/// (inside `serde_json::json!({...})` envelopes and messages).
///
/// Mirrors the logic of `to_json_arg()` in translate.rs but produces structured
/// nodes instead of formatted strings.
fn to_json_arg_ir(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::Ident(name) => {
            // VEIL null → JSON null
            if name == "null" {
                return RustExpr::JsonNull;
            }
            // Shared step-state value → state["name"].clone()
            if ctx.state_locals.contains(name.as_str()) {
                return RustExpr::Clone(Box::new(RustExpr::Index {
                    base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
                    index: Box::new(RustExpr::StringLit(name.clone())),
                    ty: Some(RustType::Json),
                }));
            }
            // Struct-captured input (step impl) → self.<field>.clone()
            if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                return RustExpr::Clone(Box::new(RustExpr::FieldAccess {
                    base: Box::new(RustExpr::Ident { name: "self".to_string(), ty: None }),
                    field: to_snake(name),
                    ty: None,
                }));
            }
            // Local variable → name.clone()
            if ctx.is_local(name) {
                return RustExpr::Clone(Box::new(RustExpr::Ident {
                    name: name.clone(),
                    ty: None,
                }));
            }
            // Non-local bare ident → symbolic string (enum variant, marker)
            RustExpr::StringLit(name.clone())
        }
        Expr::FieldAccess(base, field) => {
            // Field of a state-local → state["name"]["field"].clone()
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return RustExpr::Clone(Box::new(RustExpr::Index {
                        base: Box::new(RustExpr::Index {
                            base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
                            index: Box::new(RustExpr::StringLit(name.clone())),
                            ty: Some(RustType::Json),
                        }),
                        index: Box::new(RustExpr::StringLit(field.clone())),
                        ty: Some(RustType::Json),
                    }));
                }
                // serde_json::Value local → name["field"].clone()
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return RustExpr::Clone(Box::new(RustExpr::Index {
                        base: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                        index: Box::new(RustExpr::StringLit(field.clone())),
                        ty: Some(RustType::Json),
                    }));
                }
            }
            // Otherwise serialize base then index
            let base_ir = to_json_arg_ir(base, ctx);
            RustExpr::Clone(Box::new(RustExpr::Index {
                base: Box::new(RustExpr::FnCall {
                    path: "serde_json::json!".to_string(),
                    args: vec![base_ir],
                    ty: Some(RustType::Json),
                }),
                index: Box::new(RustExpr::StringLit(field.to_string())),
                ty: Some(RustType::Json),
            }))
        }
        Expr::ArrayLit(items) if items.is_empty() => RustExpr::JsonEmptyArray,
        Expr::ArrayLit(items) => {
            let vals: Vec<RustExpr> = items.iter().map(|e| to_json_arg_ir(e, ctx)).collect();
            RustExpr::VecMacro(vals)
        }
        _ => {
            // Fall back to recursive lowering
            lower_to_rust(expr, ctx)
        }
    }
}

/// Build a structured `RustExpr::JsonMacro` for a named message payload
/// (desugared `invoke MessageType { field: val, ... }`).
///
/// Emits: `serde_json::json!({ "type": "Name", "field1": val1, ... })`
fn json_message_ir(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> RustExpr {
    let mut entries: Vec<(String, RustExpr)> = Vec::with_capacity(fields.len() + 1);
    entries.push(("type".to_string(), RustExpr::StringLit(name.to_string())));
    for (k, v) in fields {
        entries.push((k.clone(), to_json_arg_ir(v, ctx)));
    }
    RustExpr::JsonMacro { entries }
}

/// Build a structured `RustExpr::JsonMacro` for a cross-boundary JSON envelope.
///
/// Emits: `serde_json::json!({ "target": "T", "method": "m", "args": [...] })`
fn json_envelope_ir(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> RustExpr {
    let arg_vals: Vec<RustExpr> = args.iter().map(|a| to_json_arg_ir(a, ctx)).collect();
    let entries = vec![
        ("target".to_string(), RustExpr::StringLit(target.to_string())),
        ("method".to_string(), RustExpr::StringLit(method.to_string())),
        ("args".to_string(), RustExpr::VecMacro(arg_vals)),
    ];
    RustExpr::JsonMacro { entries }
}

/// Attempt to lower a bus routing call to structured `RustExpr`.
///
/// Handles three paths:
/// 1. Routing trait calls (`ctx.routing_traits`) with json_message/envelope args
/// 2. Typed bus decode (invoke/request with known return type → from_value)
/// 3. Envelope routing (cross-boundary calls via `routing_ref.invoke(envelope)`)
///
/// Returns `Some(RustExpr)` if the call was handled, `None` to fall through.
fn lower_call_bus_routing(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    use super::inference::{bus_message_name_from_args, bus_return_type_in_scope};

    // ── Path 1 & 2: Trait-shaped target with routing_traits ──────────────────
    if ctx.is_trait_target(&call.target) && ctx.routing_traits.contains(&call.target) {
        let dep_name = ctx.deps_field_for(&call.target);
        let method = if call.method.is_empty() { "call" } else { &call.method };

        // Build the JSON payload (sugar path or direct call)
        let payload_ir = if call.sugar.is_some() {
            match call.args.first() {
                Some(Expr::StructLit(name, fields)) => json_message_ir(name, fields, ctx),
                Some(Expr::Ident(evt)) => {
                    // Simple event identifier → json!({ "type": "EventName" })
                    let entries = vec![
                        ("type".to_string(), RustExpr::StringLit(evt.clone())),
                    ];
                    RustExpr::JsonMacro { entries }
                }
                _ => json_envelope_ir(&call.target, method, &call.args, ctx),
            }
        } else {
            // Direct routing-trait call: these use clone_args style, not JSON envelope.
            // This sub-path does NOT produce a JSON envelope — it passes args directly.
            // Fall through to the old path for now; only sugar-based routing uses envelopes.
            return None;
        };

        // Build the receiver reference
        let rref = if ctx.routing_ref.is_empty() {
            format!("deps.{}", dep_name)
        } else {
            ctx.routing_ref.clone()
        };
        let bare = to_snake(method);

        // The base call: routing_ref.method(payload).await?
        let base_call = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident { name: rref, ty: None }),
            method: bare.clone(),
            args: vec![payload_ir],
            ty: Some(RustType::Json),
            is_async: true,
            is_fallible: true,
        };

        // Path 2: Typed bus decode for invoke/request with known return type
        if matches!(bare.as_str(), "invoke" | "request") {
            let decode = bus_message_name_from_args(&call.args)
                .and_then(|msg| ctx.bus_returns.get(&msg).map(|ret| (msg, ret.clone())));
            if let Some((_msg, ref ret)) = decode.filter(|(_, r)| bus_return_type_in_scope(ctx, r)) {
                // serde_json::from_value::<RetType>(call_expr).map_err(|e| Error::External(...))?
                let from_value = RustExpr::FnCall {
                    path: format!("serde_json::from_value::<{}>", ret),
                    args: vec![base_call],
                    ty: Some(RustType::parse(ret)),
                };
                return Some(RustExpr::MapErr {
                    inner: Box::new(from_value),
                    variant: ctx.error_model.external_path(),
                });
            }
        }

        return Some(base_call);
    }

    // ── Path 3: Envelope routing (cross-boundary calls) ──────────────────────
    let is_lang_target = matches!(
        call.target.as_str(),
        "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "Int" | "UUID"
    );
    let is_typed_local = ctx.is_local(&call.target) && ctx.local_type(&call.target).is_some();
    if ctx.envelope_routing
        && !is_lang_target
        && !is_typed_local
        && !ctx.stub_pkg_crate.contains_key(&call.target)
        && (ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty())
    {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        let rref = if ctx.routing_ref.is_empty() {
            "deps".to_string()
        } else {
            ctx.routing_ref.clone()
        };
        let envelope = json_envelope_ir(&call.target, method, &call.args, ctx);
        let invoke_call = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident { name: rref, ty: None }),
            method: "invoke".to_string(),
            args: vec![envelope],
            ty: Some(RustType::Json),
            is_async: true,
            is_fallible: true,
        };
        return Some(invoke_call);
    }

    None
}

// ─── Builder chain lowering ──────────────────────────────────────────────────

/// Methods that require custom pre-processing in translate_call and cannot be
/// structurally lowered as simple MethodCall nodes in a builder chain.
const SPECIAL_METHODS: &[&str] = &[
    "get", "as_str", "as_s", "as_n", "trim", "unwrap", "unwrap_or",
    "unwrap_or_else", "body", "limit", "parse_int", "parse_json", "first",
];

/// Check if a method (after stripping `!`/`?`) is in the special-case list.
fn is_special_method(method: &str) -> bool {
    let bare = method.trim_end_matches(['!', '?']);
    SPECIAL_METHODS.contains(&bare)
}

/// Attempt to lower a receiver-based method call (builder chain) to structured
/// `RustExpr::MethodCall` nodes.
///
/// Intercepts calls where:
/// - The call has a receiver (it's `receiver.method(args)`)
/// - The receiver is itself a Call (chained builder pattern)
/// - The terminal method is not in the special-case list
///
/// Each method in the chain becomes a nested `MethodCall`. The terminal method
/// gets its async/fallible flags from `receiver_call_suffix`. If the suffix
/// includes a `.map_err(...)`, the result is wrapped in `RustExpr::MapErr`.
///
/// Returns `Some(RustExpr)` if handled, `None` to fall through.
fn lower_call_builder_chain(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    use super::calls::{receiver_call_suffix, clone_args_for_typed_method, rust_method_name};

    // Only handle calls with a receiver that is itself a Call (chained)
    let recv = call.receiver.as_ref()?;
    if !matches!(recv.as_ref(), Expr::Call(_)) {
        return None;
    }

    // Skip special methods that need custom handling in translate_call
    if is_special_method(&call.method) {
        return None;
    }

    // Get the suffix to determine async/fallible for the terminal method
    let suffix = receiver_call_suffix(recv, &call.method, ctx);

    // Only intercept when the terminal method has a meaningful suffix
    // (async, fallible, or both). Plain builder intermediates with no suffix
    // could still be part of a chain that ends with .send() — but those are
    // handled when the send() call is the outer Call and this intermediate
    // is the receiver.
    if suffix.is_empty() {
        return None;
    }

    // Parse suffix into structural flags
    let (is_async, is_fallible, needs_map_err, owns_str) = parse_suffix(&suffix);

    // Build the receiver chain recursively
    let receiver_ir = lower_chain_receiver(recv, ctx);

    // Build args (rendered as Raw strings matching clone_args_for_typed_method)
    let method_name = rust_method_name(&call.method);
    let recv_lookup: Option<&str> = match recv.as_ref() {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::FieldAccess(_, field) => Some(field.as_str()),
        _ => None,
    };
    let args_str = clone_args_for_typed_method(recv_lookup, &call.method, &call.args, ctx);
    let args_ir = if args_str.is_empty() {
        vec![]
    } else {
        vec![RustExpr::Raw { text: args_str, ty: None }]
    };

    // Build the terminal method call
    let method_call = RustExpr::MethodCall {
        receiver: Box::new(receiver_ir),
        method: method_name,
        args: args_ir,
        ty: infer_call_type_from_ctx(call, ctx),
        is_async,
        is_fallible: is_fallible && !needs_map_err,
    };

    // Wrap in MapErr if the suffix demands it
    if needs_map_err {
        let variant = if owns_str {
            // map(|s| s.to_string()).map_err(...) — too complex for structured.
            // Fall through to Raw for this edge case.
            return None;
        } else {
            ctx.error_model.external_path()
        };
        return Some(RustExpr::MapErr {
            inner: Box::new(method_call),
            variant,
        });
    }

    Some(method_call)
}

/// Parse a receiver_call_suffix string into (is_async, is_fallible, needs_map_err, owns_str).
fn parse_suffix(suffix: &str) -> (bool, bool, bool, bool) {
    if suffix.contains(".await") && suffix.contains("map_err") {
        // .await.map_err(|e| ...)? → async, fallible via MapErr
        (true, true, true, false)
    } else if suffix == ".await?" {
        (true, true, false, false)
    } else if suffix == ".await" {
        (true, false, false, false)
    } else if suffix.contains("map(|s| s.to_string())") {
        // .map(|s| s.to_string()).map_err(...)? → sync, owns string, needs special handling
        (false, true, true, true)
    } else if suffix.contains("map_err") {
        // .map_err(|e| ...)? → sync fallible via MapErr
        (false, true, true, false)
    } else if suffix.ends_with('?') {
        (false, true, false, false)
    } else {
        (false, false, false, false)
    }
}

/// Recursively lower a chain receiver (Call → nested MethodCalls).
/// Non-Call receivers (Ident, FieldAccess) are lowered via lower_to_rust.
fn lower_chain_receiver(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::Call(inner_call) => {
            // Check if this inner call also has a receiver (deeper chain)
            if let Some(inner_recv) = &inner_call.receiver {
                // Skip special methods — fall back for the whole sub-chain
                if is_special_method(&inner_call.method) {
                    return RustExpr::Ident {
                        name: expr_to_rust(expr, ctx),
                        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                    };
                }
                let receiver_ir = lower_chain_receiver(inner_recv, ctx);
                let method_name = super::calls::rust_method_name(&inner_call.method);
                let recv_lookup: Option<&str> = match inner_recv.as_ref() {
                    Expr::Ident(name) => Some(name.as_str()),
                    Expr::FieldAccess(_, field) => Some(field.as_str()),
                    _ => None,
                };
                let args_str = super::calls::clone_args_for_typed_method(
                    recv_lookup, &inner_call.method, &inner_call.args, ctx,
                );
                let args_ir = if args_str.is_empty() {
                    vec![]
                } else {
                    vec![RustExpr::Raw { text: args_str, ty: None }]
                };
                // Intermediate chain methods have no async/fallible suffix
                RustExpr::MethodCall {
                    receiver: Box::new(receiver_ir),
                    method: method_name,
                    args: args_ir,
                    ty: None,
                    is_async: false,
                    is_fallible: false,
                }
            } else {
                // Call without a receiver (e.g. `client.query()` where client is the target)
                // This is the chain root — render it since it may need
                // target-based resolution (struct constructor, free fn, etc.)
                RustExpr::Ident {
                    name: expr_to_rust(expr, ctx),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                }
            }
        }
        // Non-Call receivers: use lower_to_rust for idents/field access
        _ => lower_to_rust(expr, ctx),
    }
}

/// Infer call type from context without rendering (avoids double translate_call).
fn infer_call_type_from_ctx(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustType> {
    let method_key = call.method.trim_end_matches(['!', '?']);
    if let Some(ret) = (!call.target.is_empty())
        .then(|| ctx.method_returns.get(&(call.target.clone(), method_key.to_string())))
        .flatten()
    {
        return Some(RustType::parse(ret));
    }
    if let Some(ret) = call.receiver.as_ref()
        .and_then(|recv| infer_expr_type(recv, ctx))
        .and_then(|recv_ty| ctx.method_returns.get(&(recv_ty, method_key.to_string())))
    {
        return Some(RustType::parse(ret));
    }
    None
}

// ─── lower_call ──────────────────────────────────────────────────────────────

/// Lower `Expr::Call` to structured `RustExpr`.
///
/// Strategy: handle the common patterns structurally, fall through to
/// a `Raw` node wrapping `translate_call` for complex sub-paths that
/// are not worth migrating now.
///
/// Migrated paths:
/// - Bus routing (envelope/message JSON calls via routing traits)
/// - Cross-boundary envelope routing
///
/// The key win: async/fallible suffixes become structural composition
/// (`RustExpr::Await`, `RustExpr::Try`) instead of string appending.

/// Lower non-routing port/trait calls to structural MethodCall nodes.
///
/// Handles calls of the form `PortName.method(args)` where `PortName` is a trait
/// target that is NOT a routing trait (routing is handled by lower_call_bus_routing).
/// These produce: `deps.<field>.method(args).await?` or `.await` depending on
/// the method's return type.
fn lower_call_port(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    use super::calls::param_types_for;

    // Only handle trait-shaped targets that are NOT routing traits
    if !ctx.is_trait_target(&call.target) {
        return None;
    }
    if ctx.routing_traits.contains(&call.target) {
        return None;
    }
    // Sugar calls go through bus routing
    if call.sugar.is_some() {
        return None;
    }

    let dep_name = ctx.deps_field_for(&call.target);
    let method = if call.method.is_empty() { "call" } else { &call.method };
    let method_key = method.trim_end_matches(['!', '?']);

    // Determine args (using the same logic as translate_call)
    let param_tys = param_types_for(Some(call.target.as_str()), method_key, ctx);
    let args_str = call.args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let expected = param_tys.get(i).map(|s| s.as_str());
            let s = super::calls::arg_to_rust(a, expected, ctx);
            match a {
                Expr::Ident(name) if ctx.local_type(name) == Some("serde_json::Value") => {
                    format!("{}.clone()", name)
                }
                Expr::Ident(name)
                    if ctx.local_type(name)
                        .map(|t| t.starts_with("Option<"))
                        .unwrap_or(false) =>
                {
                    let expects_opt = expected
                        .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                        .unwrap_or(false);
                    if expects_opt {
                        format!("{}.clone()", name)
                    } else {
                        format!("{}.clone().ok_or({})?", name, ctx.error_model.not_found_path())
                    }
                }
                _ => s,
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    // Determine fallibility
    let has_bang = method.ends_with('!');
    let ret_type = ctx.return_type_of(&call.target, method)
        .or_else(|| {
            ctx.dep_fields.iter()
                .find(|(_, v)| *v == &call.target)
                .and_then(|(trait_name, _)| ctx.return_type_of(trait_name, method))
        });
    let is_fallible = if has_bang {
        true
    } else {
        match ret_type {
            Some("bool") | Some("Bool") | Some("i64") | Some("f64")
            | Some("String") | Some("()") | Some("") => false,
            Some(t) if t.starts_with("Option<") || t.starts_with("Opt<") => false,
            _ => true,
        }
    };

    // Build receiver: deps.<field> or self.<field>
    let prefix = if ctx.in_method && ctx.self_fields.contains(&dep_name) {
        format!("self.{}", dep_name)
    } else {
        format!("deps.{}", dep_name)
    };

    let args_ir = if args_str.is_empty() {
        vec![]
    } else {
        vec![RustExpr::Raw { text: args_str, ty: None }]
    };

    let ty = infer_call_type(call, ctx);

    Some(RustExpr::MethodCall {
        receiver: Box::new(RustExpr::Ident { name: prefix, ty: None }),
        method: to_snake(method_key),
        args: args_ir,
        ty,
        is_async: true,
        is_fallible,
    })
}

fn lower_call(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> RustExpr {
    // Try structured bus routing first
    if let Some(expr) = lower_call_bus_routing(call, ctx) {
        return expr;
    }

    // Try builder chain lowering (chained receiver method calls with async/fallible terminal)
    if let Some(expr) = lower_call_builder_chain(call, ctx) {
        return expr;
    }

    // Try structured port/trait calls (non-routing, non-sugar)
    if let Some(expr) = lower_call_port(call, ctx) {
        return expr;
    }

    // Fall through: wrap translate_call output for everything else
    let text = super::calls::translate_call(call, ctx);
    let ty = infer_call_type(call, ctx);
    RustExpr::Ident { name: text, ty }
}

/// Infer the RustType for a call expression from context.
fn infer_call_type(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustType> {
    // Try the method_returns map first (most precise)
    let method_key = call.method.trim_end_matches(['!', '?']);
    if !call.target.is_empty()
        && let Some(ret) = ctx.method_returns.get(&(call.target.clone(), method_key.to_string())) {
            return Some(RustType::parse(ret));
        }
    // Check receiver type for chained calls
    if let Some(recv) = &call.receiver
        && let Some(recv_ty) = infer_expr_type(recv, ctx)
            && let Some(ret) = ctx.method_returns.get(&(recv_ty.clone(), method_key.to_string())) {
                return Some(RustType::parse(ret));
            }
    // Struct constructors return the struct type
    if (call.method.is_empty() || method_key == "new" || method_key == "default")
        && !call.target.is_empty() && call.target.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            return Some(RustType::Named(call.target.clone()));
        }
    // Fall back to general inference
    infer_expr_type(&Expr::Call(call.clone()), ctx).map(|s| RustType::parse(&s))
}

// ─── apply_ownership ─────────────────────────────────────────────────────────

/// Apply ownership semantics to a `RustExpr`: wrap in `Clone` when the value
/// is non-Copy, multi-use, and not already owned.
///
/// This is the IR-level replacement for the old string-based `clone_for_reuse`.
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
            // Call results are owned — anything ending with ) or )? or .await? or .await
            // that contains a `(` is a function/method call producing an owned value.
            if raw_is_call_result(text) {
                return expr;
            }
            // Statements don't produce values — never clone them.
            if raw_is_statement(text) {
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
            | RustExpr::JsonMacro { .. }
            | RustExpr::JsonNull
            | RustExpr::JsonEmptyArray
            | RustExpr::VecMacro(_)
            | RustExpr::Array { .. }
            | RustExpr::Tuple { .. }
            | RustExpr::StructLit { .. }
            | RustExpr::BinOp { .. }
            | RustExpr::UnaryOp { .. }
            | RustExpr::For { .. }
            | RustExpr::While { .. }
            | RustExpr::Loop { .. }
            | RustExpr::Let { .. }
            | RustExpr::Await(_)
            | RustExpr::Try(_)
            | RustExpr::MapErr { .. }
            | RustExpr::Borrow { .. }
    )
}

/// Whether a raw text expression represents a call result (owned value).
///
/// TRANSITION DEBT: remove when translate_call is fully migrated to structured
/// RustExpr — at that point all calls will be MethodCall/FnCall variants and
/// `is_already_owned` handles those structurally.
///
/// Heuristic: contains `(` and ends with one of:
/// - `)` — plain function/method call
/// - `)?` — fallible call
/// - `.await?` — async+fallible call
/// - `.await` — async call
/// - `.to_string()` / `.clone()` — already owned (redundant with rust_already_owned but safe)
/// - `}` — block expression producing a value (e.g. Process.run)
///
/// Also catches format!(...), serde_json::from_str(...), etc.
fn raw_is_call_result(text: &str) -> bool {
    let t = text.trim();
    // Block expressions `{ ... }` are owned values
    if t.starts_with('{') && t.ends_with('}') {
        return true;
    }
    // Must contain a `(` to be a call
    if !t.contains('(') {
        return false;
    }
    // Call patterns
    t.ends_with(')')
        || t.ends_with(")?")
        || t.ends_with(".await?")
        || t.ends_with(".await")
        || t.ends_with(".unwrap()")
}

/// Whether a raw text expression is a statement (doesn't produce a value).
/// Statements must not be cloned — they're used for side effects.
fn raw_is_statement(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("let ")
        || t.starts_with("for ")
        || t.starts_with("while ")
        || t.starts_with("loop {")
        || t.starts_with("if ")
        || t.starts_with("match ")
        || t.starts_with("return ")
        || t.starts_with("return\n")
        || t == "break"
        || t == "continue"
        || t.starts_with("state[")
        || t.starts_with("self.")
        || t.contains(" = ")
        || t.contains(" += ")
        || t.contains(" -= ")
        || t.contains(" *= ")
        || t.ends_with('}')
        || t.starts_with("compile_error!")
}

/// Whether the expression's type is Copy (primitives, unit enums, refs).
fn is_expr_copy(expr: &RustExpr, ctx: &GenCtx) -> bool {
    match expr {
        RustExpr::IntLit(_) | RustExpr::FloatLit(_) | RustExpr::BoolLit(_) => true,
        RustExpr::Ident { name, ty } => {
            // Check type annotation first
            if let Some(t) = ty
                && t.is_copy() {
                    return true;
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
            if let Some(t) = ty
                && t.is_copy() {
                    return true;
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
    // Qualified enum paths (e.g. "Kind::Event") are values, not borrowable names.
    // Either they're Copy unit variants or constructors — neither needs cloning.
    if name.contains("::") {
        return false;
    }
    // Uppercase non-local ident: likely an enum variant or type constant — don't clone.
    if !ctx.is_local(name)
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    {
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

// ─── Closure try-suppression ─────────────────────────────────────────────────

/// Structurally transform a `RustExpr` tree to suppress `?` / `map_err(...)?`
/// inside closure bodies. Closures don't return `Result`, so the try operator
/// is invalid — replace with `.unwrap()`.
///
/// Transforms:
/// - `Try(inner)` → `MethodCall { receiver: inner, method: "unwrap", ... }`
/// - `MapErr { inner, .. }` → `MethodCall { receiver: inner, method: "unwrap", ... }`
/// - `MethodCall { is_fallible: true, .. }` → same with `is_fallible: false` + `.unwrap()`
/// - `Raw { text }` → text-level fixup (fallback for unmigrated paths)
///
/// This is a recursive traversal — it walks the entire expression tree.
pub fn suppress_try_in_closure(expr: RustExpr) -> RustExpr {
    match expr {
        // Direct ? operator → .unwrap()
        RustExpr::Try(inner) => {
            let inner = suppress_try_in_closure(*inner);
            RustExpr::MethodCall {
                receiver: Box::new(inner),
                method: "unwrap".to_string(),
                args: vec![],
                ty: None,
                is_async: false,
                is_fallible: false,
            }
        }
        // .map_err(...)? → .unwrap()  (drop the error mapping entirely)
        RustExpr::MapErr { inner, .. } => {
            let inner = suppress_try_in_closure(*inner);
            RustExpr::MethodCall {
                receiver: Box::new(inner),
                method: "unwrap".to_string(),
                args: vec![],
                ty: None,
                is_async: false,
                is_fallible: false,
            }
        }
        // Fallible method call → call .unwrap() on the result
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible: true,
        } => {
            let inner = RustExpr::MethodCall {
                receiver: Box::new(suppress_try_in_closure(*receiver)),
                method,
                args: args.into_iter().map(suppress_try_in_closure).collect(),
                ty: ty.clone(),
                is_async,
                is_fallible: false,
            };
            RustExpr::MethodCall {
                receiver: Box::new(inner),
                method: "unwrap".to_string(),
                args: vec![],
                ty,
                is_async: false,
                is_fallible: false,
            }
        }
        // Raw text: apply string-level fixup as fallback for unmigrated paths
        RustExpr::Raw { text, ty } => {
            let text = fixup_closure_raw(&text);
            RustExpr::Raw { text, ty }
        }
        // Recurse into compound expressions
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible,
        } => RustExpr::MethodCall {
            receiver: Box::new(suppress_try_in_closure(*receiver)),
            method,
            args: args.into_iter().map(suppress_try_in_closure).collect(),
            ty,
            is_async,
            is_fallible,
        },
        RustExpr::FnCall { path, args, ty } => RustExpr::FnCall {
            path,
            args: args.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Clone(inner) => {
            RustExpr::Clone(Box::new(suppress_try_in_closure(*inner)))
        }
        RustExpr::Borrow { inner, mutable } => RustExpr::Borrow {
            inner: Box::new(suppress_try_in_closure(*inner)),
            mutable,
        },
        RustExpr::Await(inner) => {
            RustExpr::Await(Box::new(suppress_try_in_closure(*inner)))
        }
        RustExpr::Block { stmts, value } => RustExpr::Block {
            stmts: stmts.into_iter().map(suppress_try_in_closure).collect(),
            value: value.map(|v| Box::new(suppress_try_in_closure(*v))),
        },
        RustExpr::If {
            condition,
            then_body,
            else_body,
        } => RustExpr::If {
            condition: Box::new(suppress_try_in_closure(*condition)),
            then_body: Box::new(suppress_try_in_closure(*then_body)),
            else_body: else_body.map(|e| Box::new(suppress_try_in_closure(*e))),
        },
        RustExpr::Format { template, args } => RustExpr::Format {
            template,
            args: args.into_iter().map(suppress_try_in_closure).collect(),
        },
        RustExpr::FieldAccess { base, field, ty } => RustExpr::FieldAccess {
            base: Box::new(suppress_try_in_closure(*base)),
            field,
            ty,
        },
        RustExpr::Let {
            name,
            mutable,
            ty,
            value,
        } => RustExpr::Let {
            name,
            mutable,
            ty,
            value: Box::new(suppress_try_in_closure(*value)),
        },
        RustExpr::Match { scrutinee, arms } => RustExpr::Match {
            scrutinee: Box::new(suppress_try_in_closure(*scrutinee)),
            arms: arms
                .into_iter()
                .map(|(pat, body)| (pat, suppress_try_in_closure(body)))
                .collect(),
        },
        // Leaves: no recursion needed
        RustExpr::Ident { .. }
        | RustExpr::StringLit(_)
        | RustExpr::IntLit(_)
        | RustExpr::FloatLit(_)
        | RustExpr::BoolLit(_)
        | RustExpr::JsonNull
        | RustExpr::JsonEmptyArray => expr,
        RustExpr::JsonMacro { entries } => RustExpr::JsonMacro {
            entries: entries
                .into_iter()
                .map(|(k, v)| (k, suppress_try_in_closure(v)))
                .collect(),
        },
        RustExpr::VecMacro(items) => {
            RustExpr::VecMacro(items.into_iter().map(suppress_try_in_closure).collect())
        }
        // Structural nodes: recurse into children
        RustExpr::BinOp { left, op, right, ty } => RustExpr::BinOp {
            left: Box::new(suppress_try_in_closure(*left)),
            op,
            right: Box::new(suppress_try_in_closure(*right)),
            ty,
        },
        RustExpr::UnaryOp { op, expr, ty } => RustExpr::UnaryOp {
            op,
            expr: Box::new(suppress_try_in_closure(*expr)),
            ty,
        },
        RustExpr::Array { items, ty } => RustExpr::Array {
            items: items.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Tuple { items, ty } => RustExpr::Tuple {
            items: items.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Index { base, index, ty } => RustExpr::Index {
            base: Box::new(suppress_try_in_closure(*base)),
            index: Box::new(suppress_try_in_closure(*index)),
            ty,
        },
        RustExpr::StructLit { name, fields, ty } => RustExpr::StructLit {
            name,
            fields: fields.into_iter().map(|(k, v)| (k, suppress_try_in_closure(v))).collect(),
            ty,
        },
        RustExpr::For { binding, iterable, body, ty } => RustExpr::For {
            binding,
            iterable: Box::new(suppress_try_in_closure(*iterable)),
            body: body.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::While { condition, body, ty } => RustExpr::While {
            condition: Box::new(suppress_try_in_closure(*condition)),
            body: body.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Loop { body, ty } => RustExpr::Loop {
            body: body.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
    }
}
fn fixup_closure_raw(s: &str) -> String {
    let mut s = s
        .replace(
            ".map_err(|e| DomainError::External(format!(\"{:?}\", e)))?",
            ".unwrap()",
        )
        .replace(
            ".map_err(|e| DomainError::External(format!(\"{e:?}\")))?",
            ".unwrap()",
        )
        .replace(
            ".map_err(|e| DomainError::External(e.to_string()))?",
            ".unwrap()",
        );
    // Replace trailing `)?` with `).unwrap()`
    while let Some(pos) = s.find(")?") {
        let after = if pos + 2 < s.len() {
            &s[pos + 2..pos + 3]
        } else {
            ""
        };
        if after.is_empty()
            || after == ")"
            || after == "."
            || after == ","
            || after == ";"
            || after == " "
        {
            s = format!("{}).unwrap(){}", &s[..pos], &s[pos + 2..]);
        } else {
            break;
        }
    }
    s
}

/// Lower `Expr::Closure` to a structured `RustExpr`, applying try-suppression
/// on body expressions so that `?` and `.map_err(...)` become `.unwrap()`.
fn lower_closure(params: &[String], body: &[Expr], ctx: &GenCtx) -> RustExpr {
    // Create a closure-scoped context with params as locals
    let mut closure_ctx = ctx.clone_for_inference();
    for param in params {
        closure_ctx.locals.insert(param.clone());
    }

    // Lower and suppress-try each body expression
    let body_exprs: Vec<RustExpr> = body
        .iter()
        .map(|e| {
            let lowered = lower_to_rust(e, &closure_ctx);
            suppress_try_in_closure(lowered)
        })
        .collect();

    let p = params.join(", ");
    if body_exprs.len() == 1 {
        let body_str = emit(&body_exprs[0]);
        RustExpr::Ident {
            name: format!("|{}| {}", p, body_str),
            ty: None,
        }
    } else {
        let stmts = body_exprs
            .iter()
            .map(|e| format!("    {};", emit(e)))
            .collect::<Vec<_>>()
            .join("\n");
        RustExpr::Ident {
            name: format!("|{}| {{\n{}\n}}", p, stmts),
            ty: None,
        }
    }
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

    // ─── lower_string_interp tests ───────────────────────────────────

    #[test]
    fn lower_string_interp_basic() {
        use veil_ir::ast::StringPart;
        let ctx = GenCtx::new(std::collections::HashMap::new());
        let parts = vec![
            StringPart::Literal("Hello, ".to_string()),
            StringPart::Expr(Expr::Ident("name".to_string())),
            StringPart::Literal("!".to_string()),
        ];
        let result = lower_string_interp(&parts, &ctx);
        assert_eq!(emit(&result), "format!(\"Hello, {}!\", name)");
    }

    #[test]
    fn lower_string_interp_no_exprs() {
        use veil_ir::ast::StringPart;
        let ctx = GenCtx::new(std::collections::HashMap::new());
        let parts = vec![StringPart::Literal("static text".to_string())];
        let result = lower_string_interp(&parts, &ctx);
        assert_eq!(emit(&result), "\"static text\".to_string()");
    }

    #[test]
    fn lower_string_interp_brace_escape() {
        use veil_ir::ast::StringPart;
        let ctx = GenCtx::new(std::collections::HashMap::new());
        let parts = vec![
            StringPart::Literal("/{".to_string()),
            StringPart::Expr(Expr::Ident("id".to_string())),
            StringPart::Literal("}".to_string()),
        ];
        let result = lower_string_interp(&parts, &ctx);
        // `{` → `{{`, `}` → `}}` in literal parts; expr part → `{}`
        // Template: /{{ + {} + }} = /{{{}}}, which renders as /{<value>}
        assert_eq!(emit(&result), "format!(\"/{{{}}}\", id)");
    }

    // ─── lower_call tests ────────────────────────────────────────────

    #[test]
    fn lower_call_wraps_translate_call_output() {
        use veil_ir::ast::CallExpr;
        use veil_ir::Span;
        let ctx = GenCtx::new(std::collections::HashMap::new());
        let call = CallExpr {
            target: "Uuid".to_string(),
            method: "new_v4".to_string(),
            args: vec![],
            receiver: None,
            sugar: None,
            span: Span::default(),
        };
        let result = lower_call(&call, &ctx);
        assert_eq!(emit(&result), "Uuid::new_v4()");
    }

    // ─── apply_ownership on call results ─────────────────────────────

    #[test]
    fn ownership_raw_call_result_no_clone() {
        let ctx = GenCtx::new(std::collections::HashMap::new());
        // A function call result is already owned
        let expr = RustExpr::Raw {
            text: "Uuid::new_v4()".to_string(),
            ty: Some(RustType::Named("Uuid".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "Uuid::new_v4()"); // no clone
    }

    #[test]
    fn ownership_raw_async_fallible_no_clone() {
        let ctx = GenCtx::new(std::collections::HashMap::new());
        // async+fallible call result is owned
        let expr = RustExpr::Raw {
            text: "deps.repo.save(entity).await?".to_string(),
            ty: Some(RustType::Named("String".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "deps.repo.save(entity).await?"); // no clone
    }

    #[test]
    fn ownership_raw_block_expr_no_clone() {
        let ctx = GenCtx::new(std::collections::HashMap::new());
        // Block expression is owned
        let expr = RustExpr::Raw {
            text: "{ let x = 1; x }".to_string(),
            ty: Some(RustType::Named("i64".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "{ let x = 1; x }"); // no clone
    }

    #[test]
    fn ownership_raw_bare_ident_still_clones() {
        let ctx = make_ctx_with_uses("data", 2);
        // A bare ident in Raw should still get cloned when multi-use
        let expr = RustExpr::Raw {
            text: "data".to_string(),
            ty: Some(RustType::Named("String".to_string())),
        };
        let result = apply_ownership(expr, &ctx);
        assert_eq!(emit(&result), "data.clone()");
    }

    // ─── suppress_try_in_closure tests ───────────────────────────────

    #[test]
    fn suppress_try_converts_try_to_unwrap() {
        let expr = RustExpr::Try(Box::new(RustExpr::Ident {
            name: "result".to_string(),
            ty: None,
        }));
        let result = suppress_try_in_closure(expr);
        assert_eq!(emit(&result), "result.unwrap()");
    }

    #[test]
    fn suppress_try_converts_map_err_to_unwrap() {
        let expr = RustExpr::MapErr {
            inner: Box::new(RustExpr::MethodCall {
                receiver: Box::new(RustExpr::Ident {
                    name: "serde_json".to_string(),
                    ty: None,
                }),
                method: "from_str".to_string(),
                args: vec![RustExpr::Ident {
                    name: "s".to_string(),
                    ty: None,
                }],
                ty: None,
                is_async: false,
                is_fallible: false,
            }),
            variant: "DomainError::External".to_string(),
        };
        let result = suppress_try_in_closure(expr);
        assert_eq!(emit(&result), "serde_json.from_str(s).unwrap()");
    }

    #[test]
    fn suppress_try_converts_fallible_method_call() {
        let expr = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident {
                name: "repo".to_string(),
                ty: None,
            }),
            method: "save".to_string(),
            args: vec![],
            ty: None,
            is_async: true,
            is_fallible: true,
        };
        let result = suppress_try_in_closure(expr);
        // save().await? → save().await.unwrap()
        assert_eq!(emit(&result), "repo.save().await.unwrap()");
    }

    #[test]
    fn suppress_try_raw_fixup_map_err() {
        let expr = RustExpr::Raw {
            text: "serde_json::from_str(&s).map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string(),
            ty: None,
        };
        let result = suppress_try_in_closure(expr);
        assert_eq!(emit(&result), "serde_json::from_str(&s).unwrap()");
    }

    #[test]
    fn suppress_try_raw_fixup_question_mark() {
        // `)?` pattern: parenthesized expr followed by `?`
        let expr = RustExpr::Raw {
            text: "serde_json::from_str(&s)?".to_string(),
            ty: None,
        };
        let result = suppress_try_in_closure(expr);
        assert_eq!(emit(&result), "serde_json::from_str(&s).unwrap()");
    }

    #[test]
    fn suppress_try_leaves_non_fallible_unchanged() {
        let expr = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident {
                name: "items".to_string(),
                ty: None,
            }),
            method: "len".to_string(),
            args: vec![],
            ty: Some(RustType::Named("usize".to_string())),
            is_async: false,
            is_fallible: false,
        };
        let result = suppress_try_in_closure(expr);
        assert_eq!(emit(&result), "items.len()");
    }
}
