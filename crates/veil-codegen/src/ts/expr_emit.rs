//! TypeScript expression emitter for VEIL function bodies.
//!
//! `expr_to_display` is a diagnostic formatter and must not be used for TS
//! output. This module renders nested `for` / `if` / `let` / interpolation
//! as valid TypeScript. Identifiers are preserved as written so Svelte 5
//! `$state` field names match the VEIL source.

use std::collections::HashSet;

use veil_ir::ast::{
    ActionExpr, BinOp, Expr, Pattern, StringPart, TypeExpr, UnaryOp,
};

/// Options for TypeScript fn-body emission.
///
/// Svelte 5 forbids exporting a `$state` binding that is later reassigned.
/// Store codegen therefore emits `export const StoreName = $state({ ... })` and
/// qualifies state field reads/writes as `StoreName.field`.
#[derive(Clone, Debug, Default)]
pub struct TsExprEmitOpts {
    /// Object prefix for state fields (the VEIL store construct name).
    pub qualify_prefix: Option<String>,
    /// Field names that live on `qualify_prefix`.
    pub qualify_names: HashSet<String>,
}

impl TsExprEmitOpts {
    pub fn for_store(
        store_name: &str,
        state_fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            qualify_prefix: Some(store_name.to_string()),
            qualify_names: state_fields.into_iter().map(Into::into).collect(),
        }
    }
}

/// Render one VEIL expression as a TypeScript statement at `indent` (2-space levels).
///
/// Block forms (`if`, `for`, `while`, `match`, …) include nested bodies and do
/// not take a trailing semicolon. Empty / decorative nodes return `""`.
pub fn expr_to_typescript(expr: &Expr, indent: usize) -> String {
    emit_stmt(expr, indent, &TsExprEmitOpts::default())
}

/// Render a sequence of statements, skipping empty nodes, each on its own line
/// including a trailing newline after the last statement.
pub fn emit_typescript_stmts(body: &[Expr], indent: usize) -> String {
    emit_typescript_stmts_with(body, indent, &TsExprEmitOpts::default())
}

/// Like [`emit_typescript_stmts`], with store-field qualification.
pub fn emit_typescript_stmts_with(
    body: &[Expr],
    indent: usize,
    opts: &TsExprEmitOpts,
) -> String {
    let mut out = String::new();
    for expr in body {
        let line = emit_stmt(expr, indent, opts);
        if line.is_empty() {
            continue;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Emit a single expression in VALUE position (e.g. a `derived` RHS or an
/// argument), so control-flow like `if/else` becomes a ternary rather than a
/// statement. Prefer this when the result must be an expression.
pub fn emit_expr_value_with(expr: &Expr, indent: usize, opts: &TsExprEmitOpts) -> String {
    emit_expr(expr, indent, opts)
}

/// `export const Name = $state({ field: default, ... });`
pub fn emit_store_state(store_name: &str, fields: &[(String, TypeExpr)]) -> String {
    let mut out = format!("export const {store_name} = $state({{\n");
    for (name, ty) in fields {
        out.push_str(&format!("  {}: {},\n", name, store_field_default(ty)));
    }
    out.push_str("});\n");
    out
}

fn store_field_default(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => match n.as_str() {
            "Str" | "String" | "Id" | "UUID" | "Dt" | "DateTime" => "''".into(),
            "Bool" => "false".into(),
            "Int" | "F64" | "Float" => "0".into(),
            "Json" => "[] as any".into(),
            _ => "null as any".into(),
        },
        TypeExpr::Optional(_) => format!("null as {}", type_to_typescript(ty)),
        TypeExpr::List(_) => "[]".into(),
        TypeExpr::Map(_, _) => "{} as any".into(),
        _ => "null as any".into(),
    }
}

fn qualify(name: &str, opts: &TsExprEmitOpts) -> String {
    if opts.qualify_names.contains(name) {
        if let Some(prefix) = &opts.qualify_prefix {
            return format!("{prefix}.{name}");
        }
    }
    name.to_string()
}

/// Map a VEIL type to a TypeScript type annotation.
///
/// `Json` → `any` (not `Record<string, unknown>`) so array literals type-check.
pub fn type_to_typescript(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => match n.as_str() {
            "Str" | "String" | "Id" | "UUID" => "string".into(),
            "Bool" => "boolean".into(),
            "Int" | "F64" | "Float" => "number".into(),
            "Json" => "any".into(),
            "Bytes" => "Uint8Array".into(),
            "Dt" | "DateTime" => "string".into(),
            other => other.to_string(),
        },
        TypeExpr::List(inner) => format!("{}[]", type_to_typescript(inner)),
        TypeExpr::Optional(inner) => format!("{} | null", type_to_typescript(inner)),
        TypeExpr::Map(_, v) => format!("Record<string, {}>", type_to_typescript(v)),
        TypeExpr::Set(inner) => format!("Set<{}>", type_to_typescript(inner)),
        TypeExpr::Result(Some(inner)) => format!("Promise<{}>", type_to_typescript(inner)),
        TypeExpr::Result(None) => "Promise<void>".into(),
        TypeExpr::Generic(name, args) => {
            let inner = args
                .iter()
                .map(type_to_typescript)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}<{}>", name, inner)
        }
        TypeExpr::Tuple(items) => {
            let inner = items
                .iter()
                .map(type_to_typescript)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{}]", inner)
        }
        TypeExpr::Array(inner, size) => {
            let t = type_to_typescript(inner);
            let parts: Vec<String> = (0..*size).map(|_| t.clone()).collect();
            format!("[{}]", parts.join(", "))
        }
        TypeExpr::Ref(inner, _) | TypeExpr::Dyn(inner) | TypeExpr::ImplTrait(inner) => {
            type_to_typescript(inner)
        }
        TypeExpr::FnPtr(params, ret) => {
            let p = params
                .iter()
                .enumerate()
                .map(|(i, t)| format!("arg{}: {}", i, type_to_typescript(t)))
                .collect::<Vec<_>>()
                .join(", ");
            let r = ret
                .as_ref()
                .map(|t| type_to_typescript(t))
                .unwrap_or_else(|| "void".into());
            format!("({}) => {}", p, r)
        }
        TypeExpr::LitStr(_) => "string".into(),
    }
}

fn pad(indent: usize) -> String {
    "  ".repeat(indent)
}

fn emit_stmt(expr: &Expr, indent: usize, opts: &TsExprEmitOpts) -> String {
    let ind = pad(indent);
    match expr {
        Expr::Stock => String::new(),
        Expr::Ident(n) if n == "let" || n == "noop" => String::new(),
        Expr::IfExpr(ie) => {
            let cond = emit_expr(&ie.condition, indent, opts);
            let then_s = emit_block_inner(&ie.then_body, indent, opts);
            match &ie.else_body {
                Some(eb) => {
                    let else_s = emit_block_inner(eb, indent, opts);
                    format!(
                        "{ind}if ({cond}) {{{then_s}}} else {{{else_s}}}"
                    )
                }
                None => format!("{ind}if ({cond}) {{{then_s}}}"),
            }
        }
        Expr::ForLoop {
            binding,
            index,
            iterable,
            body,
        } => {
            let iter = emit_expr(iterable, indent, opts);
            match index {
                Some(idx) => {
                    let mut stmts = String::new();
                    stmts.push_str(&format!(
                        "{}const {} = {}[{}];\n",
                        pad(indent + 1),
                        binding,
                        iter,
                        idx
                    ));
                    stmts.push_str(&emit_typescript_stmts_with(body, indent + 1, opts));
                    format!(
                        "{ind}for (let {idx} = 0; {idx} < {iter}.length; {idx}++) {{\n{}\n{ind}}}",
                        stmts.trim_end()
                    )
                }
                None => {
                    let inner = emit_block_inner(body, indent, opts);
                    format!("{ind}for (const {binding} of {iter}) {{{inner}}}")
                }
            }
        }
        Expr::WhileLoop { condition, body } => {
            let cond = emit_expr(condition, indent, opts);
            let inner = emit_block_inner(body, indent, opts);
            format!("{ind}while ({cond}) {{{inner}}}")
        }
        Expr::Loop(body) => {
            let inner = emit_block_inner(body, indent, opts);
            format!("{ind}while (true) {{{inner}}}")
        }
        Expr::Match(scrutinee, arms) => emit_match(scrutinee, arms, indent, opts),
        Expr::IfLet {
            pattern: _,
            expr: inner,
            then_body,
            else_body,
        } => {
            let cond = format!("{} != null", emit_expr(inner, indent, opts));
            let then_s = emit_block_inner(then_body, indent, opts);
            match else_body {
                Some(eb) => {
                    let else_s = emit_block_inner(eb, indent, opts);
                    format!("{ind}if ({cond}) {{{then_s}}} else {{{else_s}}}")
                }
                None => format!("{ind}if ({cond}) {{{then_s}}}"),
            }
        }
        Expr::WhileLet {
            pattern: _,
            expr: inner,
            body,
        } => {
            let cond = format!("{} != null", emit_expr(inner, indent, opts));
            let inner_s = emit_block_inner(body, indent, opts);
            format!("{ind}while ({cond}) {{{inner_s}}}")
        }
        Expr::DoBlock(body) => {
            let inner = emit_block_inner(body, indent, opts);
            format!("{ind}{{{inner}}}")
        }
        Expr::Assign(name, rhs, ty) => {
            format!("{ind}{};", emit_binding(name, rhs, ty, false, indent, opts))
        }
        Expr::MutAssign(name, rhs, ty) => {
            format!("{ind}{};", emit_binding(name, rhs, ty, true, indent, opts))
        }
        Expr::LetPattern(pattern, rhs, _) => {
            format!(
                "{ind}const {} = {};",
                pattern_to_ts(pattern),
                emit_expr(rhs, indent, opts)
            )
        }
        Expr::Return(inner) => match inner.as_ref() {
            Expr::Ident(n) if n == "Ok" => format!("{ind}return;"),
            Expr::Tuple(items) if items.is_empty() => format!("{ind}return;"),
            other => format!("{ind}return {};", emit_expr(other, indent, opts)),
        },
        Expr::Break => format!("{ind}break;"),
        Expr::Continue => format!("{ind}continue;"),
        other => {
            let e = emit_expr(other, indent, opts);
            if e.is_empty() {
                String::new()
            } else {
                format!("{ind}{e};")
            }
        }
    }
}

/// `{` + optional body + `}` inner: leading newline, body, newline, ready for closer.
fn emit_block_inner(body: &[Expr], indent: usize, opts: &TsExprEmitOpts) -> String {
    let inner = emit_typescript_stmts_with(body, indent + 1, opts);
    if inner.is_empty() {
        format!("\n{}", pad(indent))
    } else {
        format!("\n{}\n{}", inner.trim_end(), pad(indent))
    }
}

fn emit_binding(
    name: &str,
    rhs: &Expr,
    ty: &Option<TypeExpr>,
    is_mut_decl: bool,
    indent: usize,
    opts: &TsExprEmitOpts,
) -> String {
    let value = emit_expr(rhs, indent, opts);
    // Field write (`loan.returned = true`) is never a new binding.
    if name.contains('.') {
        let qualified = if let Some((base, rest)) = name.split_once('.') {
            format!("{}.{}", qualify(base, opts), rest)
        } else {
            name.to_string()
        };
        return format!("{} = {}", qualified, value);
    }
    if opts.qualify_names.contains(name) {
        return format!("{} = {}", qualify(name, opts), value);
    }
    if is_mut_decl {
        match ty {
            Some(t) => format!("let {}: {} = {}", name, decl_type(t, rhs), value),
            None => format!("let {} = {}", name, value),
        }
    } else if let Some(t) = ty {
        // Typed `name: T = expr` (and `let name: T = expr`) → declaration.
        format!("let {}: {} = {}", name, decl_type(t, rhs), value)
    } else {
        // Untyped `name = expr` → assignment to existing state / local.
        format!("{} = {}", name, value)
    }
}

fn decl_type(ty: &TypeExpr, rhs: &Expr) -> String {
    if matches!(ty, TypeExpr::Named(n) if n == "Json") && matches!(rhs, Expr::ArrayLit(_)) {
        return "any[]".into();
    }
    type_to_typescript(ty)
}

fn emit_expr(expr: &Expr, indent: usize, opts: &TsExprEmitOpts) -> String {
    match expr {
        Expr::Stock => String::new(),
        Expr::Ident(n) if n == "let" || n == "noop" => String::new(),
        Expr::Ident(n) if n == "null" || n == "None" => "null".into(),
        Expr::Ident(n) => qualify(n, opts),
        Expr::FieldAccess(base, field) => format!("{}.{}", emit_expr(base, indent, opts), field),
        Expr::Call(call) => emit_call(call, indent, opts),
        Expr::BinaryOp(op) => {
            // Use strict equality (`===`/`!==`) by default, but keep loose
            // equality for null/None comparisons — `x == null` is the JS idiom
            // that matches both `null` and `undefined`.
            let is_null_cmp = matches!(op.op, BinOp::Eq | BinOp::NotEq)
                && (is_null_literal(&op.left) || is_null_literal(&op.right));
            let op_str = match op.op {
                BinOp::Eq if !is_null_cmp => "===",
                BinOp::NotEq if !is_null_cmp => "!==",
                _ => binop_to_ts(&op.op),
            };
            format!(
                "{} {} {}",
                emit_expr(&op.left, indent, opts),
                op_str,
                emit_expr(&op.right, indent, opts)
            )
        }
        Expr::UnaryOp(op) => {
            let inner = emit_expr(&op.expr, indent, opts);
            match op.op {
                UnaryOp::Not => format!("!{}", maybe_paren(&op.expr, inner)),
                UnaryOp::Neg => format!("-{}", maybe_paren(&op.expr, inner)),
            }
        }
        Expr::IfExpr(ie) => emit_if_as_value(ie, indent, opts),
        Expr::Assign(name, rhs, ty) => emit_binding(name, rhs, ty, false, indent, opts),
        Expr::MutAssign(name, rhs, ty) => emit_binding(name, rhs, ty, true, indent, opts),
        Expr::StringLit(s) => format!("\"{}\"", escape_ts_string(s)),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => {
            let s = f.to_string();
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                format!("{s}.0")
            } else {
                s
            }
        }
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("return {}", emit_expr(inner, indent, opts)),
        Expr::Action(a) => emit_action(a, indent, opts),
        Expr::StructLit(name, fields) => emit_object_lit(name, fields, indent, opts),
        Expr::Match(scrutinee, arms) => emit_match(scrutinee, arms, indent, opts),
        Expr::ForLoop { .. }
        | Expr::WhileLoop { .. }
        | Expr::Loop(_)
        | Expr::IfLet { .. }
        | Expr::WhileLet { .. }
        | Expr::DoBlock(_) => emit_stmt(expr, indent, opts),
        Expr::Closure { params, body } => emit_closure(params, body, indent, opts),
        Expr::Tuple(items) => {
            let parts = items
                .iter()
                .map(|e| emit_expr(e, indent, opts))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{}]", parts)
        }
        Expr::StringInterp(parts) => emit_template_lit(parts, indent, opts),
        Expr::Await(inner) => format!("await {}", emit_expr(inner, indent, opts)),
        Expr::Break => "break".into(),
        Expr::Continue => "continue".into(),
        Expr::Index(base, idx) => {
            format!(
                "{}[{}]",
                emit_expr(base, indent, opts),
                emit_expr(idx, indent, opts)
            )
        }
        Expr::IndexAssign { target, value } => format!(
            "{} = {}",
            emit_expr(target, indent, opts),
            emit_expr(value, indent, opts)
        ),
        Expr::New { class, args } => {
            let a = args
                .iter()
                .map(|e| emit_expr(e, indent, opts))
                .collect::<Vec<_>>()
                .join(", ");
            format!("new {}({})", class, a)
        }
        Expr::ArrayLit(items) => {
            let parts = items
                .iter()
                .map(|e| emit_expr(e, indent, opts))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{}]", parts)
        }
        Expr::Spread(inner) => format!("...{}", emit_expr(inner, indent, opts)),
        Expr::Range { start, end, .. } => {
            let s = start
                .as_ref()
                .map(|e| emit_expr(e, indent, opts))
                .unwrap_or_else(|| "0".into());
            let e = end
                .as_ref()
                .map(|e| emit_expr(e, indent, opts))
                .unwrap_or_else(|| "0".into());
            format!("[{s}, {e}]")
        }
        Expr::Cast(inner, ty) => {
            format!("{} as {}", emit_expr(inner, indent, opts), map_cast_type(ty))
        }
        Expr::Try(inner) => emit_expr(inner, indent, opts),
        Expr::Require(inner) => format!("{}!", emit_expr(inner, indent, opts)),
        Expr::StructUpdate { name: _, fields, base } => {
            let mut parts = vec![format!("...{}", emit_expr(base, indent, opts))];
            for (k, v) in fields {
                parts.push(format!("{}: {}", k, emit_expr(v, indent, opts)));
            }
            format!("{{ {} }}", parts.join(", "))
        }
        Expr::LetPattern(pattern, rhs, _) => {
            format!(
                "const {} = {}",
                pattern_to_ts(pattern),
                emit_expr(rhs, indent, opts)
            )
        }
    }
}

fn maybe_paren(inner: &Expr, rendered: String) -> String {
    if matches!(inner, Expr::BinaryOp(_) | Expr::UnaryOp(_)) {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn emit_if_as_value(ie: &veil_ir::ast::IfExprData, indent: usize, opts: &TsExprEmitOpts) -> String {
    let cond = emit_expr(&ie.condition, indent, opts);
    let then_simple = ie.then_body.len() == 1 && is_value_expr(&ie.then_body[0]);
    let else_simple = ie
        .else_body
        .as_ref()
        .map(|b| b.len() == 1 && is_value_expr(&b[0]))
        .unwrap_or(false);
    if then_simple {
        let t = emit_expr(&ie.then_body[0], indent, opts);
        if let Some(eb) = &ie.else_body {
            if else_simple {
                return format!(
                    "{} ? {} : {}",
                    cond,
                    t,
                    emit_expr(&eb[0], indent, opts)
                );
            }
        } else {
            return format!("{} ? {} : undefined", cond, t);
        }
    }
    // Fall back to a statement-shaped if wrapped as an IIFE so it is still valid TS.
    let stmt = emit_stmt(&Expr::IfExpr(ie.clone()), indent, opts);
    format!("(() => {{\n{}\n{}}})()", stmt, pad(indent))
}

fn is_value_expr(expr: &Expr) -> bool {
    !matches!(
        expr,
        Expr::Assign(_, _, _)
            | Expr::MutAssign(_, _, _)
            | Expr::LetPattern(_, _, _)
            | Expr::Return(_)
            | Expr::Break
            | Expr::Continue
            | Expr::ForLoop { .. }
            | Expr::WhileLoop { .. }
            | Expr::Loop(_)
            | Expr::IfLet { .. }
            | Expr::WhileLet { .. }
            | Expr::DoBlock(_)
            | Expr::Match(_, _)
    )
}

fn emit_call(call: &veil_ir::ast::CallExpr, indent: usize, opts: &TsExprEmitOpts) -> String {
    let args = call
        .args
        .iter()
        .map(|a| emit_expr(a, indent, opts))
        .collect::<Vec<_>>()
        .join(", ");
    let method = call.method.trim_end_matches(['!', '?']);
    if let Some(recv) = &call.receiver {
        format!("{}.{}({})", emit_expr(recv, indent, opts), method, args)
    } else if method.is_empty() {
        format!("{}({})", call.target, args)
    } else if call.target.is_empty() {
        format!("{}({})", method, args)
    } else {
        format!("{}.{}({})", call.target, method, args)
    }
}

fn emit_object_lit(
    _name: &str,
    fields: &[(String, Expr)],
    indent: usize,
    opts: &TsExprEmitOpts,
) -> String {
    if fields.is_empty() {
        return "{}".into();
    }
    let parts = fields
        .iter()
        .map(|(k, v)| {
            let val = emit_expr(v, indent, opts);
            // Object-spread element: key "..." emits as `...base`.
            if k == "..." {
                return val;
            }
            if let Expr::Ident(n) = v {
                if n == k && !opts.qualify_names.contains(n) {
                    return k.clone();
                }
            }
            format!("{k}: {val}")
        })
        .collect::<Vec<_>>();
    format!("{{ {} }}", parts.join(", "))
}

fn emit_template_lit(parts: &[StringPart], indent: usize, opts: &TsExprEmitOpts) -> String {
    let mut out = String::from("`");
    for part in parts {
        match part {
            StringPart::Literal(s) => out.push_str(&escape_template_literal(s)),
            StringPart::Expr(e) => {
                out.push_str("${");
                out.push_str(&emit_expr(e, indent, opts));
                out.push('}');
            }
        }
    }
    out.push('`');
    out
}

fn emit_closure(params: &[String], body: &[Expr], indent: usize, opts: &TsExprEmitOpts) -> String {
    let param_str = params.join(", ");
    if body.len() == 1 && is_value_expr(&body[0]) {
        let rendered = emit_expr(&body[0], indent, opts);
        // An arrow whose body is an object literal must wrap it in parens,
        // otherwise `=> { ... }` is parsed as a block body (hard TS error).
        if expr_is_object_literal(&body[0]) {
            return format!("({}) => ({})", param_str, rendered);
        }
        return format!("({}) => {}", param_str, rendered);
    }
    let inner = emit_block_inner(body, indent, opts);
    format!("({}) => {{{inner}}}", param_str)
}

/// True when an expression renders as a `{ ... }` object literal, so an arrow
/// body must wrap it in parens: `(x) => ({ ... })`.
fn expr_is_object_literal(e: &Expr) -> bool {
    matches!(
        e,
        // Anonymous record literal `{ ... }` (StructLit with empty type name)
        Expr::StructLit(name, _) if name.is_empty()
    ) || matches!(e, Expr::StructUpdate { .. })
}

fn emit_match(
    scrutinee: &Expr,
    arms: &[veil_ir::ast::MatchArm],
    indent: usize,
    opts: &TsExprEmitOpts,
) -> String {
    let ind = pad(indent);
    let case_ind = pad(indent + 1);
    let body_ind_n = indent + 2;
    let mut out = format!(
        "{}switch ({}) {{\n",
        ind,
        emit_expr(scrutinee, indent, opts)
    );
    for arm in arms {
        let is_default = arm.pattern == "_"
            || matches!(&arm.rich_pattern, Some(Pattern::Wildcard));
        if is_default {
            out.push_str(&format!("{case_ind}default:\n"));
        } else {
            out.push_str(&format!(
                "{case_ind}case {}:\n",
                match_case_label(&arm.pattern)
            ));
        }
        let body = emit_typescript_stmts_with(&arm.body, body_ind_n, opts);
        out.push_str(&body);
        if !is_default {
            out.push_str(&format!("{}break;\n", pad(body_ind_n)));
        }
    }
    out.push_str(&format!("{ind}}}"));
    out
}

fn match_case_label(pattern: &str) -> String {
    let p = pattern.trim();
    if p.parse::<i64>().is_ok() || p == "true" || p == "false" || p == "null" {
        p.to_string()
    } else if (p.starts_with('"') && p.ends_with('"')) || (p.starts_with('\'') && p.ends_with('\''))
    {
        p.to_string()
    } else {
        format!("\"{}\"", escape_ts_string(p))
    }
}

fn emit_action(a: &ActionExpr, indent: usize, opts: &TsExprEmitOpts) -> String {
    let core = if !a.named_args.is_empty() {
        let fields = a
            .named_args
            .iter()
            .map(|(k, v)| {
                let vs = emit_expr(v, indent, opts);
                if k == &vs {
                    k.clone()
                } else {
                    format!("{k}: {vs}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        if a.target.is_empty() {
            format!("{}({{ {} }})", a.keyword, fields)
        } else if a.method.is_empty() {
            format!("{}({})", a.target, fields)
        } else {
            format!(
                "{}.{}({{ {} }})",
                a.target,
                a.method.trim_end_matches(['!', '?']),
                fields
            )
        }
    } else {
        let args = a
            .args
            .iter()
            .map(|e| emit_expr(e, indent, opts))
            .collect::<Vec<_>>()
            .join(", ");
        if !a.target.is_empty() && !a.method.is_empty() {
            format!(
                "{}.{}({})",
                a.target,
                a.method.trim_end_matches(['!', '?']),
                args
            )
        } else if !a.target.is_empty() {
            format!("{}({})", a.target, args)
        } else {
            format!("{}({})", a.keyword, args)
        }
    };
    match &a.result_binding {
        Some(b) => format!("{b} = {core}"),
        None => core,
    }
}

fn pattern_to_ts(pat: &Pattern) -> String {
    match pat {
        Pattern::Ident(s) => s.clone(),
        Pattern::Wildcard => "_".into(),
        Pattern::Tuple(parts) => {
            let inner = parts
                .iter()
                .map(pattern_to_ts)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Pattern::Struct(_, fields, has_rest) => {
            let mut fs: Vec<String> = fields
                .iter()
                .map(|(k, v)| match v {
                    Some(p) => format!("{}: {}", k, pattern_to_ts(p)),
                    None => k.clone(),
                })
                .collect();
            if *has_rest {
                fs.push("...rest".into());
            }
            format!("{{ {} }}", fs.join(", "))
        }
        other => other.to_string_repr(),
    }
}

/// True when an expression is a null/None literal (`null`, `None`), used to
/// keep loose equality for JS null-check idioms (`x == null`).
fn is_null_literal(e: &Expr) -> bool {
    matches!(e, Expr::Ident(n) if n == "null" || n == "None")
}

fn binop_to_ts(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn map_cast_type(ty_name: &str) -> String {
    match ty_name {
        "Str" | "String" => "string".into(),
        "Int" | "i64" | "i32" | "u64" | "u32" | "F64" | "f64" | "Float" => "number".into(),
        "Bool" | "bool" => "boolean".into(),
        "Json" => "any".into(),
        other => other.to_string(),
    }
}

fn escape_ts_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn escape_template_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::ast::{BinaryOpExpr, CallExpr, IfExprData};
    use veil_ir::span::Span;

    fn span() -> Span {
        Span::new(0, 0)
    }

    fn ident(n: &str) -> Expr {
        Expr::Ident(n.into())
    }

    fn field(base: Expr, name: &str) -> Expr {
        Expr::FieldAccess(Box::new(base), name.into())
    }

    fn call_recv(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr::Call(CallExpr {
            target: String::new(),
            method: method.into(),
            args,
            receiver: Some(Box::new(recv)),
            sugar: None,
            span: span(),
        })
    }

    fn call_free(target: &str, method: &str, args: Vec<Expr>) -> Expr {
        Expr::Call(CallExpr {
            target: target.into(),
            method: method.into(),
            args,
            receiver: None,
            sugar: None,
            span: span(),
        })
    }

    fn named(ty: &str) -> TypeExpr {
        TypeExpr::Named(ty.into())
    }

    #[test]
    fn typed_assign_emits_let() {
        let expr = Expr::Assign(
            "result".into(),
            Box::new(call_free("ApiClient", "fetch", vec![
                Expr::StringLit("/api".into()),
                Expr::StructLit(String::new(), vec![]),
            ])),
            Some(named("Json")),
        );
        assert_eq!(
            expr_to_typescript(&expr, 1),
            "  let result: any = ApiClient.fetch(\"/api\", {});"
        );
    }

    #[test]
    fn untyped_assign_is_reassignment() {
        let expr = Expr::Assign("loading".into(), Box::new(Expr::BoolLit(true)), None);
        assert_eq!(expr_to_typescript(&expr, 1), "  loading = true;");
    }

    #[test]
    fn store_fields_qualify_as_object_properties() {
        let opts = TsExprEmitOpts::for_store("MyStore", ["count", "items", "loading"]);
        let expr = Expr::Assign(
            "count".into(),
            Box::new(Expr::BinaryOp(BinaryOpExpr {
                left: Box::new(ident("count")),
                op: BinOp::Add,
                right: Box::new(Expr::IntLit(1)),
            })),
            None,
        );
        assert_eq!(
            emit_stmt(&expr, 1, &opts),
            "  MyStore.count = MyStore.count + 1;"
        );
        let concat = Expr::Assign(
            "items".into(),
            Box::new(call_recv(ident("items"), "concat", vec![ident("item")])),
            None,
        );
        assert_eq!(
            emit_stmt(&concat, 1, &opts),
            "  MyStore.items = MyStore.items.concat(item);"
        );
    }

    #[test]
    fn mut_assign_json_array_is_any_vec() {
        let expr = Expr::MutAssign(
            "allMenu".into(),
            Box::new(Expr::ArrayLit(vec![])),
            Some(named("Json")),
        );
        assert_eq!(expr_to_typescript(&expr, 1), "  let allMenu: any[] = [];");
    }

    #[test]
    fn decorative_let_ident_is_skipped() {
        assert_eq!(expr_to_typescript(&ident("let"), 1), "");
    }

    #[test]
    fn return_emits_return() {
        let expr = Expr::Return(Box::new(ident("result")));
        assert_eq!(expr_to_typescript(&expr, 1), "  return result;");
    }

    #[test]
    fn string_interp_emits_template_literal() {
        let expr = Expr::StringInterp(vec![
            StringPart::Literal("hello ".into()),
            StringPart::Expr(ident("name")),
        ]);
        assert_eq!(
            emit_expr(&expr, 0, &TsExprEmitOpts::default()),
            "`hello ${name}`"
        );
        assert_eq!(expr_to_typescript(&expr, 0), "`hello ${name}`;");
    }

    #[test]
    fn closure_emits_arrow() {
        let expr = Expr::Closure {
            params: vec!["x".into()],
            body: vec![Expr::BinaryOp(BinaryOpExpr {
                left: Box::new(ident("x")),
                op: BinOp::Add,
                right: Box::new(Expr::IntLit(1)),
            })],
        };
        assert_eq!(
            emit_expr(&expr, 0, &TsExprEmitOpts::default()),
            "(x) => x + 1"
        );
    }

    #[test]
    fn nested_for_if_for() {
        let inner_assign = Expr::Assign(
            "allMenu".into(),
            Box::new(call_recv(ident("allMenu"), "concat", vec![ident("item")])),
            None,
        );
        let inner_for = Expr::ForLoop {
            binding: "item".into(),
            index: None,
            iterable: Box::new(field(field(ident("contrib"), "slots"), "main_menu")),
            body: vec![inner_assign],
        };
        let if_expr = Expr::IfExpr(IfExprData {
            condition: Box::new(field(ident("contrib"), "enabled")),
            then_body: vec![inner_for],
            else_body: None,
        });
        let outer = Expr::ForLoop {
            binding: "contrib".into(),
            index: None,
            iterable: Box::new(ident("contributions")),
            body: vec![if_expr],
        };
        let got = expr_to_typescript(&outer, 1);
        let expected = concat!(
            "  for (const contrib of contributions) {\n",
            "    if (contrib.enabled) {\n",
            "      for (const item of contrib.slots.main_menu) {\n",
            "        allMenu = allMenu.concat(item);\n",
            "      }\n",
            "    }\n",
            "  }",
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn type_mapping() {
        assert_eq!(type_to_typescript(&named("Str")), "string");
        assert_eq!(type_to_typescript(&named("Bool")), "boolean");
        assert_eq!(type_to_typescript(&named("Int")), "number");
        assert_eq!(type_to_typescript(&named("Json")), "any");
        assert_eq!(
            type_to_typescript(&TypeExpr::List(Box::new(named("Str")))),
            "string[]"
        );
        assert_eq!(
            type_to_typescript(&TypeExpr::Optional(Box::new(named("Int")))),
            "number | null"
        );
    }

    #[test]
    fn null_check_keeps_double_equals() {
        let expr = Expr::IfExpr(IfExprData {
            condition: Box::new(Expr::BinaryOp(BinaryOpExpr {
                left: Box::new(ident("x")),
                op: BinOp::NotEq,
                right: Box::new(ident("null")),
            })),
            then_body: vec![Expr::Return(Box::new(ident("x")))],
            else_body: None,
        });
        let got = expr_to_typescript(&expr, 0);
        assert!(got.contains("if (x != null)"), "{got}");
        assert!(got.contains("return x;"), "{got}");
    }

    #[test]
    fn parse_store_fn_body_roundtrip() {
        let src = r#"
pkg T
  struct S
    fn loadContributions()
      loading = true
      let result: Json = ApiClient.fetch("/api/contributions?app=dlx-ai", {})
      contributions = result.contributions
      mut allMenu: Json = []
      for contrib in contributions
        if contrib.enabled
          for item in contrib.slots.main_menu
            allMenu = allMenu.concat(item)
      menuItems = allMenu
      loading = false
"#;
        let tokens = veil_parser::lex(src);
        let sol = veil_parser::parse(&tokens).expect("parse");
        let s = sol.items.iter().find_map(|i| match i {
            veil_ir::ast::TopLevelItem::Construct(c) if c.name == "S" => Some(c),
            _ => None,
        }).expect("struct S");
        let f = s.fns.iter().find(|f| f.name == "loadContributions").expect("fn");
        assert!(
            !f.body.iter().any(|e| matches!(e, Expr::Ident(n) if n == "let")),
            "let leaked: {:?}",
            f.body
        );
        let got = emit_typescript_stmts(&f.body, 1);
        let expected = concat!(
            "  loading = true;\n",
            "  let result: any = ApiClient.fetch(\"/api/contributions?app=dlx-ai\", {});\n",
            "  contributions = result.contributions;\n",
            "  let allMenu: any[] = [];\n",
            "  for (const contrib of contributions) {\n",
            "    if (contrib.enabled) {\n",
            "      for (const item of contrib.slots.main_menu) {\n",
            "        allMenu = allMenu.concat(item);\n",
            "      }\n",
            "    }\n",
            "  }\n",
            "  menuItems = allMenu;\n",
            "  loading = false;\n",
        );
        assert_eq!(got, expected);
    }
}
