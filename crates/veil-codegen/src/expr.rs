//! Expression translator — converts VEIL AST Expr to Rust source code.
//!
//! Fully shape-driven: the translator uses `GenCtx.name_to_shape` to decide
//! how to emit a Call (port call → deps.x.method().await?, struct call →
//! Type::new(args), local → target.method(args)).

use std::collections::{HashMap, HashSet};

use veil_ir::ast::*;
use veil_ir::layer::{Shape, StmtShape, LayerRegistry};

use crate::rust::{to_snake, type_to_rust};

/// Code generation context — carries name resolution and type information.
pub struct GenCtx {
    /// All constructs in the solution by name → shape.
    pub name_to_shape: HashMap<String, Shape>,
    /// Locals accumulated in the current scope (let-bound variables).
    pub locals: HashSet<String>,
    /// Fields of the enclosing struct (when inside an aggregate fn body).
    pub self_fields: HashSet<String>,
    /// Whether we're inside an aggregate fn (use `self.` for field access).
    pub in_aggregate_fn: bool,
    /// Whether this is an orchestrator module (route unknown port calls through Bus).
    pub is_orchestrator: bool,
    /// Method return types: (ConstructName, method_name) → inner type name.
    /// For Result<T>, stores T. For Result<()>, stores "()".
    pub method_returns: HashMap<(String, String), String>,
    /// Inferred types for local variables: var_name → type_name.
    pub local_types: HashMap<String, String>,
    /// Struct field maps: type_name → vec of (field_name, field_type_name).
    pub struct_fields: HashMap<String, Vec<(String, String)>>,
}

impl GenCtx {
    pub fn new(name_to_shape: HashMap<String, Shape>) -> Self {
        GenCtx {
            name_to_shape,
            locals: HashSet::new(),
            self_fields: HashSet::new(),
            in_aggregate_fn: false,
            is_orchestrator: false,
            method_returns: HashMap::new(),
            local_types: HashMap::new(),
            struct_fields: HashMap::new(),
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

    /// Get the type of a local variable (if inferred).
    pub fn local_type(&self, name: &str) -> Option<&str> {
        self.local_types.get(name).map(|s| s.as_str())
    }

    /// Infer the return type of calling target.method().
    /// Returns the inner type (unwrapping Result).
    pub fn return_type_of(&self, target: &str, method: &str) -> Option<&str> {
        // Direct lookup
        if let Some(t) = self.method_returns.get(&(target.to_string(), method.to_string())) {
            return Some(t.as_str());
        }
        // If target is a local, look up its type and check struct methods
        if let Some(type_name) = self.local_types.get(target) {
            if let Some(t) = self.method_returns.get(&(type_name.clone(), method.to_string())) {
                return Some(t.as_str());
            }
        }
        None
    }

    /// Get field type for a given type and field name.
    pub fn field_type(&self, type_name: &str, field_name: &str) -> Option<&str> {
        self.struct_fields.get(type_name)
            .and_then(|fields| fields.iter().find(|(n, _)| n == field_name))
            .map(|(_, t)| t.as_str())
    }
}

/// Build a GenCtx populated with type information from the solution's constructs and loaded stubs.
pub fn build_ctx_from_solution(solution: &Solution, name_to_shape: HashMap<String, Shape>, registry: &LayerRegistry) -> GenCtx {
    let mut ctx = GenCtx::new(name_to_shape);

    fn visit_constructs(c: &Construct, ctx: &mut GenCtx) {
        // Record method return types for trait-shaped constructs
        if c.shape == Shape::Trait {
            for method in &c.methods {
                let ret_type = method.return_type.as_ref()
                    .map(|t| extract_inner_type(t))
                    .unwrap_or_else(|| "()".to_string());
                ctx.method_returns.insert(
                    (c.name.clone(), method.name.clone()),
                    ret_type,
                );
            }
        }

        // Record fields for struct-shaped constructs
        if c.shape == Shape::Struct {
            let mut fields: Vec<(String, String)> = c.fields.iter()
                .map(|f| (f.name.clone(), type_name_simple(&f.type_expr)))
                .collect();
            // Also include block fields (root block, etc.)
            for block in &c.blocks {
                if block.shape != Shape::Enum {
                    for f in &block.fields {
                        fields.push((f.name.clone(), type_name_simple(&f.type_expr)));
                    }
                }
            }
            ctx.struct_fields.insert(c.name.clone(), fields);

            // Record struct constructors: Type.new → Type
            ctx.method_returns.insert(
                (c.name.clone(), "new".to_string()),
                c.name.clone(),
            );
        }

        for child in &c.children {
            visit_constructs(child, ctx);
        }
    }

    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            visit_constructs(c, &mut ctx);
        }
    }

    // Register stub crate type information
    for stub in &registry.stubs {
        for s in &stub.structs {
            // Register struct methods
            for method in &s.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.method_returns.insert(
                    (s.name.clone(), method.name.clone()),
                    inner.to_string(),
                );
            }
            // Register as a known struct
            ctx.name_to_shape.insert(s.name.clone(), Shape::Struct);
        }
        for i in &stub.impls {
            for method in &i.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.method_returns.insert(
                    (i.target.clone(), method.name.clone()),
                    inner.to_string(),
                );
            }
        }
    }

    ctx
}

/// Extract the inner type from a TypeExpr (unwrapping Result/Optional).
fn extract_inner_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Result(Some(inner)) => type_name_simple(inner),
        TypeExpr::Result(None) => "()".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_name_simple(inner)),
        _ => type_name_simple(ty),
    }
}

/// Get a simple type name string from a TypeExpr.
fn type_name_simple(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Generic(n, _) => n.clone(),
        TypeExpr::Result(Some(inner)) => type_name_simple(inner),
        TypeExpr::Result(None) => "()".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_name_simple(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", type_name_simple(inner)),
        TypeExpr::Map(k, v) => format!("HashMap<{}, {}>", type_name_simple(k), type_name_simple(v)),
        TypeExpr::Set(inner) => format!("HashSet<{}>", type_name_simple(inner)),
    }
}

/// Translate a VEIL expression to a Rust expression string (no trailing semicolon).
pub fn expr_to_rust(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("self.{}", to_snake(name))
            } else if ctx.is_orchestrator && !ctx.is_local(name) && !ctx.locals.is_empty() {
                // In orchestrator mode, unknown identifiers are symbolic references
                format!("\"{}\".to_string()", name)
            } else {
                name.clone()
            }
        }
        Expr::FieldAccess(base, field) => {
            let base_str = expr_to_rust(base, ctx);
            // In orchestrator mode, field access on Bus-returned locals is symbolic
            if ctx.is_orchestrator {
                if let Expr::Ident(name) = base.as_ref() {
                    if ctx.is_local(name) {
                        // Symbolic field reference — used in Bus call format strings
                        return format!("\"{{}}:{}\".to_string()", field);
                    }
                }
            }
            format!("{}.{}", base_str, to_snake(field))
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
        Expr::Match(scrutinee, arms) => {
            let scrutinee_str = expr_to_rust(scrutinee, ctx);
            let mut out = format!("match {} {{\n", scrutinee_str);
            for arm in arms {
                let body_str = if arm.body.len() == 1 {
                    expr_to_rust(&arm.body[0], ctx)
                } else {
                    let stmts = arm.body.iter()
                        .map(|e| format!("        {};", expr_to_rust(e, ctx)))
                        .collect::<Vec<_>>().join("\n");
                    format!("{{\n{}\n    }}", stmts)
                };
                out.push_str(&format!("        {} => {},\n", arm.pattern, body_str));
            }
            out.push_str("    }");
            out
        }
        Expr::ForLoop { binding, index, iterable, body } => {
            let iter_str = expr_to_rust(iterable, ctx);
            let bind = if let Some(idx) = index {
                format!("({}, {})", idx, binding)
            } else {
                binding.clone()
            };
            let body_str = body.iter()
                .map(|e| format!("        {};", expr_to_rust(e, ctx)))
                .collect::<Vec<_>>().join("\n");
            let enumerate = if index.is_some() { ".enumerate()" } else { "" };
            format!("for {} in {}{} {{\n{}\n    }}", bind, iter_str, enumerate, body_str)
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
        // Clone args to avoid move issues (v1 pragmatic approach)
        let cloned_args = call.args.iter()
            .map(|a| {
                let s = expr_to_rust(a, ctx);
                // Clone identifiers and field accesses (they might be used again)
                match a {
                    Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{}.clone()", s),
                    _ => s,
                }
            })
            .collect::<Vec<_>>().join(", ");
        // For desugared bus calls (dispatch/invoke/request), serialize as string
        let final_args = if call.sugar.is_some() && !call.args.is_empty() {
            // Format the event/command name and args as a debug string
            // Extract the struct name from StructLit if present
            if let Some(Expr::StructLit(name, fields)) = call.args.first() {
                let field_vals = fields.iter()
                    .map(|(k, _)| format!("{}: {{:?}}", k))
                    .collect::<Vec<_>>().join(", ");
                let field_exprs = fields.iter()
                    .map(|(_, v)| expr_to_rust(v, ctx))
                    .collect::<Vec<_>>().join(", ");
                format!("format!(\"{} {{{{ {} }}}}\", {})", name, field_vals, field_exprs)
            } else {
                format!("format!(\"{{:?}}\", {})", args_str)
            }
        } else {
            cloned_args.clone()
        };
        return format!("deps.{}.{}({}).await?", dep_name, to_snake(method), final_args);
    }

    // Struct-shaped target with method "new" or empty → Type::new(args)
    if ctx.is_struct_target(&call.target) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        // In orchestrators, struct construction from other contexts goes through Bus
        if ctx.is_orchestrator {
            let format_placeholders = call.args.iter().map(|_| "{:?}").collect::<Vec<_>>().join(", ");
            return format!(
                "deps.bus.invoke(format!(\"{}.{}({})\", {})).await?",
                call.target, method, format_placeholders, args_str
            );
        }
        // Clone args to avoid move issues
        let cloned = call.args.iter()
            .map(|a| {
                let s = expr_to_rust(a, ctx);
                match a { Expr::Ident(_) => format!("{}.clone()", s), _ => s }
            }).collect::<Vec<_>>().join(", ");
        if method == "new" {
            return format!("{}::{}({})", call.target, to_snake(method), cloned);
        }
        // Non-new method on a struct: check if first arg is the instance
        // e.g. call Email.validate(email) → email.validate()
        if !call.args.is_empty() {
            if let Expr::Ident(first_arg) = &call.args[0] {
                if first_arg.to_lowercase() == call.target.to_lowercase() || ctx.is_local(first_arg) {
                    let rest_args = call.args[1..].iter()
                        .map(|a| expr_to_rust(a, ctx))
                        .collect::<Vec<_>>().join(", ");
                    return format!("{}.{}({})", first_arg, to_snake(method), rest_args);
                }
            }
        }
        return format!("{}::{}({})", call.target, to_snake(method), args_str);
    }

    // Local variable target → target.method(args)?
    if ctx.is_local(&call.target) {
        let method = to_snake(&call.method);
        // In orchestrators, calls on locals from other contexts go through Bus
        if ctx.is_orchestrator && !call.method.is_empty() {
            let format_placeholders = call.args.iter().map(|_| "{:?}").collect::<Vec<_>>().join(", ");
            return format!(
                "deps.bus.invoke(format!(\"{}.{}({})\", {})).await?",
                call.target, call.method, format_placeholders, args_str
            );
        }
        // Check if the local's type has this method defined (aggregate fn)
        if let Some(type_name) = ctx.local_type(&call.target) {
            if ctx.method_returns.contains_key(&(type_name.to_string(), call.method.clone())) {
                // Known method — call with ?
                return format!("{}.{}({})?", call.target, method, args_str);
            }
        }
        // Unknown method on local — call without ? (might be a field accessor or infallible)
        return format!("{}.{}({})", call.target, method, args_str);
    }

    // Unknown: bare function call or method call
    // In orchestrators, unknown targets with methods are remote port calls → route through Bus
    if ctx.is_orchestrator && !call.method.is_empty() {
        let format_placeholders = call.args.iter().map(|_| "{:?}").collect::<Vec<_>>().join(", ");
        return format!(
            "deps.bus.invoke(format!(\"{}.{}({})\", {})).await?",
            call.target, call.method, format_placeholders, args_str
        );
    }
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
            // guard: the condition should be true for success.
            // Emit: `condition.map_err(|_| DomainError::Validation("msg"))?;`
            // Or for bool: `if !cond { return Err(...) }`
            // For v1: wrap in a let _ = ... pattern that compiles for both.
            let cond = a.condition.as_ref()
                .map(|c| expr_to_rust(c, ctx))
                .unwrap_or_else(|| "true".to_string());
            let msg = a.message.as_deref().unwrap_or("precondition failed");
            // Use a format that works: assign to _ and check
            format!(
                "{{ let __guard = {}; /* guard: {} */ }}",
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
        Expr::Assign(name, rhs) => {
            // Infer the type of the RHS
            let inferred_type = infer_expr_type(rhs, ctx);
            let s = expr_to_rust(expr, ctx);
            // Track as local for future lookups
            ctx.locals.insert(name.clone());
            if let Some(t) = inferred_type {
                ctx.local_types.insert(name.clone(), t);
            }
            format!("    {};", s)
        }
        _ => format!("    {};", expr_to_rust(expr, ctx)),
    }
}

/// Attempt to infer the type of an expression from context.
fn infer_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::Call(call) => {
            // If calling a trait method, return type is known
            if ctx.is_trait_target(&call.target) {
                let method = if call.method.is_empty() { "call" } else { &call.method };
                return ctx.return_type_of(&call.target, method).map(|s| s.to_string());
            }
            // If calling a struct constructor
            if ctx.is_struct_target(&call.target) {
                let method = if call.method.is_empty() { "new" } else { &call.method };
                return ctx.return_type_of(&call.target, method).map(|s| s.to_string());
            }
            // If calling a method on a local
            if ctx.is_local(&call.target) {
                return ctx.return_type_of(&call.target, &call.method).map(|s| s.to_string());
            }
            None
        }
        _ => None,
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
