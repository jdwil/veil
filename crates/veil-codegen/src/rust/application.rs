use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;

/// Find a construct by name anywhere in the solution (top-level or nested).
pub fn find_construct_by_name<'a>(solution: &'a Solution, name: &str) -> Option<&'a Construct> {
    fn walk<'a>(c: &'a Construct, name: &str) -> Option<&'a Construct> {
        if c.name == name {
            return Some(c);
        }
        c.children.iter().find_map(|ch| walk(ch, name))
    }
    solution.items.iter().find_map(|i| match i {
        TopLevelItem::Construct(c) => walk(c, name),
        _ => None,
    })
}

/// Build a name→shape map from ALL constructs in the solution (top-level and
/// nested), used by the expression translator for shape-driven call resolution.
pub fn build_name_to_shape(solution: &Solution, registry: &LayerRegistry) -> std::collections::HashMap<String, Shape> {
    use std::collections::HashMap;
    fn index(c: &Construct, map: &mut HashMap<String, Shape>) {
        map.insert(c.name.clone(), c.shape);
        for child in &c.children {
            index(child, map);
        }
    }
    let mut map = HashMap::new();
    for item in &solution.items {
        match item {
            TopLevelItem::Construct(c) => index(c, &mut map),
            // Type aliases to traits act as ports for call resolution.
            TopLevelItem::TypeAlias { name, target } => {
                // EntityRepo may be nested under a context; Generic aliases
                // always resolve as Trait for DI (type WearTestRepo = EntityRepo<…>).
                if matches!(target, TypeExpr::Generic(_, _) | TypeExpr::Named(_)) {
                    map.insert(name.clone(), Shape::Trait);
                }
            }
            _ => {}
        }
    }
    // Also include layer-defined constructs (from all loaded layers)
    // so adapters can reference types like S3Client, DdbClient etc.
    for spec in &registry.constructs {
        map.insert(spec.name.clone(), spec.shape);
    }
    // Also include stub-declared structs so adapter bodies recognize
    // them as struct targets (generating Type::new() instead of type_new())
    for stub in &registry.stubs {
        for s in &stub.structs {
            let type_name = if let Some(alias) = &stub.alias {
                let cap_alias = alias.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() + &alias[1..];
                format!("{}{}", cap_alias, s.name)
            } else {
                s.name.clone()
            };
            map.insert(type_name, Shape::Struct);
        }
    }
    map
}

/// Walk an expression tree collecting external-effect hook calls: a `Call`
/// with a non-empty method whose target is neither a known construct nor a
/// local. Records `(snake(target)_snake(method), arg_count)`.
/// Targets that lower to real Rust paths in expr.rs (not external-effect hooks).
pub fn is_known_codegen_module(target: &str) -> bool {
    matches!(
        target,
        "serde_json"
            | "serde"
            | "tokio"
            | "tracing"
            | "uuid"
            | "chrono"
            | "std"
            | "aws_sdk_dynamodb"
            | "aws_sdk_s3"
            | "aws_config"
            | "sqlx"
            | "Json"
            | "Map"
            | "List"
            | "Opt"
            | "Dt"
            | "Uuid"
            | "Env"
            | "Str"
    ) || target.starts_with("aws_sdk_")
        || target.starts_with("aws_config")
}

pub fn collect_effect_hooks_tracked(
    expr: &Expr,
    name_to_shape: &std::collections::HashMap<String, Shape>,
    locals: &mut std::collections::HashSet<String>,
    hooks: &mut std::collections::BTreeSet<(String, usize)>,
    product_free_fns: &std::collections::HashSet<String>,
    stub_pkg_roots: &std::collections::HashSet<String>,
) {
    match expr {
        Expr::Call(call) => {
            if !call.method.is_empty()
                && call.receiver.is_none()
                && !name_to_shape.contains_key(&call.target)
                && !locals.contains(&call.target)
                && !call.target.is_empty()
                && !call.target.contains('.') // dotted paths resolve as Struct::method
                && !is_known_codegen_module(&call.target)
                && !stub_pkg_roots.contains(&call.target)
            {
                let name = format!(
                    "{}_{}",
                    to_snake(&call.target),
                    to_snake(call.method.trim_end_matches(['!', '?']))
                );
                hooks.insert((name, call.args.len()));
            }
            // Bare function calls: skip product free fns (emitted later in this module).
            // Bang form stores target as `name!` — strip for lookup and hook name.
            let bare_target = call.target.trim_end_matches(['!', '?']);
            if call.method.is_empty()
                && call.receiver.is_none()
                && !name_to_shape.contains_key(bare_target)
                && !locals.contains(bare_target)
                && !call.target.is_empty()
                && bare_target != "drop"
                && bare_target
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_lowercase())
                && !is_known_codegen_module(bare_target)
                && !product_free_fns.contains(&to_snake(bare_target))
            {
                let name = to_snake(bare_target);
                hooks.insert((name, call.args.len()));
            }
            if let Some(recv) = &call.receiver {
                collect_effect_hooks_tracked(
                    recv,
                    name_to_shape,
                    locals,
                    hooks,
                    product_free_fns,
                    stub_pkg_roots,
                );
            }
            for a in &call.args {
                collect_effect_hooks_tracked(
                    a,
                    name_to_shape,
                    locals,
                    hooks,
                    product_free_fns,
                    stub_pkg_roots,
                );
            }
        }
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            collect_effect_hooks_tracked(
                rhs,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
            if !name.contains('.') {
                locals.insert(name.clone());
            }
        }
        Expr::Return(rhs) | Expr::Await(rhs) => {
            collect_effect_hooks_tracked(
                rhs,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
        }
        Expr::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_effect_hooks_tracked(
                    v,
                    name_to_shape,
                    locals,
                    hooks,
                    product_free_fns,
                    stub_pkg_roots,
                );
            }
        }
        Expr::BinaryOp(op) => {
            collect_effect_hooks_tracked(
                &op.left,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
            collect_effect_hooks_tracked(
                &op.right,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
        }
        Expr::IfExpr(ie) => {
            collect_effect_hooks_tracked(
                &ie.condition,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
            let mut then_locals = locals.clone();
            for e in &ie.then_body {
                collect_effect_hooks_tracked(
                    e,
                    name_to_shape,
                    &mut then_locals,
                    hooks,
                    product_free_fns,
                    stub_pkg_roots,
                );
            }
            if let Some(eb) = &ie.else_body {
                let mut else_locals = locals.clone();
                for e in eb {
                    collect_effect_hooks_tracked(
                        e,
                        name_to_shape,
                        &mut else_locals,
                        hooks,
                        product_free_fns,
                        stub_pkg_roots,
                    );
                }
            }
        }
        Expr::WhileLoop { condition, body } => {
            collect_effect_hooks_tracked(
                condition,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
            let mut body_locals = locals.clone();
            for e in body {
                collect_effect_hooks_tracked(
                    e,
                    name_to_shape,
                    &mut body_locals,
                    hooks,
                    product_free_fns,
                    stub_pkg_roots,
                );
            }
        }
        Expr::ForLoop {
            binding,
            index,
            iterable,
            body,
        } => {
            collect_effect_hooks_tracked(
                iterable,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
            let mut body_locals = locals.clone();
            body_locals.insert(binding.clone());
            if let Some(idx) = index {
                body_locals.insert(idx.clone());
            }
            for e in body {
                collect_effect_hooks_tracked(
                    e,
                    name_to_shape,
                    &mut body_locals,
                    hooks,
                    product_free_fns,
                    stub_pkg_roots,
                );
            }
        }
        Expr::Match(scrutinee, arms) => {
            collect_effect_hooks_tracked(
                scrutinee,
                name_to_shape,
                locals,
                hooks,
                product_free_fns,
                stub_pkg_roots,
            );
            for arm in arms {
                let mut arm_locals = locals.clone();
                for e in &arm.body {
                    collect_effect_hooks_tracked(
                        e,
                        name_to_shape,
                        &mut arm_locals,
                        hooks,
                        product_free_fns,
                        stub_pkg_roots,
                    );
                }
            }
        }
        _ => {}
    }
}

/// True if any subexpression calls a stub-declared type (S3Client, DdbClient, …).
pub fn expr_refs_stub_type(
    expr: &Expr,
    stubs: &std::collections::HashMap<String, (String, String)>,
) -> bool {
    match expr {
        Expr::Call(call) => {
            let target = if call.target.contains('.') {
                call.target.split('.').next_back().unwrap_or(&call.target)
            } else {
                call.target.as_str()
            };
            if stubs.contains_key(target) || stubs.contains_key(&call.target) {
                return true;
            }
            if call.args.iter().any(|a| expr_refs_stub_type(a, stubs)) {
                return true;
            }
            call.receiver
                .as_ref()
                .map(|r| expr_refs_stub_type(r, stubs))
                .unwrap_or(false)
        }
        Expr::FieldAccess(base, _) | Expr::Await(base) | Expr::Try(base) | Expr::Require(base) | Expr::Return(base) => {
            expr_refs_stub_type(base, stubs)
        }
        Expr::UnaryOp(u) => expr_refs_stub_type(&u.expr, stubs),
        Expr::Assign(_, v, _) | Expr::MutAssign(_, v, _) | Expr::LetPattern(_, v, _) => {
            expr_refs_stub_type(v, stubs)
        }
        Expr::BinaryOp(op) => {
            expr_refs_stub_type(&op.left, stubs) || expr_refs_stub_type(&op.right, stubs)
        }
        Expr::IfExpr(data) => {
            expr_refs_stub_type(&data.condition, stubs)
                || data.then_body.iter().any(|e| expr_refs_stub_type(e, stubs))
                || data
                    .else_body
                    .iter()
                    .flatten()
                    .any(|e| expr_refs_stub_type(e, stubs))
        }
        Expr::Match(scrut, arms) => {
            expr_refs_stub_type(scrut, stubs)
                || arms
                    .iter()
                    .any(|a| a.body.iter().any(|e| expr_refs_stub_type(e, stubs)))
        }
        Expr::ForLoop { iterable, body, .. } => {
            expr_refs_stub_type(iterable, stubs)
                || body.iter().any(|e| expr_refs_stub_type(e, stubs))
        }
        Expr::WhileLoop { condition, body } => {
            expr_refs_stub_type(condition, stubs)
                || body.iter().any(|e| expr_refs_stub_type(e, stubs))
        }
        Expr::Loop(body) | Expr::Closure { body, .. } => {
            body.iter().any(|e| expr_refs_stub_type(e, stubs))
        }
        Expr::Tuple(xs) | Expr::ArrayLit(xs) => xs.iter().any(|e| expr_refs_stub_type(e, stubs)),
        Expr::Index(a, b) => expr_refs_stub_type(a, stubs) || expr_refs_stub_type(b, stubs),
        Expr::StructLit(_, fields) | Expr::StructUpdate { fields, .. } => {
            fields.iter().any(|(_, v)| expr_refs_stub_type(v, stubs))
        }
        Expr::Cast(e, _) => expr_refs_stub_type(e, stubs),
        Expr::StringInterp(parts) => parts.iter().any(|p| match p {
            StringPart::Expr(e) => expr_refs_stub_type(e, stubs),
            _ => false,
        }),
        Expr::Action(a) => {
            a.args.iter().any(|e| expr_refs_stub_type(e, stubs))
                || a.named_args.iter().any(|(_, e)| expr_refs_stub_type(e, stubs))
                || a.condition
                    .as_ref()
                    .map(|c| expr_refs_stub_type(c, stubs))
                    .unwrap_or(false)
                || stubs.contains_key(&a.target)
        }
        Expr::Ident(name) => stubs.contains_key(name),
        _ => false,
    }
}

/// Produce a compiling `Ok(...)` expression for a `Result<T, E>` return type.
pub fn default_ok_for(ret_rust: &str) -> String {
    // Extract T from `Result<T, DomainError>`.
    let inner = ret_rust
        .strip_prefix("Result<")
        .and_then(|s| s.rfind(", ").map(|i| &s[..i]))
        .unwrap_or("()")
        .trim();
    match inner {
        "()" => "Ok(())".to_string(),
        "String" => "Ok(String::new())".to_string(),
        "Uuid" => "Ok(Uuid::new_v4())".to_string(),
        "i64" | "i32" | "u64" | "u32" | "usize" | "isize" => "Ok(0)".to_string(),
        "f64" | "f32" => "Ok(0.0)".to_string(),
        "bool" => "Ok(false)".to_string(),
        // Unknown concrete type: no guaranteed constructor. compile_error!()
        // makes the generated code fail at compile-time rather than panicking.
        _ => "compile_error!(\"unknown return type — stub needed\")".to_string(),
    }
}

/// Something that generates an application function — either a core `flow`
/// or an fn-shaped layer construct (service, saga, handler, …).
pub enum FlowLike<'a> {
    Flow(&'a Flow),
    Construct(&'a Construct),
}

/// Infer a flow's Rust return type as `Result<T, DomainError>`. Pre-scans step
/// bodies to learn local-binding types, then inspects the return expression:
/// a field access / ident resolves to its known type; a literal to its type.
/// Unknown or absent returns become `Result<(), DomainError>`.
pub fn infer_flow_return_type(
    return_expr: Option<&Expr>,
    steps: &[FlowStep],
    base_ctx: &crate::expr::GenCtx,
    envelope_routing: bool,
) -> String {
    // If there's an explicit top-level return expression, use it.
    // Otherwise, scan step bodies for `ret` (Expr::Return) statements.
    let ret: Option<&Expr> = return_expr.or_else(|| {
        for step in steps {
            if let FlowStep::Step(s) = step {
                for expr in &s.body {
                    if let Expr::Return(inner) = expr {
                        return Some(inner.as_ref());
                    }
                }
            }
        }
        None
    });

    let Some(ret) = ret else {
        return "Result<(), DomainError>".to_string();
    };

    // Pre-scan: clone the ctx and walk step bodies recording let-binding types
    // (mirrors what stmt_to_rust does), so `ret c.id` can resolve `c`'s type.
    let mut ctx = base_ctx.clone_for_inference();
    for step in steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = expr
                    && !name.contains('.') {
                        ctx.locals.insert(name.clone());
                        if envelope_routing {
                            // Envelope-routing locals are JSON message results.
                            ctx.types.local_types.insert(name.clone(), "serde_json::Value".to_string());
                        } else if let Some(t) = crate::expr::infer_expr_type_pub(rhs, &ctx) {
                            ctx.types.local_types.insert(name.clone(), t);
                        }
                    }
            }
        }
    }

    let inner = crate::expr::infer_return_expr_type(ret, &ctx);
    match inner {
        Some(t) if !t.is_empty() && t != "()" => format!("Result<{}, DomainError>", t),
        _ => "Result<(), DomainError>".to_string(),
    }
}

/// Scan an expression tree for ! method calls that indicate dep usage.
/// Registers the trait name in `deps` and records the call target as the preferred field name.
pub fn scan_dep_calls(
    expr: &Expr,
    name_to_shape: &std::collections::HashMap<String, Shape>,
    deps: &mut std::collections::HashSet<String>,
    field_names: &mut std::collections::HashMap<String, String>,
) {
    match expr {
        Expr::Call(call) => {
            if !call.target.is_empty() && call.method.ends_with('!') {
                // If a field name matching call.target is already claimed by an
                // explicit @dep annotation, skip inference — the dep is resolved.
                let already_claimed = field_names.values().any(|v| v == &call.target);
                if !already_claimed {
                    // Find matching trait
                    for (name, shape) in name_to_shape {
                        if *shape == Shape::Trait {
                            let trait_snake = to_snake(name);
                            if trait_snake == call.target || trait_snake.ends_with(&call.target) {
                                deps.insert(name.clone());
                                field_names.entry(name.clone()).or_insert_with(|| call.target.clone());
                                break;
                            }
                        }
                    }
                }
            }
            if let Some(recv) = &call.receiver {
                scan_dep_calls(recv, name_to_shape, deps, field_names);
            }
            for arg in &call.args {
                scan_dep_calls(arg, name_to_shape, deps, field_names);
            }
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => {
            scan_dep_calls(rhs, name_to_shape, deps, field_names);
        }
        Expr::IfExpr(data) => {
            scan_dep_calls(&data.condition, name_to_shape, deps, field_names);
            for e in &data.then_body { scan_dep_calls(e, name_to_shape, deps, field_names); }
            if let Some(eb) = &data.else_body {
                for e in eb { scan_dep_calls(e, name_to_shape, deps, field_names); }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            scan_dep_calls(iterable, name_to_shape, deps, field_names);
            for e in body {
                scan_dep_calls(e, name_to_shape, deps, field_names);
            }
        }
        Expr::WhileLoop { condition, body } => {
            scan_dep_calls(condition, name_to_shape, deps, field_names);
            for e in body {
                scan_dep_calls(e, name_to_shape, deps, field_names);
            }
        }
        Expr::Return(inner) => scan_dep_calls(inner, name_to_shape, deps, field_names),
        _ => {}
    }
}

pub fn gen_application(flows: &[FlowLike], module_contents: &ModuleContents, crate_name: &str, solution: &Solution, registry: &LayerRegistry, deps_decl: Option<&veil_ir::DepsDecl>, layer_fn_attrs: Option<&str>, template_output: &crate::template::TemplateOutput) -> GeneratedFile {
    use crate::expr::{build_ctx_from_solution, collect_deps, stmt_to_rust, expr_to_rust};
    use std::collections::HashMap;

    let mut out = String::new();
    out.push_str("//! Application services and flow functions.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables)]\n\n");
    out.push_str("use crate::ports::*;\nuse crate::domain::types::*;\nuse crate::domain::messages::*;\n");
    out.push_str("use std::sync::Arc;\nuse std::collections::HashMap;\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\n\n");

    if flows.is_empty() {
        out.push_str("// No flows defined in this module.\n");
        return GeneratedFile {
            path: format!("crates/{}/src/application/mod.rs", crate_name),
            content: out,
        };
    }

    // Build name→shape map from ALL constructs in the solution (traits, structs, etc.)
    let mut name_to_shape: HashMap<String, Shape> = HashMap::new();
    // From module contents
    for t in &module_contents.traits {
        name_to_shape.insert(t.name.clone(), Shape::Trait);
    }
    for s in &module_contents.structs {
        name_to_shape.insert(s.name.clone(), Shape::Struct);
    }
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            name_to_shape.insert(c.name.clone(), c.shape);
            // Also index children recursively
            fn index_children(c: &Construct, map: &mut HashMap<String, Shape>) {
                for child in &c.children {
                    map.insert(child.name.clone(), child.shape);
                    index_children(child, map);
                }
            }
            index_children(c, &mut name_to_shape);
        }
    }
    // Layer `declare` traits/structs don't appear in registry.constructs —
    // parse declaration source so name→shape still sees them.
    for item in parse_layer_declare_items(registry) {
        if let TopLevelItem::Construct(c) = item {
            name_to_shape.insert(c.name, c.shape);
        }
    }

    // INV-003: JSON envelope routing is opt-in via layer routing traits +
    // step context refs. Packages without routing stay direct-call.
    let has_ctx_refs = flows.iter().any(|flow| {
        let steps = match flow {
            FlowLike::Flow(f) => &f.steps,
            FlowLike::Construct(c) => &c.steps,
        };
        steps.iter().any(|s| {
            if let FlowStep::Step(sd) = s {
                !sd.refs.is_empty()
            } else {
                false
            }
        })
    });
    let envelope_routing = has_ctx_refs && !registry.routing_traits().is_empty();

    // With envelope routing, only routing traits are direct deps — other
    // cross-boundary calls go through the message-routing port.
    let mut effective_name_to_shape = name_to_shape.clone();
    if envelope_routing {
        let routing = registry.routing_traits();
        // Remove all non-routing traits from the shape map so they don't become direct deps
        effective_name_to_shape.retain(|name, shape| {
            *shape != Shape::Trait || routing.contains(name)
        });
    }

    // Shared trait → Deps field map (application + harness + port-call lowering).
    let flow_constructs: Vec<&Construct> = flows
        .iter()
        .filter_map(|f| match f {
            FlowLike::Construct(c) => Some(*c),
            FlowLike::Flow(_) => None,
        })
        .collect();
    // Core Flow nodes aren't Constructs — fold their inputs/steps via the
    // same collection logic by synthesizing from FlowLike below.
    let (mut all_deps, mut dep_field_names) =
        collect_deps_field_map(&flow_constructs, registry, &effective_name_to_shape);
    let mut base_ctx = build_ctx_from_solution(solution, effective_name_to_shape.clone(), registry);
    // Types this crate can name (`use crate::domain::types::*`).
    for s in &module_contents.structs {
        base_ctx.local_domain_types.insert(s.name.clone());
    }
    for e in &module_contents.enums {
        base_ctx.local_domain_types.insert(e.name.clone());
    }
    for flow in flows {
        let (steps, inputs) = match flow {
            FlowLike::Flow(f) => (&f.steps, &f.inputs),
            FlowLike::Construct(_) => continue, // already in collect_deps_field_map
        };
        all_deps.extend(collect_deps(steps, &base_ctx));
        for field in inputs {
            if registry.field_is_dependency(field) {
                let trait_name = match &field.type_expr {
                    TypeExpr::Named(type_name) => type_name.clone(),
                    TypeExpr::Generic(base, _) => base.clone(),
                    _ => continue,
                };
                all_deps.insert(trait_name.clone());
                dep_field_names
                    .entry(trait_name)
                    .or_insert_with(|| to_snake(&field.name));
            }
        }
        for step in steps {
            if let FlowStep::Step(s) = step {
                for expr in &s.body {
                    scan_dep_calls(
                        expr,
                        &effective_name_to_shape,
                        &mut all_deps,
                        &mut dep_field_names,
                    );
                }
            }
        }
    }
    for t in &all_deps {
        dep_field_names
            .entry(t.clone())
            .or_insert_with(|| to_snake(t));
    }

    // Generate Deps struct: declared `deps` construct wins (INV-001 authored type).
    if let Some(decl) = deps_decl {
        out.push_str(&format!(
            "/// Declared dependency bundle (`deps {}`).\npub struct {} {{\n",
            decl.type_name, decl.type_name
        ));
        for f in &decl.fields {
            out.push_str(&format!(
                "    pub {}: std::sync::Arc<dyn {} + Send + Sync>,\n",
                f.name, f.trait_name
            ));
        }
        out.push_str("}\n\n");
        if decl.type_name != "Deps" {
            out.push_str(&format!("pub type Deps = {};\n\n", decl.type_name));
        }
    } else if !all_deps.is_empty() {
        out.push_str("/// Injected dependencies (ports).\npub struct Deps {\n");
        let mut sorted: Vec<&String> = all_deps.iter().collect();
        sorted.sort();
        for trait_name in sorted {
            let field_name = dep_field_names
                .get(trait_name)
                .cloned()
                .unwrap_or_else(|| to_snake(trait_name));
            out.push_str(&format!(
                "    pub {}: std::sync::Arc<dyn {} + Send + Sync>,\n",
                field_name, trait_name
            ));
        }
        out.push_str("}\n\n");
    }

    // DomainService twins for ApplicationService (handler) collapse.
    // Message key = bus_policy strip (e.g. HandleGetX → GetX) → domain construct.
    let mut domain_by_message: std::collections::HashMap<String, &Construct> =
        std::collections::HashMap::new();
    // Filled when domain fns are emitted so thin wrappers share exact signatures.
    let mut domain_ret_by_message: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for flow in flows {
        if let FlowLike::Construct(c) = flow {
            let is_domain = registry.construct_in_group(&c.keyword, "domain")
                || registry.construct_in_group(&c.subkind, "domain");
            if is_domain {
                let msg = registry.bus_message_name(&c.name);
                domain_by_message.insert(msg, c);
            }
        }
    }

    for flow in flows {
        // ─── Construct lowers_to: template takes full control ──────────────
        if let FlowLike::Construct(c) = flow {
            if let Some(template) = registry.construct_lowers_to(c, "rust") {
                let rendered = crate::rust::interpolate_construct_template(template, c, registry);
                out.push_str(&rendered);
                out.push_str("\n\n");
                continue;
            }
        }

        let (name, subkind, annotations, inputs, steps, keyword) = match flow {
            FlowLike::Flow(f) => (
                &f.name,
                "Flow",
                &f.annotations,
                &f.inputs,
                &f.steps,
                "flow",
            ),
            FlowLike::Construct(c) => (
                &c.name,
                c.subkind.as_str(),
                &c.annotations,
                &c.inputs,
                &c.steps,
                c.keyword.as_str(),
            ),
        };

        // Get return_expr handling the Box difference
        let return_expr: Option<&Expr> = match flow {
            FlowLike::Flow(f) => f.return_expr.as_ref(),
            FlowLike::Construct(c) => c.return_expr.as_deref(),
        };

        // Does the construct's layer declare a runtime binding (e.g. `saga`
        // delegating to `run_saga`)? If so, steps are packaged into trait impls
        // and handed to the coordinator — the engine names nothing saga-specific.
        let runtime = registry.construct_by_name(subkind).and_then(|c| c.runtime.clone());

        out.push_str(&format!("/// {}: {}\n", subkind, name));
        for ann in annotations {
            out.push_str(&format!("/// @{}\n", ann.name));
        }

        let params = inputs
            .iter()
            .filter(|f| !registry.field_is_dependency(f))
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>()
            .join(",\n    ");

        // Determine if we need deps parameter — dependency-role inputs (INV-001)
        let dep_inputs: Vec<&Field> = inputs
            .iter()
            .filter(|f| registry.field_is_dependency(f))
            .collect();
        let flow_deps = collect_deps(steps, &base_ctx);
        // Also check construct-level @dep(name: Type) annotations
        let has_annotation_deps = annotations.iter().any(|a| registry.is_dependency_annotation(&a.name));
        let has_deps = !flow_deps.is_empty() || !dep_inputs.is_empty() || has_annotation_deps;
        let deps_param = if has_deps { "deps: &Deps, " } else { "" };

        // ApplicationService with a DomainService twin → thin delegate (no 2× body).
        // Skip delegation when the handler has extra steps (e.g. authorize) that the
        // domain service doesn't — emit full body so macro-expanded auth runs.
        let is_app = registry.construct_in_group(keyword, "application")
            || registry.construct_in_group(subkind, "application");
        if is_app && runtime.is_none() {
            let msg = registry.bus_message_name(name);
            if let Some(domain_c) = domain_by_message.get(&msg) {
                // Only delegate if step counts match — extra steps (authorize, etc.)
                // mean the handler has its own logic to emit.
                let domain_step_count = domain_c.steps.len();
                let handler_step_count = steps.len();
                if handler_step_count <= domain_step_count {
                let domain_fn = to_snake(&domain_c.name);
                let call_args: Vec<String> = inputs
                    .iter()
                    .filter(|f| !registry.field_is_dependency(f))
                    .map(|f| to_snake(&f.name))
                    .collect();
                // Prefer return type recorded when domain twin was emitted.
                let ret_type = domain_ret_by_message
                    .get(&msg)
                    .cloned()
                    .unwrap_or_else(|| "Result<(), DomainError>".to_string());
                let deps_arg = if has_deps { "deps, " } else { "" };
                let rest = call_args.join(", ");
                // fn_attrs: layer-driven (e.g. "pub async" from tokio.layer).
                // Engine fallback: plain "pub" (sync).
                let fn_mod = layer_fn_attrs.unwrap_or("pub");
                let await_suffix = if fn_mod.contains("async") { ".await" } else { "" };
                out.push_str(&format!(
                    "#[tracing::instrument(skip_all)]\n{fn_mod} fn {}(\n    {}{}\n) -> {} {{\n\
                        // Thin HTTP/application surface — delegates to domain `{domain_fn}`.\n\
                        {domain_fn}({deps_arg}{rest}){await_suffix}\n}}\n\n",
                    to_snake(name),
                    deps_param,
                    params,
                    ret_type,
                ));
                // Append inline template contributions for this fn.
                if let Some(inline) = crate::template::compose_inline(template_output, name) {
                    out.push_str(&inline);
                    out.push_str("\n\n");
                }
                continue;
                } // handler_step_count <= domain_step_count
            }
        }

        // Build context for this flow
        let mut ctx = build_ctx_from_solution(solution, effective_name_to_shape.clone(), registry);
        ctx.routing.envelope_routing = envelope_routing;
        if envelope_routing && ctx.routing.routing_ref.is_empty() {
            ctx.routing.routing_ref = ctx.default_routing_ref_as_dep();
        }
        ctx.dep_fields = dep_field_names.clone();
        ctx.local_domain_types = base_ctx.local_domain_types.clone();
        ctx.routing.bus_returns = base_ctx.routing.bus_returns.clone();
        // Register inputs as locals, with their declared types for inference.
        // Skip dependency-role inputs — accessed via deps.x, not as locals.
        for input in inputs {
            if registry.field_is_dependency(input) {
                // Register the dep field name as Trait so calls route through deps.x
                ctx.name_to_shape.insert(input.name.clone(), Shape::Trait);
                // Also register the type name (PascalCase) so macro-expanded code
                // that references services by type (e.g. CheckScope.check!) routes correctly.
                let type_name = match &input.type_expr {
                    veil_ir::TypeExpr::Named(n) => Some(n.clone()),
                    veil_ir::TypeExpr::Generic(base, _) => Some(base.clone()),
                    _ => None,
                };
                if let Some(tn) = type_name {
                    ctx.name_to_shape.insert(tn, Shape::Trait);
                }
                continue;
            }
            ctx.locals.insert(input.name.clone());
            ctx.types.local_types.insert(input.name.clone(), type_to_rust(&input.type_expr));
        }
        // For DomainService flows: register step-level dep call targets as Trait
        // and copy method_returns / method_params so Option<T> pass-through works.
        for (trait_name, field_name) in &dep_field_names {
            if !ctx.name_to_shape.contains_key(field_name) {
                ctx.name_to_shape.insert(field_name.clone(), Shape::Trait);
            }
            // Copy method_returns from PascalCase trait to the field name
            let mut extra: Vec<((String, String), String)> = Vec::new();
            for ((tn, mn), ret) in &ctx.types.method_returns {
                if tn == trait_name {
                    extra.push(((field_name.clone(), mn.clone()), ret.clone()));
                    let clean = mn.trim_end_matches('!').to_string();
                    if clean != *mn {
                        extra.push(((field_name.clone(), clean), ret.clone()));
                    }
                }
            }
            for (k, v) in extra {
                ctx.types.method_returns.entry(k).or_insert(v);
            }
            // Copy method_params so call-site Option args are not auto-unwrapped
            // when the port expects Option (e.g. list_by_repo status: Opt<…>).
            let mut extra_params: Vec<((String, String), Vec<String>)> = Vec::new();
            for ((tn, mn), params) in &ctx.types.method_params {
                if tn == trait_name {
                    extra_params.push(((field_name.clone(), mn.clone()), params.clone()));
                    let clean = mn.trim_end_matches('!').to_string();
                    if clean != *mn {
                        extra_params.push(((field_name.clone(), clean), params.clone()));
                    }
                }
            }
            for (k, v) in extra_params {
                ctx.types.method_params.entry(k).or_insert(v);
            }
        }

        if let Some(rt) = &runtime {
            // Runtime-delegated construct: emit the step impls + a body that
            // builds the step list and calls the coordinator.
            emit_runtime_delegated(&mut out, name, inputs, steps, rt, deps_param, solution, registry, &ctx, layer_fn_attrs);
            // Append inline template contributions for this fn.
            if let Some(inline) = crate::template::compose_inline(template_output, name) {
                out.push_str(&inline);
                out.push_str("\n\n");
            }
            continue;
        }

        // Infer the flow's return type from the returned expression, using a
        // pre-scan of step bodies so local bindings resolve. Falls back to
        // Result<(), _> when there's no return or the type is unknown.
        // First check if the construct/flow has an explicit return_type declared.
        let explicit_return = match flow {
            FlowLike::Flow(_) => None,
            FlowLike::Construct(c) => c.return_type.as_ref(),
        };
        let ret_type = if let Some(rt) = explicit_return {
            let inner = type_to_rust(rt);
            if inner.starts_with("Result<") { inner } else { format!("Result<{}, DomainError>", inner) }
        } else {
            infer_flow_return_type(return_expr, steps, &ctx, envelope_routing)
        };

        // Record domain return types for thin ApplicationService wrappers.
        let is_domain_emit = registry.construct_in_group(keyword, "domain")
            || registry.construct_in_group(subkind, "domain");
        if is_domain_emit {
            domain_ret_by_message.insert(registry.bus_message_name(name), ret_type.clone());
        }

        out.push_str(&format!(
            "#[tracing::instrument(skip_all)]\n{} fn {}(\n    {}{}\n) -> {} {{\n",
            layer_fn_attrs.unwrap_or("pub"),
            to_snake(name),
            deps_param,
            params,
            ret_type
        ));

        // GEN-010: only `let mut` when the binding is actually mutated later.
        ctx.ownership.mut_locals = crate::expr::analyze_mut_locals_in_steps(steps);
        ctx.ownership.ident_uses = crate::expr::count_ident_uses_in_steps(steps);

        for step in steps {
            match step {
                FlowStep::Step(s) => {
                    out.push_str(&format!("    // step: {}\n", s.name));
                    for expr in &s.body {
                        out.push_str(&stmt_to_rust(expr, &mut ctx));
                        out.push('\n');
                    }
                    out.push('\n');
                }
                FlowStep::Parallel(par) => {
                    out.push_str("    // parallel execution\n");
                    out.push_str("    tokio::join!(\n");
                    for s in &par.steps {
                        let branch: Vec<String> = s.body.iter()
                            .map(|e| expr_to_rust(e, &ctx))
                            .collect();
                        out.push_str(&format!(
                            "        async {{ {} }},\n",
                            branch.iter().map(|b| format!("let _ = {};", b)).collect::<Vec<_>>().join(" ")
                        ));
                    }
                    out.push_str("    );\n\n");
                }
                FlowStep::Match(m) => {
                    let match_expr = Expr::Match(Box::new(m.expr.clone()), m.arms.clone());
                    out.push_str(&format!("    {}\n\n", expr_to_rust(&match_expr, &ctx)));
                }
            }
        }

        // Return expression
        if let Some(ret) = return_expr {
            out.push_str(&format!("    Ok({})\n", expr_to_rust(ret, &ctx)));
        } else {
            // Only emit Ok(()) if no step body contains an explicit `ret`
            let has_return_in_body = steps.iter().any(|s| {
                if let FlowStep::Step(sd) = s {
                    sd.body.iter().any(expr_contains_return)
                } else { false }
            });
            if !has_return_in_body {
                out.push_str("    Ok(())\n");
            }
        }
        out.push_str("}\n\n");
        // Append inline template contributions for this fn (emit without emit_to/emit_file).
        if let Some(inline) = crate::template::compose_inline(template_output, name) {
            out.push_str(&inline);
            out.push_str("\n\n");
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/application/mod.rs", crate_name),
        content: out,
    }
}

/// Emit a runtime-delegated construct: one `struct` + trait impl per step, then
/// a function body that builds the boxed step list and calls the layer-declared
/// coordinator. Keys entirely off the `RuntimeBinding` and step-trait method
/// signatures from the layer — no domain vocabulary.
pub fn emit_runtime_delegated(
    out: &mut String,
    name: &str,
    inputs: &[Field],
    steps: &[FlowStep],
    rt: &veil_ir::layer::RuntimeBinding,
    deps_param: &str,
    solution: &Solution,
    registry: &LayerRegistry,
    ctx: &crate::expr::GenCtx,
    layer_fn_attrs: Option<&str>,
) {
    let step_trait = &rt.step_trait;
    // Capture the construct's inputs on each step struct so step bodies can use
    // them. Fields are cloned into the struct at construction.
    // Skip @dep inputs — they're handled via dep_fields with proper Arc<dyn> wrapping.
    let input_fields: Vec<(String, String)> = inputs
        .iter()
        .filter(|f| !registry.field_is_dependency(f))
        .map(|f| (to_snake(&f.name), type_to_rust(&f.type_expr)))
        .collect();

    // Dep port fields: each step struct also captures the Arc'd port deps so
    // step bodies can call `self.customer_repo.save(...)` etc.
    // Skip dep fields whose name collides with an input field (e.g. `@dep bus: Bus`
    // is already captured as an input).
    let input_names: std::collections::HashSet<&str> = input_fields.iter().map(|(n, _)| n.as_str()).collect();
    let mut dep_fields: Vec<(String, String)> = ctx
        .dep_fields
        .iter()
        .filter(|(_, field_name)| !input_names.contains(field_name.as_str()))
        .map(|(trait_name, field_name)| {
            (field_name.clone(), format!("std::sync::Arc<dyn {} + Send + Sync>", trait_name))
        })
        .collect();
    dep_fields.sort_by(|a, b| a.0.cmp(&b.0));

    // A trait method threads state iff the layer declares it returning a payload
    // (`Res!<T>` → Result<T, _>); a payload-less `Res!` method takes state
    // read-only. This keeps codegen keyed off the layer, not a hardcoded name.
    let step_trait_construct = find_construct_by_name(solution, step_trait);
    let method_returns_state = |method: &str| -> bool {
        step_trait_construct
            .and_then(|t| t.methods.iter().find(|m| m.name == method))
            .map(|m| matches!(&m.return_type, Some(TypeExpr::Result(Some(_)))))
            .unwrap_or(false)
    };
    let lookup_method = |method: &str| -> Option<&veil_ir::ast::Method> {
        step_trait_construct.and_then(|t| t.methods.iter().find(|m| m.name == method))
    };

    // Trait names in scope for param rendering (step trait + routing + any
    // named traits the step methods reference).
    let mut trait_names: std::collections::HashSet<String> = ctx.routing.routing_traits.clone();
    trait_names.insert(step_trait.clone());
    if let Some(tc) = step_trait_construct {
        for m in &tc.methods {
            for p in &m.params {
                if let TypeExpr::Named(n) = &p.type_expr
                    && n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                        // Candidate trait/type name — only box known traits.
                        if ctx.routing.routing_traits.contains(n) || n == step_trait {
                            trait_names.insert(n.clone());
                        }
                    }
            }
        }
    }

    // Every let-binding across ALL step bodies is a shared state key, so a
    // later step can read an earlier step's result.
    let mut state_locals: std::collections::HashSet<String> = std::collections::HashSet::new();
    for step in steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                if let Expr::Assign(n, _, _) | Expr::MutAssign(n, _, _) = expr
                    && !n.contains('.') {
                        state_locals.insert(n.clone());
                    }
            }
        }
    }

    // Routing param name from the step trait's first method that names a
    // routing trait (e.g. `bus: Bus` → `"bus"`). Falls back to snake_case of
    // the primary routing trait.
    let routing_param = lookup_method("action")
        .or_else(|| step_trait_construct.and_then(|t| t.methods.first()))
        .and_then(|m| {
            m.params.iter().find_map(|p| {
                if let TypeExpr::Named(ty) = &p.type_expr
                    && ctx.routing.routing_traits.contains(ty) {
                        return Some(to_snake(&p.name));
                    }
                None
            })
        })
        .or_else(|| ctx.primary_routing_trait().map(to_snake))
        .unwrap_or_default();

    let use_envelope = !ctx.routing.routing_traits.is_empty();

    // One struct + impl per Step (skip par/match — delegated runtimes use
    // plain steps).
    for (i, step) in steps.iter().enumerate() {
        let FlowStep::Step(s) = step else { continue };
        let type_name = format!("{}Step{}", name, i);

        // Struct holding captured inputs.
        out.push_str(&format!("/// Step `{}` of `{}` (impl {}).\nstruct {} {{\n", s.name, name, step_trait, type_name));
        for (fname, ftype) in &input_fields {
            out.push_str(&format!("    {}: {},\n", fname, ftype));
        }
        for (fname, ftype) in &dep_fields {
            out.push_str(&format!("    {}: {},\n", fname, ftype));
        }
        out.push_str("}\n\n");

        // Step body ctx: inputs are `self.<field>`; routing trait is the injected
        // param from the step-trait signature; cross-step locals live in threaded state.
        let mut step_ctx = ctx.clone_for_inference();
        step_ctx.locals.clear(); // Step body starts fresh — inputs are self_fields, not locals.
        step_ctx.routing.envelope_routing = use_envelope;
        step_ctx.routing.routing_ref = routing_param.clone();
        step_ctx.in_method = true; // input idents render as self.<field>
        for (fname, ftype) in &input_fields {
            step_ctx.self_fields.insert(fname.clone());
            step_ctx.types.local_types.insert(fname.clone(), ftype.clone());
        }
        for (fname, ftype) in &dep_fields {
            step_ctx.self_fields.insert(fname.clone());
            step_ctx.types.local_types.insert(fname.clone(), ftype.clone());
        }
        step_ctx.state_locals = state_locals.clone();

        out.push_str(&format!("#[async_trait::async_trait]\nimpl {} for {} {{\n", step_trait, type_name));

        // The main body fills `action` (returns updated state); each sub-block
        // fills its mapped method. Signatures come from the layer step trait.
        emit_step_method(
            out,
            "action",
            &s.body,
            method_returns_state("action"),
            lookup_method("action"),
            &trait_names,
            &step_ctx,
        );
        for block in &s.sub_blocks {
            if let Some((_, method)) = rt.method_map.iter().find(|(kw, _)| kw == &block.keyword) {
                emit_step_method(
                    out,
                    method,
                    &block.body,
                    method_returns_state(method),
                    lookup_method(method),
                    &trait_names,
                    &step_ctx,
                );
            }
        }
        out.push_str("}\n\n");
    }

    // The delegated function: build the step list and call the coordinator.
    let params = inputs
        .iter()
        .filter(|f| !registry.field_is_dependency(f))
        .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
        .collect::<Vec<_>>()
        .join(", ");
    let fn_mod = layer_fn_attrs.unwrap_or("pub");
    let await_suffix = if fn_mod.contains("async") { ".await" } else { "" };
    out.push_str(&format!(
        "#[tracing::instrument(skip_all)]\n{fn_mod} fn {}({}{}) -> Result<(), DomainError> {{\n",
        to_snake(name),
        deps_param,
        params,
    ));
    out.push_str(&format!("    let steps: Vec<Box<dyn {} + Send + Sync>> = vec![\n", step_trait));
    for (i, step) in steps.iter().enumerate() {
        if !matches!(step, FlowStep::Step(_)) { continue; }
        let type_name = format!("{}Step{}", name, i);
        let mut ctor_parts: Vec<String> = input_fields
            .iter()
            .map(|(fname, _)| format!("{}: {}.clone()", fname, fname))
            .collect();
        for (fname, _) in &dep_fields {
            ctor_parts.push(format!("{}: deps.{}.clone()", fname, fname));
        }
        let ctor_args = ctor_parts.join(", ");
        out.push_str(&format!("        Box::new({} {{ {} }}),\n", type_name, ctor_args));
    }
    out.push_str("    ];\n");
    // Coordinator args follow the layer-declared fn. A routing-trait first
    // argument is only passed when a loaded layer actually declared one.
    let coord = to_snake(&rt.coordinator);
    match ctx.primary_routing_trait() {
        Some(t) => out.push_str(&format!(
            "    {coord}(deps.{}.as_ref(), &steps){await_suffix}\n",
            to_snake(t)
        )),
        None => out.push_str(&format!("    {coord}(&steps){await_suffix}\n")),
    }
    out.push_str("}\n\n");
}

/// Emit one step-trait method impl with a translated body.
/// Parameter list and types are taken from the layer-declared step trait method
/// (not hardcoded). Value-typed params (e.g. `Json`) are `mut` so step bodies
/// can reassign threaded state; trait params are shared references.
pub fn emit_step_method(
    out: &mut String,
    method: &str,
    body: &[Expr],
    returns_state: bool,
    step_method: Option<&veil_ir::ast::Method>,
    trait_names: &std::collections::HashSet<String>,
    base_ctx: &crate::expr::GenCtx,
) {
    let (params_str, ret_inner) = if let Some(m) = step_method {
        let params: Vec<String> = m
            .params
            .iter()
            .map(|p| {
                let ty = param_type_to_rust(&p.type_expr, trait_names);
                // Threaded JSON state bags need `mut` so the body can reassign.
                let mut_kw = if matches!(&p.type_expr, TypeExpr::Named(n) if n == "Json") {
                    "mut "
                } else {
                    ""
                };
                format!("{}{}: {}", mut_kw, to_snake(&p.name), ty)
            })
            .collect();
        let ret = match &m.return_type {
            Some(TypeExpr::Result(Some(inner))) => type_to_rust_with_traits(inner, trait_names),
            Some(TypeExpr::Result(None)) | None => "()".to_string(),
            Some(other) => type_to_rust_with_traits(other, trait_names),
        };
        (params.join(", "), ret)
    } else {
        // Fallback when the step trait is missing from the solution (should not
        // happen when layers inject declare blocks).
        let ret = if returns_state {
            "serde_json::Value".to_string()
        } else {
            "()".to_string()
        };
        (String::new(), ret)
    };

    let sep = if params_str.is_empty() { "" } else { ", " };
    out.push_str(&format!(
        "    async fn {}(&self{}{}) -> Result<{}, DomainError> {{\n",
        method, sep, params_str, ret_inner
    ));
    let mut ctx = base_ctx.clone_for_inference();
    ctx.ownership.mut_locals = crate::expr::analyze_mut_locals(body);
    ctx.ownership.ident_uses = crate::expr::count_ident_uses(body);
    for expr in body {
        let stmt = crate::expr::stmt_to_rust(expr, &mut ctx);
        let stmt = stmt.trim_start();
        out.push_str(&format!("        {stmt}\n"));
        if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = expr
            && !name.contains('.') {
                ctx.locals.insert(name.clone());
            }
    }
    if returns_state {
        // Return the threaded state param if present; else unit Ok.
        let state_name = step_method
            .and_then(|m| {
                m.params.iter().rev().find_map(|p| {
                    if matches!(&p.type_expr, TypeExpr::Named(n) if n == "Json") {
                        Some(to_snake(&p.name))
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_else(|| "state".to_string());
        out.push_str(&format!("        Ok({})\n    }}\n", state_name));
    } else {
        out.push_str("        Ok(())\n    }\n");
    }
}

/// Detect which sibling modules a module's flows reference (via step ctx refs).
pub fn detect_sibling_refs(module: &Construct, solution: &Solution) -> Vec<String> {
    let mut needed = std::collections::HashSet::new();
    let module_names: std::collections::HashMap<String, String> = solution.items.iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some((c.name.clone(), module_crate_name(c, solution))),
            _ => None,
        }).collect();

    fn scan_refs(c: &Construct, module_names: &std::collections::HashMap<String, String>, needed: &mut std::collections::HashSet<String>) {
        // Construct-level refs (e.g. `contexts Identity, Billing` on a saga)
        for r in &c.refs {
            for val in &r.values {
                if let Some(crate_name) = module_names.get(val) {
                    needed.insert(crate_name.clone());
                }
            }
        }
        for step in &c.steps {
            if let FlowStep::Step(s) = step {
                for r in &s.refs {
                    // ctx ref like "ctx Identity" → need the identity crate
                    for val in &r.values {
                        if let Some(crate_name) = module_names.get(val) {
                            needed.insert(crate_name.clone());
                        }
                    }
                }
            }
        }
        for child in &c.children {
            scan_refs(child, module_names, needed);
        }
    }
    scan_refs(module, &module_names, &mut needed);
    needed.into_iter().collect()
}
// ─── Helper functions ─────────────────────────────────────────────────────

