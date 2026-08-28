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

/// RT-001/003/004: working local harness main — bus layer + first app svc.
/// CAP-002 / CAP-006: `@main` + `link veil_server` → ProductHost listen.
pub fn gen_product_host_main(sol: &Solution, handler_names: &[String], registry: &LayerRegistry) -> String {
    let _ = handler_names;
    // Routing layer loaded? Check if any layer provides shared_emit for rust
    // (bus.layer provides InProcessBus + handler registry via this mechanism).
    let has_routing = registry.shared_emit.iter().any(|(t, _)| t == "rust");
    let register_fn = format!("{}_{}", "register", "all");
    let use_register = if has_routing {
        format!("\nuse veil_shared::{register_fn};\n")
    } else {
        String::new()
    };
    let register_block = if has_routing {
        format!("\n    // CAP-003: register generated handler names (dispatch is host/platform).\n    let mut n = 0usize;\n    {register_fn}(|_name| n += 1);\n    tracing::info!(\"veil_bin: {{n}} handlers from {register_fn}\");\n")
    } else {
        String::new()
    };

    // Try layer-provided template first
    if let Some(tpl) = registry.harness_render_templates.get("product_host") {
        return tpl
            .replace("{{package_name}}", &sol.name)
            .replace("{{use_register}}", &use_register)
            .replace("{{register_block}}", &register_block);
    }

    // Fallback: harness.layer not loaded (should not happen in practice)
    format!(
        "//! Product host for package `{pkg}` — harness.layer not loaded.\nfn main() {{ eprintln!(\"harness.layer required for product host\"); }}\n",
        pkg = sol.name,
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
        || !stub.free_fns.is_empty()
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


/// Collect snake_case field names marked role:secret across the solution.
/// Used to compute the `{secret_keys}` placeholder for layer shared_emit templates.
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

// ─── FALLBACK HELPERS ────────────────────────────────────────────────────────
// These functions are superseded by `shared_emit rust_bin` in harness.layer.
// They remain as fallback for configurations that don't load the harness layer.
// In normal operation, the layer provides all helper code and these are unused.

/// Harness helper: Serialize then strip secret keys (INV-001 roles).
/// FALLBACK: superseded by harness.layer shared_emit rust_bin.
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
    let err_ext = registry.error_model.as_ref()
        .and_then(|em| em.variant_path("external"))
        .unwrap_or_else(|| "__VEIL_NO_ERROR_MODEL__::__NO_EXTERNAL__".to_string());
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
                 .map_err(|e| {err_ext}(e.to_string()))?"
            ));
        }
    }

    let call_args = call_parts.join(", ");
    out.push_str(&format!(
        "                let __result = {crate_name}_app::{app_fn}({call_args}).await?;\n\
         \x20               Ok(serde_json::to_value(__result)\
         .map_err(|e| {err_ext}(e.to_string()))?)\n\
         \x20           }}\n\
         \x20       }});\n\
         \x20   }}\n"
    ));
    out
}

