//! TypeScript project generation pipeline using TsExpr IR.
//!
//! Routes function bodies through `lower_to_ts → transforms → emit_ts`.
//!
//! ## Pipeline
//!
//! ```text
//! Solution
//!   → build GenCtx (shared with Rust backend)
//!   → for each module:
//!       types.ts     — interfaces for struct-shaped constructs
//!       interfaces.ts — interfaces for trait-shaped constructs (ports)
//!       services.ts  — functions lowered through TsExpr IR
//!   → index.ts, package.json, tsconfig.json
//!   → import tracking on each file
//!   → layer-driven construct files via `lowers_to` templates
//! ```

use veil_ir::ast::*;
use veil_ir::layer::{LayerRegistry, Shape};

use crate::expr::{build_ctx_from_solution, GenCtx};
use crate::rust::build_name_to_shape;

use super::api_client::{TsFile, TsProject};
use super::emit::emit_ts;
use super::lower::{lower_to_ts, to_camel_case, type_to_ts, infer_field_type_ts};
use super::transforms::detect_async;

// ─── Public Entry Point ──────────────────────────────────────────────────────

/// Generate a TypeScript project using the new TsExpr IR pipeline.
///
/// This produces the same file structure as the old `generate_ts` but routes
/// function bodies through `lower_to_ts → emit_ts` instead of `expr_to_ts`.
pub fn generate_ts_ir(solution: &Solution, registry: &LayerRegistry) -> TsProject {
    // Run layer pre-passes on a mutable copy of the AST.
    let mut solution_owned = solution.clone();
    crate::pass_exec::execute_pre_passes(&mut solution_owned, registry, false);
    let solution = &solution_owned;

    let name_to_shape = build_name_to_shape(solution, registry);
    let ctx = build_ctx_from_solution(solution, name_to_shape, registry);
    let sol_name = to_camel_case(&solution.name);

    // Collect module constructs
    let modules: Vec<&Construct> = solution
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(c),
            _ => None,
        })
        .collect();

    let mut files = Vec::new();

    // Generate types.ts — interfaces for struct-shaped constructs
    files.push(gen_types_ir(&modules, registry));

    // Generate interfaces.ts — interfaces for trait-shaped constructs
    files.push(gen_interfaces_ir(&modules, solution));

    // Generate services.ts — functions lowered through TsExpr IR
    files.push(gen_services_ir(&modules, solution, &ctx));

    // index.ts (re-exports) is generated AFTER layer/construct files are
    // collected, so it can also re-export the Svelte page components those
    // layers emit into src/lib/components/ — the contribution harness mounts
    // pages by their named export from this entry (see build_contribution.rs).

    // Generate package.json
    files.push(gen_package_json_ir(&sol_name));

    // Generate tsconfig.json
    files.push(gen_tsconfig_ir());

    // Layer-driven templates (if any)
    let template_output = crate::template::execute_templates(solution, registry, "typescript");
    for tf in template_output.files {
        files.push(TsFile {
            path: tf.path,
            content: tf.content,
        });
    }

    // Layer-driven construct files: if a construct has a `lowers_to { typescript: "..." }`
    // template in the layer, interpolate it and emit the file.
    files.extend(gen_construct_files(&modules, solution, registry));

    // Deduplicate by path: when both the template system (emit_file) and
    // gen_construct_files (lowers_to) produce the same file path, keep only one.
    // Later entries (gen_construct_files) win over earlier template output.
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for file in files.into_iter().rev() {
        if seen_paths.insert(file.path.clone()) {
            deduped.push(file);
        }
    }
    deduped.reverse();
    let mut files = deduped;

    // Now that all layer/construct files are known, generate src/index.ts. In
    // addition to the type/interface/service barrels, collect every Svelte
    // component emitted under src/lib/components/ by its PascalCase name so the
    // entry can expose the framework-neutral `mount(exportName, target, props)`
    // contract (see gen_index_ir). The DLX AI harness (and any contribution
    // consumer) is framework-blind and mounts pages by calling that function;
    // named exports (AgentsPage / WorkflowsPage / etc.) are also kept for direct
    // consumers.
    let mut component_names: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let p = &f.path;
            let name = p
                .strip_prefix("src/lib/components/")
                .or_else(|| p.strip_prefix("lib/components/"))?
                .strip_suffix(".svelte")?;
            if name.is_empty() || name.contains('/') {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect();
    component_names.sort();
    component_names.dedup();
    files.push(gen_index_ir(&sol_name, &component_names));

    let files = files;

    TsProject { files }
}

// ─── Types Generation ────────────────────────────────────────────────────────

/// Generate `src/types.ts` — interfaces for all struct-shaped constructs.
fn gen_types_ir(modules: &[&Construct], registry: &LayerRegistry) -> TsFile {
    let mut out = String::from("// Generated by VEIL — do not edit\n\n");

    for module in modules {
        let structs = collect_shape(module, Shape::Struct);
        let enums = collect_shape(module, Shape::Enum);

        for s in &structs {
            if s.layer_provided {
                continue;
            }
            if matches!(s.keyword.as_str(), "deps" | "compose" | "endpoint") {
                continue;
            }

            // Check for semantic modifiers (constraint-based or annotation-based)
            let constraints: Vec<String> = registry
                .spec_for_construct(s)
                .map(|spec| spec.constraints.clone())
                .unwrap_or_default();
            let is_immutable = constraints.iter().any(|c| c == "immutable")
                || s.annotations.iter().any(|a| registry.annotation_has_role(&a.name, "immutable"));
            let is_equality_by_value = constraints.iter().any(|c| c == "equality_by_value")
                || s.annotations.iter().any(|a| registry.annotation_has_role(&a.name, "equality_by_value"));

            let generics = generic_params_ts(&s.type_params);
            out.push_str(&format!("export interface {}{} {{\n", s.name, generics));

            let fields: Vec<&Field> = if !s.blocks.is_empty() {
                s.blocks
                    .iter()
                    .filter(|b| b.shape != Shape::Enum)
                    .flat_map(|b| b.fields.iter())
                    .collect()
            } else {
                s.fields.iter().collect()
            };

            let readonly_prefix = if is_immutable { "readonly " } else { "" };
            for f in &fields {
                let ts_type = field_type_ts(f);
                out.push_str(&format!("  {}{}: {};\n", readonly_prefix, to_camel_case(&f.name), ts_type));
            }
            out.push_str("}\n\n");

            // @equality_by_value → generate a standalone equals() helper function
            if is_equality_by_value && !fields.is_empty() {
                let name = &s.name;
                out.push_str(&format!("export function {name}Equals(a: {name}{generics}, b: {name}{generics}): boolean {{\n"));
                out.push_str("  return (\n");
                let comparisons: Vec<String> = fields.iter()
                    .map(|f| format!("    a.{f} === b.{f}", f = to_camel_case(&f.name)))
                    .collect();
                out.push_str(&comparisons.join(" &&\n"));
                out.push_str("\n  );\n}\n\n");
            }
        }

        for e in &enums {
            if e.layer_provided {
                continue;
            }
            if e.variants.is_empty() && e.rich_variants.is_empty() {
                continue;
            }
            let generics = generic_params_ts(&e.type_params);

            if !e.rich_variants.is_empty() {
                // Discriminated union for variants with data
                out.push_str(&format!("export type {}{} =\n", e.name, generics));
                let variants: Vec<String> = e
                    .rich_variants
                    .iter()
                    .map(|v| match v {
                        EnumVariant::Unit(name) => format!("  | {{ type: \"{}\" }}", name),
                        EnumVariant::Tuple(name, types) => {
                            let fields = types
                                .iter()
                                .enumerate()
                                .map(|(i, t)| format!("field{}: {}", i, type_to_ts(t)))
                                .collect::<Vec<_>>()
                                .join("; ");
                            format!("  | {{ type: \"{}\"; {} }}", name, fields)
                        }
                        EnumVariant::Struct(name, fields) => {
                            let fs = fields
                                .iter()
                                .map(|f| {
                                    format!(
                                        "{}: {}",
                                        to_camel_case(&f.name),
                                        type_to_ts(&f.type_expr)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("; ");
                            format!("  | {{ type: \"{}\"; {} }}", name, fs)
                        }
                    })
                    .collect();
                out.push_str(&variants.join("\n"));
                out.push_str(";\n\n");
            } else {
                // Simple string union for unit-only enums
                out.push_str(&format!("export type {}{} =\n", e.name, generics));
                let variants: Vec<String> =
                    e.variants.iter().map(|v| format!("  | \"{}\"", v)).collect();
                out.push_str(&variants.join("\n"));
                out.push_str(";\n\n");
            }
        }
    }

    TsFile {
        path: "src/types.ts".to_string(),
        content: out,
    }
}

// ─── Interfaces Generation ───────────────────────────────────────────────────

/// Generate `src/interfaces.ts` — interfaces for all trait-shaped constructs (ports).
fn gen_interfaces_ir(modules: &[&Construct], solution: &Solution) -> TsFile {
    let mut out = String::from("// Generated by VEIL — do not edit\n\n");
    out.push_str("import type * as T from './types';\n\n");
    let type_names = collect_type_names_set(modules);

    let mut all_traits: Vec<&Construct> = Vec::new();
    for module in modules {
        all_traits.extend(
            collect_shape(module, Shape::Trait)
                .into_iter()
                .filter(|t| !t.layer_provided),
        );
    }
    // Also include top-level traits not nested in modules
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape == Shape::Trait && !c.layer_provided {
                all_traits.push(c);
            }
        }
    }

    for t in &all_traits {
        let generics = generic_params_ts(&t.type_params);
        out.push_str(&format!("export interface {}{} {{\n", t.name, generics));
        for method in &t.methods {
            let params = method
                .params
                .iter()
                .map(|p| {
                    format!(
                        "{}: {}",
                        to_camel_case(&p.name),
                        type_to_ts_qualified(&p.type_expr, &type_names)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let ret = match &method.return_type {
                Some(ty) => {
                    let inner = type_to_ts_qualified(ty, &type_names);
                    if inner.starts_with("Promise<") {
                        inner
                    } else {
                        format!("Promise<{}>", inner)
                    }
                }
                None => "Promise<void>".to_string(),
            };
            let method_name = to_camel_case(method.name.trim_end_matches(['!', '?']));
            out.push_str(&format!("  {}({}): {};\n", method_name, params, ret));
        }
        out.push_str("}\n\n");
    }

    TsFile {
        path: "src/interfaces.ts".to_string(),
        content: out,
    }
}

// ─── Services Generation (IR-based) ─────────────────────────────────────────

/// Generate `src/services.ts` — functions lowered through the TsExpr IR pipeline.
fn gen_services_ir(modules: &[&Construct], solution: &Solution, ctx: &GenCtx) -> TsFile {
    let mut out = String::from("// Generated by VEIL — do not edit\n\n");
    out.push_str("import type * as T from './types';\n");
    out.push_str("import type * as I from './interfaces';\n\n");
    let type_names = collect_type_names_set(modules);

    for module in modules {
        let fns = collect_shape(module, Shape::Fn);
        for f in &fns {
            if f.layer_provided {
                continue;
            }
            out.push_str(&gen_function_ir(f, ctx, &type_names));
        }
    }

    // Also generate top-level flows
    for item in &solution.items {
        if let TopLevelItem::Flow(flow) = item {
            out.push_str(&gen_flow_ir(flow, ctx));
        }
    }

    TsFile {
        path: "src/services.ts".to_string(),
        content: out,
    }
}

/// Generate a single function through the TsExpr IR pipeline.
fn gen_function_ir(
    f: &Construct,
    ctx: &GenCtx,
    type_names: &std::collections::HashSet<String>,
) -> String {
    let fn_name = to_camel_case(&f.name);
    let params = f
        .inputs
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                to_camel_case(&p.name),
                type_to_ts_qualified(&p.type_expr, type_names)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    // Collect all body expressions from steps and lower through TsExpr IR
    let mut ts_body = Vec::new();
    for step in &f.steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                ts_body.push(super::lower::lower_to_ts(expr, ctx));
            }
        }
    }

    // Determine if async based on the IR tree
    let is_async = detect_async(&ts_body);

    // Determine return type
    let ret = f
        .return_type
        .as_ref()
        .map(|t| {
            let inner = type_to_ts_qualified(t, type_names);
            if inner.starts_with("Promise<") {
                inner
            } else if is_async {
                format!("Promise<{}>", inner)
            } else {
                inner
            }
        })
        .unwrap_or_else(|| {
            if is_async {
                "Promise<void>".to_string()
            } else {
                "void".to_string()
            }
        });

    // Emit the body
    let mut out = String::new();
    let async_kw = if is_async { "async " } else { "" };
    out.push_str(&format!(
        "export {}function {}({}): {} {{\n",
        async_kw, fn_name, params, ret
    ));

    // Emit step comments and bodies
    let mut expr_idx = 0;
    for step in &f.steps {
        if let FlowStep::Step(s) = step {
            out.push_str(&format!("  // Step: {}\n", s.name));
            for _ in &s.body {
                if expr_idx < ts_body.len() {
                    let emitted = emit_ts(&ts_body[expr_idx]);
                    out.push_str(&format!("  {};\n", emitted));
                    expr_idx += 1;
                }
            }
        }
    }

    // If no steps, emit body directly
    if f.steps.is_empty() && !ts_body.is_empty() {
        for expr in &ts_body {
            let emitted = emit_ts(expr);
            out.push_str(&format!("  {};\n", emitted));
        }
    }

    out.push_str("}\n\n");
    out
}

/// Generate a flow function through the TsExpr IR pipeline.
fn gen_flow_ir(flow: &Flow, ctx: &GenCtx) -> String {
    let fn_name = to_camel_case(&flow.name);
    let params = flow
        .inputs
        .iter()
        .map(|p| format!("{}: {}", to_camel_case(&p.name), type_to_ts(&p.type_expr)))
        .collect::<Vec<_>>()
        .join(", ");

    // Lower step bodies through TsExpr IR
    let mut ts_body = Vec::new();
    for step in &flow.steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                ts_body.push(super::lower::lower_to_ts(expr, ctx));
            }
        }
    }

    // Determine if async
    let is_async = detect_async(&ts_body);

    let mut out = String::new();
    let async_kw = if is_async { "async " } else { "" };
    let ret = if is_async {
        "Promise<void>"
    } else {
        "void"
    };
    out.push_str(&format!(
        "export {}function {}({}): {} {{\n",
        async_kw, fn_name, params, ret
    ));

    let mut expr_idx = 0;
    for step in &flow.steps {
        if let FlowStep::Step(s) = step {
            out.push_str(&format!("  // Step: {}\n", s.name));
            for _ in &s.body {
                if expr_idx < ts_body.len() {
                    let emitted = emit_ts(&ts_body[expr_idx]);
                    out.push_str(&format!("  {};\n", emitted));
                    expr_idx += 1;
                }
            }
        }
    }

    out.push_str("}\n\n");
    out
}

// ─── Index + Config Files ────────────────────────────────────────────────────

fn gen_index_ir(sol_name: &str, component_names: &[String]) -> TsFile {
    let mut content = format!(
        "// Generated by VEIL — do not edit\n\
         // Package: {}\n\n\
         export * from './types';\n\
         export * from './interfaces';\n\
         export * from './services';\n",
        sol_name
    );
    if !component_names.is_empty() {
        // Framework-agnostic contribution-mount contract.
        //
        // The DLX AI harness (and any contribution consumer) is FRAMEWORK-BLIND:
        // it never imports svelte/react/vue. Each contribution bundle is
        // self-contained (its own framework bundled in) and exposes a single
        // neutral, DOM-level entry point:
        //
        //   mount(exportName, target, { context, params }) => () => void
        //
        // The harness does: `const unmount = bundle.mount(name, el, props)` and
        // later `unmount()`. This is the SVELTE target's implementation of that
        // contract (svelte's mount/unmount, self-contained). A future React
        // target implements the same signature via createRoot().render()/
        // root.unmount(); Vue via createApp().mount()/app.unmount(). Same
        // contract, framework-specific innards — one harness hosts any target.
        content.push('\n');
        content.push_str("import { mount as __svelteMount, unmount as __svelteUnmount } from 'svelte';\n");
        for name in component_names {
            content.push_str(&format!(
                "import {name} from './lib/components/{name}.svelte';\n"
            ));
        }

        // Keep the existing named exports too (harmless — a consumer may still
        // import a component class directly), plus the barrels above.
        content.push('\n');
        for name in component_names {
            content.push_str(&format!("export {{ {name} }};\n"));
        }

        // exportName -> component registry (the mount contract keys off this).
        content.push_str("\nconst __components: Record<string, any> = {\n");
        for name in component_names {
            content.push_str(&format!("  {name},\n"));
        }
        content.push_str("};\n");

        // The neutral mount function — the ONLY thing the harness calls.
        content.push_str(
            "\n\
             /**\n\
             \x20* Framework-neutral mount: instantiate the contribution export named\n\
             \x20* `exportName` into `target`, returning a teardown function. The harness\n\
             \x20* calls this without importing any UI framework.\n\
             \x20*/\n\
             export function mount(\n\
             \x20 exportName: string,\n\
             \x20 target: HTMLElement,\n\
             \x20 props: { context?: any; params?: any } = {}\n\
             ): () => void {\n\
             \x20 const Component = __components[exportName];\n\
             \x20 if (!Component) {\n\
             \x20   throw new Error(`Export \"${exportName}\" not found in contribution bundle`);\n\
             \x20 }\n\
             \x20 const instance = __svelteMount(Component, { target, props });\n\
             \x20 return () => {\n\
             \x20   try {\n\
             \x20     __svelteUnmount(instance);\n\
             \x20   } catch {\n\
             \x20     /* already torn down */\n\
             \x20   }\n\
             \x20 };\n\
             }\n",
        );
    }
    TsFile {
        path: "src/index.ts".to_string(),
        content,
    }
}

fn gen_package_json_ir(sol_name: &str) -> TsFile {
    let content = format!(
        r#"{{
  "name": "{}",
  "version": "0.1.0",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {{
    "build": "tsc",
    "dev": "tsc --watch"
  }},
  "devDependencies": {{
    "typescript": "^5.4.0"
  }}
}}
"#,
        to_kebab(sol_name)
    );
    TsFile {
        path: "package.json".to_string(),
        content,
    }
}

fn gen_tsconfig_ir() -> TsFile {
    let content = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "outDir": "dist",
    "rootDir": "src",
    "declaration": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*.ts"]
}
"#
    .to_string();
    TsFile {
        path: "tsconfig.json".to_string(),
        content,
    }
}

// ─── Layer-Driven Construct File Generation ──────────────────────────────────

/// For each construct in the solution tree, check if the layer provides a
/// `lowers_to { typescript: "..." }` template. If so, interpolate the template
/// with the construct's data and emit a file.
///
/// This is framework-agnostic: any layer can provide templates for any construct.
fn gen_construct_files(
    modules: &[&Construct],
    solution: &Solution,
    registry: &LayerRegistry,
) -> Vec<TsFile> {
    let mut files = Vec::new();

    // Walk module trees for constructs with lowers_to templates
    for module in modules {
        collect_construct_files(module, registry, &mut files);
    }

    // Also check top-level constructs (not nested in modules)
    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            if c.shape != Shape::Mod {
                collect_construct_files(c, registry, &mut files);
            }
        }
    }

    files
}

/// Recursively walk construct tree, generating files for constructs that have
/// layer-provided `lowers_to` templates.
fn collect_construct_files(
    c: &Construct,
    registry: &LayerRegistry,
    files: &mut Vec<TsFile>,
) {
    // Check if this construct's layer spec has a `lowers_to { typescript: "..." }` template
    if let Some(template) = registry.construct_lowers_to(c, "typescript") {
        let content = interpolate_construct_template(template, c, registry);
        let path = construct_output_path(c, registry);
        // For .svelte files: auto-inject imports for referenced components
        let content = if path.ends_with(".svelte") {
            let content = inject_svelte_component_imports(&content, &path);
            // Developer/edit mode: stamp provenance attributes onto the emitted
            // markup so a clicked DOM node maps back to this construct. Pure
            // property of the `developer` layer being active — never present in
            // a normal build. Generic across any HTML/CSS-lowering layer.
            if developer_layer_active(registry) {
                stamp_provenance(&content, c, &provenance_project_slug())
            } else {
                content
            }
        } else {
            content
        };
        files.push(TsFile { path, content });
    }

    // Recurse into children
    for child in &c.children {
        collect_construct_files(child, registry, files);
    }
}

/// Interpolate a construct template with construct data.
///
/// Supported placeholders:
/// - `{{name}}` → construct name
/// - `{{script}}` → content of the `script` raw block
/// - `{{template}}` → content of the `template` raw block
/// - `{{style}}` → content of the `style` raw block
/// - `{{for field in props}}...{{end}}` → iterate props fields
/// - `{{for field in state}}...{{end}}` → iterate state fields
/// - `{{#if style}}...{{/if}}` → conditional on style existence
/// - `{{field.name}}`, `{{field.type}}`, `{{field.default}}` — inside for loops
/// - `{{derived_decl}}` → `derived` block fields as `let {name} = $derived({expr});`
/// - `{{effect_decl}}` → `effect` blocks as `$effect(() => { ... })` (async-aware)
/// - `{{fn_decl}}` → construct `fn`s as LOCAL (non-exported) component functions
///
/// The `$derived` / `$effect` shapes come from the layer's `reactivity_policy`
/// (never hardcoded here) so framework APIs stay layer-owned (MISSION).
fn interpolate_construct_template(template: &str, c: &Construct, registry: &LayerRegistry) -> String {
    let mut result = template.to_string();

    // Simple replacements
    result = result.replace("{{name}}", &c.name);

    // Raw blocks
    let script_content = c.raw_blocks.iter()
        .find(|(k, _)| k == "script")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let template_content = c.raw_blocks.iter()
        .find(|(k, _)| k == "template")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let style_content = c.raw_blocks.iter()
        .find(|(k, _)| k == "style")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    result = result.replace("{{script}}", script_content);
    result = result.replace("{{template}}", template_content);
    result = result.replace("{{style}}", style_content);

    // {{props_decl}} — single $props() destructure with all prop fields
    if result.contains("{{props_decl}}") {
        let props_block = c.blocks.iter().find(|b| b.keyword == "props");
        let props_script = if let Some(props) = props_block {
            if props.fields.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = props.fields.iter().map(|f| f.name.as_str()).collect();
                let types: Vec<String> = props.fields.iter()
                    .map(|f| format!("{}: {}", f.name, ts_type_for_field(&f.type_expr)))
                    .collect();
                format!("  let {{ {} }}: {{ {} }} = $props();\n", names.join(", "), types.join("; "))
            }
        } else {
            String::new()
        };
        result = result.replace("{{props_decl}}", &props_script);
    }

    // {{state_decl}} — individual $state() declarations per field
    if result.contains("{{state_decl}}") {
        let state_block = c.blocks.iter().find(|b| b.keyword == "state");
        let state_script = if let Some(state) = state_block {
            let mut s = String::new();
            for field in &state.fields {
                let ty = ts_type_for_field(&field.type_expr);
                let default = ts_default_for_type(&field.type_expr);
                s.push_str(&format!("  let {}: {} = $state({});\n", field.name, ty, default));
            }
            s
        } else {
            String::new()
        };
        result = result.replace("{{state_decl}}", &state_script);
    }

    // {{derived_decl}} — `derived` block fields → layer's derived_line pattern.
    // Value form (`let x = $derived(expr)`) keeps object literals valid.
    if result.contains("{{derived_decl}}") {
        let pattern = &registry.reactivity_policy.derived_line;
        let mut s = String::new();
        if !pattern.is_empty() {
            let fn_opts = store_fn_opts(c);
            for block in c.blocks.iter().filter(|b| b.keyword == "derived") {
                for field in &block.fields {
                    let expr_ts = field
                        .default_expr
                        .as_ref()
                        .map(|e| expr_as_ts_value(e, &fn_opts))
                        .unwrap_or_else(|| "undefined".to_string());
                    let line = veil_ir::layer::ReactivityPolicy::fill(
                        pattern,
                        &[("name", &field.name), ("expr", &expr_ts)],
                    );
                    s.push_str("  ");
                    s.push_str(&line);
                    s.push('\n');
                }
            }
        }
        result = result.replace("{{derived_decl}}", &s);
    }

    // {{fn_decl}} — construct `fn`s as LOCAL (non-exported) component functions.
    // Unlike {{fn_declarations}} (stores → `export function`), components keep
    // handlers local so the template can reference them directly.
    if result.contains("{{fn_decl}}") {
        let mut s = String::new();
        let fn_opts = store_fn_opts(c);
        for f in &c.fns {
            if f.name == "template" || f.name == "style" || f.name == "script" {
                continue;
            }
            let params = f
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, ts_type_for_field(&p.type_expr)))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_type = match &f.return_type {
                Some(ty) => format!(": {}", ts_type_for_field(ty)),
                None => String::new(),
            };
            let async_kw = if f.is_async { "async " } else { "" };
            s.push_str(&format!(
                "  {}function {}({}){} {{\n",
                async_kw, f.name, params, ret_type
            ));
            s.push_str(&super::expr_emit::emit_typescript_stmts_with(&f.body, 2, &fn_opts));
            s.push_str("  }\n");
        }
        result = result.replace("{{fn_decl}}", &s);
    }

    // {{effect_decl}} — `effect` blocks → layer's effect_sync / effect_async.
    // An effect whose body contains an `await` (recursively) uses effect_async.
    if result.contains("{{effect_decl}}") {
        let sync_pat = &registry.reactivity_policy.effect_sync;
        let async_pat = &registry.reactivity_policy.effect_async;
        let mut s = String::new();
        if !sync_pat.is_empty() {
            let fn_opts = store_fn_opts(c);
            for effect in &c.effects {
                let body_ts = super::expr_emit::emit_typescript_stmts_with(
                    &effect.body,
                    2,
                    &fn_opts,
                );
                let mut body_full = body_ts.trim_end().to_string();
                if !effect.cleanup.is_empty() {
                    let cleanup_ts = super::expr_emit::emit_typescript_stmts_with(
                        &effect.cleanup,
                        3,
                        &fn_opts,
                    );
                    body_full.push_str(&format!(
                        "\n    return () => {{\n{}\n    }};",
                        cleanup_ts.trim_end()
                    ));
                }
                let is_async = body_full.contains("await ");
                let pattern = if is_async { async_pat } else { sync_pat };
                let block = veil_ir::layer::ReactivityPolicy::fill(
                    pattern,
                    &[("name", &effect.name), ("body", &body_full)],
                );
                s.push_str("  ");
                s.push_str(&block);
                s.push('\n');
            }
        }
        result = result.replace("{{effect_decl}}", &s);
    }

    // {{store_state}} — Svelte 5 legal shared state object.
    if result.contains("{{store_state}}") {
        let fields: Vec<(String, TypeExpr)> = c
            .blocks
            .iter()
            .filter(|b| b.keyword == "state")
            .flat_map(|b| {
                b.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.type_expr.clone()))
            })
            .collect();
        result = result.replace(
            "{{store_state}}",
            &super::expr_emit::emit_store_state(&c.name, &fields),
        );
    }

    // {{fn_declarations}} — exported functions from construct.fns
    if result.contains("{{fn_declarations}}") {
        let mut s = String::new();
        let fn_opts = store_fn_opts(c);
        for f in &c.fns {
            if f.name == "template" || f.name == "style" || f.name == "script" {
                continue;
            }
            let params = f.params.iter()
                .map(|p| format!("{}: {}", p.name, ts_type_for_field(&p.type_expr)))
                .collect::<Vec<_>>().join(", ");
            let ret_type = match &f.return_type {
                Some(ty) => format!(": {}", ts_type_for_field(ty)),
                None => String::new(),
            };
            s.push_str(&format!("export function {}({}){} {{\n", f.name, params, ret_type));
            s.push_str(&super::expr_emit::emit_typescript_stmts_with(&f.body, 1, &fn_opts));
            s.push_str("}\n\n");
        }
        result = result.replace("{{fn_declarations}}", &s);
    }

    // {{state_exports}} — re-export state fields (for stores that need explicit exports)
    if result.contains("{{state_exports}}") {
        let state_block = c.blocks.iter().find(|b| b.keyword == "state");
        let exports = if let Some(state) = state_block {
            if state.fields.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = state.fields.iter().map(|f| f.name.as_str()).collect();
                format!("export {{ {} }};\n", names.join(", "))
            }
        } else {
            String::new()
        };
        result = result.replace("{{state_exports}}", &exports);
    }

    // Conditional blocks: {{#if style}}...{{/if}}
    result = process_conditionals(&result, "style", !style_content.is_empty());
    result = process_conditionals(&result, "script", !script_content.is_empty());
    result = process_conditionals(&result, "template", !template_content.is_empty());

    // For loops: {{for field in props}}...{{end}}
    result = process_for_loop(&result, "props", &collect_block_fields(c, "props"));
    result = process_for_loop(&result, "state", &collect_block_fields(c, "state"));

    result
}

/// Scan a .svelte file for PascalCase component tags (e.g. `<Header ...>`) and inject
/// import statements into the `<script>` block. Skips standard HTML elements and
/// Svelte built-ins ({#if}, {#each}, {@render}, etc.).
fn inject_svelte_component_imports(content: &str, file_path: &str) -> String {
    use std::collections::BTreeSet;

    // Find all PascalCase tags: <ComponentName or <ComponentName>
    let mut components = BTreeSet::new();
    let mut i = 0;
    let bytes = content.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_uppercase() {
            // Extract tag name
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let tag = &content[start..end];
            // Skip if already imported or if it's a Svelte built-in
            if !tag.is_empty() && tag.chars().next().unwrap().is_uppercase() {
                components.insert(tag.to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }

    if components.is_empty() {
        return content.to_string();
    }

    // Remove components that are already imported
    let already_imported: BTreeSet<String> = content.lines()
        .filter(|l| l.trim_start().starts_with("import"))
        .filter_map(|l| {
            // Match: import X from or import { X } from
            let trimmed = l.trim();
            if let Some(after) = trimmed.strip_prefix("import ") {
                let name = after.split_whitespace().next().unwrap_or("");
                if name.chars().next().map_or(false, |c| c.is_uppercase()) {
                    return Some(name.to_string());
                }
            }
            None
        })
        .collect();

    let to_import: Vec<&String> = components.iter()
        .filter(|c| !already_imported.contains(*c))
        .collect();

    if to_import.is_empty() {
        return content.to_string();
    }

    // Determine relative import path based on file location
    // Files in src/routes/ import from $lib/components/
    let import_prefix = "$lib/components";

    // Build import lines
    let imports: String = to_import.iter()
        .map(|name| format!("  import {name} from '{import_prefix}/{name}.svelte';"))
        .collect::<Vec<_>>()
        .join("\n");

    // Inject after <script lang="ts"> line
    if let Some(script_pos) = content.find("<script") {
        if let Some(close_pos) = content[script_pos..].find('>') {
            let inject_at = script_pos + close_pos + 1;
            let mut result = String::with_capacity(content.len() + imports.len() + 2);
            result.push_str(&content[..inject_at]);
            result.push('\n');
            result.push_str(&imports);
            result.push_str(&content[inject_at..]);
            return result;
        }
    }

    content.to_string()
}

// ─── Developer Overlay: Provenance Stamping ──────────────────────────────────

/// True when the `developer` (edit-mode) layer is active in this registry.
/// Stamping is a pure property of the layer being present — the runtime injects
/// it only in preview/developer mode, so a normal build never stamps.
fn developer_layer_active(registry: &LayerRegistry) -> bool {
    registry.layers.iter().any(|l| l == "developer")
}

/// Project slug for provenance, from `VEIL_PROJECT_SLUG` (set by the runtime
/// preview builder) or a neutral default. Generic — no hardcoded project.
fn provenance_project_slug() -> String {
    std::env::var("VEIL_PROJECT_SLUG").unwrap_or_else(|_| "project".to_string())
}

/// Stamp provenance attributes onto emitted Svelte/HTML markup so a clicked or
/// selected DOM node maps back to its authoring VEIL construct.
///
/// Adds, to each top-level element open tag in the `template` region:
/// - `data-veil-project` — project slug
/// - `data-veil-construct` — the construct name
/// - `data-veil-el` — a 0-based element hint (order within the construct)
///
/// Deliberately conservative: only stamps plain lowercase HTML element open
/// tags (`<div`, `<section`, …), skipping Svelte control blocks (`{#if}`),
/// components (`<Header`), closing tags, comments, and already-stamped tags.
/// This is v1 string-level stamping over raw template blocks; the structured
/// HTML/CSS lowering is the future home (palace: structured-html-css-targets).
fn stamp_provenance(content: &str, c: &Construct, project: &str) -> String {
    let construct = &c.name;

    // Locate the template region: the markup after the closing </script> (and
    // any {{...}}-free area). Stamp within [start, style_start) so we do not
    // touch <script> or <style> blocks.
    let start = content
        .find("</script>")
        .map(|i| i + "</script>".len())
        .unwrap_or(0);
    let style_start = content[start..]
        .find("<style")
        .map(|i| start + i)
        .unwrap_or(content.len());

    let head = &content[..start];
    let body = &content[start..style_start];
    let tail = &content[style_start..];

    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len() + 96);
    let mut i = 0;
    let mut el_index = 0usize;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'<' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_lowercase() {
            // Plain lowercase HTML element open tag. Extract tag name (ASCII).
            let name_start = i + 1;
            let mut j = name_start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-') {
                j += 1;
            }
            out.push('<');
            out.push_str(&body[name_start..j]);
            out.push_str(&format!(
                " data-veil-project=\"{}\" data-veil-construct=\"{}\" data-veil-el=\"{}\"",
                escape_attr(project),
                escape_attr(construct),
                el_index
            ));
            el_index += 1;
            i = j;
            continue;
        }
        // Advance by one full UTF-8 char to stay on a boundary.
        let width = utf8_char_width(ch);
        out.push_str(&body[i..i + width]);
        i += width;
    }

    format!("{}{}{}", head, out, tail)
}

/// Byte-width of a UTF-8 sequence given its leading byte.
fn utf8_char_width(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// Minimal HTML-attribute escaping for provenance values.
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

/// Process `{{#if name}}...{{/if}}` conditionals.
fn process_conditionals(input: &str, name: &str, condition: bool) -> String {
    let open_tag = format!("{{{{#if {}}}}}", name);
    let close_tag = format!("{{{{/if}}}}");

    let mut result = input.to_string();
    while let Some(start) = result.find(&open_tag) {
        let after_open = start + open_tag.len();
        if let Some(end_offset) = result[after_open..].find(&close_tag) {
            let end = after_open + end_offset;
            let inner = &result[after_open..end].to_string();
            let replacement = if condition { inner.clone() } else { String::new() };
            result = format!("{}{}{}", &result[..start], replacement, &result[end + close_tag.len()..]);
        } else {
            break;
        }
    }
    result
}

/// Process `{{for field in block_name}}...{{end}}` loops.
fn process_for_loop(input: &str, block_name: &str, fields: &[(String, String, String)]) -> String {
    let open_tag = format!("{{{{for field in {}}}}}", block_name);
    let close_tag = "{{end}}";

    let mut result = input.to_string();
    while let Some(start) = result.find(&open_tag) {
        let after_open = start + open_tag.len();
        if let Some(end_offset) = result[after_open..].find(close_tag) {
            let end = after_open + end_offset;
            let body_template = result[after_open..end].to_string();

            let mut expanded = String::new();
            for (name, ty, default) in fields {
                let mut line = body_template.clone();
                line = line.replace("{{field.name}}", name);
                line = line.replace("{{field.type}}", ty);
                line = line.replace("{{field.default}}", default);
                expanded.push_str(&line);
            }

            result = format!("{}{}{}", &result[..start], expanded, &result[end + close_tag.len()..]);
        } else {
            break;
        }
    }
    result
}

/// Collect fields from a named block as (name, type_string, default_string) tuples.
fn collect_block_fields(c: &Construct, block_keyword: &str) -> Vec<(String, String, String)> {
    let default_ctx = GenCtx::new(std::collections::HashMap::new());
    c.blocks.iter()
        .filter(|b| b.keyword == block_keyword)
        .flat_map(|b| b.fields.iter())
        .map(|f| {
            let ty = match &f.type_expr {
                TypeExpr::Named(n) if n.is_empty() => infer_field_type_ts(&f.name),
                ty => type_to_ts(ty),
            };
            let default = f.default_expr.as_ref()
                .map(|e| emit_ts(&lower_to_ts(e, &default_ctx)))
                .unwrap_or_default();
            (f.name.clone(), ty, default)
        })
        .collect()
}

/// Determine output path for a construct based on layer metadata.
///
/// First checks if the layer's ConstructSpec provides an `output_path` key in
/// `lowers_to`, which can contain `{{name}}` and `{{route}}` placeholders.
/// Falls back to a generic pattern based on construct subkind/keyword.
fn construct_output_path(c: &Construct, registry: &LayerRegistry) -> String {
    // Check for layer-provided output path template
    if let Some(path_template) = registry.construct_lowers_to(c, "output_path") {
        let route = extract_route_segment(c, registry);
        let name_snake = to_snake_from_name(&c.name);
        return path_template
            .replace("{{name}}", &c.name)
            .replace("{{name_snake}}", &name_snake)
            .replace("{{route}}", &route);
    }

    // Generic fallback — derive from construct keyword
    let sk = c.subkind.to_lowercase();
    let kw = c.keyword.to_lowercase();
    let route = extract_route_segment(c, registry);

    match (sk.as_str(), kw.as_str()) {
        ("page", _) | (_, "page") => {
            if route.is_empty() || route == "/" {
                format!("src/routes/+page.{}", construct_file_extension(c, registry))
            } else {
                format!("src/routes/{}/+page.{}", route, construct_file_extension(c, registry))
            }
        }
        ("layout", _) | (_, "layout") => {
            if route.is_empty() {
                format!("src/routes/+layout.{}", construct_file_extension(c, registry))
            } else {
                format!("src/routes/{}/+layout.{}", route, construct_file_extension(c, registry))
            }
        }
        ("store", _) | (_, "store") => {
            let name = to_snake_from_name(&c.name);
            format!("src/lib/stores/{}.ts", name)
        }
        _ => {
            // Generic component
            format!("src/lib/components/{}.ts", c.name)
        }
    }
}

/// Extract route segment from annotations or derive from name.
fn extract_route_segment(c: &Construct, registry: &LayerRegistry) -> String {
    c.annotations.iter()
        .find(|a| a.name == "route" || registry.annotation_has_role(&a.name, "ui_route"))
        .and_then(|a| a.args.first())
        .map(|s| s.trim_matches('"').trim_matches('/').to_string())
        .unwrap_or_else(|| to_kebab_from_name(&c.name))
}

/// Determine file extension from layer or fall back to generic "ts".
fn construct_file_extension(_c: &Construct, _registry: &LayerRegistry) -> &'static str {
    // The layer's lowers_to output_path should be the canonical source.
    // When no output_path is provided, use a generic extension.
    "ts"
}

/// Convert a PascalCase name to a kebab-case route segment.
fn to_kebab_from_name(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('/');
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c.to_lowercase().next().unwrap());
        }
    }
    result
}

/// Convert a PascalCase name to snake_case.
fn to_snake_from_name(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap());
    }
    result
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Collect constructs of a given shape from a module tree.
fn collect_shape(module: &Construct, shape: Shape) -> Vec<&Construct> {
    let mut result = Vec::new();
    fn walk<'a>(c: &'a Construct, shape: Shape, result: &mut Vec<&'a Construct>) {
        for child in &c.children {
            if child.shape == shape {
                result.push(child);
            }
            if child.shape == Shape::Group || child.shape == Shape::Mod {
                walk(child, shape, result);
            }
        }
    }
    walk(module, shape, &mut result);
    result
}

/// Collect type names (structs + enums) for qualified import generation.
fn collect_type_names_set(modules: &[&Construct]) -> std::collections::HashSet<String> {
    let mut types = std::collections::HashSet::new();
    for module in modules {
        for s in collect_shape(module, Shape::Struct) {
            if !s.layer_provided {
                types.insert(s.name.clone());
            }
        }
        for e in collect_shape(module, Shape::Enum) {
            if !e.layer_provided {
                types.insert(e.name.clone());
            }
        }
    }
    types
}

/// Qualify a named type as `T.Book` when it is a generated export.
fn type_to_ts_qualified(ty: &TypeExpr, type_names: &std::collections::HashSet<String>) -> String {
    match ty {
        TypeExpr::Named(name) => {
            if type_names.contains(name) {
                format!("T.{}", name)
            } else {
                type_to_ts(ty)
            }
        }
        TypeExpr::Optional(inner) => {
            format!("{} | null", type_to_ts_qualified(inner, type_names))
        }
        TypeExpr::List(inner) => {
            format!("{}[]", type_to_ts_qualified(inner, type_names))
        }
        TypeExpr::Result(Some(inner)) => {
            format!("Promise<{}>", type_to_ts_qualified(inner, type_names))
        }
        TypeExpr::Result(None) => "Promise<void>".to_string(),
        _ => type_to_ts(ty),
    }
}

/// Format generic type parameters for TypeScript: `<T, U>` or empty string.
fn generic_params_ts(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Field type as TS string, using explicit type or inferring from name.
fn field_type_ts(field: &Field) -> String {
    match &field.type_expr {
        TypeExpr::Named(n) if n.is_empty() => infer_field_type_ts(&field.name),
        ty => type_to_ts(ty),
    }
}

/// Convert PascalCase/snake_case to kebab-case for package names.
fn to_kebab(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('-');
            }
            result.push(c.to_lowercase().next().unwrap());
        } else if c == '_' {
            result.push('-');
        } else {
            result.push(c);
        }
    }
    result
}

/// Emit a VEIL expression as a TypeScript *value* string (for `$derived(expr)`).
///
/// `expr_to_typescript` is a statement emitter — for a bare expression it yields
/// `<expr>;` at the given indent. We reuse it and strip the leading indent and a
/// trailing semicolon so the result nests inside the derived_line pattern.
fn expr_as_ts_value(expr: &Expr, opts: &super::expr_emit::TsExprEmitOpts) -> String {
    let rendered = super::expr_emit::emit_expr_value_with(expr, 0, opts);
    let trimmed = rendered.trim();
    trimmed.strip_suffix(';').unwrap_or(trimmed).to_string()
}

/// Convert a VEIL TypeExpr to a TypeScript type string.
fn store_fn_opts(c: &Construct) -> super::expr_emit::TsExprEmitOpts {
    let is_store = c.keyword.eq_ignore_ascii_case("store")
        || c.subkind.eq_ignore_ascii_case("Store");
    if !is_store {
        return super::expr_emit::TsExprEmitOpts::default();
    }
    let fields = c.blocks.iter().filter(|b| b.keyword == "state").flat_map(|b| {
        b.fields.iter().map(|f| f.name.clone())
    });
    super::expr_emit::TsExprEmitOpts::for_store(&c.name, fields)
}

fn ts_type_for_field(ty: &veil_ir::TypeExpr) -> String {
    use veil_ir::TypeExpr;
    match ty {
        TypeExpr::Named(n) => match n.as_str() {
            "Str" | "String" => "string".into(),
            "Bool" => "boolean".into(),
            "Int" | "F64" | "Float" => "number".into(),
            "Json" => "any".into(),
            "Id" | "UUID" => "string".into(),
            "Dt" | "DateTime" => "string".into(),
            other => other.to_string(),
        },
        TypeExpr::List(inner) => format!("{}[]", ts_type_for_field(inner)),
        TypeExpr::Optional(inner) => format!("{} | null", ts_type_for_field(inner)),
        TypeExpr::Map(_, v) => format!("Record<string, {}>", ts_type_for_field(v)),
        _ => "any".into(),
    }
}

/// Get a sensible default value for a TypeExpr in TypeScript.
fn ts_default_for_type(ty: &veil_ir::TypeExpr) -> String {
    use veil_ir::TypeExpr;
    match ty {
        TypeExpr::Named(n) => match n.as_str() {
            "Str" | "String" => "''".into(),
            "Bool" => "false".into(),
            "Int" | "F64" | "Float" => "0".into(),
            "Json" => "{}".into(),
            _ => "undefined as any".into(),
        },
        TypeExpr::List(_) => "[]".into(),
        TypeExpr::Map(_, _) => "{}".into(),
        TypeExpr::Optional(_) => "null".into(),
        _ => "undefined as any".into(),
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::*;
    use veil_ir::ast::Construct;

    fn construct(name: &str) -> Construct {
        Construct::new("comp", "Component", Shape::Struct, name.to_string(), Default::default())
    }

    #[test]
    fn stamps_html_open_tags_with_provenance() {
        let content = "<script lang=\"ts\">\n  let x = 1;\n</script>\n\n<div class=\"a\">\n  <span>hi</span>\n</div>\n\n<style>\n  div { color: red }\n</style>\n";
        let out = stamp_provenance(content, &construct("AgentsPage"), "agent-core");
        // Each HTML open tag carries the full provenance triple.
        assert!(out.contains("<div data-veil-project=\"agent-core\" data-veil-construct=\"AgentsPage\" data-veil-el=\"0\" class=\"a\">"), "div not stamped:\n{out}");
        assert!(out.contains("<span data-veil-project=\"agent-core\" data-veil-construct=\"AgentsPage\" data-veil-el=\"1\">"), "span not stamped:\n{out}");
        // <script> and <style> regions are never stamped.
        assert!(out.contains("<script lang=\"ts\">"), "script tag was mutated:\n{out}");
        assert!(out.contains("div { color: red }"), "style body mutated:\n{out}");
    }

    #[test]
    fn skips_components_control_blocks_and_closing_tags() {
        let content = "<script></script>\n<Header />\n{#if ok}\n<p>x</p>\n{/if}\n";
        let out = stamp_provenance(content, &construct("Home"), "proj");
        // Component tags (PascalCase) are not stamped.
        assert!(!out.contains("<Header data-veil"), "component wrongly stamped:\n{out}");
        // Svelte control blocks untouched.
        assert!(out.contains("{#if ok}"), "control block mutated:\n{out}");
        // Plain <p> IS stamped; its closing tag </p> is not.
        assert!(out.contains("<p data-veil-project=\"proj\" data-veil-construct=\"Home\""), "p not stamped:\n{out}");
        assert!(!out.contains("</p data-veil"), "closing tag stamped:\n{out}");
    }

    #[test]
    fn utf8_content_is_preserved() {
        let content = "<script></script>\n<button>Café ☕ 日本語</button>\n";
        let out = stamp_provenance(content, &construct("Menu"), "proj");
        assert!(out.contains("Café ☕ 日本語"), "utf8 mangled:\n{out}");
        assert!(out.contains("<button data-veil-project=\"proj\" data-veil-construct=\"Menu\""), "button not stamped:\n{out}");
    }

    #[test]
    fn developer_layer_active_reflects_registry() {
        let mut reg = LayerRegistry::builtin();
        assert!(!developer_layer_active(&reg), "should be inactive by default");
        reg.layers.push("developer".to_string());
        assert!(developer_layer_active(&reg), "should be active when layer present");
    }
}

#[cfg(test)]
mod index_mount_contract_tests {
    use super::*;

    #[test]
    fn index_without_components_is_barrel_only() {
        let file = gen_index_ir("some-lib", &[]);
        assert_eq!(file.path, "src/index.ts");
        assert!(file.content.contains("export * from './types';"));
        assert!(file.content.contains("export * from './services';"));
        // No components → no framework import, no mount contract.
        assert!(!file.content.contains("from 'svelte'"), "bare lib leaked svelte import:\n{}", file.content);
        assert!(!file.content.contains("export function mount"), "bare lib emitted mount:\n{}", file.content);
    }

    #[test]
    fn index_emits_framework_neutral_mount_contract() {
        let components = vec!["AgentsPage".to_string(), "TeamsPage".to_string()];
        let file = gen_index_ir("agent-core", &components);
        let c = &file.content;

        // Barrels preserved.
        assert!(c.contains("export * from './types';"), "missing types barrel:\n{c}");

        // Each component is imported (default import) so the bundle is self-contained.
        assert!(
            c.contains("import AgentsPage from './lib/components/AgentsPage.svelte';"),
            "AgentsPage not imported:\n{c}"
        );
        assert!(
            c.contains("import TeamsPage from './lib/components/TeamsPage.svelte';"),
            "TeamsPage not imported:\n{c}"
        );

        // Named exports kept (harmless direct-consumer path).
        assert!(c.contains("export { AgentsPage };"), "AgentsPage named export missing:\n{c}");
        assert!(c.contains("export { TeamsPage };"), "TeamsPage named export missing:\n{c}");

        // exportName -> component registry.
        assert!(c.contains("const __components: Record<string, any> = {"), "component registry missing:\n{c}");
        assert!(c.contains("  AgentsPage,"), "AgentsPage not in registry:\n{c}");
        assert!(c.contains("  TeamsPage,"), "TeamsPage not in registry:\n{c}");

        // The neutral mount contract: mount(exportName, target, props) => () => void.
        assert!(c.contains("export function mount("), "mount fn missing:\n{c}");
        assert!(c.contains("exportName: string,"), "mount exportName param missing:\n{c}");
        assert!(c.contains("target: HTMLElement,"), "mount target param missing:\n{c}");
        assert!(c.contains("props: { context?: any; params?: any }"), "mount props param shape wrong:\n{c}");
        assert!(c.contains("): () => void {"), "mount return type (teardown) wrong:\n{c}");

        // Svelte target implements the contract via svelte mount/unmount — the ONLY
        // place svelte appears; the harness never imports it.
        assert!(
            c.contains("import { mount as __svelteMount, unmount as __svelteUnmount } from 'svelte';"),
            "svelte impl import missing:\n{c}"
        );
        assert!(c.contains("__svelteMount(Component, { target, props })"), "svelte mount call missing:\n{c}");
        assert!(c.contains("__svelteUnmount(instance)"), "svelte unmount call missing:\n{c}");

        // Missing-export error surfaces (harness maps this to "Failed to load component").
        assert!(
            c.contains("not found in contribution bundle"),
            "missing-export error not emitted:\n{c}"
        );
    }
}
