//! Rust code generation from VEIL AST.
//!
//! Fully shape-driven: constructs are generated according to their core
//! shape (`mod` → crate, `struct`/`enum` → types, `trait` → async traits,
//! `impl` → adapter structs, `fn` → orchestrator functions). The construct's
//! layer subkind appears only in doc comments — never in generation logic.

use veil_ir::ast::*;
use veil_ir::layer::Shape;

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

    files.push(gen_workspace_toml(solution));

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
        ));
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

fn gen_workspace_toml(sol: &Solution) -> GeneratedFile {
    let mut members = Vec::new();
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Mod {
                members.push(format!("    \"crates/{}\"", to_snake(&c.name)));
            }
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

fn gen_module_crate(
    module: &Construct,
    all_impls: &[&Construct],
    top_level_flows: &[&Flow],
    flow_generated: &mut bool,
    solution: &Solution,
) -> Vec<GeneratedFile> {
    let crate_name = to_snake(&module.name);
    let mut files = Vec::new();
    let mut contents = flatten_module(module);

    // Include solution-level trait constructs (like declared Bus) in every module
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Trait {
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
            // Add sibling crate dependencies only if this module's flows
            // reference constructs from them (detected by step ctx refs).
            let needed_siblings = detect_sibling_refs(module, solution);
            for sibling in &needed_siblings {
                if *sibling != crate_name {
                    cargo.push_str(&format!("\n{} = {{ path = \"../{}\" }}", sibling, sibling));
                }
            }
            cargo.push_str("\n");
            cargo.push_str("chrono.workspace = true\ntracing.workspace = true\n");
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
    files.push(gen_impls(&impls_for_module, &crate_name));

    // Application: fn-shaped constructs in this module, plus top-level flows
    // (generated once, in the first module that has traits).
    let mut app_flows: Vec<FlowLike> = contents.fns.iter().map(|c| FlowLike::Construct(c)).collect();
    if !*flow_generated && !contents.traits.is_empty() && !top_level_flows.is_empty() {
        *flow_generated = true;
        app_flows.extend(top_level_flows.iter().map(|f| FlowLike::Flow(f)));
    }
    files.push(gen_application(&app_flows, &contents, &crate_name, solution));

    files
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
    for t in &contents.traits {
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
        "Str", "Int", "F64", "Bool", "Bytes", "UUID", "DateTime", "List", "Map", "Set", "Opt",
        "Res", "String",
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
        "/// {}: {}\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub struct {} {{\n",
        c.subkind, c.name, c.name
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
        // Generate a simple constructor for all struct-shaped constructs with fields
        out.push_str(&format!(
            "impl {} {{\n    pub fn new({}) -> Self {{\n        Self {{ {} }}\n    }}\n}}\n\n",
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

    // Determine the event wrapper enum name (from children with children that are struct-shaped message types)
    let event_enum_name = format!("{}Event", c.name);

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

        out.push_str(&format!(
            "    pub fn {}(&mut self, {}) -> Result<Vec<{}>, DomainError> {{\n",
            to_snake(&func.name), params_str, event_enum_name
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

        for expr in &func.body {
            match expr {
                Expr::Assign(field, rhs) if field_names.contains(field) => {
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
                    let fields_str = if !a.named_args.is_empty() {
                        a.named_args.iter().map(|(k, v)| {
                            let v_str = translate_emit_field(v, &ctx, &field_names);
                            if k == &v_str { k.clone() } else { format!("{}: {}", to_snake(k), v_str) }
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
                    out.push_str(&format!("        {};\n", expr_to_rust(other, &ctx)));
                }
            }
        }

        out.push_str("        Ok(events)\n");
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
        "/// {}: {}\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\npub enum {} {{\n",
        c.subkind, c.name, c.name
    ));
    for v in &c.variants {
        out.push_str(&format!("    {},\n", v));
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

fn gen_traits(contents: &ModuleContents, crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Trait definitions (async traits).\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse uuid::Uuid;\n\n");
    out.push_str("use crate::domain::types::*;\n\n");
    out.push_str("/// Domain error type.\n#[derive(Debug, thiserror::Error)]\npub enum DomainError {\n");
    out.push_str("    #[error(\"Not found\")]\n    NotFound,\n");
    out.push_str("    #[error(\"Validation failed: {0}\")]\n    Validation(String),\n");
    out.push_str("    #[error(\"External service error: {0}\")]\n    External(String),\n");
    out.push_str("}\n\n");
    out.push_str("/// Validation error type.\n#[derive(Debug, thiserror::Error)]\n#[error(\"Validation error: {0}\")]\npub struct ValidationError(pub String);\n\n");

    for t in &contents.traits {
        out.push_str(&format!("/// {}: {}\n#[async_trait]\npub trait {} {{\n", t.subkind, t.name, t.name));
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

fn gen_impls(impls: &[&Construct], crate_name: &str) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Implementations of traits.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables, dead_code)]\n\n");
    out.push_str("use async_trait::async_trait;\nuse crate::ports::*;\nuse crate::domain::types::*;\nuse uuid::Uuid;\n\n");

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
                        out.push_str(&format!("    pub {}: String,\n", arg.to_lowercase()));
                    }
                }
            }
            out.push_str("}\n\n");

            out.push_str(&format!(
                "// TODO: Implement {} for {}\n// #[async_trait]\n// impl {} for {} {{ ... }}\n\n",
                target, c.name, target, c.name
            ));
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/adapters/mod.rs", crate_name),
        content: out,
    }
}

/// Something that generates an orchestrator function — either a core `flow`
/// or an fn-shaped layer construct (service, saga, ...).
enum FlowLike<'a> {
    Flow(&'a Flow),
    Construct(&'a Construct),
}

fn gen_application(flows: &[FlowLike], module_contents: &ModuleContents, crate_name: &str, solution: &Solution) -> GeneratedFile {
    use crate::expr::{GenCtx, collect_deps, gen_deps_struct, stmt_to_rust, expr_to_rust};
    use std::collections::HashMap;

    let mut out = String::new();
    out.push_str("//! Application services and flow orchestrators.\n\n");
    out.push_str("#![allow(unused_imports, unused_variables, dead_code)]\n\n");
    out.push_str("use crate::ports::*;\nuse crate::domain::types::*;\nuse crate::domain::messages::*;\n");
    out.push_str("use std::sync::Arc;\nuse uuid::Uuid;\nuse chrono::Utc;\n");
    // Import sibling crate types (only those we depend on via ctx refs)
    // Collect needed siblings from the module we're generating for
    let mut current_module: Option<&Construct> = None;
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Mod && to_snake(&c.name) == crate_name {
                current_module = Some(c);
            }
        }
    }
    if let Some(module) = current_module {
        let needed = detect_sibling_refs(module, solution);
        for sibling in &needed {
            if *sibling != crate_name {
                out.push_str(&format!("use {}::domain::types::*;\n", sibling));
                out.push_str(&format!("use {}::domain::messages::*;\n", sibling));
                out.push_str(&format!("use {}::ports::*;\n", sibling));
            }
        }
    }
    out.push_str("\n");

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
    // Also include top-level constructs (like injected Bus)
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

    // Collect all deps across all flows
    let base_ctx = GenCtx::new(name_to_shape.clone());
    let mut all_deps = std::collections::HashSet::new();
    for flow in flows {
        let steps = match flow {
            FlowLike::Flow(f) => &f.steps,
            FlowLike::Construct(c) => &c.steps,
        };
        all_deps.extend(collect_deps(steps, &base_ctx));
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

        out.push_str(&format!("/// {}: {}\n", subkind, name));
        for ann in annotations {
            out.push_str(&format!("/// @{}\n", ann.name));
        }

        let params = inputs
            .iter()
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>()
            .join(",\n    ");

        // Determine if we need deps parameter
        let flow_deps = collect_deps(steps, &base_ctx);
        let deps_param = if !flow_deps.is_empty() { "deps: &Deps, " } else { "" };

        out.push_str(&format!(
            "#[tracing::instrument(skip_all)]\npub async fn {}(\n    {}{}\n) -> Result<(), DomainError> {{\n",
            to_snake(name),
            deps_param,
            params
        ));

        // Build context for this flow
        let mut ctx = GenCtx::new(name_to_shape.clone());
        // Register inputs as locals
        for input in inputs {
            ctx.locals.insert(input.name.clone());
        }

        for step in steps {
            match step {
                FlowStep::Step(s) => {
                    out.push_str(&format!("    // step: {}\n", s.name));
                    for expr in &s.body {
                        out.push_str(&stmt_to_rust(expr, &mut ctx));
                        out.push_str("\n");
                    }
                    out.push_str("\n");
                }
                FlowStep::Parallel(par) => {
                    out.push_str("    // parallel execution\n");
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

        // Return expression
        if let Some(ret) = return_expr {
            out.push_str(&format!("    Ok({})\n", expr_to_rust(ret, &ctx)));
        } else {
            out.push_str("    Ok(())\n");
        }
        out.push_str("}\n\n");
    }

    GeneratedFile {
        path: format!("crates/{}/src/application/mod.rs", crate_name),
        content: out,
    }
}

/// Detect which sibling modules a module's flows reference (via step ctx refs).
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
            let rust_args = args.iter().map(type_to_rust).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, rust_args)
        }
        TypeExpr::Result(Some(inner)) => format!("Result<{}, DomainError>", type_to_rust(inner)),
        TypeExpr::Result(None) => "Result<(), DomainError>".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_to_rust(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", type_to_rust(inner)),
        TypeExpr::Map(k, v) => format!(
            "std::collections::HashMap<{}, {}>",
            type_to_rust(k),
            type_to_rust(v)
        ),
        TypeExpr::Set(inner) => format!("std::collections::HashSet<{}>", type_to_rust(inner)),
    }
}

/// Infer a Rust type for shorthand fields (untyped, name-only).
/// Purely conventional inference on the field NAME — not domain knowledge.
fn infer_field_type(name: &str) -> String {
    if name == "id" || name.ends_with("_id") {
        return "Uuid".to_string();
    }
    if name.ends_with("_at") || name == "created" || name == "updated" {
        return "DateTime<Utc>".to_string();
    }
    "String".to_string()
}
