//! Typed intermediate representation for Rust expressions.
//!
//! `RustExpr` sits between VEIL AST lowering and final Rust text emission.
//! It preserves type and ownership information so downstream transforms
//! (clone insertion, `?` suppression in closures, etc.) operate on structure
//! rather than rendered strings.
//!
//! ## Module structure
//!
//! - `lower` — `lower_to_rust()` and per-expression lowering helpers
//! - `ownership` — `apply_ownership()`, clone/borrow helpers, try-suppression in closures
//! - `tests` — unit tests

mod lower;
mod ownership;
#[cfg(test)]
mod tests;

use super::types::rust_string_lit;

// Re-export public API
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

    /// If expression / if-else block
    If {
        condition: Box<RustExpr>,
        then_body: Vec<RustExpr>,
        else_body: Option<Vec<RustExpr>>,
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

    /// Pre-rendered Rust statement (assignment, reassignment, complex expressions
    /// that haven't been fully decomposed into structural nodes yet).
    /// Unlike the deleted `Raw`, this is explicitly a STATEMENT that produces
    /// no value for ownership purposes.
    Statement { text: String, ty: Option<RustType> },

    /// Layer/action template output — pre-rendered by interpolate_action_template.
    LayerEmit(String),

    /// compile_error!("...") — intentional error marker in generated code.
    CompileError(String),

    /// Return statement: `return expr`
    Return { value: Box<RustExpr>, wraps_ok: bool },

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
/// every migrated expression category. Pre-rendered expressions use
/// `RustExpr::Statement` which is already a rendered string.
pub fn emit(expr: &RustExpr) -> String {
    match expr {
        RustExpr::Statement { text, .. } => text.clone(),
        RustExpr::LayerEmit(s) => s.clone(),
        RustExpr::CompileError(msg) => format!("compile_error!(\"{}\")", msg),
        RustExpr::Return { value, wraps_ok } => {
            if *wraps_ok {
                format!("return Ok({})", emit(value))
            } else {
                format!("return {}", emit(value))
            }
        }
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
            // Single-expression ternary: `if cond { expr } else { expr }`
            if then_body.len() == 1 && else_body.as_ref().is_some_and(|b| b.len() == 1) {
                let then_str = emit(&then_body[0]);
                let else_str = emit(&else_body.as_ref().unwrap()[0]);
                if !then_str.contains('\n') && !else_str.contains('\n') {
                    return format!("if {} {{ {} }} else {{ {} }}", cond, then_str, else_str);
                }
            }
            // Multi-line block format
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
            let body_str = emit_block(body, "    ");
            format!("for {} in {} {{\n{}\n}}", binding, emit(iterable), body_str)
        }
        RustExpr::While { condition, body, .. } => {
            let body_str = emit_block(body, "        ");
            format!("while {} {{\n{}\n    }}", emit(condition), body_str)
        }
        RustExpr::Loop { body, .. } => {
            let body_str = emit_block(body, "    ");
            format!("loop {{\n{}\n}}", body_str)
        }
    }
}

/// Render a block of statements with indentation and semicolons.
/// Used by If/Match/For/While emit to produce multi-line bodies.
fn emit_block(stmts: &[RustExpr], indent: &str) -> String {
    stmts
        .iter()
        .map(|e| {
            let rendered = emit(e);
            let line = if !rendered.ends_with('}') && !rendered.ends_with(';') {
                format!("{};", rendered)
            } else {
                rendered
            };
            // Indent each line of the rendered statement
            line.lines()
                .map(|l| if l.is_empty() { String::new() } else { format!("{}{}", indent, l) })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a block where the last expression is a value (no trailing semicolon).
fn emit_value_block_ir(stmts: &[RustExpr], indent: &str) -> String {
    stmts
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let rendered = emit(e);
            let is_last = i + 1 == stmts.len();
            let line = if !is_last && !rendered.ends_with('}') && !rendered.ends_with(';') {
                format!("{};", rendered)
            } else {
                rendered
            };
            line.lines()
                .map(|l| if l.is_empty() { String::new() } else { format!("{}{}", indent, l) })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
