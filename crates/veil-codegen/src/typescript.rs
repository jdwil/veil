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

        Expr::Assign(name, value) => {
            format!("{} = {}", to_camel(name), expr_to_ts(value, indent))
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
pub fn generate_ts(solution: &Solution, _registry: &LayerRegistry) -> TsProject {
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

    // Generate Svelte component files for Component-shaped structs
    // (identified by subkind "Component" from the svelte5 layer)
    files.extend(gen_svelte_components(&modules));

    // Generate index.ts (re-exports)
    files.push(gen_index(&sol_name));

    // Generate package.json
    files.push(gen_package_json(&sol_name));

    // Generate tsconfig.json
    files.push(gen_tsconfig());

    TsProject { files }
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

/// Generate .svelte files for Component-shaped constructs.
/// A component with `props`, `state`, `derived`, and `fn` blocks becomes a
/// single-file Svelte 5 component using runes ($props, $state, $derived).
/// The `template` and `style` fields (if present as StringLit expressions in
/// the component's methods/fields) are emitted as raw markup and CSS.
fn gen_svelte_components(modules: &[&Construct]) -> Vec<TsFile> {
    let mut files = Vec::new();

    for module in modules {
        let components = collect_all_components(module);
        for comp in components {
            files.push(gen_svelte_file(comp));
        }
    }

    files
}

/// Recursively collect all struct-shaped constructs with subkind "Component",
/// "Page", "Layout", or "Store" from the module tree.
fn collect_all_components(c: &Construct) -> Vec<&Construct> {
    let mut result = Vec::new();
    for child in &c.children {
        let sk = child.subkind.as_str();
        if child.shape == Shape::Struct && (sk == "Component" || sk == "Page" || sk == "Layout") {
            result.push(child);
        }
        // Recurse into groups
        if child.shape == Shape::Group || child.shape == Shape::Mod {
            result.extend(collect_all_components(child));
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
            script.push_str(&format!("  let {} = $derived<{}>(() => {{ /* TODO */ return undefined as any; }});\n", to_camel(&field.name), ty));
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
    let template_section = if template_content.is_empty() {
        "\n<!-- TODO: Add template markup -->\n".to_string()
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
