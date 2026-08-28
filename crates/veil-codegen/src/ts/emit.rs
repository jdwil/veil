//! TypeScript expression emission — renders `TsExpr` to TypeScript source text.
//!
//! The main entry point is `emit_ts()` which recursively renders a `TsExpr` tree
//! into idiomatic TypeScript with proper indentation, semicolons, and braces.

use super::expr::{TsExpr, TsPattern, TsTemplatePart};

/// Render a `TsExpr` to its TypeScript source string.
///
/// This is the bottom of the TS codegen pipeline:
/// `VEIL Expr → lower_to_ts() → TsExpr → emit_ts() → String`
pub fn emit_ts(expr: &TsExpr) -> String {
    emit_ts_indent(expr, 0)
}

/// Render with indentation level (number of 2-space indents).
fn emit_ts_indent(expr: &TsExpr, indent: usize) -> String {
    match expr {
        // ── Literals ──────────────────────────────────────────────────────
        TsExpr::Ident { name, .. } => name.clone(),

        TsExpr::StringLit(s) => format!("\"{}\"", escape_ts_string(s)),

        TsExpr::TemplateLit { parts } => {
            let mut out = String::from("`");
            for part in parts {
                match part {
                    TsTemplatePart::Literal(s) => out.push_str(&escape_template_literal(s)),
                    TsTemplatePart::Expr(e) => {
                        out.push_str("${");
                        out.push_str(&emit_ts(e));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }

        TsExpr::IntLit(n) => n.to_string(),
        TsExpr::FloatLit(f) => format_float(*f),
        TsExpr::BoolLit(b) => b.to_string(),
        TsExpr::NullLit => "null".to_string(),
        TsExpr::UndefinedLit => "undefined".to_string(),

        TsExpr::ArrayLit { items, .. } => {
            if items.is_empty() {
                "[]".to_string()
            } else {
                let parts: Vec<String> = items.iter().map(emit_ts).collect();
                format!("[{}]", parts.join(", "))
            }
        }

        TsExpr::ObjectLit { fields, .. } => {
            if fields.is_empty() {
                "{}".to_string()
            } else {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| {
                        let val = emit_ts(v);
                        // Object-spread element: key "..." with a Spread value emits
                        // as `...base`, not a `key: value` property.
                        if k == "..." {
                            return val;
                        }
                        // Shorthand: `{ name }` when key === value ident
                        if let TsExpr::Ident { name, .. } = v {
                            if name == k {
                                return k.clone();
                            }
                        }
                        format!("{}: {}", k, val)
                    })
                    .collect();
                format!("{{ {} }}", parts.join(", "))
            }
        }

        // ── Operators ─────────────────────────────────────────────────────
        TsExpr::BinOp { left, op, right, .. } => {
            format!("{} {} {}", emit_ts(left), op.as_str(), emit_ts(right))
        }

        TsExpr::UnaryOp { op, expr } => {
            let inner = emit_ts(expr);
            let needs_parens = matches!(
                expr.as_ref(),
                TsExpr::BinOp { .. }
                    | TsExpr::NullishCoalesce { .. }
                    | TsExpr::UnaryOp { .. }
            );
            if needs_parens {
                format!("{}({})", op.as_str(), inner)
            } else {
                format!("{}{}", op.as_str(), inner)
            }
        }

        TsExpr::OptionalChain { base, field } => {
            format!("{}?.{}", emit_ts(base), field)
        }

        TsExpr::NullishCoalesce { left, right } => {
            format!("{} ?? {}", emit_ts(left), emit_ts(right))
        }

        // ── Access ────────────────────────────────────────────────────────
        TsExpr::FieldAccess { base, field, .. } => {
            format!("{}.{}", emit_ts(base), field)
        }

        TsExpr::Index { base, index } => {
            format!("{}[{}]", emit_ts(base), emit_ts(index))
        }

        // ── Calls ─────────────────────────────────────────────────────────
        TsExpr::MethodCall {
            receiver,
            method,
            args,
            is_async,
            ..
        } => {
            let recv = emit_ts(receiver);
            let arg_strs: Vec<String> = args.iter().map(emit_ts).collect();
            let call = format!("{}.{}({})", recv, method, arg_strs.join(", "));
            if *is_async {
                format!("await {}", call)
            } else {
                call
            }
        }

        TsExpr::FnCall { name, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(emit_ts).collect();
            format!("{}({})", name, arg_strs.join(", "))
        }

        TsExpr::NewCall { class, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(emit_ts).collect();
            format!("new {}({})", class, arg_strs.join(", "))
        }

        // ── Bindings ──────────────────────────────────────────────────────
        TsExpr::Const { name, ty, value } => {
            let ty_ann = ty.as_ref().map(|t| format!(": {}", t)).unwrap_or_default();
            format!("const {}{} = {}", name, ty_ann, emit_ts(value))
        }

        TsExpr::Let { name, ty, value } => {
            let ty_ann = ty.as_ref().map(|t| format!(": {}", t)).unwrap_or_default();
            format!("let {}{} = {}", name, ty_ann, emit_ts(value))
        }

        TsExpr::Destructure { pattern, value } => {
            let pat = match pattern {
                TsPattern::Object { fields } => format!("{{ {} }}", fields.join(", ")),
                TsPattern::Array { items } => format!("[{}]", items.join(", ")),
            };
            format!("const {} = {}", pat, emit_ts(value))
        }

        TsExpr::Assign { target, value } => {
            format!("{} = {}", emit_ts(target), emit_ts(value))
        }

        // ── Control Flow ──────────────────────────────────────────────────
        TsExpr::If {
            condition,
            then_body,
            else_body,
        } => {
            let cond = emit_ts(condition);
            let then_str = emit_body(then_body, indent + 1);
            match else_body {
                Some(eb) => {
                    let else_str = emit_body(eb, indent + 1);
                    format!(
                        "if ({}) {{\n{}\n{}}} else {{\n{}\n{}}}",
                        cond,
                        then_str,
                        indent_str(indent),
                        else_str,
                        indent_str(indent)
                    )
                }
                None => {
                    format!(
                        "if ({}) {{\n{}\n{}}}",
                        cond,
                        then_str,
                        indent_str(indent)
                    )
                }
            }
        }

        TsExpr::Ternary {
            condition,
            then_expr,
            else_expr,
        } => {
            format!(
                "{} ? {} : {}",
                emit_ts(condition),
                emit_ts(then_expr),
                emit_ts(else_expr)
            )
        }

        TsExpr::Switch {
            scrutinee,
            cases,
            default,
        } => {
            let scrut = emit_ts(scrutinee);
            let ind = indent_str(indent);
            let case_ind = indent_str(indent + 1);
            let body_ind = indent_str(indent + 2);
            let mut out = format!("switch ({}) {{\n", scrut);
            for (label, body) in cases {
                out.push_str(&format!("{}case \"{}\":\n", case_ind, label));
                for stmt in body {
                    out.push_str(&format!(
                        "{}{};\n",
                        body_ind,
                        emit_ts_indent(stmt, indent + 2)
                    ));
                }
                out.push_str(&format!("{}break;\n", body_ind));
            }
            if let Some(def) = default {
                out.push_str(&format!("{}default:\n", case_ind));
                for stmt in def {
                    out.push_str(&format!(
                        "{}{};\n",
                        body_ind,
                        emit_ts_indent(stmt, indent + 2)
                    ));
                }
            }
            out.push_str(&format!("{}}}", ind));
            out
        }

        TsExpr::For {
            binding,
            iterable,
            body,
        } => {
            let iter = emit_ts(iterable);
            let body_str = emit_body(body, indent + 1);
            format!(
                "for (const {} of {}) {{\n{}\n{}}}",
                binding,
                iter,
                body_str,
                indent_str(indent)
            )
        }

        TsExpr::ForIndex {
            index,
            binding,
            iterable,
            body,
        } => {
            let iter = emit_ts(iterable);
            let ind = indent_str(indent + 1);
            let body_str = emit_body(body, indent + 1);
            format!(
                "for (let {idx} = 0; {idx} < {iter}.length; {idx}++) {{\n\
                 {ind}const {binding} = {iter}[{idx}];\n\
                 {body}\n\
                 {close}}}",
                idx = index,
                iter = iter,
                ind = ind,
                binding = binding,
                body = body_str,
                close = indent_str(indent),
            )
        }

        TsExpr::While { condition, body } => {
            let cond = emit_ts(condition);
            let body_str = emit_body(body, indent + 1);
            format!(
                "while ({}) {{\n{}\n{}}}",
                cond,
                body_str,
                indent_str(indent)
            )
        }

        TsExpr::Loop { body } => {
            let body_str = emit_body(body, indent + 1);
            format!("while (true) {{\n{}\n{}}}", body_str, indent_str(indent))
        }

        // ── Functions ─────────────────────────────────────────────────────
        TsExpr::ArrowFn {
            params,
            body,
            is_async,
        } => {
            let async_prefix = if *is_async { "async " } else { "" };
            let param_str = params.join(", ");
            // Single-expression body: `(x) => x + 1`
            if body.len() == 1 && is_expression_node(&body[0]) {
                return format!(
                    "{}({}) => {}",
                    async_prefix,
                    param_str,
                    emit_ts(&body[0])
                );
            }
            let body_str = emit_body(body, indent + 1);
            format!(
                "{}({}) => {{\n{}\n{}}}",
                async_prefix,
                param_str,
                body_str,
                indent_str(indent)
            )
        }

        TsExpr::Return(value) => {
            // `return undefined` → bare `return` (idiomatic TS)
            if matches!(value.as_ref(), TsExpr::UndefinedLit) {
                "return".to_string()
            } else {
                format!("return {}", emit_ts(value))
            }
        }

        TsExpr::Await(expr) => {
            format!("await {}", emit_ts(expr))
        }

        TsExpr::Throw { message } => {
            format!("throw {}", emit_ts(message))
        }

        // ── TS-Specific ───────────────────────────────────────────────────
        TsExpr::TypeAssertion { expr, ty } => {
            format!("{} as {}", emit_ts(expr), ty)
        }

        TsExpr::NonNullAssertion(expr) => {
            format!("{}!", emit_ts(expr))
        }

        TsExpr::Spread(expr) => {
            format!("...{}", emit_ts(expr))
        }

        // ── Statements ────────────────────────────────────────────────────
        TsExpr::Break => "break".to_string(),
        TsExpr::Continue => "continue".to_string(),

        // ── Layer-provided / terminal ─────────────────────────────────────
        TsExpr::Noop => String::new(),
        TsExpr::LayerEmit(s) => s.clone(),
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Render a body (list of statements) with proper indentation and semicolons.
fn emit_body(stmts: &[TsExpr], indent: usize) -> String {
    let ind = indent_str(indent);
    stmts
        .iter()
        .map(|s| {
            let rendered = emit_ts_indent(s, indent);
            if needs_semicolon(s) {
                format!("{}{};", ind, rendered)
            } else {
                format!("{}{}", ind, rendered)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produce indentation string (2 spaces per level).
fn indent_str(level: usize) -> String {
    "  ".repeat(level)
}

/// Determine if a statement needs a trailing semicolon.
/// Block-level constructs (if, switch, for, while, loop, arrow fn with block body)
/// do NOT get semicolons.
fn needs_semicolon(expr: &TsExpr) -> bool {
    !matches!(
        expr,
        TsExpr::If { .. }
            | TsExpr::Switch { .. }
            | TsExpr::For { .. }
            | TsExpr::ForIndex { .. }
            | TsExpr::While { .. }
            | TsExpr::Loop { .. }
    )
}

/// Check if a node is a simple expression (suitable for arrow fn shorthand).
fn is_expression_node(expr: &TsExpr) -> bool {
    matches!(
        expr,
        TsExpr::Ident { .. }
            | TsExpr::StringLit(_)
            | TsExpr::IntLit(_)
            | TsExpr::FloatLit(_)
            | TsExpr::BoolLit(_)
            | TsExpr::NullLit
            | TsExpr::UndefinedLit
            | TsExpr::BinOp { .. }
            | TsExpr::UnaryOp { .. }
            | TsExpr::FieldAccess { .. }
            | TsExpr::OptionalChain { .. }
            | TsExpr::NullishCoalesce { .. }
            | TsExpr::Index { .. }
            | TsExpr::MethodCall { .. }
            | TsExpr::FnCall { .. }
            | TsExpr::NewCall { .. }
            | TsExpr::ArrayLit { .. }
            | TsExpr::ObjectLit { .. }
            | TsExpr::TemplateLit { .. }
            | TsExpr::Await(_)
            | TsExpr::TypeAssertion { .. }
            | TsExpr::NonNullAssertion(_)
            | TsExpr::Spread(_)
            | TsExpr::Ternary { .. }
    )
}

/// Escape special characters in a regular string literal.
fn escape_ts_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Escape special characters in template literal content.
fn escape_template_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

/// Format a float so that it always has a decimal point.
fn format_float(f: f64) -> String {
    let s = f.to_string();
    if s.contains('.') {
        s
    } else {
        format!("{}.0", s)
    }
}
