//! Expression translator — converts VEIL AST Expr to Rust source code.
//!
//! Fully shape-driven: the translator uses `GenCtx.name_to_shape` to decide
//! how to emit a Call (port call → deps.x.method().await?, struct call →
//! Type::new(args), local → target.method(args)).

use std::collections::{HashMap, HashSet};

use veil_ir::ast::*;
use veil_ir::layer::{Shape, StmtShape};

use crate::rust::{to_snake, type_to_rust};

/// Code generation context — carries name resolution information.
pub struct GenCtx {
    /// All constructs in the solution by name → shape.
    pub name_to_shape: HashMap<String, Shape>,
    /// Locals accumulated in the current scope (let-bound variables).
    pub locals: HashSet<String>,
    /// Fields of the enclosing struct (when inside an aggregate fn body).
    pub self_fields: HashSet<String>,
    /// Whether we're inside an aggregate fn (use `self.` for field access).
    pub in_aggregate_fn: bool,
}

impl GenCtx {
    pub fn new(name_to_shape: HashMap<String, Shape>) -> Self {
        GenCtx {
            name_to_shape,
            locals: HashSet::new(),
            self_fields: HashSet::new(),
            in_aggregate_fn: false,
        }
    }

    /// Is this name a known trait-shaped construct (port/repo/integration)?
    pub fn is_trait_target(&self, name: &str) -> bool {
        matches!(self.name_to_shape.get(name), Some(Shape::Trait))
    }

    /// Is this name a known struct-shaped construct?
    pub fn is_struct_target(&self, name: &str) -> bool {
        matches!(self.name_to_shape.get(name), Some(Shape::Struct))
    }

    /// Is this name a known local variable?
    pub fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }
}

/// Translate a VEIL expression to a Rust expression string (no trailing semicolon).
pub fn expr_to_rust(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("self.{}", to_snake(name))
            } else {
                name.clone()
            }
        }
        Expr::FieldAccess(base, field) => {
            format!("{}.{}", expr_to_rust(base, ctx), to_snake(field))
        }
        Expr::Call(call) => translate_call(call, ctx),
        Expr::BinaryOp(op) => {
            let l = expr_to_rust(&op.left, ctx);
            let r = expr_to_rust(&op.right, ctx);
            format!("{} {} {}", l, binop_to_rust(&op.op), r)
        }
        Expr::UnaryOp(op) => {
            let inner = expr_to_rust(&op.expr, ctx);
            format!("{}{}", unaryop_to_rust(&op.op), inner)
        }
        Expr::IfExpr(ie) => {
            let cond = expr_to_rust(&ie.condition, ctx);
            let then_body = ie.then_body.iter()
                .map(|e| format!("    {};", expr_to_rust(e, ctx)))
                .collect::<Vec<_>>().join("\n");
            if let Some(else_body) = &ie.else_body {
                let else_stmts = else_body.iter()
                    .map(|e| format!("    {};", expr_to_rust(e, ctx)))
                    .collect::<Vec<_>>().join("\n");
                format!("if {} {{\n{}\n}} else {{\n{}\n}}", cond, then_body, else_stmts)
            } else {
                format!("if {} {{\n{}\n}}", cond, then_body)
            }
        }
        Expr::Assign(name, rhs) => {
            let rhs_str = expr_to_rust(rhs, ctx);
            if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("self.{} = {}", to_snake(name), rhs_str)
            } else {
                format!("let {} = {}", name, rhs_str)
            }
        }
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => {
            let inner_str = expr_to_rust(inner, ctx);
            format!("return Ok({})", inner_str)
        }
        Expr::Action(a) => translate_action(a, ctx),
        Expr::StructLit(name, fields) => {
            let fs = fields.iter().map(|(k, v)| {
                let v_str = expr_to_rust(v, ctx);
                if k == &v_str { k.clone() } else { format!("{}: {}", to_snake(k), v_str) }
            }).collect::<Vec<_>>().join(", ");
            format!("{} {{ {} }}", name, fs)
        }
    }
}

/// Translate a Call expression with shape-aware name resolution.
fn translate_call(call: &CallExpr, ctx: &GenCtx) -> String {
    let args_str = call.args.iter()
        .map(|a| expr_to_rust(a, ctx))
        .collect::<Vec<_>>().join(", ");

    // Trait-shaped target → deps.target.method(args).await?
    if ctx.is_trait_target(&call.target) {
        let dep_name = to_snake(&call.target);
        let method = if call.method.is_empty() { "call" } else { &call.method };
        return format!("deps.{}.{}({}).await?", dep_name, to_snake(method), args_str);
    }

    // Struct-shaped target with method "new" or empty → Type::new(args)?
    if ctx.is_struct_target(&call.target) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        if method == "new" {
            return format!("{}::new({})?", call.target, args_str);
        }
        return format!("{}::{}({})", call.target, to_snake(method), args_str);
    }

    // Local variable target → target.method(args)?
    if ctx.is_local(&call.target) {
        let method = to_snake(&call.method);
        return format!("{}.{}({})?", call.target, method, args_str);
    }

    // Unknown: bare function call or method call
    if call.method.is_empty() {
        // Bare call: now() → Utc::now(), others → as-is
        match call.target.as_str() {
            "now" => "Utc::now()".to_string(),
            _ => format!("{}({})", to_snake(&call.target), args_str),
        }
    } else {
        format!("{}.{}({})", call.target, to_snake(&call.method), args_str)
    }
}

/// Translate a layer-defined Action that was NOT desugared (e.g. emit, guard).
fn translate_action(a: &ActionExpr, ctx: &GenCtx) -> String {
    match a.shape {
        StmtShape::If => {
            // guard: `if !(cond) { return Err(DomainError::Validation("msg".into())); }`
            let cond = a.condition.as_ref()
                .map(|c| expr_to_rust(c, ctx))
                .unwrap_or_else(|| "true".to_string());
            let msg = a.message.as_deref().unwrap_or("precondition failed");
            format!(
                "if !({}) {{ return Err(DomainError::Validation(\"{}\".into())); }}",
                cond, msg
            )
        }
        StmtShape::Call => {
            // Remaining actions (emit) — handle based on keyword-like semantics.
            // For now, emit as a comment + placeholder.
            let args_str = if !a.named_args.is_empty() {
                let fields = a.named_args.iter()
                    .map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx)))
                    .collect::<Vec<_>>().join(", ");
                format!("{} {{ {} }}", a.target, fields)
            } else if !a.args.is_empty() {
                a.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ")
            } else {
                a.target.clone()
            };
            // `emit` keyword → events.push(...)
            format!("/* {} {} */", a.keyword, args_str)
        }
    }
}

/// Translate a full statement (expression at statement position) with semicolons.
pub fn stmt_to_rust(expr: &Expr, ctx: &mut GenCtx) -> String {
    match expr {
        Expr::Assign(name, _) => {
            let s = expr_to_rust(expr, ctx);
            // Track as local for future lookups
            ctx.locals.insert(name.clone());
            format!("    {};", s)
        }
        _ => format!("    {};", expr_to_rust(expr, ctx)),
    }
}

/// Collect all trait-shaped construct names referenced in flow step bodies.
/// Returns the set of port names that need to be in the Deps struct.
pub fn collect_deps(steps: &[FlowStep], ctx: &GenCtx) -> HashSet<String> {
    let mut deps = HashSet::new();
    for step in steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                collect_deps_from_expr(expr, ctx, &mut deps);
            }
        }
    }
    deps
}

fn collect_deps_from_expr(expr: &Expr, ctx: &GenCtx, deps: &mut HashSet<String>) {
    match expr {
        Expr::Call(call) => {
            if ctx.is_trait_target(&call.target) {
                deps.insert(call.target.clone());
            }
            for arg in &call.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
        }
        Expr::Assign(_, rhs) => collect_deps_from_expr(rhs, ctx, deps),
        Expr::Action(a) => {
            for arg in &a.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
        }
        Expr::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_deps_from_expr(v, ctx, deps);
            }
        }
        _ => {}
    }
}

/// Generate the Deps struct source for a set of trait dependencies.
pub fn gen_deps_struct(dep_names: &HashSet<String>) -> String {
    if dep_names.is_empty() {
        return String::new();
    }
    let mut out = String::from("/// Injected dependencies (ports).\npub struct Deps {\n");
    let mut sorted: Vec<&String> = dep_names.iter().collect();
    sorted.sort();
    for name in sorted {
        out.push_str(&format!(
            "    pub {}: std::sync::Arc<dyn {} + Send + Sync>,\n",
            to_snake(name), name
        ));
    }
    out.push_str("}\n\n");
    out
}

fn binop_to_rust(op: &BinOp) -> &'static str {
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

fn unaryop_to_rust(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}
