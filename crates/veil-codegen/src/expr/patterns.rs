use std::collections::HashMap;
use veil_ir::ast::*;
use crate::rust::to_snake;
use super::*;

/// Binding names introduced by a match arm pattern string (e.g. `Some(item)` → `item`).
pub fn pattern_binding_names(pattern: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cur = String::new();
    let mut in_ident = false;
    for ch in pattern.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
            in_ident = true;
        } else if in_ident {
            // Skip keywords / constructors (Some, None, Ok, Err, true, false)
            let skip = matches!(
                cur.as_str(),
                "Some" | "None" | "Ok" | "Err" | "true" | "false" | "_"
            ) || cur
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false);
            if !skip && !cur.is_empty() {
                names.push(cur.clone());
            }
            cur.clear();
            in_ident = false;
        }
    }
    if in_ident {
        let skip = matches!(
            cur.as_str(),
            "Some" | "None" | "Ok" | "Err" | "true" | "false" | "_"
        ) || cur
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if !skip && !cur.is_empty() {
            names.push(cur);
        }
    }
    names
}

/// Convert a structured Pattern to Rust pattern syntax.
pub fn pattern_to_rust(pat: &Pattern) -> String {
    pattern_to_rust_qualified(pat, None)
}

pub fn qualify_variant_name(name: &str, enums: Option<&HashMap<String, String>>) -> String {
    if name.contains("::") {
        return name.to_string();
    }
    if name.contains('.') && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        return name.replace('.', "::");
    }
    if let Some(en) = enums.and_then(|m| m.get(name)) {
        return format!("{en}::{name}");
    }
    name.to_string()
}

pub fn pattern_to_rust_qualified(
    pat: &Pattern,
    enums: Option<&HashMap<String, String>>,
) -> String {
    match pat {
        Pattern::Ident(s) => {
            if s.contains('.') && s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                s.replace('.', "::")
            } else if let Some(en) = enums.and_then(|m| m.get(s)) {
                format!("{en}::{s}")
            } else if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                s.clone()
            } else {
                to_snake(s)
            }
        }
        Pattern::Tuple(parts) => {
            let inner = parts
                .iter()
                .map(|p| pattern_to_rust_qualified(p, enums))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({})", inner)
        }
        Pattern::Struct(name, fields, has_rest) => {
            let rust_name = qualify_variant_name(name, enums);
            let mut fs: Vec<String> = fields
                .iter()
                .map(|(k, v)| match v {
                    Some(pat) => format!(
                        "{}: {}",
                        to_snake(k),
                        pattern_to_rust_qualified(pat, enums)
                    ),
                    None => to_snake(k),
                })
                .collect();
            if *has_rest {
                fs.push("..".to_string());
            }
            format!("{} {{ {} }}", rust_name, fs.join(", "))
        }
        Pattern::Variant(name, args) => {
            let rust_name = qualify_variant_name(name, enums);
            if args.is_empty() {
                rust_name
            } else {
                let inner = args
                    .iter()
                    .map(|p| pattern_to_rust_qualified(p, enums))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", rust_name, inner)
            }
        }
        Pattern::Literal(s) => s.clone(),
        Pattern::Or(alts) => alts
            .iter()
            .map(|p| pattern_to_rust_qualified(p, enums))
            .collect::<Vec<_>>()
            .join(" | "),
        Pattern::Wildcard => "_".to_string(),
        Pattern::Rest => "..".to_string(),
    }
}

impl GenCtx {
    /// A shallow clone carrying just the maps needed for type inference (used
    /// by the return-type pre-scan in rust.rs).
    pub fn clone_for_inference(&self) -> GenCtx {
        GenCtx {
            name_to_shape: self.name_to_shape.clone(),
            locals: self.locals.clone(),
            self_fields: self.self_fields.clone(),
            in_method: self.in_method,
            types: self.types.clone(),
            ownership: self.ownership.clone(),
            stubs: self.stubs.clone(),
            routing: self.routing.clone(),
            async_fns: self.async_fns.clone(),
            state_locals: self.state_locals.clone(),
            expected_return_rust: self.expected_return_rust.clone(),
            option_value_wrap: self.option_value_wrap,
            defaultable_types: self.defaultable_types.clone(),
            dep_fields: self.dep_fields.clone(),
            local_domain_types: self.local_domain_types.clone(),
            self_field_types: self.self_field_types.clone(),
            statement_specs: self.statement_specs.clone(),
            enum_variants: self.enum_variants.clone(),
            unit_enums: self.unit_enums.clone(),
            known_modules: self.known_modules.clone(),
            error_model: self.error_model.clone(),
        }
    }
}

/// Infer turbofish type for `serde_json::from_str` from the enclosing return type.
pub fn from_str_turbofish_type(ctx: &GenCtx) -> Option<String> {
    let ret = ctx.expected_return_rust.as_deref()?;
    // Result<Option<T>, _> / Result<Vec<T>, _> / Result<T, _> / Option<T>
    let mut s = ret.trim();
    if let Some(inner) = s.strip_prefix("Result<").and_then(|x| {
        // split last , DomainError>
        let depth = 0i32;
        let _ = depth;
        x.rsplit_once(", ").map(|(a, _)| a.trim())
    }) {
        s = inner;
    }
    if let Some(inner) = s.strip_prefix("Option<").and_then(|x| x.strip_suffix('>')) {
        s = inner.trim();
    }
    if let Some(inner) = s.strip_prefix("Vec<").and_then(|x| x.strip_suffix('>')) {
        s = inner.trim();
    }
    // Domain types are PascalCase; skip Value / primitives.
    if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        && s != ctx.error_model.type_name
        && !s.starts_with("Result")
    {
        return Some(s.to_string());
    }
    // OAuth token JSON and similar → Value
    Some("serde_json::Value".into())
}

/// Emit a block of statements, tracking locals so later lines see earlier binds
/// (needed for if/while/for bodies: `mut req = …` then `req = req.header(…)`).
pub fn emit_tracked_block(body: &[Expr], ctx: &GenCtx, indent: &str) -> String {
    emit_block_lines(body, ctx, indent, false)
}

/// Block used as a value: last expression has no semicolon (and may
/// `Some`-wrap when `ctx.option_value_wrap`).
pub fn emit_value_block(body: &[Expr], ctx: &GenCtx, indent: &str) -> String {
    emit_block_lines(body, ctx, indent, true)
}

pub fn emit_block_lines(body: &[Expr], ctx: &GenCtx, indent: &str, last_is_value: bool) -> String {
    let mut body_ctx = ctx.clone_for_inference();
    body_ctx.option_value_wrap = false;
    body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
    let mut lines = Vec::new();
    for (i, e) in body.iter().enumerate() {
        let is_last = i + 1 == body.len();
        if is_last && last_is_value {
            body_ctx.option_value_wrap = ctx.option_value_wrap;
        }
        let rust = if is_last && last_is_value {
            expr_to_rust_value(e, &body_ctx)
        } else {
            expr_to_rust(e, &body_ctx)
        };
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e
            && !name.contains('.') {
                body_ctx.locals.insert(name.clone());
                if let Some(t) = infer_expr_type(rhs, &body_ctx) {
                    body_ctx.types.local_types.insert(name.clone(), t);
                }
            }
        let semi = !(is_last && last_is_value) && !is_rust_block_stmt(e);
        let piece = if semi {
            format!("{};", rust.trim_end_matches(';'))
        } else {
            rust
        };
        lines.push(indent_lines(&piece, indent));
    }
    lines.join("\n")
}

pub fn is_rust_block_stmt(e: &Expr) -> bool {
    matches!(
        e,
        Expr::IfExpr(_)
            | Expr::ForLoop { .. }
            | Expr::WhileLoop { .. }
            | Expr::Loop(_)
            | Expr::Match(_, _)
            | Expr::IfLet { .. }
            | Expr::WhileLet { .. }
    )
}

pub fn indent_lines(s: &str, indent: &str) -> String {
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
