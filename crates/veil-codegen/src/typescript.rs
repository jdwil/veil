//! TypeScript code generation from VEIL AST.
//!
//! Fully shape-driven, parallel to `rust.rs`: constructs are generated
//! according to their core shape. No domain-specific knowledge.

use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};

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
fn generic_params_ts(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Field type as TS string, using explicit type or inferring from name.
fn field_type_ts(field: &Field) -> String {
    match &field.type_expr {
        TypeExpr::Named(n) if n.is_empty() => infer_field_type_ts(&field.name),
        ty => type_to_ts(ty),
    }
}

// ─── Expression Translation ──────────────────────────────────────────────────

/// Translate a VEIL expression to TypeScript source.
pub fn expr_to_ts(expr: &Expr, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    match expr {
        Expr::Ident(name) => to_camel(name),
        Expr::FieldAccess(base, field) => format!("{}.{}", expr_to_ts(base, indent), to_camel(field)),
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
            match ty_ann {
                Some(ty) => format!(
                    "const {}: {} = {}",
                    to_camel(name),
                    type_to_ts(ty),
                    expr_to_ts(value, indent)
                ),
                None => format!("{} = {}", to_camel(name), expr_to_ts(value, indent)),
            }
        }
        Expr::MutAssign(name, value, ty_ann) => {
            match ty_ann {
                Some(ty) => format!("let {}: {} = {}", to_camel(name), type_to_ts(ty), expr_to_ts(value, indent)),
                None => format!("let {} = {}", to_camel(name), expr_to_ts(value, indent)),
            }
        }

        Expr::Return(inner) => {
            let val = expr_to_ts(inner, indent);
            format!("return {}", val)
        }

        Expr::Await(inner) => {
            format!("await {}", expr_to_ts(inner, indent))
        }

        Expr::Try(inner) => {
            // expr? → await expr (errors throw in TS)
            format!("await {}", expr_to_ts(inner, indent))
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
            let fs = fields.iter()
                .map(|(k, v)| {
                    let val = expr_to_ts(v, indent);
                    let key = to_camel(k);
                    if key == val { key } else { format!("{}: {}", key, val) }
                })
                .collect::<Vec<_>>().join(", ");
            format!("{{ {} }}", fs)  // anonymous object, type is `name`
        }

        Expr::StructUpdate { name: _, fields, base } => {
            let base_str = expr_to_ts(base, indent);
            let fs = fields.iter()
                .map(|(k, v)| format!("{}: {}", to_camel(k), expr_to_ts(v, indent)))
                .collect::<Vec<_>>().join(", ");
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

        Expr::Action(a) => {
            // Layer statement — translate like a call
            let target = if a.target.is_empty() { String::new() } else { format!("{}.", to_camel(&a.target)) };
            let method = to_camel(&a.method);
            if !a.named_args.is_empty() {
                let fields = a.named_args.iter()
                    .map(|(k, v)| {
                        let val = expr_to_ts(v, indent);
                        let key = to_camel(k);
                        if key == val { key } else { format!("{}: {}", key, val) }
                    })
                    .collect::<Vec<_>>().join(", ");
                format!("await {}{}{}", target, method, if method.is_empty() { format!("({{ {} }})", fields) } else { format!("({{ {} }})", fields) })
            } else {
                let args = a.args.iter().map(|e| expr_to_ts(e, indent)).collect::<Vec<_>>().join(", ");
                format!("await {}{}({})", target, method, args)
            }
        }

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
    }
}

/// Translate a function/method call to TypeScript.
fn translate_call_ts(call: &CallExpr, indent: usize) -> String {
    let args = call.args.iter()
        .map(|a| expr_to_ts(a, indent))
        .collect::<Vec<_>>().join(", ");

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
            _ => return format!("{}({})", to_camel(&call.target), args),
        }
    }

    // target.method(args)
    let target = to_camel(&call.target);
    let method = to_camel(&call.method);

    // new() → constructor
    if call.method == "new" {
        return format!("new {}({})", call.target, args);
    }

    format!("{}.{}({})", target, method, args)
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

// ─── Project Generation ──────────────────────────────────────────────────────

/// Generate a TypeScript project from a VEIL Solution AST.
/// `used_packages` optionally provides expose blocks from packages the solution
/// imports, enabling typed API client generation alongside the frontend code.
pub fn generate_ts(solution: &Solution, registry: &LayerRegistry) -> TsProject {
    generate_ts_with_packages(solution, registry, &[])
}

pub fn generate_ts_with_packages(
    solution: &Solution,
    registry: &LayerRegistry,
    used_packages: &[(String, ExposeBlock)],
) -> TsProject {
    let mut files = Vec::new();
    let sol_name = to_camel(&solution.name);

    // Collect all constructs by shape across all modules
    let modules: Vec<&Construct> = solution.items.iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(c),
            _ => None,
        })
        .collect();

    // Generate types file (all structs/enums across all modules)
    files.push(gen_types(&modules));

    // Generate interfaces file (all traits)
    files.push(gen_interfaces(&modules, solution));

    // Generate service functions (fn-shaped constructs)
    files.push(gen_services(&modules, solution));

    // INV-005: UI file emit keyed on layer identity (is_a), not hardcoded subkind strings
    files.extend(gen_svelte_components(&modules, registry));

    // Generate typed API clients for any used packages with expose blocks
    for (pkg_name, expose) in used_packages {
        files.extend(generate_api_client(pkg_name, expose));
    }

    // Generate index.ts (re-exports)
    files.push(gen_index(&sol_name));

    // Generate package.json
    files.push(gen_package_json(&sol_name));

    // Generate tsconfig.json
    files.push(gen_tsconfig());

    // CAP-005: browser-ready SPA when package has UI constructs (app/page/comp).
    if package_has_ui_constructs(solution) {
        files.extend(gen_spa_bundle(solution, &sol_name));
    }

    TsProject { files }
}

/// CAP-005: detect UI packages (svelte5 / ui layers).
fn package_has_ui_constructs(solution: &Solution) -> bool {
    fn walk(c: &Construct) -> bool {
        let sk = c.subkind.to_lowercase();
        if sk == "app" || sk == "page" || sk == "comp" || sk == "layout" || sk == "store" {
            return true;
        }
        c.children.iter().any(walk)
    }
    solution.items.iter().any(|i| match i {
        TopLevelItem::Construct(c) => walk(c),
        _ => false,
    })
}

/// CAP-005: emit index.html + browser app that talks to same-origin `/api`.
fn gen_spa_bundle(solution: &Solution, sol_name: &str) -> Vec<TsFile> {
    let mut files = Vec::new();

    // Collect @route paths if present on pages
    let mut routes: Vec<(String, String)> = Vec::new();
    fn strip_ann_quotes(s: &str) -> String {
        let t = s.trim();
        if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
            || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
        {
            t[1..t.len() - 1].to_string()
        } else {
            t.to_string()
        }
    }
    fn js_escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    fn collect_routes(c: &Construct, routes: &mut Vec<(String, String)>) {
        if c.subkind.eq_ignore_ascii_case("page") {
            let route = c
                .annotations
                .iter()
                .find(|a| a.name == "route")
                .and_then(|a| a.args.first())
                .map(|s| strip_ann_quotes(s))
                .unwrap_or_else(|| format!("/{}", to_camel(&c.name)));
            routes.push((c.name.clone(), route));
        }
        for ch in &c.children {
            collect_routes(ch, routes);
        }
    }
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            collect_routes(c, &mut routes);
        }
    }

    let nav_items: String = if routes.is_empty() {
        r#"{ href: "/", label: "Dashboard" },
      { href: "/projects", label: "Projects" },
      { href: "/config", label: "Config" }"#
            .into()
    } else {
        routes
            .iter()
            .map(|(name, path)| {
                format!(
                    "{{ href: \"{}\", label: \"{}\" }}",
                    js_escape(path),
                    js_escape(name)
                )
            })
            .collect::<Vec<_>>()
            .join(",\n      ")
    };

    let app_js = format!(
        r#"// Generated SPA entry for {sol_name} (CAP-005) — same-origin /api
const NAV = [
      {nav_items}
];

async function api(path, opts) {{
  const r = await fetch(path, {{
    headers: {{ "Content-Type": "application/json", ...(opts?.headers || {{}}) }},
    ...opts,
  }});
  if (!r.ok) throw new Error(await r.text());
  return r.json();
}}

function el(tag, attrs = {{}}, ...kids) {{
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {{
    if (k === "className") n.className = v;
    else if (k.startsWith("on") && typeof v === "function") n.addEventListener(k.slice(2).toLowerCase(), v);
    else if (v != null) n.setAttribute(k, v);
  }}
  for (const c of kids.flat()) {{
    if (c == null) continue;
    n.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
  }}
  return n;
}}

function shell(main) {{
  const root = document.getElementById("app");
  root.replaceChildren(
    el("aside", {{ className: "sidebar" }},
      el("div", {{ className: "logo" }}, el("span", {{}}, "◆"), " veil-runtime"),
      el("nav", {{}}, ...NAV.map(i => el("a", {{ href: i.href, className: location.pathname === i.href ? "active" : "" }}, i.label)))
    ),
    el("main", {{}}, main),
  );
}}

async function viewDashboard() {{
  let projects = [];
  try {{
    const data = await api("/api/projects");
    projects = data.projects || data.repos || [];
  }} catch (e) {{
    shell(el("div", {{}}, el("h1", {{}}, "Dashboard"), el("p", {{ className: "err" }}, String(e))));
    return;
  }}
  shell(el("div", {{}},
    el("h1", {{}}, "Dashboard"),
    el("p", {{ className: "sub" }}, "Generated shell · live multi-project API"),
    el("div", {{ className: "stats" }},
      el("div", {{ className: "stat" }}, el("div", {{ className: "v" }}, String(projects.length)), el("div", {{ className: "l" }}, "Projects")),
    ),
    el("h2", {{}}, "Projects"),
    ...projects.map(p => {{
      const name = p.name || p.id || "?";
      return el("a", {{ className: "card", href: `/projects/${{encodeURIComponent(name)}}/ide` }},
        el("div", {{ className: "name" }}, name),
        el("div", {{ className: "meta" }}, p.path || p.default_branch || "open IDE"),
      );
    }}),
  ));
}}

async function viewProjects() {{
  await viewDashboard();
}}

async function viewPage(title, load) {{
  try {{
    const body = await load();
    shell(el("div", {{}}, el("h1", {{}}, title), el("p", {{ className: "sub" }}, "Live API"), body));
  }} catch (e) {{
    shell(el("div", {{}}, el("h1", {{}}, title), el("p", {{ className: "err" }}, String(e))));
  }}
}}

async function viewDeploy() {{
  await viewPage("Deploy", async () => {{
    const data = await api("/api/artifacts");
    const arts = data.artifacts || [];
    if (!arts.length) return el("p", {{ className: "sub" }}, "No local artifacts yet. Compile a project first.");
    return el("div", {{}}, ...arts.map(a =>
      el("div", {{ className: "card" }},
        el("div", {{ className: "name" }}, a.repo || a.name || "?"),
        el("div", {{ className: "meta" }}, a.path || a.artifact_dir || JSON.stringify(a)),
      )
    ));
  }});
}}

async function viewRegistry() {{
  await viewPage("Registry", async () => {{
    const data = await api("/api/layers");
    const layers = data.layers || [];
    if (!layers.length) return el("p", {{ className: "sub" }}, "No layers found (set VEIL_LAYERS_DIR or use monorepo layers/).");
    return el("div", {{}}, ...layers.map(l =>
      el("div", {{ className: "card" }},
        el("div", {{ className: "name" }}, l.name || l.id || "?"),
        el("div", {{ className: "meta" }}, l.path || l.kind || ""),
      )
    ));
  }});
}}

async function viewBus() {{
  const out = el("pre", {{ className: "sub" }}, "");
  const typeIn = el("input", {{ value: "ListRepos", id: "busType" }});
  const go = el("button", {{
    type: "button",
    onClick: async () => {{
      try {{
        const message = {{ type: typeIn.value || "ListRepos" }};
        const r = await api("/bus/invoke", {{ method: "POST", body: JSON.stringify({{ message }}) }});
        out.textContent = JSON.stringify(r, null, 2);
      }} catch (e) {{ out.textContent = String(e); }}
    }},
  }}, "Invoke");
  shell(el("div", {{}},
    el("h1", {{}}, "Bus"),
    el("p", {{ className: "sub" }}, "POST /bus/invoke — generated storage handlers"),
    el("label", {{}}, "message.type"),
    el("div", {{ className: "row" }}, typeIn, go),
    out,
  ));
}}

async function viewAgents() {{
  shell(el("div", {{}},
    el("h1", {{}}, "Agents"),
    el("p", {{ className: "sub" }}, "Full ACP turns run in the dual-loop IDE agent dock."),
    el("p", {{}}, "Open a project IDE, then use the agent panel:"),
    el("code", {{}}, "POST /api/p/{{project}}/agent/turn"),
    el("p", {{ className: "sub" }}, "Bus HandleAgentMessage returns a pointer to that path."),
  ));
}}

async function viewConfig() {{
  let cfg = {{}};
  try {{ cfg = await api("/api/config"); }} catch (e) {{
    shell(el("div", {{}}, el("h1", {{}}, "Config"), el("p", {{ className: "err" }}, String(e))));
    return;
  }}
  const input = el("input", {{ value: cfg.projects_dir || "", id: "pd" }});
  const status = el("p", {{ className: "sub" }}, "");
  const save = el("button", {{
    type: "button",
    onClick: async () => {{
      try {{
        const body = {{ projects_dir: input.value }};
        const r = await api("/api/config", {{ method: "PATCH", body: JSON.stringify(body) }});
        status.textContent = r.ok === false ? (r.error || "failed") : "Saved.";
      }} catch (e) {{ status.textContent = String(e); }}
    }},
  }}, "Save projects_dir");
  shell(el("div", {{}},
    el("h1", {{}}, "Config"),
    el("p", {{ className: "sub" }}, cfg.config_path || ""),
    el("label", {{}}, "projects_dir"),
    el("div", {{ className: "row" }}, input, save),
    status,
  ));
}}

function route() {{
  // Strip trailing slash without a regex (avoids codegen escape bugs).
  let p = location.pathname || "/";
  while (p.length > 1 && p.endsWith("/")) p = p.slice(0, -1);
  if (p === "/config" || p.startsWith("/config/")) return viewConfig();
  if (p === "/deploy" || p.startsWith("/deploy/")) return viewDeploy();
  if (p === "/registry" || p.startsWith("/registry/")) return viewRegistry();
  if (p === "/bus" || p.startsWith("/bus/")) return viewBus();
  if (p === "/agents" || p.startsWith("/agents/")) return viewAgents();
  if (p === "/projects" || p.startsWith("/projects/")) {{
    // /projects/{{name}}/ide is a full HTML page (iframe), not SPA
    if (p.includes("/ide")) return;
    return viewProjects();
  }}
  return viewDashboard();
}}

route();
window.addEventListener("popstate", route);
"#,
        sol_name = sol_name,
        nav_items = nav_items,
    );

    files.push(TsFile {
        path: "src/spa.js".into(),
        content: app_js.clone(),
    });

    let index_html = r#"<!DOCTYPE html>
<html lang="en" data-theme="dark">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>VEIL Runtime</title>
  <style>
    :root {
      --bg: #0f0f0f; --surface: #1a1a1a; --border: #2e2e2e;
      --text: #e5e5e5; --dim: #737373; --accent: #a5b4fc;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0; font-family: Inter, system-ui, sans-serif;
      background: var(--bg); color: var(--text); min-height: 100vh;
    }
    #app { display: grid; grid-template-columns: 220px 1fr; min-height: 100vh; }
    .sidebar { border-right: 1px solid var(--border); padding: 20px 0; background: var(--surface); }
    .logo { padding: 0 20px 24px; font-weight: 700; }
    .logo span { color: var(--accent); }
    nav a {
      display: block; padding: 10px 20px; color: var(--dim);
      text-decoration: none; font-size: 14px;
    }
    nav a:hover, nav a.active { color: var(--accent); background: rgba(165,180,252,0.08); }
    main { padding: 28px 32px; }
    h1 { margin: 0 0 8px; font-size: 24px; }
    .sub { color: var(--dim); font-size: 13px; margin-bottom: 24px; }
    .err { color: #f87171; }
    .stats { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 12px; margin-bottom: 28px; }
    .stat { background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 16px; }
    .stat .v { font-size: 28px; font-weight: 700; color: var(--accent); }
    .stat .l { font-size: 11px; text-transform: uppercase; color: var(--dim); }
    .card {
      display: block; padding: 14px 16px; margin-bottom: 10px;
      border: 1px solid var(--border); border-radius: 8px;
      text-decoration: none; color: var(--text); background: var(--surface);
    }
    .card:hover { border-color: var(--accent); }
    .card .name { font-weight: 600; }
    .card .meta { font-size: 12px; color: var(--dim); margin-top: 4px; }
    .row { display: flex; gap: 8px; margin: 16px 0; flex-wrap: wrap; align-items: center; }
    input {
      padding: 8px 12px; border-radius: 6px; border: 1px solid var(--border);
      background: #0003; color: var(--text); min-width: 280px;
    }
    button {
      padding: 8px 14px; border-radius: 6px; border: 1px solid var(--accent);
      background: transparent; color: var(--accent); cursor: pointer;
    }
    button:hover { background: rgba(165,180,252,0.12); }
    label { display: block; font-size: 12px; color: var(--dim); margin-top: 12px; }
  </style>
</head>
<body>
  <div id="app"></div>
  <!-- Absolute path so ProductHost can serve from /static/dist/ -->
  <script type="module" src="/static/dist/spa.js"></script>
</body>
</html>
"#
    .to_string();

    files.push(TsFile {
        path: "index.html".into(),
        content: index_html.clone(),
    });
    // dist/ is what ProductHost prefers as primary SPA
    files.push(TsFile {
        path: "dist/index.html".into(),
        content: index_html.clone(),
    });
    files.push(TsFile {
        path: "dist/spa.js".into(),
        content: app_js,
    });

    // Zero-deps "build": copy src/spa.js → dist (script for make pure-runtime)
    files.push(TsFile {
        path: "scripts/bundle-spa.sh".into(),
        content: "#!/bin/sh\nset -e\nmkdir -p dist\ncp index.html dist/\ncp src/spa.js dist/\necho \"SPA dist ready\"\n".into(),
    });

    // package.json build script for SPA
    files.push(TsFile {
        path: "package.spa.json".into(),
        content: format!(
            r#"{{
  "name": "{sol_name}-spa",
  "private": true,
  "type": "module",
  "scripts": {{
    "build": "mkdir -p dist && cp index.html dist/ && cp src/spa.js dist/",
    "dev": "echo 'Serve dist/ via veil-runtime ProductHost'"
  }}
}}
"#
        ),
    });

    files
}

/// Collect constructs of a given shape from a module tree.
fn collect_shape<'a>(module: &'a Construct, shape: Shape) -> Vec<&'a Construct> {
    let mut result = Vec::new();
    fn walk<'a>(c: &'a Construct, shape: Shape, result: &mut Vec<&'a Construct>) {
        for child in &c.children {
            if child.shape == shape {
                result.push(child);
            }
            if child.shape == Shape::Group || child.shape == Shape::Mod {
                walk(child, shape, result);
            }
        }
    }
    walk(module, shape, &mut result);
    result
}

/// Generate types.ts — interfaces for all struct-shaped constructs.
fn gen_types(modules: &[&Construct]) -> TsFile {
    let mut out = String::from("// Generated by VEIL — do not edit\n\n");

    for module in modules {
        let structs = collect_shape(module, Shape::Struct);
        let enums = collect_shape(module, Shape::Enum);

        for s in &structs {
            if s.layer_provided { continue; }
            let generics = generic_params_ts(&s.type_params);
            out.push_str(&format!("export interface {}{} {{\n", s.name, generics));
            // Fields from root block or direct fields
            let fields = if !s.blocks.is_empty() {
                s.blocks.iter()
                    .filter(|b| b.shape != Shape::Enum)
                    .flat_map(|b| b.fields.iter())
                    .collect::<Vec<_>>()
            } else {
                s.fields.iter().collect::<Vec<_>>()
            };
            for f in &fields {
                let ts_type = field_type_ts(f);
                out.push_str(&format!("  {}: {};\n", to_camel(&f.name), ts_type));
            }
            out.push_str("}\n\n");
        }

        for e in &enums {
            if e.layer_provided { continue; }
            if e.variants.is_empty() && e.rich_variants.is_empty() { continue; }
            let generics = generic_params_ts(&e.type_params);

            if !e.rich_variants.is_empty() {
                // Discriminated union for variants with data
                out.push_str(&format!("export type {}{} =\n", e.name, generics));
                let variants: Vec<String> = e.rich_variants.iter()
                    .map(|v| match v {
                        EnumVariant::Unit(name) => format!("  | {{ type: \"{}\" }}", name),
                        EnumVariant::Tuple(name, types) => {
                            let fields = types.iter().enumerate()
                                .map(|(i, t)| format!("field{}: {}", i, type_to_ts(t)))
                                .collect::<Vec<_>>().join("; ");
                            format!("  | {{ type: \"{}\"; {} }}", name, fields)
                        }
                        EnumVariant::Struct(name, fields) => {
                            let fs = fields.iter()
                                .map(|f| format!("{}: {}", to_camel(&f.name), type_to_ts(&f.type_expr)))
                                .collect::<Vec<_>>().join("; ");
                            format!("  | {{ type: \"{}\"; {} }}", name, fs)
                        }
                    })
                    .collect();
                out.push_str(&variants.join("\n"));
                out.push_str(";\n\n");
            } else {
                // Simple string union for unit-only enums
                out.push_str(&format!("export type {}{} =\n", e.name, generics));
                let variants: Vec<String> = e.variants.iter()
                    .map(|v| format!("  | \"{}\"", v))
                    .collect();
                out.push_str(&variants.join("\n"));
                out.push_str(";\n\n");
            }
        }
    }

    TsFile { path: "src/types.ts".to_string(), content: out }
}

/// Generate interfaces.ts — interfaces for all trait-shaped constructs (ports).
fn gen_interfaces(modules: &[&Construct], solution: &Solution) -> TsFile {
    let mut out = String::from("// Generated by VEIL — do not edit\n\n");

    // Collect all traits
    let mut all_traits: Vec<&Construct> = Vec::new();
    for module in modules {
        all_traits.extend(collect_shape(module, Shape::Trait));
    }
    // Also include top-level layer-provided traits
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Trait && c.layer_provided {
                all_traits.push(c);
            }
        }
    }

    for t in &all_traits {
        let generics = generic_params_ts(&t.type_params);
        out.push_str(&format!("export interface {}{} {{\n", t.name, generics));
        for method in &t.methods {
            let params = method.params.iter()
                .map(|p| format!("{}: {}", to_camel(&p.name), type_to_ts(&p.type_expr)))
                .collect::<Vec<_>>().join(", ");
            let ret = match &method.return_type {
                Some(ty) => type_to_ts(ty),
                None => "Promise<void>".to_string(),
            };
            out.push_str(&format!("  {}({}): {};\n", to_camel(&method.name), params, ret));
        }
        out.push_str("}\n\n");
    }

    TsFile { path: "src/interfaces.ts".to_string(), content: out }
}

/// Generate services.ts — async functions for all fn-shaped constructs.
fn gen_services(modules: &[&Construct], solution: &Solution) -> TsFile {
    let mut out = String::from("// Generated by VEIL — do not edit\n\n");
    out.push_str("import type * as T from './types';\n");
    out.push_str("import type * as I from './interfaces';\n\n");

    for module in modules {
        let fns = collect_shape(module, Shape::Fn);
        for f in &fns {
            if f.layer_provided { continue; }
            // Generate as async function
            let fn_name = to_camel(&f.name);
            let params = f.inputs.iter()
                .map(|p| format!("{}: {}", to_camel(&p.name), type_to_ts(&p.type_expr)))
                .collect::<Vec<_>>().join(", ");
            let ret = f.return_type.as_ref()
                .map(|t| type_to_ts(t))
                .unwrap_or_else(|| "Promise<void>".to_string());

            out.push_str(&format!("export async function {}({}): {} {{\n", fn_name, params, ret));

            // Generate step bodies
            if !f.steps.is_empty() {
                for step in &f.steps {
                    if let FlowStep::Step(s) = step {
                        out.push_str(&format!("  // Step: {}\n", s.name));
                        for expr in &s.body {
                            out.push_str(&format!("  {};\n", expr_to_ts(expr, 1)));
                        }
                    }
                }
            } else if !f.fns.is_empty() {
                // Inline method bodies
                out.push_str("  // TODO: implement\n");
            }

            out.push_str("}\n\n");
        }
    }

    // Also generate top-level flows
    for item in &solution.items {
        if let TopLevelItem::Flow(flow) = item {
            let fn_name = to_camel(&flow.name);
            let params = flow.inputs.iter()
                .map(|p| format!("{}: {}", to_camel(&p.name), type_to_ts(&p.type_expr)))
                .collect::<Vec<_>>().join(", ");

            out.push_str(&format!("export async function {}({}): Promise<void> {{\n", fn_name, params));
            for step in &flow.steps {
                if let FlowStep::Step(s) = step {
                    out.push_str(&format!("  // Step: {}\n", s.name));
                    for expr in &s.body {
                        out.push_str(&format!("  {};\n", expr_to_ts(expr, 1)));
                    }
                }
            }
            out.push_str("}\n\n");
        }
    }

    TsFile { path: "src/services.ts".to_string(), content: out }
}

/// Generate .svelte files for UI constructs (layer-identified, INV-005).
fn gen_svelte_components(modules: &[&Construct], registry: &LayerRegistry) -> Vec<TsFile> {
    let mut files = Vec::new();

    for module in modules {
        let components = collect_all_components(module, registry);
        for comp in components {
            files.push(gen_svelte_file(comp));
        }
    }

    files
}

/// True if this construct is a Svelte UI emit target per layer identity.
/// Uses `LayerRegistry::is_a` so aliases mapping to Component/Page/Layout work.
fn is_svelte_ui_construct(c: &Construct, registry: &LayerRegistry) -> bool {
    if c.shape != Shape::Struct {
        return false;
    }
    let kw = c.keyword.as_str();
    let sk = c.subkind.as_str();
    // Layer name or keyword chain (supports alias constructs)
    for ancestor in ["Component", "Page", "Layout"] {
        if registry.is_a(kw, ancestor) || registry.is_a(sk, ancestor) {
            return true;
        }
        if let Some(spec) = registry.construct_by_name(sk).or_else(|| registry.construct(kw)) {
            if spec.name == ancestor || registry.is_a(&spec.keyword, ancestor) {
                return true;
            }
        }
    }
    false
}

/// Recursively collect UI constructs from the module tree (INV-005).
fn collect_all_components<'a>(c: &'a Construct, registry: &LayerRegistry) -> Vec<&'a Construct> {
    let mut result = Vec::new();
    for child in &c.children {
        if is_svelte_ui_construct(child, registry) {
            result.push(child);
        }
        if child.shape == Shape::Group || child.shape == Shape::Mod {
            result.extend(collect_all_components(child, registry));
        }
    }
    result
}

/// Generate a single .svelte file from a Component construct.
fn gen_svelte_file(comp: &Construct) -> TsFile {
    let mut script = String::new();
    let mut template_content = String::new();
    let mut style_content = String::new();

    // Collect blocks by keyword
    let props_block = comp.blocks.iter().find(|b| b.keyword == "props");
    let state_block = comp.blocks.iter().find(|b| b.keyword == "state");
    let derived_block = comp.blocks.iter().find(|b| b.keyword == "derived");

    // Look for template and style — first from raw_blocks (preferred),
    // then fall back to fn template()/fn style() hack for backward compat
    for (name, content) in &comp.raw_blocks {
        if name == "template" {
            template_content = content.clone();
        } else if name == "style" {
            style_content = content.clone();
        }
    }

    // Fallback: check fns named template/style (old pattern)
    if template_content.is_empty() || style_content.is_empty() {
        for f in &comp.fns {
            if f.name == "template" || f.name == "style" {
                let content_expr = f.body.first().and_then(|expr| match expr {
                    Expr::StringLit(s) => Some(s.clone()),
                    Expr::Return(inner) => {
                        if let Expr::StringLit(s) = inner.as_ref() {
                            Some(s.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                });
                if let Some(content) = content_expr {
                    if f.name == "template" && template_content.is_empty() {
                        template_content = content;
                    } else if f.name == "style" && style_content.is_empty() {
                        style_content = content;
                    }
                }
            }
        }
    }
    // Also check direct fields named template/style
    for field in &comp.fields {
        if field.name == "template" || field.name == "style" {
            // The field default value would be in a StringLit — but fields don't
            // carry default values in the current AST. Check blocks instead.
        }
    }

    // ─── <script lang="ts"> ───────────────────────────────────────────
    script.push_str("<script lang=\"ts\">\n");
    script.push_str("  // Generated by VEIL\n");

    // Detect component references in template (PascalCase tags like <CustomerCard .../>)
    let mut imports: Vec<String> = Vec::new();
    if !template_content.is_empty() {
        let re_chars: Vec<char> = template_content.chars().collect();
        let mut i = 0;
        while i < re_chars.len() {
            if re_chars[i] == '<' && i + 1 < re_chars.len() && re_chars[i + 1].is_uppercase() {
                i += 1;
                let mut comp_name = String::new();
                while i < re_chars.len() && (re_chars[i].is_alphanumeric() || re_chars[i] == '_') {
                    comp_name.push(re_chars[i]);
                    i += 1;
                }
                if !comp_name.is_empty() && comp_name != comp.name && !imports.contains(&comp_name) {
                    imports.push(comp_name);
                }
            } else {
                i += 1;
            }
        }
    }
    if !imports.is_empty() {
        script.push('\n');
        for imp in &imports {
            script.push_str(&format!("  import {} from './{}.svelte';\n", imp, imp));
        }
    }

    // Props: interface + $props()
    if let Some(props) = props_block {
        script.push_str("\n  interface Props {\n");
        for field in &props.fields {
            let ty = field_type_ts(field);
            script.push_str(&format!("    {}: {};\n", to_camel(&field.name), ty));
        }
        script.push_str("  }\n");
        let prop_names: Vec<String> = props.fields.iter()
            .map(|f| to_camel(&f.name))
            .collect();
        script.push_str(&format!("  let {{ {} }}: Props = $props();\n", prop_names.join(", ")));
    }

    // State: $state() declarations
    if let Some(state) = state_block {
        script.push('\n');
        for field in &state.fields {
            let ty = field_type_ts(field);
            let default = default_value_for_ts(&ty);
            script.push_str(&format!("  let {} = $state<{}>({});\n", to_camel(&field.name), ty, default));
        }
    }

    // Derived: $derived() declarations
    if let Some(derived) = derived_block {
        script.push('\n');
        for field in &derived.fields {
            let ty = field_type_ts(field);
            if let Some(expr) = &field.default_expr {
                let expr_str = expr_to_ts(expr, 1);
                script.push_str(&format!("  let {} = $derived(() => {});\n", to_camel(&field.name), expr_str));
            } else {
                script.push_str(&format!("  let {} = $derived<{}>(() => undefined as any);\n", to_camel(&field.name), ty));
            }
        }
    }

    // Effects: $effect(() => { ... })
    if !comp.effects.is_empty() {
        script.push('\n');
        for eff in &comp.effects {
            if eff.cleanup.is_empty() {
                script.push_str(&format!("  $effect(() => {{ // {}\n", eff.name));
                for expr in &eff.body {
                    script.push_str(&format!("    {};\n", expr_to_ts(expr, 2)));
                }
                script.push_str("  });\n");
            } else {
                script.push_str(&format!("  $effect(() => {{ // {}\n", eff.name));
                for expr in &eff.body {
                    script.push_str(&format!("    {};\n", expr_to_ts(expr, 2)));
                }
                script.push_str("    return () => {\n");
                for expr in &eff.cleanup {
                    script.push_str(&format!("      {};\n", expr_to_ts(expr, 3)));
                }
                script.push_str("    };\n");
                script.push_str("  });\n");
            }
        }
    }

    // Methods: fn → function
    let method_fns: Vec<&FnDef> = comp.fns.iter()
        .filter(|f| f.name != "template" && f.name != "style")
        .collect();
    if !method_fns.is_empty() {
        script.push('\n');
        for f in &method_fns {
            let params = f.params.iter()
                .map(|p| format!("{}: {}", to_camel(&p.name), type_to_ts(&p.type_expr)))
                .collect::<Vec<_>>().join(", ");
            let is_async = f.return_type.as_ref()
                .map(|t| matches!(t, TypeExpr::Result(_)))
                .unwrap_or(false);
            let async_kw = if is_async { "async " } else { "" };
            script.push_str(&format!("  {}function {}({}) {{\n", async_kw, to_camel(&f.name), params));
            for expr in &f.body {
                script.push_str(&format!("    {};\n", expr_to_ts(expr, 2)));
            }
            script.push_str("  }\n");
        }
    }

    script.push_str("</script>\n");

    // ─── Template ─────────────────────────────────────────────────────
    // GEN-004: zero-raw shell is valid — explicit empty placeholder, not a fake TODO.
    let template_section = if template_content.is_empty() {
        format!(
            "\n<!-- veil: empty template shell for {} — add `template` raw block when needed -->\n<div class=\"veil-shell\"></div>\n",
            comp.name
        )
    } else {
        format!("\n{}\n", dedent_block(&template_content))
    };

    // ─── <style> ──────────────────────────────────────────────────────
    let style_section = if style_content.is_empty() {
        "\n<style>\n  /* TODO: Add component styles */\n</style>\n".to_string()
    } else {
        format!("\n<style>\n{}\n</style>\n", dedent_block(&style_content))
    };

    // Assemble the .svelte file
    let content = format!("{}{}{}", script, template_section, style_section);
    let path = format!("src/components/{}.svelte", comp.name);

    TsFile { path, content }
}

/// Get a sensible default value for a TypeScript type.
fn default_value_for_ts(ty: &str) -> &str {
    match ty {
        "boolean" => "false",
        "number" => "0",
        "string" => "''",
        _ if ty.ends_with("[]") => "[]",
        _ if ty.ends_with("| null") => "null",
        _ => "undefined as any",
    }
}

/// Remove common leading indentation from a multi-line string block.
fn dedent_block(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.is_empty() { return String::new(); }
    // Find minimum non-empty indent
    let min_indent = lines.iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines.iter()
        .map(|l| if l.len() > min_indent { &l[min_indent..] } else { l.trim() })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate index.ts with re-exports.
fn gen_index(sol_name: &str) -> TsFile {
    let content = format!(
        "// {} — Generated by VEIL\n\nexport * from './types';\nexport * from './interfaces';\nexport * from './services';\n",
        sol_name
    );
    TsFile { path: "src/index.ts".to_string(), content }
}

/// Generate package.json.
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

/// Generate tsconfig.json.
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
