//! Constructors and finishers for `RustExpr`.
//!
//! Lowering builds trees with these helpers. `emit` is the only place that
//! turns a node into source text.

use veil_ir::ast::Expr;

use super::super::context::{ErrorModel, GenCtx};
use super::ownership::apply_ownership;
use super::{CallFinish, MapErrStyle, RustExpr, RustType, lower_to_rust};

pub fn ident(name: impl Into<String>) -> RustExpr {
    RustExpr::Ident {
        name: name.into(),
        ty: None,
    }
}

pub fn ident_ty(name: impl Into<String>, ty: Option<RustType>) -> RustExpr {
    RustExpr::Ident {
        name: name.into(),
        ty,
    }
}

pub fn field(base: RustExpr, name: impl Into<String>) -> RustExpr {
    RustExpr::FieldAccess {
        base: Box::new(base),
        field: name.into(),
        ty: None,
    }
}

/// `a.b.c` from path segments.
pub fn field_path(parts: &[&str]) -> RustExpr {
    let mut iter = parts.iter();
    let first = ident(*iter.next().unwrap_or(&""));
    iter.fold(first, |acc, p| field(acc, *p))
}

pub fn method(recv: RustExpr, name: impl Into<String>, args: Vec<RustExpr>) -> RustExpr {
    RustExpr::MethodCall {
        receiver: Box::new(recv),
        method: name.into(),
        args,
        ty: None,
        is_async: false,
        is_fallible: false,
    }
}

pub fn fn_call(path: impl Into<String>, args: Vec<RustExpr>) -> RustExpr {
    RustExpr::FnCall {
        path: path.into(),
        args,
        ty: None,
    }
}

pub fn clone_of(expr: RustExpr) -> RustExpr {
    match expr {
        RustExpr::Clone(_) => expr,
        other => RustExpr::Clone(Box::new(other)),
    }
}

pub fn borrow_of(expr: RustExpr) -> RustExpr {
    match expr {
        RustExpr::Borrow { .. } => expr,
        other => RustExpr::Borrow {
            inner: Box::new(other),
            mutable: false,
        },
    }
}

pub fn borrow_mut(expr: RustExpr) -> RustExpr {
    RustExpr::Borrow {
        inner: Box::new(expr),
        mutable: true,
    }
}

pub fn try_of(expr: RustExpr) -> RustExpr {
    RustExpr::Try(Box::new(expr))
}

pub fn await_of(expr: RustExpr) -> RustExpr {
    RustExpr::Await(Box::new(expr))
}

pub fn to_string_of(expr: RustExpr) -> RustExpr {
    method(expr, "to_string", vec![])
}

pub fn owned_str(s: &str) -> RustExpr {
    to_string_of(RustExpr::StringLit(s.to_string()))
}

pub fn some_of(expr: RustExpr) -> RustExpr {
    fn_call("Some", vec![expr])
}

pub fn none() -> RustExpr {
    ident("None")
}

pub fn unit() -> RustExpr {
    ident("()")
}

pub fn ok_of(expr: RustExpr) -> RustExpr {
    fn_call("Ok", vec![expr])
}

pub fn err_of(expr: RustExpr) -> RustExpr {
    fn_call("Err", vec![expr])
}

pub fn assign(target: RustExpr, value: RustExpr) -> RustExpr {
    RustExpr::Assign {
        target: Box::new(target),
        op: "=".to_string(),
        value: Box::new(value),
    }
}

pub fn assign_op(target: RustExpr, op: &str, value: RustExpr) -> RustExpr {
    RustExpr::Assign {
        target: Box::new(target),
        op: op.to_string(),
        value: Box::new(value),
    }
}

pub fn cast(expr: RustExpr, ty: impl Into<String>) -> RustExpr {
    RustExpr::Cast {
        expr: Box::new(expr),
        ty: ty.into(),
    }
}

pub fn closure(params: Vec<String>, body: RustExpr) -> RustExpr {
    RustExpr::Closure {
        params,
        body: vec![body],
    }
}

pub fn map_err_debug(inner: RustExpr, variant: impl Into<String>) -> RustExpr {
    RustExpr::MapErr {
        inner: Box::new(inner),
        variant: variant.into(),
        style: MapErrStyle::Debug,
    }
}

pub fn map_err_to_string(inner: RustExpr, variant: impl Into<String>) -> RustExpr {
    RustExpr::MapErr {
        inner: Box::new(inner),
        variant: variant.into(),
        style: MapErrStyle::ToString,
    }
}

pub fn map_err_display(inner: RustExpr, variant: impl Into<String>) -> RustExpr {
    RustExpr::MapErr {
        inner: Box::new(inner),
        variant: variant.into(),
        style: MapErrStyle::Display,
    }
}

pub fn map_err_ignore(inner: RustExpr, err: RustExpr) -> RustExpr {
    RustExpr::MapErr {
        inner: Box::new(inner),
        variant: String::new(),
        style: MapErrStyle::Ignore(Box::new(err)),
    }
}

pub fn map_to_string(inner: RustExpr) -> RustExpr {
    method(
        inner,
        "map",
        vec![closure(
            vec!["s".to_string()],
            method(ident("s"), "to_string", vec![]),
        )],
    )
}

/// `expr.ok_or(NotFound)?`
pub fn ok_or_not_found(expr: RustExpr, ctx: &GenCtx) -> RustExpr {
    try_of(method(
        expr,
        "ok_or",
        vec![ident(ctx.error_model.not_found_path())],
    ))
}

/// `expr.ok_or_else(|| External("missing key".into()))?`
pub fn ok_or_else_missing(expr: RustExpr, key: &str, ctx: &GenCtx) -> RustExpr {
    let msg = format!("missing {key}");
    let err = fn_call(
        ctx.error_model.external_path(),
        vec![method(RustExpr::StringLit(msg), "into", vec![])],
    );
    try_of(method(
        expr,
        "ok_or_else",
        vec![closure(vec![], err)],
    ))
}

pub fn json_object() -> RustExpr {
    fn_call(
        "serde_json::Value::Object",
        vec![fn_call("serde_json::Map::new", vec![])],
    )
}

pub fn json_array_new() -> RustExpr {
    fn_call(
        "serde_json::Value::Array",
        vec![fn_call("Vec::new", vec![])],
    )
}

pub fn ret_ok(value: RustExpr) -> RustExpr {
    RustExpr::Return {
        value: Box::new(value),
        wraps_ok: true,
    }
}

pub fn ret(value: RustExpr) -> RustExpr {
    RustExpr::Return {
        value: Box::new(value),
        wraps_ok: false,
    }
}

pub fn ret_err(err: RustExpr) -> RustExpr {
    ret(err_of(err))
}

pub fn compile_error(msg: impl Into<String>) -> RustExpr {
    RustExpr::CompileError(msg.into())
}

pub fn validation_err(msg: &str, ctx: &GenCtx) -> RustExpr {
    fn_call(
        ctx.error_model.validation_path(),
        vec![owned_str(msg)],
    )
}

pub fn not_found_err(ctx: &GenCtx) -> RustExpr {
    ident(ctx.error_model.not_found_path())
}

pub fn external_err_str(msg: &str, ctx: &GenCtx) -> RustExpr {
    fn_call(ctx.error_model.external_path(), vec![owned_str(msg)])
}

/// Lower a VEIL expr and apply ownership (argument / RHS positions).
pub fn lower_value(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    apply_ownership(lower_to_rust(expr, ctx), ctx)
}

/// Value-position lowering: string lits become owned `String`.
pub fn lower_owned(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::StringLit(s) => owned_str(s),
        _ => lower_value(expr, ctx),
    }
}

/// Apply `CallFinish` to a call node (MethodCall/FnCall or any expr).
pub fn apply_finish(expr: RustExpr, finish: CallFinish, em: &ErrorModel) -> RustExpr {
    match finish {
        CallFinish::Bare => expr,
        CallFinish::Await => set_async(expr, true, false),
        CallFinish::AwaitTry => set_async(expr, true, true),
        CallFinish::Try => set_fallible(expr),
        CallFinish::MapErrDebug => map_err_debug(expr, em.external_path()),
        CallFinish::MapErrOwnStr => map_err_debug(map_to_string(expr), em.external_path()),
        CallFinish::AwaitMapErr => map_err_debug(set_async(expr, true, false), em.external_path()),
    }
}

fn set_async(expr: RustExpr, is_async: bool, is_fallible: bool) -> RustExpr {
    match expr {
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            ..
        } => RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible,
        },
        other if is_async && is_fallible => try_of(await_of(other)),
        other if is_async => await_of(other),
        other if is_fallible => try_of(other),
        other => other,
    }
}

fn set_fallible(expr: RustExpr) -> RustExpr {
    match expr {
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            ..
        } => RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible: true,
        },
        other => try_of(other),
    }
}

/// Drop a trailing try / map_err so `match` can consume a `Result`.
pub fn strip_try_ir(expr: RustExpr) -> RustExpr {
    match expr {
        RustExpr::Try(inner) => strip_try_ir(*inner),
        RustExpr::MapErr { inner, .. } => strip_try_ir(*inner),
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible: true,
        } => RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible: false,
        },
        other => other,
    }
}

/// Wrap a value as `Some(...)` unless it is already Option-shaped.
pub fn wrap_as_option_ir(expr: &Expr, node: RustExpr, ctx: &GenCtx) -> RustExpr {
    match &node {
        RustExpr::Ident { name, .. } if name == "None" || name == "()" => none(),
        RustExpr::FnCall { path, .. } if path == "Some" => node,
        RustExpr::Return { .. } => node,
        RustExpr::Ident { name, .. } => {
            if let Expr::Ident(n) = expr
                && n == name
            {
                let local_ty = ctx.local_type(n);
                if local_ty.is_some_and(|ty| ty.starts_with("Option<")) {
                    return node;
                }
                // Unknown type: skip wrapping to avoid double-Option when the
                // local already holds Option from fetch_optional / similar.
                if local_ty.is_none() {
                    return node;
                }
            }
            some_of(node)
        }
        _ => some_of(node),
    }
}

pub fn is_vec_node(expr: &RustExpr) -> bool {
    matches!(
        expr,
        RustExpr::Array { .. } | RustExpr::VecMacro(_)
    )
}

pub fn is_none_node(expr: &RustExpr) -> bool {
    matches!(expr, RustExpr::Ident { name, .. } if name == "None")
}

pub fn is_unit_node(expr: &RustExpr) -> bool {
    matches!(expr, RustExpr::Ident { name, .. } if name == "()")
        || matches!(expr, RustExpr::Tuple { items, .. } if items.is_empty())
}

/// `bytes_from_str` as a block: `{ let __s = (arg).to_string(); __s.into_bytes() }`
pub fn bytes_from_str_ir(arg: RustExpr) -> RustExpr {
    RustExpr::Block {
        stmts: vec![RustExpr::Let {
            name: "__s".to_string(),
            mutable: false,
            ty: None,
            value: Box::new(to_string_of(arg)),
        }],
        value: Some(Box::new(method(ident("__s"), "into_bytes", vec![]))),
    }
}

/// Hex-decode loop as a structured block (replaces the old string mill).
pub fn bytes_from_hex_ir(hex: RustExpr) -> RustExpr {
    let h_as_str = method(ident("__h"), "as_str", vec![]);
    let capacity = hex_capacity();
    RustExpr::Block {
        stmts: vec![
            RustExpr::Let {
                name: "__h".to_string(),
                mutable: false,
                ty: Some("String".to_string()),
                value: Box::new(to_string_of(hex)),
            },
            RustExpr::Let {
                name: "__h".to_string(),
                mutable: false,
                ty: None,
                value: Box::new(h_as_str),
            },
            RustExpr::Let {
                name: "__b".to_string(),
                mutable: true,
                ty: None,
                value: Box::new(fn_call("Vec::with_capacity", vec![capacity])),
            },
            RustExpr::Let {
                name: "__i".to_string(),
                mutable: true,
                ty: Some("usize".to_string()),
                value: Box::new(RustExpr::IntLit(0)),
            },
            RustExpr::While {
                condition: Box::new(RustExpr::BinOp {
                    left: Box::new(RustExpr::BinOp {
                        left: Box::new(ident("__i")),
                        op: "+".to_string(),
                        right: Box::new(RustExpr::IntLit(1)),
                        ty: None,
                    }),
                    op: "<".to_string(),
                    right: Box::new(method(ident("__h"), "len", vec![])),
                    ty: None,
                }),
                body: vec![
                    RustExpr::IfLet {
                        pattern: "Ok(__v)".to_string(),
                        expr: Box::new(fn_call(
                            "u8::from_str_radix",
                            vec![
                                borrow_of(RustExpr::Index {
                                    base: Box::new(ident("__h")),
                                    index: Box::new(RustExpr::Range {
                                        start: Some(Box::new(ident("__i"))),
                                        end: Some(Box::new(RustExpr::BinOp {
                                            left: Box::new(ident("__i")),
                                            op: "+".to_string(),
                                            right: Box::new(RustExpr::IntLit(2)),
                                            ty: None,
                                        })),
                                        inclusive: false,
                                    }),
                                    ty: None,
                                }),
                                RustExpr::IntLit(16),
                            ],
                        )),
                        then_body: vec![method(ident("__b"), "push", vec![ident("__v")])],
                        else_body: None,
                    },
                    assign_op(ident("__i"), "+=", RustExpr::IntLit(2)),
                ],
                ty: None,
            },
        ],
        value: Some(Box::new(ident("__b"))),
    }
}

fn hex_capacity() -> RustExpr {
    RustExpr::BinOp {
        left: Box::new(method(ident("__h"), "len", vec![])),
        op: "/".to_string(),
        right: Box::new(RustExpr::IntLit(2)),
        ty: None,
    }
}

/// `String::from_utf8_lossy(x.as_ref()).to_string()`
pub fn utf8_lossy_string(recv: RustExpr) -> RustExpr {
    to_string_of(fn_call(
        "String::from_utf8_lossy",
        vec![method(recv, "as_ref", vec![])],
    ))
}

/// `x.as_str().map(|s| s.to_string())`
pub fn as_str_owned(recv: RustExpr) -> RustExpr {
    map_to_string(method(recv, "as_str", vec![]))
}

pub fn parse_i64(inner: RustExpr, em: &ErrorModel) -> RustExpr {
    map_err_debug(
        method(inner, "parse::<i64>", vec![]),
        em.external_path(),
    )
}

/// Json object constructor for empty anonymous struct lits.
pub fn empty_json_object() -> RustExpr {
    RustExpr::JsonMacro { entries: vec![] }
}
