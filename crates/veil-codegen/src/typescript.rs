//! TypeScript code generation from VEIL AST.
//!
//! Fully shape-driven, parallel to `rust.rs`: constructs are generated
//! according to their core shape. No domain-specific knowledge.

use std::cell::RefCell;
use std::collections::HashMap;

use veil_ir::ast::*;
use veil_ir::layer::StatementSpec;

thread_local! {
    /// Statement specs active during a `generate_ts` pass (for `lowers_to` templates).
    static TS_STATEMENT_SPECS: RefCell<HashMap<String, StatementSpec>> = RefCell::new(HashMap::new());
    /// Backend services emit camelCase idents; Svelte/UI keeps authored snake_case.
    static TS_CAMEL_IDENTS: RefCell<bool> = const { RefCell::new(false) };
}

fn ts_camel_idents() -> bool {
    TS_CAMEL_IDENTS.with(|c| *c.borrow())
}

/// Generated TypeScript project output.
pub struct TsProject {
    pub files: Vec<TsFile>,
}

pub struct TsFile {
    pub path: String,
    pub content: String,
}

// ─── Type Mapping ────────────────────────────────────────────────────────────

/// Convert a VEIL type expression to its TypeScript equivalent.
pub fn type_to_ts(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Str" => "string".to_string(),
            "Int" | "F64" => "number".to_string(),
            "Bool" => "boolean".to_string(),
            "Bytes" => "Uint8Array".to_string(),
            "UUID" | "Id" => "string".to_string(),
            "DateTime" | "Dt" => "Date".to_string(),
            "Json" => "Record<string, unknown>".to_string(),
            other => other.to_string(),
        },
        TypeExpr::Generic(name, args) => {
            let ts_args = args.iter().map(type_to_ts).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, ts_args)
        }
        TypeExpr::Result(Some(inner)) => format!("Promise<{}>", type_to_ts(inner)),
        TypeExpr::Result(None) => "Promise<void>".to_string(),
        TypeExpr::Optional(inner) => format!("{} | null", type_to_ts(inner)),
        TypeExpr::List(inner) => format!("{}[]", type_to_ts(inner)),
        TypeExpr::Map(k, v) => format!("Map<{}, {}>", type_to_ts(k), type_to_ts(v)),
        TypeExpr::Set(inner) => format!("Set<{}>", type_to_ts(inner)),
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_to_ts).collect::<Vec<_>>().join(", ");
            format!("[{}]", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}]", (0..*size).map(|_| type_to_ts(inner)).collect::<Vec<_>>().join(", ")),
        TypeExpr::Ref(inner, _) => type_to_ts(inner), // no refs in TS
        TypeExpr::Dyn(inner) => type_to_ts(inner),    // just the interface
        TypeExpr::ImplTrait(inner) => type_to_ts(inner),
        TypeExpr::FnPtr(params, ret) => {
            let p = params.iter().enumerate()
                .map(|(i, t)| format!("arg{}: {}", i, type_to_ts(t)))
                .collect::<Vec<_>>().join(", ");
            let r = ret.as_ref().map(|t| type_to_ts(t)).unwrap_or_else(|| "void".to_string());
            format!("({}) => {}", p, r)
        }
        TypeExpr::LitStr(_) => "string".to_string(),
    }
}

/// Infer a TypeScript type for shorthand (untyped) fields by naming convention.
pub fn infer_field_type_ts(name: &str) -> String {
    if name == "id" || name.ends_with("_id") {
        return "string".to_string();
    }
    if name.ends_with("_at") || name == "created" || name == "updated"
        || name == "deleted" || name == "expires" || name == "timestamp" {
        return "Date".to_string();
    }
    if name.starts_with("is_") || name.starts_with("has_") || name.starts_with("can_")
        || name == "active" || name == "enabled" || name == "verified" || name == "deleted" {
        return "boolean".to_string();
    }
    if name == "count" || name == "total" || name == "amount" || name == "quantity"
        || name == "score" || name == "age" || name == "size" || name == "length"
        || name == "port" || name == "retries" {
        return "number".to_string();
    }
    "string".to_string()
}

/// Convert a name to camelCase (for variables/functions).
pub fn to_camel(s: &str) -> String {
    let mut result = String::new();
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

/// Format generic type parameters for TypeScript: `<T, U>` or empty string.
/// Field type as TS string, using explicit type or inferring from name.
pub fn field_type_ts(field: &Field) -> String {
    match &field.type_expr {
        TypeExpr::Named(n) if n.is_empty() => infer_field_type_ts(&field.name),
        ty => type_to_ts(ty),
    }
}

// ─── Expression Translation ──────────────────────────────────────────────────

/// Translate a layer ActionExpr to TypeScript (templates + fallback).
fn translate_action_ts(a: &ActionExpr, indent: usize) -> String {
    let core = TS_STATEMENT_SPECS.with(|cell| {
        let specs = cell.borrow();
        if let Some(spec) = specs.get(&a.keyword) {
            if let Some(template) = spec.lowers_to.get("typescript") {
                return interpolate_ts_action_template(template, a, indent, spec);
            }
            // Port.method fallback
            if let (Some(port), Some(method)) = (&spec.port_target, &spec.port_method) {
                let dep = to_camel(port);
                let args = action_args_ts(a, indent);
                return format!("await this.{}.{}({})", dep, to_camel(method), args);
            }
        }
        translate_action_ts_default(a, indent)
    });
    if let Some(binding) = &a.result_binding {
        format!("const {} = {}", to_camel(binding), core)
    } else {
        core
    }
}

fn action_args_ts(a: &ActionExpr, indent: usize) -> String {
    if !a.named_args.is_empty() {
        let fields = a
            .named_args
            .iter()
            .map(|(k, v)| {
                let val = expr_to_ts(v, indent);
                let key = to_camel(k);
                if key == val {
                    key
                } else {
                    format!("{}: {}", key, val)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        if a.target.is_empty() {
            format!("{{ {} }}", fields)
        } else {
            format!("{{ type: \"{}\", {} }}", a.target, fields)
        }
    } else if !a.args.is_empty() {
        a.args
            .iter()
            .map(|e| expr_to_ts(e, indent))
            .collect::<Vec<_>>()
            .join(", ")
    } else if !a.target.is_empty() {
        a.target.clone()
    } else {
        String::new()
    }
}

fn interpolate_ts_action_template(
    template: &str,
    a: &ActionExpr,
    indent: usize,
    spec: &StatementSpec,
) -> String {
    let mut result = template.to_string();
    let args_str = action_args_ts(a, indent);
    result = result.replace("{args}", &args_str);
    for (i, arg) in a.args.iter().enumerate() {
        result = result.replace(&format!("{{arg{i}}}"), &expr_to_ts(arg, indent));
    }
    if let Some(dep_type) = &spec.requires_dep {
        result = result.replace("{dep}", &to_camel(dep_type));
    } else if let Some(port) = &spec.port_target {
        result = result.replace("{dep}", &to_camel(port));
    }
    result = result.replace("{self}", "this");
    for (key, val) in &a.named_args {
        result = result.replace(&format!("{{named.{key}}}"), &expr_to_ts(val, indent));
    }
    if result.contains("{body}") {
        let body = a
            .body
            .iter()
            .map(|e| expr_to_ts(e, indent))
            .collect::<Vec<_>>()
            .join("; ");
        result = result.replace("{body}", &body);
    }
    result
}

fn translate_action_ts_default(a: &ActionExpr, indent: usize) -> String {
    if a.keyword == "guard" {
        let cond = a
            .condition
            .as_ref()
            .map(|c| expr_to_ts(c, indent))
            .or_else(|| a.args.first().map(|c| expr_to_ts(c, indent)))
            .unwrap_or_else(|| "true".into());
        let msg = a
            .message
            .clone()
            .or_else(|| {
                a.args.get(1).and_then(|e| match e {
                    Expr::StringLit(s) => Some(s.clone()),
                    _ => None,
                })
            })
            .unwrap_or_else(|| "precondition failed".into());
        let msg = msg.replace('\\', "\\\\").replace('"', "\\\"");
        return format!("if (!({cond})) throw new Error(\"{msg}\")");
    }
    let target = if a.target.is_empty() {
        String::new()
    } else {
        format!("{}.", to_camel(&a.target))
    };
    let method = to_camel(&a.method);
    if !a.named_args.is_empty() {
        let fields = a
            .named_args
            .iter()
            .map(|(k, v)| {
                let val = expr_to_ts(v, indent);
                let key = to_camel(k);
                if key == val {
                    key
                } else {
                    format!("{}: {}", key, val)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "await {}{}{}",
            target,
            method,
            if method.is_empty() {
                format!("({{ {} }})", fields)
            } else {
                format!("({{ {} }})", fields)
            }
        )
    } else {
        let args = a
            .args
            .iter()
            .map(|e| expr_to_ts(e, indent))
            .collect::<Vec<_>>()
            .join(", ");
        format!("await {}{}({})", target, method, args)
    }
}

/// Translate a VEIL expression to TypeScript source.
pub fn expr_to_ts(expr: &Expr, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match expr {
        // Keep VEIL idents as authored (snake_case matches state + templates + JSON APIs)
        // unless a backend-services pass requested camelCase.
        Expr::Ident(name) if name == "null" || name == "None" => "null".to_string(),
        Expr::Ident(name) if name == "Ok" => "undefined /* Ok */".to_string(),
        Expr::Ident(name) => {
            if ts_camel_idents() {
                to_camel(name)
            } else {
                name.clone()
            }
        }
        Expr::FieldAccess(base, field) => {
            let f = if ts_camel_idents() {
                to_camel(field)
            } else {
                field.clone()
            };
            format!("{}.{}", expr_to_ts(base, indent), f)
        }
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::ArrayLit(items) => {
            let elems = items.iter().map(|e| expr_to_ts(e, indent)).collect::<Vec<_>>().join(", ");
            format!("[{}]", elems)
        }
        Expr::Tuple(items) => {
            let elems = items.iter().map(|e| expr_to_ts(e, indent)).collect::<Vec<_>>().join(", ");
            format!("[{}]", elems)
        }

        Expr::BinaryOp(op) => {
            let left = expr_to_ts(&op.left, indent);
            let right = expr_to_ts(&op.right, indent);
            format!("{} {} {}", left, binop_to_ts(&op.op), right)
        }
        Expr::UnaryOp(op) => {
            let operand = expr_to_ts(&op.expr, indent);
            format!("{}{}", unaryop_to_ts(&op.op), operand)
        }

        Expr::Call(call) => translate_call_ts(call, indent),

        Expr::Assign(name, value, ty_ann) => {
            let rhs = expr_to_ts(value, indent);
            // Field write: `loan.returned = true` — not a new binding.
            if name.contains('.') {
                let path = name
                    .split('.')
                    .enumerate()
                    .map(|(i, seg)| if i == 0 { to_camel(seg) } else { to_camel(seg) })
                    .collect::<Vec<_>>()
                    .join(".");
                return format!("{} = {}", path, rhs);
            }
            match ty_ann {
                Some(ty) => format!(
                    "const {}: {} = {}",
                    to_camel(name),
                    type_to_ts(ty),
                    rhs
                ),
                None => format!("const {} = {}", to_camel(name), rhs),
            }
        }
        Expr::MutAssign(name, value, ty_ann) => {
            match ty_ann {
                Some(ty) => format!("let {}: {} = {}", name, type_to_ts(ty), expr_to_ts(value, indent)),
                None => format!("let {} = {}", name, expr_to_ts(value, indent)),
            }
        }

        Expr::Return(inner) => {
            match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => "return".to_string(),
                Expr::Ident(n) if n == "null" || n == "None" => "return null".to_string(),
                _ => format!("return {}", expr_to_ts(inner, indent)),
            }
        }

        Expr::Await(inner) => {
            format!("await {}", expr_to_ts(inner, indent))
        }

        Expr::Try(inner) => {
            // expr? → await expr (errors throw in TS)
            format!("await {}", expr_to_ts(inner, indent))
        }
        Expr::Require(inner) => {
            let v = expr_to_ts(inner, indent);
            format!("(({v}) ?? (() => {{ throw new Error(\"NotFound\"); }})())")
        }

        Expr::IfExpr(data) => {
            let cond = expr_to_ts(&data.condition, indent);
            let then_body = body_to_ts(&data.then_body, indent + 1);
            let mut out = format!("if ({}) {{\n{}\n{}}}", cond, then_body, pad);
            if let Some(else_body) = &data.else_body {
                let else_str = body_to_ts(else_body, indent + 1);
                out.push_str(&format!(" else {{\n{}\n{}}}", else_str, pad));
            }
            out
        }

        Expr::Match(scrutinee, arms) => {
            let scrut = expr_to_ts(scrutinee, indent);
            let mut out = format!("switch ({}) {{\n", scrut);
            for arm in arms {
                let arm_pad = "  ".repeat(indent + 1);
                let body_str = body_to_ts(&arm.body, indent + 2);
                if arm.pattern == "_" {
                    out.push_str(&format!("{}default: {{\n{}\n{}  break;\n{}}}\n", arm_pad, body_str, arm_pad, arm_pad));
                } else {
                    out.push_str(&format!("{}case \"{}\": {{\n{}\n{}  break;\n{}}}\n", arm_pad, arm.pattern, body_str, arm_pad, arm_pad));
                }
            }
            out.push_str(&format!("{}}}", pad));
            out
        }

        Expr::ForLoop { binding, index, iterable, body } => {
            let iter_str = expr_to_ts(iterable, indent);
            let body_str = body_to_ts(body, indent + 1);
            if let Some(idx) = index {
                format!("for (let [{}, {}] of {}.entries()) {{\n{}\n{}}}", to_camel(idx), to_camel(binding), iter_str, body_str, pad)
            } else {
                format!("for (const {} of {}) {{\n{}\n{}}}", to_camel(binding), iter_str, body_str, pad)
            }
        }

        Expr::WhileLoop { condition, body } => {
            let cond = expr_to_ts(condition, indent);
            let body_str = body_to_ts(body, indent + 1);
            format!("while ({}) {{\n{}\n{}}}", cond, body_str, pad)
        }

        Expr::Loop(body) => {
            let body_str = body_to_ts(body, indent + 1);
            format!("while (true) {{\n{}\n{}}}", body_str, pad)
        }

        Expr::DoBlock(body) => {
            // Lower to IIFE: (() => { stmts; return last; })()
            if body.is_empty() {
                "(() => {})()".to_string()
            } else {
                let inner_pad = "  ".repeat(indent + 1);
                let mut lines = Vec::new();
                for (i, e) in body.iter().enumerate() {
                    if i == body.len() - 1 {
                        lines.push(format!("{}return {};", inner_pad, expr_to_ts(e, indent + 1)));
                    } else {
                        lines.push(format!("{}{};", inner_pad, expr_to_ts(e, indent + 1)));
                    }
                }
                format!("(() => {{\n{}\n{}}})()", lines.join("\n"), pad)
            }
        }

        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),

        Expr::Closure { params, body } => {
            let ps = params.iter().map(|p| to_camel(p)).collect::<Vec<_>>().join(", ");
            if body.len() == 1 {
                format!("({}) => {}", ps, expr_to_ts(&body[0], indent))
            } else {
                let body_str = body_to_ts(body, indent + 1);
                format!("({}) => {{\n{}\n{}}}", ps, body_str, pad)
            }
        }

        Expr::StructLit(_name, fields) => {
            let fs = fields
                .iter()
                .map(|(k, v)| {
                    let val = expr_to_ts(v, indent);
                    let key = if ts_camel_idents() { to_camel(k) } else { k.clone() };
                    if key == val {
                        key
                    } else {
                        format!("{}: {}", key, val)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {} }}", fs)
        }

        Expr::StructUpdate { name: _, fields, base } => {
            let base_str = expr_to_ts(base, indent);
            let fs = fields
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_ts(v, indent)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ ...{}, {} }}", base_str, fs)
        }

        Expr::Index(base, idx) => {
            format!("{}[{}]", expr_to_ts(base, indent), expr_to_ts(idx, indent))
        }

        Expr::StringInterp(parts) => {
            let mut out = String::from("`");
            for part in parts {
                match part {
                    StringPart::Literal(s) => out.push_str(s),
                    StringPart::Expr(e) => {
                        out.push_str("${");
                        out.push_str(&expr_to_ts(e, indent));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }

        Expr::Cast(inner, ty) => {
            format!("{} as {}", expr_to_ts(inner, indent), ty)
        }

        Expr::Range { start, end, inclusive: _ } => {
            // No native range in TS — emit a comment placeholder
            let s = start.as_ref().map(|e| expr_to_ts(e, indent)).unwrap_or_else(|| "0".to_string());
            let e = end.as_ref().map(|e| expr_to_ts(e, indent)).unwrap_or_else(|| "Infinity".to_string());
            format!("/* range */[{}, {}]", s, e)
        }

        Expr::Action(a) => translate_action_ts(a, indent),

        Expr::IfLet { pattern, expr: scrutinee, then_body, else_body } => {
            let val = expr_to_ts(scrutinee, indent);
            let then_str = body_to_ts(then_body, indent + 1);
            let mut out = format!("const {} = {};\nif ({} != null) {{\n{}\n{}}}", pattern, val, pattern, then_str, pad);
            if let Some(else_b) = else_body {
                let else_str = body_to_ts(else_b, indent + 1);
                out.push_str(&format!(" else {{\n{}\n{}}}", else_str, pad));
            }
            out
        }

        Expr::WhileLet { pattern, expr: scrutinee, body } => {
            let val = expr_to_ts(scrutinee, indent);
            let body_str = body_to_ts(body, indent + 1);
            format!("while (({} = {}) != null) {{\n{}\n{}}}", pattern, val, body_str, pad)
        }

        Expr::LetPattern(pattern, expr, ty_ann) => {
            let pat_str = pattern_to_ts(pattern);
            let val = expr_to_ts(expr, indent);
            match ty_ann {
                Some(ty) => format!("const {}: {} = {}", pat_str, type_to_ts(ty), val),
                None => format!("const {} = {}", pat_str, val),
            }
        }
        // Expanded by adapt merge before codegen — should never remain.
        Expr::Stock => "/* error: stock not expanded */ undefined".to_string(),
    }
}

/// Translate a function/method call to TypeScript.
fn translate_call_ts(call: &CallExpr, indent: usize) -> String {
    let args_list: Vec<String> = call
        .args
        .iter()
        .map(|a| expr_to_ts(a, indent))
        .collect();
    let args = args_list.join(", ");

    // Skip .clone() calls — no ownership in TS
    if call.method == "clone" && call.args.is_empty() {
        return if call.target.is_empty() {
            "this".to_string()
        } else {
            to_camel(&call.target)
        };
    }

    // Receiver-based chaining: receiver.method(args)
    if let Some(recv) = &call.receiver {
        let recv_str = expr_to_ts(recv, indent);
        return format!("{}.{}({})", recv_str, to_camel(&call.method), args);
    }

    if call.target.is_empty() && !call.method.is_empty() {
        // bare method
        return format!("{}({})", to_camel(&call.method), args);
    }

    if call.method.is_empty() {
        // bare function call: target(args)
        match call.target.as_str() {
            "now" => return "new Date()".to_string(),
            "goto" | "navigate" => {
                let url = args_list.first().map(|s| s.as_str()).unwrap_or("\"/\"");
                return format!("(window.location.href = {url})");
            }
            _ => return format!("{}({})", call.target, args),
        }
    }

    // ── Browser / Svelte layer builtins (declared on svelte5, lowered here) ──
    // ApiClient: HTTPS REST — not the Bus.
    if call.target == "ApiClient" {
        match call.method.as_str() {
            "fetch" if !args_list.is_empty() => {
                let url = &args_list[0];
                let params = args_list.get(1).map(|s| s.as_str()).unwrap_or("{}");
                return format!(
                    "(async () => {{ const __u = new URL({url}, typeof window !== 'undefined' ? window.location.origin : 'http://localhost'); const __p = {params} as Record<string, unknown>; for (const [k, v] of Object.entries(__p)) {{ if (v != null && v !== '') __u.searchParams.set(k, String(v)); }} const __r = await fetch(__u.toString()); if (!__r.ok) throw new Error(await __r.text()); return await __r.json(); }})()"
                );
            }
            "mutate" if !args_list.is_empty() => {
                let url = &args_list[0];
                let body = args_list.get(1).map(|s| s.as_str()).unwrap_or("{}");
                return format!(
                    "(async () => {{ const __r = await fetch({url}, {{ method: 'POST', headers: {{ 'Content-Type': 'application/json' }}, body: JSON.stringify({body}) }}); if (!__r.ok) throw new Error(await __r.text()); const __t = await __r.text(); return __t ? JSON.parse(__t) : null; }})()"
                );
            }
            "put" if !args_list.is_empty() => {
                let url = &args_list[0];
                let body = args_list.get(1).map(|s| s.as_str()).unwrap_or("{}");
                return format!(
                    "(async () => {{ const __r = await fetch({url}, {{ method: 'PUT', headers: {{ 'Content-Type': 'application/json' }}, body: JSON.stringify({body}) }}); if (!__r.ok) throw new Error(await __r.text()); const __t = await __r.text(); return __t ? JSON.parse(__t) : null; }})()"
                );
            }
            "delete" if !args_list.is_empty() => {
                let url = &args_list[0];
                return format!(
                    "(async () => {{ const __r = await fetch({url}, {{ method: 'DELETE' }}); if (!__r.ok) throw new Error(await __r.text()); const __t = await __r.text(); return __t ? JSON.parse(__t) : null; }})()"
                );
            }
            _ => {}
        }
    }
    // LocalStorage helpers (layer-declared free fns or type methods)
    if call.target == "LocalStorage" || call.target == "local_storage" {
        match call.method.as_str() {
            "get" | "get_opt" if args_list.len() == 1 => {
                return format!("localStorage.getItem({})", args_list[0]);
            }
            "get_or" if args_list.len() == 2 => {
                return format!(
                    "(localStorage.getItem({}) ?? {})",
                    args_list[0], args_list[1]
                );
            }
            "set" if args_list.len() == 2 => {
                return format!(
                    "localStorage.setItem({}, {})",
                    args_list[0], args_list[1]
                );
            }
            _ => {}
        }
    }
    if call.target == "Env" {
        match call.method.as_str() {
            "get_or" if args_list.len() == 2 => {
                // Browser: prefer localStorage then default (no process.env in SPA)
                return format!(
                    "(localStorage.getItem({}) ?? {})",
                    args_list[0], args_list[1]
                );
            }
            _ => {}
        }
    }
    // navigate / goto from svelte5 layer
    if ((call.target == "navigate" || call.method == "navigate" || call.target == "goto")
        && !call.method.is_empty()
        || call.target == "goto")
        && (call.method == "goto" || call.target == "goto" || call.method == "navigate") {
            let url = args_list.first().map(|s| s.as_str()).unwrap_or("\"/\"");
            return format!("(window.location.href = {url})");
        }
    if call.target == "navigate" && call.method.is_empty() {
        let url = args_list.first().map(|s| s.as_str()).unwrap_or("\"/\"");
        return format!("(window.location.href = {url})");
    }

    // target.method(args)
    let target = to_camel(&call.target);
    let bare_method = call.method.trim_end_matches('!');
    let method = to_camel(bare_method);

    if call.target == "guard" && call.method.is_empty() {
        let cond = args_list.first().cloned().unwrap_or_else(|| "true".into());
        let msg = args_list.get(1).cloned().unwrap_or_else(|| "\"precondition failed\"".into());
        return format!("if (!({cond})) throw new Error({msg})");
    }

    // new() → constructor. Id/UUID are opaque strings in TS.
    if bare_method == "new" {
        if matches!(call.target.as_str(), "Id" | "UUID" | "Uuid") {
            return "crypto.randomUUID()".to_string();
        }
        return format!("new {}({})", call.target, args);
    }

    if bare_method == "is_none" {
        return format!("({} == null)", target);
    }
    if bare_method == "is_some" {
        return format!("({} != null)", target);
    }

    // Port / repo methods are async; bang is ACS-010 (not a TS identifier).
    let awaited = call.method.ends_with('!');
    let call_s = if method.is_empty() {
        format!("{}({})", target, args)
    } else {
        format!("{}.{}({})", target, method, args)
    };
    if awaited {
        format!("await {}", call_s)
    } else {
        call_s
    }
}

/// Translate a block of statements.
fn body_to_ts(exprs: &[Expr], indent: usize) -> String {
    let pad = "  ".repeat(indent);
    exprs.iter()
        .map(|e| {
            let line = expr_to_ts(e, indent);
            format!("{}{};", pad, line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Like `expr_to_ts` but awaits ApiClient / async IIFE results on assignment.
/// Recurses into `if` / blocks so nested `membership_options = ApiClient.fetch(...)` awaits.
pub fn expr_to_ts_async(expr: &Expr, indent: usize) -> String {
    match expr {
        // Keep `let` for mut bindings: `mut repo = ApiClient.fetch(...)`
        // must not become bare `repo = await ...` (ReferenceError).
        Expr::MutAssign(name, rhs, ty_ann) => {
            let r = expr_to_ts(rhs, indent);
            let rhs_str = if r.contains("await fetch") || r.contains("(async () =>") {
                format!("await {}", r)
            } else {
                r
            };
            match ty_ann {
                Some(ty) => format!("let {}: {} = {}", name, type_to_ts(ty), rhs_str),
                None => format!("let {} = {}", name, rhs_str),
            }
        }
        Expr::Assign(name, rhs, _) => {
            let r = expr_to_ts(rhs, indent);
            if r.contains("await fetch") || r.contains("(async () =>") {
                format!("{} = await {}", name, r)
            } else {
                expr_to_ts(expr, indent)
            }
        }
        Expr::Call(c) if c.target == "ApiClient" => {
            format!("await {}", expr_to_ts(expr, indent))
        }
        Expr::IfExpr(data) => {
            let pad = "  ".repeat(indent);
            let cond = expr_to_ts(&data.condition, indent);
            let then_body = data
                .then_body
                .iter()
                .map(|e| format!("{}  {};", pad, expr_to_ts_async(e, indent + 1)))
                .collect::<Vec<_>>()
                .join("\n");
            let mut out = format!("if ({}) {{\n{}\n{}}}", cond, then_body, pad);
            if let Some(else_body) = &data.else_body {
                let else_str = else_body
                    .iter()
                    .map(|e| format!("{}  {};", pad, expr_to_ts_async(e, indent + 1)))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push_str(&format!(" else {{\n{}\n{}}}", else_str, pad));
            }
            out
        }
        _ => expr_to_ts(expr, indent),
    }
}

/// Convert a structured Pattern to TypeScript destructuring syntax.
fn pattern_to_ts(pat: &Pattern) -> String {
    match pat {
        Pattern::Ident(s) => to_camel(s),
        Pattern::Tuple(parts) => {
            let inner = parts.iter().map(pattern_to_ts).collect::<Vec<_>>().join(", ");
            format!("[{}]", inner)  // TS uses array destructuring for tuples
        }
        Pattern::Struct(_, fields, has_rest) => {
            let mut fs: Vec<String> = fields.iter().map(|(k, v)| {
                match v {
                    Some(pat) => format!("{}: {}", to_camel(k), pattern_to_ts(pat)),
                    None => to_camel(k),
                }
            }).collect();
            if *has_rest { fs.push("...rest".to_string()); }
            format!("{{ {} }}", fs.join(", "))
        }
        Pattern::Variant(name, args) => {
            // TS doesn't have native variant destructuring — emit as comment + binding
            if args.is_empty() { format!("/* {} */", name) }
            else {
                let inner = args.iter().map(pattern_to_ts).collect::<Vec<_>>().join(", ");
                format!("[{}] /* {} */", inner, name)
            }
        }
        Pattern::Literal(s) => s.clone(),
        Pattern::Or(alts) => alts.iter().map(pattern_to_ts).collect::<Vec<_>>().join(" /* | */ "),
        Pattern::Wildcard => "_".to_string(),
        Pattern::Rest => "...rest".to_string(),
    }
}

fn binop_to_ts(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "===",
        BinOp::NotEq => "!==",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn unaryop_to_ts(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}

// ─── Project Scaffolding (used by API client generation) ─────────────────────

fn gen_package_json(sol_name: &str) -> TsFile {
    let content = format!(
        r#"{{
  "name": "{}",
  "version": "0.1.0",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {{
    "build": "tsc",
    "dev": "tsc --watch"
  }},
  "devDependencies": {{
    "typescript": "^5.4.0"
  }}
}}
"#,
        sol_name
    );
    TsFile { path: "package.json".to_string(), content }
}

fn gen_tsconfig() -> TsFile {
    let content = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src",
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true
  },
  "include": ["src"]
}
"#.to_string();
    TsFile { path: "tsconfig.json".to_string(), content }
}

// ─── API Client Generation (from expose blocks) ──────────────────────────────

/// Generate a typed API client module from an expose block.
/// Produces typed interfaces for inputs/outputs and async functions that
/// call the API with correct types.
pub fn generate_api_client(pkg_name: &str, expose: &ExposeBlock) -> Vec<TsFile> {
    let mut files = Vec::new();
    let module_name = to_camel(pkg_name);

    let mut client = String::new();
    client.push_str("// Generated API client — typed bindings for the backend expose contract\n");
    client.push_str("// Do not edit — regenerated from the backend .veil package\n\n");

    // Generate input/output interfaces for each node
    for node in &expose.nodes {
        if !node.inputs.is_empty() {
            client.push_str(&format!("export interface {}Input {{\n", node.name));
            for field in &node.inputs {
                client.push_str(&format!("  {}: {};\n", to_camel(&field.name), type_to_ts(&field.type_expr)));
            }
            client.push_str("}\n\n");
        }

        if !node.outputs.is_empty() {
            client.push_str(&format!("export interface {}Output {{\n", node.name));
            for field in &node.outputs {
                client.push_str(&format!("  {}: {};\n", to_camel(&field.name), type_to_ts(&field.type_expr)));
            }
            client.push_str("}\n\n");
        }
    }

    // Generate the client class with typed methods
    client.push_str(&format!("export class {}Client {{\n", module_name));
    client.push_str("  private baseUrl: string;\n");
    client.push_str("  private headers: Record<string, string>;\n\n");
    client.push_str("  constructor(baseUrl: string, headers: Record<string, string> = {}) {\n");
    client.push_str("    this.baseUrl = baseUrl;\n");
    client.push_str("    this.headers = { 'Content-Type': 'application/json', ...headers };\n");
    client.push_str("  }\n\n");

    for node in &expose.nodes {
        let fn_name = to_camel(&node.name);
        let has_input = !node.inputs.is_empty();
        let has_output = !node.outputs.is_empty();

        let input_param = if has_input {
            format!("input: {}Input", node.name)
        } else {
            String::new()
        };
        let return_type = if has_output {
            format!("Promise<{}Output>", node.name)
        } else {
            "Promise<void>".to_string()
        };

        // Add description as JSDoc if available
        if let Some(desc) = &node.description {
            client.push_str(&format!("  /** {} */\n", desc));
        }

        client.push_str(&format!("  async {}({}): {} {{\n", fn_name, input_param, return_type));

        // Generate the endpoint path from the node name (kebab-case)
        let endpoint = node.name.chars().enumerate().map(|(i, c)| {
            if c.is_uppercase() && i > 0 { format!("-{}", c.to_lowercase()) }
            else { c.to_lowercase().to_string() }
        }).collect::<String>();

        if has_input {
            client.push_str(&format!(
                "    const res = await fetch(`${{this.baseUrl}}/{}`, {{\n      method: 'POST',\n      headers: this.headers,\n      body: JSON.stringify(input),\n    }});\n",
                endpoint
            ));
        } else {
            client.push_str(&format!(
                "    const res = await fetch(`${{this.baseUrl}}/{}`, {{\n      headers: this.headers,\n    }});\n",
                endpoint
            ));
        }

        client.push_str("    if (!res.ok) throw new Error(`API error: ${res.status}`);\n");
        if has_output {
            client.push_str("    return res.json();\n");
        }
        client.push_str("  }\n\n");
    }

    client.push_str("}\n");

    files.push(TsFile {
        path: format!("src/api/{}.ts", to_camel(pkg_name)),
        content: client,
    });

    files
}

/// Generate a typed API client from a Package's expose block.
/// Called when `veil gen package.veil -t ts` targets a pkg file.
pub fn generate_api_client_from_package(pkg: &Package) -> TsProject {
    let mut files = Vec::new();

    if let Some(expose) = &pkg.expose {
        files.extend(generate_api_client(&pkg.name, expose));
    }

    // Also generate shared types (DTOs from the expose block are in items)
    // Export the package as a typed module
    let mut index = String::from("// API client for ");
    index.push_str(&pkg.name);
    index.push_str(" — generated by VEIL\n\n");
    if pkg.expose.is_some() {
        index.push_str(&format!("export * from './api/{}';\n", to_camel(&pkg.name)));
    }
    files.push(TsFile { path: "src/index.ts".to_string(), content: index });

    files.push(gen_package_json(&to_camel(&pkg.name)));
    files.push(gen_tsconfig());

    TsProject { files }
}
