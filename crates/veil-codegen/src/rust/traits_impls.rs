use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;


pub fn gen_shared_crate(
    traits: &[&Construct],
    structs: &[&Construct],
    functions: &[&FnDef],
    solution: &Solution,
    registry: &LayerRegistry,
    links: &[crate::links::ResolvedLink],
    handler_names: &[String],
    layer_fn_attrs: Option<&str>,
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

    // ── Error model: generate from registry (layer-declared) ────────────
    let (err_type, err_not_found, err_validation, err_external) = if let Some(em) = &registry.error_model {
        let nf = em.variant("not_found").unwrap_or("NotFound").to_string();
        let val = em.variant("validation").unwrap_or("Validation").to_string();
        let ext = em.variant("external").unwrap_or("External").to_string();
        (em.type_name.clone(), nf, val, ext)
    } else {
        ("DomainError".to_string(), "NotFound".to_string(), "Validation".to_string(), "External".to_string())
    };
    lib.push_str(&format!("/// Domain error type.\n#[derive(Debug, thiserror::Error)]\npub enum {} {{\n", err_type));
    lib.push_str(&format!("    #[error(\"Not found\")]\n    {},\n", err_not_found));
    lib.push_str(&format!("    #[error(\"Validation failed: {{0}}\")]\n    {}(String),\n", err_validation));
    lib.push_str(&format!("    #[error(\"External service error: {{0}}\")]\n    {}(String),\n", err_external));
    // Emit any additional variants declared by the layer.
    if let Some(em) = &registry.error_model {
        for (role, variant) in &em.variants {
            if role != "not_found" && role != "validation" && role != "external" {
                // Additional variants get String payload by default.
                lib.push_str(&format!("    #[error(\"{role}: {{0}}\")]\n    {variant}(String),\n"));
            }
        }
    }
    lib.push_str("}\n\n");
    lib.push_str(&format!("/// Validation error type.\n#[derive(Debug, thiserror::Error)]\n#[error(\"Validation error: {{0}}\")]\npub struct ValidationError(pub String);\n\nimpl From<ValidationError> for {err_type} {{\n    fn from(e: ValidationError) -> Self {{\n        {err_type}::{err_validation}(e.0)\n    }}\n}}\n\n"));
    lib.push_str(&format!("impl From<serde_json::Error> for {err_type} {{\n    fn from(e: serde_json::Error) -> Self {{\n        {err_type}::{err_external}(e.to_string())\n    }}\n}}\n\n"));
    lib.push_str(&format!("impl From<String> for {err_type} {{\n    fn from(e: String) -> Self {{\n        {err_type}::{err_external}(e)\n    }}\n}}\n\n"));

    // Trait names in scope — used to box value-position references (List<Trait>).
    let trait_names: std::collections::HashSet<String> =
        traits.iter().map(|t| t.name.clone()).collect();

    // Local harness impls: routing trait(s) + auth trait from layer policy.
    let routing = registry.routing_traits();
    let mut routing_trait: Option<&Construct> = None;
    let mut auth_trait: Option<&Construct> = None;
    for t in traits {
        if routing.iter().any(|r| r == &t.name) && routing_trait.is_none() {
            routing_trait = Some(t);
        }
        if registry.is_auth_service_trait(&t.name) {
            auth_trait = Some(t);
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

    // RT-001 / RT-004: InProcessBus methods from the routing trait surface only.
    if let Some(rt) = routing_trait {
        lib.push_str(&gen_inprocess_bus_impl(rt, &trait_names, registry));
    }
    // RT-008: AllowAllAuth methods from the configured auth trait + Principal-like struct.
    if let Some(at) = auth_trait {
        lib.push_str(&gen_allow_all_auth_impl(at, structs, &trait_names, registry));
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
            ctx.types.local_types.insert(p.name.clone(), local_type_for_param(&p.type_expr, &trait_names));
        }
        ctx.ownership.mut_locals = crate::expr::analyze_mut_locals(&f.body);
        ctx.ownership.ident_uses = crate::expr::count_ident_uses(&f.body);

        let params = f
            .params
            .iter()
            .map(|p| format!("{}: {}", to_snake(&p.name), param_type_to_rust(&p.type_expr, &trait_names)))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = match &f.return_type {
            Some(t) => type_to_rust_with_traits_and_error(t, &trait_names, &err_type),
            None => format!("Result<(), {}>", err_type),
        };
        ctx.expected_return_rust = Some(ret.clone());
        let fn_mod = layer_fn_attrs.unwrap_or("pub");
        lib.push_str(&format!(
            "/// Layer-declared coordinator.\n{fn_mod} fn {}({}) -> {} {{\n",
            to_snake(&f.name),
            params,
            ret,
        ));
        for (i, expr) in f.body.iter().enumerate() {
            // stmt_to_rust tracks let-bindings so `mut x` then `x = ..` becomes
            // a declaration then a reassignment (not shadowing).
            let mut line = stmt_to_rust(expr, &mut ctx);
            // In tail position, strip `return ` since Rust allows expression-return.
            // This avoids clippy::needless_return.
            let is_last = i == f.body.len() - 1;
            if is_last {
                // stmt_to_rust returns "    return Ok(...);" — strip the return keyword
                // and trailing semicolon so it becomes a tail expression.
                if let Some(stripped) = line.strip_prefix("    return ") {
                    let stripped = stripped.strip_suffix(';').unwrap_or(stripped);
                    line = format!("    {}", stripped);
                }
            }
            lib.push_str(&line);
            lib.push('\n');
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

pub fn gen_traits(
    contents: &ModuleContents,
    crate_name: &str,
    solution: &Solution,
    registry: &LayerRegistry,
    layer_trait_attrs: Option<&str>,
    template_output: &crate::template::TemplateOutput,
) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Trait definitions (async traits).\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse uuid::Uuid;\n\n");
    out.push_str("use crate::domain::types::*;\n");
    // Common error types live in veil_shared. Layer-declared names are
    // re-exported unless the product defined the same name locally.
    out.push_str("pub use veil_shared::{DomainError, ValidationError};\n");
    let product_trait_names: std::collections::HashSet<&str> =
        contents.traits.iter().map(|t| t.name.as_str()).collect();
    let declared_types = layer_declared_type_names(registry);
    // A product construct that reuses a layer-declared name must not collide
    // with `pub use veil_shared::*`. Re-export the rest by name.
    let conflicts_shared = contents
        .traits
        .iter()
        .any(|t| declared_types.contains(&t.name));
    if conflicts_shared {
        for name in &declared_types {
            if !product_trait_names.contains(name.as_str()) {
                out.push_str(&format!("pub use veil_shared::{name};\n"));
            }
        }
        for fn_name in layer_declared_fn_names(registry) {
            let rust = to_snake(&fn_name);
            out.push_str(&format!("pub use veil_shared::{rust};\n"));
        }
        if !registry.routing_traits().is_empty() {
            out.push_str("pub use veil_shared::InProcessBus;\n");
        }
        out.push_str(
            "pub use veil_shared::{register_all, handler_count, HANDLER_NAMES};\n\n",
        );
    } else {
        out.push_str("pub use veil_shared::*;\n\n");
    }

    for t in &contents.traits {
        // ─── Construct lowers_to: template takes full control ──────────────
        if let Some(template) = registry.construct_lowers_to(t, "rust") {
            let rendered = crate::rust::interpolate_construct_template(template, t, registry);
            out.push_str(&rendered);
            out.push_str("\n\n");
            continue;
        }

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
        // Layer-driven trait attributes: if a layer declares emit_to "trait_attrs",
        // use that. Otherwise use the backend default (#[async_trait]).
        let trait_attr = layer_trait_attrs.unwrap_or("#[async_trait]");
        out.push_str(&format!(
            "/// {}: {}\n{}\npub trait {}{}: Send + Sync{where_bounds} {{\n",
            t.subkind, t.name, trait_attr, t.name, tp
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
        // Append inline template contributions for this trait (emit without emit_to/emit_file).
        if let Some(inline) = crate::template::compose_inline(template_output, &t.name) {
            out.push_str(&inline);
            out.push_str("\n\n");
        }
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

pub fn gen_impls(
    impls: &[&Construct],
    traits: &[&Construct],
    crate_name: &str,
    solution: &Solution,
    registry: &LayerRegistry,
) -> GeneratedFile {
    use crate::expr::{build_ctx_from_solution, expr_to_rust, GenCtx};

    let mut out = String::new();
    out.push_str("//! Implementations of traits.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables)]\n\n");
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
    // Product free fns and stub package free-fn roots are real symbols — skip.
    let product_free_fn_names: std::collections::HashSet<String> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Function(f) if !f.layer_provided => Some(to_snake(&f.name)),
            _ => None,
        })
        .collect();
    let stub_pkg_roots: std::collections::HashSet<String> = registry
        .stubs
        .iter()
        .flat_map(|s| {
            let rust = s.name.replace('-', "_");
            let mut names = vec![s.name.clone(), rust];
            if let Some(a) = &s.alias {
                names.push(a.clone());
            }
            names
        })
        .collect();
    let mut hooks: std::collections::BTreeSet<(String, usize)> = std::collections::BTreeSet::new();
    for c in impls {
        for mimpl in &c.impls {
            let mut locals: std::collections::HashSet<String> =
                mimpl.params.iter().cloned().collect();
            for expr in &mimpl.body {
                collect_effect_hooks_tracked(
                    expr,
                    &name_to_shape,
                    &mut locals,
                    &mut hooks,
                    &product_free_fn_names,
                    &stub_pkg_roots,
                );
            }
        }
    }
    // Unknown `target.method` calls emit `compile_error!("unstubbed external…")`
    // in expr lowering. We do not generate no-op hook functions — a missing
    // .stub must fail closed for every third-party crate.
    let _ = hooks;

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
            // Collect adapter fields into a map so @field and @env never double-declare
            // the same name (e.g. @field(pool: Pool) + @env(DATABASE_URL) → one `pool`).
            // @field wins on type; @env only fills gaps.
            let mut adapter_fields: std::collections::BTreeMap<String, String> =
                std::collections::BTreeMap::new();
            let seeded = build_ctx_from_solution(solution, name_to_shape.clone(), registry);
            for ann in &c.annotations {
                if registry.is_adapter_field_annotation(&ann.name) {
                    for arg in &ann.args {
                        if let Some((fname, ftype)) = arg.split_once(':') {
                            let fname = fname.trim().to_string();
                            let ftype = ftype.trim();
                            let qualified_type = if let Some((crate_name, original_name)) =
                                seeded.stubs.stub_type_crate.get(ftype)
                            {
                                format!("{}::{}", crate_name, original_name)
                            } else if let Some((crate_name, path)) =
                                stub_type_path(registry, ftype)
                            {
                                format!("{crate_name}::{path}")
                            } else {
                                // Convert VEIL primitive types to Rust equivalents
                                veil_field_type_to_rust(ftype)
                            };
                            adapter_fields.insert(fname, qualified_type);
                        } else {
                            adapter_fields
                                .entry(arg.to_lowercase())
                                .or_insert_with(|| "String".to_string());
                        }
                    }
                }
            }
            for ann in &c.annotations {
                if registry.is_adapter_env_annotation(&ann.name) {
                    for arg in &ann.args {
                        if arg.contains("DATABASE") {
                            // Only add pool when @field did not already declare it.
                            adapter_fields.entry("pool".to_string()).or_insert_with(|| {
                                if let Some((crate_name, path)) =
                                    stub_type_path(registry, "Pool")
                                {
                                    format!("{crate_name}::{path}")
                                } else {
                                    "String".to_string()
                                }
                            });
                        } else {
                            let field_name = env_var_field_name(arg);
                            adapter_fields
                                .entry(field_name)
                                .or_insert_with(|| "String".to_string());
                        }
                    }
                }
            }
            // @dep / injected port fields on the adapter (`@dep sns_client: SnsClient`).
            for f in &c.fields {
                let fname = to_snake(&f.name);
                if adapter_fields.contains_key(&fname) {
                    continue;
                }
                let rust_ty = match &f.type_expr {
                    TypeExpr::Named(n) => {
                        adapter_field_rust_type(n, &name_to_shape, registry)
                    }
                    other => type_to_rust(other),
                };
                adapter_fields.insert(fname, rust_ty);
            }
            for ann in &c.annotations {
                if !registry.is_dependency_annotation(&ann.name) {
                    continue;
                }
                for arg in &ann.args {
                    if let Some((n, t)) = arg.split_once(':') {
                        let fname = to_snake(n.trim());
                        let t = t.trim();
                        adapter_fields.entry(fname).or_insert_with(|| {
                            adapter_field_rust_type(t, &name_to_shape, registry)
                        });
                    }
                }
            }
            // Auto-detect self.client usage when no @field(client: ...) already.
            let has_explicit_client_field = adapter_fields.contains_key("client");
            let body_uses_client = c.impls.iter().any(|m| {
                m.body.iter().any(|e| expr_mentions_self_field(e, "client"))
            }) || generic_template
                .map(|t| {
                    t.impls
                        .iter()
                        .any(|m| m.body.iter().any(|e| expr_mentions_self_field(e, "client")))
                })
                .unwrap_or(false);
            if body_uses_client && !has_explicit_client_field
                && let Some((crate_name, path)) = stub_type_path(registry, "Client") {
                    adapter_fields
                        .entry("client".to_string())
                        .or_insert_with(|| format!("{crate_name}::{path}"));
                }
            for (fname, fty) in &adapter_fields {
                // Map/HashMap fields need interior mutability since trait methods
                // take `&self` but insert/remove require mutation. Wrap in RwLock.
                if fty.contains("HashMap") || fty.starts_with("std::collections::HashMap") {
                    out.push_str(&format!("    pub {fname}: tokio::sync::RwLock<{fty}>,\n"));
                } else {
                    out.push_str(&format!("    pub {fname}: {fty},\n"));
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
                // The impl's bare params are locals in the body; seed types from
                // the trait signature so Json/Option methods lower correctly.
                for p in &mimpl.params {
                    ctx.locals.insert(p.clone());
                }
                if let Some(m) = trait_method {
                    for p in &m.params {
                        let ty = match target_trait {
                            Some(t) => monomorphize_type(&p.type_expr, c, t),
                            None => p.type_expr.clone(),
                        };
                        ctx.types.local_types
                            .insert(to_snake(&p.name), type_to_rust(&ty));
                        ctx.locals.insert(to_snake(&p.name));
                    }
                }
                ctx.ownership.mut_locals = crate::expr::analyze_mut_locals(&mimpl.body);
                ctx.ownership.ident_uses = crate::expr::count_ident_uses(&mimpl.body);
                // @env annotation fields are available as self.field in the body.
                ctx.in_method = true;
                for ann in &c.annotations {
                    if registry.is_adapter_env_annotation(&ann.name) {
                        for arg in &ann.args {
                            let primary = env_var_field_name(arg);
                            ctx.self_fields.insert(primary.clone());
                            ctx.self_fields.insert(arg.to_ascii_lowercase());
                            ctx.self_fields.insert(arg.to_string());
                            // Last-segment alias (`TABLE_NAME` → `name`) still resolves
                            // at the use site to the full snake field.
                            if let Some(short) = arg.to_ascii_lowercase().rsplit('_').next()
                                && short != primary {
                                    ctx.self_fields.insert(short.to_string());
                                }
                            if arg.contains("DATABASE") {
                                ctx.self_fields.insert("pool".to_string());
                            }
                        }
                    }
                }
                // @field annotation typed fields are also available as self.field
                for ann in &c.annotations {
                    if registry.is_adapter_field_annotation(&ann.name) {
                        for arg in &ann.args {
                            let fname = arg.split(':').next().unwrap_or(arg).trim().to_lowercase();
                            ctx.self_fields.insert(fname);
                        }
                    }
                }
                // @dep / parsed fields (sns_client: SnsClient) are self.fields too.
                for f in &c.fields {
                    ctx.self_fields.insert(to_snake(&f.name));
                    ctx.self_fields.insert(f.name.clone());
                }
                for ann in &c.annotations {
                    if registry.is_dependency_annotation(&ann.name) {
                        for arg in &ann.args {
                            let fname = arg.split(':').next().unwrap_or(arg).trim();
                            ctx.self_fields.insert(to_snake(fname));
                            ctx.self_fields.insert(fname.to_string());
                        }
                    }
                }
                // Populate self_field_types so the expression translator can detect
                // Map/HashMap fields that need RwLock lock acquisition + &key args,
                // and so @dep port fields get trait `.await` / `.await?`.
                for (fname, fty) in &adapter_fields {
                    ctx.self_fields.insert(fname.clone());
                    ctx.self_field_types.insert(fname.clone(), fty.clone());
                }
                for ann in &c.annotations {
                    if registry.is_dependency_annotation(&ann.name) {
                        for arg in &ann.args {
                            if let Some((n, t)) = arg.split_once(':') {
                                let field = to_snake(n.trim());
                                let trait_name = t.trim().to_string();
                                ctx.dep_fields.insert(trait_name, field);
                            }
                        }
                    }
                }
                // Seed name→shape and method returns from stubs too.
                let seeded = build_ctx_from_solution(solution, name_to_shape.clone(), registry);
                ctx.types.method_returns = seeded.types.method_returns;
                ctx.types.method_params = seeded.types.method_params;
                ctx.types.struct_fields = seeded.types.struct_fields;
                ctx.stubs.stub_type_crate = seeded.stubs.stub_type_crate;
                ctx.stubs.fallible_methods = seeded.stubs.fallible_methods;
                ctx.stubs.non_fallible_methods = seeded.stubs.non_fallible_methods;
                ctx.stubs.type_fallible_methods = seeded.stubs.type_fallible_methods;
                ctx.stubs.async_fallible_methods = seeded.stubs.async_fallible_methods;
                ctx.stubs.type_async_fallible_methods = seeded.stubs.type_async_fallible_methods;
                ctx.stubs.stub_pkg_crate = seeded.stubs.stub_pkg_crate;
                ctx.stubs.stub_free_fns = seeded.stubs.stub_free_fns;
                ctx.async_fns = seeded.async_fns;
                ctx.types.ref_params = seeded.types.ref_params;
                ctx.name_to_shape = seeded.name_to_shape;
                ctx.enum_variants = seeded.enum_variants;
                ctx.unit_enums = seeded.unit_enums;
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
                    .any(|e| expr_refs_stub_type(e, &ctx.stubs.stub_type_crate));

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
                        "        compile_error!(\"implement adapter method: {}::{}\")\n",
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
                        if is_last && ret_rust.contains("Option<") {
                            ctx.option_value_wrap = true;
                        }
                        let rust_expr = expr_to_rust(&expr, &ctx);
                        ctx.option_value_wrap = false;
                        // Track local assignments AFTER translation so first use gets 'let mut'
                        if let Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) = &expr
                            && !name.contains('.') {
                                ctx.locals.insert(name.clone());
                                // Infer type for local variables so downstream calls
                                // (e.g. `blob_id.detach()`) resolve the receiver type.
                                if let Some(ty) = ty_ann {
                                    ctx.types.local_types.insert(name.clone(), crate::rust::type_to_rust(ty));
                                } else if let Some(t) = crate::expr::infer_expr_type_pub(rhs, &ctx) {
                                    ctx.types.local_types.insert(name.clone(), t);
                                }
                            }
                        if is_last {
                            // GEN-002: lower authored adapter bodies. If the last
                            // expr already returns (`ret Ok` → `return Ok(...)`),
                            // emit it as-is — do not wrap again.
                            let is_return = rust_expr.trim_start().starts_with("return ")
                                || rust_expr.contains("return Ok(")
                                || rust_expr.contains("return Err(");
                            let is_ctrl = rust_expr.trim_start().starts_with("match ")
                                || rust_expr.trim_start().starts_with("if ")
                                || rust_expr.trim_start().starts_with('{');
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
                                } else if rust_expr.contains(".await") && !is_ctrl {
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

    // Local/dev fallback: HashMap adapters for product ports with no authored impl.
    out.push_str(&gen_in_memory_adapters(traits, impls, solution));

    // Product free functions (non-layer) live next to adapters so they can use
    // domain types. Layer free fns stay in veil_shared.
    let product_fns: Vec<&FnDef> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Function(f) if !f.layer_provided => Some(f),
            _ => None,
        })
        .collect();
    if !product_fns.is_empty() {
        let name_to_shape = build_name_to_shape(solution, registry);
        for f in product_fns {
            let mut ctx = build_ctx_from_solution(solution, name_to_shape.clone(), registry);
            for p in &f.params {
                ctx.locals.insert(p.name.clone());
                ctx.types.local_types
                    .insert(p.name.clone(), type_to_rust(&p.type_expr));
            }
            ctx.ownership.mut_locals = crate::expr::analyze_mut_locals(&f.body);
            ctx.ownership.ident_uses = crate::expr::count_ident_uses(&f.body);
            let params = f
                .params
                .iter()
                .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
                .collect::<Vec<_>>()
                .join(", ");
            let ret = match &f.return_type {
                Some(t) => type_to_rust(t),
                None => "()".to_string(),
            };
            ctx.expected_return_rust = Some(ret.clone());
            // Sync helpers unless the body calls layer async free fns.
            let needs_async = f.body.iter().any(|e| expr_calls_async_fn(e, &ctx));
            let async_kw = if needs_async { "async " } else { "" };
            out.push_str(&format!(
                "/// Product free function.\npub {async_kw}fn {}({}) -> {} {{\n",
                to_snake(&f.name),
                params,
                ret,
            ));
            for expr in &f.body {
                out.push_str(&format!(
                    "{}\n",
                    crate::expr::stmt_to_rust(expr, &mut ctx)
                ));
            }
            let ends_in_return = matches!(f.body.last(), Some(Expr::Return(_)));
            if !ends_in_return && ret == "()" {
                out.push_str("    // ok\n");
            }
            out.push_str("}\n\n");
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/adapters/mod.rs", crate_name),
        content: out,
    }
}

pub fn rust_entity_fields<'a>(sol: &'a Solution, entity: &str) -> Vec<&'a Field> {
    fn walk<'a>(c: &'a Construct, entity: &str, out: &mut Vec<&'a Field>) {
        if c.name == entity && !c.fields.is_empty() {
            out.extend(c.fields.iter());
        }
        for b in &c.blocks {
            if c.name == entity {
                out.extend(b.fields.iter());
            }
        }
        for ch in &c.children {
            walk(ch, entity, out);
        }
    }
    let mut out = Vec::new();
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            walk(c, entity, &mut out);
        }
    }
    out
}

pub fn infer_repo_entity(t: &Construct) -> Option<String> {
    for m in &t.methods {
        let name = m.name.trim_end_matches('!');
        if (name == "save" || name == "put" || name == "insert")
            && let Some(p) = m.params.first()
                && let TypeExpr::Named(n) = &p.type_expr
                    && n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                        return Some(n.clone());
                    }
        if let Some(TypeExpr::Optional(inner)) = &m.return_type
            && let TypeExpr::Named(n) = inner.as_ref()
                && n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    return Some(n.clone());
                }
        if let Some(TypeExpr::List(inner)) = &m.return_type
            && let TypeExpr::Named(n) = inner.as_ref()
                && n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    return Some(n.clone());
                }
    }
    None
}

pub fn filter_field_for_method(method: &str, fields: &[&Field]) -> Option<String> {
    let m = method.trim_end_matches('!');
    let mut rest = m
        .strip_prefix("find_")
        .or_else(|| m.strip_prefix("list_"))
        .or_else(|| m.strip_prefix("get_"))
        .unwrap_or(m);
    rest = rest
        .strip_prefix("open_by_")
        .or_else(|| rest.strip_prefix("by_"))
        .unwrap_or(rest);
    if rest.is_empty() || matches!(rest, "find" | "list" | "get" | "save" | "delete" | "all") {
        return None;
    }
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    if names.contains(&rest) {
        return Some(rest.to_string());
    }
    let with_id = format!("{rest}_id");
    if names.iter().any(|n| *n == with_id) {
        return Some(with_id);
    }
    None
}

pub fn gen_in_memory_adapters(
    traits: &[&Construct],
    impls: &[&Construct],
    sol: &Solution,
) -> String {
    let mut out = String::new();
    for t in traits {
        if t.layer_provided {
            continue;
        }
        if impls.iter().any(|i| i.target.as_deref() == Some(&t.name)) {
            continue;
        }
        if t.methods.is_empty() {
            continue;
        }
        let Some(entity) = infer_repo_entity(t) else {
            continue;
        };
        let fields = rust_entity_fields(sol, &entity);
        let name = format!("InMemory{}", t.name);
        out.push_str(&format!(
            "/// Generated in-memory {0} for local smoke / greenfield runs.\n\
             #[derive(Default)]\n\
             pub struct {name} {{\n\
                 rows: tokio::sync::RwLock<std::collections::HashMap<String, {entity}>>,\n\
             }}\n\n\
             impl {name} {{\n\
                 pub fn new() -> Self {{ Self::default() }}\n\
                 fn key(id: impl std::fmt::Display) -> String {{ id.to_string() }}\n\
             }}\n\n\
             #[async_trait]\n\
             impl {trait_name} for {name} {{\n",
            t.name,
            trait_name = t.name,
        ));
        for m in &t.methods {
            let mname = to_snake(m.name.trim_end_matches('!'));
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
            let sep = if params.is_empty() { "" } else { ", " };
            let body = in_memory_method_body(&mname, m, &entity, &fields, &ret);
            out.push_str(&format!(
                "    async fn {mname}(&self{sep}{params}) -> {ret} {{\n{body}    }}\n\n"
            ));
        }
        out.push_str("}\n\n");
    }
    out
}

pub fn in_memory_method_body(
    mname: &str,
    m: &Method,
    entity: &str,
    fields: &[&Field],
    ret: &str,
) -> String {
    let first = m.params.first().map(|p| to_snake(&p.name));
    if matches!(mname, "save" | "put" | "insert")
        && let Some(arg) = first.as_deref() {
            let id_field = if fields.iter().any(|f| f.name == "id") {
                "id"
            } else {
                fields.first().map(|f| f.name.as_str()).unwrap_or("id")
            };
            return format!(
                "        let mut g = self.rows.write().await;\n\
                        g.insert(Self::key({arg}.{id_field}.clone()), {arg});\n\
                        Ok(())\n"
            );
        }
    if matches!(mname, "delete" | "remove")
        && let Some(arg) = first.as_deref() {
            return format!(
                "        self.rows.write().await.remove(&Self::key({arg}.clone()));\n\
                        Ok(())\n"
            );
        }
    if matches!(mname, "find" | "get") && m.params.len() == 1
        && let Some(arg) = first.as_deref()
            && ret.contains("Option<") {
                if arg != "id"
                    && let Some(field) = filter_field_for_method(&format!("by_{arg}"), fields)
                        .or_else(|| Some(arg.to_string()))
                    {
                        let field_s = to_snake(&field);
                        return format!(
                            "        Ok(self.rows.read().await.values().find(|e| e.{field_s} == {arg}).cloned())\n"
                        );
                    }
                return format!(
                    "        Ok(self.rows.read().await.get(&Self::key({arg}.clone())).cloned())\n"
                );
            }
    if matches!(mname, "list" | "list_all" | "all") && m.params.is_empty() {
        return "        Ok(self.rows.read().await.values().cloned().collect())\n".into();
    }
    if let Some(field) = filter_field_for_method(mname, fields)
        && let Some(arg) = first.as_deref() {
            let field_s = to_snake(&field);
            let open = mname.contains("open");
            let extra = if open && fields.iter().any(|f| f.name == "returned") {
                " && !e.returned"
            } else if open && fields.iter().any(|f| f.name == "available") {
                " && e.available"
            } else {
                ""
            };
            if ret.contains("Option<") {
                return format!(
                    "        Ok(self.rows.read().await.values().find(|e| e.{field_s} == {arg}{extra}).cloned())\n"
                );
            }
            if ret.contains("Vec<") {
                return format!(
                    "        Ok(self.rows.read().await.values().filter(|e| e.{field_s} == {arg}{extra}).cloned().collect())\n"
                );
            }
        }
    if ret.contains("Option<") {
        return "        Ok(None)\n".into();
    }
    if ret.contains("Vec<") {
        return "        Ok(Vec::new())\n".into();
    }
    if ret.contains("Result<(),") {
        return "        Ok(())\n".into();
    }
    format!("        compile_error!(\"implement in-memory {entity}.{mname}\")\n")
}

/// Recursively check if an expression contains a Return (ret) at any depth
/// (including inside match arms, if bodies, etc.).
pub fn expr_contains_return(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::Match(_, arms) => arms.iter().any(|a| a.body.iter().any(expr_contains_return)),
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(expr_contains_return)
                || ie.else_body.as_ref().map(|b| b.iter().any(expr_contains_return)).unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } | Expr::Loop(body) => {
            body.iter().any(expr_contains_return)
        }
        _ => false,
    }
}

/// True if any call in `expr` targets a known async free function.
pub fn expr_calls_async_fn(expr: &Expr, ctx: &crate::expr::GenCtx) -> bool {
    match expr {
        Expr::Call(c) if c.method.is_empty() && ctx.async_fns.contains(&c.target) => true,
        Expr::Call(c) => {
            c.args.iter().any(|a| expr_calls_async_fn(a, ctx))
                || c.receiver
                    .as_ref()
                    .map(|r| expr_calls_async_fn(r, ctx))
                    .unwrap_or(false)
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) | Expr::Return(rhs) => {
            expr_calls_async_fn(rhs, ctx)
        }
        Expr::IfExpr(ie) => {
            expr_calls_async_fn(&ie.condition, ctx)
                || ie.then_body.iter().any(|e| expr_calls_async_fn(e, ctx))
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(|e| expr_calls_async_fn(e, ctx)))
                    .unwrap_or(false)
        }
        Expr::WhileLoop { condition, body } => {
            expr_calls_async_fn(condition, ctx) || body.iter().any(|e| expr_calls_async_fn(e, ctx))
        }
        Expr::ForLoop { iterable, body, .. } => {
            expr_calls_async_fn(iterable, ctx) || body.iter().any(|e| expr_calls_async_fn(e, ctx))
        }
        Expr::BinaryOp(b) => {
            expr_calls_async_fn(&b.left, ctx) || expr_calls_async_fn(&b.right, ctx)
        }
        Expr::FieldAccess(base, _) | Expr::Try(base) | Expr::Require(base) | Expr::Await(base) => {
            expr_calls_async_fn(base, ctx)
        }
        _ => false,
    }
}

/// Pure generic adapter template: `adapter Foo<T> for Trait<T>` (or unbound
/// `adapter Foo<T> for Trait`). Used only as monomorphization source in VEIL;
/// not emitted as Rust.
pub fn is_pure_generic_adapter_template(c: &Construct) -> bool {
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
pub fn find_generic_adapter_template<'a>(
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
pub fn monomorphize_type(ty: &TypeExpr, adapter: &Construct, trait_: &Construct) -> TypeExpr {
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
            })
                && let Some(arg) = adapter.target_type_args.get(idx) {
                    return arg.clone();
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
pub fn monomorphize_expr(expr: &Expr, adapter: &Construct, trait_: &Construct) -> Expr {
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

pub fn rename_type_expr(
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

pub fn monomorphize_expr_with(
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
            a.body = a
                .body
                .iter()
                .map(|x| monomorphize_expr_with(renames, x))
                .collect();
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

