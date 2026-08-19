use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;

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
pub fn harness_body_field_extract(field: &str, rust_type: &str) -> String {
    match rust_type {
        // Issue 2: never invent UUIDs — missing/invalid → 400.
        "Uuid" => format!(
            "    let {field} = body.get(\"{field}\").and_then(|v| v.as_str()).and_then(|s| s.parse::<Uuid>().ok()).ok_or(StatusCode::BAD_REQUEST)?;\n"
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
pub fn harness_body_dt_helper() -> &'static str {
    r#"
/// HTML `<input type="date">` and form empties → JSON values chrono/serde accept.
/// `""` → null; bare `YYYY-MM-DD` → `YYYY-MM-DDT00:00:00Z`.
pub fn veil_normalize_body_dt(v: Value) -> Value {
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
pub fn generic_params_rust(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Dyn trait type for harness wiring: prefer type-alias marker (WearTestRepo)
/// when the adapter monomorphizes EntityRepo&lt;WearTest&gt;.
pub fn adapter_dyn_type(solution: &Solution, ad: &Construct) -> String {
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
pub fn adapter_deps_field_name(
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
pub fn collect_deps_field_map(
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
        // Process construct-level @dep(name: Type) annotations.
        // These declare named dependencies for the service.
        for ann in &f.annotations {
            if registry.is_dependency_annotation(&ann.name) {
                for arg in &ann.args {
                    if let Some((field_name, type_name)) = arg.split_once(':') {
                        let field_name = field_name.trim().to_string();
                        let type_name = type_name.trim().to_string();
                        all_deps.insert(type_name.clone());
                        dep_field_names.insert(type_name, field_name);
                    }
                }
            }
        }

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

    // Remove inferred deps that conflict with explicitly-declared @dep annotations.
    // If `@dep(registry: AcpSessionRegistry)` is declared, an inferred dep like
    // `ExtensionRegistry` (whose field name "registry" or suffix matches) is spurious.
    let explicit_field_names: std::collections::HashSet<String> =
        dep_field_names.values().cloned().collect();
    all_deps.retain(|t| {
        // Keep if it's already explicitly declared (has entry in dep_field_names)
        if dep_field_names.contains_key(t) {
            return true;
        }
        // Otherwise, the inferred field name would be to_snake(t).
        // If that field name (or a suffix of it) is already claimed, discard.
        let inferred_field = to_snake(t);
        // Check if any explicit field name is a suffix of the inferred one
        // e.g. explicit "registry" vs inferred "extension_registry" - the call.target
        // "registry" was already resolved by the @dep annotation.
        for ef in &explicit_field_names {
            if &inferred_field == ef || inferred_field.ends_with(&format!("_{}", ef)) {
                return false;
            }
        }
        true
    });

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
pub fn param_type_to_rust(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
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
pub fn local_type_for_param(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
    if let TypeExpr::Named(name) = ty {
        if traits.contains(name) {
            return name.clone();
        }
    }
    type_to_rust_impl(ty, traits)
}

pub fn type_to_rust_impl(ty: &TypeExpr, traits: &std::collections::HashSet<String>) -> String {
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
        TypeExpr::LitStr(s) => format!("&'static str /* {s} */"),
    }
}

/// Infer a Rust type for shorthand fields (untyped, name-only).
/// Purely conventional inference on the field NAME — not domain knowledge.
pub fn infer_field_type(name: &str) -> String {
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
                    let cn = module_crate_name(c, sol);
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
    main_rs.push_str("use axum::{Router, Json, extract::State, extract::Query, routing::{get, post, put, patch, delete}, http::{HeaderMap, StatusCode}, middleware::{from_fn, Next}, response::Response, extract::Request};\n");
    main_rs.push_str("use tower_http::cors::{Any, CorsLayer};\n");
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
        let flat = flatten_module(module, registry);
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
            // @field wins; @env fills gaps only (no duplicate pool init).
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
            let has_explicit_client = field_inits.contains_key("client");
            let body_uses_client = ad.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            });
            if body_uses_client && !has_explicit_client {
                if let Some((let_name, _)) = stub_harness_field_expr(registry, "Client") {
                    field_inits
                        .entry("client".to_string())
                        .or_insert_with(|| format!("{let_name}.clone()"));
                }
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

        // Build routes for this context from HarnessIR only.
        let router_name = format!("{crate_name}_routes");
        main_rs.push_str(&format!("    let {router_name} = Router::new()\n"));
        let mut ir = veil_ir::lower_harness(sol, registry);
        apply_compat_synthesis(&mut ir, sol, registry);
        let declared_eps = ir
            .contexts
            .iter()
            .find(|c| c.crate_name == *crate_name || c.module_name == module.name)
            .map(|c| c.endpoints.as_slice())
            .unwrap_or(&[]);
        for ep in declared_eps {
            let fn_name = to_snake(&ep.handler);
            let method = ep.method.to_ascii_lowercase();
            main_rs.push_str(&format!(
                "        .route(\"{}\", {method}({fn_name}_handler))\n",
                ep.path
            ));
        }
        main_rs.push_str("        .layer(from_fn(veil_api_key_middleware))\n");
        main_rs.push_str("        .layer(veil_cors_layer())\n");
        main_rs.push_str(&format!("        .with_state({crate_name}_deps);\n\n"));
        router_names.push(router_name);
    }

    // Merge all routers
    main_rs.push_str("    let app = Router::new()\n");
    for rn in &router_names {
        main_rs.push_str(&format!("        .merge({rn})\n"));
    }
    main_rs.push_str("        .route(\"/health\", get(|| async { \"ok\" }));\n\n");

    main_rs.push_str("    println!(\"veil_bin: listening on :{}\", port);\n");
    main_rs.push_str("    let listener = tokio::net::TcpListener::bind(format!(\"0.0.0.0:{}\", port)).await?;\n");
    main_rs.push_str("    axum::serve(listener, app).await?;\n");
    main_rs.push_str("    Ok(())\n}\n\n");

    // Generate handler functions from HarnessIR endpoints (same as single-package).
    for (module, crate_name, registry, sol) in &all_modules {
        let flat = flatten_module(module, registry);
        let mut ir = veil_ir::lower_harness(sol, registry);
        apply_compat_synthesis(&mut ir, sol, registry);
        let routable: Vec<(&Construct, String, String)> = ir
            .contexts
            .iter()
            .find(|c| c.crate_name == *crate_name || c.module_name == module.name)
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
        for (svc, method, path) in &routable {
            let fn_name = to_snake(&svc.name);
            let method = method.clone();
            let path = path.clone();
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
                registry.field_is_dependency(i)
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
                    !registry.field_is_dependency(i)
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
                if registry.field_is_dependency(input) {
                    continue;
                }
                let field = to_snake(&input.name);
                // Skip if already bound from path
                if args.iter().any(|a| a == &field) {
                    continue;
                }
                let rust_type = type_to_rust(&input.type_expr);
                if is_list_get {
                    // Query string (plain values; Opt/Option never hard-400)
                    if rust_type == "Uuid" {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").and_then(|s| s.parse::<Uuid>().ok()).ok_or(StatusCode::BAD_REQUEST)?;\n"
                        ));
                    } else if rust_type == "String" {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").cloned().unwrap_or_default();\n"
                        ));
                    } else if rust_type == "Option<String>" {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").filter(|s| !s.is_empty()).cloned();\n"
                        ));
                    } else if rust_type.starts_with("Option<") {
                        main_rs.push_str(&format!(
                            "    let {field} = q.get(\"{field}\").filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str(s).ok());\n"
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
                "        Err(e) => Err(veil_domain_error_status(e)),\n",
            );
            main_rs.push_str("    }\n}\n\n");
        }
    }

    main_rs.push_str(harness_domain_error_status_helper());
    main_rs.push_str(harness_body_dt_helper());
    main_rs.push_str(harness_auth_cors_helpers());

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

/// Convert a VEIL type annotation string (from @field) to a Rust type.
/// Handles: Str → String, Int → i64, Bool → bool, Map<K,V> → HashMap<K,V>,
/// List<T> → Vec<T>, Opt<T> → Option<T>, and passes domain types through.
pub fn veil_field_type_to_rust(veil_type: &str) -> String {
    let t = veil_type.trim();
    // Handle generic wrappers
    if let Some(inner) = t.strip_prefix("Map<").and_then(|s| s.strip_suffix('>')) {
        // Split at the top-level comma (respect nested generics)
        let parts = split_generic_args(inner);
        if parts.len() == 2 {
            let k = veil_field_type_to_rust(&parts[0]);
            let v = veil_field_type_to_rust(&parts[1]);
            return format!("std::collections::HashMap<{}, {}>", k, v);
        }
        return format!("std::collections::HashMap<String, String>");
    }
    if let Some(inner) = t.strip_prefix("List<").and_then(|s| s.strip_suffix('>')) {
        let inner_rust = veil_field_type_to_rust(inner);
        return format!("Vec<{}>", inner_rust);
    }
    if let Some(inner) = t.strip_prefix("Opt<").and_then(|s| s.strip_suffix('>')) {
        let inner_rust = veil_field_type_to_rust(inner);
        return format!("Option<{}>", inner_rust);
    }
    // Primitive mappings
    match t {
        "Str" => "String".to_string(),
        "Int" => "i64".to_string(),
        "Bool" => "bool".to_string(),
        "F64" => "f64".to_string(),
        "Dt" => "chrono::DateTime<chrono::Utc>".to_string(),
        "Id" => "uuid::Uuid".to_string(),
        "Json" => "serde_json::Value".to_string(),
        "Bytes" => "Vec<u8>".to_string(),
        _ => t.to_string(), // Domain types pass through as-is
    }
}

/// Split generic args at top-level commas (respecting nested `<>`).
pub fn split_generic_args(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    for ch in s.chars() {
        match ch {
            '<' => { depth += 1; current.push(ch); }
            '>' => { depth = depth.saturating_sub(1); current.push(ch); }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }
    parts
}
