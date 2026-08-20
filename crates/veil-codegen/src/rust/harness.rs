use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;

/// True if expr tree contains a trait dependency call that requires a Deps parameter.
/// Matches `TraitName.method!(…)` / dep-local calls; ignores `Type.new(...)` constructors.
pub fn expr_mentions_trait_dep(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => {
            let method = call.method.trim_end_matches(['!', '?']);
            // Skip constructors (Type.new / Type{})
            let is_ctor = method.is_empty() || method == "new";
            if !is_ctor && !call.method.is_empty() {
                let t = call.target.as_str();
                // Language primitives are not trait dep calls
                let is_lang = matches!(t, "Dt" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "UUID" | "Int" | "Float" | "Bool");
                // Trait dep calls: PascalCase target, or snake_case @dep local ending in _repo/_port/_svc
                let pascal = t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
                let dep_local = t.ends_with("_repo")
                    || t.ends_with("_port")
                    || t.ends_with("_svc")
                    || t.ends_with("_client");
                // Bang-suffix methods (method!) are always trait dep calls
                let is_bang = call.method.ends_with('!');
                if !is_lang && (pascal || dep_local || (is_bang && !t.is_empty())) {
                    return true;
                }
            }
            if let Some(recv) = &call.receiver
                && expr_mentions_trait_dep(recv) {
                    return true;
                }
            call.args.iter().any(expr_mentions_trait_dep)
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => expr_mentions_trait_dep(rhs),
        Expr::Return(inner) | Expr::Try(inner) | Expr::Require(inner) | Expr::Await(inner) | Expr::UnaryOp(UnaryOpExpr { expr: inner, .. }) => {
            expr_mentions_trait_dep(inner)
        }
        Expr::BinaryOp(b) => {
            expr_mentions_trait_dep(&b.left) || expr_mentions_trait_dep(&b.right)
        }
        Expr::IfExpr(i) => {
            expr_mentions_trait_dep(&i.condition)
                || i.then_body.iter().any(expr_mentions_trait_dep)
                || i.else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_mentions_trait_dep))
                    .unwrap_or(false)
        }
        Expr::ArrayLit(items) => items.iter().any(expr_mentions_trait_dep),
        Expr::Match(scrutinee, arms) => {
            expr_mentions_trait_dep(scrutinee)
                || arms.iter().any(|a| a.body.iter().any(expr_mentions_trait_dep))
        }
        Expr::ForLoop { iterable, body, .. } | Expr::WhileLoop { condition: iterable, body } => {
            expr_mentions_trait_dep(iterable) || body.iter().any(expr_mentions_trait_dep)
        }
        Expr::Action(_) => true, // invoke/request layer actions always need Bus dep
        _ => false,
    }
}

/// Check if any expression in a tree references `self.<field_name>`.
pub fn expr_mentions_self_field(expr: &Expr, field_name: &str) -> bool {
    match expr {
        Expr::Call(call) => {
            let target_matches = call.target == format!("self.{}", field_name)
                || (call.target.starts_with("self.") && call.target.split('.').nth(1) == Some(field_name));
            if target_matches {
                return true;
            }
            if let Some(recv) = &call.receiver
                && expr_mentions_self_field(recv, field_name) {
                    return true;
                }
            call.args.iter().any(|a| expr_mentions_self_field(a, field_name))
        }
        Expr::FieldAccess(base, field) => {
            if field == field_name
                && let Expr::Ident(id) = base.as_ref() {
                    return id == "self";
                }
            expr_mentions_self_field(base, field_name)
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => {
            expr_mentions_self_field(rhs, field_name)
        }
        Expr::Return(inner) => expr_mentions_self_field(inner, field_name),
        _ => false,
    }
}

pub fn package_has_declared_endpoints(sol: &Solution, registry: &LayerRegistry) -> bool {
    fn walk(c: &Construct, registry: &LayerRegistry) -> bool {
        if registry.construct_has_role(c, "http_endpoint") {
            return true;
        }
        c.children.iter().any(|ch| walk(ch, registry))
    }
    sol.items.iter().any(|i| match i {
        TopLevelItem::Construct(c) => walk(c, registry),
        _ => false,
    })
}

pub fn package_has_main_annotation(sol: &Solution, registry: &LayerRegistry) -> bool {
    fn walk(c: &Construct, registry: &LayerRegistry) -> bool {
        if registry.construct_has_main(c) {
            return true;
        }
        c.children.iter().any(|ch| walk(ch, registry))
            || c.fns.iter().any(|f| {
                f.annotations
                    .iter()
                    .any(|a| registry.is_main_annotation(&a.name))
            })
    }
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) if walk(c, registry) => return true,
            TopLevelItem::Function(f)
                if f.annotations
                    .iter()
                    .any(|a| registry.is_main_annotation(&a.name)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// RT-001/003/004: working local harness main — InProcessBus + first app svc.
/// CAP-002 / CAP-006: `@main` + `link veil_server` → ProductHost listen.
pub fn gen_product_host_main(sol: &Solution, handler_names: &[String]) -> String {
    let _ = handler_names;
    format!(
        r#"//! Generated product host for package `{pkg}` (CAP-002/006).
//! Uses `veil_server::ProductHost` for IDE multi + SPA + config.
//! `cargo run -p veil_bin` from the generated workspace root.

use veil_server::{{resolve_static_dir, ProductHost}};
use veil_shared::register_all;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {{
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let port: u16 = std::env::var("VEIL_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    let non_interactive = std::env::var_os("CI").is_some()
        || std::env::var_os("VEIL_NONINTERACTIVE").is_some();
    let static_dir = resolve_static_dir(None);

    // CAP-003: register generated handler names (dispatch is host/platform).
    let mut n = 0usize;
    register_all(|_name| n += 1);
    tracing::info!("veil_bin: {{n}} handlers from register_all");

    ProductHost::new()
        .port(port)
        .static_dir(static_dir)
        .ensure_config(non_interactive)?
        .listen()
        .await?;
    Ok(())
}}
"#,
        pkg = sol.name
    )
}

/// Whether a stub contributes Cargo deps / workspace entries (not a hollow parse).
pub fn stub_is_active_cargo(stub: &veil_ir::StubCrate) -> bool {
    !stub.row_type_derives.is_empty()
        || !stub.wrapper_type_derives.is_empty()
        || !stub.cargo_features.is_empty()
        || !stub.cargo_deps.is_empty()
        || !stub.codegen_imports.is_empty()
        || !stub.structs.is_empty()
        || !stub.harness_fields.is_empty()
}

/// Resolve a stub type to `(crate_name, path_under_crate)` (e.g. Pool → sqlx, PgPool path).
pub fn split_stub_type_qual(type_name: &str) -> (Option<String>, String) {
    let t = type_name.replace("::", ".");
    if let Some((crate_hint, ty)) = t.rsplit_once('.') {
        (Some(crate_hint.replace('-', "_")), ty.to_string())
    } else {
        (None, t)
    }
}

pub fn stub_defines_type(stub: &veil_ir::StubCrate, type_name: &str) -> bool {
    stub.structs.iter().any(|s| s.name == type_name) || stub.harness_fields.contains_key(type_name)
}

pub fn stub_type_path(registry: &LayerRegistry, type_name: &str) -> Option<(String, String)> {
    let (crate_hint, ty) = split_stub_type_qual(type_name);
    if let Some(hint) = crate_hint {
        for stub in &registry.stubs {
            let cn = stub.name.replace('-', "_");
            if (cn == hint || stub.alias.as_deref().map(|a| a.replace('-', "_")) == Some(hint.clone()))
                && stub_defines_type(stub, &ty)
            {
                return Some((cn, stub.rust_type_path(&ty)));
            }
        }
        return None;
    }
    let mut hits = Vec::new();
    for stub in &registry.stubs {
        if stub_defines_type(stub, &ty) {
            hits.push((stub.name.replace('-', "_"), stub.rust_type_path(&ty)));
        }
    }
    if hits.len() == 1 {
        return Some(hits.remove(0));
    }
    None
}

/// Look up a stub `harness_field Type` recipe. Returns (local_let_name, rust_expr).
///
/// Matches crate-qualified names (`aws_sdk_sns.Client`), unique bare names
/// (`Client` only when a single stub exports it), and use-aliases
/// (`use reqwest as rw` → `RwClient` matches that stub's `harness_field Client`).
pub fn stub_harness_field_expr(
    registry: &LayerRegistry,
    type_name: &str,
) -> Option<(String, String)> {
    let (crate_hint, ty) = split_stub_type_qual(type_name);
    if let Some(hint) = crate_hint {
        for stub in &registry.stubs {
            let cn = stub.name.replace('-', "_");
            let alias_ok = stub
                .alias
                .as_deref()
                .map(|a| a.replace('-', "_") == hint)
                .unwrap_or(false);
            if (cn == hint || alias_ok) && stub.harness_fields.contains_key(&ty) {
                let expr = stub.harness_fields.get(&ty).unwrap();
                let let_name = format!("_stub_{}_{}", hint, to_snake(&ty));
                return Some((let_name, expr.trim().to_string()));
            }
        }
        return None;
    }
    let mut hits: Vec<(String, String)> = Vec::new();
    for stub in &registry.stubs {
        if let Some(expr) = stub.harness_fields.get(type_name) {
            hits.push((
                format!("_stub_{}", to_snake(type_name)),
                expr.trim().to_string(),
            ));
        }
        if let Some(alias) = &stub.alias {
            let cap = alias
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
                .unwrap_or_default()
                + alias.get(1..).unwrap_or("");
            for (key, expr) in &stub.harness_fields {
                if type_name == format!("{cap}{key}") {
                    hits.push((
                        format!("_stub_{}", to_snake(type_name)),
                        expr.trim().to_string(),
                    ));
                }
            }
        }
    }
    if hits.len() == 1 {
        return Some(hits.remove(0));
    }
    None
}

/// Trait / struct names injected by layer `declare` blocks (`Bus`, `SagaStep`, …).
pub fn layer_declared_type_names(registry: &LayerRegistry) -> std::collections::HashSet<String> {
    registry.declared_type_names()
}

/// Rust field for `@env(VAR)`. `DATABASE*` stays `pool`; everything else is
/// the full lowercased name (`TABLE_NAME` → `table_name`).
pub fn env_var_field_name(var: &str) -> String {
    if var.contains("DATABASE") {
        "pool".into()
    } else {
        var.to_ascii_lowercase()
    }
}

/// Apply **every** `@env` annotation on an adapter (not just the first).
/// Two `@env TABLE` + `@env TTL` must both become struct fields.
pub fn apply_adapter_env_field_inits(
    ad: &Construct,
    registry: &LayerRegistry,
    field_inits: &mut std::collections::BTreeMap<String, String>,
) {
    for env_a in ad
        .annotations
        .iter()
        .filter(|a| registry.is_adapter_env_annotation(&a.name))
    {
        for arg in &env_a.args {
            if arg.contains("DATABASE") {
                field_inits.entry("pool".to_string()).or_insert_with(|| {
                    if let Some((_, expr)) = stub_harness_field_expr(registry, "Pool") {
                        expr
                    } else {
                        format!(
                            "std::env::var(\"{arg}\").unwrap_or_else(|_| \"default\".into())"
                        )
                    }
                });
            } else {
                let field_name = env_var_field_name(arg);
                field_inits.entry(field_name).or_insert_with(|| {
                    format!(
                        "std::env::var(\"{arg}\").unwrap_or_else(|_| \"default\".into())"
                    )
                });
            }
        }
    }
}

/// Parse every layer `declare` block into IR. Used so veil_shared always
/// emits layer-declared types/fns even when a product construct reused a name.
pub fn parse_layer_declare_items(registry: &LayerRegistry) -> Vec<TopLevelItem> {
    let mut items = Vec::new();
    for decl_source in &registry.declarations {
        let indented: String = decl_source
            .lines()
            .map(|l| {
                if l.is_empty() {
                    String::new()
                } else {
                    format!("  {l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let wrapped = format!("sol __decl__\n{indented}");
        let tokens = veil_parser::lex(&wrapped);
        let Ok(file) = veil_parser::parse_file_with_registry(&tokens, registry.clone()) else {
            continue;
        };
        let parsed = match file {
            veil_ir::ast::VeilFile::Solution(s) => s.items,
            veil_ir::ast::VeilFile::Package(p) => p.items,
            _ => continue,
        };
        for mut item in parsed {
            match &mut item {
                TopLevelItem::Construct(c) => c.layer_provided = true,
                TopLevelItem::Function(f) => f.layer_provided = true,
                _ => {}
            }
            items.push(item);
        }
    }
    items
}

pub fn layer_declared_fn_names(registry: &LayerRegistry) -> Vec<String> {
    registry.declared_fn_names()
}

pub fn adapter_field_rust_type(
    type_name: &str,
    name_to_shape: &std::collections::HashMap<String, Shape>,
    registry: &LayerRegistry,
) -> String {
    if name_to_shape.get(type_name) == Some(&Shape::Trait) {
        return format!("std::sync::Arc<dyn {type_name} + Send + Sync>");
    }
    if let Some((crate_name, path)) = stub_type_path(registry, type_name) {
        return format!("{crate_name}::{path}");
    }
    veil_field_type_to_rust(type_name)
}

pub fn harness_ctx<'a>(
    ir: &'a veil_ir::HarnessIR,
    crate_name: &str,
    module_name: &str,
) -> Option<&'a veil_ir::HarnessContext> {
    ir.contexts
        .iter()
        .find(|c| c.crate_name == crate_name || c.module_name == module_name)
}

/// Fill missing deps/compose when `compat=auto` so the emitter reads IR only.
/// Endpoints are already synthesized in `lower_harness`.
pub fn apply_compat_synthesis(ir: &mut veil_ir::HarnessIR, sol: &Solution, registry: &LayerRegistry) {
    if ir.compat != veil_ir::CompatMode::Auto {
        return;
    }
    for ctx in &mut ir.contexts {
        let Some(module) = sol.items.iter().find_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod && c.name == ctx.module_name => {
                Some(c)
            }
            _ => None,
        }) else {
            continue;
        };
        let flat = flatten_module(module, registry);
        let name_to_shape = build_name_to_shape(sol, registry);
        let (deps_set, dep_fields) =
            collect_deps_field_map(&flat.fns, registry, &name_to_shape);

        if ctx.deps.is_none() && !deps_set.is_empty() {
            let mut fields: Vec<veil_ir::DepsField> = deps_set
                .iter()
                .map(|t| veil_ir::DepsField {
                    name: dep_fields
                        .get(t)
                        .cloned()
                        .unwrap_or_else(|| to_snake(t)),
                    trait_name: t.clone(),
                })
                .collect();
            fields.sort_by(|a, b| a.trait_name.cmp(&b.trait_name));
            ctx.deps = Some(veil_ir::DepsDecl {
                type_name: "Deps".into(),
                fields,
            });
        }

        if ctx.compose.is_none() {
            let mut wires: Vec<veil_ir::WireDecl> = Vec::new();
            let mut wired_fields: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for ad in &flat.impls {
                if is_pure_generic_adapter_template(ad) {
                    continue;
                }
                let Some(target) = ad.target.as_deref() else {
                    continue;
                };
                let field = adapter_deps_field_name(sol, ad, target, &dep_fields);
                if !dep_fields.is_empty()
                    && !dep_fields.values().any(|v| v == &field)
                    && !dep_fields.contains_key(target)
                {
                    continue;
                }
                if !wired_fields.insert(field.clone()) {
                    continue;
                }
                wires.push(veil_ir::WireDecl {
                    field,
                    kind: veil_ir::WireKind::Adapter {
                        name: ad.name.clone(),
                    },
                });
            }
            if let Some(deps) = &ctx.deps {
                for f in &deps.fields {
                    if wires.iter().any(|w| w.field == f.name) {
                        continue;
                    }
                    if veil_ir::trait_is_provided_runtime(&f.trait_name, registry) {
                        wires.push(veil_ir::WireDecl {
                            field: f.name.clone(),
                            kind: veil_ir::WireKind::ProvidedRuntime,
                        });
                    }
                }
            }
            if !wires.is_empty() {
                ctx.compose = Some(veil_ir::ComposeDecl {
                    name: format!("{}Local", ctx.module_name),
                    bundle: ctx
                        .deps
                        .as_ref()
                        .map(|d| d.type_name.clone())
                        .unwrap_or_else(|| "Deps".into()),
                    wires,
                });
            }
        }
    }
}

pub fn gen_local_harness_main(
    sol: &Solution,
    modules: &[&Construct],
    registry: &LayerRegistry,
    ir: &veil_ir::HarnessIR,
) -> String {
    // ── Pre-scan: free-fn routing imports + whether any handler needs Query ─
    // Axum: only the first method on a path is a free fn (`get(h)`); chained
    // methods are MethodRouter methods (`.post(h)`), so do not import them.
    let mut free_fn_methods: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::from(["get".to_string()]); // /health
    let mut any_query = false;
    for module in modules {
        let flat = flatten_module(module, registry);
        let crate_name_ps = module_crate_name(module, sol);
        let ctx = harness_ctx(ir, &crate_name_ps, &module.name);
        let mut by_path: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        if let Some(ctx) = ctx {
            for ep in &ctx.endpoints {
                let method = ep.method.to_ascii_lowercase();
                let path = ep.path.clone();
                if !seen.insert((method.clone(), path.clone())) {
                    continue;
                }
                by_path.entry(path.clone()).or_default().push(method.clone());
                if ep
                    .binds
                    .iter()
                    .any(|b| matches!(b.source, veil_ir::BindSource::Query))
                {
                    any_query = true;
                }
                if let Some(svc) = flat.fns.iter().find(|s| s.name == ep.handler) {
                    let path_params = path_param_names(&path);
                    if harness_handler_needs_query(svc, registry, &method, &path, &path_params)
                    {
                        any_query = true;
                    }
                }
            }
        }
        for methods in by_path.values() {
            if let Some(first) = methods.first() {
                free_fn_methods.insert(first.clone());
            }
        }
    }
    let routing_imports = free_fn_methods
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let query_import = if any_query {
        "extract::Query, "
    } else {
        ""
    };

    let mut out = String::new();
    out.push_str(&format!(
        "//! HTTP harness for package `{}` (RT-001 / RT-003).\n\
         //! Wires adapters + exposes services as REST endpoints.\n\
         //! `cargo run -p veil_bin` from the generated workspace root.\n\n",
        sol.name
    ));
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use std::sync::Arc;\n");
    out.push_str(&format!(
        "use axum::{{Router, Json, extract::State, {query_import}routing::{{{routing_imports}}}, http::{{HeaderMap, StatusCode}}, middleware::{{from_fn, Next}}, response::Response, extract::Request}};\n"
    ));
    out.push_str("use tower_http::cors::{{Any, CorsLayer}};\n");
    out.push_str("use uuid::Uuid;\n");
    out.push_str("use serde_json::Value;\n");
    out.push_str("use veil_shared::*;\n");
    for m in modules {
        let cn = module_crate_name(m, sol);
        let declared_deps = harness_ctx(ir, &cn, &m.name).and_then(|c| c.deps.as_ref());
        if let Some(deps) = declared_deps {
            if deps.type_name == "Deps" {
                out.push_str(&format!(
                    "use {cn}::application::{{self as {cn}_app, Deps as {cn}_Deps}};\n"
                ));
            } else {
                out.push_str(&format!(
                    "use {cn}::application::{{self as {cn}_app, {} as {cn}_Deps}};\n",
                    deps.type_name
                ));
            }
        } else {
            out.push_str(&format!("use {cn}::application::{{self as {cn}_app}};\n"));
        }
    }
    out.push_str("\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
    out.push_str("    let port: u16 = std::env::var(\"PORT\").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);\n\n");

    // Instantiate InProcessBus only when a context wires a routing trait
    // (ProvidedRuntime) or has declared bus handlers.
    let has_bus = ir.contexts.iter().any(|c| {
        !c.bus_handlers.is_empty()
            || c.compose.as_ref().is_some_and(|co| {
                co.wires
                    .iter()
                    .any(|w| matches!(w.kind, veil_ir::WireKind::ProvidedRuntime))
            })
    });
    if has_bus {
        out.push_str("    let bus = veil_shared::InProcessBus::new();\n\n");
    }

    // Cross-context route uniqueness: first module wins the bare path; later
    // collisions get `/api/{crate}/…` so axum::merge does not panic.
    let mut global_method_path: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    // Remember (crate, deps_var, svc) for bus handler registration after wiring.
    let mut bus_handler_targets: Vec<(String, bool, Construct)> = Vec::new();

    for module in modules {
        let crate_name = module_crate_name(module, sol);
        let flat = flatten_module(module, registry);
        let adapters = &flat.impls;
        let services = &flat.fns;
        if adapters.is_empty() && services.is_empty() {
            continue;
        }

        out.push_str(&format!("    // ── context {} ──\n", module.name));

        // Shared Deps field names must match application crate (dependency-role
        // input names preferred over snake(trait)).
        let name_to_shape = build_name_to_shape(sol, registry);
        let (_deps_set, dep_fields) =
            collect_deps_field_map(services, registry, &name_to_shape);

        // Wire only adapters named on the IR compose (authored or compat-synthesized).
        let mut wired: Vec<(String, String, &Construct)> = Vec::new(); // field, snake, ad
        let mut wired_fields: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut wired_adapter_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let ctx = harness_ctx(ir, &crate_name, &module.name);
        let declared_compose = ctx.and_then(|c| c.compose.as_ref());
        let declared_deps = ctx.and_then(|c| c.deps.as_ref());
        for ad in adapters {
            if is_pure_generic_adapter_template(ad) {
                continue;
            }
            let Some(compose) = declared_compose else {
                continue;
            };
            let named = compose.wires.iter().any(|w| match &w.kind {
                veil_ir::WireKind::Adapter { name } => name == &ad.name,
                _ => false,
            });
            if !named {
                continue;
            }
            if let Some(target) = &ad.target {
                let field = compose
                    .wires
                    .iter()
                    .find(|w| matches!(&w.kind, veil_ir::WireKind::Adapter { name } if name == &ad.name))
                    .map(|w| w.field.clone())
                    .unwrap_or_else(|| adapter_deps_field_name(sol, ad, target, &dep_fields));
                if !wired_fields.insert(field.clone()) {
                    continue;
                }
                wired_adapter_names.insert(ad.name.clone());
                wired.push((field, to_snake(&ad.name), ad));
            }
        }

        // stub harness_field constructors only for wired adapters.
        let mut emitted_harness_lets: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ad in adapters {
            if !wired_adapter_names.contains(&ad.name) {
                continue;
            }
            for ann in &ad.annotations {
                if !registry.is_adapter_field_annotation(&ann.name) {
                    continue;
                }
                for arg in &ann.args {
                    let ftype = arg
                        .split_once(':')
                        .map(|(_, t)| t.trim())
                        .unwrap_or("")
                        .to_string();
                    if ftype.is_empty() || emitted_harness_lets.contains(&ftype) {
                        continue;
                    }
                    if let Some((let_name, expr)) = stub_harness_field_expr(registry, &ftype) {
                        out.push_str(&format!(
                            "    // stub harness_field {ftype}\n\
                             let {let_name} = {expr};\n\n"
                        ));
                        emitted_harness_lets.insert(ftype);
                    }
                }
            }
            // Body may reference self.client without @field — still need Client.
            let body_uses_client = ad.impls.iter().any(|m| {
                m.body
                    .iter()
                    .any(|e| expr_mentions_self_field(e, "client"))
            });
            let has_field_client = ad.annotations.iter().any(|a| {
                registry.is_adapter_field_annotation(&a.name)
                    && a.args
                        .iter()
                        .any(|arg| arg.split_once(':').map(|(n, _)| n.trim()) == Some("client"))
            });
            if body_uses_client
                && !has_field_client
                && !emitted_harness_lets.contains("Client")
                && let Some((let_name, expr)) = stub_harness_field_expr(registry, "Client") {
                    out.push_str(&format!(
                        "    // stub harness_field Client\n\
                         let {let_name} = {expr};\n\n"
                    ));
                    emitted_harness_lets.insert("Client".into());
                }
        }

        // Leaf adapters (stub/env fields only) before orchestrators that @dep on ports.
        let mut adapters_ordered: Vec<&Construct> = adapters.to_vec();
        adapters_ordered.sort_by_key(|ad| {
            ad.fields.iter().any(|f| {
                matches!(&f.type_expr, TypeExpr::Named(n) if n.chars().next().is_some_and(|c| c.is_uppercase()))
            }) as u8
        });
        for ad in adapters_ordered {
            if !wired_adapter_names.contains(&ad.name) {
                continue;
            }
            // Wire adapter fields: @field first, @env only for names not yet set
            // (avoids double-init of `pool` from @field(pool) + @env(DATABASE_URL)).
            let mut field_inits: std::collections::BTreeMap<String, String> =
                std::collections::BTreeMap::new();
            for ann in &ad.annotations {
                if registry.is_adapter_field_annotation(&ann.name) {
                    for arg in &ann.args {
                        let (fname, ftype) = if let Some((n, t)) = arg.split_once(':') {
                            (n.trim().to_string(), t.trim())
                        } else {
                            (arg.trim().to_string(), "String")
                        };
                        let init = if let Some((let_name, _)) =
                            stub_harness_field_expr(registry, ftype)
                        {
                            format!("{let_name}.clone()")
                        } else {
                            harness_string_field_default(&fname, ftype)
                        };
                        field_inits.insert(fname, init);
                    }
                }
            }
            apply_adapter_env_field_inits(ad, registry, &mut field_inits);
            let has_explicit_client_field = field_inits.contains_key("client");
            let body_uses_client = ad.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            });
            if body_uses_client && !has_explicit_client_field
                && let Some((let_name, _)) = stub_harness_field_expr(registry, "Client") {
                    field_inits
                        .entry("client".to_string())
                        .or_insert_with(|| format!("{let_name}.clone()"));
                }
            for f in &ad.fields {
                let field_name = to_snake(&f.name);
                if field_inits.contains_key(&field_name) {
                    continue;
                }
                if let TypeExpr::Named(tn) = &f.type_expr {
                    if let Some(impl_ad) = adapters
                        .iter()
                        .find(|a| a.target.as_deref() == Some(tn.as_str()))
                    {
                        field_inits.insert(
                            field_name,
                            format!("{}_inst.clone()", to_snake(&impl_ad.name)),
                        );
                        continue;
                    }
                    if let Some((let_name, _)) = stub_harness_field_expr(registry, tn) {
                        field_inits.insert(field_name, format!("{let_name}.clone()"));
                        continue;
                    }
                }
                let env_key = f.name.to_uppercase();
                field_inits.insert(
                    field_name,
                    format!(
                        "std::env::var(\"{env_key}\").unwrap_or_else(|_| \"default\".into())"
                    ),
                );
            }
            let mut fields_init = String::new();
            for (fname, init) in &field_inits {
                fields_init.push_str(&format!("        {fname}: {init},\n"));
            }
            let raw_dyn_ty = adapter_dyn_type(sol, ad);
            // Qualify with crate::ports to avoid ambiguity when multiple crates export same trait
            let dyn_ty = format!("{}::ports::{}", crate_name, raw_dyn_ty);
            if fields_init.is_empty() {
                out.push_str(&format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{name}{{}});\n",
                    sn = to_snake(&ad.name),
                    name = ad.name,
                ));
            } else {
                out.push_str(&format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{name} {{\n{fields_init}    }});\n",
                    sn = to_snake(&ad.name),
                    name = ad.name,
                ));
            }
        }

        if services.is_empty() {
            continue;
        }

        // Required Deps fields with no adapter → fail closed with a clear message.
        // provided_runtime wires (Bus / auth / role:runtime_provider) use InProcessBus.
        let mut missing: Vec<String> = Vec::new();
        let mut provided_fields: Vec<String> = Vec::new();
        if let Some(compose) = declared_compose {
            for w in &compose.wires {
                if matches!(w.kind, veil_ir::WireKind::ProvidedRuntime) {
                    provided_fields.push(w.field.clone());
                }
            }
        }
        if let Some(deps) = declared_deps {
            for f in &deps.fields {
                if !wired_fields.contains(&f.name) && !provided_fields.iter().any(|p| p == &f.name) {
                    missing.push(format!("`{}` (trait {})", f.name, f.trait_name));
                }
            }
            if !missing.is_empty() {
                // Greenfield / local smoke: wire generated InMemory{Trait} instead of failing.
                for f in &deps.fields {
                    if wired_fields.contains(&f.name)
                        || provided_fields.iter().any(|p| p == &f.name)
                    {
                        continue;
                    }
                    let inmem = format!("InMemory{}", f.trait_name);
                    let sn = to_snake(&inmem);
                    out.push_str(&format!(
                        "    let {sn}_inst: Arc<dyn {crate_name}::ports::{trait_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{inmem}::new());\n",
                        trait_ty = f.trait_name,
                    ));
                    wired.push((f.name.clone(), sn, module));
                    wired_fields.insert(f.name.clone());
                }
            }
        }
        let has_deps = declared_deps
            .map(|d| !d.fields.is_empty())
            .unwrap_or(!dep_fields.is_empty());
        if has_deps {
            out.push_str(&format!("    let {crate_name}_deps = Arc::new({crate_name}_Deps {{\n"));
            for (field, sn, _) in &wired {
                out.push_str(&format!("        {field}: {sn}_inst.clone(),\n"));
            }
            for bus_field in &provided_fields {
                out.push_str(&format!("        {bus_field}: Arc::new(bus.clone()),\n"));
            }
            out.push_str("    });\n\n");
        }

        // Bus registration: only HarnessIR-declared handlers (deps bundle
        // actually wires a routing trait). Do not dump every fn.
        if has_bus
            && let Some(ctx) = ctx {
                for bh in &ctx.bus_handlers {
                    if let Some(svc) = services.iter().find(|s| s.name == bh.name) {
                        bus_handler_targets.push((
                            crate_name.clone(),
                            has_deps,
                            (*svc).clone(),
                        ));
                    }
                }
            }

        // HTTP surface: only IR endpoints (authored or compat-synthesized).
        let declared_eps = ctx.map(|c| c.endpoints.as_slice()).unwrap_or(&[]);
        out.push_str(&format!("    let {crate_name}_router = Router::new()\n"));
        let mut routes_emitted: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        // path → list of (method, handler_fn_name)
        let mut seen_method_path: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for ep in declared_eps {
            let fn_name = format!("{}_{}", crate_name, to_snake(&ep.handler));
            let method = ep.method.to_ascii_lowercase();
            let mut path = ep.path.clone();
            let prefix_on_collide = ep.via.starts_with("compat")
                || ir.collide == veil_ir::CollideMode::PrefixCrate;
            let key = (method.clone(), path.clone());
            if prefix_on_collide && global_method_path.contains(&key) {
                if let Some(rest) = path.strip_prefix("/api/") {
                    path = format!("/api/{crate_name}/{rest}");
                } else {
                    path = format!("/{crate_name}{path}");
                }
            }
            let key = (method.clone(), path.clone());
            if !seen_method_path.insert(key.clone()) {
                continue;
            }
            global_method_path.insert(key);
            routes_emitted
                .entry(path)
                .or_default()
                .push((method, format!("{fn_name}_handler")));
        }
        for (path, handlers) in &routes_emitted {
            let chained = handlers
                .iter()
                .map(|(m, h)| format!("{m}({h})"))
                .collect::<Vec<_>>()
                .join(".");
            out.push_str(&format!("        .route(\"{path}\", {chained})\n"));
        }
        // /health is attached once on the merged app (not each context router)
        // so Router::merge does not panic on overlapping routes.
        // Tower: last layer is outermost. CORS must be outside auth so browser
        // OPTIONS preflight is not blocked by missing API key.
        out.push_str("        .layer(from_fn(veil_api_key_middleware))\n");
        out.push_str("        .layer(veil_cors_layer())\n");
        if has_deps {
            // Clone so bus registration can still capture {crate}_deps.
            out.push_str(&format!("        .with_state({crate_name}_deps.clone());\n\n"));
        } else {
            out.push_str("        .with_state(());\n\n");
        }
    }

    // Wire bus handlers so cross-context `invoke` / `request` resolve.
    if has_bus && !bus_handler_targets.is_empty() {
        out.push_str("    // ── bus handlers (cross-context invoke / request) ──\n");
        let mut registered: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (crate_name, has_deps, svc) in &bus_handler_targets {
            let message = registry.bus_message_name(&svc.name);
            if !registered.insert(message.clone()) {
                continue; // first context wins on name collision
            }
            out.push_str(&gen_bus_handler_registration(
                crate_name,
                *has_deps,
                svc,
                &message,
                registry,
            ));
        }
        out.push('\n');
    }

    // Merge all context routers into a single app
    let router_names: Vec<String> = modules.iter()
        .filter(|m| {
            let flat = flatten_module(m, registry);
            !flat.fns.is_empty()
        })
        .map(|m| format!("{}_router", module_crate_name(m, sol)))
        .collect();
    if router_names.is_empty() {
        out.push_str("    let app = Router::new().route(\"/health\", get(|| async { \"ok\" }));\n");
    } else {
        out.push_str(&format!("    let app = {}", router_names[0]));
        for r in &router_names[1..] {
            out.push_str(&format!(".merge({})", r));
        }
        // Single shared health probe after merge (avoids path overlap across contexts).
        out.push_str("\n        .route(\"/health\", get(|| async { \"ok\" }));\n");
    }
    let n: usize = ir.contexts.iter().map(|c| c.endpoints.len()).sum();
    out.push_str(&format!(
        "    println!(\"veil_bin: profile={} endpoints={n}\");\n",
        ir.profile
    ));
    out.push_str("    println!(\"veil_bin: listening on :{}\", port);\n");
    out.push_str("    let listener = tokio::net::TcpListener::bind(format!(\"0.0.0.0:{}\", port)).await?;\n");
    out.push_str("    axum::serve(listener, app.into_make_service()).await?;\n");
    out.push_str("    Ok(())\n}\n\n");

    // Generate handler functions only for HTTP-routable services
    for module in modules {
        let crate_name = module_crate_name(module, sol);
        let flat = flatten_module(module, registry);
        let declared_ctx = harness_ctx(ir, &crate_name, &module.name);
        let routable: Vec<(&Construct, String, String)> = declared_ctx
            .map(|ctx| {
                ctx.endpoints
                    .iter()
                    .filter_map(|ep| {
                        let svc = flat.fns.iter().find(|s| s.name == ep.handler)?;
                        Some((
                            *svc,
                            ep.method.to_ascii_lowercase(),
                            ep.path.clone(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut seen_handler_fns: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (svc, method, path) in &routable {
            let app_fn_name = to_snake(&svc.name);
            let fn_name = format!("{}_{}", crate_name, &app_fn_name);
            let method = method.clone();
            let path = path.clone();
            // Deduplicate by handler fn name — two services that name-derive to
            // the same (method, path) may still need distinct handlers when one
            // gets a collision-adjusted path prefix during route registration.
            if !seen_handler_fns.insert(fn_name.clone()) {
                continue;
            }
            let path_params = path_param_names(&path);
            let _needs_path = !path_params.is_empty();
            let has_non_path_inputs = svc.inputs.iter().any(|i| {
                !registry.field_is_dependency(i)
                    && !path_params.iter().any(|p| p == &to_snake(&i.name))
            });
            // DELETE with extra inputs uses query string —
            // many clients drop DELETE bodies (review / HTTP practice).
            let needs_body =
                method == "post" || method == "put" || method == "patch";
            let needs_query =
                harness_handler_needs_query(svc, registry, &method, &path, &path_params)
                    || (method == "delete" && has_non_path_inputs);

            // Path extractors: single param → Path(String); multi → Path<(String, …)>.
            let path_extractor = match path_params.len() {
                0 => String::new(),
                1 => format!(
                    "\n    axum::extract::Path({p}): axum::extract::Path<String>,",
                    p = path_params[0]
                ),
                n => {
                    let names = path_params.join(", ");
                    let tys = std::iter::repeat_n("String", n)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "\n    axum::extract::Path(({names})): axum::extract::Path<({tys})>,"
                    )
                }
            };
            let query_extractor = if needs_query {
                "\n    Query(q): Query<std::collections::HashMap<String, String>>,"
            } else {
                ""
            };
            let body_extractor = if needs_body {
                "\n    Json(body): Json<Value>,"
            } else {
                ""
            };
            // Axum: body extractors (Json) must be last.
            // Only include State(deps) when the context has deps (IR).
            let name_to_shape_h = build_name_to_shape(sol, registry);
            let (deps_set_h, _) = collect_deps_field_map(&flat.fns, registry, &name_to_shape_h);
            let has_deps = harness_ctx(ir, &crate_name, &module.name)
                .and_then(|c| c.deps.as_ref())
                .map(|d| !d.fields.is_empty())
                .unwrap_or(!deps_set_h.is_empty());
            let state_extractor = if !has_deps {
                String::new()
            } else {
                format!("\n    State(deps): State<Arc<{crate_name}_Deps>>,")
            };
            out.push_str(&format!(
                "async fn {fn_name}_handler({state_extractor}{path_extractor}{query_extractor}{body_extractor}\n) -> Result<Json<Value>, StatusCode> {{\n"
            ));

            // Only pass &deps when the application fn actually takes deps
            // (dependency-role inputs or body references trait deps).
            let svc_has_deps = !deps_set_h.is_empty() && (svc.inputs.iter().any(|i| registry.field_is_dependency(i))
                || {
                    svc.steps.iter().any(|st| {
                        if let FlowStep::Step(s) = st {
                            s.body.iter().any(expr_mentions_trait_dep)
                        } else {
                            false
                        }
                    })
                });
            let mut args: Vec<String> = if svc_has_deps {
                vec!["&deps".to_string()]
            } else {
                Vec::new()
            };
            for input in &svc.inputs {
                if registry.field_is_dependency(input) {
                    continue;
                }
                let field = to_snake(&input.name);
                let rust_type = crate::rust::type_to_rust(&input.type_expr);

                // Path params from endpoint `{name}` segments.
                if path_params.iter().any(|p| p == &field) {
                    if rust_type == "Uuid" {
                        out.push_str(&format!(
                            "    let {field} = {field}.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;\n"
                        ));
                    }
                    // else: already String from Path extractor
                } else if needs_query {
                    // GET/DELETE: plain query string values (not JSON-encoded).
                    // Opt/Option fields are optional — missing → None (do not 400).
                    if rust_type == "Uuid" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok())\
                             .ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else if rust_type == "String" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").cloned().unwrap_or_default();\n"
                        ));
                    } else if rust_type == "Option<String>" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").filter(|s| !s.is_empty()).cloned();\n"
                        ));
                    } else if rust_type == "Option<i64>" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<i64>().ok());\n"
                        ));
                    } else if rust_type == "Option<bool>" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").map(|s| s == \"true\" || s == \"1\");\n"
                        ));
                    } else if rust_type.starts_with("Option<") {
                        // Optional complex: try JSON parse of query value; missing → None
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").filter(|s| !s.is_empty())\
                             .and_then(|s| serde_json::from_str(s).ok());\n"
                        ));
                    } else if rust_type == "i64" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<i64>().ok())\
                             .ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else if rust_type == "bool" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").map(|s| s == \"true\" || s == \"1\")\
                             .unwrap_or(false);\n"
                        ));
                    } else {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| serde_json::from_str(s).ok())\
                             .ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    }
                } else if needs_body {
                    // Extract from JSON body (POST/PUT) — HTML dates + empty optionals
                    out.push_str(&harness_body_field_extract(&field, &rust_type));
                }
                args.push(field);
            }
            out.push_str(&format!(
                "    match {crate_name}_app::{app_fn_name}({}).await {{\n",
                args.join(", ")
            ));
            if method == "delete" {
                out.push_str("        Ok(_) => Ok(Json(serde_json::json!({\"ok\": true}))),\n");
            } else {
                // Redact role:secret fields on the way out (storage still full Serialize).
                out.push_str(
                    "        Ok(result) => Ok(Json(veil_json_public(&result))),\n",
                );
            }
            // Match DomainError variants — never substring Display text.
            out.push_str("        Err(e) => Err(veil_domain_error_status(e)),\n");
            out.push_str("    }\n}\n\n");
        }
    }

    out.push_str(&harness_json_public_helper(modules, registry));
    if let Some(em) = &registry.error_model {
        let not_found = em.variant("not_found").unwrap_or("NotFound");
        let validation = em.variant("validation").unwrap_or("Validation");
        let external = em.variant("external").unwrap_or("External");
        out.push_str(&harness_domain_error_status_helper_dynamic(&em.type_name, not_found, validation, external));
    } else {
        out.push_str(harness_domain_error_status_helper());
    }
    out.push_str(harness_auth_cors_helpers());
    out.push_str(harness_body_dt_helper());
    out
}

/// Collect snake_case field names marked role:secret across the solution.
pub fn collect_secret_field_names(modules: &[&Construct], registry: &LayerRegistry) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    fn walk(c: &Construct, registry: &LayerRegistry, names: &mut std::collections::BTreeSet<String>) {
        for f in &c.fields {
            if registry.field_is_secret(f) {
                names.insert(to_snake(&f.name));
            }
        }
        for block in &c.blocks {
            for f in &block.fields {
                if registry.field_is_secret(f) {
                    names.insert(to_snake(&f.name));
                }
            }
        }
        for ch in &c.children {
            walk(ch, registry, names);
        }
    }
    for m in modules {
        walk(m, registry, &mut names);
    }
    names.into_iter().collect()
}

/// Harness helper: Serialize then strip secret keys (INV-001 roles).
pub fn harness_json_public_helper(modules: &[&Construct], registry: &LayerRegistry) -> String {
    let secrets = collect_secret_field_names(modules, registry);
    let keys: String = secrets
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
/// Serialize for HTTP JSON, omitting fields annotated role:secret.
/// Persistence (repos) uses full `Serialize` — secrets still round-trip to storage.
pub fn veil_json_public<T: serde::Serialize>(value: &T) -> serde_json::Value {{
    let mut v = serde_json::to_value(value).unwrap_or_default();
    veil_redact_secrets(&mut v);
    v
}}

pub fn veil_redact_secrets(v: &mut serde_json::Value) {{
    // Scalar secret fields from role:secret annotations (INV-001).
    const SECRET_KEYS: &[&str] = &[{keys}];
    // Header maps/lists often carry API keys — redact values, keep names.
    const HEADER_CONTAINERS: &[&str] = &["headers"];
    match v {{
        serde_json::Value::Object(map) => {{
            for k in SECRET_KEYS {{
                map.remove(*k);
            }}
            for hk in HEADER_CONTAINERS {{
                if let Some(headers) = map.get_mut(*hk) {{
                    veil_redact_header_values(headers);
                }}
            }}
            for (_k, child) in map.iter_mut() {{
                veil_redact_secrets(child);
            }}
        }}
        serde_json::Value::Array(items) => {{
            for item in items.iter_mut() {{
                veil_redact_secrets(item);
            }}
        }}
        _ => {{}}
    }}
}}

pub fn veil_redact_header_values(v: &mut serde_json::Value) {{
    match v {{
        serde_json::Value::Array(items) => {{
            for item in items.iter_mut() {{
                if let serde_json::Value::Object(h) = item {{
                    if h.contains_key("value") {{
                        h.insert("value".into(), serde_json::Value::String(String::new()));
                    }}
                    if h.contains_key("Value") {{
                        h.insert("Value".into(), serde_json::Value::String(String::new()));
                    }}
                }}
            }}
        }}
        serde_json::Value::Object(map) => {{
            // Map-shaped headers: redact all values
            for (_k, val) in map.iter_mut() {{
                *val = serde_json::Value::String(String::new());
            }}
        }}
        _ => {{}}
    }}
}}
"#
    )
}

/// API key middleware + CORS policy for generated harness.
pub fn harness_auth_cors_helpers() -> &'static str {
    r#"
/// Production-oriented auth:
/// - `/health` + OPTIONS always open
/// - `VEIL_DEV=1` → open (local dual-loop only)
/// - else require a key: `VEIL_API_KEY`
/// - Present key via `X-Api-Key` or `Authorization: Bearer <key>`
async fn veil_api_key_middleware(
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.uri().path() == "/health" || request.method() == axum::http::Method::OPTIONS {
        return Ok(next.run(request).await);
    }
    let dev = std::env::var("VEIL_DEV").ok().as_deref() == Some("1");
    let require = std::env::var("VEIL_REQUIRE_AUTH").ok().as_deref() == Some("1");
    let admin_key = std::env::var("VEIL_API_KEY").ok().filter(|s| !s.is_empty());
    let presented = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        });

    if dev && !require && admin_key.is_none() {
        return Ok(next.run(request).await);
    }

    let Some(presented) = presented else {
        eprintln!("error: missing X-Api-Key / Authorization Bearer");
        return Err(StatusCode::UNAUTHORIZED);
    };

    if admin_key.as_deref() != Some(presented.as_str()) {
        eprintln!("warn: unauthorized — key not recognized");
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}

/// Restrict CORS: `CORS_ORIGINS=http://a,http://b` or localhost defaults (not *).
pub fn veil_cors_layer() -> CorsLayer {
    use axum::http::{HeaderValue, Method};
    if let Ok(raw) = std::env::var("CORS_ORIGINS") {
        let origins: Vec<HeaderValue> = raw
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !origins.is_empty() {
            return CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers(Any);
        }
    }
    let local = [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:5174",
        "http://127.0.0.1:5174",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
    ];
    let origins: Vec<HeaderValue> = local.iter().filter_map(|s| s.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any)
}
"#
}

/// Map domain errors to HTTP statuses — match enum variants (never Display text).
pub fn harness_domain_error_status_helper() -> &'static str {
    r#"
pub fn veil_domain_error_status(e: DomainError) -> StatusCode {
    match &e {
        DomainError::NotFound => {
            eprintln!("warn: not found: {e}");
            StatusCode::NOT_FOUND
        }
        DomainError::Validation(msg) => {
            eprintln!("warn: validation: {msg}");
            StatusCode::BAD_REQUEST
        }
        DomainError::External(msg) => {
            eprintln!("error: upstream: {msg}");
            StatusCode::BAD_GATEWAY
        }
    }
}
"#
}

/// Dynamic version: generates the error→status helper using the layer-declared error model.
pub fn harness_domain_error_status_helper_dynamic(error_type: &str, not_found: &str, validation: &str, external: &str) -> String {
    format!(
        r#"
pub fn veil_domain_error_status(e: {error_type}) -> StatusCode {{
    match &e {{
        {error_type}::{not_found} => {{
            eprintln!("warn: not found: {{e}}");
            StatusCode::NOT_FOUND
        }}
        {error_type}::{validation}(msg) => {{
            eprintln!("warn: validation: {{msg}}");
            StatusCode::BAD_REQUEST
        }}
        {error_type}::{external}(msg) => {{
            eprintln!("error: upstream: {{msg}}");
            StatusCode::BAD_GATEWAY
        }}
    }}
}}
"#
    )
}

/// Whether a handler needs `Query(q)` — non-dep inputs that are not path
/// params (GET list/filters, DELETE with extra inputs).
pub fn harness_handler_needs_query(
    svc: &Construct,
    registry: &LayerRegistry,
    method: &str,
    path: &str,
    path_params: &[String],
) -> bool {
    if method != "get" && method != "delete" {
        return false;
    }
    let _ = path; // reserved for future path-only heuristics
    svc.inputs.iter().any(|i| {
        if registry.field_is_dependency(i) {
            return false;
        }
        let field = to_snake(&i.name);
        !path_params.iter().any(|p| p == &field)
    })
}

pub fn demo_value_for_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) if n == "Str" || n == "String" => "\"widget\".to_string()".into(),
        TypeExpr::Named(n) if n == "Int" || n == "I64" => "1".into(),
        TypeExpr::Named(n) if n == "F64" => "1.0".into(),
        TypeExpr::Named(n) if n == "Bool" => "true".into(),
        TypeExpr::Named(n) if n == "UUID" || n == "Id" => "Uuid::new_v4()".into(),
        _ => "Default::default()".into(),
    }
}

/// One intended HTTP route from package IR (ACS-011 / AGT-026).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrRestRoute {
    pub method: String,
    pub path: String,
    pub handler: String,
    /// `endpoint` | `compat_route` | `compat_name` (HarnessIR only).
    pub via: &'static str,
}

pub fn has_route_annotation(svc: &Construct, registry: &LayerRegistry) -> bool {
    registry.construct_has_http_route(svc)
}

/// Extract `{param}` names from an HTTP path in order.
pub fn path_param_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            names.push(after[..end].to_string());
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    names
}

/// Fns that become HTTP endpoints in the harness.
/// If any construct has an annotation with role:http_route, only those are
/// routable. Otherwise fall back to name-derived for layer service keywords.
pub fn http_routable_services<'a>(
    services: &[&'a Construct],
    registry: &LayerRegistry,
) -> Vec<&'a Construct> {
    let with_route: Vec<&'a Construct> = services
        .iter()
        .copied()
        .filter(|s| !veil_ir::is_deploy_hook(s, registry))
        .filter(|s| has_route_annotation(s, registry))
        .collect();
    if !with_route.is_empty() {
        with_route
    } else {
        services
            .iter()
            .copied()
            .filter(|s| !veil_ir::is_deploy_hook(s, registry))
            .collect()
    }
}

/// Collect REST routes from HarnessIR only (authored + compat synthesis).
pub fn list_rest_routes_from_solution(
    sol: &Solution,
    registry: &LayerRegistry,
) -> Vec<IrRestRoute> {
    let mut ir = veil_ir::lower_harness(sol, registry);
    apply_compat_synthesis(&mut ir, sol, registry);
    veil_ir::list_endpoints_from_ir(&ir)
        .into_iter()
        .map(|e| IrRestRoute {
            method: e.method.to_ascii_lowercase(),
            path: e.path.clone(),
            handler: e.handler.clone(),
            via: match e.via.as_str() {
                "endpoint" => "endpoint",
                "compat_route" => "compat_route",
                _ => "compat_name",
            },
        })
        .collect()
}

/// Compat / migrate helper — delegates to HarnessIR `compat_rest_route`.
pub fn rest_route_for_service(svc: &Construct, registry: &LayerRegistry) -> (String, String) {
    let (method, path, _) = veil_ir::compat_rest_route(svc, registry);
    (method.to_ascii_lowercase(), path)
}

/// Emit `bus.register("Msg", …)` that deserializes the JSON envelope and calls
/// the application function for `svc`.
pub fn gen_bus_handler_registration(
    crate_name: &str,
    module_has_deps: bool,
    svc: &Construct,
    message: &str,
    registry: &LayerRegistry,
) -> String {
    let app_fn = to_snake(&svc.name);
    // Only pass &deps when *this* service takes dependency-role inputs (or
    // uses trait deps in its body). Module-level Deps may exist for other svcs.
    let svc_takes_deps = module_has_deps
        && (svc
            .inputs
            .iter()
            .any(|i| registry.field_is_dependency(i))
            || svc.steps.iter().any(|st| {
                if let FlowStep::Step(s) = st {
                    s.body.iter().any(expr_mentions_trait_dep)
                } else {
                    false
                }
            }));

    let mut out = String::new();
    if svc_takes_deps {
        out.push_str(&format!(
            "    {{\n\
             \x20       let __deps = {crate_name}_deps.clone();\n\
             \x20       bus.register(\"{message}\", move |cmd| {{\n\
             \x20           let __deps = __deps.clone();\n\
             \x20           async move {{\n"
        ));
    } else {
        out.push_str(&format!(
            "    {{\n\
             \x20       bus.register(\"{message}\", move |cmd| {{\n\
             \x20           async move {{\n"
        ));
    }

    // Build call args with from_value so domain types resolve via inference
    // against the application function signature (no bare RepoId in bin scope).
    let mut call_parts: Vec<String> = Vec::new();
    if svc_takes_deps {
        call_parts.push("&__deps".to_string());
    }
    for input in &svc.inputs {
        if registry.field_is_dependency(input) {
            continue;
        }
        let field = to_snake(&input.name);
        let rust_type = type_to_rust(&input.type_expr);
        if rust_type == "String" {
            call_parts.push(format!(
                "cmd.get(\"{field}\").and_then(|v| v.as_str()).unwrap_or(\"\").to_string()"
            ));
        } else if rust_type == "bool" {
            call_parts.push(format!(
                "cmd.get(\"{field}\").and_then(|v| v.as_bool()).unwrap_or(false)"
            ));
        } else if rust_type == "i64" {
            call_parts.push(format!(
                "cmd.get(\"{field}\").and_then(|v| v.as_i64()).unwrap_or(0)"
            ));
        } else if rust_type == "serde_json::Value" {
            call_parts.push(format!(
                "cmd.get(\"{field}\").cloned().unwrap_or(serde_json::Value::Null)"
            ));
        } else {
            // Option / domain structs / enums — deserialize; null → None for Option.
            call_parts.push(format!(
                "serde_json::from_value(cmd.get(\"{field}\").cloned()\
                 .unwrap_or(serde_json::Value::Null))\
                 .map_err(|e| DomainError::External(e.to_string()))?"
            ));
        }
    }

    let call_args = call_parts.join(", ");
    out.push_str(&format!(
        "                let __result = {crate_name}_app::{app_fn}({call_args}).await?;\n\
         \x20               Ok(serde_json::to_value(__result)\
         .map_err(|e| DomainError::External(e.to_string()))?)\n\
         \x20           }}\n\
         \x20       }});\n\
         \x20   }}\n"
    ));
    out
}

