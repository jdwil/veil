use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;

/// Render the inline-table body for a git dependency from a `git:` scheme value.
///
/// Accepts `<url>` or `<url>#<key>=<val>` where key is `branch`, `rev`, or
/// `tag`. Bare URLs pin to the repo default branch. Used for stub crates that
/// live in a git repo but are not published to crates.io (e.g. `veil-jwks`).
/// Mirrors the `path:` convention so the engine stays generic.
pub fn git_dep_fields(spec: &str) -> String {
    let (url, refspec) = match spec.split_once('#') {
        Some((u, r)) => (u.trim(), Some(r.trim())),
        None => (spec.trim(), None),
    };
    match refspec.and_then(|r| r.split_once('=')) {
        Some((k, v)) if matches!(k.trim(), "branch" | "rev" | "tag") => {
            format!("git = \"{url}\", {} = \"{}\"", k.trim(), v.trim())
        }
        // Unqualified ref after `#` is treated as a branch for convenience.
        _ => match refspec {
            Some(r) if !r.is_empty() => format!("git = \"{url}\", branch = \"{r}\""),
            _ => format!("git = \"{url}\""),
        },
    }
}

/// Render a companion `cargo_deps` entry. Supports the same `path:`/`git:`
/// schemes as a stub's own version line, defaulting to a crates.io version.
pub fn companion_dep_line(dep_name: &str, dep_ver: &str) -> String {
    if let Some(rel) = dep_ver.strip_prefix("path:") {
        format!("{dep_name} = {{ path = \"{rel}\" }}")
    } else if let Some(git_spec) = dep_ver.strip_prefix("git:") {
        format!("{dep_name} = {{ {} }}", git_dep_fields(git_spec))
    } else {
        format!("{dep_name} = \"{dep_ver}\"")
    }
}

/// Default for non-client adapter fields (`table`, `bucket`, `dir`, plain Str).
pub fn harness_string_field_default(fname: &str, ftype: &str) -> String {
    let f = fname.to_ascii_lowercase();
    let ty = ftype.trim();
    let is_stringish = matches!(ty, "Str" | "String" | "str" | "")
        || ty.ends_with("String");
    if !is_stringish && ty != "Str" {
        // Non-string typed fields without a stub harness still need *something*.
        if f == "table" || f == "bucket" || f == "dir" {
            // fall through to string env defaults
        } else {
            return "Default::default()".to_string();
        }
    }
    match f.as_str() {
        "table" => {
            "std::env::var(\"VEIL_DDB_TABLE\")\
             .or_else(|_| std::env::var(\"TABLE\"))\
             .unwrap_or_else(|_| \"veil\".into())"
                .to_string()
        }
        "bucket" => {
            "std::env::var(\"VEIL_S3_BUCKET\")\
             .or_else(|_| std::env::var(\"BUCKET\"))\
             .unwrap_or_else(|_| \"veil\".into())"
                .to_string()
        }
        "dir" => {
            "std::env::var(\"VEIL_EXTENSIONS_DIR\")\
             .unwrap_or_else(|_| \".veil/extensions\".into())"
                .to_string()
        }
        _ => format!(
            "std::env::var(\"{}\").unwrap_or_else(|_| \"default\".into())",
            fname.to_ascii_uppercase()
        ),
    }
}



/// RT-001b: dedicated binary crate for `@main` / composition root.
pub fn gen_bin_crate(
    sol: &Solution,
    module_crates: &[String],
    main_body: &str,
    links: &[crate::links::ResolvedLink],
    registry: &LayerRegistry,
) -> Vec<GeneratedFile> {
    let mut deps = String::from(
        "tokio = { workspace = true }\nuuid = { workspace = true }\nserde = { workspace = true }\nserde_json = { workspace = true }\nveil_shared = { path = \"../veil_shared\" }\n",
    );
    // Framework-specific bin deps from layer (e.g. axum, tower-http).
    if let Some(cargo_deps) = registry.harness_render_templates.get("rust_bin_cargo") {
        for line in cargo_deps.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                deps.push_str(trimmed);
                deps.push('\n');
            }
        }
    }
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
    // Use statements: shared crate import from layer template or default.
    let shared_import = registry.harness_render_templates.get("shared_imports")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "use veil_shared::*;".to_string());
    let mut uses = format!("{shared_import}\n");
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
    let main_rs = if main_body.contains("#[tokio::") || main_body.contains("fn main") {
        main_body.to_string()
    } else {
        // Layer-provided async main wrapper
        let wrapper = registry.harness_render_templates.get("rust_bin_main_wrapper")
            .map(|t| t.as_str())
            .unwrap_or("fn main() {\n{body}\n}\n");
        format!(
            "//! Generated entrypoint for package `{}` (@main contributors).\n\
             //! Run: `cargo run -p veil_bin` from the generated workspace root.\n\
             {uses}\n\
             {}\n",
            sol.name,
            wrapper.replace("{body}", main_body)
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

pub fn gen_workspace_toml(
    sol: &Solution,
    registry: &LayerRegistry,
    links: &[crate::links::ResolvedLink],
) -> GeneratedFile {
    let mut members = vec!["    \"crates/veil_shared\"".to_string()];
    // veil_shared is included because layers provide content for it (shared_emit,
    // declare blocks, traits). If this list were empty, the crate wouldn't be needed.
    // Future: skip when no layer contributes content.
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item
            && c.shape == Shape::Mod {
                members.push(format!("    \"crates/{}\"", module_crate_name(c, sol)));
            }
    }

    // GEN-006: deps/features from stub metadata only (no engine hardcode).
    // Emit every stub the package loaded via `use` plus cargo_deps companions
    // (e.g. aws-config for aws-sdk-dynamodb) so veil_bin workspace=true resolves.
    // Use BTreeMap keyed by crate name to prevent duplicate entries (Issue 7).
    // Never re-emit keys already in [workspace.dependencies] (serde_json stub → "duplicate key").
    const WORKSPACE_DEP_KEYS: &[&str] = &[
        "tokio",
        "async-trait",
        "thiserror",
        "serde",
        "uuid",
        "chrono",
        "tracing",
        "serde_json",
    ];
    let mut dep_map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for stub in &registry.stubs {
        if !stub_is_active_cargo(stub) {
            continue;
        }
        // Path stubs: version line `path:../relative` (local platform crates, not crates.io).
        // Keeps filesystem/SDK details out of the engine; the .stub still declares the API.
        let dep_line = if let Some(rel) = stub.version.strip_prefix("path:") {
            format!("{} = {{ path = \"{}\" }}", stub.name, rel)
        } else if let Some(git_spec) = stub.version.strip_prefix("git:") {
            // Git stubs: version line `git:<url>` or `git:<url>#<key>=<val>`
            // where key is one of branch|rev|tag (default branch when omitted).
            // For crates that live in a git repo but aren't published to crates.io
            // (e.g. veil-jwks in the engine repo). Mirrors the `path:` convention.
            format!("{} = {{ {} }}", stub.name, git_dep_fields(git_spec))
        } else if stub.cargo_features.is_empty() {
            format!("{} = \"{}\"", stub.name, stub.version)
        } else {
            let feats: Vec<String> = stub
                .cargo_features
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect();
            format!(
                "{} = {{ version = \"{}\", features = [{}] }}",
                stub.name,
                stub.version,
                feats.join(", ")
            )
        };
        // Direct stubs take priority over companion deps (more specific version).
        if !WORKSPACE_DEP_KEYS.contains(&stub.name.as_str()) {
            dep_map.insert(stub.name.clone(), dep_line);
        }

        // Companion crates declared on the stub (e.g. aws-config for dynamodb).
        for (dep_name, dep_ver) in &stub.cargo_deps {
            if WORKSPACE_DEP_KEYS.contains(&dep_name.as_str()) {
                continue;
            }
            dep_map.entry(dep_name.clone())
                .or_insert_with(|| companion_dep_line(dep_name, dep_ver));
        }
    }
    // CAP-001: path deps from `link` declarations.
    for link in links {
        let line = crate::links::cargo_dep_line(link);
        if let Some(name) = line.split('=').next() {
            dep_map.entry(name.trim().to_string())
                .or_insert(line.trim_end().to_string());
        }
    }
    // Linked VEIL projects need libloading for .so loading.
    if sol.links.iter().any(|l| l.is_project_link) {
        dep_map.entry("libloading".to_string())
            .or_insert_with(|| "libloading = \"0.8\"".to_string());
    }
    let extra_deps: String = dep_map.values().map(|v| format!("{v}\n")).collect();

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

/// Compute a unique crate name for a Mod-shaped construct. When multiple modules
/// share the same snake_case name (e.g. `ctx Deploy` and `svc Deploy`), prefix
/// with the keyword to disambiguate.
pub fn module_crate_name(module: &Construct, solution: &Solution) -> String {
    let base = to_snake(&module.name);
    let collision = solution.items.iter().any(|i| {
        if let TopLevelItem::Construct(c) = i {
            // Another top-level Shape::Mod whose snake_case name collides with
            // ours (same base name from a different keyword OR different
            // PascalCase that happens to produce the same snake_case).
            c.shape == Shape::Mod
                && to_snake(&c.name) == base
                && !std::ptr::eq(c, module)
        } else {
            false
        }
    });
    if collision {
        format!("{}_{}", to_snake(&module.keyword), base)
    } else {
        base
    }
}

pub fn gen_module_crate(
    module: &Construct,
    all_impls: &[&Construct],
    top_level_flows: &[&Flow],
    flow_generated: &mut bool,
    solution: &Solution,
    registry: &LayerRegistry,
    links: &[crate::links::ResolvedLink],
    harness_ir: &veil_ir::HarnessIR,
    layer_derives: Option<&str>,
    layer_trait_attrs: Option<&str>,
    layer_fn_attrs: Option<&str>,
    template_output: &crate::template::TemplateOutput,
) -> Vec<GeneratedFile> {
    let crate_name = module_crate_name(module, solution);
    let mut files = Vec::new();
    let mut contents = flatten_module(module, registry);

    // Cross-context sibling dependencies (orchestrators referencing other contexts).
    let sibling_crates = crate::rust::application::detect_sibling_refs(module, solution);

    // Solution-level layer-provided traits live in the shared crate and are
    // re-exported by gen_traits — do NOT duplicate them here. A product
    // construct that reuses a declared name is emitted locally; gen_traits
    // then avoids `pub use veil_shared::*` so the names do not collide.
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item
            && c.shape == Shape::Trait && !c.layer_provided {
                contents.traits.push(c);
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
            // Exception: orchestrators that directly reference sibling context types
            // (via `contexts X, Y` or step-level `ctx X` refs) need path deps.
            cargo.push('\n');
            cargo.push_str("chrono.workspace = true\ntracing.workspace = true\nserde_json.workspace = true\n");
            // Shared error types + Bus trait, defined once.
            cargo.push_str("veil_shared = { path = \"../veil_shared\" }\n");
            for sibling in &sibling_crates {
                cargo.push_str(&format!("{} = {{ path = \"../{}\" }}\n", sibling, sibling));
            }
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

    let crate_tests = crate::testing::generate_crate_tests(
        solution,
        registry,
        &crate_name,
        module,
        &contents.traits,
        &contents.fns,
        &contents.enums,
        &contents.structs,
    );
    let mut lib_rs = format!(
        "//! {} — {}.\n\npub mod domain;\npub mod ports;\npub mod adapters;\npub mod application;\n",
        module.name, module.subkind
    );
    if crate_tests.is_some() {
        lib_rs.push_str("\n#[cfg(test)]\nmod tests;\n");
    }
    files.push(GeneratedFile {
        path: format!("crates/{}/src/lib.rs", crate_name),
        content: lib_rs,
    });
    if let Some(tests) = crate_tests {
        files.push(tests);
    }

    files.push(gen_types(&contents, &crate_name, registry, solution, layer_derives, &sibling_crates, template_output));
    files.push(gen_child_types(&contents, &crate_name));
    files.push(GeneratedFile {
        path: format!("crates/{}/src/domain/mod.rs", crate_name),
        content: "pub mod types;\npub mod messages;\n".to_string(),
    });

    // For modules that reference siblings, re-export ports from the first sibling
    // instead of generating duplicate DomainError / shared traits.
    files.push(gen_traits(&contents, &crate_name, solution, registry, layer_trait_attrs, template_output));

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
        if let TopLevelItem::Construct(c) = item
            && c.shape == Shape::Trait
                && c.layer_provided
                && !traits_for_impls.iter().any(|t| t.name == c.name)
            {
                traits_for_impls.push(c);
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
    let deps_decl = harness_ctx(harness_ir, &crate_name, &module.name)
        .and_then(|c| c.deps.clone());
    files.push(gen_application(
        &app_flows,
        &contents,
        &crate_name,
        solution,
        registry,
        deps_decl.as_ref(),
        layer_fn_attrs,
        template_output,
    ));

    // Generate manifest.json only for deployment units (constructs marked with `au`)
    if module.deployment_unit {
        files.push(gen_manifest(
            module,
            &contents,
            &impls_for_module,
            &crate_name,
            solution,
            registry,
        ));
    }

    files
}


/// Generate a manifest.json describing the module's wiring requirements.
/// The runtime reads this to construct Deps and register Bus handlers.
pub fn gen_manifest(
    module: &Construct,
    contents: &ModuleContents,
    impls: &[&Construct],
    crate_name: &str,
    solution: &Solution,
    registry: &LayerRegistry,
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
                .filter(|a| registry.is_adapter_env_annotation(&a.name))
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
        if let Some(strategy_ann) = t.annotations.iter().find(|a| registry.is_runtime_strategy_annotation(&a.name))
            && let Some(strategy_value) = strategy_ann.args.first() {
                dep_info.insert("strategy".to_string(), json!(strategy_value));
            }

        deps.insert(dep_name, serde_json::Value::Object(dep_info));
    }

    // Bus handler map: all fn-shaped constructs; message key via layer bus_policy.
    let mut handlers = serde_json::Map::new();
    for f in &contents.fns {
        let fn_name = to_snake(&f.name);
        let message_name = registry.bus_message_name(&f.name);
        handlers.insert(
            message_name,
            json!({
                "function": fn_name,
                "inputs": f.inputs.iter().map(|i| {
                    json!({ "name": i.name, "type": format!("{:?}", i.type_expr) })
                }).collect::<Vec<_>>(),
            }),
        );
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
