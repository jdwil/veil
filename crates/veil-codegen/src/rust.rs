//! Rust code generation from VEIL IR.
//!
//! Generates a hexagonal architecture Rust workspace from a VEIL Solution AST.
//! Structure per bounded context:
//!   src/
//!     domain/
//!       mod.rs
//!       types.rs (value objects, entities)
//!       events.rs (domain events enum)
//!       commands.rs (command structs)
//!     ports/
//!       mod.rs (async traits)
//!     adapters/
//!       mod.rs (adapter stubs)
//!     application/
//!       mod.rs (flow orchestrators)
//!     lib.rs

use veil_ir::ast::*;

/// Generated Rust project output.
pub struct GeneratedProject {
    pub files: Vec<GeneratedFile>,
}

pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// Generate a Rust project from a VEIL Solution AST.
pub fn generate(solution: &Solution) -> GeneratedProject {
    let mut files = Vec::new();

    // Generate workspace Cargo.toml
    files.push(gen_workspace_toml(solution));

    // Generate per-context crates
    let mut flow_generated = false;
    for item in &solution.items {
        if let TopLevelItem::Context(ctx) = item {
            files.extend(gen_context_crate(ctx, &solution.name, solution, &mut flow_generated));
        }
    }

    GeneratedProject { files }
}

fn gen_workspace_toml(sol: &Solution) -> GeneratedFile {
    let mut members = Vec::new();
    for item in &sol.items {
        if let TopLevelItem::Context(ctx) = item {
            members.push(format!("    \"crates/{}\"", to_snake(&ctx.name)));
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
"#,
        members.join(",\n")
    );

    GeneratedFile {
        path: "Cargo.toml".to_string(),
        content,
    }
}

fn gen_context_crate(ctx: &Context, _sol_name: &str, solution: &Solution, flow_generated: &mut bool) -> Vec<GeneratedFile> {
    let crate_name = to_snake(&ctx.name);
    let mut files = Vec::new();

    // Cargo.toml for the crate
    files.push(GeneratedFile {
        path: format!("crates/{}/Cargo.toml", crate_name),
        content: format!(
            r#"[package]
name = "{crate_name}"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace = true
async-trait.workspace = true
thiserror.workspace = true
serde.workspace = true
uuid.workspace = true
chrono.workspace = true
tracing.workspace = true
"#
        ),
    });

    // lib.rs
    files.push(GeneratedFile {
        path: format!("crates/{}/src/lib.rs", crate_name),
        content: format!(
            "//! {} bounded context.\n\npub mod domain;\npub mod ports;\npub mod adapters;\npub mod application;\n",
            ctx.name
        ),
    });

    // Domain types
    files.push(gen_domain_types(ctx, &crate_name));
    files.push(gen_domain_events(ctx, &crate_name));
    files.push(gen_domain_commands(ctx, &crate_name));
    files.push(gen_domain_mod(ctx, &crate_name));

    // Ports
    files.push(gen_ports(ctx, &crate_name));

    // Adapters — find adapters that target ports in this context
    let ctx_port_names: Vec<&str> = ctx.items.iter().filter_map(|i| {
        if let ContextItem::Port(p) = i { Some(p.name.as_str()) } else { None }
    }).collect();

    let adapters_for_ctx: Vec<&Adapter> = solution.items.iter().filter_map(|item| {
        if let TopLevelItem::Adapter(a) = item {
            if ctx_port_names.contains(&a.target_port.as_str()) {
                return Some(a);
            }
        }
        None
    }).collect();

    files.push(gen_adapters(&adapters_for_ctx, &crate_name));

    // Application — generate flows only in the first context with ports
    let has_ports = ctx.items.iter().any(|i| matches!(i, ContextItem::Port(_)));
    let flows: Vec<&Flow> = solution.items.iter().filter_map(|item| {
        if let TopLevelItem::Flow(f) = item { Some(f) } else { None }
    }).collect();

    if !*flow_generated && has_ports {
        *flow_generated = true;
        files.push(gen_application(&flows, ctx, &crate_name));
    } else {
        files.push(GeneratedFile {
            path: format!("crates/{}/src/application/mod.rs", crate_name),
            content: "//! Application services and flow orchestrators.\n".to_string(),
        });
    }

    files
}

fn gen_domain_mod(_ctx: &Context, crate_name: &str) -> GeneratedFile {
    GeneratedFile {
        path: format!("crates/{}/src/domain/mod.rs", crate_name),
        content: "pub mod types;\npub mod events;\npub mod commands;\n".to_string(),
    }
}

fn gen_domain_types(ctx: &Context, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Domain types — value objects, entities, and aggregates.\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\nuse crate::ports::ValidationError;\n\n");

    // Collect all defined type names
    let mut defined_types: Vec<String> = Vec::new();
    let mut all_referenced_types: Vec<String> = Vec::new();

    for item in &ctx.items {
        match item {
            ContextItem::ValueObject(vo) => {
                defined_types.push(vo.name.clone());
                collect_referenced_types(&vo.fields, &mut all_referenced_types);
            }
            ContextItem::Entity(ent) => {
                defined_types.push(ent.name.clone());
                collect_referenced_types(&ent.fields, &mut all_referenced_types);
            }
            ContextItem::Aggregate(agg) => {
                defined_types.push(agg.name.clone());
                collect_referenced_types(&agg.fields, &mut all_referenced_types);
                for cmd in &agg.commands {
                    collect_referenced_types(&cmd.fields, &mut all_referenced_types);
                    if let Some(rt) = &cmd.return_type {
                        collect_type_refs(rt, &mut all_referenced_types);
                    }
                }
            }
            ContextItem::Port(port) => {
                for method in &port.methods {
                    for param in &method.params {
                        collect_type_refs(&param.type_expr, &mut all_referenced_types);
                    }
                    if let Some(rt) = &method.return_type {
                        collect_type_refs(rt, &mut all_referenced_types);
                    }
                }
            }
            _ => {}
        }
    }

    // Generate stub type aliases for referenced-but-not-defined types
    let builtin = ["Str", "Int", "F64", "Bool", "Bytes", "UUID", "DateTime",
                   "List", "Map", "Set", "Opt", "Res", "String"];
    let undefined: Vec<String> = all_referenced_types
        .iter()
        .filter(|t| !defined_types.contains(t) && !builtin.contains(&t.as_str()))
        .cloned()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if !undefined.is_empty() {
        out.push_str("// Stub types — replace with actual definitions\n");
        for t in &undefined {
            out.push_str(&format!("pub type {} = String;\n", t));
        }
        out.push_str("\n");
    }

    for item in &ctx.items {
        match item {
            ContextItem::ValueObject(vo) => {
                out.push_str(&gen_value_object(vo));
            }
            ContextItem::Entity(ent) => {
                out.push_str(&gen_entity(ent));
            }
            ContextItem::Aggregate(agg) => {
                out.push_str(&gen_aggregate_struct(agg));
            }
            _ => {}
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/types.rs", crate_name),
        content: out,
    }
}

fn collect_referenced_types(fields: &[Field], refs: &mut Vec<String>) {
    for field in fields {
        collect_type_refs(&field.type_expr, refs);
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
    }
}

fn gen_value_object(vo: &ValueObject) -> String {
    let mut out = String::new();
    let has_invariant = vo.annotations.iter().any(|a| a.name == "invariant");

    out.push_str(&format!(
        "/// Value object: {}\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub struct {} {{\n",
        vo.name, vo.name
    ));
    for field in &vo.fields {
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake(&field.name),
            type_to_rust(&field.type_expr)
        ));
    }
    out.push_str("}\n\n");

    // Generate TryFrom/new with validation if there's an invariant
    if has_invariant {
        out.push_str(&format!(
            "impl {} {{\n    pub fn new({}) -> Result<Self, ValidationError> {{\n        let value = Self {{ {} }};\n        value.validate()?;\n        Ok(value)\n    }}\n\n    fn validate(&self) -> Result<(), ValidationError> {{\n        // TODO: implement invariant validation\n        Ok(())\n    }}\n}}\n\n",
            vo.name,
            vo.fields.iter().map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr))).collect::<Vec<_>>().join(", "),
            vo.fields.iter().map(|f| to_snake(&f.name)).collect::<Vec<_>>().join(", "),
        ));
    }

    out
}

fn gen_entity(ent: &Entity) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "/// Entity: {}\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {} {{\n",
        ent.name, ent.name
    ));
    for field in &ent.fields {
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake(&field.name),
            type_to_rust(&field.type_expr)
        ));
    }
    out.push_str("}\n\n");
    out
}

fn gen_aggregate_struct(agg: &Aggregate) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "/// Aggregate root: {}\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {} {{\n",
        agg.name, agg.name
    ));
    for field in &agg.fields {
        out.push_str(&format!(
            "    pub {}: {},\n",
            to_snake(&field.name),
            type_to_rust(&field.type_expr)
        ));
    }
    out.push_str("}\n\n");
    out
}

fn gen_domain_events(ctx: &Context, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Domain events.\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\n\n");

    // Collect all events from all aggregates
    let mut events: Vec<&Event> = Vec::new();
    for item in &ctx.items {
        if let ContextItem::Aggregate(agg) = item {
            events.extend(&agg.events);
        }
    }

    if events.is_empty() {
        out.push_str("// No domain events defined in this context.\n");
    } else {
        // Generate enum
        out.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\npub enum DomainEvent {\n");
        for evt in &events {
            out.push_str(&format!("    {}({}Data),\n", evt.name, evt.name));
        }
        out.push_str("}\n\n");

        // Generate data structs for each event
        for evt in &events {
            out.push_str(&format!(
                "#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {}Data {{\n",
                evt.name
            ));
            for field in &evt.fields {
                let rust_type = if field.name == field.type_expr.to_string_simple() {
                    // Shorthand field — infer common types
                    infer_field_type(&field.name)
                } else {
                    type_to_rust(&field.type_expr)
                };
                out.push_str(&format!("    pub {}: {},\n", to_snake(&field.name), rust_type));
            }
            out.push_str("}\n\n");
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/events.rs", crate_name),
        content: out,
    }
}

fn gen_domain_commands(ctx: &Context, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Domain commands.\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\n\n");
    out.push_str("use super::types::*;\n\n");

    let mut has_commands = false;
    for item in &ctx.items {
        if let ContextItem::Aggregate(agg) = item {
            for cmd in &agg.commands {
                has_commands = true;
                out.push_str(&format!(
                    "/// Command: {}\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct {} {{\n",
                    cmd.name, cmd.name
                ));
                for field in &cmd.fields {
                    out.push_str(&format!(
                        "    pub {}: {},\n",
                        to_snake(&field.name),
                        type_to_rust(&field.type_expr)
                    ));
                }
                out.push_str("}\n\n");
            }
        }
    }

    if !has_commands {
        out.push_str("// No commands defined in this context.\n");
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/commands.rs", crate_name),
        content: out,
    }
}

fn gen_ports(ctx: &Context, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Port definitions (async traits).\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse uuid::Uuid;\n\n");
    out.push_str("use crate::domain::types::*;\n\n");
    out.push_str("/// Domain error type.\n#[derive(Debug, thiserror::Error)]\npub enum DomainError {\n");
    out.push_str("    #[error(\"Not found\")]\n    NotFound,\n");
    out.push_str("    #[error(\"Validation failed: {0}\")]\n    Validation(String),\n");
    out.push_str("    #[error(\"External service error: {0}\")]\n    External(String),\n");
    out.push_str("}\n\n");
    out.push_str("/// Validation error for value objects.\n#[derive(Debug, thiserror::Error)]\n#[error(\"Validation error: {0}\")]\npub struct ValidationError(pub String);\n\n");

    for item in &ctx.items {
        if let ContextItem::Port(port) = item {
            out.push_str(&format!(
                "#[async_trait]\npub trait {} {{\n",
                port.name
            ));
            for method in &port.methods {
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
                    to_snake(&method.name), params
                ));
            }
            out.push_str("}\n\n");
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/ports/mod.rs", crate_name),
        content: out,
    }
}

fn gen_adapters(adapters: &[&Adapter], crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Adapter implementations.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables, dead_code)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse crate::ports::*;\nuse crate::domain::types::*;\nuse uuid::Uuid;\n\n");

    if adapters.is_empty() {
        out.push_str("// No adapters target ports in this context.\n");
    } else {
        for adapter in adapters {
            // Generate struct
            out.push_str(&format!(
                "/// Adapter: {} (implements {})\npub struct {} {{\n",
                adapter.name, adapter.target_port, adapter.name
            ));
            // Add env config fields from annotations
            for ann in &adapter.annotations {
                if ann.name == "env" {
                    for arg in &ann.args {
                        out.push_str(&format!("    pub {}: String,\n", arg.to_lowercase()));
                    }
                }
            }
            out.push_str("}\n\n");

            // Note: full trait impl would require matching exact method signatures
            // from the port trait. For now, generate a placeholder comment.
            out.push_str(&format!(
                "// TODO: Implement {} for {}\n// #[async_trait]\n// impl {} for {} {{ ... }}\n\n",
                adapter.target_port, adapter.name, adapter.target_port, adapter.name
            ));
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/adapters/mod.rs", crate_name),
        content: out,
    }
}

fn gen_application(flows: &[&Flow], ctx: &Context, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Application services and flow orchestrators.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables)]\n\n");
    out.push_str("use crate::ports::*;\nuse crate::domain::types::*;\nuse uuid::Uuid;\n\n");

    // Only generate flows in the first context that has ports
    let has_ports = ctx.items.iter().any(|i| matches!(i, ContextItem::Port(_)));

    if has_ports && !flows.is_empty() {
        for flow in flows {
            out.push_str(&format!("/// Flow orchestrator: {}\n", flow.name));

            for ann in &flow.annotations {
                out.push_str(&format!("/// @{}\n", ann.name));
            }

            let params = flow.inputs.iter().map(|f| {
                format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr))
            }).collect::<Vec<_>>().join(",\n    ");

            out.push_str(&format!(
                "#[tracing::instrument(skip_all)]\npub async fn {}(\n    {}\n) -> Result<Uuid, DomainError> {{\n",
                to_snake(&flow.name), params
            ));

            for step in &flow.steps {
                match step {
                    FlowStep::Step(s) => {
                        out.push_str(&format!(
                            "    // Step: {}\n    tracing::info!(\"executing: {}\");\n\n",
                            s.name, s.name
                        ));
                    }
                    FlowStep::Parallel(par) => {
                        out.push_str("    // Parallel execution\n");
                        out.push_str("    tokio::join!(\n");
                        for s in &par.steps {
                            out.push_str(&format!(
                                "        async {{ tracing::info!(\"parallel: {}\"); }},\n",
                                s.name
                            ));
                        }
                        out.push_str("    );\n\n");
                    }
                    FlowStep::Match(_) => {
                        out.push_str("    // TODO: match/branch logic\n\n");
                    }
                }
            }

            out.push_str("    // TODO: implement full flow logic\n");
            out.push_str("    Ok(Uuid::new_v4())\n");
            out.push_str("}\n\n");
        }
    } else {
        out.push_str("// No flows generated for this context.\n");
    }

    GeneratedFile {
        path: format!("crates/{}/src/application/mod.rs", crate_name),
        content: out,
    }
}

// ─── Helper functions ─────────────────────────────────────────────────────

fn to_snake(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap());
    }
    result
}

fn type_to_rust(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Str" => "String".to_string(),
            "Int" => "i64".to_string(),
            "F64" => "f64".to_string(),
            "Bool" => "bool".to_string(),
            "Bytes" => "Vec<u8>".to_string(),
            "UUID" => "Uuid".to_string(),
            "DateTime" => "DateTime<Utc>".to_string(),
            other => other.to_string(),
        },
        TypeExpr::Generic(name, args) => {
            let rust_args = args.iter().map(|a| type_to_rust(a)).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, rust_args)
        }
        TypeExpr::Result(Some(inner)) => format!("Result<{}, DomainError>", type_to_rust(inner)),
        TypeExpr::Result(None) => "Result<(), DomainError>".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_to_rust(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", type_to_rust(inner)),
        TypeExpr::Map(k, v) => format!("std::collections::HashMap<{}, {}>", type_to_rust(k), type_to_rust(v)),
        TypeExpr::Set(inner) => format!("std::collections::HashSet<{}>", type_to_rust(inner)),
    }
}

fn infer_field_type(name: &str) -> String {
    match name {
        "id" => "Uuid".to_string(),
        "created" | "updated" | "verified_at" | "created_at" | "updated_at" => {
            "DateTime<Utc>".to_string()
        }
        "email" => "String".to_string(),
        "name" | "status" | "reason" => "String".to_string(),
        _ => "String".to_string(),
    }
}

// Helper trait for TypeExpr to get simple string representation
trait TypeExprExt {
    fn to_string_simple(&self) -> String;
}

impl TypeExprExt for TypeExpr {
    fn to_string_simple(&self) -> String {
        match self {
            TypeExpr::Named(n) => n.clone(),
            _ => String::new(),
        }
    }
}
