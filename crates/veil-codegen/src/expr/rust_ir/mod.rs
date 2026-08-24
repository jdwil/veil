//! Typed intermediate representation for Rust expressions.
//!
//! `RustExpr` sits between VEIL AST lowering and final Rust text emission.
//! It preserves type and ownership information so downstream transforms
//! (clone insertion, `?` suppression in closures, etc.) operate on structure
//! rather than rendered strings.
//!
//! Emission (`emit`) is the only stage that produces target text. Lowering
//! never concatenates child source.

mod build;
mod lower;
mod ownership;
#[cfg(test)]
mod tests;

use super::types::rust_string_lit;

pub use build::*;
pub use lower::lower_to_rust;
pub use ownership::{apply_ownership, suppress_try_in_closure};

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

    /// Parse a simple type string into a RustType. Handles nested generics
    /// via angle-bracket-depth tracking.
    pub fn parse(s: &str) -> RustType {
        let s = s.trim();
        if s == "()" {
            return RustType::Unit;
        }
        if s == "serde_json::Value" || s == "Value" {
            return RustType::Json;
        }
        if let Some(inner) = strip_generic_prefix(s, "Option") {
            return RustType::Option(Box::new(RustType::parse(inner)));
        }
        if let Some(inner) = strip_generic_prefix(s, "Result") {
            let inner_ty = split_type_params(inner)
                .first()
                .copied()
                .unwrap_or(inner);
            return RustType::Result(Box::new(RustType::parse(inner_ty)));
        }
        if let Some(inner) = strip_generic_prefix(s, "Vec") {
            return RustType::Vec(Box::new(RustType::parse(inner)));
        }
        if let Some(inner) = s.strip_prefix('&') {
            return RustType::Ref(Box::new(RustType::parse(inner)));
        }
        RustType::Named(s.to_string())
    }
}

fn strip_generic_prefix<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(name)?.strip_prefix('<')?;
    let mut depth = 1u32;
    for (i, ch) in rest.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    if i + 1 == rest.len() {
                        return Some(&rest[..i]);
                    }
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

fn split_type_params(s: &str) -> Vec<&str> {
    let mut params = Vec::new();
    let mut depth = 0u32;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                params.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        params.push(last);
    }
    params
}

// ─── CallFinish / MapErrStyle / Arm ──────────────────────────────────────────

/// How a call's Result/async-ness is finished after the call node is built.
/// Applied as IR wrappers (`Await` / `Try` / `MapErr`) — never concatenated
/// onto rendered source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFinish {
    Bare,
    Await,
    AwaitTry,
    Try,
    /// `.map_err(|e| Variant(format!("{e:?}")))?`
    MapErrDebug,
    /// `.map(|s| s.to_string()).map_err(|e| Variant(format!("{e:?}")))?`
    MapErrOwnStr,
    /// `.await.map_err(|e| Variant(format!("{e:?}")))?`
    AwaitMapErr,
}

impl CallFinish {
    pub fn is_bare(&self) -> bool {
        matches!(self, CallFinish::Bare)
    }
}

/// Style of `.map_err(...)` closure.
#[derive(Debug, Clone)]
pub enum MapErrStyle {
    /// `|e| Variant(format!("{e:?}"))`
    Debug,
    /// `|e| Variant(format!("{e}"))`
    Display,
    /// `|e| Variant(e.to_string())`
    ToString,
    /// `|_| expr`
    Ignore(Box<RustExpr>),
}

/// One arm of a match expression. Pattern text is a token (VEIL pattern
/// already lowered); guard and body are structured.
#[derive(Debug, Clone)]
pub struct Arm {
    pub pattern: String,
    pub guard: Option<Box<RustExpr>>,
    pub body: Vec<RustExpr>,
}

// ─── RustExpr ────────────────────────────────────────────────────────────────

/// Typed intermediate representation of a Rust expression.
#[derive(Debug, Clone)]
pub enum RustExpr {
    Ident { name: String, ty: Option<RustType> },
    StringLit(String),
    IntLit(i64),
    FloatLit(f64),
    BoolLit(bool),
    FieldAccess {
        base: Box<RustExpr>,
        field: String,
        ty: Option<RustType>,
    },
    MethodCall {
        receiver: Box<RustExpr>,
        method: String,
        args: Vec<RustExpr>,
        ty: Option<RustType>,
        is_async: bool,
        is_fallible: bool,
    },
    FnCall {
        path: String,
        args: Vec<RustExpr>,
        ty: Option<RustType>,
    },
    Clone(Box<RustExpr>),
    Borrow { inner: Box<RustExpr>, mutable: bool },
    Await(Box<RustExpr>),
    Try(Box<RustExpr>),
    MapErr {
        inner: Box<RustExpr>,
        variant: String,
        style: MapErrStyle,
    },
    Format { template: String, args: Vec<RustExpr> },
    Block {
        stmts: Vec<RustExpr>,
        value: Option<Box<RustExpr>>,
    },
    If {
        condition: Box<RustExpr>,
        then_body: Vec<RustExpr>,
        else_body: Option<Vec<RustExpr>>,
    },
    Match {
        scrutinee: Box<RustExpr>,
        arms: Vec<Arm>,
    },
    Let {
        name: String,
        mutable: bool,
        ty: Option<String>,
        value: Box<RustExpr>,
    },
    JsonMacro {
        entries: Vec<(String, RustExpr)>,
    },
    /// `serde_json::json!(inner)` wrapping a single value.
    JsonValue(Box<RustExpr>),
    JsonNull,
    JsonEmptyArray,
    VecMacro(Vec<RustExpr>),
    /// Layer `lowers_to` template. Bindings are substituted at emit time.
    LayerTemplate {
        template: String,
        bindings: Vec<(String, RustExpr)>,
    },
    CompileError(String),
    Return { value: Box<RustExpr>, wraps_ok: bool },
    BinOp {
        left: Box<RustExpr>,
        op: String,
        right: Box<RustExpr>,
        ty: Option<RustType>,
    },
    UnaryOp {
        op: String,
        expr: Box<RustExpr>,
        ty: Option<RustType>,
    },
    Array {
        items: Vec<RustExpr>,
        ty: Option<RustType>,
    },
    Tuple {
        items: Vec<RustExpr>,
        ty: Option<RustType>,
    },
    Index {
        base: Box<RustExpr>,
        index: Box<RustExpr>,
        ty: Option<RustType>,
    },
    StructLit {
        name: String,
        fields: Vec<(String, RustExpr)>,
        rest: Option<Box<RustExpr>>,
        ty: Option<RustType>,
    },
    For {
        binding: String,
        iterable: Box<RustExpr>,
        body: Vec<RustExpr>,
        ty: Option<RustType>,
    },
    While {
        condition: Box<RustExpr>,
        body: Vec<RustExpr>,
        ty: Option<RustType>,
    },
    Loop {
        body: Vec<RustExpr>,
        ty: Option<RustType>,
    },
    Assign {
        target: Box<RustExpr>,
        op: String,
        value: Box<RustExpr>,
    },
    Closure {
        params: Vec<String>,
        body: Vec<RustExpr>,
    },
    Cast {
        expr: Box<RustExpr>,
        ty: String,
    },
    Range {
        start: Option<Box<RustExpr>>,
        end: Option<Box<RustExpr>>,
        inclusive: bool,
    },
    IfLet {
        pattern: String,
        expr: Box<RustExpr>,
        then_body: Vec<RustExpr>,
        else_body: Option<Vec<RustExpr>>,
    },
    WhileLet {
        pattern: String,
        expr: Box<RustExpr>,
        body: Vec<RustExpr>,
    },
    Break,
    Continue,
    /// `/* text */`
    Comment(String),
    /// Comma/separator-joined items (template `{args}`, argument lists stored as nodes).
    Join {
        items: Vec<RustExpr>,
        sep: String,
    },
}

// ─── emit() ──────────────────────────────────────────────────────────────────

/// Render a `RustExpr` to its final Rust source string.
///
/// This is the only function that produces target text from the expression
/// tree. Lowering must not call `emit` on children and glue the results.
pub fn emit(expr: &RustExpr) -> String {
    match expr {
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
        RustExpr::MapErr {
            inner,
            variant,
            style,
        } => match style {
            MapErrStyle::Debug => format!(
                "{}.map_err(|e| {}(format!(\"{{e:?}}\")))?",
                emit(inner),
                variant
            ),
            MapErrStyle::Display => format!(
                "{}.map_err(|e| {}(format!(\"{{e}}\")))?",
                emit(inner),
                variant
            ),
            MapErrStyle::ToString => {
                format!("{}.map_err(|e| {}(e.to_string()))?", emit(inner), variant)
            }
            MapErrStyle::Ignore(err) => {
                format!("{}.map_err(|_| {})?", emit(inner), emit(err))
            }
        },
        RustExpr::Format { template, args } => {
            if args.is_empty() {
                format!("format!(\"{}\")", template)
            } else {
                let arg_strs: Vec<String> = args.iter().map(emit).collect();
                format!("format!(\"{}\", {})", template, arg_strs.join(", "))
            }
        }
        RustExpr::Block { stmts, value } => emit_block_expr(stmts, value.as_deref()),
        RustExpr::If {
            condition,
            then_body,
            else_body,
        } => {
            let cond = emit(condition);
            if then_body.len() == 1 && else_body.as_ref().is_some_and(|b| b.len() == 1) {
                let then_str = emit(&then_body[0]);
                let else_str = emit(&else_body.as_ref().unwrap()[0]);
                if !then_str.contains('\n') && !else_str.contains('\n') {
                    return format!("if {} {{ {} }} else {{ {} }}", cond, then_str, else_str);
                }
            }
            let then_str = emit_value_block_ir(then_body, "    ");
            match else_body {
                Some(eb) => {
                    let else_str = emit_value_block_ir(eb, "    ");
                    format!("if {} {{\n{}\n}} else {{\n{}\n}}", cond, then_str, else_str)
                }
                None => format!("if {} {{\n{}\n}}", cond, then_str),
            }
        }
        RustExpr::Match { scrutinee, arms } => {
            let scrut = emit(scrutinee);
            let mut lines = Vec::new();
            for arm in arms {
                let guard = arm
                    .guard
                    .as_ref()
                    .map(|g| format!(" if {}", emit(g)))
                    .unwrap_or_default();
                let body = emit_arm_body(&arm.body);
                lines.push(format!("        {}{} => {},", arm.pattern, guard, body));
            }
            format!("match {} {{\n{}\n    }}", scrut, lines.join("\n"))
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
            if entries.is_empty() {
                return "serde_json::json!({})".to_string();
            }
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("\"{}\": {}", k, emit(v)))
                .collect();
            format!("serde_json::json!({{ {} }})", parts.join(", "))
        }
        RustExpr::JsonValue(inner) => format!("serde_json::json!({})", emit(inner)),
        RustExpr::JsonNull => "serde_json::Value::Null".to_string(),
        RustExpr::JsonEmptyArray => "serde_json::Value::Array(vec![])".to_string(),
        RustExpr::VecMacro(items) => {
            let vals: Vec<String> = items.iter().map(emit).collect();
            format!("vec![{}]", vals.join(", "))
        }
        RustExpr::LayerTemplate {
            template,
            bindings,
        } => {
            let mut out = template.clone();
            let mut pairs: Vec<(&String, &RustExpr)> =
                bindings.iter().map(|(k, v)| (k, v)).collect();
            pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
            for (k, v) in pairs {
                out = out.replace(&format!("{{{k}}}"), &emit(v));
            }
            out
        }
        RustExpr::CompileError(msg) => {
            let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
            format!("compile_error!(\"{escaped}\")")
        }
        RustExpr::Return { value, wraps_ok } => {
            if *wraps_ok {
                format!("return Ok({})", emit(value))
            } else {
                format!("return {}", emit(value))
            }
        }
        RustExpr::BinOp {
            left, op, right, ..
        } => {
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
        RustExpr::StructLit {
            name,
            fields,
            rest,
            ..
        } => emit_struct_lit(name, fields, rest.as_deref()),
        RustExpr::For {
            binding,
            iterable,
            body,
            ..
        } => {
            let body_str = emit_block(body, "    ");
            format!("for {} in {} {{\n{}\n}}", binding, emit(iterable), body_str)
        }
        RustExpr::While {
            condition, body, ..
        } => {
            let body_str = emit_block(body, "    ");
            format!("while {} {{\n{}\n}}", emit(condition), body_str)
        }
        RustExpr::Loop { body, .. } => {
            let body_str = emit_block(body, "    ");
            format!("loop {{\n{}\n}}", body_str)
        }
        RustExpr::Assign { target, op, value } => {
            format!("{} {} {}", emit(target), op, emit(value))
        }
        RustExpr::Closure { params, body } => {
            let p = params.join(", ");
            if body.len() == 1 && !emit(&body[0]).contains('\n') {
                format!("|{}| {}", p, emit(&body[0]))
            } else {
                let inner = emit_value_block_ir(body, "    ");
                format!("|{}| {{\n{}\n}}", p, inner)
            }
        }
        RustExpr::Cast { expr, ty } => {
            format!("{} as {}", emit_maybe_paren(expr), ty)
        }
        RustExpr::Range {
            start,
            end,
            inclusive,
        } => {
            let op = if *inclusive { "..=" } else { ".." };
            let s = start.as_ref().map(|e| emit(e)).unwrap_or_default();
            let e = end.as_ref().map(|e| emit(e)).unwrap_or_default();
            format!("{s}{op}{e}")
        }
        RustExpr::IfLet {
            pattern,
            expr,
            then_body,
            else_body,
        } => {
            let then_str = emit_block(then_body, "    ");
            match else_body {
                Some(eb) => {
                    let else_str = emit_block(eb, "    ");
                    format!(
                        "if let {} = {} {{\n{}\n}} else {{\n{}\n}}",
                        pattern,
                        emit(expr),
                        then_str,
                        else_str
                    )
                }
                None => format!(
                    "if let {} = {} {{\n{}\n}}",
                    pattern,
                    emit(expr),
                    then_str
                ),
            }
        }
        RustExpr::WhileLet {
            pattern,
            expr,
            body,
        } => {
            let body_str = emit_block(body, "    ");
            format!(
                "while let {} = {} {{\n{}\n}}",
                pattern,
                emit(expr),
                body_str
            )
        }
        RustExpr::Break => "break".to_string(),
        RustExpr::Continue => "continue".to_string(),
        RustExpr::Comment(text) => {
            let safe = text.replace("*/", "* /");
            format!("/* {safe} */")
        }
        RustExpr::Join { items, sep } => items
            .iter()
            .map(emit)
            .collect::<Vec<_>>()
            .join(sep),
    }
}

fn emit_struct_lit(
    name: &str,
    fields: &[(String, RustExpr)],
    rest: Option<&RustExpr>,
) -> String {
    let mut field_strs: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            let val = emit(v);
            if *k == val {
                k.clone()
            } else {
                format!("{}: {}", k, val)
            }
        })
        .collect();
    if let Some(r) = rest {
        field_strs.push(format!("..{}", emit(r)));
    }
    if field_strs.is_empty() {
        if name.is_empty() {
            "serde_json::json!({})".to_string()
        } else {
            format!("{} {{}}", name)
        }
    } else if name.is_empty() {
        format!("{{ {} }}", field_strs.join(", "))
    } else {
        format!("{} {{ {} }}", name, field_strs.join(", "))
    }
}

fn emit_arm_body(body: &[RustExpr]) -> String {
    if body.is_empty() {
        return "()".to_string();
    }
    if body.len() == 1 {
        return emit(&body[0]);
    }
    let inner = emit_value_block_ir(body, "            ");
    format!("{{\n{}\n        }}", inner)
}

fn emit_maybe_paren(expr: &RustExpr) -> String {
    match expr {
        RustExpr::BinOp { .. } | RustExpr::Cast { .. } | RustExpr::Assign { .. } => {
            format!("({})", emit(expr))
        }
        _ => emit(expr),
    }
}

fn emit_block_expr(stmts: &[RustExpr], value: Option<&RustExpr>) -> String {
    if stmts.is_empty() && value.is_none() {
        return "{}".to_string();
    }
    let mut parts: Vec<String> = stmts.iter().map(emit).collect();
    if let Some(val) = value {
        parts.push(emit(val));
    }
    let multiline = parts.len() > 2 || parts.iter().any(|p| p.contains('\n'));
    if multiline {
        let n = parts.len();
        let rendered: Vec<String> = parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let is_last = i + 1 == n;
                let needs_semi = !is_last && !p.ends_with(';')
                    && (!p.ends_with('}') || p.contains(" = ") || p.starts_with("let "));
                let line = if needs_semi {
                    format!("{p};")
                } else {
                    p
                };
                indent_lines(&line, "    ")
            })
            .collect();
        format!("{{\n{}\n}}", rendered.join("\n"))
    } else {
        format!("{{ {} }}", parts.join("; "))
    }
}

fn indent_lines(s: &str, indent: &str) -> String {
    s.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{indent}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a brace-terminated expression needs a trailing `;` in statement position.
/// Match expressions, let bindings, and assignments used as statements need
/// semicolons; control flow (if, for, while, loop) and bare blocks do not.
fn needs_semi_despite_brace(expr: &RustExpr) -> bool {
    matches!(expr, RustExpr::Match { .. } | RustExpr::Let { .. } | RustExpr::Assign { .. })
}

fn emit_block(stmts: &[RustExpr], indent: &str) -> String {
    stmts
        .iter()
        .map(|e| {
            let rendered = emit(e);
            let needs_semi = !rendered.ends_with(';')
                && (!rendered.ends_with('}') || needs_semi_despite_brace(e));
            let line = if needs_semi {
                format!("{};", rendered)
            } else {
                rendered
            };
            indent_lines(&line, indent)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_value_block_ir(stmts: &[RustExpr], indent: &str) -> String {
    stmts
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let rendered = emit(e);
            let is_last = i + 1 == stmts.len();
            let needs_semi = !is_last && !rendered.ends_with(';')
                && (!rendered.ends_with('}') || needs_semi_despite_brace(e));
            let line = if needs_semi {
                format!("{};", rendered)
            } else {
                rendered
            };
            indent_lines(&line, indent)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
