use veil_ir::ast::*;
use crate::rust::to_snake;
use super::*;

/// Translate a VEIL expression to a Rust expression string (no trailing semicolon).
///
/// This is the production entry point. It lowers to the typed `RustExpr` IR,
/// applies ownership analysis (clone insertion for multi-use values), and
/// emits the final Rust source string.
pub fn expr_to_rust(expr: &Expr, ctx: &GenCtx) -> String {
    if ctx.option_value_wrap && !expr_handles_option_wrap(expr) {
        let mut inner_ctx = ctx.clone_for_inference();
        inner_ctx.option_value_wrap = false;
        let inner = expr_to_rust(expr, &inner_ctx);
        return wrap_as_option_value(expr, inner, ctx);
    }
    emit(&apply_ownership(lower_to_rust(expr, ctx), ctx))
}

// ─── Helpers for edge-case ident translation ─────────────────────────────────

/// Translate inline VEIL ternary expressions with nested f-strings.
/// Input: `if x.is_some() then f" in {x.unwrap()}" else ""`
/// Output: `if x.is_some() { format!(" in {}", x.unwrap()) } else { "".to_string() }`
pub fn translate_inline_ternary_fstring(raw: &str) -> String {
    // Parse: `if <cond> then <then_expr> else <else_expr>`
    let Some(then_idx) = raw.find(" then ") else {
        return raw.to_string();
    };
    let cond = &raw[3..then_idx]; // skip "if "
    let after_then = &raw[then_idx + 6..]; // skip " then "

    // Find the `else` boundary — must handle nested quotes
    let (then_expr, else_expr) = if let Some(else_idx) = find_top_level_else(after_then) {
        (&after_then[..else_idx], after_then[else_idx + 5..].trim()) // skip " else "
    } else {
        (after_then, "\"\"")
    };

    let then_rust = translate_fstring_value(then_expr.trim());
    let else_rust = translate_fstring_value(else_expr.trim());

    format!("if {} {{ {} }} else {{ {} }}", cond, then_rust, else_rust)
}

/// Find top-level " else " that's not inside quotes.
pub fn find_top_level_else(s: &str) -> Option<usize> {
    let mut in_quote = false;
    let mut quote_char = '"';
    let bytes = s.as_bytes();
    let else_pat = b" else ";
    for i in 0..s.len().saturating_sub(5) {
        let ch = bytes[i] as char;
        if !in_quote && (ch == '"' || ch == '\'') {
            in_quote = true;
            quote_char = ch;
        } else if in_quote && ch == quote_char && (i == 0 || bytes[i - 1] != b'\\') {
            in_quote = false;
        } else if !in_quote && i + 6 <= s.len() && &bytes[i..i + 6] == else_pat {
            return Some(i);
        }
    }
    None
}

/// Translate a value that may be an f-string or a plain string literal.
pub fn translate_fstring_value(val: &str) -> String {
    // f"..." or f'...' → format!(...)
    if (val.starts_with("f\"") && val.ends_with('"'))
        || (val.starts_with("f'") && val.ends_with('\''))
    {
        let inner = &val[2..val.len() - 1];
        // Convert {expr} interpolations to format! args
        let mut fmt = String::new();
        let mut args = Vec::new();
        let mut chars = inner.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut depth = 1;
                let mut expr_text = String::new();
                for c in chars.by_ref() {
                    if c == '{' {
                        depth += 1;
                    }
                    if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    expr_text.push(c);
                }
                fmt.push_str("{}");
                args.push(expr_text);
            } else {
                fmt.push(ch);
            }
        }
        if args.is_empty() {
            format!("\"{}\".to_string()", fmt)
        } else {
            format!("format!(\"{}\", {})", fmt, args.join(", "))
        }
    } else if val.starts_with('"') && val.ends_with('"') {
        // Plain string literal
        format!("{}.to_string()", val)
    } else if val.starts_with('\'') && val.ends_with('\'') {
        let inner = &val[1..val.len() - 1];
        format!("\"{}\".to_string()", inner)
    } else {
        val.to_string()
    }
}

// ─── JSON argument serialization ─────────────────────────────────────────────

/// Serialize an expression for embedding inside a `json!` payload.
///
/// Values are cloned to avoid moving locals that are reused across bus calls;
/// bare non-local identifiers (e.g. enum variants like `FreeTier`) become JSON
/// strings; field access uses JSON indexing on the serialized base so it works
/// regardless of the (opaque) source type.
pub fn to_json_arg(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            // VEIL null in JSON envelopes must be JSON null, not the string "null".
            if name == "null" {
                return "serde_json::Value::Null".to_string();
            }
            // A shared step-state value → read from the threaded state.
            if ctx.state_locals.contains(name.as_str()) {
                format!("state[\"{}\"].clone()", name)
            } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                // A struct-captured input (step impl) → self.<field>.
                format!("self.{}.clone()", to_snake(name))
            } else if ctx.is_local(name) {
                format!("{}.clone()", name)
            } else {
                // Non-local bare ident in a payload → symbolic string (enum variant, marker).
                format!("\"{}\"", name)
            }
        }
        Expr::FieldAccess(base, field) => {
            // A field of a state-local → index into the threaded state.
            if let Expr::Ident(name) = base.as_ref()
                && ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"].clone()", name, field);
                }
            // If the base is already a serde_json::Value local, index it directly.
            if let Expr::Ident(name) = base.as_ref()
                && ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return format!("{}[\"{}\"].clone()", name, field);
                }
            // Otherwise serialize the base then index (works for opaque stub types;
            // Index yields Null on mismatch rather than panicking).
            format!(
                "serde_json::json!({})[\"{}\"].clone()",
                to_json_arg(base, ctx),
                field
            )
        }
        // Empty arrays in json! context need explicit typing
        Expr::ArrayLit(items) if items.is_empty() => {
            "serde_json::Value::Array(vec![])".to_string()
        }
        Expr::ArrayLit(items) => {
            let vals: Vec<String> = items.iter().map(|e| to_json_arg(e, ctx)).collect();
            format!("vec![{}]", vals.join(", "))
        }
        _ => expr_to_rust(expr, ctx),
    }
}
