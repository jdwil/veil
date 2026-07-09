//! Rust code generation from VEIL AST.
//!
//! Fully shape-driven: constructs are generated according to their core
//! shape (`mod` → crate, `struct`/`enum` → types, `trait` → async traits,
//! `impl` → adapter structs, `fn` → orchestrator functions). The construct's
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

    files.push(gen_workspace_toml(solution, registry));

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
    files.extend(gen_shared_crate(&shared_traits, &shared_structs, &shared_fns, solution, registry));

    // Each top-level mod-shaped construct becomes a crate.
    let modules: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(c),
            _ => None,
        })
        .collect();

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
        ));
    }

    // ─── Layer Template Augmentation ─────────────────────────────────────
    // Execute any codegen templates from loaded layers (di.layer, rust.layer, etc.)
    // Template output augments the backend's output — it doesn't replace it.
    let template_output = crate::template::execute_templates(solution, registry, "rust");

    // If there's a composed "main" section from @main contributors, add it
    if let Some(main_content) = crate::template::compose_main_section(&template_output, "rust") {
        files.push(GeneratedFile {
            path: "src/main.rs".to_string(),
            content: main_content,
        });
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

fn gen_workspace_toml(sol: &Solution, registry: &LayerRegistry) -> GeneratedFile {
    let mut members = vec!["    \"crates/veil_shared\"".to_string()];
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Mod {
                members.push(format!("    \"crates/{}\"", to_snake(&c.name)));
            }
        }
    }

    let mut extra_deps = String::new();
    for stub in &registry.stubs {
        if stub.name == "sqlx" {
            // sqlx needs runtime and driver features
            extra_deps.push_str(&format!(
                "sqlx = {{ version = \"{}\", features = [\"runtime-tokio\", \"postgres\", \"uuid\", \"chrono\", \"json\"] }}\n",
                stub.version
            ));
        } else {
            extra_deps.push_str(&format!(
                "{} = \"{}\"\n", stub.name, stub.version
            ));
        }
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
            // Stub crate dependencies
            for stub in &registry.stubs {
                cargo.push_str(&format!("{}.workspace = true\n", stub.name));
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

    files.push(gen_types(&contents, &crate_name));
    files.push(gen_child_types(&contents, &crate_name));
    files.push(GeneratedFile {
        path: format!("crates/{}/src/domain/mod.rs", crate_name),
        content: "pub mod types;\npub mod messages;\n".to_string(),
    });

    // For modules that reference siblings (orchestrators), re-export ports from the first sibling
    // instead of generating duplicate DomainError/Bus/etc.
    files.push(gen_traits(&contents, &crate_name));

    // Impls targeting traits defined in this module (from anywhere in the tree).
    let trait_names: Vec<&str> = contents.traits.iter().map(|t| t.name.as_str()).collect();
    let impls_for_module: Vec<&Construct> = all_impls
        .iter()
        .filter(|i| {
            i.target
                .as_deref()
                .map(|t| trait_names.contains(&t))
                .unwrap_or(false)
        })
        .copied()
        .collect();
    files.push(gen_impls(&impls_for_module, &contents.traits, &crate_name, solution, registry));

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

fn gen_types(contents: &ModuleContents, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Domain types.\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\nuse crate::ports::{ValidationError, DomainError};\nuse crate::domain::messages::*;\n\n");

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
    let undefined: Vec<String> = referenced
        .iter()
        .filter(|t| !defined_types.contains(t) && !builtin.contains(&t.as_str()))
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

    for c in &contents.structs {
        out.push_str(&gen_struct(c));
    }
    for e in &contents.enums {
        out.push_str(&gen_enum(e));
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/types.rs", crate_name),
        content: out,
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

/// Generate a struct-shaped construct: struct + enum blocks + invariant impl.
fn gen_struct(c: &Construct) -> String {
    let mut out = String::new();
    let has_invariant = c.annotations.iter().any(|a| a.name == "invariant");

    // Fields: direct plus struct-shaped named blocks (e.g. root).
    let mut fields: Vec<&Field> = c.fields.iter().collect();
    for block in &c.blocks {
        if block.shape != Shape::Enum {
            fields.extend(block.fields.iter());
        }
    }

    out.push_str(&format!(
        "/// {}: {}\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub struct {}{} {{\n",
        c.subkind, c.name, c.name, generic_params_rust(&c.type_params)
    ));
    for field in &fields {
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake(&field.name),
            type_to_rust(&field.type_expr)
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

    if has_invariant {
        out.push_str(&format!(
            "impl {} {{\n    pub fn new({}) -> Result<Self, ValidationError> {{\n        let value = Self {{ {} }};\n        value.validate()?;\n        Ok(value)\n    }}\n\n    pub fn validate(&self) -> Result<(), ValidationError> {{\n        Ok(())\n    }}\n}}\n\n",
            c.name,
            fields
                .iter()
                .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
                .collect::<Vec<_>>()
                .join(", "),
            fields
                .iter()
                .map(|f| to_snake(&f.name))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    } else if !fields.is_empty() {
        // Generate a smart constructor — auto-defaulting id, timestamps, and enum-state fields
        let auto_fields = ["id", "created", "updated", "created_at", "updated_at", "created_on", "updated_on", "deleted_on", "date_joined"];

        // id is special: it's auto-generated in the constructor body (Uuid::new_v4())
        // but if explicitly passed by caller, it's a parameter too.
        // For entities/aggregates, we generate id internally — callers pass it separately.
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

        // Fields with scalar defaults (Int→0, Bool→false, F64→0.0, Json→{}) are auto-initialized
        let scalar_default_fields: std::collections::HashSet<String> = fields.iter()
            .filter(|f| matches!(&f.type_expr, TypeExpr::Named(n) if n == "Int" || n == "Bool" || n == "F64" || n == "Json"))
            .map(|f| f.name.clone())
            .collect();

        let user_fields: Vec<&&Field> = fields.iter()
            .filter(|f| {
                !auto_fields.contains(&f.name.as_str())
                && !enum_field_names.contains(&f.name)
                && !scalar_default_fields.contains(&f.name)
                // Optional fields default to None — exclude from constructor params
                && !matches!(&f.type_expr, TypeExpr::Optional(_))
                && !matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option")
            })
            .collect();

        let params_str = user_fields.iter()
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>().join(", ");

        let init_fields = fields.iter().map(|f| {
            let snake = to_snake(&f.name);
            if f.name == "id" {
                format!("{}: Uuid::new_v4()", snake)
            } else if auto_fields.contains(&f.name.as_str()) {
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
                // Scalar defaults: Int→0, Bool→false, F64→0.0, Json→{}
                let default = match &f.type_expr {
                    TypeExpr::Named(n) if n == "Bool" => "false",
                    TypeExpr::Named(n) if n == "F64" => "0.0",
                    TypeExpr::Named(n) if n == "Json" => "serde_json::json!({})",
                    _ => "0", // Int and anything else numeric
                };
                format!("{}: {}", snake, default)
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
    }

    // Generate impl block with business logic fns (if any exist).
    if !c.fns.is_empty() {
        out.push_str(&gen_aggregate_impl(c, &fields));
    }

    out
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

        // Determine the return type: if the function has an explicit return type,
        // use it. Otherwise default to Vec<Events> for event-collecting methods.
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

        out.push_str(&format!(
            "    pub fn {}(&mut self, {}) -> {} {{\n",
            to_snake(&func.name), params_str, return_type_str
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

        out.push_str(&format!("        let mut events: Vec<{}> = Vec::new();\n", event_enum_name));

        // Build context for body translation
        let mut ctx = GenCtx::new(HashMap::new());
        ctx.in_aggregate_fn = true;
        ctx.self_fields = field_names.clone();
        // Register params as locals
        for p in &func.params {
            ctx.locals.insert(p.name.clone());
        }

        let mut has_explicit_ret = false;
        for expr in &func.body {
            match expr {
                Expr::Assign(field, rhs) | Expr::MutAssign(field, rhs, _) if field_names.contains(field) => {
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
    out.push_str(&format!(
        "/// {}: {}\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub enum {}{} {{\n",
        c.subkind, c.name, c.name, generic_params_rust(&c.type_params)
    ));

    // Use rich_variants if available, otherwise fall back to flat string variants
    if !c.rich_variants.is_empty() {
        for v in &c.rich_variants {
            match v {
                EnumVariant::Unit(name) => out.push_str(&format!("    {},\n", name)),
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
        for v in &c.variants {
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

/// Generate the shared library crate that all context crates depend on. It
/// owns the common error types and the layer-provided top-level traits (Bus),
/// so there is exactly one definition of each across the workspace.
fn gen_shared_crate(
    traits: &[&Construct],
    structs: &[&Construct],
    functions: &[&FnDef],
    solution: &Solution,
    registry: &LayerRegistry,
) -> Vec<GeneratedFile> {
    use crate::expr::{build_ctx_from_solution, stmt_to_rust};
    let mut files = Vec::new();

    files.push(GeneratedFile {
        path: "crates/veil_shared/Cargo.toml".to_string(),
        content: r#"[package]
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
"#.to_string(),
    });

    let mut lib = String::new();
    lib.push_str("//! Shared types across all context crates — common errors and\n");
    lib.push_str("//! layer-provided infrastructure traits (the message Bus).\n\n");
    lib.push_str("#![allow(unused_imports)]\n\n");
    lib.push_str("use async_trait::async_trait;\nuse uuid::Uuid;\n\n");
    lib.push_str("/// Domain error type.\n#[derive(Debug, thiserror::Error)]\npub enum DomainError {\n");
    lib.push_str("    #[error(\"Not found\")]\n    NotFound,\n");
    lib.push_str("    #[error(\"Validation failed: {0}\")]\n    Validation(String),\n");
    lib.push_str("    #[error(\"External service error: {0}\")]\n    External(String),\n");
    lib.push_str("}\n\n");
    lib.push_str("/// Validation error type.\n#[derive(Debug, thiserror::Error)]\n#[error(\"Validation error: {0}\")]\npub struct ValidationError(pub String);\n\n");

    // Trait names in scope — used to box value-position references (List<Trait>).
    let trait_names: std::collections::HashSet<String> =
        traits.iter().map(|t| t.name.clone()).collect();

    for t in traits {
        lib.push_str(&format!("/// {}: {}\n#[async_trait]\npub trait {}: Send + Sync {{\n", t.subkind, t.name, t.name));
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

fn gen_traits(contents: &ModuleContents, crate_name: &str) -> GeneratedFile {
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
        out.push_str(&format!("/// {}: {}\n#[async_trait]\npub trait {}{} {{\n", t.subkind, t.name, t.name, generic_params_rust(&t.type_params)));
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
            out.push_str(&format!(
                "    async fn {}(&self, {}){ret};\n",
                to_snake(&method.name),
                params
            ));
        }
        out.push_str("}\n\n");
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
    use crate::expr::{build_ctx_from_solution, expr_to_rust, stmt_to_rust, GenCtx};

    let mut out = String::new();
    out.push_str("//! Implementations of traits.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables, dead_code)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse crate::ports::*;\nuse crate::domain::types::*;\nuse std::collections::HashMap;\nuse uuid::Uuid;\nuse chrono::Utc;\n");

    // Add sqlx import if any adapter uses DATABASE_URL (i.e., is a sqlx adapter)
    let uses_sqlx = impls.iter().any(|c| c.annotations.iter().any(|a| {
        a.name == "env" && a.args.iter().any(|arg| arg.contains("DATABASE"))
    }));
    if uses_sqlx {
        out.push_str("use sqlx::PgPool;\n");
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
            let target = c.target.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "/// {}: {} (implements {})\npub struct {} {{\n",
                c.subkind, c.name, target, c.name
            ));
            for ann in &c.annotations {
                if ann.name == "env" {
                    for arg in &ann.args {
                        if arg.contains("DATABASE") {
                            // DATABASE_URL → pool: PgPool (the adapter holds a connection pool)
                            out.push_str("    pub pool: PgPool,\n");
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
            out.push_str("}\n\n");

            // Look up the target trait to recover real method signatures
            // (the impl only carries bare parameter names).
            let target_trait = traits.iter().find(|t| t.name == target);

            out.push_str(&format!("#[async_trait]\nimpl {} for {} {{\n", target, c.name));

            for mimpl in &c.impls {
                // Find the trait method to get typed params + return type.
                let trait_method = target_trait
                    .and_then(|t| t.methods.iter().find(|m| m.name == mimpl.method_name));

                // Build the signature: prefer the trait's typed params, zipping
                // the impl's bare names by position; fall back to the impl names.
                let (sig_params, ret_rust) = match trait_method {
                    Some(m) => {
                        let params = m.params.iter()
                            .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
                            .collect::<Vec<_>>().join(", ");
                        let ret = m.return_type.as_ref()
                            .map(type_to_rust)
                            .unwrap_or_else(|| "Result<(), DomainError>".to_string());
                        (params, ret)
                    }
                    None => {
                        // No trait match — use the impl's bare names, untyped.
                        let params = mimpl.params.iter()
                            .map(|p| format!("{}: ()", to_snake(p)))
                            .collect::<Vec<_>>().join(", ");
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
                ctx.in_aggregate_fn = true;
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

                for (i, expr) in mimpl.body.iter().enumerate() {
                    let is_last = i == mimpl.body.len() - 1;
                    let rust_expr = expr_to_rust(expr, &ctx);
                    // Track local assignments AFTER translation so first use gets 'let mut'
                    if let Expr::Assign(name, _) | Expr::MutAssign(name, _, _) = expr {
                        ctx.locals.insert(name.clone());
                    }
                    if is_last {
                        // Last expression is the return value
                        if ret_rust == "Result<(), DomainError>" {
                            // Void result — execute the expression, then Ok(())
                            // Detect if the expression involves an external SDK/stub call
                            let uses_stub = ctx.stub_type_crate.values()
                                .any(|(crate_name, _)| rust_expr.contains(crate_name.as_str()));
                            let uses_effect = hooks.iter().any(|(h, _)| rust_expr.contains(h.as_str()));
                            if uses_stub {
                                // Adapter calls external SDK (sqlx, etc.) — emit todo!
                                // to avoid type mismatches. The SQL intent is in manifest.json.
                                let sql_hint = mimpl.body.iter().find_map(|e| {
                                    if let Expr::Call(c) = e { c.args.first().and_then(|a| {
                                        if let Expr::StringLit(s) = a { Some(s.clone()) } else { None }
                                    }) } else { None }
                                }).unwrap_or_default();
                                out.push_str(&format!("        todo!(\"SQL: {}\")\n", sql_hint.replace('"', "'")));
                            } else if uses_effect {
                                out.push_str(&format!("        {};\\n", rust_expr));
                                out.push_str("        Ok(())\n");
                            } else {
                                out.push_str(&format!("        {};\\n", rust_expr));
                                out.push_str("        Ok(())\n");
                            }
                        } else if ret_rust.starts_with("Result<") {
                            // Non-void result with possible SDK call
                            let uses_stub = ctx.stub_type_crate.values()
                                .any(|(crate_name, _)| rust_expr.contains(crate_name.as_str()));
                            let uses_effect = hooks.iter().any(|(h, _)| rust_expr.contains(h.as_str()));
                            if uses_stub || uses_effect {
                                let sql_hint = mimpl.body.iter().find_map(|e| {
                                    if let Expr::Call(c) = e { c.args.first().and_then(|a| {
                                        if let Expr::StringLit(s) = a { Some(s.clone()) } else { None }
                                    }) } else { None }
                                }).unwrap_or_default();
                                out.push_str(&format!("        todo!(\"SQL: {}\")\n", sql_hint.replace('"', "'")));
                            } else {
                                out.push_str(&format!("        Ok({})\n", rust_expr));
                            }
                        } else {
                            out.push_str(&format!("        {}\n", rust_expr));
                        }
                    } else {
                        out.push_str(&format!("        {};\n", rust_expr));
                    }
                }
                if mimpl.body.is_empty() {
                    out.push_str(&format!("        {}\n", default_ok_for(&ret_rust)));
                }
                out.push_str("    }\n\n");
            }

            // A trait impl must cover ALL trait methods. Emit default stubs for
            // any method the adapter did not implement, so the code compiles.
            if let Some(t) = target_trait {
                let implemented: std::collections::HashSet<&str> =
                    c.impls.iter().map(|m| m.method_name.as_str()).collect();
                for m in &t.methods {
                    if implemented.contains(m.name.as_str()) {
                        continue;
                    }
                    let params = m.params.iter()
                        .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
                        .collect::<Vec<_>>().join(", ");
                    let ret = m.return_type.as_ref()
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
        if let TopLevelItem::Construct(c) = item {
            index(c, &mut map);
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
        Expr::Assign(_, rhs) | Expr::MutAssign(_, rhs, _) | Expr::Return(rhs) | Expr::Await(rhs) => {
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

/// Something that generates an orchestrator function — either a core `flow`
/// or an fn-shaped layer construct (service, saga, ...).
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
    is_orchestrator: bool,
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
                if let Expr::Assign(name, rhs) | Expr::MutAssign(name, rhs, _) = expr {
                    ctx.locals.insert(name.clone());
                    if is_orchestrator {
                        // Orchestrator locals are JSON Bus results.
                        ctx.local_types.insert(name.clone(), "serde_json::Value".to_string());
                    } else if let Some(t) = crate::expr::infer_expr_type_pub(rhs, &ctx) {
                        ctx.local_types.insert(name.clone(), t);
                    }
                }
            }
        }
    }

    let inner = crate::expr::infer_return_expr_type(ret, &ctx);
    match inner {
        Some(t) if !t.is_empty() && t != "()" => format!("Result<{}, DomainError>", t),
        // Fallback: handler returns data but we can't infer the exact type.
        // Use serde_json::Value which works for any serializable return.
        _ => {
            // Check if the ret expression is a non-trivial ident (not a keyword)
            if matches!(ret, Expr::Ident(n) if n != "Ok" && n != "Err" && !n.is_empty()) {
                "Result<serde_json::Value, DomainError>".to_string()
            } else {
                "Result<(), DomainError>".to_string()
            }
        }
    }
}

fn gen_application(flows: &[FlowLike], module_contents: &ModuleContents, crate_name: &str, solution: &Solution, registry: &LayerRegistry) -> GeneratedFile {
    use crate::expr::{build_ctx_from_solution, collect_deps, gen_deps_struct, stmt_to_rust, expr_to_rust};
    use std::collections::HashMap;

    let mut out = String::new();
    out.push_str("//! Application services and flow orchestrators.\n\n");
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

    // Detect if this module is an orchestrator (steps have ctx refs = cross-context calls)
    let is_orchestrator = flows.iter().any(|flow| {
        let steps = match flow {
            FlowLike::Flow(f) => &f.steps,
            FlowLike::Construct(c) => &c.steps,
        };
        steps.iter().any(|s| {
            if let FlowStep::Step(sd) = s { !sd.refs.is_empty() } else { false }
        })
    });

    // For orchestrators, only routing traits (e.g. Bus) are direct deps — all
    // other calls go through the message bus.
    let mut effective_name_to_shape = name_to_shape.clone();
    if is_orchestrator {
        let routing = registry.routing_traits();
        // Remove all non-routing traits from the shape map so they don't become direct deps
        effective_name_to_shape.retain(|name, shape| {
            *shape != Shape::Trait || routing.contains(name)
        });
    }

    // Collect all deps across all flows
    let base_ctx = build_ctx_from_solution(solution, effective_name_to_shape.clone(), registry);
    let mut all_deps = std::collections::HashSet::new();
    for flow in flows {
        let steps = match flow {
            FlowLike::Flow(f) => &f.steps,
            FlowLike::Construct(c) => &c.steps,
        };
        all_deps.extend(collect_deps(steps, &base_ctx));

        // Also collect @dep annotated inputs as dependencies
        let inputs = match flow {
            FlowLike::Flow(f) => &f.inputs,
            FlowLike::Construct(c) => &c.inputs,
        };
        for field in inputs {
            if field.annotations.iter().any(|a| a.name == "dep") {
                // The type_expr of a @dep field is the trait name
                if let TypeExpr::Named(type_name) = &field.type_expr {
                    all_deps.insert(type_name.clone());
                }
            }
        }
    }

    // Emit the Deps struct
    out.push_str(&gen_deps_struct(&all_deps));

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
            .filter(|f| !f.annotations.iter().any(|a| a.name == "dep"))
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>()
            .join(",\n    ");

        // Determine if we need deps parameter — include @dep annotated inputs
        let dep_inputs: Vec<&Field> = inputs.iter()
            .filter(|f| f.annotations.iter().any(|a| a.name == "dep"))
            .collect();
        let flow_deps = collect_deps(steps, &base_ctx);
        let has_deps = !flow_deps.is_empty() || !dep_inputs.is_empty();
        let deps_param = if has_deps { "deps: &Deps, " } else { "" };

        // Build context for this flow
        let mut ctx = build_ctx_from_solution(solution, effective_name_to_shape.clone(), registry);
        ctx.is_orchestrator = is_orchestrator;
        // Register inputs as locals, with their declared types for inference.
        // Skip @dep annotated inputs — they're accessed via deps.x, not as locals.
        for input in inputs {
            if input.annotations.iter().any(|a| a.name == "dep") {
                // Register the dep field name (e.g. "cohort_repo") as a Trait in name_to_shape
                // so the expression translator routes calls through deps.x.method().await?
                ctx.name_to_shape.insert(input.name.clone(), Shape::Trait);
                continue;
            }
            ctx.locals.insert(input.name.clone());
            ctx.local_types.insert(input.name.clone(), type_to_rust(&input.type_expr));
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
            infer_flow_return_type(return_expr, steps, &ctx, is_orchestrator)
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

/// Emit a runtime-delegated construct (e.g. a saga): one `struct` + trait impl
/// per step, then a function body that builds the boxed step list and calls the
/// layer-declared coordinator. Contains NO saga-specific vocabulary — it keys
/// entirely off the `RuntimeBinding` from the layer.
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

    // Every let-binding across ALL step bodies is a shared saga-state key, so a
    // later step can read an earlier step's result.
    let mut state_locals: std::collections::HashSet<String> = std::collections::HashSet::new();
    for step in steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                if let Expr::Assign(n, _) | Expr::MutAssign(n, _, _) = expr {
                    state_locals.insert(n.clone());
                }
            }
        }
    }

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

        // The step body ctx: inputs are `self.<field>`; the Bus is the injected
        // `bus` param; cross-step locals live in the threaded `state`.
        let mut step_ctx = ctx.clone_for_inference();
        step_ctx.is_orchestrator = true;
        step_ctx.bus_ref = "bus".to_string();
        step_ctx.in_aggregate_fn = true; // input idents render as self.<field>
        for (fname, ftype) in &input_fields {
            step_ctx.self_fields.insert(fname.clone());
            step_ctx.local_types.insert(fname.clone(), ftype.clone());
        }
        step_ctx.state_locals = state_locals.clone();

        out.push_str(&format!("#[async_trait::async_trait]\nimpl {} for {} {{\n", step_trait, type_name));

        // The main body fills `action` (returns updated state); each sub-block
        // fills its mapped method.
        emit_step_method(out, "action", &s.body, method_returns_state("action"), &step_ctx);
        for block in &s.sub_blocks {
            if let Some((_, method)) = rt.method_map.iter().find(|(kw, _)| kw == &block.keyword) {
                emit_step_method(out, method, &block.body, method_returns_state(method), &step_ctx);
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
    // Call the coordinator with the Bus dependency and the step list.
    out.push_str(&format!("    {}(deps.bus.as_ref(), &steps).await\n", to_snake(&rt.coordinator)));
    out.push_str("}\n\n");
}

/// Emit one trait-method impl (`action`/`compensate`) with a translated body.
/// State-threading methods take a `state` param and return the updated state;
/// others take state read-only and return unit.
fn emit_step_method(out: &mut String, method: &str, body: &[Expr], returns_state: bool, ctx: &crate::expr::GenCtx) {
    use crate::expr::expr_to_rust;
    let ret = if returns_state { "serde_json::Value" } else { "()" };
    out.push_str(&format!(
        "    async fn {}(&self, bus: &(dyn Bus + Send + Sync), mut state: serde_json::Value) -> Result<{}, DomainError> {{\n",
        method, ret
    ));
    for expr in body {
        out.push_str(&format!("        {};\n", expr_to_rust(expr, ctx)));
    }
    if returns_state {
        out.push_str("        Ok(state)\n    }\n");
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

/// Format generic type parameters: `<T, U>` or empty string if none.
fn generic_params_rust(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
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
