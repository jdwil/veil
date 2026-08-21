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
    // Used for field types, parameter types, etc. that never contain Result.
    // If a Result somehow leaks through, the sentinel makes the generated code
    // fail to compile with an obvious error message.
    type_to_rust_with_error_impl(ty, &std::collections::HashSet::new(), "__VEIL_NO_ERROR_MODEL__")
}

/// Convert a TypeExpr to Rust type string using the given error type name for Result<T, E>.
pub fn type_to_rust_with_error(ty: &TypeExpr, error_type: &str) -> String {
    type_to_rust_with_error_impl(ty, &std::collections::HashSet::new(), error_type)
}

/// REST body field extraction for dual-loop harness handlers.
///
/// Accepts HTML date inputs (`YYYY-MM-DD` → RFC3339 midnight UTC) and form empties
/// (`""` → null) so browser `<input type="date">` and optional fields do not 400.
pub fn harness_body_field_extract(field: &str, rust_type: &str) -> String {
    match rust_type {
        // Issue 2: never invent UUIDs — missing/invalid → 400.
        "Uuid" => format!(
            "    let {field} = body.get(\"{field}\").and_then(|v| v.as_str()).and_then(|s| s.parse::<Uuid>().ok()).ok_or(veil_bad_request_status())?;\n"
        ),
        "String" => format!(
            "    let {field} = body.get(\"{field}\").and_then(|v| v.as_str()).unwrap_or_default().to_string();\n"
        ),
        "DateTime<Utc>" => format!(
            "    let {field} = serde_json::from_value(veil_normalize_body_dt(body.get(\"{field}\").cloned().unwrap_or(Value::Null))).map_err(|_| veil_bad_request_status())?;\n"
        ),
        t if t.starts_with("Option<") && t.contains("DateTime") => format!(
            "    let {field} = serde_json::from_value(veil_normalize_body_dt(body.get(\"{field}\").cloned().unwrap_or(Value::Null))).map_err(|_| veil_bad_request_status())?;\n"
        ),
        t if t.starts_with("Option<") => format!(
            "    let {field} = {{\n        let __v = body.get(\"{field}\").cloned().unwrap_or(Value::Null);\n        let __v = if matches!(&__v, Value::String(s) if s.is_empty()) {{ Value::Null }} else {{ __v }};\n        serde_json::from_value(__v).map_err(|_| veil_bad_request_status())?\n    }};\n"
        ),
        _ => format!(
            "    let {field} = serde_json::from_value(body.get(\"{field}\").cloned().unwrap_or(Value::Null)).map_err(|_| veil_bad_request_status())?;\n"
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
        if let TopLevelItem::TypeAlias { name, target: te } = item
            && let TypeExpr::Generic(base, args) = te
                && base == target
                    && args.len() == ad.target_type_args.len()
                    && args
                        .iter()
                        .zip(ad.target_type_args.iter())
                        .all(|(a, b)| type_to_rust(a) == type_to_rust(b))
                {
                    return name.clone();
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
            if let TypeExpr::Generic(base, args) = te
                && base == target
                    && args.len() == ad.target_type_args.len()
                    && args
                        .iter()
                        .zip(ad.target_type_args.iter())
                        .all(|(a, b)| type_to_rust(a) == type_to_rust(b))
                {
                    return to_snake(name);
                }
            if let TypeExpr::Named(base) = te
                && base == target {
                    return to_snake(name);
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
pub fn type_to_rust_with_traits(ty: &TypeExpr, traits: &std::collections::HashSet<String>, error_type: &str) -> String {
    type_to_rust_with_error_impl(ty, traits, error_type)
}

/// Like `type_to_rust_with_traits` but uses the given error type name for Result<T, E>.
pub fn type_to_rust_with_traits_and_error(ty: &TypeExpr, traits: &std::collections::HashSet<String>, error_type: &str) -> String {
    type_to_rust_with_error_impl(ty, traits, error_type)
}

/// Render a function parameter type. A bare trait-typed parameter is passed by
/// shared reference (`&(dyn Trait + Send + Sync)`); a `List<Trait>` is passed as
/// a borrowed slice (`&[Box<dyn Trait + Send + Sync>]`) since boxed trait
/// objects aren't Clone and shouldn't be moved into a coordinator; other types
/// use the standard rendering.
pub fn param_type_to_rust(ty: &TypeExpr, traits: &std::collections::HashSet<String>, error_type: &str) -> String {
    if let TypeExpr::Named(name) = ty
        && traits.contains(name) {
            return format!("&(dyn {} + Send + Sync)", name);
        }
    if let TypeExpr::List(inner) = ty
        && let TypeExpr::Named(name) = inner.as_ref()
            && traits.contains(name) {
                return format!("&[Box<dyn {} + Send + Sync>]", name);
            }
    type_to_rust_with_error_impl(ty, traits, error_type)
}

/// The type name tracked for a parameter local, for method resolution. A bare
/// trait param tracks the unboxed trait name (so `x.method()` resolves to an
/// async trait call); other types track their Rust rendering.
pub fn local_type_for_param(ty: &TypeExpr, traits: &std::collections::HashSet<String>, error_type: &str) -> String {
    if let TypeExpr::Named(name) = ty
        && traits.contains(name) {
            return name.clone();
        }
    type_to_rust_with_error_impl(ty, traits, error_type)
}

pub fn type_to_rust_with_error_impl(ty: &TypeExpr, traits: &std::collections::HashSet<String>, error_type: &str) -> String {
    let rec = |t: &TypeExpr| type_to_rust_with_error_impl(t, traits, error_type);
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
        TypeExpr::Result(Some(inner)) => format!("Result<{}, {}>", rec(inner), error_type),
        TypeExpr::Result(None) => format!("Result<(), {}>", error_type),
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
            let p = params.iter().map(&rec).collect::<Vec<_>>().join(", ");
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
    // Compute template data from all packages
    let tpl_data = compute_multi_harness_template_data(packages);

    // Render main.rs via layer template (or fallback to old render function)
    let first_reg = packages.first().map(|(_, r)| *r);
    let main_rs = if let Some(reg) = first_reg {
        if let Some(layer_tpl) = reg.harness_render_templates.get("rust_bin") {
            render_harness_from_layer_template(layer_tpl, &tpl_data)
        } else {
            format!("fn main() {{\n    eprintln!(\"veil_bin: harness layer not loaded\");\n}}\n")
        }
    } else {
        format!("fn main() {{\n    eprintln!(\"veil_bin: no packages\");\n}}\n")
    };

    // Build Cargo.toml for veil_bin
    let mut cargo_toml = String::new();
    cargo_toml.push_str("[package]\nname = \"veil_bin\"\nversion.workspace = true\nedition.workspace = true\n\n");
    cargo_toml.push_str("[[bin]]\nname = \"veil_bin\"\npath = \"src/main.rs\"\n\n");
    cargo_toml.push_str("[dependencies]\ntokio = { workspace = true }\nuuid = { workspace = true }\nserde_json = { workspace = true }\n");
    cargo_toml.push_str("veil_shared = { path = \"../veil_shared\" }\n");
    // Framework-specific deps from layer
    if let Some(reg) = first_reg {
        if let Some(cargo_deps) = reg.harness_render_templates.get("rust_bin_cargo") {
            for line in cargo_deps.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    cargo_toml.push_str(trimmed);
                    cargo_toml.push('\n');
                }
            }
        }
    }

    // Stub crates from the packages being harnessed
    let mut seen_stub = std::collections::BTreeSet::new();
    for (_, reg) in packages {
        for stub in &reg.stubs {
            if !seen_stub.insert(stub.name.clone()) {
                continue;
            }
            if !stub_is_active_cargo(stub) {
                continue;
            }
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
    let mut all_crate_names: Vec<String> = Vec::new();
    for (sol, reg) in packages {
        for item in &sol.items {
            if let TopLevelItem::Construct(c) = item
                && c.shape == veil_ir::layer::Shape::Mod
            {
                let cn = module_crate_name(c, sol);
                if !all_crate_names.contains(&cn) {
                    all_crate_names.push(cn);
                }
            }
        }
    }
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
        return "std::collections::HashMap<String, String>".to_string();
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
