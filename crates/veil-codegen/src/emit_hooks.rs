//! Emit `crates/veil_hooks` — provisioner binary for `role:deploy_hook` fns.
//!
//! Hooks compile into application like handlers, but they are not bus names
//! and not HTTP routes. This crate builds the same `Deps` adapters the
//! provisioner needs and calls each hook with `DeployContext` from
//! `VEIL_DEPLOY_CONTEXT` (file) or stdin.

use veil_ir::ast::*;
use veil_ir::layer::LayerRegistry;
use veil_ir::is_deploy_hook;

use crate::rust::{
    adapter_deps_field_name, adapter_dyn_type, apply_adapter_env_field_inits,
    collect_deps_field_map, flatten_module, harness_string_field_default,
    is_pure_generic_adapter_template, module_crate_name, stub_harness_field_expr, to_snake,
    GeneratedFile,
};

/// Hooks in `sol`, paired with the crate that owns them.
pub fn collect_hooks_by_crate<'a>(
    solution: &'a Solution,
    modules: &[&'a Construct],
    registry: &LayerRegistry,
) -> Vec<(&'a Construct, String)> {
    let mut out = Vec::new();
    for module in modules {
        let crate_name = module_crate_name(module, solution);
        let flat = flatten_module(module, registry);
        for f in flat.fns {
            if is_deploy_hook(f, registry) {
                out.push((f, crate_name.clone()));
            }
        }
    }
    out
}

/// Emit the veil_hooks bin when the package declares at least one hook.
pub fn emit_hooks_crate(
    solution: &Solution,
    modules: &[&Construct],
    registry: &LayerRegistry,
    ir: &veil_ir::HarnessIR,
) -> Option<Vec<GeneratedFile>> {
    let hooks = collect_hooks_by_crate(solution, modules, registry);
    if hooks.is_empty() {
        return None;
    }

    let mut module_crates: Vec<String> = modules
        .iter()
        .map(|m| module_crate_name(m, solution))
        .collect();
    module_crates.sort();
    module_crates.dedup();

    let mut deps = String::from(
        "tokio = { workspace = true }\n\
         serde = { workspace = true }\n\
         serde_json = { workspace = true }\n\
         veil_shared = { path = \"../veil_shared\" }\n",
    );
    for c in &module_crates {
        deps.push_str(&format!("{c} = {{ path = \"../{c}\" }}\n"));
    }
    for stub in &registry.stubs {
        if stub.name.is_empty() {
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

    let cargo = format!(
        r#"[package]
name = "veil_hooks"
version.workspace = true
edition.workspace = true

[[bin]]
name = "veil_hooks"
path = "src/main.rs"

[dependencies]
{deps}"#
    );

    let main_rs = gen_hooks_main(solution, modules, registry, ir, &hooks);

    Some(vec![
        GeneratedFile {
            path: "crates/veil_hooks/Cargo.toml".into(),
            content: cargo,
        },
        GeneratedFile {
            path: "crates/veil_hooks/src/main.rs".into(),
            content: main_rs,
        },
    ])
}

fn gen_hooks_main(
    solution: &Solution,
    modules: &[&Construct],
    registry: &LayerRegistry,
    ir: &veil_ir::HarnessIR,
    hooks: &[(&Construct, String)],
) -> String {
    let mut out = String::new();
    out.push_str(
        "//! Provisioner entry for `role:deploy_hook` functions.\n\
         //! Reads DeployContext from VEIL_DEPLOY_CONTEXT (path) or stdin.\n\
         //! Fail closed: any hook Err exits non-zero.\n\n",
    );
    out.push_str("use std::sync::Arc;\n");
    out.push_str("use veil_shared::*;\n");
    let mut used_crates: Vec<String> = hooks.iter().map(|(_, c)| c.clone()).collect();
    used_crates.sort();
    used_crates.dedup();
    for c in &used_crates {
        out.push_str(&format!(
            "use {c}::application::{{self as {c}_app, Deps as {c}_Deps}};\n"
        ));
    }
    out.push_str(
        "\nfn read_context() -> Result<DeployContext, Box<dyn std::error::Error>> {\n\
         let raw = if let Ok(path) = std::env::var(\"VEIL_DEPLOY_CONTEXT\") {\n\
         let s = std::fs::read_to_string(path)?;\n\
         s\n\
         } else {\n\
         use std::io::Read;\n\
         let mut buf = String::new();\n\
         std::io::stdin().read_to_string(&mut buf)?;\n\
         buf\n\
         };\n\
         Ok(serde_json::from_str(&raw)?)\n\
         }\n\n",
    );
    // Async entry point: use layer-provided main wrapper header
    if let Some(tpl) = registry.harness_render_templates.get("rust_bin_main_wrapper") {
        // Extract attribute + fn signature lines from the template
        for line in tpl.lines() {
            let l = line.trim();
            if l.is_empty() { continue; }
            out.push_str(l);
            out.push('\n');
            if l.contains("fn main") { break; }
        }
    }
    out.push_str("    let context = read_context()?;\n");
    out.push_str(
        "    eprintln!(\"veil_hooks: service={} env={}\", context.service_name, context.environment);\n",
    );

    // Wire each crate that owns a hook.
    for crate_name in &used_crates {
        let Some(module) = modules.iter().find(|m| module_crate_name(m, solution) == *crate_name)
        else {
            continue;
        };
        out.push_str(&emit_crate_deps_wiring(
            solution, module, registry, ir, crate_name,
        ));
    }

    for (hook, crate_name) in hooks {
        let fn_name = to_snake(&hook.name);
        let has_context = hook.inputs.iter().any(|f| match &f.type_expr {
            TypeExpr::Named(n) => n == "DeployContext",
            _ => false,
        });
        let has_deps = hook.inputs.iter().any(|f| registry.field_is_dependency(f))
            || hook
                .annotations
                .iter()
                .any(|a| registry.is_dependency_annotation(&a.name));
        let mut args = Vec::new();
        if has_deps {
            args.push(format!("{crate_name}_deps.as_ref()"));
        }
        if has_context {
            args.push("context.clone()".into());
        }
        // Other non-dep inputs: default empty / skip — hooks should take DeployContext.
        for f in &hook.inputs {
            if registry.field_is_dependency(f) {
                continue;
            }
            if matches!(&f.type_expr, TypeExpr::Named(n) if n == "DeployContext") {
                continue;
            }
            args.push("Default::default()".into());
        }
        out.push_str(&format!(
            "    eprintln!(\"veil_hooks: run {name}\");\n\
             {crate_name}_app::{fn_name}({args}).await?;\n",
            name = hook.name,
            args = args.join(", "),
        ));
    }
    out.push_str("    eprintln!(\"veil_hooks: ok\");\n    Ok(())\n}\n");
    out
}

/// Instantiate adapters + `{crate}_deps` for one module.
///
/// Compose wires when present; otherwise first adapter per Deps field
/// (provisioner must run real AWS adapters even when compat=off).
fn emit_crate_deps_wiring(
    solution: &Solution,
    module: &Construct,
    registry: &LayerRegistry,
    ir: &veil_ir::HarnessIR,
    crate_name: &str,
) -> String {
    let flat = flatten_module(module, registry);
    let adapters = &flat.impls;
    let services = &flat.fns;
    let name_to_shape = crate::rust::build_name_to_shape(solution, registry);
    let (_deps_set, dep_fields) = collect_deps_field_map(services, registry, &name_to_shape);
    let ctx = ir
        .contexts
        .iter()
        .find(|c| c.crate_name == crate_name || c.module_name == module.name);
    let declared_compose = ctx.and_then(|c| c.compose.as_ref());
    let declared_deps = ctx.and_then(|c| c.deps.as_ref());

    let mut out = String::new();
    out.push_str(&format!("    // ── {crate_name} adapters ──\n"));

    let mut wired: Vec<(String, String)> = Vec::new(); // field, snake
    let mut wired_fields: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut wired_adapter_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for ad in adapters {
        if is_pure_generic_adapter_template(ad) {
            continue;
        }
        let Some(target) = &ad.target else {
            continue;
        };
        let named_on_compose = declared_compose.is_some_and(|co| {
            co.wires.iter().any(|w| match &w.kind {
                veil_ir::WireKind::Adapter { name } => name == &ad.name,
                _ => false,
            })
        });
        // No compose → wire every adapter (first field-name wins below).
        if declared_compose.is_some() && !named_on_compose {
            continue;
        }
        let field = declared_compose
            .and_then(|co| {
                co.wires
                    .iter()
                    .find(|w| matches!(&w.kind, veil_ir::WireKind::Adapter { name } if name == &ad.name))
                    .map(|w| w.field.clone())
            })
            .unwrap_or_else(|| adapter_deps_field_name(solution, ad, target, &dep_fields));
        if !wired_fields.insert(field.clone()) {
            continue;
        }
        wired_adapter_names.insert(ad.name.clone());
        wired.push((field, to_snake(&ad.name)));
    }

    let mut emitted_harness_lets: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for ad in adapters {
        if !wired_adapter_names.contains(&ad.name) {
            continue;
        }
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
                    out.push_str(&format!("    let {let_name} = {expr};\n"));
                    emitted_harness_lets.insert(ftype);
                }
            }
        }
    }

    let mut adapters_ordered: Vec<&Construct> = adapters.to_vec();
    adapters_ordered.sort_by_key(|ad| {
        ad.fields.iter().any(|f| {
            matches!(&f.type_expr, TypeExpr::Named(n) if n.chars().next().is_some_and(|c| c.is_uppercase()))
        }) as u8
    });
    for ad in adapters_ordered {
        if !wired_adapter_names.contains(&ad.name) {
            continue;
        }
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
                    let init = if let Some((let_name, _)) = stub_harness_field_expr(registry, ftype)
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
        for f in &ad.fields {
            let field_name = to_snake(&f.name);
            if field_inits.contains_key(&field_name) {
                continue;
            }
            if let TypeExpr::Named(tn) = &f.type_expr
                && let Some(impl_ad) = adapters
                    .iter()
                    .find(|a| a.target.as_deref() == Some(tn.as_str()))
                {
                    field_inits.insert(
                        field_name,
                        format!("{}_inst.clone()", to_snake(&impl_ad.name)),
                    );
                    continue;
                }
            let env_key = f.name.to_uppercase();
            field_inits.insert(
                field_name,
                format!("std::env::var(\"{env_key}\").unwrap_or_else(|_| \"default\".into())"),
            );
        }
        let mut fields_init = String::new();
        for (fname, init) in &field_inits {
            fields_init.push_str(&format!("        {fname}: {init},\n"));
        }
        let dyn_ty = format!("{crate_name}::ports::{}", adapter_dyn_type(solution, ad));
        let sn = to_snake(&ad.name);
        if fields_init.is_empty() {
            out.push_str(&format!(
                "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{}{{}});\n",
                ad.name
            ));
        } else {
            out.push_str(&format!(
                "    let {sn}_inst: Arc<dyn {dyn_ty} + Send + Sync> = Arc::new({crate_name}::adapters::{} {{\n{fields_init}    }});\n",
                ad.name
            ));
        }
    }

    // Deps fields = `@dep` / scanned ports on handlers+hooks, NOT every adapter.
    // Extra adapters still get instances (nested @dep on other adapters).
    if !dep_fields.is_empty() || declared_deps.is_some_and(|d| !d.fields.is_empty()) {
        out.push_str(&format!(
            "    let {crate_name}_deps = Arc::new({crate_name}_Deps {{\n"
        ));
        let mut filled: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (trait_name, field) in &dep_fields {
            if !filled.insert(field.clone()) {
                continue;
            }
            if let Some((_, sn)) = wired
                .iter()
                .find(|(f, _)| f == field)
                .or_else(|| wired.iter().find(|(f, _)| f == &to_snake(trait_name)))
            {
                out.push_str(&format!("        {field}: {sn}_inst.clone(),\n"));
            }
        }
        if let Some(dd) = declared_deps {
            for f in &dd.fields {
                if filled.contains(&f.name) {
                    continue;
                }
                if let Some((_, sn)) = wired.iter().find(|(n, _)| n == &f.name) {
                    out.push_str(&format!("        {}: {sn}_inst.clone(),\n", f.name));
                }
            }
        }
        out.push_str("    });\n\n");
    }
    out
}


