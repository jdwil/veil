use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;


pub fn gen_types(
    contents: &ModuleContents,
    crate_name: &str,
    registry: &LayerRegistry,
    solution: &Solution,
    layer_derives: Option<&str>,
    sibling_crates: &[String],
    template_output: &crate::template::TemplateOutput,
) -> GeneratedFile {
    let mut out = String::new();
    out.push_str("//! Domain types.\n\n");
    out.push_str("#![allow(unused_imports)]\n\n");
    out.push_str("use serde::{Deserialize, Serialize};\nuse uuid::Uuid;\nuse chrono::{DateTime, Utc};\nuse std::collections::HashMap;\nuse crate::ports::*;\nuse crate::domain::messages::*;\n\n");

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
            if block.shape == Shape::Enum
                && let Some(name) = &block.name {
                    defined_types.push(name.clone());
                }
        }
    }

    let builtin = [
        "Str", "Int", "F64", "Bool", "Bytes", "UUID", "Id", "DateTime", "Dt", "List", "Map", "Set", "Opt",
        "Res", "String", "Json",
    ];
    // Type params (T, U) and type-alias names (WearTestRepo) are not domain stubs.
    let mut skip_stubs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in &contents.traits {
        for p in &t.type_params {
            skip_stubs.insert(p.split(':').next().unwrap_or(p).trim().to_string());
        }
        skip_stubs.insert(t.name.clone());
    }
    for item in &solution.items {
        match item {
            TopLevelItem::TypeAlias { name, .. } => {
                skip_stubs.insert(name.clone());
            }
            TopLevelItem::Construct(c) if c.shape == Shape::Trait => {
                for p in &c.type_params {
                    skip_stubs.insert(p.split(':').next().unwrap_or(p).trim().to_string());
                }
                skip_stubs.insert(c.name.clone());
            }
            _ => {}
        }
    }
    let declared = layer_declared_type_names(registry);
    let undefined: Vec<String> = referenced
        .iter()
        .filter(|t| {
            !defined_types.contains(t)
                && !builtin.contains(&t.as_str())
                && !skip_stubs.contains(*t)
        })
        .cloned()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // Layer-declared types live in veil_shared. Never alias them to String —
    // that shadows the real struct (DeployContext became String and broke hooks).
    let mut reexports: Vec<String> = Vec::new();
    let mut stubs: Vec<String> = Vec::new();
    for t in undefined {
        if declared.contains(&t) {
            reexports.push(t);
        } else {
            stubs.push(t);
        }
    }
    // Product constructs that reuse a layer-declared name must not be
    // emitted locally — rustc would see two types (SL-027). Re-export
    // the veil_shared original instead.
    for t in &defined_types {
        if declared.contains(t) && !reexports.contains(t) {
            reexports.push(t.clone());
        }
    }
    reexports.sort();
    stubs.sort();
    if !reexports.is_empty() {
        out.push_str(&format!(
            "pub use veil_shared::{{{}}};\n\n",
            reexports.join(", ")
        ));
    }

    // Cross-context re-exports: if this module depends on sibling crates,
    // re-export their types and ports so local code can use them directly.
    if !sibling_crates.is_empty() {
        let mut emitted_sibling: std::collections::HashSet<String> = std::collections::HashSet::new();
        for item in &solution.items {
            if let TopLevelItem::Construct(c) = item
                && c.shape == Shape::Mod {
                    let sib_crate = super::workspace::module_crate_name(c, solution);
                    if sibling_crates.contains(&sib_crate) && emitted_sibling.insert(sib_crate.clone()) {
                        out.push_str(&format!("pub use {}::domain::types::*;\n", sib_crate));
                        out.push_str(&format!("pub use {}::ports::*;\n", sib_crate));
                    }
                }
        }
        out.push('\n');
    }

    if !stubs.is_empty() {
        // Filter out stubs that are satisfied by sibling crate re-exports.
        let sibling_provided: std::collections::HashSet<String> = if !sibling_crates.is_empty() {
            let mut set = std::collections::HashSet::new();
            fn collect_names(c: &Construct, set: &mut std::collections::HashSet<String>) {
                set.insert(c.name.clone());
                for child in &c.children {
                    collect_names(child, set);
                }
            }
            for item in &solution.items {
                if let TopLevelItem::Construct(c) = item
                    && c.shape == Shape::Mod {
                        let sib_crate = super::workspace::module_crate_name(c, solution);
                        if sibling_crates.contains(&sib_crate) {
                            collect_names(c, &mut set);
                        }
                    }
            }
            set
        } else {
            std::collections::HashSet::new()
        };
        let remaining_stubs: Vec<&String> = stubs.iter()
            .filter(|t| !sibling_provided.contains(*t))
            .collect();
        if !remaining_stubs.is_empty() {
            out.push_str("// Stub types — replace with actual definitions\n");
            for t in &remaining_stubs {
                if let Some((crate_name, path)) = stub_type_path(registry, t) {
                    out.push_str(&format!("pub type {t} = {crate_name}::{path};\n"));
                } else {
                    out.push_str(&format!("pub type {t} = String;\n"));
                }
            }
            out.push('\n');
        }
    }

    // Enums first (unit enums derive Default for fill-in). Nested VOs that are
    // all-defaultable join `defaultable_structs` so later structs can omit
    // them from smart-ctor params (`retry_settings: RetrySettings::default()`).
    // Domain enums stay as required ctor params (AuthType is intentional input).
    let mut defaultable_structs: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for e in &contents.enums {
        if declared.contains(&e.name) {
            continue;
        }
        out.push_str(&gen_enum(e));
    }
    for c in &contents.structs {
        if declared.contains(&c.name) {
            continue;
        }
        let (chunk, is_defaultable) = gen_struct(c, registry, &defaultable_structs, layer_derives);
        out.push_str(&chunk);
        if is_defaultable {
            defaultable_structs.insert(c.name.clone());
        }
        // Append inline template contributions for this struct (emit without emit_to/emit_file).
        if let Some(inline) = crate::template::compose_inline(template_output, &c.name) {
            out.push_str(&inline);
            out.push_str("\n\n");
        }
    }

    GeneratedFile {
        path: format!("crates/{}/src/domain/types.rs", crate_name),
        content: out,
    }
}

pub fn enum_is_unit_only(c: &Construct) -> bool {
    if !c.rich_variants.is_empty() {
        c.rich_variants
            .iter()
            .all(|v| matches!(v, EnumVariant::Unit(_)))
    } else {
        !c.variants.is_empty()
    }
}

/// Collect type references from a struct-shaped construct (fields + blocks + nested).
pub fn collect_construct_type_refs(c: &Construct, refs: &mut Vec<String>) {
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

pub fn collect_type_refs(ty: &TypeExpr, refs: &mut Vec<String>) {
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
        TypeExpr::LitStr(_) => {}
    }
}

/// Collect stub-declared derives/attrs for domain structs used with that SDK.
/// Multi-field → `row_type_derives`; single-field → `wrapper_type_derives` + attrs.
pub fn stub_domain_type_attrs(registry: &LayerRegistry, is_single_field: bool) -> (String, String) {
    let mut row_derives: Vec<String> = Vec::new();
    let mut wrap_derives: Vec<String> = Vec::new();
    let mut wrap_attrs: Vec<String> = Vec::new();
    for stub in &registry.stubs {
        for d in &stub.row_type_derives {
            if !row_derives.contains(d) {
                row_derives.push(d.clone());
            }
        }
        for d in &stub.wrapper_type_derives {
            if !wrap_derives.contains(d) {
                wrap_derives.push(d.clone());
            }
        }
        for a in &stub.wrapper_type_attrs {
            if !wrap_attrs.contains(a) {
                wrap_attrs.push(a.clone());
            }
        }
    }
    if is_single_field && (!wrap_derives.is_empty() || !wrap_attrs.is_empty()) {
        let derive = if wrap_derives.is_empty() {
            String::new()
        } else {
            format!("\n#[derive({})]", wrap_derives.join(", "))
        };
        let attrs: String = wrap_attrs
            .iter()
            .map(|a| format!("\n#[{a}]"))
            .collect();
        // Wrapper derives are separate from Debug/Clone line when they're Type-only.
        // Wrapper derives on their own line; extra_derive on main Debug line stays empty.
        (String::new(), format!("{derive}{attrs}"))
    } else if !row_derives.is_empty() {
        (format!(", {}", row_derives.join(", ")), String::new())
    } else {
        (String::new(), String::new())
    }
}

/// Generate a struct-shaped construct: struct + enum blocks + invariant impl.
pub fn gen_struct(
    c: &Construct,
    registry: &LayerRegistry,
    defaultable: &std::collections::HashSet<String>,
    layer_derives: Option<&str>,
) -> (String, bool) {
    let mut out = String::new();

    // ─── Construct lowers_to: template takes full control ──────────────
    if let Some(template) = registry.construct_lowers_to(c, "rust") {
        let rendered = interpolate_construct_template(template, c, registry);
        out.push_str(&rendered);
        out.push_str("\n\n");
        return (out, false);
    }

    let has_invariant = c.annotations.iter().any(|a| registry.is_invariant_annotation(&a.name));

    // ─── Phase 6: Constraint-driven emission ───────────────────────────
    // Look up layer constraints for this construct (equality_by_value, immutable).
    let constraints: Vec<String> = registry
        .spec_for_construct(c)
        .map(|spec| spec.constraints.clone())
        .unwrap_or_default();
    let has_equality_by_value = constraints.iter().any(|c| c == "equality_by_value");
    let has_immutable = constraints.iter().any(|c| c == "immutable");

    // Fields: direct plus struct-shaped named blocks (e.g. root).
    let mut fields: Vec<&Field> = c.fields.iter().collect();
    for block in &c.blocks {
        if block.shape != Shape::Enum {
            fields.extend(block.fields.iter());
        }
    }

    // Stub-driven derives/attrs (row drivers, serde crates, …) — no crate names here.
    let is_single_field = fields.len() == 1;
    let (extra_derive, extra_attr) = stub_domain_type_attrs(registry, is_single_field);

    // Phase 6c: equality_by_value → append Eq, Hash to derives.
    let constraint_derive = if has_equality_by_value {
        ", Eq, Hash"
    } else {
        ""
    };

    // Layer-driven derives: if a layer declares emit_to "derives", use that
    // as the derive attribute line. Otherwise use the backend default.
    let derive_line = if let Some(layer_d) = layer_derives {
        // Layer provides the full derive line (e.g. "#[derive(Debug, Clone)]")
        // Append any stub-driven extra derives + constraint derives.
        let combined_extra = format!("{extra_derive}{constraint_derive}");
        if combined_extra.is_empty() {
            format!("{layer_d}{extra_attr}")
        } else {
            // Merge: layer_d is something like "#[derive(Debug, Clone)]"
            let merged = layer_d.trim_end_matches(")]").to_string() + &combined_extra + ")]";
            format!("{merged}{extra_attr}")
        }
    } else {
        // Backend default
        format!("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize{extra_derive}{constraint_derive})]{extra_attr}")
    };
    out.push_str(&format!(
        "/// {}: {}\n{}\npub struct {}{} {{\n",
        c.subkind, c.name,
        derive_line,
        c.name, generic_params_rust(&c.type_params)
    ));
    for field in &fields {
        let mut ty = type_to_rust(&field.type_expr);
        // PAR-014: optional @shared → Arc<T> (no lifetimes in .veil)
        if field.annotations.iter().any(|a| registry.is_shared_annotation(&a.name)) {
            ty = format!("std::sync::Arc<{ty}>");
        }
        let snake = to_snake(&field.name);
        // role:secret: still *persist* (repos use Serialize for Dynamo/PG payload).
        // Redaction is harness-side via veil_json_public (skip only on API JSON).
        if registry.field_is_secret(field) {
            out.push_str("    /// Secret — stored; redacted from dual-loop HTTP responses.\n");
            if ty.starts_with("Option<") {
                out.push_str("    #[serde(default)]\n");
            }
        }
        out.push_str(&format!("    pub {snake}: {ty},\n"));
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

    // INV-002: constructor auto-fields / type defaults from layer policy.
    let ctor_pol = if registry.constructor_policy.auto_fields.is_empty() {
        veil_ir::layer::ConstructorPolicy::rust_defaults()
    } else {
        registry.constructor_policy.clone()
    };

    if has_invariant {
        // Smart constructor with invariant validation — same field filtering as non-invariant
        let scalar_default_fields: std::collections::HashSet<String> = fields.iter()
            .filter(|f| matches!(&f.type_expr, TypeExpr::Named(n) if ctor_pol.type_default(n).is_some()))
            .map(|f| f.name.clone())
            .collect();

        let user_fields: Vec<&&Field> = fields.iter()
            .filter(|f| {
                !ctor_pol.is_auto_field(&f.name)
                && !scalar_default_fields.contains(&f.name)
                && !matches!(&f.type_expr, TypeExpr::Optional(_))
                && !matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option")
            })
            .collect();

        let params_str = user_fields.iter()
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>().join(", ");

        let init_fields = fields.iter().map(|f| {
            let snake = to_snake(&f.name);
            if ctor_pol.is_auto_field(&f.name) {
                let is_optional = matches!(&f.type_expr, TypeExpr::Optional(_))
                    || matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option");
                if is_optional { format!("{}: None", snake) } else { format!("{}: Utc::now()", snake) }
            } else if scalar_default_fields.contains(&f.name) {
                let default = match &f.type_expr {
                    TypeExpr::Named(n) => ctor_pol.type_default(n).unwrap_or("0"),
                    _ => "0",
                };
                format!("{}: {}", snake, default)
            } else if matches!(&f.type_expr, TypeExpr::Optional(_)) || matches!(&f.type_expr, TypeExpr::Generic(name, _) if name == "Opt" || name == "Option") {
                format!("{}: None", snake)
            } else {
                snake
            }
        }).collect::<Vec<_>>().join(", ");

        out.push_str(&format!(
            "impl {} {{\n    pub fn new({}) -> Result<Self, ValidationError> {{\n        let value = Self {{ {} }};\n        value.validate()?;\n        Ok(value)\n    }}\n\n    pub fn validate(&self) -> Result<(), ValidationError> {{\n        Ok(())\n    }}\n}}\n\n",
            c.name, params_str, init_fields,
        ));
    } else if !fields.is_empty() {
        // Generate a smart constructor — auto-defaulting timestamps / scalars (INV-002 policy)
        // id is accepted as a parameter — callers provide it (or pass Uuid::new_v4())
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

        // INV-002: scalar type defaults (Int/Bool/…) apply to every struct shape —
        // no subkind branching (MISSION: zero domain knowledge).
        let scalar_default_fields: std::collections::HashSet<String> = fields
            .iter()
            .filter(|f| {
                matches!(
                    &f.type_expr,
                    TypeExpr::Named(n) if ctor_pol.type_default(n).is_some()
                )
            })
            .map(|f| f.name.clone())
            .collect();

        // Empty collections default like scalars so call sites can pass only
        // non-defaultable fields (e.g. name/url/auth, not embedded lists).
        let collection_default_fields: std::collections::HashSet<String> = fields
            .iter()
            .filter(|f| field_has_empty_collection_default(&f.type_expr))
            .map(|f| f.name.clone())
            .collect();

        let user_fields: Vec<&&Field> = fields
            .iter()
            .filter(|f| {
                field_is_required_ctor_param(f, &ctor_pol, &enum_field_names, defaultable)
            })
            .collect();

        let params_str = user_fields.iter()
            .map(|f| format!("{}: {}", to_snake(&f.name), type_to_rust(&f.type_expr)))
            .collect::<Vec<_>>().join(", ");

        let init_fields = fields.iter().map(|f| {
            let snake = to_snake(&f.name);
            if ctor_pol.is_auto_field(&f.name) {
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
                let default = match &f.type_expr {
                    TypeExpr::Named(n) => ctor_pol.type_default(n).unwrap_or("0"),
                    _ => "0",
                };
                format!("{}: {}", snake, default)
            } else if collection_default_fields.contains(&f.name) {
                format!("{}: {}", snake, empty_collection_default(&f.type_expr))
            } else if let Some(sdef) = string_field_default(&f.name) {
                format!("{}: {}", snake, sdef)
            } else if field_has_named_default(&f.type_expr, defaultable) {
                let ty_name = match &f.type_expr {
                    TypeExpr::Named(n) => n.as_str(),
                    _ => "Default",
                };
                format!("{}: {}::default()", snake, ty_name)
            } else if enum_field_names.contains(&f.name) {
                // Use first variant of the enum
                let first_variant = c.blocks.iter()
                    .filter(|b| b.shape == Shape::Enum)
                    .find_map(|b| {
                        let enum_name = b.name.clone().unwrap_or_else(|| format!("{}State", c.name));
                        if let TypeExpr::Named(n) = &f.type_expr
                            && &enum_name == n {
                                return b.variants.first().map(|v| format!("{}::{}", enum_name, v));
                            }
                        None
                    })
                    .unwrap_or_else(|| "Default::default()".to_string());
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

        // Emit `Default` when every field is fillable without caller input
        // (zero-arg `new()`). Call sites like `T.new(a,b,c)` on such types
        // lower to a positional struct update via `defaultable_types` in GenCtx.
        if user_fields.is_empty() {
            out.push_str(&format!(
                "impl Default for {} {{\n    fn default() -> Self {{\n        Self::new()\n    }}\n}}\n\n",
                c.name
            ));
        }
    }

    // Single-field String wrappers (newtypes like RepoId, ArtifactId) get
    // From<T> for String and From<String> for T so they work with APIs
    // accepting impl Into<String>.
    if fields.len() == 1 {
        let single = &fields[0];
        let is_string = matches!(&single.type_expr, TypeExpr::Named(n) if n == "Str" || n == "String");
        if is_string {
            let fname = to_snake(&single.name);
            out.push_str(&format!(
                "impl From<{}> for String {{\n    fn from(v: {}) -> String {{\n        v.{}\n    }}\n}}\n\nimpl From<String> for {} {{\n    fn from(s: String) -> Self {{\n        Self {{ {}: s }}\n    }}\n}}\n\n",
                c.name, c.name, fname, c.name, fname
            ));
        }
    }

    // Generate impl block with business logic fns (if any exist).
    if !c.fns.is_empty() {
        out.push_str(&gen_struct_impl(c, &fields, registry, has_immutable));
    }

    // Types with zero-arg smart ctors (all fields defaultable) are reusable as
    // nested `Type::default()` and as partial-init targets.
    let is_defaultable = !has_invariant
        && fields.iter().all(|f| {
            let ctor_pol = if registry.constructor_policy.auto_fields.is_empty() {
                veil_ir::layer::ConstructorPolicy::rust_defaults()
            } else {
                registry.constructor_policy.clone()
            };
            let enum_field_names: std::collections::HashSet<String> = c
                .blocks
                .iter()
                .filter(|b| b.shape == Shape::Enum)
                .flat_map(|b| {
                    fields
                        .iter()
                        .filter(|ff| {
                            if let TypeExpr::Named(n) = &ff.type_expr {
                                b.name.as_ref().map(|bn| bn == n).unwrap_or(false)
                            } else {
                                false
                            }
                        })
                        .map(|ff| ff.name.clone())
                })
                .collect();
            !field_is_required_ctor_param(f, &ctor_pol, &enum_field_names, defaultable)
        });

    (out, is_defaultable)
}

/// True when the field must appear as a `new(...)` parameter (shape/type policy only).
pub fn field_is_required_ctor_param(
    f: &Field,
    ctor_pol: &veil_ir::layer::ConstructorPolicy,
    enum_field_names: &std::collections::HashSet<String>,
    defaultable: &std::collections::HashSet<String>,
) -> bool {
    if ctor_pol.is_auto_field(&f.name) {
        return false;
    }
    if enum_field_names.contains(&f.name) {
        return false;
    }
    if matches!(
        &f.type_expr,
        TypeExpr::Named(n) if ctor_pol.type_default(n).is_some()
    ) {
        return false;
    }
    if field_has_empty_collection_default(&f.type_expr) {
        return false;
    }
    if field_has_named_default(&f.type_expr, defaultable) {
        return false;
    }
    if string_field_default(&f.name).is_some() {
        return false;
    }
    if matches!(&f.type_expr, TypeExpr::Optional(_))
        || matches!(
            &f.type_expr,
            TypeExpr::Generic(name, _) if name == "Opt" || name == "Option"
        )
    {
        return false;
    }
    true
}

pub fn field_has_named_default(
    ty: &TypeExpr,
    defaultable: &std::collections::HashSet<String>,
) -> bool {
    match ty {
        TypeExpr::Named(n) => defaultable.contains(n),
        _ => false,
    }
}

/// Conventional string defaults for known field names (not domain magic —
/// common infrastructure field conventions used across adapters).
pub fn string_field_default(field_name: &str) -> Option<&'static str> {
    match field_name {
        "authorization_header_string" => Some("\"Authorization\".to_string()"),
        _ => None,
    }
}

/// Generate `impl Name { ... }` block for struct business logic fns.
pub fn gen_struct_impl(c: &Construct, fields: &[&Field], registry: &LayerRegistry, is_immutable: bool) -> String {
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

        // Explicit return type from the VEIL signature; otherwise event-collecting
        // methods default to `Result<Vec<Events>, DomainError>`.
        let err_type_name = registry.error_model.as_ref().map(|em| em.type_name.as_str()).unwrap_or("DomainError");
        let has_explicit_return = func.return_type.as_ref()
            .map(|t| !matches!(t, TypeExpr::Result(None)))
            .unwrap_or(false);
        let return_type_str = if has_explicit_return {
            func.return_type.as_ref()
                .map(|t| type_to_rust_with_error(t, err_type_name))
                .unwrap_or_else(|| format!("Result<Vec<{}>, {}>", event_enum_name, err_type_name))
        } else {
            format!("Result<Vec<{}>, {}>", event_enum_name, err_type_name)
        };

        // Pure query methods use `&self`; mutations / emits need `&mut self`.
        // Phase 6b: immutable constructs always use `&self` (no mutations allowed).
        let needs_mut_self = !is_immutable && method_body_mutates_self(&func.body, &field_names);
        let self_recv = if needs_mut_self { "&mut self" } else { "&self" };
        // Only allocate an events bag when the body emits or the default return is events.
        let needs_events = method_body_has_emit(&func.body)
            || (!has_explicit_return && !has_explicit_return_stmt(&func.body));

        out.push_str(&format!(
            "    pub fn {}({}{}) -> {} {{\n",
            to_snake(&func.name),
            self_recv,
            if params_str.is_empty() { String::new() } else { format!(", {}", params_str) },
            return_type_str
        ));

        // @invariant annotation → guard
        for ann in &func.annotations {
            if registry.is_invariant_annotation(&ann.name) {
                let cond_text = ann.args.first().map(|s| s.as_str()).unwrap_or("true");
                // Simple invariant: field == Value → self.field == EnumName::Value
                let cond_rust = translate_invariant_condition(cond_text, &field_names, &enum_map);
                out.push_str(&format!(
                    "        if !({}) {{ return Err({}::{}(\"invariant violated\".into())); }}\n",
                    cond_rust,
                    err_type_name,
                    registry.error_model.as_ref().and_then(|em| em.variant("validation")).unwrap_or("Validation"),
                ));
            }
        }

        if needs_events {
            out.push_str(&format!("        let mut events: Vec<{}> = Vec::new();\n", event_enum_name));
        }

        // Build context for body translation — thread the real return type so
        // `ret x` matches Option vs Result signatures (not default Ok-wrap).
        let mut ctx = GenCtx::new(HashMap::new());
        ctx.in_method = true;
        ctx.self_fields = field_names.clone();
        ctx.expected_return_rust = Some(return_type_str.clone());
        // Seed struct field types so `for x in self.list` can type elements.
        ctx.types.struct_fields.insert(
            c.name.clone(),
            fields
                .iter()
                .map(|f| (f.name.clone(), type_name_for_field(&f.type_expr)))
                .collect(),
        );
        for p in &func.params {
            ctx.locals.insert(p.name.clone());
            ctx.types.local_types
                .insert(p.name.clone(), type_to_rust(&p.type_expr));
        }
        ctx.ownership.mut_locals = crate::expr::analyze_mut_locals(&func.body);
        ctx.ownership.ident_uses = crate::expr::count_ident_uses(&func.body);

        let mut has_explicit_ret = false;
        for expr in &func.body {
            match expr {
                Expr::Assign(field, rhs, _) | Expr::MutAssign(field, rhs, _) if field_names.contains(field) => {
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
                    // Register let-bindings *after* lowering so the first
                    // occurrence emits `let mut x = …`, and later statements
                    // treat `x` as a local (`out.insert` not `out_insert`).
                    if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = other
                        && !name.contains('.') && !field_names.contains(name) {
                            ctx.locals.insert(name.clone());
                            if let Some(t) = crate::expr::infer_expr_type_pub(rhs, &ctx) {
                                ctx.types.local_types.insert(name.clone(), t);
                            }
                        }
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

/// Does the method body assign to `self` fields or emit domain events?
pub fn method_body_mutates_self(body: &[Expr], field_names: &std::collections::HashSet<String>) -> bool {
    body.iter().any(|e| expr_mutates_self(e, field_names))
}

pub fn expr_mutates_self(expr: &Expr, field_names: &std::collections::HashSet<String>) -> bool {
    match expr {
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            if field_names.contains(name) || name.starts_with("self.") {
                return true;
            }
            expr_mutates_self(rhs, field_names)
        }
        Expr::Action(a) if a.keyword == "emit" => true,
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(|e| expr_mutates_self(e, field_names))
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(|e| expr_mutates_self(e, field_names)))
                    .unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } => {
            body.iter().any(|e| expr_mutates_self(e, field_names))
        }
        Expr::Match(_, arms) => arms
            .iter()
            .any(|arm| arm.body.iter().any(|e| expr_mutates_self(e, field_names))),
        _ => false,
    }
}

pub fn method_body_has_emit(body: &[Expr]) -> bool {
    body.iter().any(expr_has_emit)
}

pub fn expr_has_emit(expr: &Expr) -> bool {
    match expr {
        Expr::Action(a) if a.keyword == "emit" => true,
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(expr_has_emit)
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_has_emit))
                    .unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } => {
            body.iter().any(expr_has_emit)
        }
        Expr::Match(_, arms) => arms.iter().any(|arm| arm.body.iter().any(expr_has_emit)),
        _ => false,
    }
}

pub fn has_explicit_return_stmt(body: &[Expr]) -> bool {
    body.iter().any(expr_has_return)
}

pub fn expr_has_return(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::IfExpr(ie) => {
            ie.then_body.iter().any(expr_has_return)
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_has_return))
                    .unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } => {
            body.iter().any(expr_has_return)
        }
        Expr::Match(_, arms) => arms.iter().any(|arm| arm.body.iter().any(expr_has_return)),
        _ => false,
    }
}

/// Type name stored on struct_fields for element/type inference (Rust form).
pub fn type_name_for_field(ty: &TypeExpr) -> String {
    type_to_rust(ty)
}

/// Empty collection defaults for smart constructors (List → vec![], Map → HashMap::new()).
pub fn field_has_empty_collection_default(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::List(_) | TypeExpr::Map(_, _) | TypeExpr::Set(_) => true,
        TypeExpr::Generic(name, _) => {
            matches!(
                name.as_str(),
                "List" | "Map" | "Set" | "Vec" | "HashMap" | "HashSet"
            )
        }
        _ => false,
    }
}

pub fn empty_collection_default(ty: &TypeExpr) -> &'static str {
    match ty {
        TypeExpr::List(_) => "Vec::new()",
        TypeExpr::Set(_) => "std::collections::HashSet::new()",
        TypeExpr::Map(_, _) => "std::collections::HashMap::new()",
        TypeExpr::Generic(name, _) => match name.as_str() {
            "List" | "Vec" => "Vec::new()",
            "Set" | "HashSet" => "std::collections::HashSet::new()",
            "Map" | "HashMap" => "std::collections::HashMap::new()",
            _ => "Default::default()",
        },
        _ => "Default::default()",
    }
}

/// Translate an invariant condition expression (simple text form).
/// e.g. "status == Pending" → "self.status == CustomerStatus::Pending"
pub fn translate_invariant_condition(
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
pub fn translate_emit_field(
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
pub fn gen_enum(c: &Construct) -> String {
    let mut out = String::new();
    // Unit-only enums get Default (first variant) so partial smart-ctors can
    // fill omitted enum fields via `Enum::default()`.
    let unit_only = if !c.rich_variants.is_empty() {
        c.rich_variants
            .iter()
            .all(|v| matches!(v, EnumVariant::Unit(_)))
    } else {
        !c.variants.is_empty()
    };
    let derives = if unit_only {
        "Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default"
    } else {
        "Debug, Clone, PartialEq, Serialize, Deserialize"
    };
    out.push_str(&format!(
        "/// {}: {}\n#[derive({})]\npub enum {}{} {{\n",
        c.subkind, c.name, derives, c.name, generic_params_rust(&c.type_params)
    ));

    // Use rich_variants if available, otherwise fall back to flat string variants
    if !c.rich_variants.is_empty() {
        let mut first = true;
        for v in &c.rich_variants {
            match v {
                EnumVariant::Unit(name) => {
                    if unit_only && first {
                        out.push_str("    #[default]\n");
                        first = false;
                    }
                    out.push_str(&format!("    {},\n", name));
                }
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
        for (i, v) in c.variants.iter().enumerate() {
            if unit_only && i == 0 {
                out.push_str("    #[default]\n");
            }
            out.push_str(&format!("    {},\n", v));
        }
    }

    out.push_str("}\n\n");
    out
}

/// commands, or any layer-defined message-like constructs).
pub fn gen_child_types(contents: &ModuleContents, crate_name: &str) -> GeneratedFile {

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


// ─── Construct lowers_to template interpolation ──────────────────────────────

/// Interpolate a construct's `lowers_to` template with construct data.
///
/// Supported variables:
/// - `{{name}}` → construct name (PascalCase)
/// - `{{subkind}}` → layer subkind
/// - `{{for field in fields}}...{{end}}` → iterate fields
///   - `{{field.name}}` → field name (snake_case)
///   - `{{field.type}}` → Rust type (via type_to_rust)
/// - `{{for method in methods}}...{{end}}` → iterate methods
///   - `{{method.name}}` → method name (snake_case)
///   - `{{method.params}}` → parameter list (`name: Type, ...`)
///   - `{{method.return_type}}` → return type or empty string
pub fn interpolate_construct_template(
    template: &str,
    c: &Construct,
    registry: &LayerRegistry,
) -> String {
    let mut output = template.to_string();

    // Simple substitutions
    output = output.replace("{{name}}", &c.name);
    output = output.replace("{{subkind}}", &c.subkind);

    // {{for field in fields}}...{{end}} loop
    if let Some(start) = output.find("{{for field in fields}}") {
        if let Some(end_offset) = output[start..].find("{{end}}") {
            let end = start + end_offset + "{{end}}".len();
            let body = &output[start + "{{for field in fields}}".len()..start + end_offset];

            // Collect fields: direct + struct-shaped named blocks (same as gen_struct)
            let mut fields: Vec<&Field> = c.fields.iter().collect();
            for block in &c.blocks {
                if block.shape != Shape::Enum {
                    fields.extend(block.fields.iter());
                }
            }

            let mut expanded = String::new();
            for field in &fields {
                let mut line = body.to_string();
                line = line.replace("{{field.name}}", &to_snake(&field.name));
                line = line.replace("{{field.type}}", &type_to_rust(&field.type_expr));
                expanded.push_str(&line);
            }

            output = format!("{}{}{}", &output[..start], expanded, &output[end..]);
        }
    }

    // {{for method in methods}}...{{end}} loop
    if let Some(start) = output.find("{{for method in methods}}") {
        if let Some(end_offset) = output[start..].find("{{end}}") {
            let end = start + end_offset + "{{end}}".len();
            let body = &output[start + "{{for method in methods}}".len()..start + end_offset];

            let mut expanded = String::new();
            for method in &c.methods {
                let mut line = body.to_string();
                line = line.replace("{{method.name}}", &to_snake(&method.name));
                let params = method
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", to_snake(&p.name), type_to_rust(&p.type_expr)))
                    .collect::<Vec<_>>()
                    .join(", ");
                line = line.replace("{{method.params}}", &params);
                let ret = match &method.return_type {
                    Some(t) => type_to_rust(t),
                    None => String::new(),
                };
                line = line.replace("{{method.return_type}}", &ret);
                expanded.push_str(&line);
            }

            output = format!("{}{}{}", &output[..start], expanded, &output[end..]);
        }
    }

    // Dedent: find minimum indentation of non-empty lines and strip it.
    let lines: Vec<&str> = output.lines().collect();
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    if min_indent > 0 {
        output = lines
            .iter()
            .map(|l| {
                if l.len() >= min_indent {
                    &l[min_indent..]
                } else {
                    l.trim()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Trim leading/trailing blank lines
    let _ = registry; // used for future expansions (e.g. type resolution)
    output.trim().to_string()
}
