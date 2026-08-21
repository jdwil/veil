//! Harness template rendering — produces handler + router code from HarnessIR
//! using layer-provided templates. The engine computes semantic data (bind sources,
//! type parsing, deps wiring) and the layer template arranges it into framework syntax.
//!
//! This module replaces direct axum string emission with a template-driven approach:
//! - Engine → HarnessTemplateData (semantic: field names, types, sources)
//! - Layer template → framework syntax (axum extractors, Router, etc.)

use std::collections::{BTreeMap, BTreeSet, HashSet};

use veil_ir::ast::*;
use veil_ir::layer::LayerRegistry;
use veil_ir::{BindSource, HarnessIR, WireKind};

use super::*;

/// Pre-computed data for a single endpoint handler, ready for template interpolation.
#[derive(Debug, Clone)]
pub struct EndpointTemplateData {
    /// Handler function name (e.g. `my_crate_list_items_handler`)
    pub fn_name: String,
    /// Application function name (e.g. `list_items`)
    pub app_fn_name: String,
    /// Crate name prefix (e.g. `my_crate`)
    pub crate_name: String,
    /// HTTP method lowercase (e.g. `get`)
    pub method: String,
    /// Route path (e.g. `/api/items/{id}`)
    pub path: String,
    /// Whether this handler needs State<Deps>
    pub has_deps: bool,
    /// Deps type name qualified (e.g. `my_crate_Deps`)
    pub deps_type: String,
    /// Path parameter names extracted from route
    pub path_params: Vec<String>,
    /// Whether handler needs Query extractor
    pub needs_query: bool,
    /// Whether handler needs Json body extractor
    pub needs_body: bool,
    /// Pre-computed field extraction lines (each is a `let field = ...;` line)
    pub field_extractions: Vec<String>,
    /// Arguments to pass to the application function
    pub call_args: Vec<String>,
    /// Whether to use delete-style response (json!({ok:true})) vs public serialize
    pub is_delete: bool,
}

/// Pre-computed data for a context's router registration.
#[derive(Debug, Clone)]
pub struct RouterTemplateData {
    /// Crate name (e.g. `my_crate`)
    pub crate_name: String,
    /// Module display name
    pub module_name: String,
    /// Whether this context has deps
    pub has_deps: bool,
    /// Route lines: (path, chained_method_handlers) e.g. ("/api/items", "get(my_crate_list_items_handler).post(my_crate_create_item_handler)")
    pub routes: Vec<(String, String)>,
}

/// Pre-computed data for deps wiring in main().
#[derive(Debug, Clone)]
pub struct DepsWiringData {
    /// Crate name
    pub crate_name: String,
    /// Module name
    pub module_name: String,
    /// Stub harness_field let bindings (e.g. `let pool = ...;`)
    pub stub_lets: Vec<String>,
    /// Adapter instantiation lines
    pub adapter_insts: Vec<String>,
    /// Deps struct construction (field: value pairs)
    pub deps_fields: Vec<(String, String)>,
    /// Bus-related fields
    pub provided_fields: Vec<String>,
    /// Whether this context has deps
    pub has_deps: bool,
    /// Deps type alias name (e.g. `my_crate_Deps`)
    pub deps_type_alias: String,
    /// Router data for this context (None if no services)
    pub router: Option<RouterTemplateData>,
}

/// Complete pre-computed template data for the entire harness.
#[derive(Debug, Clone)]
pub struct HarnessTemplateData {
    /// Package name
    pub package_name: String,
    /// Port env var name and default
    pub port_env: String,
    pub port_default: u16,
    /// Endpoint handlers (all contexts)
    pub endpoints: Vec<EndpointTemplateData>,
    /// Deps wiring per context (includes router data)
    pub deps_wiring: Vec<DepsWiringData>,
    /// Whether any context uses the bus
    pub has_bus: bool,
    /// Bus type name if applicable (e.g. `InProcessBus`)
    pub bus_type: Option<String>,
    /// Router variable names for merge
    pub router_names: Vec<String>,
    /// Profile string
    pub profile: String,
    /// Total endpoint count
    pub endpoint_count: usize,
    /// Use statements needed
    pub use_statements: Vec<String>,
    /// Routing method imports needed (get, post, etc.)
    pub routing_imports: Vec<String>,
    /// Whether any handler uses Query
    pub any_query: bool,
    /// Bus handler registrations
    pub bus_registrations: Vec<String>,
    /// Layer shared_emit helpers (substituted)
    pub layer_helpers: String,
    /// Health path
    pub health_path: String,
}

/// Compute the full harness template data from the solution + HarnessIR.
/// This is the semantic analysis step — no framework syntax here.
pub fn compute_harness_template_data(
    sol: &Solution,
    modules: &[&Construct],
    registry: &LayerRegistry,
    ir: &HarnessIR,
) -> HarnessTemplateData {
    let mut all_endpoints = Vec::new();
    let mut deps_wiring_list = Vec::new();
    let mut free_fn_methods: BTreeSet<String> = BTreeSet::from(["get".to_string()]);
    let mut any_query = false;
    let mut global_method_path: HashSet<(String, String)> = HashSet::new();
    let mut bus_handler_targets: Vec<(String, bool, Construct)> = Vec::new();
    let mut router_names = Vec::new();
    let mut use_stmts = Vec::new();

    // Pre-scan for imports
    for module in modules {
        let flat = flatten_module(module, registry);
        let crate_name = module_crate_name(module, sol);
        let ctx = harness_ctx(ir, &crate_name, &module.name);
        let mut by_path: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        if let Some(ctx) = ctx {
            for ep in &ctx.endpoints {
                let method = ep.method.to_ascii_lowercase();
                let path = ep.path.clone();
                if !seen.insert((method.clone(), path.clone())) {
                    continue;
                }
                by_path.entry(path.clone()).or_default().push(method.clone());
                if ep.binds.iter().any(|b| matches!(b.source, BindSource::Query)) {
                    any_query = true;
                }
                if let Some(svc) = flat.fns.iter().find(|s| s.name == ep.handler) {
                    let path_params = path_param_names(&path);
                    if harness_handler_needs_query(svc, registry, &method, &path, &path_params) {
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

    // Derive bus info
    let has_bus = ir.contexts.iter().any(|c| {
        !c.bus_handlers.is_empty()
            || c.compose.as_ref().is_some_and(|co| {
                co.wires.iter().any(|w| matches!(w.kind, WireKind::ProvidedRuntime))
            })
    });
    let routing_trait_name: Option<String> = ir.contexts.iter().find_map(|c| {
        let compose = c.compose.as_ref()?;
        let wire = compose.wires.iter().find(|w| matches!(w.kind, WireKind::ProvidedRuntime))?;
        let deps = c.deps.as_ref()?;
        deps.fields.iter().find(|f| f.name == wire.field).map(|f| f.trait_name.clone())
    });
    let bus_type = routing_trait_name.as_ref().map(|t| format!("InProcess{t}"));

    // Use statements
    for m in modules {
        let cn = module_crate_name(m, sol);
        let declared_deps = harness_ctx(ir, &cn, &m.name).and_then(|c| c.deps.as_ref());
        if let Some(deps) = declared_deps {
            if deps.type_name == "Deps" {
                use_stmts.push(format!(
                    "use {cn}::application::{{self as {cn}_app, Deps as {cn}_Deps}};"
                ));
            } else {
                use_stmts.push(format!(
                    "use {cn}::application::{{self as {cn}_app, {} as {cn}_Deps}};",
                    deps.type_name
                ));
            }
        } else {
            use_stmts.push(format!("use {cn}::application::{{self as {cn}_app}};"));
        }
    }

    // Process each module/context
    for module in modules {
        let crate_name = module_crate_name(module, sol);
        let flat = flatten_module(module, registry);
        let adapters = &flat.impls;
        let services = &flat.fns;
        if adapters.is_empty() && services.is_empty() {
            continue;
        }

        let name_to_shape = build_name_to_shape(sol, registry);
        let (deps_set, dep_fields) = collect_deps_field_map(services, registry, &name_to_shape);
        let ctx = harness_ctx(ir, &crate_name, &module.name);
        let declared_compose = ctx.and_then(|c| c.compose.as_ref());
        let declared_deps = ctx.and_then(|c| c.deps.as_ref());

        // Compute wiring (same logic as gen_local_harness_main)
        let mut wired: Vec<(String, String, &Construct)> = Vec::new();
        let mut wired_fields: HashSet<String> = HashSet::new();
        let mut wired_adapter_names: HashSet<String> = HashSet::new();
        for ad in adapters {
            if is_pure_generic_adapter_template(ad) {
                continue;
            }
            let Some(compose) = declared_compose else { continue };
            let named = compose.wires.iter().any(|w| match &w.kind {
                WireKind::Adapter { name } => name == &ad.name,
                _ => false,
            });
            if !named { continue }
            if let Some(target) = &ad.target {
                let field = compose
                    .wires
                    .iter()
                    .find(|w| matches!(&w.kind, WireKind::Adapter { name } if name == &ad.name))
                    .map(|w| w.field.clone())
                    .unwrap_or_else(|| adapter_deps_field_name(sol, ad, target, &dep_fields));
                if !wired_fields.insert(field.clone()) { continue }
                wired_adapter_names.insert(ad.name.clone());
                wired.push((field, to_snake(&ad.name), ad));
            }
        }

        // Compute deps wiring data
        let mut stub_lets = Vec::new();
        let mut adapter_insts = Vec::new();
        let mut emitted_harness_lets: HashSet<String> = HashSet::new();

        // Stub harness_field lets
        for ad in adapters {
            if !wired_adapter_names.contains(&ad.name) { continue }
            for ann in &ad.annotations {
                if !registry.is_adapter_field_annotation(&ann.name) { continue }
                for arg in &ann.args {
                    let ftype = arg.split_once(':').map(|(_, t)| t.trim()).unwrap_or("").to_string();
                    if ftype.is_empty() || emitted_harness_lets.contains(&ftype) { continue }
                    if let Some((let_name, expr)) = stub_harness_field_expr(registry, &ftype) {
                        stub_lets.push(format!("    let {let_name} = {expr};"));
                        emitted_harness_lets.insert(ftype);
                    }
                }
            }
            let body_uses_client = ad.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            });
            let has_field_client = ad.annotations.iter().any(|a| {
                registry.is_adapter_field_annotation(&a.name)
                    && a.args.iter().any(|arg| arg.split_once(':').map(|(n, _)| n.trim()) == Some("client"))
            });
            if body_uses_client && !has_field_client && !emitted_harness_lets.contains("Client") {
                if let Some((let_name, expr)) = stub_harness_field_expr(registry, "Client") {
                    stub_lets.push(format!("    let {let_name} = {expr};"));
                    emitted_harness_lets.insert("Client".into());
                }
            }
        }

        // Adapter instantiations
        let mut adapters_ordered: Vec<&Construct> = adapters.to_vec();
        adapters_ordered.sort_by_key(|ad| {
            ad.fields.iter().any(|f| {
                matches!(&f.type_expr, TypeExpr::Named(n) if n.chars().next().is_some_and(|c| c.is_uppercase()))
            }) as u8
        });
        for ad in adapters_ordered {
            if !wired_adapter_names.contains(&ad.name) { continue }
            let mut field_inits: BTreeMap<String, String> = BTreeMap::new();
            for ann in &ad.annotations {
                if registry.is_adapter_field_annotation(&ann.name) {
                    for arg in &ann.args {
                        let (fname, ftype) = if let Some((n, t)) = arg.split_once(':') {
                            (n.trim().to_string(), t.trim())
                        } else {
                            (arg.trim().to_string(), "String")
                        };
                        let init = if let Some((let_name, _)) = stub_harness_field_expr(registry, ftype) {
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
            if body_uses_client && !has_explicit_client_field {
                if let Some((let_name, _)) = stub_harness_field_expr(registry, "Client") {
                    field_inits.entry("client".to_string()).or_insert_with(|| format!("{let_name}.clone()"));
                }
            }
            for f in &ad.fields {
                let field_name = to_snake(&f.name);
                if field_inits.contains_key(&field_name) { continue }
                if let TypeExpr::Named(tn) = &f.type_expr {
                    if let Some(impl_ad) = adapters.iter().find(|a| a.target.as_deref() == Some(tn.as_str())) {
                        field_inits.insert(field_name, format!("{}_inst.clone()", to_snake(&impl_ad.name)));
                        continue;
                    }
                    if let Some((let_name, _)) = stub_harness_field_expr(registry, tn) {
                        field_inits.insert(field_name, format!("{let_name}.clone()"));
                        continue;
                    }
                }
                let env_key = f.name.to_uppercase();
                field_inits.insert(field_name, format!("std::env::var(\"{env_key}\").unwrap_or_else(|_| \"default\".into())"));
            }
            let raw_dyn_ty = adapter_dyn_type(sol, ad);
            let dyn_ty = format!("{}::ports::{}", crate_name, raw_dyn_ty);
            let sn = to_snake(&ad.name);
            if field_inits.is_empty() {
                adapter_insts.push(format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{}{{}});",
                    ad.name
                ));
            } else {
                let mut s = format!(
                    "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{} {{\n",
                    ad.name
                );
                for (fname, init) in &field_inits {
                    s.push_str(&format!("        {fname}: {init},\n"));
                }
                s.push_str("    });");
                adapter_insts.push(s);
            }
        }

        // Missing deps → InMemory fallback
        let mut provided_fields: Vec<String> = Vec::new();
        if let Some(compose) = declared_compose {
            for w in &compose.wires {
                if matches!(w.kind, WireKind::ProvidedRuntime) {
                    provided_fields.push(w.field.clone());
                }
            }
        }
        if let Some(deps) = declared_deps {
            for f in &deps.fields {
                if !wired_fields.contains(&f.name) && !provided_fields.iter().any(|p| p == &f.name) {
                    let inmem = format!("InMemory{}", f.trait_name);
                    let sn = to_snake(&inmem);
                    adapter_insts.push(format!(
                        "    let {sn}_inst: Arc<dyn {crate_name}::ports::{} + Send + Sync> = Arc::new({crate_name}::adapters::{inmem}::new());",
                        f.trait_name
                    ));
                    wired.push((f.name.clone(), sn, module));
                    wired_fields.insert(f.name.clone());
                }
            }
        }

        let has_deps = declared_deps
            .map(|d| !d.fields.is_empty())
            .unwrap_or(!dep_fields.is_empty());

        let mut deps_fields_vec: Vec<(String, String)> = Vec::new();
        for (field, sn, _) in &wired {
            deps_fields_vec.push((field.clone(), format!("{sn}_inst.clone()")));
        }
        for bus_field in &provided_fields {
            deps_fields_vec.push((bus_field.clone(), "Arc::new(bus.clone())".to_string()));
        }

        // Bus handler targets
        if has_bus {
            if let Some(ctx) = ctx {
                for bh in &ctx.bus_handlers {
                    if let Some(svc) = services.iter().find(|s| s.name == bh.name) {
                        bus_handler_targets.push((crate_name.clone(), has_deps, (*svc).clone()));
                    }
                }
            }
        }

        // Router routes (compute before deps_wiring push so we can embed it)
        let router_data = if !services.is_empty() {
            router_names.push(format!("{crate_name}_router"));

            let declared_eps = ctx.map(|c| c.endpoints.as_slice()).unwrap_or(&[]);
            let mut routes_emitted: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
            let mut seen_method_path: HashSet<(String, String)> = HashSet::new();

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
                if !seen_method_path.insert(key.clone()) { continue }
                global_method_path.insert(key);
                routes_emitted.entry(path).or_default().push((method, format!("{fn_name}_handler")));
            }

            let routes: Vec<(String, String)> = routes_emitted
                .into_iter()
                .map(|(path, handlers)| {
                    let chained = handlers.iter()
                        .map(|(m, h)| format!("{m}({h})"))
                        .collect::<Vec<_>>()
                        .join(".");
                    (path, chained)
                })
                .collect();

            Some(RouterTemplateData {
                crate_name: crate_name.clone(),
                module_name: module.name.clone(),
                has_deps,
                routes,
            })
        } else {
            None
        };

        deps_wiring_list.push(DepsWiringData {
            crate_name: crate_name.clone(),
            module_name: module.name.clone(),
            stub_lets: stub_lets.clone(),
            adapter_insts: adapter_insts.clone(),
            deps_fields: deps_fields_vec,
            provided_fields: provided_fields.clone(),
            has_deps,
            deps_type_alias: format!("{crate_name}_Deps"),
            router: router_data,
        });

        // Endpoint handlers
        let handler_eps = ctx.map(|c| c.endpoints.as_slice()).unwrap_or(&[]);
        let mut seen_handler_fns: HashSet<String> = HashSet::new();
        for ep in handler_eps {
            let app_fn_name = to_snake(&ep.handler);
            let fn_name = format!("{}_{}", crate_name, &app_fn_name);
            if !seen_handler_fns.insert(fn_name.clone()) { continue }

            let method = ep.method.to_ascii_lowercase();
            let path = ep.path.clone();
            let pp = path_param_names(&path);

            let svc = flat.fns.iter().find(|s| s.name == ep.handler);
            let has_non_path_inputs = svc.map(|s| {
                s.inputs.iter().any(|i| {
                    !registry.field_is_dependency(i)
                        && !pp.iter().any(|p| p == &to_snake(&i.name))
                })
            }).unwrap_or(false);

            let needs_body = method == "post" || method == "put" || method == "patch";
            let needs_query = svc.map(|s| {
                harness_handler_needs_query(s, registry, &method, &path, &pp)
                    || (method == "delete" && has_non_path_inputs)
            }).unwrap_or(false);

            // Compute field extraction lines
            let mut field_extractions = Vec::new();
            let mut call_args: Vec<String> = Vec::new();

            // Deps arg
            let svc_has_deps = svc.map(|s| {
                !deps_set.is_empty() && (s.inputs.iter().any(|i| registry.field_is_dependency(i))
                    || s.steps.iter().any(|st| {
                        if let FlowStep::Step(step) = st {
                            step.body.iter().any(expr_mentions_trait_dep)
                        } else {
                            false
                        }
                    }))
            }).unwrap_or(false);

            if svc_has_deps {
                call_args.push("&deps".to_string());
            }

            if let Some(svc) = svc {
                for input in &svc.inputs {
                    if registry.field_is_dependency(input) { continue }
                    let field = to_snake(&input.name);
                    let rust_type = type_to_rust(&input.type_expr);

                    if pp.iter().any(|p| p == &field) {
                        // Path param
                        if rust_type == "Uuid" {
                            field_extractions.push(format!(
                                "    let {field} = {field}.parse::<Uuid>().map_err(|_| StatusCode::BAD_REQUEST)?;"
                            ));
                        }
                        // String: already extracted from Path
                    } else if needs_query {
                        field_extractions.push(
                            query_field_extraction(&field, &rust_type)
                        );
                    } else if needs_body {
                        field_extractions.push(
                            harness_body_field_extract(&field, &rust_type).trim_end().to_string()
                        );
                    }
                    call_args.push(field);
                }
            }

            all_endpoints.push(EndpointTemplateData {
                fn_name: fn_name.clone(),
                app_fn_name,
                crate_name: crate_name.clone(),
                method: method.clone(),
                path: path.clone(),
                has_deps,
                deps_type: format!("{crate_name}_Deps"),
                path_params: pp.clone(),
                needs_query,
                needs_body,
                field_extractions,
                call_args,
                is_delete: method == "delete",
            });
        }
    }

    // Bus registrations
    let mut bus_registrations = Vec::new();
    if has_bus && !bus_handler_targets.is_empty() {
        let mut registered: HashSet<String> = HashSet::new();
        for (crate_name, has_deps, svc) in &bus_handler_targets {
            let message = registry.bus_message_name(&svc.name);
            if !registered.insert(message.clone()) { continue }
            bus_registrations.push(gen_bus_handler_registration(
                crate_name, *has_deps, svc, &message, registry,
            ));
        }
    }

    // Layer helpers
    let secret_keys = collect_secret_field_names(modules, registry)
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let err_type = registry.error_model.as_ref().map(|em| em.type_name.as_str()).unwrap_or("__VEIL_NO_ERROR_MODEL__");
    let not_found = registry.error_model.as_ref()
        .and_then(|em| em.variant("not_found"))
        .unwrap_or("__NO_NOT_FOUND__");
    let validation = registry.error_model.as_ref()
        .and_then(|em| em.variant("validation"))
        .unwrap_or("__NO_VALIDATION__");
    let external = registry.error_model.as_ref()
        .and_then(|em| em.variant("external"))
        .unwrap_or("__NO_EXTERNAL__");

    let mut layer_helpers = String::new();
    let mut emitted_layer_helpers = false;
    for (target, code) in &registry.shared_emit {
        if target == "rust_bin" {
            let substituted = code
                .replace("{error_type}", err_type)
                .replace("{not_found_variant}", not_found)
                .replace("{validation_variant}", validation)
                .replace("{external_variant}", external)
                .replace("{secret_keys}", &secret_keys);
            layer_helpers.push('\n');
            layer_helpers.push_str(&substituted);
            layer_helpers.push('\n');
            emitted_layer_helpers = true;
        }
    }
    if !emitted_layer_helpers {
        layer_helpers.push_str(&harness_json_public_helper(modules, registry));
        layer_helpers.push_str(&harness_domain_error_status_helper_dynamic(err_type, not_found, validation, external));
        layer_helpers.push_str(harness_auth_cors_helpers());
        layer_helpers.push_str(harness_body_dt_helper());
    }

    let endpoint_count: usize = ir.contexts.iter().map(|c| c.endpoints.len()).sum();
    let health_path = ir.health_path.clone().unwrap_or_else(|| "/health".to_string());

    HarnessTemplateData {
        package_name: sol.name.clone(),
        port_env: "PORT".to_string(),
        port_default: ir.listen.default_port,
        endpoints: all_endpoints,
        deps_wiring: deps_wiring_list,
        has_bus,
        bus_type,
        router_names,
        profile: ir.profile.clone(),
        endpoint_count,
        use_statements: use_stmts,
        routing_imports: free_fn_methods.into_iter().collect(),
        any_query,
        bus_registrations,
        layer_helpers,
        health_path,
    }
}

/// Generate the complete harness main.rs from pre-computed template data.
/// This is the axum-aware rendering step — the template format is layer-agnostic.
pub fn render_harness_from_template_data(data: &HarnessTemplateData) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "//! HTTP harness for package `{}` (RT-001 / RT-003).\n\
         //! Wires adapters + exposes services as REST endpoints.\n\
         //! `cargo run -p veil_bin` from the generated workspace root.\n\n",
        data.package_name
    ));
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use std::sync::Arc;\n");

    // Imports
    let routing_imports = data.routing_imports.join(", ");
    let query_import = if data.any_query { "extract::Query, " } else { "" };
    out.push_str(&format!(
        "use axum::{{Router, Json, extract::State, {query_import}routing::{{{routing_imports}}}, http::{{HeaderMap, StatusCode}}, middleware::{{from_fn, Next}}, response::Response, extract::Request}};\n"
    ));
    out.push_str("use tower_http::cors::{Any, CorsLayer};\n");
    out.push_str("use uuid::Uuid;\n");
    out.push_str("use serde_json::Value;\n");
    out.push_str("use veil_shared::*;\n");
    for stmt in &data.use_statements {
        out.push_str(stmt);
        out.push('\n');
    }

    // main()
    out.push_str("\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
    out.push_str(&format!(
        "    let port: u16 = std::env::var(\"{}\").ok().and_then(|s| s.parse().ok()).unwrap_or({});\n\n",
        data.port_env, data.port_default
    ));

    // Bus
    if data.has_bus {
        if let Some(bus_type) = &data.bus_type {
            out.push_str(&format!("    let bus = veil_shared::{bus_type}::new();\n\n"));
        }
    }

    // Deps wiring + router per context (interleaved as in original)
    for wiring in &data.deps_wiring {
        out.push_str(&format!("    // ── context {} ──\n", wiring.module_name));
        for s in &wiring.stub_lets {
            out.push_str(s);
            out.push('\n');
        }
        if !wiring.stub_lets.is_empty() { out.push('\n'); }
        for s in &wiring.adapter_insts {
            out.push_str(s);
            out.push('\n');
        }
        if wiring.has_deps {
            out.push_str(&format!("    let {}_deps = Arc::new({} {{\n", wiring.crate_name, wiring.deps_type_alias));
            for (field, value) in &wiring.deps_fields {
                out.push_str(&format!("        {field}: {value},\n"));
            }
            out.push_str("    });\n\n");
        }
        // Router for this context (immediately after deps)
        if let Some(router) = &wiring.router {
            out.push_str(&format!("    let {}_router = Router::new()\n", router.crate_name));
            for (path, chained) in &router.routes {
                out.push_str(&format!("        .route(\"{path}\", {chained})\n"));
            }
            out.push_str("        .layer(from_fn(veil_api_key_middleware))\n");
            out.push_str("        .layer(veil_cors_layer())\n");
            if router.has_deps {
                out.push_str(&format!("        .with_state({}_deps.clone());\n\n", router.crate_name));
            } else {
                out.push_str("        .with_state(());\n\n");
            }
        }
    }

    // Bus registrations
    if !data.bus_registrations.is_empty() {
        out.push_str("    // ── bus handlers (cross-context invoke / request) ──\n");
        for reg in &data.bus_registrations {
            out.push_str(reg);
        }
        out.push('\n');
    }

    // Merge routers
    if data.router_names.is_empty() {
        out.push_str(&format!(
            "    let app = Router::new().route(\"{}\", get(|| async {{ \"ok\" }}));\n",
            data.health_path
        ));
    } else {
        out.push_str(&format!("    let app = {}", data.router_names[0]));
        for r in &data.router_names[1..] {
            out.push_str(&format!(".merge({})", r));
        }
        out.push_str(&format!(
            "\n        .route(\"{}\", get(|| async {{ \"ok\" }}));\n",
            data.health_path
        ));
    }

    out.push_str(&format!(
        "    println!(\"veil_bin: profile={} endpoints={}\");\n",
        data.profile, data.endpoint_count
    ));
    out.push_str("    println!(\"veil_bin: listening on :{}\", port);\n");
    out.push_str("    let listener = tokio::net::TcpListener::bind(format!(\"0.0.0.0:{}\", port)).await?;\n");
    out.push_str("    axum::serve(listener, app.into_make_service()).await?;\n");
    out.push_str("    Ok(())\n}\n\n");

    // Handler functions
    for ep in &data.endpoints {
        let state_extractor = if ep.has_deps {
            format!("\n    State(deps): State<Arc<{}>>,", ep.deps_type)
        } else {
            String::new()
        };
        let path_extractor = match ep.path_params.len() {
            0 => String::new(),
            1 => format!(
                "\n    axum::extract::Path({p}): axum::extract::Path<String>,",
                p = ep.path_params[0]
            ),
            n => {
                let names = ep.path_params.join(", ");
                let tys = vec!["String"; n].join(", ");
                format!("\n    axum::extract::Path(({names})): axum::extract::Path<({tys})>,")
            }
        };
        let query_extractor = if ep.needs_query {
            "\n    Query(q): Query<std::collections::HashMap<String, String>>,"
        } else {
            ""
        };
        let body_extractor = if ep.needs_body {
            "\n    Json(body): Json<Value>,"
        } else {
            ""
        };

        out.push_str(&format!(
            "async fn {}_handler({state_extractor}{path_extractor}{query_extractor}{body_extractor}\n) -> Result<Json<Value>, StatusCode> {{\n",
            ep.fn_name
        ));

        for line in &ep.field_extractions {
            out.push_str(line);
            out.push('\n');
        }

        out.push_str(&format!(
            "    match {}_app::{}({}).await {{\n",
            ep.crate_name, ep.app_fn_name, ep.call_args.join(", ")
        ));
        if ep.is_delete {
            out.push_str("        Ok(_) => Ok(Json(serde_json::json!({\"ok\": true}))),\n");
        } else {
            out.push_str("        Ok(result) => Ok(Json(veil_json_public(&result))),\n");
        }
        out.push_str("        Err(e) => Err(veil_domain_error_status(e)),\n");
        out.push_str("    }\n}\n\n");
    }

    // Layer helpers
    out.push_str(&data.layer_helpers);

    out
}

/// Query field extraction helper — mirrors the logic in gen_local_harness_main.
fn query_field_extraction(field: &str, rust_type: &str) -> String {
    match rust_type {
        "Uuid" => format!(
            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok()).ok_or(StatusCode::BAD_REQUEST)?;"
        ),
        "String" => format!(
            "    let {field} = q.get(\"{field}\").cloned().unwrap_or_default();"
        ),
        "Option<String>" => format!(
            "    let {field} = q.get(\"{field}\").filter(|s| !s.is_empty()).cloned();"
        ),
        "Option<i64>" => format!(
            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<i64>().ok());"
        ),
        "Option<bool>" => format!(
            "    let {field} = q.get(\"{field}\").map(|s| s == \"true\" || s == \"1\");"
        ),
        t if t.starts_with("Option<") => format!(
            "    let {field} = q.get(\"{field}\").filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str(s).ok());"
        ),
        "i64" => format!(
            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<i64>().ok()).ok_or(StatusCode::BAD_REQUEST)?;"
        ),
        "bool" => format!(
            "    let {field} = q.get(\"{field}\").map(|s| s == \"true\" || s == \"1\").unwrap_or(false);"
        ),
        _ => format!(
            "    let {field} = q.get(\"{field}\").and_then(|s| serde_json::from_str(s).ok()).ok_or(StatusCode::BAD_REQUEST)?;"
        ),
    }
}
