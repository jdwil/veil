//! Rust code generation from VEIL AST.
//!
//! Fully shape-driven: constructs are generated according to their core
//! shape (`mod` → crate, `struct`/`enum` → types, `trait` → async traits,
//! `impl` → adapter structs, `fn` → application functions). The construct's
//! layer subkind appears only in doc comments — never in generation logic.

use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};

/// Generated Rust project output.
pub struct GeneratedProject {
    pub files: Vec<GeneratedFile>,
}

pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// Generate a Rust project from a VEIL Solution AST.
pub fn generate(solution: &Solution, registry: &LayerRegistry) -> GeneratedProject {
    let mut files = Vec::new();

    // CAP-001: resolve external crate links (skip invalid with warning-style omit:
    // only emit successfully resolved links; invalid ones are dropped so gen still
    // produces a workspace — CLI can surface resolve errors separately later).
    let resolved_links = match crate::links::resolve_links(&solution.links) {
        Ok(links) => links,
        Err(errs) => {
            for e in &errs {
                eprintln!("warning: {e}");
            }
            // Best-effort: resolve each independently
            solution
                .links
                .iter()
                .filter_map(|l| crate::links::resolve_link(l).ok())
                .collect()
        }
    };

    files.push(gen_workspace_toml(solution, registry, &resolved_links));

    // Shared crate: owns the common error types and the layer-provided
    // top-level traits (the injected Bus), so they are defined ONCE and every
    // context crate re-exports the same type — enabling a real shared bus.
    let shared_traits: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Trait && c.layer_provided => Some(c),
            _ => None,
        })
        .collect();
    // Layer-provided structs (e.g. Principal) also live in the shared crate
    // so traits can reference them.
    let shared_structs: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Struct && c.layer_provided => Some(c),
            _ => None,
        })
        .collect();
    // Top-level free functions (e.g. the layer-declared saga coordinator) also
    // live in the shared crate so every context can call them through the Bus.
    let shared_fns: Vec<&FnDef> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Function(f) => Some(f),
            _ => None,
        })
        .collect();
    // Each top-level mod-shaped construct becomes a crate.
    let modules: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(c),
            _ => None,
        })
        .collect();

    // CAP-003: collect handler message names for register_all.
    let handler_names = collect_handler_names(solution, &modules);

    files.extend(gen_shared_crate(
        &shared_traits,
        &shared_structs,
        &shared_fns,
        solution,
        registry,
        &resolved_links,
        &handler_names,
    ));

    // Impl-shaped constructs may live at top level or inside other modules;
    // collect all of them so each crate can pick up impls targeting its traits.
    let all_impls: Vec<&Construct> = collect_by_shape(solution, Shape::Impl);
    let top_level_flows: Vec<&Flow> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Flow(f) => Some(f),
            _ => None,
        })
        .collect();

    let mut flow_generated = false;
    for module in &modules {
        files.extend(gen_module_crate(
            module,
            &all_impls,
            &top_level_flows,
            &mut flow_generated,
            solution,
            registry,
            &resolved_links,
        ));
    }

    // ─── Layer Template Augmentation ─────────────────────────────────────
    // Execute any codegen templates from loaded layers (di.layer, rust.layer, etc.)
    // Template output augments the backend's output — it doesn't replace it.
    let template_output = crate::template::execute_templates(solution, registry, "rust");

    // RT-001b / RT-001: @main → dedicated veil_bin with local harness main.
    // Prefer a generated InProcessBus harness (RT-001/003/004) over raw
    // template fragments when we have context modules.
    let has_main = crate::template::compose_main_section(&template_output, "rust").is_some()
        || package_has_main_annotation(solution)
        || !modules.is_empty(); // Packages with modules always get a harness binary.
    if has_main {
        let module_crates: Vec<String> = modules.iter().map(|m| to_snake(&m.name)).collect();
        // CAP-002/006: product host bin when package links veil_server.
        let wants_product_host = resolved_links
            .iter()
            .any(|l| l.rust_name == "veil_server" || l.cargo_name == "veil-server");
        let main_body = if wants_product_host {
            gen_product_host_main(solution, &handler_names)
        } else if !modules.is_empty() {
            gen_local_harness_main(solution, &modules, registry)
        } else if let Some(body) = crate::template::compose_main_section(&template_output, "rust")
        {
            body
        } else {
            String::from(
                "#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n    println!(\"veil_bin: no modules to run\");\n    Ok(())\n}\n",
            )
        };
        files.extend(gen_bin_crate(
            solution,
            &module_crates,
            &main_body,
            &resolved_links,
            registry,
        ));
        if let Some(ws) = files.iter_mut().find(|f| f.path == "Cargo.toml") {
            if !ws.content.contains("crates/veil_bin") {
                // Insert veil_bin after veil_shared in the members list.
                ws.content = ws.content.replacen(
                    "    \"crates/veil_shared\"",
                    "    \"crates/veil_shared\",\n    \"crates/veil_bin\"",
                    1,
                );
            }
        }
    }

    // Add template-generated files
    for tpl_file in template_output.files {
        files.push(GeneratedFile {
            path: tpl_file.path,
            content: tpl_file.content,
        });
    }

    GeneratedProject { files }
}

/// Recursively collect all constructs of a given shape from the solution.
fn collect_by_shape<'a>(solution: &'a Solution, shape: Shape) -> Vec<&'a Construct> {
    let mut out = Vec::new();
    fn walk<'a>(c: &'a Construct, shape: Shape, out: &mut Vec<&'a Construct>) {
        if c.shape == shape {
            out.push(c);
        }
        for child in &c.children {
            walk(child, shape, out);
        }
    }
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            walk(c, shape, &mut out);
        }
    }
    out
}

/// Flatten a module's contents (unwrapping groups) into shape buckets.
struct ModuleContents<'a> {
    structs: Vec<&'a Construct>,
    enums: Vec<&'a Construct>,
    traits: Vec<&'a Construct>,
    impls: Vec<&'a Construct>,
    fns: Vec<&'a Construct>,
}

fn flatten_module<'a>(module: &'a Construct) -> ModuleContents<'a> {
    let mut contents = ModuleContents {
        structs: Vec::new(),
        enums: Vec::new(),
        traits: Vec::new(),
        impls: Vec::new(),
        fns: Vec::new(),
    };
    fn walk<'a>(c: &'a Construct, contents: &mut ModuleContents<'a>) {
        for child in &c.children {
            match child.shape {
                Shape::Struct => contents.structs.push(child),
                Shape::Enum => contents.enums.push(child),
                Shape::Trait => contents.traits.push(child),
                Shape::Impl => contents.impls.push(child),
                Shape::Fn => contents.fns.push(child),
                Shape::Group | Shape::Mod => walk(child, contents),
            }
        }
    }
    walk(module, &mut contents);
    contents
}

/// True if expr tree contains a port method call that requires a Deps parameter.
/// Matches `PortName.method!(…)` / dep-local calls; ignores `Type.new(...)` constructors.
fn expr_mentions_port_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => {
            let method = call.method.trim_end_matches(['!', '?']);
            // Skip constructors (Type.new / Type{})
            let is_ctor = method.is_empty() || method == "new";
            if !is_ctor && !call.method.is_empty() {
                let t = call.target.as_str();
                // Port/trait calls: PascalCase target, or snake_case @dep local ending in _repo/_port/_svc
                let pascal = t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
                let dep_local = t.ends_with("_repo")
                    || t.ends_with("_port")
                    || t.ends_with("_svc")
                    || t.ends_with("_client");
                if pascal || dep_local {
                    return true;
                }
            }
            if let Some(recv) = &call.receiver {
                if expr_mentions_port_call(recv) {
                    return true;
                }
            }
            call.args.iter().any(expr_mentions_port_call)
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) => expr_mentions_port_call(rhs),
        Expr::Return(inner) | Expr::Try(inner) | Expr::Await(inner) | Expr::UnaryOp(UnaryOpExpr { expr: inner, .. }) => {
            expr_mentions_port_call(inner)
        }
        Expr::BinaryOp(b) => {
            expr_mentions_port_call(&b.left) || expr_mentions_port_call(&b.right)
        }
        Expr::IfExpr(i) => {
            expr_mentions_port_call(&i.condition)
                || i.then_body.iter().any(expr_mentions_port_call)
                || i.else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_mentions_port_call))
                    .unwrap_or(false)
        }
        Expr::ArrayLit(items) => items.iter().any(expr_mentions_port_call),
        _ => false,
    }
}

/// Check if any expression in a tree references `self.<field_name>`.
fn expr_mentions_self_field(expr: &Expr, field_name: &str) -> bool {
    match expr {
        Expr::Call(call) => {
            let target_matches = call.target == format!("self.{}", field_name)
                || (call.target.starts_with("self.") && call.target.split('.').nth(1) == Some(field_name));
            if target_matches {
                return true;
            }
            if let Some(recv) = &call.receiver {
                if expr_mentions_self_field(recv, field_name) {
                    return true;
                }
            }
            call.args.iter().any(|a| expr_mentions_self_field(a, field_name))
        }
        Expr::FieldAccess(base, field) => {
            if field == field_name {
                if let Expr::Ident(id) = base.as_ref() {
                    return id == "self";
                }
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

fn package_has_main_annotation(sol: &Solution) -> bool {
    fn walk(c: &Construct) -> bool {
        if c.annotations.iter().any(|a| a.name == "main") {
            return true;
        }
        c.children.iter().any(walk)
            || c.fns.iter().any(|f| f.annotations.iter().any(|a| a.name == "main"))
    }
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) if walk(c) => return true,
            TopLevelItem::Function(f)
                if f.annotations.iter().any(|a| a.name == "main") =>
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
fn gen_product_host_main(sol: &Solution, handler_names: &[String]) -> String {
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
fn stub_is_active_cargo(stub: &veil_ir::StubCrate) -> bool {
    !stub.row_type_derives.is_empty()
        || !stub.wrapper_type_derives.is_empty()
        || !stub.cargo_features.is_empty()
        || !stub.cargo_deps.is_empty()
        || !stub.codegen_imports.is_empty()
        || !stub.structs.is_empty()
        || !stub.harness_fields.is_empty()
}

/// Resolve a stub type to `(crate_name, path_under_crate)` (e.g. Pool → sqlx, PgPool path).
fn stub_type_path(registry: &LayerRegistry, type_name: &str) -> Option<(String, String)> {
    for stub in &registry.stubs {
        if stub.structs.iter().any(|s| s.name == type_name) {
            let crate_name = stub.name.replace('-', "_");
            return Some((crate_name, stub.rust_type_path(type_name)));
        }
    }
    None
}

/// Look up a stub `harness_field Type` recipe. Returns (local_let_name, rust_expr).
fn stub_harness_field_expr(
    registry: &LayerRegistry,
    type_name: &str,
) -> Option<(String, String)> {
    for stub in &registry.stubs {
        if let Some(expr) = stub.harness_fields.get(type_name) {
            let let_name = format!("_stub_{}", to_snake(type_name));
            return Some((let_name, expr.trim().to_string()));
        }
        // Also match when type is aliased (e.g. S3Client) — strip common prefixes? skip.
    }
    None
}

fn gen_local_harness_main(
    sol: &Solution,
    modules: &[&Construct],
    registry: &LayerRegistry,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "//! HTTP harness for package `{}` (RT-001 / RT-003).\n\
         //! Wires adapters + exposes services as REST endpoints.\n\
         //! `cargo run -p veil_bin` from the generated workspace root.\n\n",
        sol.name
    ));
    out.push_str("use std::sync::Arc;\n");
    out.push_str("use axum::{Router, Json, extract::State, extract::Query, routing::{get, post, put, delete}, http::StatusCode};\n");
    out.push_str("use tower_http::cors::CorsLayer;\n");
    out.push_str("use uuid::Uuid;\n");
    out.push_str("use serde_json::Value;\n");
    out.push_str("use veil_shared::*;\n\n");
    for m in modules {
        let cn = to_snake(&m.name);
        out.push_str(&format!(
            "use {cn}::application::{{self as {cn}_app, Deps as {cn}_Deps}};\n"
        ));
        out.push_str(&format!("use {cn}::adapters::*;\n"));
        out.push_str(&format!("use {cn}::ports::*;\n"));
    }
    out.push_str("\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
    out.push_str("    let port: u16 = std::env::var(\"PORT\").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);\n\n");

    for module in modules {
        let crate_name = to_snake(&module.name);
        let flat = flatten_module(module);
        let adapters = &flat.impls;
        let services = &flat.fns;
        if adapters.is_empty() && services.is_empty() {
            continue;
        }

        out.push_str(&format!("    // ── context {} ──\n", module.name));
        // Collect stub harness_field constructors needed by @field annotations.
        // Construction recipes live on the .stub (not in the engine).
        let mut emitted_harness_lets: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ad in adapters {
            for ann in &ad.annotations {
                if ann.name != "field" {
                    continue;
                }
                for arg in &ann.args {
                    let ftype = arg
                        .split_once(':')
                        .map(|(_, t)| t.trim())
                        .unwrap_or("")
                        .to_string();
                    if ftype.is_empty() {
                        continue;
                    }
                    if emitted_harness_lets.contains(&ftype) {
                        continue;
                    }
                    if let Some((let_name, expr)) =
                        stub_harness_field_expr(registry, &ftype)
                    {
                        out.push_str(&format!(
                            "    // stub harness_field {ftype}\n\
                             let {let_name} = {expr};\n\n"
                        ));
                        emitted_harness_lets.insert(ftype);
                    }
                }
            }
        }
        for ad in adapters {
            // Skip pure generic templates (adapter Foo<T> for Trait<T>).
            if is_pure_generic_adapter_template(ad) {
                continue;
            }
            let target = ad.target.as_deref().unwrap_or("Send");
            // Check if adapter has @env fields to populate from environment
            let env_ann = ad.annotations.iter().find(|a| a.name == "env");
            let mut fields_init = String::new();
            // @field(name: Type) — wire from stub harness_field or Default
            for ann in &ad.annotations {
                if ann.name == "field" {
                    for arg in &ann.args {
                        let (fname, ftype) = if let Some((n, t)) = arg.split_once(':') {
                            (n.trim(), t.trim())
                        } else {
                            (arg.trim(), "String")
                        };
                        if let Some((let_name, _)) =
                            stub_harness_field_expr(registry, ftype)
                        {
                            fields_init.push_str(&format!(
                                "        {fname}: {let_name}.clone(),\n"
                            ));
                        } else {
                            fields_init.push_str(&format!(
                                "        {fname}: Default::default(),\n"
                            ));
                        }
                    }
                }
            }
            if let Some(env_a) = env_ann {
                for arg in &env_a.args {
                    // Match adapter struct field naming: last segment after '_'
                    let full = arg.to_lowercase();
                    let field_name = full.rsplit('_').next().unwrap_or(&full);
                    fields_init.push_str(&format!(
                        "        {field_name}: std::env::var(\"{arg}\").unwrap_or_else(|_| \"default\".into()),\n"
                    ));
                }
            }
            // Auto-init client field when adapter body uses self.client
            // and no @field(client: ...) already provides it.
            let has_explicit_client_field = ad.annotations.iter().any(|a| {
                a.name == "field"
                    && a.args.iter().any(|arg| {
                        arg.split(':').next().unwrap_or("").trim() == "client"
                    })
            });
            let body_uses_client = ad.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            });
            if body_uses_client && !has_explicit_client_field && !fields_init.contains("client:") {
                if let Some((let_name, _)) = stub_harness_field_expr(registry, "Client") {
                    fields_init.push_str(&format!("        client: {let_name}.clone(),\n"));
                }
            } else if !ad.fields.is_empty() {
                for f in &ad.fields {
                    let field_name = to_snake(&f.name);
                    let env_key = f.name.to_uppercase();
                    fields_init.push_str(&format!(
                        "        {field_name}: std::env::var(\"{env_key}\").unwrap_or_else(|_| \"default\".into()),\n"
                    ));
                }
            }
            let dyn_ty = adapter_dyn_type(sol, ad);
            if fields_init.is_empty() {
                out.push_str(&format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({name}{{}});\n",
                    sn = to_snake(&ad.name),
                    name = ad.name,
                ));
            } else {
                out.push_str(&format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({name} {{\n{fields_init}    }});\n",
                    sn = to_snake(&ad.name),
                    name = ad.name,
                ));
            }
        }

        if services.is_empty() {
            continue;
        }

        // Shared Deps field names must match application crate (dependency-role
        // input names preferred over snake(trait)).
        let name_to_shape = build_name_to_shape(sol, registry);
        let (_deps_set, dep_fields) =
            collect_deps_field_map(&services, registry, &name_to_shape);

        out.push_str(&format!("    let {crate_name}_deps = Arc::new({crate_name}_Deps {{\n"));
        let mut wired_fields: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ad in adapters {
            if is_pure_generic_adapter_template(ad) {
                continue;
            }
            if let Some(target) = &ad.target {
                let field = adapter_deps_field_name(sol, ad, target, &dep_fields);
                // One adapter per Deps field (first wins). Multi-adapter selection
                // policy is PR4; naming agreement is this path.
                if !wired_fields.insert(field.clone()) {
                    continue;
                }
                // Only wire fields the application Deps struct declares.
                if !dep_fields.values().any(|v| v == &field)
                    && !dep_fields.contains_key(target)
                {
                    // Still allow if map is empty (no @dep scan) and field is trait snake
                    if !dep_fields.is_empty() {
                        continue;
                    }
                }
                out.push_str(&format!(
                    "        {field}: {sn}_inst.clone(),\n",
                    sn = to_snake(&ad.name),
                ));
            }
        }
        out.push_str("    });\n\n");

        // Generate RESTful route handlers for each service.
        // Detect CRUD patterns: List* → GET /resource, Get* → GET /resource/:id,
        // Create* → POST /resource, Update* → PUT /resource/:id, Delete* → DELETE /resource/:id
        out.push_str("    let app = Router::new()\n");
        let mut routes_emitted: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for svc in services {
            let fn_name = to_snake(&svc.name);
            let (method, path) = rest_route_for_service(svc);
            routes_emitted.entry(path.clone()).or_default().push(format!("{method}({fn_name}_handler)"));
        }
        for (path, handlers) in &routes_emitted {
            let combined = handlers.join(".").replace("get(", "get(").replace("post(", "post(").replace("put(", "put(").replace("delete(", "delete(");
            out.push_str(&format!("        .route(\"{path}\", {})\n", handlers.join(".")));
        }
        out.push_str("        .route(\"/health\", get(|| async { \"ok\" }))\n");
        out.push_str("        .layer(CorsLayer::permissive())\n");
        out.push_str(&format!("        .with_state({crate_name}_deps);\n\n"));
    }

    out.push_str(&format!(
        "    println!(\"veil_bin: listening on :{{}}\", port);\n"
    ));
    out.push_str("    let listener = tokio::net::TcpListener::bind(format!(\"0.0.0.0:{}\", port)).await?;\n");
    out.push_str("    axum::serve(listener, app).await?;\n");
    out.push_str("    Ok(())\n}\n\n");

    // Generate handler functions for each service
    for module in modules {
        let crate_name = to_snake(&module.name);
        let flat = flatten_module(module);
        for svc in &flat.fns {
            let fn_name = to_snake(&svc.name);
            let (method, path) = rest_route_for_service(svc);
            let needs_path_id = path.contains("{id}")
                || (method == "get" && !svc.name.starts_with("List") && !path.contains('{'))
                || method == "put"
                || method == "delete";
            let needs_path_id = needs_path_id && path.contains('{');
            let needs_body = method == "post" || method == "put";

            // Function signature
            // List* GET: Query params (not random Uuid defaults — that silently broke local APIs).
            let is_list_get = method == "get" && (svc.name.starts_with("List") || !path.contains('{'));
            if needs_path_id && needs_body {
                out.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    axum::extract::Path(id): axum::extract::Path<String>,\n    Json(body): Json<Value>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else if needs_path_id {
                out.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    axum::extract::Path(id): axum::extract::Path<String>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else if needs_body {
                out.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    Json(body): Json<Value>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else if is_list_get {
                out.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    Query(q): Query<std::collections::HashMap<String, String>>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else {
                out.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            }

            // Only pass &deps when the application fn actually takes deps
            // (has @dep inputs or body references ports).
            let svc_has_deps = svc.inputs.iter().any(|i| {
                i.annotations.iter().any(|a| a.name == "dep")
            }) || {
                // Port calls as Type.method in body imply Deps
                svc.steps.iter().any(|st| {
                    if let FlowStep::Step(s) = st {
                        s.body.iter().any(|e| expr_mentions_port_call(e))
                    } else {
                        false
                    }
                })
            };
            let mut args: Vec<String> = if svc_has_deps {
                vec!["&deps".to_string()]
            } else {
                Vec::new()
            };
            for input in &svc.inputs {
                if input.annotations.iter().any(|a| a.name == "dep") {
                    continue;
                }
                let field = to_snake(&input.name);
                let rust_type = crate::rust::type_to_rust(&input.type_expr);

                // If this is the 'id' field and we have path extraction, use that
                if field == "id" && needs_path_id {
                    if rust_type == "Uuid" {
                        out.push_str(&format!(
                            "    let {field} = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else {
                        out.push_str(&format!("    let {field} = id.clone();\n"));
                    }
                } else if is_list_get {
                    // GET List*: query string ?tenant_id=… (required for Uuid)
                    if rust_type == "Uuid" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok())\
                             .ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else if rust_type == "String" {
                        out.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").cloned().unwrap_or_default();\n"
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
                "    match {crate_name}_app::{fn_name}({}).await {{\n",
                args.join(", ")
            ));
            if method == "delete" {
                out.push_str("        Ok(_) => Ok(Json(serde_json::json!({\"ok\": true}))),\n");
            } else {
                out.push_str("        Ok(result) => Ok(Json(serde_json::to_value(result).unwrap_or_default())),\n");
            }
            out.push_str("        Err(e) => { eprintln!(\"error: {e}\"); Err(StatusCode::INTERNAL_SERVER_ERROR) }\n");
            out.push_str("    }\n}\n\n");
        }
    }

    out.push_str(harness_body_dt_helper());
    out
}

fn demo_value_for_type(ty: &TypeExpr) -> String {
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
    /// `route` when from `@route`; `name` when name-derived fallback.
    pub via: &'static str,
}

/// Collect REST routes from package IR: `@route` first, else name-derived
/// (same policy as local harness). Works without gen (ACS-011).
pub fn list_rest_routes_from_solution(sol: &Solution) -> Vec<IrRestRoute> {
    let mut out = Vec::new();
    for item in &sol.items {
        let TopLevelItem::Construct(c) = item else {
            continue;
        };
        if c.shape != Shape::Mod {
            continue;
        }
        let flat = flatten_module(c);
        for svc in &flat.fns {
            // Skip non-HTTP helpers (@main bootstrap, pure fns without svc/handler shape)
            let is_svc = svc.subkind.eq_ignore_ascii_case("Service")
                || svc.subkind.eq_ignore_ascii_case("Handler")
                || svc.keyword == "svc"
                || svc.keyword == "handler"
                || svc.annotations.iter().any(|a| a.name == "route");
            if !is_svc {
                continue;
            }
            let has_route = svc.annotations.iter().any(|a| a.name == "route");
            let (method, path) = rest_route_for_service(svc);
            out.push(IrRestRoute {
                method,
                path,
                handler: svc.name.clone(),
                via: if has_route { "route" } else { "name" },
            });
        }
    }
    out
}

/// Derive a RESTful (method, path) from a service name.
/// ListInitiatives → (get, /api/initiatives)
/// GetInitiative → (get, /api/initiatives/{id})
/// Prefer `@route` annotation when present (AGT-026); else name-derived REST.
///
/// Annotation forms (first arg):
/// - `"GET /api/foo"` / `"POST /api/foo"` …
/// - `"/api/foo"` alone → method from service name (`derive_rest_route`)
pub fn rest_route_for_service(svc: &Construct) -> (String, String) {
    if let Some(ann) = svc.annotations.iter().find(|a| a.name == "route") {
        if let Some(raw) = ann.args.first() {
            let s = raw.trim().trim_matches('"').trim_matches('\'');
            if let Some((method, path)) = parse_route_annotation(s) {
                return (method, path);
            }
            // Path-only: keep derived method
            if s.starts_with('/') {
                let (method, _) = derive_rest_route(&svc.name);
                return (method, s.to_string());
            }
        }
    }
    derive_rest_route(&svc.name)
}

fn parse_route_annotation(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let mut parts = s.splitn(2, char::is_whitespace);
    let first = parts.next()?.trim();
    let rest = parts.next().map(|r| r.trim()).filter(|r| !r.is_empty());
    let methods = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD"];
    if let Some(path) = rest {
        if methods.iter().any(|m| first.eq_ignore_ascii_case(m)) && path.starts_with('/') {
            return Some((first.to_lowercase(), path.to_string()));
        }
    }
    None
}

/// CreateInitiative → (post, /api/initiatives)
/// UpdateInitiative → (put, /api/initiatives/{id})
/// DeleteInitiative → (delete, /api/initiatives/{id})
fn derive_rest_route(service_name: &str) -> (String, String) {
    // Strip prefix and pluralize resource
    let prefixes = [
        ("List", "get"),
        ("Get", "get"),
        ("Create", "post"),
        ("Update", "put"),
        ("Delete", "delete"),
    ];
    for (prefix, method) in prefixes {
        if let Some(resource) = service_name.strip_prefix(prefix) {
            let resource_snake = to_snake(resource);
            // Pluralize: Initiative → initiatives (simple 's' suffix if not already plural)
            let resource_plural = if resource_snake.ends_with('s') {
                resource_snake.clone()
            } else {
                format!("{resource_snake}s")
            };
            let path = match method {
                "get" if prefix == "List" => format!("/api/{resource_plural}"),
                "post" => format!("/api/{resource_plural}"),
                _ => format!("/api/{resource_plural}/{{id}}"),
            };
            return (method.to_string(), path);
        }
    }
    // Fallback: POST to kebab-case name
    let fallback_path = format!("/api/{}", to_snake(service_name).replace('_', "-"));
    ("post".to_string(), fallback_path)
}

/// RT-001b: dedicated binary crate for `@main` / composition root.
fn gen_bin_crate(
    sol: &Solution,
    module_crates: &[String],
    main_body: &str,
    links: &[crate::links::ResolvedLink],
    registry: &LayerRegistry,
) -> Vec<GeneratedFile> {
    let mut deps = String::from(
        "tokio = { workspace = true }\nuuid = { workspace = true }\nserde_json = { workspace = true }\nveil_shared = { path = \"../veil_shared\" }\naxum = \"0.8\"\ntower-http = { version = \"0.6\", features = [\"cors\"] }\n",
    );
    for c in module_crates {
        deps.push_str(&format!("{c} = {{ path = \"../{c}\" }}\n"));
    }
    // CAP-001: external crate links on veil_bin (host / @main).
    for link in links {
        deps.push_str(&crate::links::cargo_workspace_dep_line(link));
    }
    // Companion crates + primary stubs used by harness_field / @field wiring.
    // Cargo package keys use the stub name as published (hyphens), not snake_case.
    // Only active stubs (features/deps/types/harness metadata) — matches multi-package harness.
    for stub in &registry.stubs {
        if !stub_is_active_cargo(stub) {
            continue;
        }
        let crate_key = &stub.name;
        if !deps.contains(crate_key) {
            deps.push_str(&format!("{crate_key} = {{ workspace = true }}\n"));
        }
        for (dep_name, _) in &stub.cargo_deps {
            if !deps.contains(dep_name) {
                deps.push_str(&format!("{dep_name} = {{ workspace = true }}\n"));
            }
        }
    }
    // Product host needs tracing-subscriber when linking veil-server.
    if links
        .iter()
        .any(|l| l.rust_name == "veil_server" || l.cargo_name == "veil-server")
    {
        deps.push_str(
            "tracing = { workspace = true }\ntracing-subscriber = { version = \"0.3\", features = [\"env-filter\"] }\n",
        );
    }
    // Use statements so main can call into context crates when present.
    // CAP-001 linked crates are available as `veil_server::…` via Cargo deps
    // (extern prelude); no extra `use` required.
    let mut uses = String::from("use veil_shared::*;\n");
    for c in module_crates {
        uses.push_str(&format!("use {c}::*;\n"));
    }
    let cargo = format!(
        r#"[package]
name = "veil_bin"
version.workspace = true
edition.workspace = true

[[bin]]
name = "veil_bin"
path = "src/main.rs"

[dependencies]
{deps}"#
    );
    // Harness main already includes uses + #[tokio::main]; don't double-wrap.
    let main_rs = if main_body.contains("#[tokio::main]") || main_body.contains("fn main") {
        main_body.to_string()
    } else {
        format!(
            "//! Generated entrypoint for package `{}` (@main contributors).\n\
             //! Run: `cargo run -p veil_bin` from the generated workspace root.\n\
             {uses}\n\
             #[tokio::main]\n\
             async fn main() -> Result<(), Box<dyn std::error::Error>> {{\n\
             {main_body}\n\
                 Ok(())\n\
             }}\n",
            sol.name
        )
    };
    vec![
        GeneratedFile {
            path: "crates/veil_bin/Cargo.toml".into(),
            content: cargo,
        },
        GeneratedFile {
            path: "crates/veil_bin/src/main.rs".into(),
            content: main_rs,
        },
    ]
}

fn gen_workspace_toml(
    sol: &Solution,
    registry: &LayerRegistry,
    links: &[crate::links::ResolvedLink],
) -> GeneratedFile {
    let mut members = vec!["    \"crates/veil_shared\"".to_string()];
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Mod {
                members.push(format!("    \"crates/{}\"", to_snake(&c.name)));
            }
        }
    }

    // GEN-006: deps/features from stub metadata only (no engine hardcode).
    // Emit every stub the package loaded via `use` plus cargo_deps companions
    // (e.g. aws-config for aws-sdk-dynamodb) so veil_bin workspace=true resolves.
    let mut extra_deps = String::new();
    for stub in &registry.stubs {
        if !stub_is_active_cargo(stub) {
            continue;
        }
        // Path stubs: version line `path:../relative` (local platform crates, not crates.io).
        // Keeps filesystem/SDK details out of the engine; the .stub still declares the API.
        if let Some(rel) = stub.version.strip_prefix("path:") {
            extra_deps.push_str(&format!(
                "{} = {{ path = \"{}\" }}\n",
                stub.name, rel
            ));
        } else if stub.cargo_features.is_empty() {
            extra_deps.push_str(&format!("{} = \"{}\"\n", stub.name, stub.version));
        } else {
            let feats: Vec<String> = stub
                .cargo_features
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect();
            extra_deps.push_str(&format!(
                "{} = {{ version = \"{}\", features = [{}] }}\n",
                stub.name,
                stub.version,
                feats.join(", ")
            ));
        }
        // Companion crates declared on the stub (e.g. aws-config for dynamodb).
        for (dep_name, dep_ver) in &stub.cargo_deps {
            if !extra_deps.contains(dep_name) {
                extra_deps.push_str(&format!("{dep_name} = \"{dep_ver}\"\n"));
            }
        }
    }
    // CAP-001: path deps from `link` declarations.
    for link in links {
        extra_deps.push_str(&crate::links::cargo_dep_line(link));
    }

    let content = format!(
        r#"[workspace]
members = [
{}
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
tokio = {{ version = "1", features = ["full"] }}
async-trait = "0.1"
thiserror = "2"
serde = {{ version = "1", features = ["derive"] }}
uuid = {{ version = "1", features = ["v4", "serde"] }}
chrono = {{ version = "0.4", features = ["serde"] }}
tracing = "0.1"
serde_json = "1"
{}"#,
        members.join(",\n"),
        extra_deps
    );

    GeneratedFile {
        path: "Cargo.toml".to_string(),
        content,
    }
}

fn gen_module_crate(
    module: &Construct,
    all_impls: &[&Construct],
    top_level_flows: &[&Flow],
    flow_generated: &mut bool,
    solution: &Solution,
    registry: &LayerRegistry,
    links: &[crate::links::ResolvedLink],
) -> Vec<GeneratedFile> {
    let crate_name = to_snake(&module.name);
    let mut files = Vec::new();
    let mut contents = flatten_module(module);

    // Solution-level layer-provided traits (the injected Bus) live in the
    // shared crate and are re-exported by gen_traits — do NOT duplicate them
    // here. Any non-layer top-level trait is still emitted locally.
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Trait && !c.layer_provided {
                contents.traits.push(c);
            }
        }
    }

    files.push(GeneratedFile {
        path: format!("crates/{}/Cargo.toml", crate_name),
        content: {
            let mut cargo = format!(
                r#"[package]
name = "{crate_name}"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
async-trait.workspace = true
thiserror.workspace = true
serde.workspace = true
uuid.workspace = true"#);
            // Inter-context communication goes through Bus — no sibling crate deps needed.
            cargo.push_str("\n");
            cargo.push_str("chrono.workspace = true\ntracing.workspace = true\nserde_json.workspace = true\n");
            // Shared error types + Bus trait, defined once.
            cargo.push_str("veil_shared = { path = \"../veil_shared\" }\n");
            // Stub crate dependencies (active only — same policy as veil_bin / workspace)
            for stub in &registry.stubs {
                if !stub_is_active_cargo(stub) {
                    continue;
                }
                cargo.push_str(&format!("{}.workspace = true\n", stub.name));
            }
            // CAP-001: external crate links
            for link in links {
                cargo.push_str(&crate::links::cargo_workspace_dep_line(link));
            }
            cargo
        },
    });

    files.push(GeneratedFile {
        path: format!("crates/{}/src/lib.rs", crate_name),
        content: format!(
            "//! {} — {}.\n\npub mod domain;\npub mod ports;\npub mod adapters;\npub mod application;\n",
            module.name, module.subkind
        ),
    });

    files.push(gen_types(&contents, &crate_name, registry, solution));
    files.push(gen_child_types(&contents, &crate_name));
    files.push(GeneratedFile {
        path: format!("crates/{}/src/domain/mod.rs", crate_name),
        content: "pub mod types;\npub mod messages;\n".to_string(),
    });

    // For modules that reference siblings, re-export ports from the first sibling
    // instead of generating duplicate DomainError / shared traits.
    files.push(gen_traits(&contents, &crate_name, solution));

    // Impls targeting traits defined in this module (from anywhere in the tree),
    // or layer-provided generic ports (e.g. EntityRepo) implemented by product adapters.
    let trait_names: Vec<&str> = contents.traits.iter().map(|t| t.name.as_str()).collect();
    let layer_trait_names: Vec<&str> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Trait && c.layer_provided => {
                Some(c.name.as_str())
            }
            _ => None,
        })
        .collect();
    let impls_for_module: Vec<&Construct> = all_impls
        .iter()
        .filter(|i| {
            i.target.as_deref().map(|t| {
                trait_names.contains(&t) || layer_trait_names.contains(&t)
            }).unwrap_or(false)
        })
        .copied()
        .collect();
    // Merge layer-provided traits into the trait list for signature lookup.
    let mut traits_for_impls: Vec<&Construct> = contents.traits.to_vec();
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Trait
                && c.layer_provided
                && !traits_for_impls.iter().any(|t| t.name == c.name)
            {
                traits_for_impls.push(c);
            }
        }
    }
    files.push(gen_impls(
        &impls_for_module,
        &traits_for_impls,
        &crate_name,
        solution,
        registry,
    ));

    // Application: fn-shaped constructs in this module, plus top-level flows
    // (generated once, in the first module that has traits).
    let mut app_flows: Vec<FlowLike> = contents.fns.iter().map(|c| FlowLike::Construct(c)).collect();
    if !*flow_generated && !contents.traits.is_empty() && !top_level_flows.is_empty() {
        *flow_generated = true;
        app_flows.extend(top_level_flows.iter().map(|f| FlowLike::Flow(f)));
    }
    files.push(gen_application(&app_flows, &contents, &crate_name, solution, registry));

    // Generate manifest.json only for deployment units (constructs marked with `au`)
    if module.deployment_unit {
        files.push(gen_manifest(module, &contents, &impls_for_module, &crate_name, solution));
    }

    files
}


/// Generate a manifest.json describing the module's wiring requirements.
/// The runtime reads this to construct Deps and register Bus handlers.
fn gen_manifest(
    module: &Construct,
    contents: &ModuleContents,
    impls: &[&Construct],
    crate_name: &str,
    solution: &Solution,
) -> GeneratedFile {
    use serde_json::json;

    // Collect deps: each trait (port) that has an adapter implementing it
    let mut deps = serde_json::Map::new();
    for t in &contents.traits {
        let dep_name = to_snake(&t.name);
        let mut dep_info = serde_json::Map::new();
        dep_info.insert("trait".to_string(), json!(t.name));

        // Find the adapter that implements this trait
        if let Some(adapter) = impls.iter().find(|i| i.target.as_deref() == Some(&t.name)) {
            dep_info.insert("adapter".to_string(), json!(adapter.name));
            // Collect @env annotations for config requirements
            let env_vars: Vec<&str> = adapter.annotations.iter()
                .filter(|a| a.name == "env")
                .flat_map(|a| a.args.iter().map(|s| s.as_str()))
                .collect();
            if !env_vars.is_empty() {
                dep_info.insert("env".to_string(), json!(env_vars));
            }
        }

        deps.insert(dep_name, serde_json::Value::Object(dep_info));
    }

    // Layer-provided traits (from `declare` blocks) that have no adapter in
    // this module are provided by the runtime. This generalizes the old
    // Bus-only hardcode: Bus, AuthService, and any future runtime-injected
    // dependency all follow the same pattern.
    let layer_provided_traits: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Trait && c.layer_provided => Some(c),
            _ => None,
        })
        .collect();

    for t in &layer_provided_traits {
        let dep_name = to_snake(&t.name);
        if deps.contains_key(&dep_name) {
            // Already has an adapter defined in-module; skip runtime fallback
            continue;
        }
        let mut dep_info = serde_json::Map::new();
        dep_info.insert("trait".to_string(), json!(t.name));
        dep_info.insert("provided_by".to_string(), json!("runtime"));

        // Emit @strategy annotation if present (e.g. @strategy(cognito))
        if let Some(strategy_ann) = t.annotations.iter().find(|a| a.name == "strategy") {
            if let Some(strategy_value) = strategy_ann.args.first() {
                dep_info.insert("strategy".to_string(), json!(strategy_value));
            }
        }

        deps.insert(dep_name, serde_json::Value::Object(dep_info));
    }

    // Collect handlers: fn-shaped constructs in the application group
    // that have names starting with "Handle" (convention for Bus handlers)
    let mut handlers = serde_json::Map::new();
    for f in &contents.fns {
        let fn_name = to_snake(&f.name);
        // Derive the message name from the handler name
        // HandleGetCohort → GetCohort, HandleAddCohortMember → AddCohortMember
        let message_name = f.name.strip_prefix("Handle").unwrap_or(&f.name);
        handlers.insert(message_name.to_string(), json!({
            "function": fn_name,
            "inputs": f.inputs.iter().map(|i| {
                json!({ "name": i.name, "type": format!("{:?}", i.type_expr) })
            }).collect::<Vec<_>>(),
        }));
    }

    // The `expose` block lives on `Package` (pkg files), not on `Solution`
    // (sol files). For sol-based generation, expose info is empty. When
    // package-level codegen is added, this will extract from the Package AST.
    let expose_info: Vec<serde_json::Value> = Vec::new();

    let manifest = json!({
        "context": module.name,
        "crate": crate_name,
        "deps": deps,
        "handlers": handlers,
        "expose": expose_info,
    });

    GeneratedFile {
        path: format!("crates/{}/manifest.json", crate_name),
        content: serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    }
}

fn gen_types(
    contents: &ModuleContents,
    crate_name: &str,
    registry: &LayerRegistry,
    solution: &Solution,
) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Domain types.\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\nuse std::collections::HashMap;\nuse crate::ports::{ValidationError, DomainError};\nuse crate::domain::messages::*;\n\n");

    // Collect defined and referenced type names for stub generation.
    let mut defined_types: Vec<String> = Vec::new();
    let mut referenced: Vec<String> = Vec::new();

    for c in &contents.structs {
        defined_types.push(c.name.clone());
        collect_construct_type_refs(c, &mut referenced);
    }
    for e in &contents.enums {
        defined_types.push(e.name.clone());
    }
    // Traits (ports/repos) are defined in ports/mod.rs — exclude them from stubs.
    for t in &contents.traits {
        defined_types.push(t.name.clone());
        for method in &t.methods {
            for param in &method.params {
                collect_type_refs(&param.type_expr, &mut referenced);
            }
            if let Some(rt) = &method.return_type {
                collect_type_refs(rt, &mut referenced);
            }
        }
    }
    for f in &contents.fns {
        for input in &f.inputs {
            collect_type_refs(&input.type_expr, &mut referenced);
        }
    }
    // Enum-shaped named blocks define types too (e.g. `state CustomerStatus`).
    for c in &contents.structs {
        for block in &c.blocks {
            if block.shape == Shape::Enum {
                if let Some(name) = &block.name {
                    defined_types.push(name.clone());
                }
            }
        }
    }

    let builtin = [
        "Str", "Int", "F64", "Bool", "Bytes", "UUID", "Id", "DateTime", "Dt", "List", "Map", "Set", "Opt",
        "Res", "String", "Json",
    ];
    // Type params (T, U) and type-alias names (WearTestRepo) are not domain stubs.
    let mut skip_stubs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in &contents.traits {
        for p in &t.type_params {
            skip_stubs.insert(p.split(':').next().unwrap_or(p).trim().to_string());
        }
        skip_stubs.insert(t.name.clone());
    }
    for item in &solution.items {
        match item {
            TopLevelItem::TypeAlias { name, .. } => {
                skip_stubs.insert(name.clone());
            }
            TopLevelItem::Construct(c) if c.shape == Shape::Trait => {
                for p in &c.type_params {
                    skip_stubs.insert(p.split(':').next().unwrap_or(p).trim().to_string());
                }
                skip_stubs.insert(c.name.clone());
            }
            _ => {}
        }
    }
    let undefined: Vec<String> = referenced
        .iter()
        .filter(|t| {
            !defined_types.contains(t)
                && !builtin.contains(&t.as_str())
                && !skip_stubs.contains(*t)
        })
        .cloned()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if !undefined.is_empty() {
        out.push_str("// Stub types — replace with actual definitions\n");
        let mut sorted = undefined;
        sorted.sort();
        for t in &sorted {
            out.push_str(&format!("pub type {} = String;\n", t));
        }
        out.push('\n');
    }

    // Enums first (unit enums derive Default for fill-in). Nested VOs that are
    // all-defaultable join `defaultable_structs` so later aggregates can omit
    // them from smart-ctor params (`retry_settings: RetrySettings::default()`).
    // Domain enums stay as required ctor params (AuthType is intentional input).
    let mut defaultable_structs: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for e in &contents.enums {
        out.push_str(&gen_enum(e));
    }
    for c in &contents.structs {
        let (chunk, is_defaultable) = gen_struct(c, registry, &defaultable_structs);
        out.push_str(&chunk);
        if is_defaultable {
            defaultable_structs.insert(c.name.clone());
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/types.rs", crate_name),
        content: out,
    }
}

fn enum_is_unit_only(c: &Construct) -> bool {
    if !c.rich_variants.is_empty() {
        c.rich_variants
            .iter()
            .all(|v| matches!(v, EnumVariant::Unit(_)))
    } else {
        !c.variants.is_empty()
    }
}

/// Collect type references from a struct-shaped construct (fields + blocks + nested).
fn collect_construct_type_refs(c: &Construct, refs: &mut Vec<String>) {
    for field in &c.fields {
        collect_type_refs(&field.type_expr, refs);
    }
    for block in &c.blocks {
        for field in &block.fields {
            collect_type_refs(&field.type_expr, refs);
        }
    }
    for child in &c.children {
        if child.shape == Shape::Struct {
            for field in &child.fields {
                // Shorthand fields (type == name) use inferred types — skip.
                if matches!(&field.type_expr, TypeExpr::Named(n) if n == &field.name) {
                    continue;
                }
                collect_type_refs(&field.type_expr, refs);
            }
        }
    }
}

fn collect_type_refs(ty: &TypeExpr, refs: &mut Vec<String>) {
    match ty {
        TypeExpr::Named(name) => refs.push(name.clone()),
        TypeExpr::Generic(_, args) => {
            for arg in args {
                collect_type_refs(arg, refs);
            }
        }
        TypeExpr::Result(Some(inner)) => collect_type_refs(inner, refs),
        TypeExpr::Result(None) => {}
        TypeExpr::Optional(inner) => collect_type_refs(inner, refs),
        TypeExpr::List(inner) => collect_type_refs(inner, refs),
        TypeExpr::Map(k, v) => {
            collect_type_refs(k, refs);
            collect_type_refs(v, refs);
        }
        TypeExpr::Set(inner) => collect_type_refs(inner, refs),
        TypeExpr::Tuple(items) => { for item in items { collect_type_refs(item, refs); } }
        TypeExpr::Array(inner, _) => collect_type_refs(inner, refs),
        TypeExpr::Ref(inner, _) => collect_type_refs(inner, refs),
        TypeExpr::Dyn(inner) => collect_type_refs(inner, refs),
        TypeExpr::ImplTrait(inner) => collect_type_refs(inner, refs),
        TypeExpr::FnPtr(params, ret) => { for p in params { collect_type_refs(p, refs); } if let Some(r) = ret { collect_type_refs(r, refs); } }
    }
}

/// Collect stub-declared derives/attrs for domain structs used with that SDK.
/// Multi-field → `row_type_derives`; single-field → `wrapper_type_derives` + attrs.
fn stub_domain_type_attrs(registry: &LayerRegistry, is_single_field: bool) -> (String, String) {
    let mut row_derives: Vec<String> = Vec::new();
    let mut wrap_derives: Vec<String> = Vec::new();
    let mut wrap_attrs: Vec<String> = Vec::new();
    for stub in &registry.stubs {
        for d in &stub.row_type_derives {
            if !row_derives.contains(d) {
                row_derives.push(d.clone());
            }
        }
        for d in &stub.wrapper_type_derives {
            if !wrap_derives.contains(d) {
                wrap_derives.push(d.clone());
            }
        }
        for a in &stub.wrapper_type_attrs {
            if !wrap_attrs.contains(a) {
                wrap_attrs.push(a.clone());
            }
        }
    }
    if is_single_field && (!wrap_derives.is_empty() || !wrap_attrs.is_empty()) {
        let derive = if wrap_derives.is_empty() {
            String::new()
        } else {
            format!("\n#[derive({})]", wrap_derives.join(", "))
        };
        let attrs: String = wrap_attrs
            .iter()
            .map(|a| format!("\n#[{a}]"))
            .collect();
        // Wrapper derives are separate from Debug/Clone line when they're Type-only.
        // Wrapper derives on their own line; extra_derive on main Debug line stays empty.
        (String::new(), format!("{derive}{attrs}"))
    } else if !row_derives.is_empty() {
        (format!(", {}", row_derives.join(", ")), String::new())
    } else {
        (String::new(), String::new())
    }
}

/// Generate a struct-shaped construct: struct + enum blocks + invariant impl.
fn gen_struct(
    c: &Construct,
    registry: &LayerRegistry,
    defaultable: &std::collections::HashSet<String>,
) -> (String, bool) {
    let mut out = String::new();
    let has_invariant = c.annotations.iter().any(|a| a.name == "invariant");

    // Fields: direct plus struct-shaped named blocks (e.g. root).
    let mut fields: Vec<&Field> = c.fields.iter().collect();
    for block in &c.blocks {
        if block.shape != Shape::Enum {
            fields.extend(block.fields.iter());
        }
    }

    // Stub-driven derives/attrs (row drivers, serde crates, …) — no crate names here.
    let is_single_field = fields.len() == 1;
    let (extra_derive, extra_attr) = stub_domain_type_attrs(registry, is_single_field);
    out.push_str(&format!(
        "/// {}: {}\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize{})]{}\npub struct {}{} {{\n",
        c.subkind, c.name,
        extra_derive,
        extra_attr,
        c.name, generic_params_rust(&c.type_params)
    ));
    for field in &fields {
        let mut ty = type_to_rust(&field.type_expr);
        // PAR-014: optional @shared → Arc<T> (no lifetimes in .veil)
        if field.annotations.iter().any(|a| a.name == "shared") {
            ty = format!("std::sync::Arc<{ty}>");
        }
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake(&field.name),
            ty
        ));
    }
    out.push_str("}\n\n");

    // Enum-shaped named blocks become enums (e.g. state machines).
    for block in &c.blocks {
        if block.shape == Shape::Enum {
            let enum_name = block.name.clone().unwrap_or_else(|| format!("{}State", c.name));
            out.push_str(&format!(
                "/// States for {} ({} block)\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub enum {} {{\n",
                c.name, block.keyword, enum_name
            ));
            for v in &block.variants {
                out.push_str(&format!("    {},\n", v));
            }
            out.push_str("}\n\n");
        }
    }

    // INV-002: constructor auto-fields / type defaults from layer policy.
    let ctor_pol = if registry.constructor_policy.auto_fields.is_empty() {
        veil_ir::layer::ConstructorPolicy::rust_defaults()
    } else {
        registry.constructor_policy.clone()
    };

    if has_invariant {
        // Smart constructor with invariant validation — same field filtering as non-invariant
        let scalar_default_fields: std::collections::HashSet<String> = fields.iter()
            .filter(|f| matches!(&f.type_expr, TypeExpr::Named(n) if ctor_pol.type_default(n).is_some()))
            .map(|f| f.name.clone())
            .collect();

        let user_fields: Vec<&&Field> = fields.iter()
            .filter(|f| {
                !ctor_pol.is_auto_field(&f.name)
                && !scalar_default_fields.contains(&f.name)
                && !matches!(&f.type_expr, TypeExpr::Optional(_))
                && !matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option")
            })
            .collect();

        let params_str = user_fields.iter()
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>().join(", ");

        let init_fields = fields.iter().map(|f| {
            let snake = to_snake(&f.name);
            if ctor_pol.is_auto_field(&f.name) {
                let is_optional = matches!(&f.type_expr, TypeExpr::Optional(_))
                    || matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option");
                if is_optional { format!("{}: None", snake) } else { format!("{}: Utc::now()", snake) }
            } else if scalar_default_fields.contains(&f.name) {
                let default = match &f.type_expr {
                    TypeExpr::Named(n) => ctor_pol.type_default(n).unwrap_or("0"),
                    _ => "0",
                };
                format!("{}: {}", snake, default)
            } else if matches!(&f.type_expr, TypeExpr::Optional(_)) || matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option") {
                format!("{}: None", snake)
            } else {
                snake
            }
        }).collect::<Vec<_>>().join(", ");

        out.push_str(&format!(
            "impl {} {{\n    pub fn new({}) -> Result<Self, ValidationError> {{\n        let value = Self {{ {} }};\n        value.validate()?;\n        Ok(value)\n    }}\n\n    pub fn validate(&self) -> Result<(), ValidationError> {{\n        Ok(())\n    }}\n}}\n\n",
            c.name, params_str, init_fields,
        ));
    } else if !fields.is_empty() {
        // Generate a smart constructor — auto-defaulting timestamps / scalars (INV-002 policy)
        // id is accepted as a parameter — callers provide it (or pass Uuid::new_v4())
        // Enum-typed fields (like status) get their first variant as default
        let enum_field_names: std::collections::HashSet<String> = c.blocks.iter()
            .filter(|b| b.shape == Shape::Enum)
            .flat_map(|b| {
                // Find which field references this enum by matching type name
                fields.iter().filter(|f| {
                    if let TypeExpr::Named(n) = &f.type_expr {
                        b.name.as_ref().map(|bn| bn == n).unwrap_or(false)
                    } else { false }
                }).map(|f| f.name.clone())
            }).collect();

        // INV-002: scalar type defaults (Int/Bool/…) apply to every struct shape —
        // no subkind branching (MISSION: zero domain knowledge).
        let scalar_default_fields: std::collections::HashSet<String> = fields
            .iter()
            .filter(|f| {
                matches!(
                    &f.type_expr,
                    TypeExpr::Named(n) if ctor_pol.type_default(n).is_some()
                )
            })
            .map(|f| f.name.clone())
            .collect();

        // Empty collections default like scalars so call sites can pass only
        // non-defaultable fields (e.g. name/url/auth, not embedded lists).
        let collection_default_fields: std::collections::HashSet<String> = fields
            .iter()
            .filter(|f| field_has_empty_collection_default(&f.type_expr))
            .map(|f| f.name.clone())
            .collect();

        let user_fields: Vec<&&Field> = fields
            .iter()
            .filter(|f| {
                field_is_required_ctor_param(f, &ctor_pol, &enum_field_names, defaultable)
            })
            .collect();

        let params_str = user_fields.iter()
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>().join(", ");

        let init_fields = fields.iter().map(|f| {
            let snake = to_snake(&f.name);
            if ctor_pol.is_auto_field(&f.name) {
                // Timestamp fields: use Utc::now() for non-optional, None for optional
                let is_optional = matches!(&f.type_expr,
                    TypeExpr::Generic(name, _) if name == "Opt" || name == "Option")
                    || matches!(&f.type_expr, TypeExpr::Optional(_));
                if is_optional {
                    format!("{}: None", snake)
                } else {
                    format!("{}: Utc::now()", snake)
                }
            } else if scalar_default_fields.contains(&f.name) {
                let default = match &f.type_expr {
                    TypeExpr::Named(n) => ctor_pol.type_default(n).unwrap_or("0"),
                    _ => "0",
                };
                format!("{}: {}", snake, default)
            } else if collection_default_fields.contains(&f.name) {
                format!("{}: {}", snake, empty_collection_default(&f.type_expr))
            } else if let Some(sdef) = string_field_default(&f.name) {
                format!("{}: {}", snake, sdef)
            } else if field_has_named_default(&f.type_expr, defaultable) {
                let ty_name = match &f.type_expr {
                    TypeExpr::Named(n) => n.as_str(),
                    _ => "Default",
                };
                format!("{}: {}::default()", snake, ty_name)
            } else if enum_field_names.contains(&f.name) {
                // Use first variant of the enum
                let first_variant = c.blocks.iter()
                    .filter(|b| b.shape == Shape::Enum)
                    .find_map(|b| {
                        let enum_name = b.name.clone().unwrap_or_else(|| format!("{}State", c.name));
                        if let TypeExpr::Named(n) = &f.type_expr {
                            if &enum_name == n {
                                return b.variants.first().map(|v| format!("{}::{}", enum_name, v));
                            }
                        }
                        None
                    })
                    .unwrap_or_else(|| format!("Default::default()"));
                format!("{}: {}", snake, first_variant)
            } else if matches!(&f.type_expr, TypeExpr::Optional(_)) || matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option") {
                // Optional fields default to None
                format!("{}: None", snake)
            } else {
                snake
            }
        }).collect::<Vec<_>>().join(", ");

        out.push_str(&format!(
            "impl {} {{\n    pub fn new({}) -> Self {{\n        Self {{ {} }}\n    }}\n}}\n\n",
            c.name, params_str, init_fields,
        ));

        // Emit `Default` when every field is fillable without caller input
        // (zero-arg `new()`). Call sites like `T.new(a,b,c)` on such types
        // lower to a positional struct update via `defaultable_types` in GenCtx.
        if user_fields.is_empty() {
            out.push_str(&format!(
                "impl Default for {} {{\n    fn default() -> Self {{\n        Self::new()\n    }}\n}}\n\n",
                c.name
            ));
        }
    }

    // Generate impl block with business logic fns (if any exist).
    if !c.fns.is_empty() {
        out.push_str(&gen_aggregate_impl(c, &fields));
    }

    // Types with zero-arg smart ctors (all fields defaultable) are reusable as
    // nested `Type::default()` and as partial-init targets.
    let is_defaultable = !has_invariant
        && fields.iter().all(|f| {
            let ctor_pol = if registry.constructor_policy.auto_fields.is_empty() {
                veil_ir::layer::ConstructorPolicy::rust_defaults()
            } else {
                registry.constructor_policy.clone()
            };
            let enum_field_names: std::collections::HashSet<String> = c
                .blocks
                .iter()
                .filter(|b| b.shape == Shape::Enum)
                .flat_map(|b| {
                    fields
                        .iter()
                        .filter(|ff| {
                            if let TypeExpr::Named(n) = &ff.type_expr {
                                b.name.as_ref().map(|bn| bn == n).unwrap_or(false)
                            } else {
                                false
                            }
                        })
                        .map(|ff| ff.name.clone())
                })
                .collect();
            !field_is_required_ctor_param(f, &ctor_pol, &enum_field_names, defaultable)
        });

    (out, is_defaultable)
}

/// True when the field must appear as a `new(...)` parameter (shape/type policy only).
fn field_is_required_ctor_param(
    f: &Field,
    ctor_pol: &veil_ir::layer::ConstructorPolicy,
    enum_field_names: &std::collections::HashSet<String>,
    defaultable: &std::collections::HashSet<String>,
) -> bool {
    if ctor_pol.is_auto_field(&f.name) {
        return false;
    }
    if enum_field_names.contains(&f.name) {
        return false;
    }
    if matches!(
        &f.type_expr,
        TypeExpr::Named(n) if ctor_pol.type_default(n).is_some()
    ) {
        return false;
    }
    if field_has_empty_collection_default(&f.type_expr) {
        return false;
    }
    if field_has_named_default(&f.type_expr, defaultable) {
        return false;
    }
    if string_field_default(&f.name).is_some() {
        return false;
    }
    if matches!(&f.type_expr, TypeExpr::Optional(_))
        || matches!(
            &f.type_expr,
            TypeExpr::Generic(name, _) if name == "Opt" || name == "Option"
        )
    {
        return false;
    }
    true
}

fn field_has_named_default(
    ty: &TypeExpr,
    defaultable: &std::collections::HashSet<String>,
) -> bool {
    match ty {
        TypeExpr::Named(n) => defaultable.contains(n),
        _ => false,
    }
}

/// Conventional string defaults for known field names (not domain magic —
/// common infrastructure field conventions used across adapters).
fn string_field_default(field_name: &str) -> Option<&'static str> {
    match field_name {
        "authorization_header_string" => Some("\"Authorization\".to_string()"),
        _ => None,
    }
}

/// Generate `impl Name { ... }` block for aggregate business logic fns.
fn gen_aggregate_impl(c: &Construct, fields: &[&Field]) -> String {
    use crate::expr::{GenCtx, expr_to_rust};
    use std::collections::HashMap;

    let mut out = String::new();

    // Determine the event wrapper enum name from children with emit targets
    // The enum is named {ParentName}{ChildSubkind} — find the first emittable child's subkind
    let event_subkind = c.children.iter()
        .find(|child| child.shape == Shape::Struct)
        .map(|child| child.subkind.clone())
        .unwrap_or_else(|| "Event".to_string());
    let event_enum_name = format!("{}{}", c.name, event_subkind);

    // Collect field names for self-field detection
    let field_names: std::collections::HashSet<String> = fields.iter()
        .map(|f| f.name.clone())
        .collect();

    // Collect enum block variants for enum-value qualification
    let mut enum_map: HashMap<String, String> = HashMap::new(); // variant → EnumName
    for block in &c.blocks {
        if block.shape == Shape::Enum {
            let enum_name = block.name.clone().unwrap_or_else(|| format!("{}State", c.name));
            for v in &block.variants {
                enum_map.insert(v.clone(), enum_name.clone());
            }
        }
    }

    out.push_str(&format!("impl {} {{\n", c.name));

    for func in &c.fns {
        let params_str = func.params.iter()
            .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
            .collect::<Vec<_>>().join(", ");

        // Explicit return type from the VEIL signature; otherwise event-collecting
        // methods default to `Result<Vec<Events>, DomainError>`.
        let has_explicit_return = func.return_type.as_ref()
            .map(|t| !matches!(t, TypeExpr::Result(None)))
            .unwrap_or(false);
        let return_type_str = if has_explicit_return {
            func.return_type.as_ref()
                .map(|t| type_to_rust(t))
                .unwrap_or_else(|| format!("Result<Vec<{}>, DomainError>", event_enum_name))
        } else {
            format!("Result<Vec<{}>, DomainError>", event_enum_name)
        };

        // Pure query methods use `&self`; mutations / emits need `&mut self`.
        let needs_mut_self = method_body_mutates_self(&func.body, &field_names);
        let self_recv = if needs_mut_self { "&mut self" } else { "&self" };
        // Only allocate an events bag when the body emits or the default return is events.
        let needs_events = method_body_has_emit(&func.body)
            || (!has_explicit_return && !has_explicit_return_stmt(&func.body));

        out.push_str(&format!(
            "    pub fn {}({}{}) -> {} {{\n",
            to_snake(&func.name),
            self_recv,
            if params_str.is_empty() { String::new() } else { format!(", {}", params_str) },
            return_type_str
        ));

        // @invariant annotation → guard
        for ann in &func.annotations {
            if ann.name == "invariant" {
                let cond_text = ann.args.first().map(|s| s.as_str()).unwrap_or("true");
                // Simple invariant: field == Value → self.field == EnumName::Value
                let cond_rust = translate_invariant_condition(cond_text, &field_names, &enum_map);
                out.push_str(&format!(
                    "        if !({}) {{ return Err(DomainError::Validation(\"invariant violated\".into())); }}\n",
                    cond_rust
                ));
            }
        }

        if needs_events {
            out.push_str(&format!("        let mut events: Vec<{}> = Vec::new();\n", event_enum_name));
        }

        // Build context for body translation — thread the real return type so
        // `ret x` matches Option vs Result signatures (not default Ok-wrap).
        let mut ctx = GenCtx::new(HashMap::new());
        ctx.in_method = true;
        ctx.self_fields = field_names.clone();
        ctx.expected_return_rust = Some(return_type_str.clone());
        // Seed struct field types so `for x in self.list` can type elements.
        ctx.struct_fields.insert(
            c.name.clone(),
            fields
                .iter()
                .map(|f| (f.name.clone(), type_name_for_field(&f.type_expr)))
                .collect(),
        );
        for p in &func.params {
            ctx.locals.insert(p.name.clone());
            ctx.local_types
                .insert(p.name.clone(), type_to_rust(&p.type_expr));
        }

        let mut has_explicit_ret = false;
        for expr in &func.body {
            match expr {
                Expr::Assign(field, rhs, _) | Expr::MutAssign(field, rhs, _) if field_names.contains(field) => {
                    // Assign to a struct field: self.field = value
                    let rhs_str = expr_to_rust(rhs, &ctx);
                    // If the rhs is a bare ident that matches an enum variant, qualify it
                    let qualified_rhs = if let Expr::Ident(v) = rhs.as_ref() {
                        if let Some(enum_name) = enum_map.get(v.as_str()) {
                            format!("{}::{}", enum_name, v)
                        } else {
                            rhs_str
                        }
                    } else {
                        rhs_str
                    };
                    out.push_str(&format!("        self.{} = {};\n", to_snake(field), qualified_rhs));
                }
                Expr::Action(a) if a.keyword == "emit" => {
                    // emit EventName{fields} → events.push(ParentEvent::EventName(EventName { fields }))
                    let event_name = &a.target;
                    // Look up the event struct's actual field names from children
                    let event_fields: Vec<String> = c.children.iter()
                        .find(|child| child.name == *event_name)
                        .map(|child| child.fields.iter().map(|f| f.name.clone()).collect())
                        .unwrap_or_default();

                    let fields_str = if !a.named_args.is_empty() {
                        // Map positionally: use event struct field names, values from named_args
                        a.named_args.iter().enumerate().map(|(i, (_k, v))| {
                            let v_str = translate_emit_field(v, &ctx, &field_names);
                            let field_name = event_fields.get(i)
                                .map(|n| to_snake(n))
                                .unwrap_or_else(|| to_snake(_k));
                            if field_name == v_str { field_name } else { format!("{}: {}", field_name, v_str) }
                        }).collect::<Vec<_>>().join(", ")
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "        events.push({}::{}({} {{ {} }}));\n",
                        event_enum_name, event_name, event_name, fields_str
                    ));
                }
                other => {
                    if matches!(other, Expr::Return(_)) {
                        has_explicit_ret = true;
                    }
                    out.push_str(&format!("        {};\n", expr_to_rust(other, &ctx)));
                    // Register let-bindings *after* lowering so the first
                    // occurrence emits `let mut x = …`, and later statements
                    // treat `x` as a local (`out.insert` not `out_insert`).
                    if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = other {
                        if !name.contains('.') && !field_names.contains(name) {
                            ctx.locals.insert(name.clone());
                            if let Some(t) = crate::expr::infer_expr_type_pub(rhs, &ctx) {
                                ctx.local_types.insert(name.clone(), t);
                            }
                        }
                    }
                }
            }
        }

        // Only append Ok(events) if the method doesn't have an explicit return value
        if !has_explicit_ret && !has_explicit_return {
            out.push_str("        Ok(events)\n");
        }
        out.push_str("    }\n\n");
    }

    out.push_str("}\n\n");
    out
}

/// Does the method body assign to `self` fields or emit domain events?
fn method_body_mutates_self(body: &[Expr], field_names: &std::collections::HashSet<String>) -> bool {
    body.iter().any(|e| expr_mutates_self(e, field_names))
}

fn expr_mutates_self(expr: &Expr, field_names: &std::collections::HashSet<String>) -> bool {
    match expr {
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            if field_names.contains(name) || name.starts_with("self.") {
                return true;
            }
            expr_mutates_self(rhs, field_names)
        }
        Expr::Action(a) if a.keyword == "emit" => true,
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(|e| expr_mutates_self(e, field_names))
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(|e| expr_mutates_self(e, field_names)))
                    .unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } => {
            body.iter().any(|e| expr_mutates_self(e, field_names))
        }
        Expr::Match(_, arms) => arms
            .iter()
            .any(|arm| arm.body.iter().any(|e| expr_mutates_self(e, field_names))),
        _ => false,
    }
}

fn method_body_has_emit(body: &[Expr]) -> bool {
    body.iter().any(expr_has_emit)
}

fn expr_has_emit(expr: &Expr) -> bool {
    match expr {
        Expr::Action(a) if a.keyword == "emit" => true,
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(expr_has_emit)
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_has_emit))
                    .unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } => {
            body.iter().any(expr_has_emit)
        }
        Expr::Match(_, arms) => arms.iter().any(|arm| arm.body.iter().any(expr_has_emit)),
        _ => false,
    }
}

fn has_explicit_return_stmt(body: &[Expr]) -> bool {
    body.iter().any(expr_has_return)
}

fn expr_has_return(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(expr_has_return)
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_has_return))
                    .unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } => {
            body.iter().any(expr_has_return)
        }
        Expr::Match(_, arms) => arms.iter().any(|arm| arm.body.iter().any(expr_has_return)),
        _ => false,
    }
}

/// Type name stored on struct_fields for element/type inference (Rust form).
fn type_name_for_field(ty: &TypeExpr) -> String {
    type_to_rust(ty)
}

/// Empty collection defaults for smart constructors (List → vec![], Map → HashMap::new()).
fn field_has_empty_collection_default(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::List(_) | TypeExpr::Map(_, _) | TypeExpr::Set(_) => true,
        TypeExpr::Generic(name, _) => {
            matches!(
                name.as_str(),
                "List" | "Map" | "Set" | "Vec" | "HashMap" | "HashSet"
            )
        }
        _ => false,
    }
}

fn empty_collection_default(ty: &TypeExpr) -> &'static str {
    match ty {
        TypeExpr::List(_) => "Vec::new()",
        TypeExpr::Set(_) => "std::collections::HashSet::new()",
        TypeExpr::Map(_, _) => "std::collections::HashMap::new()",
        TypeExpr::Generic(name, _) => match name.as_str() {
            "List" | "Vec" => "Vec::new()",
            "Set" | "HashSet" => "std::collections::HashSet::new()",
            "Map" | "HashMap" => "std::collections::HashMap::new()",
            _ => "Default::default()",
        },
        _ => "Default::default()",
    }
}

/// Translate an invariant condition expression (simple text form).
/// e.g. "status == Pending" → "self.status == CustomerStatus::Pending"
fn translate_invariant_condition(
    text: &str,
    fields: &std::collections::HashSet<String>,
    enum_map: &std::collections::HashMap<String, String>,
) -> String {
    // Simple parser: split on spaces, qualify fields with self. and enum values with EnumName::
    let parts: Vec<&str> = text.split_whitespace().collect();
    parts.iter().map(|part| {
        if fields.contains(*part) {
            format!("self.{}", to_snake(part))
        } else if let Some(enum_name) = enum_map.get(*part) {
            format!("{}::{}", enum_name, part)
        } else {
            part.to_string()
        }
    }).collect::<Vec<_>>().join(" ")
}

/// Translate a field value in an emit expression.
/// Bare field names that match struct fields → self.field
/// now() → Utc::now()
fn translate_emit_field(
    expr: &Expr,
    ctx: &crate::expr::GenCtx,
    self_fields: &std::collections::HashSet<String>,
) -> String {
    match expr {
        Expr::Ident(name) if self_fields.contains(name.as_str()) => {
            format!("self.{}", to_snake(name))
        }
        Expr::Ident(name) => {
            // Local variables need .clone() to avoid move issues when
            // the value is also used after the emit (e.g. in a return).
            format!("{}.clone()", to_snake(name))
        }
        Expr::Call(call) if call.target == "now" && call.method.is_empty() => {
            "Utc::now()".to_string()
        }
        _ => crate::expr::expr_to_rust(expr, ctx),
    }
}

/// Generate messages.rs: structs nested inside other structs (events,
/// Generate an enum-shaped construct.
fn gen_enum(c: &Construct) -> String {
    let mut out = String::new();
    // Unit-only enums get Default (first variant) so partial smart-ctors can
    // fill omitted enum fields via `Enum::default()`.
    let unit_only = if !c.rich_variants.is_empty() {
        c.rich_variants
            .iter()
            .all(|v| matches!(v, EnumVariant::Unit(_)))
    } else {
        !c.variants.is_empty()
    };
    let derives = if unit_only {
        "Debug, Clone, PartialEq, Serialize, Deserialize, Default"
    } else {
        "Debug, Clone, PartialEq, Serialize, Deserialize"
    };
    out.push_str(&format!(
        "/// {}: {}\n#[derive({})]\npub enum {}{} {{\n",
        c.subkind, c.name, derives, c.name, generic_params_rust(&c.type_params)
    ));

    // Use rich_variants if available, otherwise fall back to flat string variants
    if !c.rich_variants.is_empty() {
        let mut first = true;
        for v in &c.rich_variants {
            match v {
                EnumVariant::Unit(name) => {
                    if unit_only && first {
                        out.push_str("    #[default]\n");
                        first = false;
                    }
                    out.push_str(&format!("    {},\n", name));
                }
                EnumVariant::Tuple(name, types) => {
                    let fields = types.iter().map(type_to_rust).collect::<Vec<_>>().join(", ");
                    out.push_str(&format!("    {}({}),\n", name, fields));
                }
                EnumVariant::Struct(name, fields) => {
                    out.push_str(&format!("    {} {{\n", name));
                    for f in fields {
                        out.push_str(&format!("        {}: {},\n", to_snake(&f.name), type_to_rust(&f.type_expr)));
                    }
                    out.push_str("    },\n");
                }
            }
        }
    } else {
        for (i, v) in c.variants.iter().enumerate() {
            if unit_only && i == 0 {
                out.push_str("    #[default]\n");
            }
            out.push_str(&format!("    {},\n", v));
        }
    }

    out.push_str("}\n\n");
    out
}

/// commands, or any layer-defined message-like constructs).
fn gen_child_types(contents: &ModuleContents, crate_name: &str) -> GeneratedFile {

    let mut out = String::new();
    out.push_str("//! Nested message types (grouped by parent construct).\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\n\nuse super::types::*;\n\n");

    let mut any = false;
    for parent in &contents.structs {
        // Group children by subkind so each layer concept gets its own enum.
        let mut by_subkind: Vec<(&str, Vec<&Construct>)> = Vec::new();
        for child in &parent.children {
            if child.shape != Shape::Struct {
                continue;
            }
            if let Some(entry) = by_subkind.iter_mut().find(|(k, _)| *k == child.subkind) {
                entry.1.push(child);
            } else {
                by_subkind.push((child.subkind.as_str(), vec![child]));
            }
        }
        for (subkind, children) in &by_subkind {
            any = true;
            // Wrapper enum per (parent, subkind): e.g. CustomerEvent.
            let enum_name = format!("{}{}", parent.name, subkind);
            out.push_str(&format!(
                "/// {} messages for {}\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub enum {} {{\n",
                subkind, parent.name, enum_name
            ));
            for child in children {
                out.push_str(&format!("    {}({}),\n", child.name, child.name));
            }
            out.push_str("}\n\n");

            for child in children {
                out.push_str(&format!(
                    "#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {} {{\n",
                    child.name
                ));
                for field in &child.fields {
                    // Shorthand fields (type == name) get inferred types.
                    let rust_type = match &field.type_expr {
                        TypeExpr::Named(n) if n == &field.name => infer_field_type(&field.name),
                        other => type_to_rust(other),
                    };
                    out.push_str(&format!("    pub {}: {},\n", to_snake(&field.name), rust_type));
                }
                out.push_str("}\n\n");
            }
        }
    }

    if !any {
        out.push_str("// No nested message types defined in this module.\n");
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/messages.rs", crate_name),
        content: out,
    }
}

/// RT-008: local allow-all auth (dev default; host swaps via strategy).
const ALLOW_ALL_AUTH_IMPL: &str = r#"
/// Dev/local AuthService — allows all tokens and permissions (RT-008).
/// Host harnesses replace this with Cognito/Auth0/etc. via `provided_by: runtime`.
pub struct AllowAllAuth;

#[async_trait]
impl AuthService for AllowAllAuth {
    async fn validate_token(&self, token: String) -> Result<Principal, DomainError> {
        Ok(Principal {
            id: if token.is_empty() {
                "anonymous".into()
            } else {
                token
            },
            roles: vec!["local".into()],
            claims: std::collections::HashMap::new(),
        })
    }

    async fn check_permission(
        &self,
        _principal: Principal,
        _permission: String,
    ) -> Result<bool, DomainError> {
        Ok(true)
    }
}
"#;

/// RT-001/004: default local Bus implementation (monolith topology).
const INPROCESS_BUS_IMPL: &str = r#"
// ─── InProcessBus (local harness, RT-001 / RT-004) ─────────────────────────
use std::collections::HashMap;
use std::sync::Arc;
use futures::future::BoxFuture;
use futures::FutureExt;

type BusHandler = Arc<
    dyn Fn(serde_json::Value) -> BoxFuture<'static, Result<serde_json::Value, DomainError>>
        + Send
        + Sync,
>;

/// In-process message bus for local multi-context runs.
#[derive(Clone, Default)]
pub struct InProcessBus {
    handlers: Arc<std::sync::Mutex<HashMap<String, BusHandler>>>,
}

impl InProcessBus {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Register a handler for a message type name (manifest `handlers` keys).
    pub fn register<F, Fut>(&self, name: impl Into<String>, f: F)
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, DomainError>> + Send + 'static,
    {
        let name = name.into();
        let handler: BusHandler = Arc::new(move |v| f(v).boxed());
        self.handlers
            .lock()
            .expect("bus lock")
            .insert(name, handler);
    }

    fn lookup(&self, type_name: &str) -> Option<BusHandler> {
        self.handlers
            .lock()
            .expect("bus lock")
            .get(type_name)
            .cloned()
    }
}

#[async_trait]
impl Bus for InProcessBus {
    async fn dispatch(&self, evt: serde_json::Value) -> Result<(), DomainError> {
        let type_name = evt
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(handler) = self.lookup(&type_name) {
            let payload = evt.clone();
            tokio::spawn(async move {
                let _ = handler(payload).await;
            });
        }
        Ok(())
    }

    async fn invoke(&self, cmd: serde_json::Value) -> Result<serde_json::Value, DomainError> {
        let type_name = cmd
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let handler = self
            .lookup(&type_name)
            .ok_or(DomainError::NotFound)?;
        handler(cmd).await
    }

    async fn request(&self, qry: serde_json::Value) -> Result<serde_json::Value, DomainError> {
        self.invoke(qry).await
    }
}
"#;

/// Generate the shared library crate that all context crates depend on. It
/// owns the common error types and layer-provided top-level traits, so there
/// is exactly one definition of each across the workspace.
/// CAP-003: handler message names from application fns across modules.
fn collect_handler_names(solution: &Solution, modules: &[&Construct]) -> Vec<String> {
    let mut names = Vec::new();
    for module in modules {
        let flat = flatten_module(module);
        for f in &flat.fns {
            // HandleX → X; DomainService CreateRepo → CreateRepo
            let message = f
                .name
                .strip_prefix("Handle")
                .unwrap_or(&f.name)
                .to_string();
            if !names.contains(&message) {
                names.push(message);
            }
        }
    }
    // Also free functions with Handle prefix at solution top-level
    for item in &solution.items {
        if let TopLevelItem::Function(f) = item {
            if let Some(msg) = f.name.strip_prefix("Handle") {
                let m = msg.to_string();
                if !names.contains(&m) {
                    names.push(m);
                }
            }
        }
    }
    names.sort();
    names
}

fn gen_register_handlers_module(handler_names: &[String]) -> String {
    let mut out = String::from(
        "//! CAP-003: generated Bus handler registry.\n\
         //! Host calls `register_all` once to wire names → dispatch.\n\n",
    );
    out.push_str("/// All Bus message types exported by this workspace.\n");
    out.push_str("pub const HANDLER_NAMES: &[&str] = &[\n");
    for n in handler_names {
        out.push_str(&format!("    \"{n}\",\n"));
    }
    out.push_str("];\n\n");
    out.push_str(
        "/// Register every generated handler name with a host-supplied registrar.\n\
         ///\n\
         /// The host provides the actual dispatch (ports / platform). This module\n\
         /// only owns the name list so trampoline code never hardcodes it.\n\
         pub fn register_all<F>(mut register: F)\n\
         where\n\
             F: FnMut(&'static str),\n\
         {\n\
             for name in HANDLER_NAMES {\n\
                 register(name);\n\
             }\n\
         }\n\n\
         /// Number of handlers in this workspace.\n\
         pub fn handler_count() -> usize {\n\
             HANDLER_NAMES.len()\n\
         }\n",
    );
    out
}

fn gen_shared_crate(
    traits: &[&Construct],
    structs: &[&Construct],
    functions: &[&FnDef],
    solution: &Solution,
    registry: &LayerRegistry,
    links: &[crate::links::ResolvedLink],
    handler_names: &[String],
) -> Vec<GeneratedFile> {
    use crate::expr::{build_ctx_from_solution, stmt_to_rust};
    let mut files = Vec::new();

    let mut shared_cargo = String::from(
        r#"[package]
name = "veil_shared"
version.workspace = true
edition.workspace = true

[dependencies]
async-trait.workspace = true
thiserror.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
tokio = { workspace = true }
futures = "0.3"
"#,
    );
    // CAP-001: allow shared layer decls / free fns to call linked crates.
    for link in links {
        shared_cargo.push_str(&crate::links::cargo_workspace_dep_line(link));
    }
    files.push(GeneratedFile {
        path: "crates/veil_shared/Cargo.toml".to_string(),
        content: shared_cargo,
    });

    // CAP-003: always emit register_handlers module (may be empty list).
    files.push(GeneratedFile {
        path: "crates/veil_shared/src/register_handlers.rs".into(),
        content: gen_register_handlers_module(handler_names),
    });

    let mut lib = String::new();
    lib.push_str("//! Shared types across all context crates — common errors and\n");
    lib.push_str("//! layer-provided infrastructure traits (routing ports, etc.).\n\n");
    lib.push_str("#![allow(unused_imports)]\n\n");
    lib.push_str("pub mod register_handlers;\n");
    lib.push_str("pub use register_handlers::{handler_count, register_all, HANDLER_NAMES};\n\n");
    lib.push_str("use async_trait::async_trait;\nuse uuid::Uuid;\n\n");
    lib.push_str("/// Domain error type.\n#[derive(Debug, thiserror::Error)]\npub enum DomainError {\n");
    lib.push_str("    #[error(\"Not found\")]\n    NotFound,\n");
    lib.push_str("    #[error(\"Validation failed: {0}\")]\n    Validation(String),\n");
    lib.push_str("    #[error(\"External service error: {0}\")]\n    External(String),\n");
    lib.push_str("}\n\n");
    lib.push_str("/// Validation error type.\n#[derive(Debug, thiserror::Error)]\n#[error(\"Validation error: {0}\")]\npub struct ValidationError(pub String);\n\nimpl From<ValidationError> for DomainError {\n    fn from(e: ValidationError) -> Self {\n        DomainError::Validation(e.0)\n    }\n}\n\n");
    lib.push_str("impl From<serde_json::Error> for DomainError {\n    fn from(e: serde_json::Error) -> Self {\n        DomainError::External(e.to_string())\n    }\n}\n\n");

    // Trait names in scope — used to box value-position references (List<Trait>).
    let trait_names: std::collections::HashSet<String> =
        traits.iter().map(|t| t.name.clone()).collect();

    // Local harness impls: emit InProcessBus when a routing trait is declared
    // (layer statement maps_to), not when a hard-coded name appears.
    let routing = registry.routing_traits();
    let mut has_routing_trait = false;
    let mut has_auth = false;
    for t in traits {
        if routing.iter().any(|r| r == &t.name) {
            has_routing_trait = true;
        }
        // AuthService is still name-keyed residual (RT-008); prefer layer
        // strategy metadata when that path is cleaned up.
        if t.name == "AuthService" {
            has_auth = true;
        }
        let tp = generic_params_rust(&t.type_params);
        let where_bounds = if t.type_params.is_empty() {
            String::new()
        } else {
            let clauses: Vec<String> = t
                .type_params
                .iter()
                .map(|p| {
                    let name = p.split(':').next().unwrap_or(p).trim();
                    format!("{name}: Send + Sync + 'static")
                })
                .collect();
            format!("\nwhere\n    {}", clauses.join(",\n    "))
        };
        lib.push_str(&format!(
            "/// {}: {}\n#[async_trait]\npub trait {}{}: Send + Sync{where_bounds} {{\n",
            t.subkind, t.name, t.name, tp
        ));
        for method in &t.methods {
            let params = method
                .params
                .iter()
                .map(|p| format!("{}: {}", to_snake(&p.name), param_type_to_rust(&p.type_expr, &trait_names)))
                .collect::<Vec<_>>()
                .join(", ");
            let sep = if params.is_empty() { "" } else { ", " };
            let ret = match &method.return_type {
                Some(t) => format!(" -> {}", type_to_rust_with_traits(t, &trait_names)),
                None => String::new(),
            };
            lib.push_str(&format!("    async fn {}(&self{}{}){ret};\n", to_snake(&method.name), sep, params));
        }
        lib.push_str("}\n\n");
    }

    // RT-001 / RT-004: local InProcessBus when a routing trait is present.
    // Residual: the impl body still names `Bus` / dispatch|invoke|request —
    // long-term this should be layer-declared or generated from the trait surface.
    if has_routing_trait {
        lib.push_str(INPROCESS_BUS_IMPL);
    }
    // RT-008: local allow-all AuthService for dev harness.
    if has_auth {
        lib.push_str(ALLOW_ALL_AUTH_IMPL);
    }

    // Emit layer-provided structs (e.g. Principal) so traits can reference them.
    for s in structs {
        lib.push_str(&format!("/// Layer-provided struct: {}\n", s.name));
        lib.push_str("#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]\n");
        lib.push_str(&format!("pub struct {} {{\n", s.name));
        for field in &s.fields {
            let ft = type_to_rust(&field.type_expr);
            lib.push_str(&format!("    pub {}: {},\n", to_snake(&field.name), ft));
        }
        // Also check named blocks (root, etc.)
        for block in &s.blocks {
            if block.shape != Shape::Enum {
                for field in &block.fields {
                    let ft = type_to_rust(&field.type_expr);
                    lib.push_str(&format!("    pub {}: {},\n", to_snake(&field.name), ft));
                }
            }
        }
        lib.push_str("}\n\n");
    }

    // Emit layer-declared free functions (e.g. the saga coordinator). The
    // author declares any Bus/step params explicitly; a bare trait-typed
    // parameter is passed by shared reference.
    for f in functions {
        let name_to_shape = build_name_to_shape(solution, registry);
        let mut ctx = build_ctx_from_solution(solution, name_to_shape, registry);
        for p in &f.params {
            ctx.locals.insert(p.name.clone());
            // Track the trait name (unboxed) so method calls resolve to .await?.
            ctx.local_types.insert(p.name.clone(), local_type_for_param(&p.type_expr, &trait_names));
        }

        let params = f
            .params
            .iter()
            .map(|p| format!("{}: {}", to_snake(&p.name), param_type_to_rust(&p.type_expr, &trait_names)))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = match &f.return_type {
            Some(t) => type_to_rust_with_traits(t, &trait_names),
            None => "Result<(), DomainError>".to_string(),
        };
        lib.push_str(&format!(
            "/// Layer-declared coordinator.\npub async fn {}({}) -> {} {{\n",
            to_snake(&f.name),
            params,
            ret,
        ));
        for expr in &f.body {
            // stmt_to_rust tracks let-bindings so `mut x` then `x = ..` becomes
            // a declaration then a reassignment (not shadowing).
            lib.push_str(&format!("    {}\n", stmt_to_rust(expr, &mut ctx)));
        }
        // Ensure a trailing Ok for () returns when the body didn't `ret`.
        let ends_in_return = matches!(f.body.last(), Some(Expr::Return(_)));
        if !ends_in_return && ret.starts_with("Result<(),") {
            lib.push_str("    Ok(())\n");
        }
        lib.push_str("}\n\n");
    }

    files.push(GeneratedFile {
        path: "crates/veil_shared/src/lib.rs".to_string(),
        content: lib,
    });

    files
}

fn gen_traits(
    contents: &ModuleContents,
    crate_name: &str,
    solution: &Solution,
) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Trait definitions (async traits).\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse uuid::Uuid;\n\n");
    out.push_str("use crate::domain::types::*;\n");
    // Common error types and the shared Bus live in veil_shared — re-export
    // them so this crate's `crate::ports::{DomainError, Bus, ...}` still resolve
    // and every crate refers to the SAME type.
    out.push_str("pub use veil_shared::{DomainError, ValidationError};\n");
    out.push_str("pub use veil_shared::*;\n\n");

    for t in &contents.traits {
        let tp = generic_params_rust(&t.type_params);
        // Generic ports get Send+Sync on type params used as entity payloads.
        let where_bounds = if t.type_params.is_empty() {
            String::new()
        } else {
            let clauses: Vec<String> = t
                .type_params
                .iter()
                .map(|p| {
                    let name = p.split(':').next().unwrap_or(p).trim();
                    format!("{name}: Send + Sync + 'static")
                })
                .collect();
            format!("\nwhere\n    {}", clauses.join(",\n    "))
        };
        out.push_str(&format!(
            "/// {}: {}\n#[async_trait]\npub trait {}{}: Send + Sync{where_bounds} {{\n",
            t.subkind, t.name, t.name, tp
        ));
        for method in &t.methods {
            let params = method
                .params
                .iter()
                .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
                .collect::<Vec<_>>()
                .join(", ");
            let ret = match &method.return_type {
                Some(t) => format!(" -> {}", type_to_rust(t)),
                None => String::new(),
            };
            let sep = if params.is_empty() { "" } else { ", " };
            out.push_str(&format!(
                "    async fn {}(&self{sep}{}){ret};\n",
                to_snake(&method.name),
                params
            ));
        }
        out.push_str("}\n\n");
    }

    // Type aliases: `type WearTestRepo = EntityRepo<WearTest>`
    // → marker trait for DI (`Arc<dyn WearTestRepo>`) over monomorphized EntityRepo.
    for item in &solution.items {
        if let TopLevelItem::TypeAlias { name, target } = item {
            match target {
                TypeExpr::Generic(base, args) => {
                    let args_rust: Vec<String> = args.iter().map(type_to_rust).collect();
                    let base_app = format!("{}<{}>", base, args_rust.join(", "));
                    out.push_str(&format!(
                        "/// Type alias: {name} = {base_app}\n\
                         pub trait {name}: {base_app} {{}}\n\
                         impl<__T: {base_app}> {name} for __T {{}}\n\n"
                    ));
                }
                TypeExpr::Named(base) => {
                    out.push_str(&format!(
                        "/// Type alias: {name} = {base}\n\
                         pub trait {name}: {base} {{}}\n\
                         impl<__T: {base}> {name} for __T {{}}\n\n"
                    ));
                }
                _ => {}
            }
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/ports/mod.rs", crate_name),
        content: out,
    }
}

fn gen_impls(
    impls: &[&Construct],
    traits: &[&Construct],
    crate_name: &str,
    solution: &Solution,
    registry: &LayerRegistry,
) -> GeneratedFile {
    use crate::expr::{build_ctx_from_solution, expr_to_rust, GenCtx};

    let mut out = String::new();
    out.push_str("//! Implementations of traits.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables, dead_code)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse crate::ports::*;\nuse crate::domain::types::*;\nuse std::collections::HashMap;\nuse uuid::Uuid;\nuse chrono::Utc;\n");

    // Stub-declared `codegen_imports` when any registered stub provides them.
    // (Adapters that use the stub get these uses; engine does not name crates.)
    let mut seen_imports = std::collections::BTreeSet::new();
    for stub in &registry.stubs {
        for imp in &stub.codegen_imports {
            if seen_imports.insert(imp.clone()) {
                out.push_str(&format!("use {imp};\n"));
            }
        }
    }
    out.push('\n');

    // Name→shape map so the body translator resolves calls correctly.
    let name_to_shape = build_name_to_shape(solution, registry);

    // Collect external-effect hooks (`target.method(...)` where target is not a
    // known construct/local) so we can emit compiling stub fns for them.
    let mut hooks: std::collections::BTreeSet<(String, usize)> = std::collections::BTreeSet::new();
    for c in impls {
        for mimpl in &c.impls {
            let locals: std::collections::HashSet<String> = mimpl.params.iter().cloned().collect();
            for expr in &mimpl.body {
                collect_effect_hooks(expr, &name_to_shape, &locals, &mut hooks);
            }
        }
    }
    if !hooks.is_empty() {
        out.push_str("// External-effect runtime hooks (stubs). Replace with real\n");
        out.push_str("// integrations; generated so adapter bodies compile.\n");
        for (name, arity) in &hooks {
            let params = (0..*arity)
                .map(|i| format!("_arg{}: impl std::fmt::Debug", i))
                .collect::<Vec<_>>().join(", ");
            out.push_str(&format!(
                "fn {}({}) {{ /* stub — replace with real integration */ }}\n",
                name, params
            ));
        }
        out.push('\n');
    }

    if impls.is_empty() {
        out.push_str("// No implementations target traits in this module.\n");
    } else {
        for c in impls {
            // Pure generic templates (`adapter Foo<T> for Trait<T>`) are monomorphization
            // sources only — VEIL bodies live there; concrete adapters get T substituted.
            // Do NOT emit Rust for the template (avoids entity.id on unconstrained T).
            if is_pure_generic_adapter_template(c) {
                continue;
            }
            let target = c.target.as_deref().unwrap_or("?");
            let adapter_tp = generic_params_rust(&c.type_params);
            let target_args_rust: Vec<String> = c
                .target_type_args
                .iter()
                .map(type_to_rust)
                .collect();
            let target_impl = if target_args_rust.is_empty() {
                // Generic adapter: DynamoJsonRepo<T> for EntityRepo<T>
                if !c.type_params.is_empty() {
                    let tp_names: Vec<&str> = c
                        .type_params
                        .iter()
                        .map(|p| p.split(':').next().unwrap_or(p).trim())
                        .collect();
                    format!("{}<{}>", target, tp_names.join(", "))
                } else {
                    target.to_string()
                }
            } else {
                format!("{}<{}>", target, target_args_rust.join(", "))
            };
            // Generic template adapter (same target trait, has type params + VEIL bodies).
            // Used to fill empty monomorphized adapters: DynamoWearTestRepo for EntityRepo<WearTest>
            // copies bodies from DynamoJsonRepo<T> for EntityRepo<T>.
            let generic_template =
                find_generic_adapter_template(c, impls);

            out.push_str(&format!(
                "/// {}: {} (implements {})\npub struct {}{} {{\n",
                c.subkind, c.name, target_impl, c.name, adapter_tp
            ));
            for ann in &c.annotations {
                if ann.name == "env" {
                    for arg in &ann.args {
                        // DATABASE_* env → `pool` field typed from stub `Pool` if present.
                        if arg.contains("DATABASE") {
                            if let Some((crate_name, path)) = stub_type_path(registry, "Pool") {
                                out.push_str(&format!(
                                    "    pub pool: {crate_name}::{path},\n"
                                ));
                            } else {
                                out.push_str("    pub pool: String,\n");
                            }
                        } else {
                            // Use the short suffix (after last _) as the field name,
                            // matching what the body references via self.field.
                            // DDB_TABLE → table, AWS_REGION → region, S3_BUCKET → bucket
                            let full = arg.to_lowercase();
                            let field_name = full.rsplit('_').next().unwrap_or(&full);
                            out.push_str(&format!("    pub {}: String,\n", field_name));
                        }
                    }
                }
                if ann.name == "field" {
                    // @field(name: Type) — generates a typed struct field.
                    // The type is resolved via stub_type_crate for qualified paths.
                    for arg in &ann.args {
                        // Parse "name: Type" or just "name" (defaults to String)
                        if let Some((fname, ftype)) = arg.split_once(':') {
                            let fname = fname.trim();
                            let ftype = ftype.trim();
                            // Check if type is from a stub (aliased)
                            let seeded = build_ctx_from_solution(solution, name_to_shape.clone(), registry);
                            let qualified_type = if let Some((crate_name, original_name)) = seeded.stub_type_crate.get(ftype) {
                                format!("{}::{}", crate_name, original_name)
                            } else {
                                ftype.to_string()
                            };
                            out.push_str(&format!("    pub {}: {},\n", fname, qualified_type));
                        } else {
                            out.push_str(&format!("    pub {}: String,\n", arg.to_lowercase()));
                        }
                    }
                }
            }
            // Auto-detect self.client usage in impl bodies → generate SDK client field.
            // Only when no @field(client: ...) annotation already provides it.
            let has_explicit_client_field = c.annotations.iter().any(|a| {
                a.name == "field"
                    && a.args.iter().any(|arg| {
                        arg.split(':').next().unwrap_or("").trim() == "client"
                    })
            });
            let body_uses_client = c.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            }) || generic_template
                .map(|t| {
                    t.impls
                        .iter()
                        .any(|m| m.body.iter().any(|e| expr_mentions_self_field(e, "client")))
                })
                .unwrap_or(false);
            // Client field: only when @field says so, or bodies (own/template) use self.client.
            // Do NOT hardcode Dynamo/S3 — type comes from @field(client: Client) when present.
            if body_uses_client && !has_explicit_client_field {
                // Fall back only when body uses client without @field (legacy). Prefer stub Client.
                if let Some((crate_name, path)) = stub_type_path(registry, "Client") {
                    out.push_str(&format!("    pub client: {crate_name}::{path},\n"));
                }
            }
            // PhantomData for generic adapters
            if !c.type_params.is_empty() {
                out.push_str("    pub _marker: std::marker::PhantomData<");
                if c.type_params.len() == 1 {
                    let n = c.type_params[0].split(':').next().unwrap_or(&c.type_params[0]).trim();
                    out.push_str(n);
                } else {
                    let names: Vec<&str> = c
                        .type_params
                        .iter()
                        .map(|p| p.split(':').next().unwrap_or(p).trim())
                        .collect();
                    out.push_str(&format!("({})", names.join(", ")));
                }
                out.push_str(">,\n");
            }
            out.push_str("}\n\n");

            // Look up the target trait to recover real method signatures
            // (the impl only carries bare parameter names).
            let target_trait = traits.iter().find(|t| t.name == target).copied();

            let impl_generics = if c.type_params.is_empty() {
                String::new()
            } else {
                // Bound type params for serde document store.
                let parts: Vec<String> = c
                    .type_params
                    .iter()
                    .map(|p| {
                        let n = p.split(':').next().unwrap_or(p).trim();
                        if p.contains(':') {
                            p.clone()
                        } else {
                            format!(
                                "{n}: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static"
                            )
                        }
                    })
                    .collect();
                format!("<{}>", parts.join(", "))
            };

            out.push_str(&format!(
                "#[async_trait]\nimpl{impl_generics} {target_impl} for {}{} {{\n",
                c.name,
                if c.type_params.is_empty() {
                    String::new()
                } else {
                    let names: Vec<&str> = c
                        .type_params
                        .iter()
                        .map(|p| p.split(':').next().unwrap_or(p).trim())
                        .collect();
                    format!("<{}>", names.join(", "))
                }
            ));

            // Effective method list: authored impls, else monomorphized from generic template.
            let effective_impls: Vec<MethodImpl> = {
                let mut by_name: std::collections::BTreeMap<String, MethodImpl> =
                    std::collections::BTreeMap::new();
                if let Some(tmpl) = generic_template {
                    for m in &tmpl.impls {
                        if !m.body.is_empty() {
                            by_name.insert(m.method_name.clone(), m.clone());
                        }
                    }
                }
                for m in &c.impls {
                    if !m.body.is_empty() {
                        by_name.insert(m.method_name.clone(), m.clone());
                    } else if !by_name.contains_key(&m.method_name) {
                        // Keep empty entry so we still emit a method (todo) if no template.
                        by_name.insert(m.method_name.clone(), m.clone());
                    }
                }
                // If monomorphized with no authored methods, still take all template methods.
                if c.impls.is_empty() && generic_template.is_some() {
                    // already filled from template
                }
                by_name.into_values().collect()
            };

            for mimpl in &effective_impls {
                // Find the trait method to get typed params + return type.
                let trait_method = target_trait
                    .and_then(|t| t.methods.iter().find(|m| m.name == mimpl.method_name
                        || to_snake(&m.name) == to_snake(&mimpl.method_name)));

                // Build the signature: prefer the trait's typed params (monomorphized),
                // zipping the impl's bare names by position; fall back to the impl names.
                let (sig_params, ret_rust) = match (trait_method, target_trait) {
                    (Some(m), Some(t)) => {
                        let params = m
                            .params
                            .iter()
                            .map(|p| {
                                let ty = monomorphize_type(&p.type_expr, c, t);
                                format!("{}: {}", to_snake(&p.name), type_to_rust(&ty))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|rt| type_to_rust(&monomorphize_type(rt, c, t)))
                            .unwrap_or_else(|| "Result<(), DomainError>".to_string());
                        (params, ret)
                    }
                    (Some(m), None) => {
                        let params = m
                            .params
                            .iter()
                            .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(type_to_rust)
                            .unwrap_or_else(|| "Result<(), DomainError>".to_string());
                        (params, ret)
                    }
                    _ => {
                        // No trait match — use the impl's bare names, untyped.
                        let params = mimpl
                            .params
                            .iter()
                            .map(|p| format!("{}: ()", to_snake(p)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        (params, "Result<(), DomainError>".to_string())
                    }
                };

                out.push_str(&format!(
                    "    async fn {}(&self{}{}) -> {} {{\n",
                    to_snake(&mimpl.method_name),
                    if sig_params.is_empty() { "" } else { ", " },
                    sig_params,
                    ret_rust,
                ));

                // Translate the body. Adapter bodies call external targets
                // (e.g. `http.post(...)`) that resolve to runtime stubs.
                let mut ctx = GenCtx::new(name_to_shape.clone());
                // The impl's bare params are locals in the body.
                for p in &mimpl.params {
                    ctx.locals.insert(p.clone());
                }
                // @env annotation fields are available as self.field in the body.
                ctx.in_method = true;
                for ann in &c.annotations {
                    if ann.name == "env" {
                        for arg in &ann.args {
                            let full = arg.to_lowercase();
                            ctx.self_fields.insert(full.clone());
                            // Also add the short suffix (after last underscore)
                            // so `DDB_TABLE` makes `table` available as self.table
                            if let Some(short) = full.rsplit('_').next() {
                                if short != full {
                                    ctx.self_fields.insert(short.to_string());
                                }
                            }
                            // DATABASE_URL → make `pool` available as self.pool
                            if arg.contains("DATABASE") {
                                ctx.self_fields.insert("pool".to_string());
                            }
                        }
                    }
                }
                // @field annotation typed fields are also available as self.field
                for ann in &c.annotations {
                    if ann.name == "field" {
                        for arg in &ann.args {
                            let fname = arg.split(':').next().unwrap_or(arg).trim().to_lowercase();
                            ctx.self_fields.insert(fname);
                        }
                    }
                }
                // Seed name→shape and method returns from stubs too.
                let seeded = build_ctx_from_solution(solution, name_to_shape.clone(), registry);
                ctx.method_returns = seeded.method_returns;
                ctx.struct_fields = seeded.struct_fields;
                ctx.stub_type_crate = seeded.stub_type_crate;
                ctx.fallible_methods = seeded.fallible_methods;
                ctx.async_fallible_methods = seeded.async_fallible_methods;
                ctx.expected_return_rust = Some(ret_rust.clone());

                // Cloud SDK types from .stub files: we can *parse* VEIL that
                // calls them, but fluent builder lowering is incomplete.
                // Prefer emitting the lowered body so `link`/`use` packages that
                // depend on the real crate can compile when expressions lower
                // cleanly. When the body is empty, keep the pure-runtime
                // placeholder (local ports). When body refs stubs *and* every
                // line still lowers to a stub hook (result_item), use Err.
                let uses_stub_sdk = mimpl
                    .body
                    .iter()
                    .any(|e| expr_refs_stub_type(e, &ctx.stub_type_crate));

                // Only short-circuit empty bodies that *would* be cloud SDKs with
                // no authored lines. Non-empty bodies always try expr_to_rust —
                // that is the real adapter path (GEN-002 / RT cloud).
                if uses_stub_sdk && mimpl.body.is_empty() {
                    out.push_str(&format!(
                        "        Err(DomainError::External(\
                         \"cloud adapter {}::{} not configured (pure-runtime uses local ports)\"\
                         .into()))\n",
                        c.name, mimpl.method_name
                    ));
                } else if mimpl.body.is_empty() {
                    // Empty adapter — compile-time placeholder; CHK-006 flags debt.
                    out.push_str(&format!(
                        "        todo!(\"empty adapter body: {}::{}\")\n",
                        c.name, mimpl.method_name
                    ));
                } else {
                    for (i, expr) in mimpl.body.iter().enumerate() {
                        let is_last = i == mimpl.body.len() - 1;
                        // Monomorphize type names in expressions (T → WearTest) when
                        // this body was copied from a generic template.
                        let expr = if !c.target_type_args.is_empty() {
                            if let Some(t) = target_trait {
                                monomorphize_expr(expr, c, t)
                            } else {
                                expr.clone()
                            }
                        } else {
                            expr.clone()
                        };
                        let rust_expr = expr_to_rust(&expr, &ctx);
                        // Track local assignments AFTER translation so first use gets 'let mut'
                        if let Expr::Assign(name, _, _) | Expr::MutAssign(name, _, _) = &expr {
                            if !name.contains('.') {
                                ctx.locals.insert(name.clone());
                            }
                        }
                        if is_last {
                            // GEN-002: lower authored adapter bodies. If the last
                            // expr already returns (`ret Ok` → `return Ok(...)`),
                            // emit it as-is — do not wrap again.
                            let is_return = rust_expr.trim_start().starts_with("return ")
                                || rust_expr.contains("return Ok(")
                                || rust_expr.contains("return Err(");
                            if is_return || rust_expr.contains("todo!") {
                                out.push_str(&format!("        {rust_expr}\n"));
                            } else if ret_rust == "Result<(), DomainError>" {
                                out.push_str(&format!("        {rust_expr};\n"));
                                out.push_str("        Ok(())\n");
                            } else if ret_rust.starts_with("Result<") {
                                if rust_expr.starts_with("Ok(") {
                                    out.push_str(&format!("        {rust_expr}\n"));
                                } else if rust_expr.ends_with('?') {
                                    // `?` unwraps the inner Result — value is now T, needs Ok(T)
                                    out.push_str(&format!("        Ok({rust_expr})\n"));
                                } else if rust_expr.contains(".await") {
                                    out.push_str(&format!(
                                        "        Ok({rust_expr}.map_err(|e| DomainError::External(e.to_string()))?)\n"
                                    ));
                                } else {
                                    out.push_str(&format!("        Ok({rust_expr})\n"));
                                }
                            } else {
                                out.push_str(&format!("        {rust_expr}\n"));
                            }
                        } else {
                            out.push_str(&format!("        {rust_expr};\n"));
                        }
                    }
                }
                out.push_str("    }\n\n");
            }

            // A trait impl must cover ALL trait methods. Emit todo for any still missing.
            if let Some(t) = target_trait {
                let implemented: std::collections::HashSet<String> = effective_impls
                    .iter()
                    .map(|m| to_snake(&m.method_name))
                    .collect();
                for m in &t.methods {
                    if implemented.contains(&to_snake(&m.name)) {
                        continue;
                    }
                    let params = m
                        .params
                        .iter()
                        .map(|p| {
                            let ty = monomorphize_type(&p.type_expr, c, t);
                            format!("{}: {}", to_snake(&p.name), type_to_rust(&ty))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret_te = m
                        .return_type
                        .as_ref()
                        .map(|rt| monomorphize_type(rt, c, t));
                    let ret = ret_te
                        .as_ref()
                        .map(type_to_rust)
                        .unwrap_or_else(|| "Result<(), DomainError>".to_string());
                    out.push_str(&format!(
                        "    async fn {}(&self{}{}) -> {} {{\n        {} // TODO: implement\n    }}\n\n",
                        to_snake(&m.name),
                        if params.is_empty() { "" } else { ", " },
                        params,
                        ret,
                        default_ok_for(&ret),
                    ));
                }
            }

            out.push_str("}\n\n");
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/adapters/mod.rs", crate_name),
        content: out,
    }
}

/// Pure generic adapter template: `adapter Foo<T> for Trait<T>` (or unbound
/// `adapter Foo<T> for Trait`). Used only as monomorphization source in VEIL;
/// not emitted as Rust.
fn is_pure_generic_adapter_template(c: &Construct) -> bool {
    if c.type_params.is_empty() {
        return false;
    }
    let tp_names: std::collections::HashSet<&str> = c
        .type_params
        .iter()
        .map(|p| p.split(':').next().unwrap_or(p).trim())
        .collect();
    if c.target_type_args.is_empty() {
        return true;
    }
    // EntityRepo<T> — all type args are type parameters, not concrete types.
    c.target_type_args.iter().all(|a| match a {
        TypeExpr::Named(n) => tp_names.contains(n.as_str()),
        _ => false,
    })
}

/// Find a generic adapter template to monomorphize into `adapter`.
///
/// Matches: same target trait name, pure generic template with at least one
/// non-empty method body. Used for `adapter Foo for EntityRepo<WearTest>`
/// filling from `adapter Bar<T> for EntityRepo<T>`.
fn find_generic_adapter_template<'a>(
    adapter: &Construct,
    all: &[&'a Construct],
) -> Option<&'a Construct> {
    if adapter.target_type_args.is_empty() {
        return None;
    }
    // Only monomorphize into concrete adapters (args are not just type params).
    if is_pure_generic_adapter_template(adapter) {
        return None;
    }
    let target = adapter.target.as_deref()?;
    all.iter().copied().find(|other| {
        other.name != adapter.name
            && other.target.as_deref() == Some(target)
            && is_pure_generic_adapter_template(other)
            && other.impls.iter().any(|m| !m.body.is_empty())
    })
}

/// Replace trait type params with monomorphized args from the adapter.
/// Works for any generic trait/adapter pair — no domain knowledge.
fn monomorphize_type(ty: &TypeExpr, adapter: &Construct, trait_: &Construct) -> TypeExpr {
    match ty {
        TypeExpr::Named(n) => {
            if let Some(idx) = trait_.type_params.iter().position(|p| {
                p.split(':').next().unwrap_or(p).trim() == n
            }) {
                if let Some(arg) = adapter.target_type_args.get(idx) {
                    return arg.clone();
                }
                if let Some(p) = adapter.type_params.get(idx) {
                    let name = p.split(':').next().unwrap_or(p).trim();
                    return TypeExpr::Named(name.to_string());
                }
            }
            // Also map adapter's own type params when monomorphizing template bodies
            // that mention T from the generic adapter (same index as target_type_args).
            if let Some(idx) = adapter.type_params.iter().position(|p| {
                p.split(':').next().unwrap_or(p).trim() == n
            }) {
                if let Some(arg) = adapter.target_type_args.get(idx) {
                    return arg.clone();
                }
            }
            TypeExpr::Named(n.clone())
        }
        TypeExpr::Optional(i) => {
            TypeExpr::Optional(Box::new(monomorphize_type(i, adapter, trait_)))
        }
        TypeExpr::List(i) => TypeExpr::List(Box::new(monomorphize_type(i, adapter, trait_))),
        TypeExpr::Result(Some(i)) => {
            TypeExpr::Result(Some(Box::new(monomorphize_type(i, adapter, trait_))))
        }
        TypeExpr::Generic(name, args) => TypeExpr::Generic(
            name.clone(),
            args.iter()
                .map(|a| monomorphize_type(a, adapter, trait_))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Substitute type-parameter names in expression AST when monomorphizing
/// generic template bodies (type ascriptions / idents mentioning `T`).
fn monomorphize_expr(expr: &Expr, adapter: &Construct, trait_: &Construct) -> Expr {
    let mut renames: std::collections::HashMap<String, TypeExpr> =
        std::collections::HashMap::new();
    for (idx, p) in trait_.type_params.iter().enumerate() {
        let pname = p.split(':').next().unwrap_or(p).trim().to_string();
        if let Some(arg) = adapter.target_type_args.get(idx) {
            renames.insert(pname, arg.clone());
        }
    }
    if renames.is_empty() {
        return expr.clone();
    }
    monomorphize_expr_with(&renames, expr)
}

fn rename_type_expr(
    ty: &TypeExpr,
    renames: &std::collections::HashMap<String, TypeExpr>,
) -> TypeExpr {
    match ty {
        TypeExpr::Named(n) => renames.get(n).cloned().unwrap_or_else(|| ty.clone()),
        TypeExpr::Optional(i) => TypeExpr::Optional(Box::new(rename_type_expr(i, renames))),
        TypeExpr::List(i) => TypeExpr::List(Box::new(rename_type_expr(i, renames))),
        TypeExpr::Result(Some(i)) => {
            TypeExpr::Result(Some(Box::new(rename_type_expr(i, renames))))
        }
        TypeExpr::Generic(name, args) => TypeExpr::Generic(
            name.clone(),
            args.iter().map(|a| rename_type_expr(a, renames)).collect(),
        ),
        other => other.clone(),
    }
}

fn monomorphize_expr_with(
    renames: &std::collections::HashMap<String, TypeExpr>,
    expr: &Expr,
) -> Expr {
    use Expr::*;
    match expr {
        Ident(name) => {
            if let Some(TypeExpr::Named(rep)) = renames.get(name) {
                Ident(rep.clone())
            } else {
                Ident(name.clone())
            }
        }
        Assign(n, e, ty) => Assign(
            n.clone(),
            Box::new(monomorphize_expr_with(renames, e)),
            ty.as_ref().map(|t| rename_type_expr(t, renames)),
        ),
        MutAssign(n, e, ty) => MutAssign(
            n.clone(),
            Box::new(monomorphize_expr_with(renames, e)),
            ty.as_ref().map(|t| rename_type_expr(t, renames)),
        ),
        Call(c) => {
            let mut c = c.clone();
            c.args = c
                .args
                .iter()
                .map(|a| monomorphize_expr_with(renames, a))
                .collect();
            if let Some(recv) = c.receiver.take() {
                c.receiver = Some(Box::new(monomorphize_expr_with(renames, &recv)));
            }
            Call(c)
        }
        BinaryOp(b) => {
            let mut b = b.clone();
            b.left = Box::new(monomorphize_expr_with(renames, &b.left));
            b.right = Box::new(monomorphize_expr_with(renames, &b.right));
            BinaryOp(b)
        }
        UnaryOp(u) => {
            let mut u = u.clone();
            u.expr = Box::new(monomorphize_expr_with(renames, &u.expr));
            UnaryOp(u)
        }
        FieldAccess(e, f) => FieldAccess(Box::new(monomorphize_expr_with(renames, e)), f.clone()),
        Index(e, i) => Index(
            Box::new(monomorphize_expr_with(renames, e)),
            Box::new(monomorphize_expr_with(renames, i)),
        ),
        Return(e) => Return(Box::new(monomorphize_expr_with(renames, e))),
        Match(e, arms) => Match(
            Box::new(monomorphize_expr_with(renames, e)),
            arms.iter()
                .map(|arm| {
                    let mut arm = arm.clone();
                    arm.body = arm
                        .body
                        .iter()
                        .map(|x| monomorphize_expr_with(renames, x))
                        .collect();
                    if let Some(g) = arm.guard.take() {
                        arm.guard = Some(monomorphize_expr_with(renames, &g));
                    }
                    arm
                })
                .collect(),
        ),
        IfExpr(i) => {
            let mut i = i.clone();
            i.condition = Box::new(monomorphize_expr_with(renames, &i.condition));
            i.then_body = i
                .then_body
                .iter()
                .map(|x| monomorphize_expr_with(renames, x))
                .collect();
            if let Some(eb) = i.else_body.take() {
                i.else_body = Some(
                    eb.iter()
                        .map(|x| monomorphize_expr_with(renames, x))
                        .collect(),
                );
            }
            IfExpr(i)
        }
        Action(a) => {
            let mut a = a.clone();
            a.args = a
                .args
                .iter()
                .map(|x| monomorphize_expr_with(renames, x))
                .collect();
            a.named_args = a
                .named_args
                .iter()
                .map(|(k, v)| (k.clone(), monomorphize_expr_with(renames, v)))
                .collect();
            if let Some(c) = a.condition.take() {
                a.condition = Some(Box::new(monomorphize_expr_with(renames, &c)));
            }
            Action(a)
        }
        ForLoop {
            binding,
            index,
            iterable,
            body,
        } => ForLoop {
            binding: binding.clone(),
            index: index.clone(),
            iterable: Box::new(monomorphize_expr_with(renames, iterable)),
            body: body
                .iter()
                .map(|x| monomorphize_expr_with(renames, x))
                .collect(),
        },
        other => other.clone(),
    }
}

/// Find a construct by name anywhere in the solution (top-level or nested).
fn find_construct_by_name<'a>(solution: &'a Solution, name: &str) -> Option<&'a Construct> {
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
fn build_name_to_shape(solution: &Solution, registry: &LayerRegistry) -> std::collections::HashMap<String, Shape> {
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
fn collect_effect_hooks(
    expr: &Expr,
    name_to_shape: &std::collections::HashMap<String, Shape>,
    locals: &std::collections::HashSet<String>,
    hooks: &mut std::collections::BTreeSet<(String, usize)>,
) {
    match expr {
        Expr::Call(call) => {
            if !call.method.is_empty()
                && call.receiver.is_none()
                && !name_to_shape.contains_key(&call.target)
                && !locals.contains(&call.target)
                && !call.target.is_empty()
                && !call.target.contains('.') // dotted paths resolve as Struct::method
            {
                let name = format!("{}_{}", to_snake(&call.target), to_snake(&call.method));
                hooks.insert((name, call.args.len()));
            }
            // Bare function calls: target is the function name, method is empty
            if call.method.is_empty()
                && call.receiver.is_none()
                && !name_to_shape.contains_key(&call.target)
                && !locals.contains(&call.target)
                && !call.target.is_empty()
                && call.target.chars().next().map_or(true, |c| c.is_lowercase())
            {
                let name = to_snake(&call.target);
                hooks.insert((name, call.args.len()));
            }
            if let Some(recv) = &call.receiver {
                collect_effect_hooks(recv, name_to_shape, locals, hooks);
            }
            for a in &call.args {
                collect_effect_hooks(a, name_to_shape, locals, hooks);
            }
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) | Expr::Return(rhs) | Expr::Await(rhs) => {
            collect_effect_hooks(rhs, name_to_shape, locals, hooks);
        }
        Expr::StructLit(_, fields) => {
            for (_, v) in fields {
                collect_effect_hooks(v, name_to_shape, locals, hooks);
            }
        }
        Expr::BinaryOp(op) => {
            collect_effect_hooks(&op.left, name_to_shape, locals, hooks);
            collect_effect_hooks(&op.right, name_to_shape, locals, hooks);
        }
        _ => {}
    }
}

/// True if any subexpression calls a stub-declared type (S3Client, DdbClient, …).
fn expr_refs_stub_type(
    expr: &Expr,
    stubs: &std::collections::HashMap<String, (String, String)>,
) -> bool {
    match expr {
        Expr::Call(call) => {
            let target = if call.target.contains('.') {
                call.target.split('.').last().unwrap_or(&call.target)
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
        Expr::FieldAccess(base, _) | Expr::Await(base) | Expr::Try(base) | Expr::Return(base) => {
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
fn default_ok_for(ret_rust: &str) -> String {
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
        // Unknown concrete type: no guaranteed constructor. `todo!()` type-checks
        // for any return type and marks the stub honestly (panics if reached).
        _ => "todo!(\"stub — not yet implemented\")".to_string(),
    }
}

/// Something that generates an application function — either a core `flow`
/// or an fn-shaped layer construct (service, saga, handler, …).
enum FlowLike<'a> {
    Flow(&'a Flow),
    Construct(&'a Construct),
}

/// Infer a flow's Rust return type as `Result<T, DomainError>`. Pre-scans step
/// bodies to learn local-binding types, then inspects the return expression:
/// a field access / ident resolves to its known type; a literal to its type.
/// Unknown or absent returns become `Result<(), DomainError>`.
fn infer_flow_return_type(
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
                if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = expr {
                    if !name.contains('.') {
                        ctx.locals.insert(name.clone());
                        if envelope_routing {
                            // Envelope-routing locals are JSON message results.
                            ctx.local_types.insert(name.clone(), "serde_json::Value".to_string());
                        } else if let Some(t) = crate::expr::infer_expr_type_pub(rhs, &ctx) {
                            ctx.local_types.insert(name.clone(), t);
                        }
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
fn scan_dep_calls(
    expr: &Expr,
    name_to_shape: &std::collections::HashMap<String, Shape>,
    deps: &mut std::collections::HashSet<String>,
    field_names: &mut std::collections::HashMap<String, String>,
) {
    match expr {
        Expr::Call(call) => {
            if !call.target.is_empty() && call.method.ends_with('!') {
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
        Expr::Return(inner) => scan_dep_calls(inner, name_to_shape, deps, field_names),
        _ => {}
    }
}

fn gen_application(flows: &[FlowLike], module_contents: &ModuleContents, crate_name: &str, solution: &Solution, registry: &LayerRegistry) -> GeneratedFile {
    use crate::expr::{build_ctx_from_solution, collect_deps, stmt_to_rust, expr_to_rust};
    use std::collections::HashMap;

    let mut out = String::new();
    out.push_str("//! Application services and flow functions.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables, dead_code)]\n\n");
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
    // Ensure layer-provided traits (Bus, SagaStep) are ALWAYS in the map.
    // Layer `declare` blocks inject traits/structs into solutions but they
    // don't appear in registry.constructs — scan declarations for trait names.
    for decl in &registry.declarations {
        for line in decl.lines() {
            let t = line.trim();
            if let Some(name) = t.strip_prefix("trait ") {
                let name = name.split_whitespace().next().unwrap_or("");
                if !name.is_empty() {
                    name_to_shape.insert(name.to_string(), Shape::Trait);
                }
            }
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
    let base_ctx = build_ctx_from_solution(solution, effective_name_to_shape.clone(), registry);
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

    // Generate Deps struct using the shared field map
    if !all_deps.is_empty() {
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

    for flow in flows {
        let (name, subkind, annotations, inputs, steps) = match flow {
            FlowLike::Flow(f) => (
                &f.name,
                "Flow",
                &f.annotations,
                &f.inputs,
                &f.steps,
            ),
            FlowLike::Construct(c) => (
                &c.name,
                c.subkind.as_str(),
                &c.annotations,
                &c.inputs,
                &c.steps,
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
        let has_deps = !flow_deps.is_empty() || !dep_inputs.is_empty();
        let deps_param = if has_deps { "deps: &Deps, " } else { "" };

        // Build context for this flow
        let mut ctx = build_ctx_from_solution(solution, effective_name_to_shape.clone(), registry);
        ctx.envelope_routing = envelope_routing;
        if envelope_routing && ctx.routing_ref.is_empty() {
            ctx.routing_ref = ctx.default_routing_ref_as_dep();
        }
        ctx.dep_fields = dep_field_names.clone();
        // Register inputs as locals, with their declared types for inference.
        // Skip dependency-role inputs — accessed via deps.x, not as locals.
        for input in inputs {
            if registry.field_is_dependency(input) {
                // Register the dep field name as Trait so calls route through deps.x
                ctx.name_to_shape.insert(input.name.clone(), Shape::Trait);
                continue;
            }
            ctx.locals.insert(input.name.clone());
            ctx.local_types.insert(input.name.clone(), type_to_rust(&input.type_expr));
        }
        // For DomainService flows: register step-level dep call targets as Trait
        // and copy method_returns so Option<T> unwrapping works.
        for (trait_name, field_name) in &dep_field_names {
            if !ctx.name_to_shape.contains_key(field_name) {
                ctx.name_to_shape.insert(field_name.clone(), Shape::Trait);
            }
            // Copy method_returns from PascalCase trait to the field name
            let mut extra: Vec<((String, String), String)> = Vec::new();
            for ((tn, mn), ret) in &ctx.method_returns {
                if tn == trait_name {
                    extra.push(((field_name.clone(), mn.clone()), ret.clone()));
                    let clean = mn.trim_end_matches('!').to_string();
                    if clean != *mn {
                        extra.push(((field_name.clone(), clean), ret.clone()));
                    }
                }
            }
            for (k, v) in extra {
                ctx.method_returns.entry(k).or_insert(v);
            }
        }

        if let Some(rt) = &runtime {
            // Runtime-delegated construct: emit the step impls + a body that
            // builds the step list and calls the coordinator.
            emit_runtime_delegated(&mut out, name, inputs, steps, rt, deps_param, solution, &ctx);
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

        out.push_str(&format!(
            "#[tracing::instrument(skip_all)]\npub async fn {}(\n    {}{}\n) -> {} {{\n",
            to_snake(name),
            deps_param,
            params,
            ret_type
        ));

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
                    sd.body.iter().any(|e| matches!(e, Expr::Return(_)))
                } else { false }
            });
            if !has_return_in_body {
                out.push_str("    Ok(())\n");
            }
        }
        out.push_str("}\n\n");
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
fn emit_runtime_delegated(
    out: &mut String,
    name: &str,
    inputs: &[Field],
    steps: &[FlowStep],
    rt: &veil_ir::layer::RuntimeBinding,
    deps_param: &str,
    solution: &Solution,
    ctx: &crate::expr::GenCtx,
) {
    let step_trait = &rt.step_trait;
    // Capture the construct's inputs on each step struct so step bodies can use
    // them. Fields are cloned into the struct at construction.
    let input_fields: Vec<(String, String)> = inputs
        .iter()
        .map(|f| (to_snake(&f.name), type_to_rust(&f.type_expr)))
        .collect();

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
    let mut trait_names: std::collections::HashSet<String> = ctx.routing_traits.clone();
    trait_names.insert(step_trait.clone());
    if let Some(tc) = step_trait_construct {
        for m in &tc.methods {
            for p in &m.params {
                if let TypeExpr::Named(n) = &p.type_expr {
                    if n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                        // Candidate trait/type name — only box known traits.
                        if ctx.routing_traits.contains(n) || n == step_trait {
                            trait_names.insert(n.clone());
                        }
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
                if let Expr::Assign(n, _, _) | Expr::MutAssign(n, _, _) = expr {
                    if !n.contains('.') {
                        state_locals.insert(n.clone());
                    }
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
                if let TypeExpr::Named(ty) = &p.type_expr {
                    if ctx.routing_traits.contains(ty) {
                        return Some(to_snake(&p.name));
                    }
                }
                None
            })
        })
        .or_else(|| ctx.primary_routing_trait().map(|t| to_snake(t)))
        .unwrap_or_default();

    let use_envelope = !ctx.routing_traits.is_empty();

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
        out.push_str("}\n\n");

        // Step body ctx: inputs are `self.<field>`; routing trait is the injected
        // param from the step-trait signature; cross-step locals live in threaded state.
        let mut step_ctx = ctx.clone_for_inference();
        step_ctx.envelope_routing = use_envelope;
        step_ctx.routing_ref = routing_param.clone();
        step_ctx.in_method = true; // input idents render as self.<field>
        for (fname, ftype) in &input_fields {
            step_ctx.self_fields.insert(fname.clone());
            step_ctx.local_types.insert(fname.clone(), ftype.clone());
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
        .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!(
        "#[tracing::instrument(skip_all)]\npub async fn {}({}{}) -> Result<(), DomainError> {{\n",
        to_snake(name),
        deps_param,
        params,
    ));
    out.push_str(&format!("    let steps: Vec<Box<dyn {} + Send + Sync>> = vec![\n", step_trait));
    for (i, step) in steps.iter().enumerate() {
        if !matches!(step, FlowStep::Step(_)) { continue; }
        let type_name = format!("{}Step{}", name, i);
        let ctor_args = input_fields
            .iter()
            .map(|(fname, _)| format!("{}: {}.clone()", fname, fname))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("        Box::new({} {{ {} }}),\n", type_name, ctor_args));
    }
    out.push_str("    ];\n");
    // Call the coordinator with the primary routing-trait dep and the step list.
    let routing_dep = ctx
        .primary_routing_trait()
        .map(|t| format!("deps.{}.as_ref()", to_snake(t)))
        .unwrap_or_else(|| "/* no routing trait */".to_string());
    out.push_str(&format!(
        "    {}({}, &steps).await\n",
        to_snake(&rt.coordinator),
        routing_dep
    ));
    out.push_str("}\n\n");
}

/// Emit one step-trait method impl with a translated body.
/// Parameter list and types are taken from the layer-declared step trait method
/// (not hardcoded). Value-typed params (e.g. `Json`) are `mut` so step bodies
/// can reassign threaded state; trait params are shared references.
fn emit_step_method(
    out: &mut String,
    method: &str,
    body: &[Expr],
    returns_state: bool,
    step_method: Option<&veil_ir::ast::Method>,
    trait_names: &std::collections::HashSet<String>,
    ctx: &crate::expr::GenCtx,
) {
    use crate::expr::expr_to_rust;

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
    for expr in body {
        out.push_str(&format!("        {};\n", expr_to_rust(expr, ctx)));
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
#[allow(dead_code)] // retained for planned cross-module import generation
fn detect_sibling_refs(module: &Construct, solution: &Solution) -> Vec<String> {
    let mut needed = std::collections::HashSet::new();
    let module_names: std::collections::HashMap<String, String> = solution.items.iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some((c.name.clone(), to_snake(&c.name))),
            _ => None,
        }).collect();

    fn scan_refs(c: &Construct, module_names: &std::collections::HashMap<String, String>, needed: &mut std::collections::HashSet<String>) {
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

pub fn to_snake(name: &str) -> String {
    // If the entire name is uppercase (like IAAA, HTTP, API), just lowercase it
    if name.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) {
        return name.to_lowercase();
    }

    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap());
    }
    result
}

pub fn type_to_rust(ty: &TypeExpr) -> String {
    type_to_rust_impl(ty, &std::collections::HashSet::new())
}

/// REST body field extraction for dual-loop harness handlers.
///
/// Accepts HTML date inputs (`YYYY-MM-DD` → RFC3339 midnight UTC) and form empties
/// (`""` → null) so browser `<input type="date">` and optional fields do not 400.
fn harness_body_field_extract(field: &str, rust_type: &str) -> String {
    match rust_type {
        "Uuid" => format!(
            "    let {field} = body.get(\"{field}\").and_then(|v| v.as_str()).and_then(|s| s.parse::<Uuid>().ok()).unwrap_or_else(Uuid::new_v4);\n"
        ),
        "String" => format!(
            "    let {field} = body.get(\"{field}\").and_then(|v| v.as_str()).unwrap_or_default().to_string();\n"
        ),
        "DateTime<Utc>" => format!(
            "    let {field} = serde_json::from_value(veil_normalize_body_dt(body.get(\"{field}\").cloned().unwrap_or(Value::Null))).map_err(|_| StatusCode::BAD_REQUEST)?;\n"
        ),
        t if t.starts_with("Option<") && t.contains("DateTime") => format!(
            "    let {field} = serde_json::from_value(veil_normalize_body_dt(body.get(\"{field}\").cloned().unwrap_or(Value::Null))).map_err(|_| StatusCode::BAD_REQUEST)?;\n"
        ),
        t if t.starts_with("Option<") => format!(
            "    let {field} = {{\n        let __v = body.get(\"{field}\").cloned().unwrap_or(Value::Null);\n        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {{ Value::Null }} else {{ __v }};\n        serde_json::from_value(__v).map_err(|_| StatusCode::BAD_REQUEST)?\n    }};\n"
        ),
        _ => format!(
            "    let {field} = serde_json::from_value(body.get(\"{field}\").cloned().unwrap_or(Value::Null)).map_err(|_| StatusCode::BAD_REQUEST)?;\n"
        ),
    }
}

/// Helper emitted into dual-loop `veil_bin` main.rs (no chrono dep required).
fn harness_body_dt_helper() -> &'static str {
    r#"
/// HTML `<input type="date">` and form empties → JSON values chrono/serde accept.
/// `""` → null; bare `YYYY-MM-DD` → `YYYY-MM-DDT00:00:00Z`.
fn veil_normalize_body_dt(v: Value) -> Value {
    match v {
        Value::String(s) if s.is_empty() => Value::Null,
        Value::String(s)
            if s.len() == 10
                && s.as_bytes().get(4) == Some(&b'-')
                && s.as_bytes().get(7) == Some(&b'-')
                && !s.contains('T') =>
        {
            Value::String(format!("{s}T00:00:00Z"))
        }
        other => other,
    }
}
"#
}

/// Format generic type parameters: `<T, U>` or empty string if none.
fn generic_params_rust(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Dyn trait type for harness wiring: prefer type-alias marker (WearTestRepo)
/// when the adapter monomorphizes EntityRepo&lt;WearTest&gt;.
fn adapter_dyn_type(solution: &Solution, ad: &Construct) -> String {
    let target = ad.target.as_deref().unwrap_or("?");
    // Match type alias `type WearTestRepo = EntityRepo<WearTest>`
    for item in &solution.items {
        if let TopLevelItem::TypeAlias { name, target: te } = item {
            if let TypeExpr::Generic(base, args) = te {
                if base == target
                    && args.len() == ad.target_type_args.len()
                    && args
                        .iter()
                        .zip(ad.target_type_args.iter())
                        .all(|(a, b)| type_to_rust(a) == type_to_rust(b))
                {
                    return name.clone();
                }
            }
        }
    }
    if !ad.target_type_args.is_empty() {
        let args: Vec<String> = ad.target_type_args.iter().map(type_to_rust).collect();
        return format!("{}<{}>", target, args.join(", "));
    }
    target.to_string()
}

/// Deps field name for an adapter given the shared trait→field map.
/// Preference: map entry for target trait → type-alias snake → snake(trait).
fn adapter_deps_field_name(
    solution: &Solution,
    ad: &Construct,
    target: &str,
    dep_fields: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(f) = dep_fields.get(target) {
        return f.clone();
    }
    for item in &solution.items {
        if let TopLevelItem::TypeAlias { name, target: te } = item {
            if let TypeExpr::Generic(base, args) = te {
                if base == target
                    && args.len() == ad.target_type_args.len()
                    && args
                        .iter()
                        .zip(ad.target_type_args.iter())
                        .all(|(a, b)| type_to_rust(a) == type_to_rust(b))
                {
                    return to_snake(name);
                }
            }
            if let TypeExpr::Named(base) = te {
                if base == target {
                    return to_snake(name);
                }
            }
        }
    }
    to_snake(target)
}

/// Collect trait → Deps field names for application fns in a module.
/// Policy: first dependency-role input name for a trait wins; body-scanned
/// traits fall back to `to_snake(Trait)`. Used by application codegen and harness.
fn collect_deps_field_map(
    fns: &[&Construct],
    registry: &LayerRegistry,
    name_to_shape: &std::collections::HashMap<String, Shape>,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashMap<String, String>,
) {
    let mut all_deps = std::collections::HashSet::new();
    let mut dep_field_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // Pseudo-ctx for collect_deps (only needs name_to_shape for trait detection).
    let base_ctx = crate::expr::GenCtx::new(name_to_shape.clone());

    for f in fns {
        all_deps.extend(crate::expr::collect_deps(&f.steps, &base_ctx));
        for field in &f.inputs {
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
        for step in &f.steps {
            if let FlowStep::Step(s) = step {
                for expr in &s.body {
                    scan_dep_calls(
                        expr,
                        name_to_shape,
                        &mut all_deps,
                        &mut dep_field_names,
                    );
                }
            }
        }
    }
    // Ensure every dep has a field name.
    for t in &all_deps {
        dep_field_names
            .entry(t.clone())
            .or_insert_with(|| to_snake(t));
    }
    (all_deps, dep_field_names)
}

/// Trait-aware type rendering: a value-position reference to a known trait
/// becomes a boxed trait object `Box<dyn Trait + Send + Sync>`. Used when
/// generating coordinator signatures (`List<SagaStep>` → `Vec<Box<dyn ..>>`).
pub fn type_to_rust_with_traits(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
    type_to_rust_impl(ty, traits)
}

/// Render a function parameter type. A bare trait-typed parameter is passed by
/// shared reference (`&(dyn Trait + Send + Sync)`); a `List<Trait>` is passed as
/// a borrowed slice (`&[Box<dyn Trait + Send + Sync>]`) since boxed trait
/// objects aren't Clone and shouldn't be moved into a coordinator; other types
/// use the standard rendering.
fn param_type_to_rust(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
    if let TypeExpr::Named(name) = ty {
        if traits.contains(name) {
            return format!("&(dyn {} + Send + Sync)", name);
        }
    }
    if let TypeExpr::List(inner) = ty {
        if let TypeExpr::Named(name) = inner.as_ref() {
            if traits.contains(name) {
                return format!("&[Box<dyn {} + Send + Sync>]", name);
            }
        }
    }
    type_to_rust_impl(ty, traits)
}

/// The type name tracked for a parameter local, for method resolution. A bare
/// trait param tracks the unboxed trait name (so `x.method()` resolves to an
/// async trait call); other types track their Rust rendering.
fn local_type_for_param(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
    if let TypeExpr::Named(name) = ty {
        if traits.contains(name) {
            return name.clone();
        }
    }
    type_to_rust_impl(ty, traits)
}

fn type_to_rust_impl(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
    let rec = |t: &TypeExpr| type_to_rust_impl(t, traits);
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Str" => "String".to_string(),
            "Int" => "i64".to_string(),
            "F64" => "f64".to_string(),
            "Bool" => "bool".to_string(),
            "Bytes" => "Vec<u8>".to_string(),
            "UUID" | "Id" => "Uuid".to_string(),
            "DateTime" | "Dt" => "DateTime<Utc>".to_string(),
            "Json" => "serde_json::Value".to_string(),
            other if traits.contains(other) => {
                format!("Box<dyn {} + Send + Sync>", other)
            }
            other => other.to_string(),
        },
        TypeExpr::Generic(name, args) => {
            let rust_args = args.iter().map(rec).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, rust_args)
        }
        TypeExpr::Result(Some(inner)) => format!("Result<{}, DomainError>", rec(inner)),
        TypeExpr::Result(None) => "Result<(), DomainError>".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", rec(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", rec(inner)),
        TypeExpr::Map(k, v) => format!(
            "std::collections::HashMap<{}, {}>",
            rec(k),
            rec(v)
        ),
        TypeExpr::Set(inner) => format!("std::collections::HashSet<{}>", rec(inner)),
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(rec).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}; {}]", rec(inner), size),
        TypeExpr::Ref(inner, is_mut) => if *is_mut { format!("&mut {}", rec(inner)) } else { format!("&{}", rec(inner)) },
        TypeExpr::Dyn(inner) => format!("dyn {}", rec(inner)),
        TypeExpr::ImplTrait(inner) => format!("impl {}", rec(inner)),
        TypeExpr::FnPtr(params, ret) => {
            let p = params.iter().map(|t| rec(t)).collect::<Vec<_>>().join(", ");
            let r = ret.as_ref().map(|t| format!(" -> {}", rec(t))).unwrap_or_default();
            format!("fn({}){}", p, r)
        }
    }
}

/// Infer a Rust type for shorthand fields (untyped, name-only).
/// Purely conventional inference on the field NAME — not domain knowledge.
fn infer_field_type(name: &str) -> String {
    // UUID conventions
    if name == "id" || name.ends_with("_id") {
        return "Uuid".to_string();
    }
    // DateTime conventions
    if name.ends_with("_at") || name == "created" || name == "updated"
        || name == "deleted" || name == "expires" || name == "timestamp" {
        return "DateTime<Utc>".to_string();
    }
    // Boolean conventions
    if name.starts_with("is_") || name.starts_with("has_") || name.starts_with("can_")
        || name == "active" || name == "enabled" || name == "verified" || name == "deleted" {
        return "bool".to_string();
    }
    // Numeric conventions
    if name == "count" || name == "total" || name == "amount" || name == "quantity"
        || name == "score" || name == "age" || name == "size" || name == "length"
        || name == "port" || name == "retries" {
        return "i64".to_string();
    }
    // Email/URL are strings
    if name == "email" || name == "url" || name == "name" || name == "title"
        || name == "description" || name == "message" || name == "reason"
        || name == "path" || name == "key" || name == "token" || name == "code" {
        return "String".to_string();
    }
    "String".to_string()
}

// ─── Multi-package harness (local dev) ─────────────────────────────────────

/// Generate a combined `veil_bin` that wires multiple packages into one HTTP server.
/// Each package's contexts get their own adapters + deps, and all routes merge.
pub fn generate_multi_package_harness(
    packages: &[(&Solution, &LayerRegistry)],
) -> Vec<GeneratedFile> {
    // (module, crate_name, registry, solution) — solution needed for type aliases / dyn types
    let mut all_modules: Vec<(&Construct, &str, &LayerRegistry, &Solution)> = Vec::new();
    let mut all_crate_names: Vec<String> = Vec::new();

    for (sol, reg) in packages {
        for item in &sol.items {
            if let TopLevelItem::Construct(c) = item {
                if c.shape == Shape::Mod {
                    let cn = to_snake(&c.name);
                    all_modules.push((c, Box::leak(cn.clone().into_boxed_str()), reg, sol));
                    if !all_crate_names.contains(&cn) {
                        all_crate_names.push(cn);
                    }
                }
            }
        }
    }

    let mut main_rs = String::new();
    main_rs.push_str("//! Multi-package HTTP harness (local dev).\n");
    main_rs.push_str("//! Wires adapters from multiple VEIL packages into one server.\n");
    main_rs.push_str("//! Auto-generated by devloop multi-package gen.\n\n");
    main_rs.push_str("use std::sync::Arc;\n");
    main_rs.push_str("use axum::{Router, Json, extract::State, extract::Query, routing::{get, post, put, delete}, http::StatusCode};\n");
    main_rs.push_str("use tower_http::cors::CorsLayer;\n");
    main_rs.push_str("use uuid::Uuid;\n");
    main_rs.push_str("use serde_json::Value;\n");
    main_rs.push_str("use veil_shared::*;\n\n");

    for cn in &all_crate_names {
        main_rs.push_str(&format!(
            "use {cn}::application::{{self as {cn}_app, Deps as {cn}_Deps}};\n"
        ));
        main_rs.push_str(&format!("use {cn}::adapters::*;\n"));
        main_rs.push_str(&format!("use {cn}::ports::*;\n"));
    }

    main_rs.push_str("\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
    main_rs.push_str("    let port: u16 = std::env::var(\"PORT\").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);\n\n");

    // For each module: wire adapters + deps (same logic as gen_local_harness_main)
    let mut router_names: Vec<String> = Vec::new();
    for (module, crate_name, registry, sol) in &all_modules {
        let flat = flatten_module(module);
        let adapters = &flat.impls;
        let services = &flat.fns;
        if adapters.is_empty() && services.is_empty() {
            continue;
        }

        main_rs.push_str(&format!("    // ── context {} ──\n", module.name));

        // Shared stub harness_field constructors for this context (same as single-package).
        let mut emitted_harness_lets: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ad in adapters {
            for ann in &ad.annotations {
                if ann.name != "field" {
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
                        main_rs.push_str(&format!(
                            "    // stub harness_field {ftype}\n\
                             let {let_name} = {expr};\n\n"
                        ));
                        emitted_harness_lets.insert(ftype);
                    }
                }
            }
        }

        // Ports actually required by application Deps (`@dep` inputs).
        // Do not wire unused adapters (e.g. TenantRepo) into Deps — mismatch fails compile.
        let mut needed_ports: std::collections::HashSet<String> = std::collections::HashSet::new();
        for svc in services {
            for field in &svc.inputs {
                if registry.field_is_dependency(field) {
                    if let TypeExpr::Named(type_name) = &field.type_expr {
                        needed_ports.insert(type_name.clone());
                    }
                }
            }
        }
        // Fallback: if nothing discovered, keep previous "all adapters" behavior
        let filter_ports = !needed_ports.is_empty();
        let name_to_shape_mp = build_name_to_shape(sol, registry);
        let (_deps_set_mp, dep_fields_mp) =
            collect_deps_field_map(&services, registry, &name_to_shape_mp);

        // Emit adapter instantiations (only for needed ports when known)
        for ad in adapters {
            // Skip pure generic templates (e.g. DynamoJsonRepo<T> for EntityRepo<T>).
            if is_pure_generic_adapter_template(ad) {
                continue;
            }
            let target = ad.target.as_deref().unwrap_or("Send");
            if filter_ports && !needed_ports.contains(target) {
                // Allow type-alias deps (WearTestRepo) via monomorphized adapters
                let field = adapter_deps_field_name(sol, ad, target, &dep_fields_mp);
                let alias_ok = needed_ports.iter().any(|p| to_snake(p) == field || p == &field);
                if !alias_ok && !needed_ports.iter().any(|p| sol.items.iter().any(|i| {
                    matches!(i, TopLevelItem::TypeAlias { name, .. } if name == p)
                })) {
                    continue;
                }
            }
            let env_ann = ad.annotations.iter().find(|a| a.name == "env");
            let mut fields_init = String::new();

            // @field annotations
            for ann in &ad.annotations {
                if ann.name == "field" {
                    for arg in &ann.args {
                        let (fname, ftype) = if let Some((n, t)) = arg.split_once(':') {
                            (n.trim(), t.trim())
                        } else {
                            (arg.trim(), "String")
                        };
                        if let Some((let_name, _)) = stub_harness_field_expr(registry, ftype) {
                            fields_init.push_str(&format!("        {fname}: {let_name}.clone(),\n"));
                        } else {
                            fields_init.push_str(&format!("        {fname}: Default::default(),\n"));
                        }
                    }
                }
            }

            // @env annotations
            if let Some(env_a) = env_ann {
                for arg in &env_a.args {
                    let full = arg.to_lowercase();
                    let field_name = full.rsplit('_').next().unwrap_or(&full);
                    if arg.contains("DATABASE") {
                        // Prefer stub harness_field Pool (declared on the DB driver stub).
                        if let Some((_, expr)) = stub_harness_field_expr(registry, "Pool") {
                            fields_init.push_str(&format!("        pool: {expr},\n"));
                        } else {
                            fields_init.push_str(&format!(
                                "        pool: std::env::var(\"{arg}\").unwrap_or_else(|_| \"default\".into()),\n"
                            ));
                        }
                    } else {
                        fields_init.push_str(&format!(
                            "        {field_name}: std::env::var(\"{arg}\").unwrap_or_else(|_| \"default\".into()),\n"
                        ));
                    }
                }
            }

            // Client only via @field harness_field — never hardcode SDK paths here.
            let has_explicit_client = ad.annotations.iter().any(|a| {
                a.name == "field" && a.args.iter().any(|arg| arg.split(':').next().unwrap_or("").trim() == "client")
            });
            let body_uses_client = ad.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            });
            if (body_uses_client || has_explicit_client) && !fields_init.contains("client:") {
                if let Some((let_name, _)) = stub_harness_field_expr(registry, "Client") {
                    fields_init.push_str(&format!("        client: {let_name}.clone(),\n"));
                }
            }

            let dyn_ty = adapter_dyn_type(sol, ad);
            if fields_init.is_empty() {
                main_rs.push_str(&format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({name} {{}});\n",
                    sn = to_snake(&ad.name), name = ad.name,
                ));
            } else {
                main_rs.push_str(&format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({name} {{\n{fields_init}    }});\n",
                    sn = to_snake(&ad.name), name = ad.name,
                ));
            }
        }

        if services.is_empty() {
            main_rs.push('\n');
            continue;
        }

        // Build Deps struct — field names from shared map (match application crate).
        main_rs.push_str(&format!("    let {crate_name}_deps = Arc::new({crate_name}_Deps {{\n"));
        let mut wired_fields: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ad in adapters {
            if is_pure_generic_adapter_template(ad) {
                continue;
            }
            if let Some(target) = &ad.target {
                let field = adapter_deps_field_name(sol, ad, target, &dep_fields_mp);
                if filter_ports
                    && !needed_ports.contains(target)
                    && !needed_ports.iter().any(|p| p == &field || to_snake(p) == field)
                {
                    // Also allow type-alias dep names (WearTestRepo)
                    let alias_match = sol.items.iter().any(|i| match i {
                        TopLevelItem::TypeAlias { name, .. } => {
                            to_snake(name) == field && needed_ports.contains(name)
                        }
                        _ => false,
                    });
                    if !alias_match {
                        continue;
                    }
                }
                if !wired_fields.insert(field.clone()) {
                    continue;
                }
                main_rs.push_str(&format!(
                    "        {field}: {sn}_inst.clone(),\n",
                    sn = to_snake(&ad.name),
                ));
            }
        }
        main_rs.push_str("    });\n\n");

        // Build routes for this context
        let router_name = format!("{crate_name}_routes");
        main_rs.push_str(&format!("    let {router_name} = Router::new()\n"));
        for svc in services {
            let fn_name = to_snake(&svc.name);
            let (method, path) = rest_route_for_service(svc);
            // Path strings come from `@route` / name-derived policy as authored
            // (brace params `{id}`). No target-framework rewrite in the engine.
            main_rs.push_str(&format!("        .route(\"{path}\", {method}({fn_name}_handler))\n"));
        }
        main_rs.push_str(&format!("        .with_state({crate_name}_deps);\n\n"));
        router_names.push(router_name);
    }

    // Merge all routers
    main_rs.push_str("    let app = Router::new()\n");
    for rn in &router_names {
        main_rs.push_str(&format!("        .merge({rn})\n"));
    }
    main_rs.push_str("        .route(\"/health\", get(|| async { \"ok\" }))\n");
    main_rs.push_str("        .layer(CorsLayer::permissive());\n\n");

    main_rs.push_str("    println!(\"veil_bin: listening on :{}\", port);\n");
    main_rs.push_str("    let listener = tokio::net::TcpListener::bind(format!(\"0.0.0.0:{}\", port)).await?;\n");
    main_rs.push_str("    axum::serve(listener, app).await?;\n");
    main_rs.push_str("    Ok(())\n}\n\n");

    // Generate handler functions for each service across all modules
    // (same path/query/body policy as single-package local harness).
    for (module, crate_name, _, _) in &all_modules {
        let flat = flatten_module(module);
        for svc in &flat.fns {
            let fn_name = to_snake(&svc.name);
            let (method, path) = rest_route_for_service(svc);
            // Same binding policy as single-package local harness: path segments
            // use brace form `{id}` in `@route` (and name-derived paths). Engine
            // does not rewrite foreign path dialects.
            let path_param_count = path.matches('{').count();
            let needs_path_id = path_param_count > 0;
            let needs_body = method == "post" || method == "put";
            // GET without path param → query string (List* / tenant-scoped lists)
            let is_list_get = method == "get" && !needs_path_id;

            if needs_path_id && needs_body {
                main_rs.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    axum::extract::Path(id): axum::extract::Path<String>,\n    Json(body): Json<Value>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else if needs_path_id {
                if path_param_count == 1 {
                    main_rs.push_str(&format!(
                        "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    axum::extract::Path(id): axum::extract::Path<String>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                    ));
                } else {
                    // Multiple path params — map of segment names
                    main_rs.push_str(&format!(
                        "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    axum::extract::Path(path_params): axum::extract::Path<std::collections::HashMap<String, String>>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                    ));
                }
            } else if needs_body {
                main_rs.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    Json(body): Json<Value>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else if is_list_get {
                main_rs.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n    Query(q): Query<std::collections::HashMap<String, String>>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            } else {
                main_rs.push_str(&format!(
                    "async fn {fn_name}_handler(\n    State(deps): State<Arc<{crate_name}_Deps>>,\n) -> Result<Json<Value>, StatusCode> {{\n"
                ));
            }

            let svc_has_deps = svc.inputs.iter().any(|i| {
                i.annotations.iter().any(|a| a.name == "dep")
            }) || svc.steps.iter().any(|st| {
                if let FlowStep::Step(s) = st {
                    s.body.iter().any(|e| expr_mentions_port_call(e))
                } else {
                    false
                }
            });
            let mut args: Vec<String> = if svc_has_deps {
                vec!["&deps".to_string()]
            } else {
                Vec::new()
            };

            // Path param parse when signature has Path(id)
            if needs_path_id && path_param_count == 1 {
                // Prefer first non-dep input that is Id for path
                if let Some(input) = svc.inputs.iter().find(|i| {
                    !i.annotations.iter().any(|a| a.name == "dep")
                        && type_to_rust(&i.type_expr) == "Uuid"
                }) {
                    let field = to_snake(&input.name);
                    main_rs.push_str(&format!(
                        "    let {field} = id.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;\n"
                    ));
                    args.push(field);
                }
            }

            for input in &svc.inputs {
                if input.annotations.iter().any(|a| a.name == "dep") {
                    continue;
                }
                let field = to_snake(&input.name);
                // Skip if already bound from path
                if args.iter().any(|a| a == &field) {
                    continue;
                }
                let rust_type = type_to_rust(&input.type_expr);
                if is_list_get {
                    // Query string
                    if rust_type == "Uuid" {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok()).ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else if rust_type == "String" {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").cloned().unwrap_or_default();\n"
                        ));
                    } else {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| serde_json::from_str(s).ok()).ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    }
                } else if needs_body {
                    main_rs.push_str(&harness_body_field_extract(&field, &rust_type));
                } else if needs_path_id && path.matches('{').count() > 1 {
                    // multi path params map
                    if rust_type == "Uuid" {
                        main_rs.push_str(&format!(
                            "    let {field} = path_params.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok()).ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else {
                        main_rs.push_str(&format!(
                            "    let {field} = path_params.get(\"{field}\").cloned().unwrap_or_default();\n"
                        ));
                    }
                } else {
                    // no inputs left
                    continue;
                }
                args.push(field);
            }

            main_rs.push_str(&format!(
                "    match {crate_name}_app::{}({}).await {{\n",
                fn_name,
                args.join(", ")
            ));
            if method == "delete" {
                main_rs.push_str(
                    "        Ok(_) => Ok(Json(serde_json::json!({\"ok\": true}))),\n",
                );
            } else {
                main_rs.push_str(
                    "        Ok(result) => Ok(Json(serde_json::to_value(result).unwrap_or_default())),\n",
                );
            }
            main_rs.push_str(
                "        Err(e) => { eprintln!(\"error: {e}\"); Err(StatusCode::INTERNAL_SERVER_ERROR) }\n",
            );
            main_rs.push_str("    }\n}\n\n");
        }
    }

    main_rs.push_str(harness_body_dt_helper());

    // Build Cargo.toml for veil_bin
    let mut cargo_toml = String::new();
    cargo_toml.push_str("[package]\nname = \"veil_bin\"\nversion.workspace = true\nedition.workspace = true\n\n");
    cargo_toml.push_str("[[bin]]\nname = \"veil_bin\"\npath = \"src/main.rs\"\n\n");
    cargo_toml.push_str("[dependencies]\ntokio = { workspace = true }\nuuid = { workspace = true }\nserde_json = { workspace = true }\n");
    cargo_toml.push_str("veil_shared = { path = \"../veil_shared\" }\n");
    cargo_toml.push_str("axum = \"0.8\"\ntower-http = { version = \"0.6\", features = [\"cors\"] }\n");

    // Stub crates from the packages being harnessed — Cargo keys use published names (hyphens).
    let mut seen_stub = std::collections::BTreeSet::new();
    for (_, reg) in packages {
        for stub in &reg.stubs {
            if !seen_stub.insert(stub.name.clone()) {
                continue;
            }
            if !stub_is_active_cargo(stub) {
                continue;
            }
            // `name.workspace = true` is invalid; use `name = { workspace = true }`.
            let key = &stub.name;
            if !cargo_toml.contains(key) {
                cargo_toml.push_str(&format!("{key} = {{ workspace = true }}\n"));
            }
            for (dep_name, _) in &stub.cargo_deps {
                if !cargo_toml.contains(dep_name) {
                    cargo_toml.push_str(&format!("{dep_name} = {{ workspace = true }}\n"));
                }
            }
        }
    }

    // Add all context crates as deps
    for cn in &all_crate_names {
        cargo_toml.push_str(&format!("{cn} = {{ path = \"../{cn}\" }}\n"));
    }

    vec![
        GeneratedFile { path: "crates/veil_bin/Cargo.toml".to_string(), content: cargo_toml },
        GeneratedFile { path: "crates/veil_bin/src/main.rs".to_string(), content: main_rs },
    ]
}
