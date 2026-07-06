//! Expression translator — converts VEIL AST Expr to Rust source code.
//!
//! Fully shape-driven: the translator uses `GenCtx.name_to_shape` to decide
//! how to emit a Call (port call → deps.x.method().await?, struct call →
//! Type::new(args), local → target.method(args)).

use std::collections::{HashMap, HashSet};

use veil_ir::ast::*;
use veil_ir::layer::{Shape, StmtShape, LayerRegistry};

use crate::rust::to_snake;

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
    /// How to reference the Bus for orchestrator routing. `deps.bus` in a
    /// flow/service; `bus` inside a saga-step impl where it's an injected param.
    pub bus_ref: String,
    /// Names backed by a threaded JSON state (saga steps). A read of such a name
    /// becomes `state["name"]`; an assignment writes `state["name"] = ...`. This
    /// lets independent step impls share results across steps.
    pub state_locals: HashSet<String>,
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
            bus_ref: "deps.bus".to_string(),
            state_locals: HashSet::new(),
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
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_name_simple).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}; {}]", type_name_simple(inner), size),
        TypeExpr::Ref(inner, _) => type_name_simple(inner),
        TypeExpr::Dyn(inner) => format!("dyn {}", type_name_simple(inner)),
        TypeExpr::ImplTrait(inner) => format!("impl {}", type_name_simple(inner)),
        TypeExpr::FnPtr(_, _) => "fn()".to_string(),
    }
}

/// Translate a VEIL expression to a Rust expression string (no trailing semicolon).
pub fn expr_to_rust(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            if ctx.state_locals.contains(name.as_str()) {
                // Shared saga state: read from the threaded JSON state.
                format!("state[\"{}\"]", name)
            } else if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("self.{}", to_snake(name))
            } else {
                name.clone()
            }
        }
        Expr::FieldAccess(base, field) => {
            // A field of a state-local: index into the threaded JSON state.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"]", name, field);
                }
            }
            // In the JSON-bus orchestrator, a field of a Bus-returned local is a
            // JSON index: `result["code"]`. Bus results are serde_json::Value.
            if ctx.is_orchestrator {
                if let Expr::Ident(name) = base.as_ref() {
                    if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                        return format!("{}[\"{}\"]", name, field);
                    }
                }
            }
            let base_str = expr_to_rust(base, ctx);
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
            if ctx.state_locals.contains(name.as_str()) {
                // Write the result into the threaded saga state as JSON.
                format!("state[\"{}\"] = serde_json::json!({})", name, rhs_str)
            } else if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("self.{} = {}", to_snake(name), rhs_str)
            } else if ctx.is_local(name) {
                // Already-declared local (e.g. a `mut` var) → reassignment, no `let`.
                format!("{} = {}", name, rhs_str)
            } else {
                format!("let {} = {}", name, rhs_str)
            }
        }
        Expr::MutAssign(name, rhs) => {
            let rhs_str = expr_to_rust(rhs, ctx);
            format!("let mut {} = {}", name, rhs_str)
        }
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => {
            // `ret Ok` / `ret Err e` construct the Result directly; anything
            // else is the success value and gets wrapped in `Ok(..)`.
            match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => "return Ok(())".to_string(),
                Expr::Ident(n) if n == "Err" => {
                    "return Err(DomainError::External(\"error\".to_string()))".to_string()
                }
                // `ret Err e` parses as a call `Err(e)` or ident chain; handle a
                // call whose target is Err.
                Expr::Call(c) if c.target == "Err" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    format!("return Err({})", if a.is_empty() { "DomainError::External(\"error\".to_string())".to_string() } else { a })
                }
                Expr::Call(c) if c.target == "Ok" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    format!("return Ok({})", if a.is_empty() { "()".to_string() } else { a })
                }
                _ => format!("return Ok({})", expr_to_rust(inner, ctx)),
            }
        }
        Expr::Await(inner) => {
            let inner_str = expr_to_rust(inner, ctx);
            format!("{}.await", inner_str)
        }
        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),
        Expr::Index(base, idx) => format!("{}[{}]", expr_to_rust(base, ctx), expr_to_rust(idx, ctx)),
        Expr::ArrayLit(items) => { let s = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", "); format!("vec![{}]", s) }
        Expr::Range { start, end, inclusive } => { let s = start.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let e = end.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let op = if *inclusive { "..=" } else { ".." }; format!("{}{}{}", s, op, e) }
        Expr::Loop(body) => { let b = body.iter().map(|e| format!("    {};", expr_to_rust(e, ctx))).collect::<Vec<_>>().join("\n"); format!("loop {{\n{}\n}}", b) }
        Expr::Cast(expr, ty) => format!("{} as {}", expr_to_rust(expr, ctx), ty),
        Expr::Try(expr) => format!("{}?", expr_to_rust(expr, ctx)),
        Expr::StructUpdate { name, fields, base } => { let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx))).collect::<Vec<_>>().join(", "); format!("{} {{ {}, ..{} }}", name, fs, expr_to_rust(base, ctx)) }
        Expr::Action(a) => translate_action(a, ctx),
        Expr::StructLit(name, fields) if name.is_empty() => {
            // Anonymous record/map literal (`{}` or `{ key: value, ... }`) → a
            // JSON object value.
            if fields.is_empty() {
                "serde_json::json!({})".to_string()
            } else {
                let pairs = fields.iter().map(|(k, v)| {
                    format!("\"{}\": {}", k, to_json_arg(v, ctx))
                }).collect::<Vec<_>>().join(", ");
                format!("serde_json::json!({{ {} }})", pairs)
            }
        }
        Expr::StructLit(name, fields) => {
            let fs = fields.iter().map(|(k, v)| {
                let v_str = expr_to_rust(v, ctx);
                if k == &v_str { k.clone() } else { format!("{}: {}", to_snake(k), v_str) }
            }).collect::<Vec<_>>().join(", ");
            format!("{} {{ {} }}", name, fs)
        }
        Expr::Match(scrutinee, arms) => {
            // The match consumes the scrutinee's Result directly, so a fallible
            // call scrutinee must NOT auto-propagate with `?`.
            let raw = expr_to_rust(scrutinee, ctx);
            let scrutinee_str = raw.strip_suffix(".await?")
                .map(|s| format!("{}.await", s))
                .unwrap_or_else(|| raw.strip_suffix('?').map(|s| s.to_string()).unwrap_or(raw));
            let mut out = format!("match {} {{\n", scrutinee_str);
            for arm in arms {
                let pattern = normalize_match_pattern(&arm.pattern);
                let body_str = if arm.body.len() == 1 {
                    expr_to_rust(&arm.body[0], ctx)
                } else {
                    let stmts = arm.body.iter()
                        .map(|e| format!("        {};", expr_to_rust(e, ctx)))
                        .collect::<Vec<_>>().join("\n");
                    format!("{{\n{}\n    }}", stmts)
                };
                out.push_str(&format!("        {} => {},\n", pattern, body_str));
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
            // The loop variable is a local within the body. Infer its element
            // type from the iterable so method calls on it resolve (e.g. a
            // `List<SagaStep>` yields `SagaStep` elements).
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.locals.insert(binding.clone());
            if let Some(elem) = element_type_of(iterable, ctx) {
                body_ctx.local_types.insert(binding.clone(), elem);
            }
            if let Some(idx) = index {
                body_ctx.locals.insert(idx.clone());
            }
            let body_str = body.iter()
                .map(|e| format!("        {};", expr_to_rust(e, &body_ctx)))
                .collect::<Vec<_>>().join("\n");
            let enumerate = if index.is_some() { ".enumerate()" } else { "" };
            format!("for {} in {}{} {{\n{}\n    }}", bind, iter_str, enumerate, body_str)
        }
        Expr::WhileLoop { condition, body } => {
            let cond_str = expr_to_rust(condition, ctx);
            let body_str = body.iter()
                .map(|e| format!("        {};", expr_to_rust(e, ctx)))
                .collect::<Vec<_>>().join("\n");
            format!("while {} {{\n{}\n    }}", cond_str, body_str)
        }
        Expr::Tuple(items) => {
            let parts = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        Expr::StringInterp(parts) => {
            use veil_ir::ast::StringPart;
            let mut fmt = String::new();
            let mut args = Vec::new();
            for p in parts {
                match p {
                    StringPart::Literal(l) => fmt.push_str(l),
                    StringPart::Expr(e) => { fmt.push_str("{}"); args.push(expr_to_rust(e, ctx)); }
                }
            }
            if args.is_empty() {
                format!("\"{}\"", fmt)
            } else {
                format!("format!(\"{}\", {})", fmt, args.join(", "))
            }
        }
        Expr::Closure { params, body } => {
            let p = params.join(", ");
            if body.len() == 1 {
                format!("|{}| {}", p, expr_to_rust(&body[0], ctx))
            } else {
                let stmts = body.iter()
                    .map(|e| format!("    {};", expr_to_rust(e, ctx)))
                    .collect::<Vec<_>>().join("\n");
                format!("|{}| {{\n{}\n}}", p, stmts)
            }
        }
    }
}

/// Render an expression for embedding inside a `json!` payload. Values are
/// cloned to avoid moving locals that are reused across bus calls; bare
/// non-local identifiers (e.g. enum variants like `FreeTier`) become JSON
/// strings; field access uses JSON indexing on the serialized base so it works
/// regardless of the (opaque) source type.
fn to_json_arg(expr: &Expr, ctx: &GenCtx) -> String {
    match expr {
        Expr::Ident(name) => {
            // A shared saga-state value → read from the threaded state.
            if ctx.state_locals.contains(name.as_str()) {
                format!("state[\"{}\"].clone()", name)
            } else if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                // A struct-captured input (saga step) → self.<field>.
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
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return format!("state[\"{}\"][\"{}\"].clone()", name, field);
                }
            }
            // If the base is already a serde_json::Value local, index it directly.
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return format!("{}[\"{}\"].clone()", name, field);
                }
            }
            // Otherwise serialize the base then index (works for opaque stub types;
            // Index yields Null on mismatch rather than panicking).
            format!("serde_json::json!({})[\"{}\"].clone()", to_json_arg(base, ctx), field)
        }
        _ => expr_to_rust(expr, ctx),
    }
}

/// Determine the call suffix for a method invoked on a chained receiver.
/// If the method is declared on any known trait (async_trait + Result), it
/// needs `.await?`; otherwise no suffix (e.g. iterator adapters).
fn receiver_call_suffix(_recv: &Expr, method: &str, ctx: &GenCtx) -> String {
    let is_trait_method = ctx
        .method_returns
        .keys()
        .any(|(ty, m)| m == method && ctx.name_to_shape.get(ty) == Some(&Shape::Trait));
    if is_trait_method { ".await?".to_string() } else { String::new() }
}

/// Build a `serde_json::json!` object for a message (event/command) with a
/// `"type"` tag plus its named fields — the wire form for a JSON Bus payload.
fn json_message(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> String {
    let mut parts = vec![format!("\"type\": \"{}\"", name)];
    for (k, v) in fields {
        parts.push(format!("\"{}\": {}", k, to_json_arg(v, ctx)));
    }
    format!("serde_json::json!({{ {} }})", parts.join(", "))
}

/// Build a JSON envelope for a cross-context call routed through the Bus:
/// `{ "target": T, "method": m, "args": [ ... ] }`. Positional args are
/// rendered as JSON values so the receiving context can decode them.
fn json_envelope(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> String {
    let arg_vals = args.iter().map(|a| to_json_arg(a, ctx)).collect::<Vec<_>>().join(", ");
    format!(
        "serde_json::json!({{ \"target\": \"{}\", \"method\": \"{}\", \"args\": [{}] }})",
        target, method, arg_vals
    )
}

/// Render call args, cloning value-bearing locals/state so passing them into a
/// by-value parameter doesn't move them out of the caller. Skips the Bus
/// reference and Copy scalars (which don't move).
fn clone_args(args: &[Expr], ctx: &GenCtx) -> String {
    args.iter()
        .map(|a| match a {
            Expr::Ident(n) if ctx.state_locals.contains(n.as_str()) => format!("state[\"{}\"].clone()", n),
            // The bus reference (bus_ref) and Copy scalars are passed as-is.
            Expr::Ident(n) if *n == ctx.bus_ref => n.clone(),
            Expr::Ident(n) if is_copy_local(n, ctx) => n.clone(),
            Expr::Ident(n) if ctx.is_local(n) => format!("{}.clone()", n),
            _ => expr_to_rust(a, ctx),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A local whose inferred type is a Copy scalar (int/bool/float) — no clone.
fn is_copy_local(name: &str, ctx: &GenCtx) -> bool {
    matches!(
        ctx.local_type(name),
        Some("i64") | Some("i32") | Some("u64") | Some("u32")
            | Some("usize") | Some("isize") | Some("f64") | Some("f32") | Some("bool")
    )
}

/// Translate a Call expression with shape-aware name resolution.
fn translate_call(call: &CallExpr, ctx: &GenCtx) -> String {
    let args_str = call.args.iter()
        .map(|a| expr_to_rust(a, ctx))
        .collect::<Vec<_>>().join(", ");

    // Built-in List methods: `.get(i)` → indexing (`[i as usize]`), `.len()` →
    // `.len() as i64`. The receiver/target is the list expression.
    let list_base = if let Some(recv) = &call.receiver {
        Some(expr_to_rust(recv, ctx))
    } else if !call.target.is_empty() && call.method == "get" || (!call.target.is_empty() && call.method == "len") {
        Some(call.target.clone())
    } else {
        None
    };
    if let Some(base) = list_base {
        if call.method == "get" && call.args.len() == 1 {
            let idx = expr_to_rust(&call.args[0], ctx);
            return format!("{}[({}) as usize]", base, idx);
        }
        if call.method == "len" && call.args.is_empty() {
            return format!("({}.len() as i64)", base);
        }
    }

    // Chained method call: `<receiver>.method(args)` (e.g. `.collect()` in
    // `items.map(f).collect()`). The receiver carries the left side of the chain.
    if let Some(recv) = &call.receiver {
        let recv_str = expr_to_rust(recv, ctx);
        // A trait method invoked on a chained receiver is async + fallible.
        let suffix = receiver_call_suffix(recv, &call.method, ctx);
        return format!("{}.{}({}){}", recv_str, to_snake(&call.method), clone_args(&call.args, ctx), suffix);
    }

    // Trait-shaped target → deps.target.method(args).await?
    if ctx.is_trait_target(&call.target) {
        let dep_name = to_snake(&call.target);
        let method = if call.method.is_empty() { "call" } else { &call.method };
        // Desugared bus calls (dispatch/invoke/request) carry a StructLit
        // event/command; build a JSON payload tagged with its type.
        let final_args = if call.sugar.is_some() {
            match call.args.first() {
                Some(Expr::StructLit(name, fields)) => json_message(name, fields, ctx),
                Some(Expr::Ident(evt)) => format!("serde_json::json!({{ \"type\": \"{}\" }})", evt),
                _ => json_envelope(&call.target, method, &call.args, ctx),
            }
        } else {
            // Direct Bus call — clone args to avoid move issues.
            call.args.iter()
                .map(|a| {
                    let s = expr_to_rust(a, ctx);
                    match a {
                        Expr::Ident(_) | Expr::FieldAccess(_, _) => format!("{}.clone()", s),
                        _ => s,
                    }
                })
                .collect::<Vec<_>>().join(", ")
        };
        // The Bus itself uses the ctx bus reference (`deps.bus` in a flow,
        // `bus` inside a saga-step impl); other trait deps come from `deps`.
        if call.target == "Bus" {
            return format!("{}.{}({}).await?", ctx.bus_ref, to_snake(method), final_args);
        }
        return format!("deps.{}.{}({}).await?", dep_name, to_snake(method), final_args);
    }

    // In an orchestrator, ANY call to another context (struct construction,
    // aggregate method, or repo/port) is routed through the JSON Bus with a
    // typed envelope — the orchestrator crate can't see the other context's
    // concrete types.
    if ctx.is_orchestrator && (ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty()) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        return format!(
            "{}.invoke({}).await?",
            ctx.bus_ref,
            json_envelope(&call.target, method, &call.args, ctx)
        );
    }

    // Struct-shaped target with method "new" or empty → Type::new(args)
    if ctx.is_struct_target(&call.target) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
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
        if let Some(type_name) = ctx.local_type(&call.target) {
            // If the local's type is a known trait, its methods are async and
            // fallible (`#[async_trait]` + `-> Result`): emit `.await?`.
            if ctx.name_to_shape.get(type_name) == Some(&Shape::Trait) {
                return format!("{}.{}({}).await?", call.target, method, args_str);
            }
            // Known concrete method (e.g. aggregate fn) — call with ?
            if ctx.method_returns.contains_key(&(type_name.to_string(), call.method.clone())) {
                return format!("{}.{}({})?", call.target, method, args_str);
            }
        }
        // Unknown method on local — call without ? (might be a field accessor or infallible)
        return format!("{}.{}({})", call.target, method, args_str);
    }
    if call.method.is_empty() {
        // Bare call: now() → Utc::now(), others → as-is (cloning value args so
        // passing locals/state into a by-value param doesn't move them).
        match call.target.as_str() {
            "now" => "Utc::now()".to_string(),
            _ => format!("{}({})", to_snake(&call.target), clone_args(&call.args, ctx)),
        }
    } else if ctx.is_local(&call.target) || ctx.name_to_shape.contains_key(&call.target) {
        // Known local/construct method call (already handled above, but be safe).
        format!("{}.{}({})", call.target, to_snake(&call.method), args_str)
    } else {
        // Unknown target with a method (e.g. `http.post(...)`): an external
        // effect. Route it to a generated runtime hook `<target>_<method>(...)`
        // so the code compiles without inventing domain knowledge. The set of
        // hooks is emitted at the bottom of the module.
        format!("{}_{}({})", to_snake(&call.target), to_snake(&call.method), args_str)
    }
}

/// Translate a layer-defined Action that was NOT desugared (e.g. emit, guard).
fn translate_action(a: &ActionExpr, ctx: &GenCtx) -> String {
    match a.shape {
        StmtShape::If => {
            // guard: the condition must hold for the flow to continue.
            let msg = a.message.as_deref().unwrap_or("precondition failed");
            let msg_escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
            match a.condition.as_deref() {
                // Fallible-call guard (`guard call X.method(...)`): the call
                // returns a Result that must be Ok — propagate its error as a
                // domain validation error.
                Some(cond @ Expr::Call(_)) | Some(cond @ Expr::Await(_)) => {
                    let call_str = expr_to_rust(cond, ctx);
                    // translate_call may already append `?`; strip it so our
                    // map_err drives the propagation.
                    let base = call_str.strip_suffix('?').unwrap_or(&call_str);
                    format!(
                        "{}.map_err(|_| DomainError::Validation(\"{}\".to_string()))?",
                        base, msg_escaped
                    )
                }
                // Boolean guard: the condition must evaluate to true.
                Some(cond) => {
                    let cond_str = expr_to_rust(cond, ctx);
                    format!(
                        "if !({}) {{ return Err(DomainError::Validation(\"{}\".to_string())); }}",
                        cond_str, msg_escaped
                    )
                }
                None => format!("/* guard: {} (no condition) */", msg_escaped),
            }
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
        Expr::Assign(name, rhs) | Expr::MutAssign(name, rhs) => {
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

impl GenCtx {
    /// A shallow clone carrying just the maps needed for type inference (used
    /// by the return-type pre-scan in rust.rs).
    pub fn clone_for_inference(&self) -> GenCtx {
        GenCtx {
            name_to_shape: self.name_to_shape.clone(),
            locals: self.locals.clone(),
            self_fields: self.self_fields.clone(),
            in_aggregate_fn: self.in_aggregate_fn,
            is_orchestrator: self.is_orchestrator,
            method_returns: self.method_returns.clone(),
            local_types: self.local_types.clone(),
            struct_fields: self.struct_fields.clone(),
            bus_ref: self.bus_ref.clone(),
            state_locals: self.state_locals.clone(),
        }
    }
}

/// Public wrapper for `infer_expr_type`, for the return-type pre-scan.
pub fn infer_expr_type_pub(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    infer_expr_type(expr, ctx)
}

/// Infer the element type of an iterable expression. If it's a local whose
/// tracked type is `Vec<T>` (or a boxed-trait vec), return the inner `T`
/// (unwrapping `Box<dyn T ..>` to `T`) so method calls on the loop var resolve.
fn element_type_of(iterable: &Expr, ctx: &GenCtx) -> Option<String> {
    if let Expr::Ident(name) = iterable {
        if let Some(t) = ctx.local_type(name) {
            let inner = t.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>'))?;
            let inner = inner.trim();
            // Unwrap Box<dyn Trait + Send + Sync> → Trait.
            if let Some(rest) = inner.strip_prefix("Box<dyn ") {
                let name = rest.split([' ', '+', '>']).next().unwrap_or(rest);
                return Some(name.to_string());
            }
            return Some(inner.to_string());
        }
    }
    None
}

/// Infer the Rust type of a flow's return expression (`ret <expr>`).
/// Resolves idents and field access against known local/struct-field types.
pub fn infer_return_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::IntLit(_) => Some("i64".to_string()),
        Expr::FloatLit(_) => Some("f64".to_string()),
        Expr::BoolLit(_) => Some("bool".to_string()),
        Expr::StringLit(_) | Expr::StringInterp(_) => Some("String".to_string()),
        Expr::Ident(name) => ctx.local_type(name).map(|s| s.to_string()),
        Expr::FieldAccess(base, field) => {
            // Resolve the base's type, then the field's declared type.
            if let Expr::Ident(name) = base.as_ref() {
                if let Some(type_name) = ctx.local_type(name) {
                    if type_name == "serde_json::Value" {
                        // Orchestrator: JSON index — type is Value.
                        return Some("serde_json::Value".to_string());
                    }
                    if let Some(ft) = ctx.field_type(type_name, field) {
                        return Some(rust_type_for_named(ft));
                    }
                }
            }
            None
        }
        Expr::Call(_) => infer_expr_type(expr, ctx),
        _ => None,
    }
}

/// Normalize a VEIL match pattern into Rust form. VEIL writes `Ok _` / `Err e`
/// (space-separated binding); Rust needs `Ok(_)` / `Err(e)`. A bare word or
/// already-parenthesized pattern is left as-is.
fn normalize_match_pattern(pattern: &str) -> String {
    let p = pattern.trim();
    // Enum-variant-with-binding: `Variant binding` → `Variant(binding)`.
    if let Some((head, rest)) = p.split_once(char::is_whitespace) {
        let rest = rest.trim();
        if !rest.is_empty() && !rest.starts_with('(') {
            // Only treat capitalized heads as variants (Ok, Err, Some, custom).
            if head.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                return format!("{}({})", head, rest);
            }
        }
    }
    p.to_string()
}

/// Map a VEIL simple type name (as stored in struct_fields) to its Rust form.
fn rust_type_for_named(name: &str) -> String {
    match name {
        "Str" => "String".to_string(),
        "Int" => "i64".to_string(),
        "F64" => "f64".to_string(),
        "Bool" => "bool".to_string(),
        "UUID" => "Uuid".to_string(),
        "DateTime" => "DateTime<Utc>".to_string(),
        "Json" => "serde_json::Value".to_string(),
        other => other.to_string(),
    }
}

/// Attempt to infer the type of an expression from context.
fn infer_expr_type(expr: &Expr, ctx: &GenCtx) -> Option<String> {
    match expr {
        Expr::Call(call) => {
            // In an orchestrator, cross-context calls route through the JSON Bus
            // and yield `serde_json::Value` (unless the target is a direct dep).
            if ctx.is_orchestrator && call.receiver.is_none() && !ctx.is_trait_target(&call.target) {
                if ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty() {
                    return Some("serde_json::Value".to_string());
                }
            }
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
            if let Some(recv) = &call.receiver {
                collect_deps_from_expr(recv, ctx, deps);
            }
            for arg in &call.args {
                collect_deps_from_expr(arg, ctx, deps);
            }
        }
        Expr::Assign(_, rhs) | Expr::MutAssign(_, rhs) => collect_deps_from_expr(rhs, ctx, deps),
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
