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
pub struct HttpRoutingData {
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
    pub router: Option<HttpRoutingData>,
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

/// Compute harness template data from multiple packages (multi-package devloop).
/// Calls compute_harness_template_data per package and merges into one HarnessTemplateData.
pub fn compute_multi_harness_template_data(
    packages: &[(&Solution, &LayerRegistry)],
) -> HarnessTemplateData {
    let mut merged = HarnessTemplateData {
        package_name: String::from("multi"),
        port_env: String::from("PORT"),
        port_default: 3000,
        endpoints: Vec::new(),
        deps_wiring: Vec::new(),
        has_bus: false,
        bus_type: None,
        router_names: Vec::new(),
        profile: String::from("http"),
        endpoint_count: 0,
        use_statements: Vec::new(),
        routing_imports: Vec::new(),
        any_query: false,
        bus_registrations: Vec::new(),
        layer_helpers: String::new(),
        health_path: String::from("/health"),
    };

    let mut seen_imports: BTreeSet<String> = BTreeSet::from(["get".to_string()]);

    for (sol, registry) in packages {
        let modules: Vec<&Construct> = sol.items.iter().filter_map(|item| {
            if let TopLevelItem::Construct(c) = item
                && c.shape == veil_ir::layer::Shape::Mod
            {
                Some(c)
            } else {
                None
            }
        }).collect();

        if modules.is_empty() {
            continue;
        }

        let ir = veil_ir::lower_harness(sol, registry);
        let per_pkg = compute_harness_template_data(sol, &modules, registry, &ir);

        merged.endpoints.extend(per_pkg.endpoints);
        merged.deps_wiring.extend(per_pkg.deps_wiring);
        merged.has_bus = merged.has_bus || per_pkg.has_bus;
        if per_pkg.bus_type.is_some() {
            merged.bus_type = per_pkg.bus_type;
        }
        merged.router_names.extend(per_pkg.router_names);
        merged.endpoint_count += per_pkg.endpoint_count;
        for stmt in &per_pkg.use_statements {
            if !merged.use_statements.contains(stmt) {
                merged.use_statements.push(stmt.clone());
            }
        }
        for imp in &per_pkg.routing_imports {
            if seen_imports.insert(imp.clone()) {
                merged.routing_imports.push(imp.clone());
            }
        }
        merged.any_query = merged.any_query || per_pkg.any_query;
        merged.bus_registrations.extend(per_pkg.bus_registrations);
        if merged.layer_helpers.is_empty() {
            merged.layer_helpers = per_pkg.layer_helpers;
        }
        if per_pkg.health_path != "/health" {
            merged.health_path = per_pkg.health_path;
        }
    }

    if !merged.routing_imports.contains(&"get".to_string()) {
        merged.routing_imports.insert(0, "get".to_string());
    }

    merged
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

            Some(HttpRoutingData {
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
                                "    let {field} = {field}.parse::<Uuid>().map_err(|_| veil_bad_request_status())?;"
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
        // Layer didn't provide shared_emit rust_bin — emit minimal stubs.
        // The real implementations live in harness.layer's shared_emit.
        layer_helpers.push_str(&harness_json_public_helper(modules, registry));
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

/// Query field extraction helper — mirrors the logic in gen_local_harness_main.
fn query_field_extraction(field: &str, rust_type: &str) -> String {
    match rust_type {
        "Uuid" => format!(
            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok()).ok_or(veil_bad_request_status())?;"
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
            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<i64>().ok()).ok_or(veil_bad_request_status())?;"
        ),
        "bool" => format!(
            "    let {field} = q.get(\"{field}\").map(|s| s == \"true\" || s == \"1\").unwrap_or(false);"
        ),
        _ => format!(
            "    let {field} = q.get(\"{field}\").and_then(|s| serde_json::from_str(s).ok()).ok_or(veil_bad_request_status())?;"
        ),
    }
}

/// Render the harness main.rs from a layer-provided template and pre-computed data.
/// The template uses `{{var}}` for scalars, `{{for X in Y}}...{{end_for}}` for loops,
/// and `{{if cond}}...{{end_if}}` / `{{if cond}}...{{else}}...{{end_if}}` for conditionals.
///
/// This replaces `render_harness_from_template_data` — all framework syntax now lives
/// in the layer template, not in engine code.
pub fn render_harness_from_layer_template(
    template: &str,
    data: &HarnessTemplateData,
) -> String {
    let mut out = template.to_string();

    // --- Top-level scalar substitutions ---
    out = out.replace("{{package_name}}", &data.package_name);
    out = out.replace("{{port_env}}", &data.port_env);
    out = out.replace("{{port_default}}", &data.port_default.to_string());
    out = out.replace("{{profile}}", &data.profile);
    out = out.replace("{{endpoint_count}}", &data.endpoint_count.to_string());
    out = out.replace("{{health_path}}", &data.health_path);
    out = out.replace("{{layer_helpers}}", &data.layer_helpers);

    // Routing imports (e.g. "get, post, put")
    out = out.replace("{{routing_imports}}", &data.routing_imports.join(", "));
    // Query import conditional
    let query_import = if data.any_query { "extract::Query, " } else { "" };
    out = out.replace("{{query_import}}", query_import);

    // Use statements (loop)
    out = expand_harness_loop(&out, "use_statement", &data.use_statements, |body, stmt| {
        body.replace("{{use_statement}}", stmt)
    });

    // Bus type
    let bus_type = data.bus_type.as_deref().unwrap_or("");
    out = out.replace("{{bus_type}}", bus_type);

    // --- Top-level conditionals ---
    out = expand_harness_if(&out, "has_bus", data.has_bus);
    out = expand_harness_if(&out, "any_query", data.any_query);
    out = expand_harness_if(&out, "has_routers", !data.router_names.is_empty());
    out = expand_harness_if(&out, "has_bus_registrations", !data.bus_registrations.is_empty());

    // --- Bus registrations loop ---
    out = expand_harness_loop(&out, "bus_registration", &data.bus_registrations, |body, reg| {
        body.replace("{{bus_registration}}", reg)
    });

    // --- Router merge ---
    // For router_names, provide first + rest pattern
    if !data.router_names.is_empty() {
        out = out.replace("{{first_router}}", &data.router_names[0]);
        let merges: String = data.router_names[1..].iter()
            .map(|r| format!(".merge({})", r))
            .collect::<Vec<_>>()
            .join("");
        out = out.replace("{{merge_routers}}", &merges);
    } else {
        out = out.replace("{{first_router}}", "");
        out = out.replace("{{merge_routers}}", "");
    }

    // --- Deps wiring loop ---
    out = expand_harness_block_loop(&out, "wiring", &data.deps_wiring, |body, wiring| {
        let mut s = body.to_string();
        s = s.replace("{{wiring.crate_name}}", &wiring.crate_name);
        s = s.replace("{{wiring.module_name}}", &wiring.module_name);
        s = s.replace("{{wiring.deps_type_alias}}", &wiring.deps_type_alias);
        s = expand_harness_if(&s, "wiring.has_deps", wiring.has_deps);
        s = expand_harness_if(&s, "wiring.has_stub_lets", !wiring.stub_lets.is_empty());
        s = expand_harness_if(&s, "wiring.has_router", wiring.router.is_some());

        // Stub lets
        s = expand_harness_loop(&s, "stub_let", &wiring.stub_lets, |b, line| {
            b.replace("{{stub_let}}", line)
        });
        // Adapter insts
        s = expand_harness_loop(&s, "adapter_inst", &wiring.adapter_insts, |b, line| {
            b.replace("{{adapter_inst}}", line)
        });
        // Deps fields
        s = expand_harness_loop(&s, "deps_field", &wiring.deps_fields, |b, (field, value)| {
            b.replace("{{deps_field.name}}", field)
             .replace("{{deps_field.value}}", value)
        });

        // Router sub-block
        if let Some(router) = &wiring.router {
            s = s.replace("{{wiring.router.crate_name}}", &router.crate_name);
            s = expand_harness_if(&s, "wiring.router.has_deps", router.has_deps);
            s = expand_harness_loop(&s, "route", &router.routes, |b, (path, chained)| {
                b.replace("{{route.path}}", path)
                 .replace("{{route.chained}}", chained)
            });
        }
        s
    });

    // --- Endpoint handlers loop ---
    out = expand_harness_block_loop(&out, "endpoint", &data.endpoints, |body, ep| {
        let mut s = body.to_string();
        s = s.replace("{{endpoint.fn_name}}", &ep.fn_name);
        s = s.replace("{{endpoint.app_fn_name}}", &ep.app_fn_name);
        s = s.replace("{{endpoint.crate_name}}", &ep.crate_name);
        s = s.replace("{{endpoint.method}}", &ep.method);
        s = s.replace("{{endpoint.path}}", &ep.path);
        s = s.replace("{{endpoint.deps_type}}", &ep.deps_type);
        s = s.replace("{{endpoint.call_args}}", &ep.call_args.join(", "));

        s = expand_harness_if(&s, "endpoint.has_deps", ep.has_deps);
        s = expand_harness_if(&s, "endpoint.needs_query", ep.needs_query);
        s = expand_harness_if(&s, "endpoint.needs_body", ep.needs_body);
        s = expand_harness_if(&s, "endpoint.is_delete", ep.is_delete);

        // Path params
        let path_param_count = ep.path_params.len();
        s = expand_harness_if(&s, "endpoint.has_path_params", path_param_count > 0);
        s = expand_harness_if(&s, "endpoint.has_single_path_param", path_param_count == 1);
        s = expand_harness_if(&s, "endpoint.has_multi_path_params", path_param_count > 1);

        if path_param_count == 1 {
            s = s.replace("{{endpoint.path_param}}", &ep.path_params[0]);
        } else if path_param_count > 1 {
            let names = ep.path_params.join(", ");
            let tys = vec!["String"; path_param_count].join(", ");
            s = s.replace("{{endpoint.path_param_names}}", &names);
            s = s.replace("{{endpoint.path_param_types}}", &tys);
        }

        // Field extractions
        s = expand_harness_loop(&s, "extraction", &ep.field_extractions, |b, line| {
            b.replace("{{extraction}}", line)
        });
        s
    });

    // Post-processing: collapse runs of 3+ blank lines to max 2, trim trailing whitespace per line.
    let mut result = String::new();
    let mut consecutive_empty = 0;
    for line in out.lines() {
        let trimmed_end = line.trim_end();
        if trimmed_end.is_empty() {
            consecutive_empty += 1;
            if consecutive_empty <= 2 {
                result.push('\n');
            }
        } else {
            consecutive_empty = 0;
            result.push_str(trimmed_end);
            result.push('\n');
        }
    }
    result
}

/// Expand `{{if NAME}}...{{end_if}}` and `{{if NAME}}...{{else}}...{{end_if}}` blocks.
fn expand_harness_if(input: &str, name: &str, condition: bool) -> String {
    let open_tag = format!("{{{{if {name}}}}}");
    let close_tag = "{{end_if}}";
    let else_tag = "{{else}}";
    let mut result = input.to_string();

    while let Some(start) = result.find(&open_tag) {
        let after_open = start + open_tag.len();
        // Find matching end_if (accounting for nesting)
        let Some(end_pos) = find_matching_end_tag(&result[after_open..], "{{if ", close_tag) else {
            break;
        };
        let block = &result[after_open..after_open + end_pos];
        let full_end = after_open + end_pos + close_tag.len();

        // Check for else
        let (then_part, else_part) = if let Some(else_offset) = find_top_level_else(block) {
            (&block[..else_offset], &block[else_offset + else_tag.len()..])
        } else {
            (block, "")
        };

        let replacement = if condition { then_part } else { else_part };

        // If replacement is empty, consume surrounding newlines to avoid blank lines
        if replacement.trim().is_empty() {
            let actual_start = if start > 0 && result.as_bytes()[start - 1] == b'\n' {
                start - 1
            } else {
                start
            };
            let actual_end = if full_end < result.len() && result.as_bytes()[full_end] == b'\n' {
                full_end + 1
            } else {
                full_end
            };
            result = format!("{}{}", &result[..actual_start], &result[actual_end..]);
        } else {
            // Strip leading/trailing newline from the selected branch
            let replacement = replacement.strip_prefix('\n').unwrap_or(replacement);
            let replacement = replacement.strip_suffix('\n').unwrap_or(replacement);
            result = format!("{}{}\n{}", &result[..start], replacement, &result[full_end..]);
        }
    }
    result
}

/// Expand `{{for NAME in COLLECTION}}...{{end_for}}` loops for simple string items.
fn expand_harness_loop<T>(
    input: &str,
    item_name: &str,
    items: &[T],
    replacer: impl Fn(&str, &T) -> String,
) -> String {
    let open_tag = format!("{{{{for {item_name} in {item_name}s}}}}");
    let close_tag = "{{end_for}}";
    let mut result = input.to_string();

    while let Some(start) = result.find(&open_tag) {
        let after_open = start + open_tag.len();
        let Some(end_pos) = find_matching_end_tag(&result[after_open..], "{{for ", close_tag) else {
            break;
        };
        let body = &result[after_open..after_open + end_pos];
        let full_end = after_open + end_pos + close_tag.len();

        // Strip leading newline from body (the newline after the opening tag line)
        let body = body.strip_prefix('\n').unwrap_or(body);

        let mut expanded = String::new();
        for item in items {
            expanded.push_str(&replacer(body, item));
        }
        // Consume the line containing the opening tag and the line containing closing tag
        // The opening tag is on its own line, so consume preceding newline
        let actual_start = if start > 0 && result.as_bytes()[start - 1] == b'\n' {
            start - 1
        } else {
            start
        };
        // The closing tag is on its own line, so consume its trailing newline
        let actual_end = if full_end < result.len() && result.as_bytes()[full_end] == b'\n' {
            full_end + 1
        } else {
            full_end
        };
        if items.is_empty() {
            result = format!("{}{}", &result[..actual_start], &result[actual_end..]);
        } else {
            // Remove trailing newline from expanded to avoid double-newline at junction
            let expanded = expanded.strip_suffix('\n').unwrap_or(&expanded);
            result = format!("{}\n{}{}", &result[..actual_start], expanded, &result[actual_end..]);
        }
    }
    result
}

/// Expand `{{for NAME in NAMEs}}...{{end_for}}` loops for structured blocks.
fn expand_harness_block_loop<T>(
    input: &str,
    item_name: &str,
    items: &[T],
    replacer: impl Fn(&str, &T) -> String,
) -> String {
    let open_tag = format!("{{{{for {item_name} in {item_name}s}}}}");
    let close_tag = "{{end_for}}";
    let mut result = input.to_string();

    while let Some(start) = result.find(&open_tag) {
        let after_open = start + open_tag.len();
        let Some(end_pos) = find_matching_end_tag(&result[after_open..], "{{for ", close_tag) else {
            break;
        };
        let body = &result[after_open..after_open + end_pos];
        let full_end = after_open + end_pos + close_tag.len();

        // Strip leading newline from body
        let body = body.strip_prefix('\n').unwrap_or(body);

        let mut expanded = String::new();
        for item in items {
            expanded.push_str(&replacer(body, item));
        }
        let actual_start = if start > 0 && result.as_bytes()[start - 1] == b'\n' {
            start - 1
        } else {
            start
        };
        let actual_end = if full_end < result.len() && result.as_bytes()[full_end] == b'\n' {
            full_end + 1
        } else {
            full_end
        };
        if items.is_empty() {
            result = format!("{}{}", &result[..actual_start], &result[actual_end..]);
        } else {
            let expanded = expanded.strip_suffix('\n').unwrap_or(&expanded);
            result = format!("{}\n{}{}", &result[..actual_start], expanded, &result[actual_end..]);
        }
    }
    result
}

/// Find matching end tag accounting for nesting.
fn find_matching_end_tag(input: &str, open_prefix: &str, close_tag: &str) -> Option<usize> {
    let mut depth = 1usize;
    let mut pos = 0;
    while pos < input.len() {
        if input[pos..].starts_with(close_tag) {
            depth -= 1;
            if depth == 0 {
                return Some(pos);
            }
            pos += close_tag.len();
        } else if input[pos..].starts_with(open_prefix) {
            depth += 1;
            pos += open_prefix.len();
        } else {
            // Advance by one UTF-8 character
            pos += input[pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
    }
    None
}

/// Find the position of a top-level `{{else}}` (not nested inside inner if blocks).
fn find_top_level_else(block: &str) -> Option<usize> {
    let else_tag = "{{else}}";
    let mut depth = 0usize;
    let mut pos = 0;
    while pos < block.len() {
        if block[pos..].starts_with("{{if ") {
            depth += 1;
            pos += 5;
        } else if block[pos..].starts_with("{{end_if}}") {
            depth = depth.saturating_sub(1);
            pos += 10;
        } else if depth == 0 && block[pos..].starts_with(else_tag) {
            return Some(pos);
        } else {
            // Advance by one UTF-8 character
            pos += block[pos..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
    }
    None
}
