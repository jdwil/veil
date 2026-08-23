//! Rust code generation from VEIL AST.
//!
//! Fully shape-driven: constructs are generated according to their core
//! shape (`mod` → crate, `struct`/`enum` → types, `trait` → async traits,
//! `impl` → adapter structs, `fn` → application functions). The construct's
//! layer subkind appears only in doc comments — never in generation logic.

use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};
use super::*;

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
    // Run layer pre-passes on a mutable copy of the AST.
    let mut solution_owned = solution.clone();
    crate::pass_exec::execute_pre_passes(&mut solution_owned, registry, false);
    let solution = &solution_owned;

    let mut files = Vec::new();

    // CAP-001: resolve external crate links (skip invalid with warning-style omit:
    // only emit successfully resolved links; invalid ones are dropped so gen still
    // produces a workspace — CLI can surface resolve errors separately later).
    let resolved_links = match crate::links::resolve_links(&solution.links) {
        Ok(links) => links,
        Err(errs) => {
            for e in &errs {
                eprintln!("warning: {e}");
            }
            // Best-effort: resolve each independently
            solution
                .links
                .iter()
                .filter_map(|l| crate::links::resolve_link(l).ok())
                .collect()
        }
    };

    files.push(gen_workspace_toml(solution, registry, &resolved_links));

    // Shared crate owns common errors plus every layer `declare` construct/fn.
    // Parsed from the layer source (not from product items) so a product
    // construct with the same name cannot starve veil_shared.
    let layer_decl_items = parse_layer_declare_items(registry);
    let mut shared_traits: Vec<&Construct> = layer_decl_items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Trait => Some(c),
            _ => None,
        })
        .collect();
    let mut shared_structs: Vec<&Construct> = layer_decl_items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Struct => Some(c),
            _ => None,
        })
        .collect();
    let mut shared_fns: Vec<&FnDef> = layer_decl_items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Function(f) => Some(f),
            _ => None,
        })
        .collect();
    for item in &solution.items {
        match item {
            TopLevelItem::Construct(c)
                if c.shape == Shape::Trait
                    && c.layer_provided
                    && !shared_traits.iter().any(|t| t.name == c.name) =>
            {
                shared_traits.push(c);
            }
            TopLevelItem::Construct(c)
                if c.shape == Shape::Struct
                    && c.layer_provided
                    && !shared_structs.iter().any(|s| s.name == c.name) =>
            {
                shared_structs.push(c);
            }
            TopLevelItem::Function(f)
                if f.layer_provided && !shared_fns.iter().any(|x| x.name == f.name) =>
            {
                shared_fns.push(f);
            }
            _ => {}
        }
    }
    // Each top-level mod-shaped construct becomes a crate.
    let modules: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(c),
            _ => None,
        })
        .collect();

    // CAP-003: collect handler message names for register_all.
    let handler_names = collect_handler_names(solution, &modules, registry);

    // ─── Layer Template Augmentation ─────────────────────────────────────
    // Execute codegen templates from loaded layers (di.layer, rust.layer, etc.)
    // BEFORE module generation so sections (derives, trait_attrs, fn_attrs) are
    // available to gen_types/gen_traits/gen_impls.
    let template_output = crate::template::execute_templates(solution, registry, "rust");

    // Extract layer-declared section overrides. When present, these replace the
    // backend's hardcoded defaults for derives, trait attributes, and fn modifiers.
    let layer_derives = crate::template::compose_section(&template_output, "derives");
    let layer_trait_attrs = crate::template::compose_section(&template_output, "trait_attrs");
    let layer_fn_attrs = crate::template::compose_section(&template_output, "fn_attrs");

    files.extend(gen_shared_crate(
        &shared_traits,
        &shared_structs,
        &shared_fns,
        solution,
        registry,
        &resolved_links,
        &handler_names,
        layer_fn_attrs.as_deref(),
    ));

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

    // Single lower + compat synthesis. Every emit path (application Deps,
    // veil_bin, list_routes) reads this IR. No parallel heuristic emitter.
    let mut harness_ir = veil_ir::lower_harness(solution, registry);
    apply_compat_synthesis(&mut harness_ir, solution, registry);

    let mut flow_generated = false;
    for module in &modules {
        files.extend(gen_module_crate(
            module,
            &all_impls,
            &top_level_flows,
            &mut flow_generated,
            solution,
            registry,
            &resolved_links,
            &harness_ir,
            layer_derives.as_deref(),
            layer_trait_attrs.as_deref(),
            layer_fn_attrs.as_deref(),
            &template_output,
        ));
    }

    // RT-001b / RT-001: emit veil_bin from HarnessIR only.
    // emit_bin=never is ignored when the package links veil_server or
    // profile=product_host (host.veil shares runtime/veil.toml).
    let wants_product_host = resolved_links
        .iter()
        .any(|l| l.rust_name == "veil_server" || l.cargo_name == "veil-server")
        || harness_ir.profile == "product_host";
    let emit_blocked = harness_ir.emit_bin == veil_ir::EmitBin::Never && !wants_product_host;
    let has_compose = harness_ir.contexts.iter().any(|c| c.compose.is_some());
    let has_declared_endpoints = package_has_declared_endpoints(solution, registry);
    let has_entry = crate::template::compose_main_section(&template_output, "rust", Some(registry)).is_some()
        || package_has_main_annotation(solution, registry)
        || wants_product_host
        || has_compose
        || has_declared_endpoints
        || harness_ir.contexts.iter().any(|c| !c.endpoints.is_empty());
    let has_main = !emit_blocked && has_entry;
    // role:deploy_hook → veil_hooks bin (provisioner). Not zipped into Lambda.
    if let Some(hook_files) = crate::emit_hooks::emit_hooks_crate(solution, &modules, registry, &harness_ir)
    {
        if let Some(ws) = files.iter_mut().find(|f| f.path == "Cargo.toml")
            && !ws.content.contains("crates/veil_hooks") {
                ws.content = ws.content.replacen(
                    "    \"crates/veil_shared\"",
                    "    \"crates/veil_shared\",\n    \"crates/veil_hooks\"",
                    1,
                );
            }
        files.extend(hook_files);
    }

    if has_main {
        let module_crates: Vec<String> = modules.iter().map(|m| module_crate_name(m, solution)).collect();
        let main_body = if wants_product_host {
            gen_product_host_main(solution, &handler_names, registry)
        } else if !modules.is_empty() {
            let tpl_data = compute_harness_template_data(solution, &modules, registry, &harness_ir);
            if let Some(layer_tpl) = registry.harness_render_templates.get("rust_bin") {
                render_harness_from_layer_template(layer_tpl, &tpl_data)
            } else {
                // Fallback: harness.layer not loaded (should not happen in practice)
                format!("fn main() {{\n    eprintln!(\"veil_bin: harness layer not loaded\");\n}}\n")
            }
        } else if let Some(body) = crate::template::compose_main_section(&template_output, "rust", Some(registry))
        {
            body
        } else {
            // Fallback: empty main wrapper from layer (or inline default)
            let wrapper = registry.harness_render_templates.get("rust_bin_main_wrapper")
                .map(|t| t.replace("{body}", "    println!(\"veil_bin: no modules to run\");\n"))
                .unwrap_or_else(|| String::from(
                    "fn main() {\n    println!(\"veil_bin: no modules to run\");\n}\n"
                ));
            wrapper
        };
        files.extend(gen_bin_crate(
            solution,
            &module_crates,
            &main_body,
            &resolved_links,
            registry,
        ));
        if let Some(ws) = files.iter_mut().find(|f| f.path == "Cargo.toml")
            && !ws.content.contains("crates/veil_bin") {
                // Insert veil_bin after veil_shared in the members list.
                ws.content = ws.content.replacen(
                    "    \"crates/veil_shared\"",
                    "    \"crates/veil_shared\",\n    \"crates/veil_bin\"",
                    1,
                );
            }
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
pub fn collect_by_shape(solution: &Solution, shape: Shape) -> Vec<&Construct> {
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
pub struct ModuleContents<'a> {
    pub(crate) structs: Vec<&'a Construct>,
    pub(crate) enums: Vec<&'a Construct>,
    pub(crate) traits: Vec<&'a Construct>,
    pub(crate) impls: Vec<&'a Construct>,
    pub(crate) fns: Vec<&'a Construct>,
}

pub fn is_harness_config_construct(c: &Construct, registry: &LayerRegistry) -> bool {
    registry.construct_has_role(c, "http_endpoint")
        || registry.construct_has_role(c, "compose")
        || registry.construct_has_role(c, "deps_bundle")
}

pub fn flatten_module<'a>(module: &'a Construct, registry: &LayerRegistry) -> ModuleContents<'a> {
    let mut contents = ModuleContents {
        structs: Vec::new(),
        enums: Vec::new(),
        traits: Vec::new(),
        impls: Vec::new(),
        fns: Vec::new(),
    };
    fn walk<'a>(c: &'a Construct, registry: &LayerRegistry, contents: &mut ModuleContents<'a>) {
        for child in &c.children {
            if is_harness_config_construct(child, registry) {
                continue;
            }
            match child.shape {
                Shape::Struct => contents.structs.push(child),
                Shape::Enum => contents.enums.push(child),
                Shape::Trait => contents.traits.push(child),
                Shape::Impl => contents.impls.push(child),
                Shape::Fn => contents.fns.push(child),
                Shape::Group | Shape::Mod => walk(child, registry, contents),
            }
        }
    }
    walk(module, registry, &mut contents);
    contents
}

