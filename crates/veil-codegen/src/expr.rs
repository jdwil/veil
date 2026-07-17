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
    /// Names of traits used as message-routing ports (e.g. "Bus"). Calls to these
    /// use `bus_ref` instead of `deps.<name>`. Derived from the layer registry.
    pub routing_traits: HashSet<String>,
    /// Names of known async free functions (e.g. layer-declared coordinators like
    /// `run_saga`, `unwind`). Calls to these need `.await?`.
    pub async_fns: HashSet<String>,
    /// Names backed by a threaded JSON state (saga steps). A read of such a name
    /// becomes `state["name"]`; an assignment writes `state["name"] = ...`. This
    /// lets independent step impls share results across steps.
    pub state_locals: HashSet<String>,
    /// Maps stub struct names to (crate_name, original_type_name) so codegen
    /// generates qualified paths like `aws_sdk_s3::Client::new()` when VEIL
    /// writes `S3Client.new()` (aliased) or `Client.new()` (unaliased).
    pub stub_type_crate: HashMap<String, (String, String)>,
    /// Stub free-fn constructors: type name → typed free-fn name + type-param template.
    /// From stub struct metadata `typed_variant` / `typed_type_params` (e.g. query_as).
    pub stub_typed_ctors: HashMap<String, (String, String)>,
    /// Methods whose stub return type is `Res!` / fallible (e.g. builder `send`).
    pub fallible_methods: HashSet<String>,
    /// Methods whose stub return type is async AND fallible (e.g. `BoxFuture<Res!<...>>`
    /// or declared with `Res!` on a struct that acts as an executor).
    /// These get `.await.map_err(...)? ` instead of just `?`.
    pub async_fallible_methods: HashSet<String>,
    /// Expected Rust return type of the enclosing fn (e.g. `Result<Option<T>, DomainError>`).
    /// Used to wrap `ret x` as `Ok(Some(x))` when returning Option.
    pub expected_return_rust: Option<String>,
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
            routing_traits: HashSet::new(),
            async_fns: HashSet::new(),
            state_locals: HashSet::new(),
            stub_type_crate: HashMap::new(),
            stub_typed_ctors: HashMap::new(),
            fallible_methods: HashSet::new(),
            async_fallible_methods: HashSet::new(),
            expected_return_rust: None,
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
    /// Call-site method names may include bang/query suffixes (`find!`); keys are
    /// stored without them (signature name is `find`, bang only wraps return type).
    pub fn return_type_of(&self, target: &str, method: &str) -> Option<&str> {
        let method_key = method.trim_end_matches(['!', '?']);
        let keys = [
            method.to_string(),
            method_key.to_string(),
            format!("{method_key}!"),
        ];
        for m in &keys {
            if let Some(t) = self.method_returns.get(&(target.to_string(), m.clone())) {
                return Some(t.as_str());
            }
        }
        // If target is a local, look up its type and check struct methods
        if let Some(type_name) = self.local_types.get(target) {
            for m in &keys {
                if let Some(t) = self
                    .method_returns
                    .get(&(type_name.clone(), m.clone()))
                {
                    return Some(t.as_str());
                }
            }
        }
        // Dep local registered as Trait shape but not in local_types — try snake_case target
        // (already covered by direct key) and PascalCase conversion is not needed.
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
                // Register under PascalCase trait name (e.g. "CohortRepo", "find")
                ctx.method_returns.insert(
                    (c.name.clone(), method.name.clone()),
                    ret_type.clone(),
                );
                // Also register under snake_case dep name (e.g. "cohort_repo", "find")
                // so lookups from @dep variable names resolve without conversion
                ctx.method_returns.insert(
                    (to_snake(&c.name), method.name.clone()),
                    ret_type.clone(),
                );
                // Type aliases (WearTestRepo = EntityRepo<WearTest>) share methods —
                // also register under any alias that monomorphizes this trait.
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

            // Record struct constructors: Type.new → Type (or Result<Type> for invariant types)
            let has_invariant = c.annotations.iter().any(|a| a.name == "invariant");
            let new_ret = if has_invariant {
                format!("Result<{}>", c.name)
            } else {
                c.name.clone()
            };
            ctx.method_returns.insert(
                (c.name.clone(), "new".to_string()),
                new_ret,
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

    // Type aliases like `type WearTestRepo = EntityRepo<WearTest>` are ports
    // for call resolution (deps.wear_test_repo) and share method return types
    // with the generic base trait (find → Option after monomorphize).
    for item in &solution.items {
        if let TopLevelItem::TypeAlias { name, target } = item {
            if let TypeExpr::Generic(base, args) = target {
                ctx.name_to_shape.insert(name.clone(), Shape::Trait);
                // Copy method_returns from base trait, monomorphizing T → arg.
                let entity = args
                    .first()
                    .map(|a| match a {
                        TypeExpr::Named(n) => n.clone(),
                        _ => "T".into(),
                    })
                    .unwrap_or_else(|| "T".into());
                let base_keys: Vec<_> = ctx
                    .method_returns
                    .keys()
                    .filter(|(t, _)| t == base || t == &to_snake(base))
                    .cloned()
                    .collect();
                for (t, method) in base_keys {
                    if let Some(ret) = ctx.method_returns.get(&(t, method.clone())).cloned() {
                        // Option<T> → Option<WearTest>
                        let mono = ret.replace("<T>", &format!("<{entity}>")).replace(
                            "Option<T>",
                            &format!("Option<{entity}>"),
                        );
                        let mono = if mono == "T" {
                            entity.clone()
                        } else {
                            mono
                        };
                        ctx.method_returns
                            .insert((name.clone(), method.clone()), mono.clone());
                        ctx.method_returns
                            .insert((to_snake(name), method), mono);
                    }
                }
            } else if let TypeExpr::Named(base) = target {
                ctx.name_to_shape.insert(name.clone(), Shape::Trait);
                let base_keys: Vec<_> = ctx
                    .method_returns
                    .keys()
                    .filter(|(t, _)| t == base || t == &to_snake(base))
                    .cloned()
                    .collect();
                for (t, method) in base_keys {
                    if let Some(ret) = ctx.method_returns.get(&(t, method.clone())).cloned() {
                        ctx.method_returns
                            .insert((name.clone(), method.clone()), ret.clone());
                        ctx.method_returns
                            .insert((to_snake(name), method), ret);
                    }
                }
            }
        }
    }

    // Register stub crate type information
    for stub in &registry.stubs {
        for s in &stub.structs {
            // Compute the aliased name for this type
            let type_name = if let Some(alias) = &stub.alias {
                let cap_alias = alias.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() + &alias[1..];
                format!("{}{}", cap_alias, s.name)
            } else {
                s.name.clone()
            };
            // Register struct methods under the aliased name
            for method in &s.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<")
                    || ret.contains("Res!");
                let is_async_fallible = ret.contains("BoxFuture") && ret.contains("Res!")
                    || (fallible && method.params.iter().any(|p| {
                        // Methods taking an executor param (e.g. `executor: E`) are async
                        p.0 == "executor" || p.0 == "pool"
                    }));
                if fallible {
                    ctx.fallible_methods.insert(method.name.clone());
                }
                if is_async_fallible {
                    ctx.async_fallible_methods.insert(method.name.clone());
                }
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.method_returns.insert(
                    (type_name.clone(), method.name.clone()),
                    inner.to_string(),
                );
            }
            // Register as a known struct with qualified crate path from stub
            // metadata (`types_module` / `root_types`) — never crate-family hardcoding.
            ctx.name_to_shape.insert(type_name.clone(), Shape::Struct);
            let crate_name = stub.name.replace('-', "_");
            let path_type = stub.rust_type_path(&s.name);
            ctx.stub_type_crate
                .insert(type_name.clone(), (crate_name, path_type));
            // Typed free-fn constructor (e.g. Query → query_as) from stub metadata.
            if let Some(ref typed_fn) = s.typed_variant {
                let params = s
                    .typed_type_params
                    .clone()
                    .unwrap_or_else(|| "_, return_type".into());
                ctx.stub_typed_ctors
                    .insert(type_name, (typed_fn.clone(), params.clone()));
                // Also key by bare stub type name (unaliased)
                ctx.stub_typed_ctors
                    .insert(s.name.clone(), (typed_fn.clone(), params));
            }
        }
        for i in &stub.impls {
            for method in &i.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<");
                if fallible {
                    ctx.fallible_methods.insert(method.name.clone());
                }
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

    // Populate routing traits from the layer registry so call generation can
    // identify which traits are message-routing ports (e.g. Bus) without
    // hardcoding names.
    ctx.routing_traits = registry.routing_traits().into_iter().collect();

    // Track layer-declared free functions (e.g. run_saga, unwind) as async —
    // they generate as `pub async fn` and calls to them need `.await?`.
    for item in &solution.items {
        if let TopLevelItem::Function(f) = item {
            ctx.async_fns.insert(f.name.clone());
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
/// Extract the inner domain struct type from a return type string.
/// e.g., `Result<Option<Tenant>, DomainError>` → Some("Tenant")
/// e.g., `Result<Vec<Cohort>, DomainError>` → Some("Cohort")
/// Only returns Some when the extracted type is a known struct in name_to_shape
/// AND all its fields are primitive types that a DB row can decode directly.
fn extract_domain_type_from_return(
    ret: &str,
    name_to_shape: &HashMap<String, Shape>,
) -> Option<String> {
    // Strip Result<..., DomainError> wrapper
    let inner = ret
        .strip_prefix("Result<")
        .and_then(|s| s.rsplit_once(", DomainError>"))
        .map(|(inner, _)| inner)
        .unwrap_or(ret);
    // Strip Option<...> / Vec<...>
    let type_name = inner
        .strip_prefix("Option<").and_then(|s| s.strip_suffix('>'))
        .or_else(|| inner.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')))
        .unwrap_or(inner);
    // Check if it's a known struct
    if name_to_shape.get(type_name) == Some(&Shape::Struct) {
        Some(type_name.to_string())
    } else {
        None
    }
}

/// Expand stub `typed_type_params` template (`_, return_type` → `_, CohortDTO`).
fn expand_typed_type_params(template: &str, domain_type: &str) -> String {
    template
        .split(',')
        .map(|p| {
            let t = p.trim();
            if t == "return_type" || t == "$ret" {
                domain_type.to_string()
            } else {
                t.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

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
            // VEIL null → Rust None
            if name == "null" {
                return "None".to_string();
            }
            if ctx.state_locals.contains(name.as_str()) {
                // Shared saga state: read from the threaded JSON state.
                format!("state[\"{}\"]", name)
            } else if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("&self.{}", to_snake(name))
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
                // Adapter / aggregate: `self.table` → clone so `&self` methods compile.
                if name == "self" && ctx.in_aggregate_fn {
                    let f = to_snake(field);
                    if ctx.self_fields.contains(field.as_str())
                        || ctx.self_fields.contains(&f)
                    {
                        return format!("self.{}.clone()", f);
                    }
                    return format!("self.{}", f);
                }
                // Enum variant access: EnumName.Variant → EnumName::Variant
                if matches!(ctx.name_to_shape.get(name.as_str()), Some(Shape::Enum)) {
                    // Capitalize the variant name (field might be snake_case from parser)
                    let variant = field.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
                        + &field[1..];
                    return format!("{}::{}", name, variant);
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
            // Special case: x != None → x.is_some(), x == None → x.is_none()
            if r == "None" {
                return match op.op {
                    veil_ir::ast::BinOp::NotEq => format!("{}.is_some()", l),
                    veil_ir::ast::BinOp::Eq => format!("{}.is_none()", l),
                    _ => format!("{} {} {}", l, binop_to_rust(&op.op), r),
                };
            }
            if l == "None" {
                return match op.op {
                    veil_ir::ast::BinOp::NotEq => format!("{}.is_some()", r),
                    veil_ir::ast::BinOp::Eq => format!("{}.is_none()", r),
                    _ => format!("{} {} {}", l, binop_to_rust(&op.op), r),
                };
            }
            // List append: `out + [x]` / `out + vec` → extend into owned Vec
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (r.starts_with("vec![") || l.starts_with("vec!["))
            {
                return format!(
                    "{{ let mut __v = {l}; __v.extend({r}); __v }}"
                );
            }
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
        Expr::Assign(name, rhs, ty_ann) => {
            let rhs_str = expr_to_rust(rhs, ctx);
            // Field assignment: `wt.name = x` stored as Assign("wt.name", …)
            // Emit path with snake_case fields; never introduce a `let` binding.
            if name.contains('.') {
                let path = name
                    .split('.')
                    .enumerate()
                    .map(|(i, seg)| {
                        if i == 0 {
                            seg.to_string()
                        } else {
                            to_snake(seg)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                return format!("{} = {}", path, rhs_str);
            }
            if ctx.state_locals.contains(name.as_str()) {
                // Write the result into the threaded saga state as JSON.
                format!("state[\"{}\"] = serde_json::json!({})", name, rhs_str)
            } else if ctx.in_aggregate_fn && ctx.self_fields.contains(name.as_str()) {
                format!("self.{} = {}", to_snake(name), rhs_str)
            } else if ctx.is_local(name) {
                // Already-declared local (e.g. a `mut` var) → reassignment, no `let`.
                format!("{} = {}", name, rhs_str)
            } else if let Some(ty) = ty_ann {
                format!("let {}: {} = {}", name, crate::rust::type_to_rust(ty), rhs_str)
            } else {
                format!("let mut {} = {}", name, rhs_str)
            }
        }
        Expr::MutAssign(name, rhs, ty_ann) => {
            let rhs_str = expr_to_rust(rhs, ctx);
            match ty_ann {
                Some(ty) => format!("let mut {}: {} = {}", name, crate::rust::type_to_rust(ty), rhs_str),
                None => format!("let mut {} = {}", name, rhs_str),
            }
        }
        Expr::StringLit(s) => format!("\"{}\".to_string()", s),
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
                    if a.is_empty() {
                        "return Err(DomainError::Validation(\"error\".to_string()))".to_string()
                    } else if a.starts_with("DomainError::") {
                        // Already a DomainError variant
                        format!("return Err({})", a)
                    } else {
                        // Check if the argument is a simple identifier (likely a caught error variable)
                        let is_simple_ident = c.args.len() == 1 && matches!(&c.args[0], Expr::Ident(_));
                        if is_simple_ident {
                            // Bare variable from a match arm — likely already DomainError
                            format!("return Err({})", a)
                        } else {
                            // String expression — wrap in DomainError::Validation
                            format!("return Err(DomainError::Validation({}))", a)
                        }
                    }
                }
                Expr::Call(c) if c.target == "Ok" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    format!("return Ok({})", if a.is_empty() { "()".to_string() } else { a })
                }
                _ => {
                    let val = expr_to_rust(inner, ctx);
                    // `ret null` → Ok(None); `ret x` into Result<Option<T>> → Ok(Some(x))
                    let returns_option = ctx
                        .expected_return_rust
                        .as_deref()
                        .map(|t| t.contains("Option<"))
                        .unwrap_or(false);
                    if returns_option && val != "None" && !val.starts_with("Some(") {
                        format!("return Ok(Some({}))", val)
                    } else {
                        format!("return Ok({})", val)
                    }
                }
            }
        }
        Expr::Await(inner) => {
            let inner_str = expr_to_rust(inner, ctx);
            format!("{}.await", inner_str)
        }
        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),
        Expr::Index(base, idx) => {
            let b = expr_to_rust(base, ctx);
            // HashMap / Dynamo item: `.get("key").cloned().ok_or(NotFound)?`
            // so subsequent `.as_s()` is on AttributeValue, not Option.
            match idx.as_ref() {
                Expr::StringLit(s) => format!(
                    "{b}.get(\"{s}\").cloned().ok_or(DomainError::NotFound)?"
                ),
                other => {
                    let i = expr_to_rust(other, ctx);
                    format!("{b}[{i}]")
                }
            }
        }
        Expr::ArrayLit(items) => { let s = items.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", "); format!("vec![{}]", s) }
        Expr::Range { start, end, inclusive } => { let s = start.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let e = end.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default(); let op = if *inclusive { "..=" } else { ".." }; format!("{}{}{}", s, op, e) }
        Expr::Loop(body) => { let b = body.iter().map(|e| format!("    {};", expr_to_rust(e, ctx))).collect::<Vec<_>>().join("\n"); format!("loop {{\n{}\n}}", b) }
        Expr::Cast(expr, ty) => format!("{} as {}", expr_to_rust(expr, ctx), ty),
        Expr::Try(expr) => format!("{}?", expr_to_rust(expr, ctx)),
        Expr::StructUpdate { name, fields, base } => { let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx))).collect::<Vec<_>>().join(", "); format!("{} {{ {}, ..{} }}", name, fs, expr_to_rust(base, ctx)) }
        Expr::IfLet { pattern, expr, then_body, else_body } => {
            let e = expr_to_rust(expr, ctx);
            let then_str = then_body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            let else_str = else_body.as_ref().map(|eb| { let s = eb.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n"); format!(" else {{\n{}\n}}", s) }).unwrap_or_default();
            format!("if let {} = {} {{\n{}\n}}{}", pattern, e, then_str, else_str)
        }
        Expr::WhileLet { pattern, expr, body } => {
            let e = expr_to_rust(expr, ctx);
            let body_str = body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            format!("while let {} = {} {{\n{}\n}}", pattern, e, body_str)
        }
        Expr::LetPattern(pattern, expr, ty_ann) => {
            let pat_str = pattern_to_rust(pattern);
            let e = expr_to_rust(expr, ctx);
            match ty_ann {
                Some(ty) => format!("let {}: {} = {}", pat_str, crate::rust::type_to_rust(ty), e),
                None => format!("let {} = {}", pat_str, e),
            }
        }
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
                // Clone ident values to prevent move issues when same var used in multiple fields
                let cloned = match v {
                    Expr::Ident(_) => format!("{}.clone()", v_str),
                    _ => v_str.clone(),
                };
                if k == &v_str { format!("{}: {}.clone()", k, k) } else { format!("{}: {}", to_snake(k), cloned) }
            }).collect::<Vec<_>>().join(", ");
            format!("{} {{ {} }}", name, fs)
        }
        Expr::Match(scrutinee, arms) => {
            // The match consumes the scrutinee's Result directly, so a fallible
            // call scrutinee must NOT auto-propagate with `?`.
            let raw = expr_to_rust(scrutinee, ctx);
            let scrutinee_str = raw
                .strip_suffix(".await.map_err(|e| DomainError::External(e.to_string()))?")
                .map(|s| format!("{}.await", s))
                .or_else(|| {
                    raw.strip_suffix(".await?")
                        .map(|s| format!("{}.await", s))
                })
                .unwrap_or_else(|| {
                    raw.strip_suffix('?')
                        .map(|s| s.to_string())
                        .unwrap_or(raw)
                });
            // If arms contain string literal patterns, add .as_str() for String scrutinees
            let has_string_patterns = arms.iter().any(|a| a.pattern.starts_with('"'));
            let scrutinee_final = if has_string_patterns {
                format!("{}.as_str()", scrutinee_str)
            } else {
                scrutinee_str
            };
            let mut out = format!("match {} {{\n", scrutinee_final);
            for arm in arms {
                // Use structured pattern if available, fall back to string normalization
                let pattern = if let Some(rich) = &arm.rich_pattern {
                    pattern_to_rust(rich)
                } else {
                    normalize_match_pattern(&arm.pattern)
                };
                let guard_str = match &arm.guard {
                    Some(g) => format!(" if {}", expr_to_rust(g, ctx)),
                    None => String::new(),
                };
                // Match arm bodies get their own local set (bindings + assigns).
                let mut arm_ctx = ctx.clone_for_inference();
                // Bind pattern idents as locals (Some(item) → item)
                for name in pattern_binding_names(&arm.pattern) {
                    arm_ctx.locals.insert(name);
                }
                let body_str = if arm.body.len() == 1 {
                    expr_to_rust(&arm.body[0], &arm_ctx)
                } else {
                    let mut stmts = Vec::new();
                    for e in &arm.body {
                        let line = expr_to_rust(e, &arm_ctx);
                        if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = e {
                            if !name.contains('.') {
                                arm_ctx.locals.insert(name.clone());
                            }
                        }
                        stmts.push(format!("        {};", line));
                    }
                    format!("{{\n{}\n    }}", stmts.join("\n"))
                };
                out.push_str(&format!("        {}{} => {},\n", pattern, guard_str, body_str));
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
            let mut body_lines = Vec::new();
            for e in body {
                let line = expr_to_rust(e, &body_ctx);
                if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = e {
                    if !name.contains('.') {
                        body_ctx.locals.insert(name.clone());
                    }
                }
                body_lines.push(format!("        {};", line));
            }
            let body_str = body_lines.join("\n");
            let enumerate = if index.is_some() { ".enumerate()" } else { "" };
            // If the iterable type is Option<_>, unwrap to empty default; else as-is.
            let iter_expr = if let Expr::Ident(name) = iterable.as_ref() {
                if ctx
                    .local_type(name)
                    .map(|t| t.starts_with("Option<"))
                    .unwrap_or(false)
                {
                    format!("{iter_str}.unwrap_or_default()")
                } else {
                    iter_str
                }
            } else {
                iter_str
            };
            format!("for {} in {}{} {{\n{}\n    }}", bind, iter_expr, enumerate, body_str)
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
        // Expanded by adapt merge before codegen — should never remain.
        Expr::Stock => {
            "/* error: stock not expanded */ ()".to_string()
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
///
/// - Fluent `.send()` / `.send_with()` are async + Result → `.await?`
/// - Stub methods marked async+fallible (BoxFuture / executor param) → `.await.map_err…?`
/// - Other stub methods marked `Res!` are sync Result → `map_err…?`
/// - Trait methods (ports) are async_trait + Result → `.await?`
///
/// **Receiver type wins over bare method name.** Port methods and stub methods
/// often share names (`get_version`, `list_versions`, `package_root`, …). Looking
/// only at the method name caused `.await?` on sync stub facades (ExtStore,
/// LocalFs) — the permanent adapter/stub-lowering bug. Always prefer the
/// receiver's Shape::Struct vs Shape::Trait when known.
///
/// Method names may carry VEIL bang/query suffixes (`fetch_all!`); strip before lookup.
fn receiver_call_suffix(recv: &Expr, method: &str, ctx: &GenCtx) -> String {
    let method = method.trim_end_matches(['!', '?']);

    // Resolve the static type of the receiver when we can (UFCS / local / self field).
    let recv_type_name: Option<String> = match recv {
        Expr::Ident(name) => {
            if ctx.is_struct_target(name) || ctx.is_trait_target(name) {
                Some(name.clone())
            } else if let Some(t) = ctx.local_type(name) {
                Some(t.to_string())
            } else if ctx.stub_type_crate.contains_key(name) {
                Some(name.clone())
            } else {
                None
            }
        }
        _ => None,
    };

    // Known struct / stub type → never treat as port trait (even if method name
    // collides with ExtensionRegistry.get_version etc.).
    if let Some(ref ty) = recv_type_name {
        if ctx.name_to_shape.get(ty.as_str()) == Some(&Shape::Struct)
            || ctx.stub_type_crate.contains_key(ty.as_str())
        {
            if method == "send"
                || method == "send_with"
                || ctx.async_fallible_methods.contains(method)
            {
                return ".await.map_err(|e| DomainError::External(e.to_string()))?".to_string();
            }
            if ctx.fallible_methods.contains(method) {
                return ".map_err(|e| DomainError::External(e.to_string()))?".to_string();
            }
            return String::new();
        }
        if ctx.name_to_shape.get(ty.as_str()) == Some(&Shape::Trait) {
            return ".await?".to_string();
        }
    }

    // Fluent SDK send / async fallible stubs (untyped receivers).
    if method == "send"
        || method == "send_with"
        || ctx.async_fallible_methods.contains(method)
    {
        return ".await.map_err(|e| DomainError::External(e.to_string()))?".to_string();
    }
    // Untyped receiver: method name appears on a port trait → async_trait.
    let is_trait_method = ctx
        .method_returns
        .keys()
        .any(|(ty, m)| m == method && ctx.name_to_shape.get(ty) == Some(&Shape::Trait));
    if is_trait_method {
        return ".await?".to_string();
    }
    // Sync Res! stub methods: map any Error into DomainError (generic — not domain-specific).
    if ctx.fallible_methods.contains(method) {
        return ".map_err(|e| DomainError::External(e.to_string()))?".to_string();
    }
    String::new()
}

/// Rust method/path segment for a call: keep PascalCase for enum variants /
/// associated constructors (`AttributeValue::S`); snake_case for normal methods.
/// Strip VEIL fallible/query suffixes (`!` / `?`) — those are typecheck sugar only.
fn rust_method_name(method: &str) -> String {
    let method = method.trim_end_matches(['!', '?']);
    if method
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        method.to_string()
    } else {
        to_snake(method)
    }
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
    } else if !call.target.is_empty() && !ctx.is_trait_target(&call.target) && (call.method == "get" || call.method == "len") {
        Some(call.target.clone())
    } else {
        None
    };
    if let Some(base) = list_base {
        if call.method == "get" && call.args.len() == 1 {
            // String args (HashMap key lookup) stay as .get("key").
            // Numeric / Int locals (including loop indices) → slice index with `as usize`
            // so saga `steps.get(i)` and list access compile (i64 is not SliceIndex).
            let is_string_arg = matches!(&call.args[0], Expr::StringLit(_));
            if !is_string_arg {
                let idx = expr_to_rust(&call.args[0], ctx);
                return format!("{}[({}) as usize]", base, idx);
            }
        }
        if call.method == "len" && call.args.is_empty() {
            return format!("({}.len() as i64)", base);
        }
    }

    // Chained method call: `<receiver>.method(args)` (e.g. `.collect()` in
    // `items.map(f).collect()`). The receiver carries the left side of the chain.
    if let Some(recv) = &call.receiver {
        let recv_str = expr_to_rust(recv, ctx);
        // DynamoDB AttributeValue .as_s() / .as_n() returns Result<&str, &AttributeValue>
        // which doesn't implement From for DomainError — use map_err.
        if (call.method == "as_s" || call.method == "as_n" || call.method.starts_with("as_"))
            && call.args.is_empty()
        {
            return format!(
                "{}.{}().map(|s| s.to_string()).map_err(|e| DomainError::External(format!(\"{{:?}}\", e)))?",
                recv_str,
                to_snake(&call.method)
            );
        }
        // A trait method invoked on a chained receiver is async + fallible.
        let suffix = receiver_call_suffix(recv, &call.method, ctx);
        let m = rust_method_name(&call.method);
        return format!(
            "{}.{}({}){}",
            recv_str,
            m,
            clone_args(&call.args, ctx),
            suffix
        );
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
        // Routing traits (e.g. Bus) use the ctx bus reference (`deps.bus` in a flow,
        // `bus` inside a saga-step impl); other trait deps come from `deps`.
        if ctx.routing_traits.contains(&call.target) {
            return format!("{}.{}({}).await?", ctx.bus_ref, to_snake(method), final_args);
        }
        // Check if this method returns Option<T> — if so, unwrap with .ok_or(NotFound)?
        // Keys may be stored with or without `!` (method declaration name).
        let method_key = method.trim_end_matches(['!', '?']);
        let returns_option = ctx
            .method_returns
            .get(&(call.target.clone(), method.to_string()))
            .or_else(|| {
                ctx.method_returns
                    .get(&(call.target.clone(), method_key.to_string()))
            })
            .or_else(|| {
                ctx.method_returns
                    .get(&(call.target.clone(), format!("{method_key}!")))
            })
            .map(|t| t.starts_with("Option<"))
            .unwrap_or(false);
        let opt_suffix = if returns_option {
            ".ok_or(DomainError::NotFound)?"
        } else {
            ""
        };
        return format!(
            "deps.{}.{}({}).await?{}",
            dep_name,
            to_snake(method_key),
            final_args,
            opt_suffix
        );
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

    // Language primitives win over stub names (e.g. gix.stub `struct Id`,
    // axum.stub `Json` — IR Json is not axum::Json).
    if !call.method.is_empty() {
        let lang = match (call.target.as_str(), call.method.as_str()) {
            ("Id", "new") | ("Id", "new_v4") | ("UUID", "new") | ("UUID", "new_v4") | ("Uuid", "new")
                => Some("Uuid::new_v4()".to_string()),
            ("Dt", "now") => Some("Utc::now()".to_string()),
            ("Json", "parse") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::from_str(&{})?", arg))
            }
            ("Json", "stringify") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::to_string(&{})?", arg))
            }
            _ => None,
        };
        if let Some(result) = lang {
            return result;
        }
    }

    // Built-in type-level method translations.
    // These are VEIL's short type names with associated methods that map
    // to Rust idioms. No framework knowledge — just language primitives.
    // ONLY apply these if the target is NOT a known domain struct (e.g., an entity
    // named "List" should NOT be translated as Vec::new()).
    if !call.method.is_empty() && !ctx.is_struct_target(&call.target) {
        let translated = match (call.target.as_str(), call.method.as_str()) {
            ("Dt", "now") => Some("Utc::now()".to_string()),
            ("Uuid", "new_v4") => Some("Uuid::new_v4()".to_string()),
            ("Map", "new") => Some("HashMap::new()".to_string()),
            ("List", "new") => Some("Vec::new()".to_string()),
            ("Opt", "empty") => Some("None".to_string()),
            ("Opt", "some") if call.args.len() == 1 => {
                Some(format!("Some({})", expr_to_rust(&call.args[0], ctx)))
            }
            ("Env", "get_or") if call.args.len() == 2 => {
                let var = expr_to_rust(&call.args[0], ctx);
                let default = expr_to_rust(&call.args[1], ctx);
                Some(format!("std::env::var({}).unwrap_or_else(|_| {}.to_string())", var, default))
            }
            ("Env", "get_opt") if call.args.len() == 1 => {
                let var = expr_to_rust(&call.args[0], ctx);
                Some(format!("std::env::var({}).ok()", var))
            }
            ("Json", "parse") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::from_str(&{})?", arg))
            }
            ("Json", "stringify") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("serde_json::to_string(&{})?", arg))
            }
            ("Str", "from_bytes") if call.args.len() == 1 => {
                let arg = expr_to_rust(&call.args[0], ctx);
                Some(format!("String::from_utf8({})?", arg))
            }
            // Filesystem / process IO must NOT live in the engine (MISSION: zero
            // domain knowledge). Author adapters against ports or .stub crates
            // (e.g. runtime/src/stubs/*_fs.stub + real crate), never Fs/Shell builtins.
            _ => None,
        };
        if let Some(result) = translated {
            return result;
        }
    }

    // Struct-shaped target with method "new" or empty → Type::new(args)
    // Handle dotted paths: `sqlx.Query` → check if `Query` is a known struct
    let effective_target = if call.target.contains('.') {
        call.target.split('.').last().unwrap_or(&call.target).to_string()
    } else {
        call.target.clone()
    };
    if ctx.is_struct_target(&effective_target) {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        // Qualify with crate path if type is from a stub
        let qualified = if let Some((crate_name, original_name)) = ctx.stub_type_crate.get(&effective_target) {
            format!("{}::{}", crate_name, original_name)
        } else {
            effective_target.clone()
        };
        // Clone args to avoid move issues
        let cloned = call.args.iter()
            .map(|a| {
                let s = expr_to_rust(a, ctx);
                match a { Expr::Ident(_) => format!("{}.clone()", s), _ => s }
            }).collect::<Vec<_>>().join(", ");
        if method == "new" {
            // Stub constructors that map to module-level free functions.
            // e.g. crate::Query::new(sql) → crate::query(sql)
            // When the stub declares `typed_variant` and the enclosing method has a
            // domain return type → crate::query_as::<_, T>(sql) (params from stub).
            if let Some(module) = qualified.split("::").next() {
                let is_module_fn = qualified.contains("::")
                    && module.chars().next().map(|c| c.is_lowercase()).unwrap_or(false);
                if is_module_fn {
                    let type_leaf = qualified.split("::").last().unwrap_or("new");
                    let fn_name = to_snake(type_leaf);
                    let raw_args = call.args.iter()
                        .map(|a| match a {
                            Expr::StringLit(s) => format!("\"{}\"", s),
                            _ => expr_to_rust(a, ctx),
                        })
                        .collect::<Vec<_>>().join(", ");

                    let domain_type = ctx
                        .expected_return_rust
                        .as_ref()
                        .and_then(|ret| extract_domain_type_from_return(ret, &ctx.name_to_shape));

                    // Prefer explicit stub metadata; fall back to sibling `TypeAs` heuristic.
                    let typed_meta = ctx
                        .stub_typed_ctors
                        .get(&effective_target)
                        .or_else(|| ctx.stub_typed_ctors.get(type_leaf));

                    if let Some(domain_type) = domain_type {
                        if let Some((typed_fn, param_tmpl)) = typed_meta {
                            let tparams = expand_typed_type_params(param_tmpl, &domain_type);
                            return format!(
                                "{module}::{typed_fn}::<{tparams}>({raw_args})"
                            );
                        }
                        // Heuristic: Query + QueryAs both registered → query_as
                        let typed_struct = format!("{type_leaf}As");
                        let has_sibling = ctx.stub_type_crate.contains_key(&typed_struct)
                            || ctx.name_to_shape.contains_key(&typed_struct);
                        if has_sibling {
                            let typed_fn_name = format!("{fn_name}_as");
                            return format!(
                                "{module}::{typed_fn_name}::<_, {domain_type}>({raw_args})"
                            );
                        }
                    }
                    return format!("{module}::{fn_name}({raw_args})");
                }
            }
            // If the struct has an `id` field and the caller doesn't provide it
            // (arg count is one fewer than expected), auto-insert Uuid::new_v4() as first arg.
            let has_id_field = ctx.struct_fields.get(&effective_target)
                .map(|fields| fields.iter().any(|(n, _)| n == "id"))
                .unwrap_or(false);
            let final_args = if has_id_field && !call.args.is_empty() {
                // Check if first arg is already named 'id' — if so, caller is providing it
                let first_is_id = matches!(&call.args[0], Expr::Ident(n) if n == "id");
                if first_is_id {
                    cloned // caller provides id explicitly
                } else {
                    // Prepend auto-generated id
                    format!("Uuid::new_v4(), {}", cloned)
                }
            } else if has_id_field && call.args.is_empty() {
                "Uuid::new_v4()".to_string()
            } else {
                cloned
            };
            // If the constructor returns Result (invariant type), append ? to unwrap
            let returns_result = ctx.method_returns.get(&(effective_target.clone(), "new".to_string()))
                .map(|t| t.starts_with("Result<"))
                .unwrap_or(false);
            let suffix = if returns_result { "?" } else { "" };
            return format!("{}::{}({}){}", qualified, to_snake(method), final_args, suffix);
        }
        // Non-new method on a struct: UFCS instance form `Email.validate(email)`
        // → `email.validate()`. Only when the first arg *names* the type
        // (Email/email). Do NOT rewrite for any local — that breaks enum
        // constructors: `AttributeValue.S(name)` must stay `AttributeValue::S(name)`.
        // PascalCase methods are always associated constructors / variants.
        let is_pascal_ctor = method
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if !is_pascal_ctor && !call.args.is_empty() {
            if let Expr::Ident(first_arg) = &call.args[0] {
                if first_arg.eq_ignore_ascii_case(&effective_target) {
                    let rest_args = call.args[1..]
                        .iter()
                        .map(|a| expr_to_rust(a, ctx))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("{}.{}({})", first_arg, to_snake(method), rest_args);
                }
            }
        }
        // Prefer stub-qualified path (aws_sdk_s3::Client) over VEIL alias (S3Client).
        // Keep PascalCase for enum variants: AttributeValue::S(x), not ::s(x).
        let m = rust_method_name(method);
        let suffix = receiver_call_suffix(
            &Expr::Ident(effective_target.clone()),
            method,
            ctx,
        );
        return format!("{}::{}({}){}", qualified, m, args_str, suffix);
    }

    // `local.field.method(args)` — parser keeps dotted target "initiative.id".
    // Emit `initiative.id.method(...)`, never `id::method(...)`.
    if call.target.contains('.') && !call.target.starts_with("self.") {
        let first = call.target.split('.').next().unwrap_or("");
        if ctx.is_local(first) {
            let path = call
                .target
                .split('.')
                .enumerate()
                .map(|(i, seg)| {
                    if i == 0 {
                        seg.to_string()
                    } else {
                        to_snake(seg)
                    }
                })
                .collect::<Vec<_>>()
                .join(".");
            let method = rust_method_name(&call.method);
            let suffix = receiver_call_suffix(
                &Expr::Ident(first.to_string()),
                &call.method,
                ctx,
            );
            // Clone String fields when calling to_string-like methods is unnecessary;
            // for by-value SDK args, Uuid/DateTime Display paths use to_string().
            return format!(
                "{}.{}({}){}",
                path,
                method,
                clone_args(&call.args, ctx),
                suffix
            );
        }
    }

    // Self field target (adapter bodies) → self.target.method(args)
    // Parser may produce target "client" or dotted "self.client".
    if ctx.in_aggregate_fn {
        let field = call
            .target
            .strip_prefix("self.")
            .unwrap_or(call.target.as_str());
        if ctx.self_fields.contains(field)
            || call.target.starts_with("self.")
        {
            let method = rust_method_name(&call.method);
            let suffix = receiver_call_suffix(
                &Expr::Ident(field.to_string()),
                &call.method,
                ctx,
            );
            return format!(
                "self.{}.{}({}){}",
                to_snake(field),
                method,
                clone_args(&call.args, ctx),
                suffix
            );
        }
    }

    // Local variable target → target.method(args)?
    if ctx.is_local(&call.target) {
        // Always strip VEIL `!`/`?` fallible/query suffixes (typecheck sugar only).
        let method = rust_method_name(&call.method);

        // HashMap/DynamoDB item .get("key") pattern: emit as &str arg + unwrap Option.
        if call.method == "get" && call.args.len() == 1 {
            if let Expr::StringLit(key) = &call.args[0] {
                // .get("key") returns Option<&V> — emit .get("key").unwrap() for now
                // (callers typically chain .as_s()? which handles the error case)
                return format!("{}.get(\"{}\").unwrap()", call.target, key);
            }
        }

        if let Some(type_name) = ctx.local_type(&call.target) {
            // If the local's type is a known trait, its methods are async and
            // fallible (`#[async_trait]` + `-> Result`): emit `.await?`.
            if ctx.name_to_shape.get(type_name) == Some(&Shape::Trait) {
                return format!("{}.{}({}).await?", call.target, method, args_str);
            }
            // Known concrete method (e.g. aggregate fn) — call with ?
            if ctx.method_returns.contains_key(&(type_name.to_string(), call.method.clone()))
                || ctx.method_returns.contains_key(&(
                    type_name.to_string(),
                    call.method.trim_end_matches(['!', '?']).to_string(),
                ))
            {
                let cloned_args = clone_args(&call.args, ctx);
                let suffix = receiver_call_suffix(
                    &Expr::Ident(call.target.clone()),
                    &call.method,
                    ctx,
                );
                return format!("{}.{}({}){}", call.target, method, cloned_args, suffix);
            }
        }
        // Stub getters that return Result<&str, _> (e.g. enum as_s): own a String.
        if (call.method == "as_s" || call.method == "as_n" || call.method.starts_with("as_"))
            && call.args.is_empty()
            && ctx.fallible_methods.contains(&call.method)
        {
            return format!(
                "{}.{}().map(|s| s.to_string()).map_err(|e| DomainError::External(format!(\"{{:?}}\", e)))?",
                call.target,
                method
            );
        }
        // Unknown method on local — clone args to avoid move issues.
        // Collection predicate methods need .iter() prefix in Rust.
        let iter_methods = ["any", "all", "find", "filter", "map", "for_each", "count", "flat_map"];
        if iter_methods.contains(&method.as_str()) {
            return format!(
                "{}.iter().{}({})",
                call.target,
                method,
                clone_args(&call.args, ctx)
            );
        }
        let suffix = receiver_call_suffix(
            &Expr::Ident(call.target.clone()),
            &call.method,
            ctx,
        );
        return format!(
            "{}.{}({}){}",
            call.target,
            method,
            clone_args(&call.args, ctx),
            suffix
        );
    }
    if call.method.is_empty() {
        // Bare call: now() → Utc::now(), others → as-is (cloning value args so
        // passing locals/state into a by-value param doesn't move them).
        match call.target.as_str() {
            "now" => "Utc::now()".to_string(),
            _ => {
                let base = format!("{}({})", to_snake(&call.target), clone_args(&call.args, ctx));
                // Layer-declared async functions (e.g. unwind, run_saga) need .await?
                if ctx.async_fns.contains(&call.target) {
                    format!("{}.await?", base)
                } else {
                    base
                }
            }
        }
    } else if ctx.is_local(&call.target) || ctx.name_to_shape.contains_key(&call.target) {
        // Known local/construct method call (already handled above, but be safe).
        format!("{}.{}({})", call.target, to_snake(&call.method), args_str)
    } else {
        // Unknown target with a method (e.g. `http.post(...)`): an external
        // effect. Route it to a generated runtime hook `<target>_<method>(...)`
        // so the code compiles without inventing domain knowledge. The set of
        // hooks is emitted at the bottom of the module.
        //
        // If target has dots (e.g. `sqlx.Query`), the last segment is the
        // struct name — emit `Struct::method(args)` (Rust path syntax).
        // Skip `self.field` — already handled above when in_aggregate_fn.
        if call.target.contains('.') && !call.target.starts_with("self.") {
            let parts: Vec<&str> = call.target.split('.').collect();
            let struct_name = parts.last().unwrap_or(&"");
            // Qualify via stub map when present
            let qualified = if let Some((crate_name, original_name)) =
                ctx.stub_type_crate.get(*struct_name).or_else(|| {
                    // case-insensitive match for Client vs client
                    ctx.stub_type_crate
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(struct_name))
                        .map(|(_, v)| v)
                }) {
                format!("{}::{}", crate_name, original_name)
            } else {
                (*struct_name).to_string()
            };
            let m = rust_method_name(&call.method);
            let bare = call.method.trim_end_matches(['!', '?']);
            let suffix = if bare == "send"
                || bare == "send_with"
                || ctx.async_fallible_methods.contains(bare)
            {
                ".await.map_err(|e| DomainError::External(e.to_string()))?"
            } else if ctx.fallible_methods.contains(bare) {
                "?"
            } else {
                ""
            };
            return format!("{}::{}({}){}", qualified, m, args_str, suffix);
        }
        // Recognize Rust module-qualified calls: serde_json.from_str, std.fs.read, etc.
        // These are lowercase targets with no dots that map to Rust crate paths using `::`.
        let known_modules = [
            "serde_json", "serde", "tokio", "tracing", "uuid", "chrono",
            "std", "aws_sdk_dynamodb", "aws_sdk_s3", "aws_config",
        ];
        let target_snake = to_snake(&call.target);
        if known_modules.contains(&target_snake.as_str()) {
            let m = to_snake(&call.method);
            let suffix = if ctx.fallible_methods.contains(&call.method)
                || call.method == "from_str"
                || call.method == "to_string"
                || call.method == "parse"
            {
                "?"
            } else {
                ""
            };
            // serde_json.from_str → serde_json::from_str(&arg)?
            // serde_json.to_string → serde_json::to_string(&arg)?
            let needs_ref = m == "from_str" || m == "to_string" || m == "to_vec";
            let final_args = if needs_ref && call.args.len() == 1 {
                format!("&{}", expr_to_rust(&call.args[0], ctx))
            } else {
                args_str.clone()
            };
            return format!("{}::{}({}){}", target_snake, m, final_args, suffix);
        }
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
                // domain validation error. Ports/traits and Result-returning
                // value methods (e.g. Email.validate) use map_err; bool
                // predicates use the branch below.
                Some(cond @ Expr::Call(c))
                    if !c.method.is_empty()
                        && (ctx.name_to_shape.contains_key(&c.target)
                            || ctx.fallible_methods.contains(&c.method)
                            || c.method == "validate") =>
                {
                    let call_str = expr_to_rust(cond, ctx);
                    // translate_call may already append `?`; strip it so our
                    // map_err drives the propagation.
                    let base = call_str
                        .strip_suffix(".await?")
                        .or_else(|| call_str.strip_suffix('?'))
                        .unwrap_or(&call_str);
                    format!(
                        "{}.map_err(|_| DomainError::Validation(\"{}\".to_string()))?",
                        base, msg_escaped
                    )
                }
                Some(cond @ Expr::Await(_)) => {
                    let call_str = expr_to_rust(cond, ctx);
                    let base = call_str.strip_suffix('?').unwrap_or(&call_str);
                    format!(
                        "{}.map_err(|_| DomainError::Validation(\"{}\".to_string()))?",
                        base, msg_escaped
                    )
                }
                // Boolean guard: the condition must evaluate to true.
                Some(cond) => {
                    let cond_str = expr_to_rust(cond, ctx);
                    // Suppress redundant `.is_some()` guards on variables that were
                    // already unwrapped from Option via `.ok_or(...)` on the port call.
                    // The codegen appends `.ok_or(DomainError::NotFound)?` to methods
                    // returning Option<T>, so the variable is T — not Option<T>.
                    if let Expr::Call(c) = cond {
                        if c.method == "is_some" && ctx.locals.contains(&c.target) {
                            let var_type = ctx.local_types.get(&c.target);
                            let is_option = var_type
                                .map(|t| t.starts_with("Option<") || t == "Option")
                                .unwrap_or(false);
                            if !is_option {
                                return format!(
                                    "/* guard {:?} — already unwrapped by .ok_or() above */",
                                    msg_escaped
                                );
                            }
                        }
                    }
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
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            let s = expr_to_rust(expr, ctx);
            // Field assigns (`wt.name = x`) are not new locals.
            if !name.contains('.') {
                let inferred_type = infer_expr_type(rhs, ctx);
                ctx.locals.insert(name.clone());
                if let Some(t) = inferred_type {
                    ctx.local_types.insert(name.clone(), t);
                }
            }
            format!("    {};", s)
        }
        _ => format!("    {};", expr_to_rust(expr, ctx)),
    }
}

/// Binding names introduced by a match arm pattern string (e.g. `Some(item)` → `item`).
fn pattern_binding_names(pattern: &str) -> Vec<String> {
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
    match pat {
        Pattern::Ident(s) => to_snake(s),
        Pattern::Tuple(parts) => {
            let inner = parts.iter().map(pattern_to_rust).collect::<Vec<_>>().join(", ");
            format!("({})", inner)
        }
        Pattern::Struct(name, fields, has_rest) => {
            let mut fs: Vec<String> = fields.iter().map(|(k, v)| {
                match v {
                    Some(pat) => format!("{}: {}", to_snake(k), pattern_to_rust(pat)),
                    None => to_snake(k),
                }
            }).collect();
            if *has_rest { fs.push("..".to_string()); }
            format!("{} {{ {} }}", name, fs.join(", "))
        }
        Pattern::Variant(name, args) => {
            if args.is_empty() { name.clone() }
            else {
                let inner = args.iter().map(pattern_to_rust).collect::<Vec<_>>().join(", ");
                format!("{}({})", name, inner)
            }
        }
        Pattern::Literal(s) => s.clone(),
        Pattern::Or(alts) => alts.iter().map(pattern_to_rust).collect::<Vec<_>>().join(" | "),
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
            in_aggregate_fn: self.in_aggregate_fn,
            is_orchestrator: self.is_orchestrator,
            method_returns: self.method_returns.clone(),
            local_types: self.local_types.clone(),
            struct_fields: self.struct_fields.clone(),
            bus_ref: self.bus_ref.clone(),
            routing_traits: self.routing_traits.clone(),
            async_fns: self.async_fns.clone(),
            state_locals: self.state_locals.clone(),
            stub_type_crate: self.stub_type_crate.clone(),
            stub_typed_ctors: self.stub_typed_ctors.clone(),
            fallible_methods: self.fallible_methods.clone(),
            async_fallible_methods: self.async_fallible_methods.clone(),
            expected_return_rust: self.expected_return_rust.clone(),
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
                let ret = ctx.return_type_of(&call.target, method).map(|s| s.to_string());
                // Bang / dep port calls: codegen does `.await?` and for Option
                // `.ok_or(NotFound)?` — effective type is the unwrapped value.
                return ret.map(|t| unwrap_codegen_call_return(&t, method));
            }
            // If calling a struct constructor
            if ctx.is_struct_target(&call.target) {
                let method = if call.method.is_empty() { "new" } else { &call.method };
                return ctx.return_type_of(&call.target, method).map(|s| s.to_string());
            }
            // If calling a method on a local (e.g. @dep wear_test_repo typed as trait via name_to_shape)
            if ctx.is_local(&call.target) || ctx.is_trait_target(&call.target) {
                if let Some(t) = ctx.return_type_of(&call.target, &call.method) {
                    return Some(unwrap_codegen_call_return(t, &call.method));
                }
            }
            None
        }
        // Empty list `[]` — element unknown until append
        Expr::ArrayLit(items) if items.is_empty() => Some("Vec<()>".to_string()),
        Expr::ArrayLit(items) => items
            .first()
            .and_then(|e| infer_expr_type(e, ctx))
            .map(|t| format!("Vec<{t}>")),
        Expr::BinaryOp(bin) if matches!(bin.op, BinOp::Add) => {
            // options + [x] → keep/upgrade Vec type
            let left = infer_expr_type(&bin.left, ctx);
            let right = infer_expr_type(&bin.right, ctx);
            match (left.as_deref(), right.as_deref()) {
                (Some("Vec<()>"), Some(r)) if r.starts_with("Vec<") => right,
                (Some(l), _) if l.starts_with("Vec<") && l != "Vec<()>" => left,
                (_, Some(r)) if r.starts_with("Vec<") => right,
                _ => left.or(right),
            }
        }
        Expr::StructLit(name, _) => Some(name.clone()),
        _ => None,
    }
}

/// Match codegen for bang/port calls: strip Option after Res (already unwrapped
/// in method_returns via extract_inner_type). Option stays only for non-bang.
fn unwrap_codegen_call_return(ty: &str, method: &str) -> String {
    let is_bang = method.ends_with('!');
    // method_returns already stores inner of Result (extract_inner_type).
    // For bang + Option return, codegen adds `.ok_or()?` → strip Option.
    if is_bang {
        if let Some(inner) = ty
            .strip_prefix("Option<")
            .and_then(|s| s.strip_suffix('>'))
        {
            return inner.to_string();
        }
    }
    // Also strip Option for non-bang when we know codegen always ok_or on
    // trait methods — currently only bang paths use ok_or consistently.
    // Keep Option for non-bang so `existing.is_some()` typechecks if someone
    // writes non-bang find.
    ty.to_string()
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
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => collect_deps_from_expr(rhs, ctx, deps),
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
        Expr::Match(scrutinee, arms) => {
            collect_deps_from_expr(scrutinee, ctx, deps);
            for arm in arms {
                for expr in &arm.body {
                    collect_deps_from_expr(expr, ctx, deps);
                }
            }
        }
        Expr::IfExpr(data) => {
            collect_deps_from_expr(&data.condition, ctx, deps);
            for expr in &data.then_body {
                collect_deps_from_expr(expr, ctx, deps);
            }
            if let Some(eb) = &data.else_body {
                for expr in eb {
                    collect_deps_from_expr(expr, ctx, deps);
                }
            }
        }
        Expr::Return(inner) => {
            collect_deps_from_expr(inner, ctx, deps);
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
