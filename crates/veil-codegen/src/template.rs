//! Template execution engine — evaluates layer-declared codegen templates
//! against the full IR to produce target-language output.
//!
//! The engine has NO domain knowledge. It executes templates from the
//! LayerRegistry, matching them against constructs in the Solution AST.

use std::collections::HashMap;

use veil_ir::ast::{Construct, FlowStep, Solution, Field};
use veil_ir::builder::{expr_to_display, type_to_display};
use veil_ir::layer::{CodegenRule, CodegenTemplate, LayerRegistry};

/// Result of executing all templates for a target.
pub struct TemplateOutput {
    /// Files emitted directly (path → content).
    pub files: Vec<TemplateFile>,
    /// Named sections (section name → ordered contributions).
    pub sections: HashMap<String, Vec<SectionContribution>>,
    /// Per-construct inline contributions (emit without emit_to or emit_file).
    /// Key is construct name, value is ordered contributions.
    pub inline: HashMap<String, Vec<SectionContribution>>,
}

pub struct TemplateFile {
    pub path: String,
    pub content: String,
}

pub struct SectionContribution {
    pub priority: u32,
    pub content: String,
    pub source_layer: String,
    pub source_rule: String,
}

/// Execute all codegen templates for the given target against the solution.
pub fn execute_templates(
    solution: &Solution,
    registry: &LayerRegistry,
    target: &str,
) -> TemplateOutput {
    let mut sections: HashMap<String, Vec<SectionContribution>> = HashMap::new();
    let mut files: Vec<TemplateFile> = Vec::new();
    let mut inline: HashMap<String, Vec<SectionContribution>> = HashMap::new();

    // Collect all templates for this target
    let templates: Vec<&CodegenTemplate> = registry
        .codegen_templates
        .iter()
        .filter(|t| t.target == target)
        .collect();

    if templates.is_empty() {
        return TemplateOutput {
            files: Vec::new(),
            sections: HashMap::new(),
            inline: HashMap::new(),
        };
    }

    // Emit scaffold files from all matching templates.
    for tpl in &templates {
        for sf in &tpl.scaffold {
            files.push(TemplateFile {
                path: sf.path.clone(),
                content: sf.content.clone(),
            });
        }
    }

    // GEN-005: walk nested constructs (not only top-level items).
    fn visit_construct(
        construct: &Construct,
        templates: &[&CodegenTemplate],
        registry: &LayerRegistry,
        target: &str,
        sections: &mut HashMap<String, Vec<SectionContribution>>,
        files: &mut Vec<TemplateFile>,
        inline: &mut HashMap<String, Vec<SectionContribution>>,
    ) {
        for template in templates {
            for rule in &template.rules {
                if matches_construct(construct, rule, registry) {
                    let output = render_template(construct, rule, registry, target);

                    if let Some(ref file_pattern) = rule.emit_file {
                        // Emit to a specific file path (expand pattern).
                        // Later emits for the same path replace earlier ones
                        // (e.g. scaffold vite.config.ts → @proxy override).
                        let path = expand_file_path(file_pattern, construct, registry);
                        if let Some(existing) = files.iter_mut().find(|f| f.path == path) {
                            existing.content = output;
                        } else {
                            files.push(TemplateFile {
                                path,
                                content: output,
                            });
                        }
                    } else if let Some(section_name) = &rule.emit_to {
                        sections
                            .entry(section_name.clone())
                            .or_default()
                            .push(SectionContribution {
                                priority: rule.priority,
                                content: output,
                                source_layer: template.layer.clone(),
                                source_rule: format!(
                                    "match {} where {}",
                                    rule.match_shape, rule.condition
                                ),
                            });
                    } else {
                        inline
                            .entry(construct.name.clone())
                            .or_default()
                            .push(SectionContribution {
                                priority: rule.priority,
                                content: output,
                                source_layer: template.layer.clone(),
                                source_rule: format!(
                                    "match {} where {}",
                                    rule.match_shape, rule.condition
                                ),
                            });
                    }
                }
            }
        }
        for child in &construct.children {
            visit_construct(child, templates, registry, target, sections, files, inline);
        }
    }

    for item in &solution.items {
        if let veil_ir::ast::TopLevelItem::Construct(c) = item {
            visit_construct(
                c,
                &templates,
                registry,
                target,
                &mut sections,
                &mut files,
                &mut inline,
            );
        }
    }

    // Sort section contributions by priority
    for contributions in sections.values_mut() {
        contributions.sort_by_key(|c| c.priority);
    }

    // Sort inline contributions by priority
    for contributions in inline.values_mut() {
        contributions.sort_by_key(|c| c.priority);
    }

    TemplateOutput { files, sections, inline }
}

/// Compose the "main" section into a complete main function (target-specific).
pub fn compose_main_section(output: &TemplateOutput, target: &str) -> Option<String> {
    let contributions = output.sections.get("main")?;
    if contributions.is_empty() {
        return None;
    }

    let body: String = contributions
        .iter()
        .map(|c| c.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    match target {
        "rust" => Some(format!(
            "#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n{}\n    Ok(())\n}}",
            body
        )),
        "typescript" => Some(format!(
            "async function main() {{\n{}\n}}\n\nmain().catch(console.error);",
            body
        )),
        _ => Some(format!("// main\n{}", body)),
    }
}

/// Compose a named section from template output. Returns the combined content
/// from all layer contributions (sorted by priority — lower runs first), or
/// None if no contributions exist for the section.
///
/// This is the primary mechanism for layers to influence code emission: a layer
/// declares `emit_to "derives"` and the backend calls `compose_section("derives")`
/// before falling back to its hardcoded defaults.
pub fn compose_section(output: &TemplateOutput, section: &str) -> Option<String> {
    let contributions = output.sections.get(section)?;
    if contributions.is_empty() {
        return None;
    }
    // Contributions are sorted by priority ascending. Highest priority wins —
    // take the last contribution (highest priority). When multiple
    // contributions share the same highest priority, take the first of that
    // tier (they should all be identical — e.g. derives from the same rule
    // matching multiple constructs).
    let highest = contributions.last()?;
    Some(highest.content.trim().to_string())
}

/// Compose inline contributions for a specific construct. Returns the combined
/// content from all layer contributions (sorted by priority — lower runs first),
/// or None if no contributions exist for this construct.
///
/// Unlike `compose_section` which picks the highest-priority winner, inline
/// contributions are ALL emitted (they represent distinct impl blocks, trait
/// impls, etc. that layers inject after a construct's primary body).
pub fn compose_inline(output: &TemplateOutput, construct_name: &str) -> Option<String> {
    let contributions = output.inline.get(construct_name)?;
    if contributions.is_empty() {
        return None;
    }
    let combined: String = contributions
        .iter()
        .map(|c| c.content.trim().to_string())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(combined)
}

/// Check if a construct matches a rule's conditions.
fn matches_construct(construct: &Construct, rule: &CodegenRule, registry: &LayerRegistry) -> bool {
    // Check shape match — accept shape name, "*", or layer subkind.
    let shape_name = construct.shape.name();
    let subkind = &construct.subkind;
    if rule.match_shape != shape_name
        && rule.match_shape != "*"
        && !rule.match_shape.eq_ignore_ascii_case(subkind)
        && !rule.match_shape.eq_ignore_ascii_case(&construct.keyword)
    {
        return false;
    }

    // Check condition
    if rule.condition.is_empty() {
        return true; // No condition = match all of this shape
    }

    // Prefer role-based matching (INV-001) over bare annotation names.
    //   has_role("dependency") — any annotation carrying that policy role
    //   has_annotation("dep")  — literal annotation name (layer self-reference)
    if rule.condition.starts_with("has_role(") {
        let role = extract_quoted_arg(&rule.condition, "has_role");
        if let Some(role) = role {
            return construct
                .annotations
                .iter()
                .any(|a| registry.annotation_has_role(&a.name, &role));
        }
    }

    if rule.condition.starts_with("has_annotation(") {
        // Extract annotation name from: has_annotation("dep")
        let ann_name = extract_quoted_arg(&rule.condition, "has_annotation");
        if let Some(name) = ann_name {
            return construct.annotations.iter().any(|a| a.name == name);
        }
    }

    if rule.condition.starts_with("subkind == ") {
        let target_subkind = extract_quoted_value(&rule.condition, "subkind == ");
        if let Some(sk) = target_subkind {
            return construct.subkind.eq_ignore_ascii_case(&sk);
        }
    }

    // Unknown condition — don't match
    false
}

/// Expand a file path pattern with construct properties.
///
/// Supported placeholders:
/// - `{{name}}` — construct name (as-is)
/// - `{{name_lower}}` — lowercase construct name
/// - `{{route}}` — UI (`role:ui_route`) or leftover HTTP route path, slash-stripped for files
/// - `{{subkind}}` — layer subkind
fn expand_file_path(pattern: &str, construct: &Construct, registry: &LayerRegistry) -> String {
    let name = &construct.name;
    let name_lower = name.to_lowercase();
    let route = route_file_segment(&construct_route_url(construct, registry));

    collapse_duplicate_slashes(
        &pattern
            .replace("{{name}}", name)
            .replace("{{name_lower}}", &name_lower)
            .replace("{{route}}", &route)
            .replace("{{subkind}}", &construct.subkind),
    )
}

/// Render a template body with interpolation against a construct.
fn render_template(construct: &Construct, rule: &CodegenRule, registry: &LayerRegistry, target: &str) -> String {
    let mut output = rule.emit_body.clone();

    // Simple interpolations
    output = output.replace("{{name}}", &construct.name);
    output = output.replace("{{subkind}}", &construct.subkind);
    output = output.replace("{{keyword}}", &construct.keyword);

    // {{route}} — role:ui_route (svelte page/layout) or leftover role:http_route
    let route_val = construct_route_url(construct, registry);
    output = output.replace("{{route}}", &route_val);

    // Generic annotation args (zero domain knowledge — any layer annotation):
    //   {{annotation_value:name}}     → first arg of @name
    //   {{annotation_arg:name:N}}     → Nth arg (0-based) of @name
    //   {{annotation_value("name")}}  → same as annotation_value:name
    //   {{annotation_arg("name", N)}} → same as annotation_arg:name:N
    output = expand_annotation_placeholders(construct, output);

    // {{raw_block:template}} — content of a named raw block (e.g. template, style)
    while let Some(start) = output.find("{{raw_block:") {
        let end = output[start..].find("}}").unwrap_or(output.len()) + start + 2;
        let block_name = &output[start + 12..end - 2];
        let block_content = construct
            .raw_blocks
            .iter()
            .find(|(n, _)| n == block_name)
            .map(|(_, c)| c.as_str())
            .unwrap_or("");
        output = format!("{}{}{}", &output[..start], block_content, &output[end..]);
    }

    // {{props_decl}} — Svelte $props() script from props block
    if output.contains("{{props_decl}}") {
        let props_block = construct.blocks.iter().find(|b| b.keyword == "props");
        let props_script = if let Some(props) = props_block {
            let mut s = String::new();
            s.push_str("  interface Props {\n");
            for field in &props.fields {
                let ty = svelte_type_display(&field.type_expr);
                s.push_str(&format!("    {}: {};\n", field.name, ty));
            }
            s.push_str("  }\n");
            let names: Vec<&str> = props.fields.iter().map(|f| f.name.as_str()).collect();
            if !names.is_empty() {
                s.push_str(&format!("  let {{ {} }}: Props = $props();\n", names.join(", ")));
            } else {
                s.push_str("  let {}: Props = $props();\n");
            }
            s
        } else {
            String::new()
        };
        output = output.replace("{{props_decl}}", &props_script);
    }

    // {{state_decl}} — Svelte 5 $state() fields from state block
    if output.contains("{{state_decl}}") {
        let state_block = construct.blocks.iter().find(|b| b.keyword == "state");
        let state_script = if let Some(state) = state_block {
            let mut s = String::new();
            for field in &state.fields {
                let default = svelte_state_default(&field.type_expr);
                s.push_str(&format!(
                    "  let {}: {} = $state({});\n",
                    field.name,
                    svelte_type_display(&field.type_expr),
                    default
                ));
            }
            s
        } else {
            String::new()
        };
        output = output.replace("{{state_decl}}", &state_script);
    }

    // {{fn_declarations}} — exported functions from construct.fns (target-aware).
    if output.contains("{{fn_declarations}}") {
        let fn_script = if !construct.fns.is_empty() {
            let mut s = String::new();
            for f in &construct.fns {
                // Skip raw block fns (template, style, script)
                if f.name == "template" || f.name == "style" || f.name == "script" {
                    continue;
                }
                match target {
                    "typescript" => {
                        let params = f.params.iter()
                            .map(|p| format!("{}: {}", p.name, svelte_type_display(&p.type_expr)))
                            .collect::<Vec<_>>().join(", ");
                        let is_async = f.return_type.as_ref()
                            .map(|t| matches!(t, veil_ir::TypeExpr::Result(_)))
                            .unwrap_or(false);
                        let ret_type = match &f.return_type {
                            Some(veil_ir::TypeExpr::Result(Some(inner))) =>
                                format!(": Promise<{}>", svelte_type_display(inner)),
                            Some(veil_ir::TypeExpr::Result(None)) => ": Promise<void>".into(),
                            Some(ty) => format!(": {}", svelte_type_display(ty)),
                            None => String::new(),
                        };
                        let async_kw = if is_async { "async " } else { "" };
                        s.push_str(&format!(
                            "export {}function {}({}){} {{\n",
                            async_kw, f.name, params, ret_type
                        ));
                        for expr in &f.body {
                            s.push_str(&format!("  {};\n", expr_to_display(expr)));
                        }
                        s.push_str("}\n\n");
                    }
                    "rust" => {
                        let params = f.params.iter()
                            .map(|p| format!("{}: {}", p.name, type_to_display(&p.type_expr)))
                            .collect::<Vec<_>>().join(", ");
                        let is_async = f.return_type.as_ref()
                            .map(|t| matches!(t, veil_ir::TypeExpr::Result(_)))
                            .unwrap_or(false);
                        let ret_type = match &f.return_type {
                            Some(ty) => format!(" -> {}", type_to_display(ty)),
                            None => String::new(),
                        };
                        let async_kw = if is_async { "async " } else { "" };
                        s.push_str(&format!(
                            "pub {}fn {}({}){} {{\n",
                            async_kw, f.name, params, ret_type
                        ));
                        for expr in &f.body {
                            s.push_str(&format!("    {};\n", expr_to_display(expr)));
                        }
                        s.push_str("}\n\n");
                    }
                    _ => {
                        // Generic fallback
                        let params = f.params.iter()
                            .map(|p| format!("{}: {}", p.name, type_to_display(&p.type_expr)))
                            .collect::<Vec<_>>().join(", ");
                        s.push_str(&format!("function {}({}) {{\n", f.name, params));
                        for expr in &f.body {
                            s.push_str(&format!("  {};\n", expr_to_display(expr)));
                        }
                        s.push_str("}\n\n");
                    }
                }
            }
            s
        } else {
            String::new()
        };
        output = output.replace("{{fn_declarations}}", &fn_script);
    }

    // {{state_exports}} — export statement for state fields (e.g. `export { field1, field2 };`)
    if output.contains("{{state_exports}}") {
        let state_block = construct.blocks.iter().find(|b| b.keyword == "state");
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
        output = output.replace("{{state_exports}}", &exports);
    }

    // {{children_names}} — comma-separated child construct names
    if output.contains("{{children_names}}") {
        let names: Vec<&str> = construct.children.iter().map(|c| c.name.as_str()).collect();
        output = output.replace("{{children_names}}", &names.join(", "));
    }

    // {{imports}} — auto-detect PascalCase component references in template
    // and emit import statements for them.
    if output.contains("{{imports}}") {
        let template_content = construct
            .raw_blocks
            .iter()
            .find(|(n, _)| n == "template")
            .map(|(_, c)| c.as_str())
            .unwrap_or("");
        let mut imports = Vec::new();
        let chars: Vec<char> = template_content.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '<' && i + 1 < chars.len() && chars[i + 1].is_uppercase() {
                i += 1;
                let mut comp_name = String::new();
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    comp_name.push(chars[i]);
                    i += 1;
                }
                if !comp_name.is_empty()
                    && comp_name != construct.name
                    && !imports.contains(&comp_name)
                {
                    imports.push(comp_name);
                }
            } else {
                i += 1;
            }
        }
        let import_stmts = imports
            .iter()
            .map(|name| format!("  import {name} from '$lib/components/{name}.svelte';"))
            .collect::<Vec<_>>()
            .join("\n");
        output = output.replace("{{imports}}", &import_stmts);
    }

    // Handle {{for field in dep_fields}}...{{end}}
    // INV-001: dependency-role fields via registry; construct-level dependency
    // annotation means all fields are injectable (di.layer pattern).
    if output.contains("{{for field in dep_fields}}") {
        let dep_fields: Vec<&Field> = {
            let field_level: Vec<&Field> = construct
                .fields
                .iter()
                .filter(|f| registry.field_is_dependency(f))
                .collect();
            if !field_level.is_empty() {
                field_level
            } else if construct
                .annotations
                .iter()
                .any(|a| registry.is_dependency_annotation(&a.name))
            {
                construct.fields.iter().collect()
            } else {
                Vec::new()
            }
        };

        output = expand_for_loop(&output, "field", "dep_fields", &dep_fields, |field, var| {
            match var {
                "field.name" => field.name.clone(),
                "field.type" => type_to_display(&field.type_expr),
                _ => format!("{{{{{}}}}}",var),
            }
        });
    }

    // Handle {{for field in fields}}...{{end}}
    if output.contains("{{for field in fields}}") {
        let fields: Vec<&Field> = construct.fields.iter().collect();

        output = expand_for_loop(&output, "field", "fields", &fields, |field, var| {
            match var {
                "field.name" => field.name.clone(),
                "field.type" => type_to_display(&field.type_expr),
                _ => format!("{{{{{}}}}}", var),
            }
        });
    }

    // Handle {{for step in steps}}...{{end}}
    if output.contains("{{for step in steps}}") {
        let steps: Vec<&FlowStep> = construct.steps.iter().collect();

        output = expand_step_loop(&output, &steps);
    }

    output
}

/// URL path for a page/layout (or leftover API route). Prefers `role:ui_route`.
fn construct_route_url(construct: &Construct, registry: &LayerRegistry) -> String {
    registry
        .ui_route_path(construct)
        .or_else(|| {
            registry
                .http_route_annotation(construct)
                .and_then(|a| a.args.first())
                .map(|s| strip_ann_arg(s))
        })
        .unwrap_or_else(|| format!("/{}", construct.name.to_lowercase()))
}

/// File-path segment for sveltekit `src/routes/{{route}}/+page.svelte`.
/// `/` → ``, `/pulls/[id]` → `pulls/[id]`.
fn route_file_segment(route: &str) -> String {
    route.trim().trim_matches('/').to_string()
}

fn collapse_duplicate_slashes(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_slash = false;
    for ch in path.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push(ch);
            }
            prev_slash = true;
        } else {
            prev_slash = false;
            out.push(ch);
        }
    }
    out
}

fn strip_ann_arg(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn annotation_arg_at(construct: &Construct, name: &str, index: usize) -> String {
    construct
        .annotations
        .iter()
        .find(|a| a.name == name)
        .and_then(|a| a.args.get(index))
        .map(|s| strip_ann_arg(s))
        .unwrap_or_default()
}

/// Expand `{{annotation_value:…}}` / `{{annotation_arg:…}}` (and quoted forms).
fn expand_annotation_placeholders(construct: &Construct, mut output: String) -> String {
    // {{annotation_arg:name:N}}
    while let Some(start) = output.find("{{annotation_arg:") {
        let after = start + "{{annotation_arg:".len();
        let Some(end_rel) = output[after..].find("}}") else {
            break;
        };
        let end = after + end_rel;
        let inner = &output[after..end]; // name:N
        let replacement = if let Some((name, idx_s)) = inner.rsplit_once(':') {
            let idx: usize = idx_s.parse().unwrap_or(0);
            annotation_arg_at(construct, name, idx)
        } else {
            String::new()
        };
        output = format!("{}{}{}", &output[..start], replacement, &output[end + 2..]);
    }

    // {{annotation_value:name}}
    while let Some(start) = output.find("{{annotation_value:") {
        let after = start + "{{annotation_value:".len();
        let Some(end_rel) = output[after..].find("}}") else {
            break;
        };
        let end = after + end_rel;
        let name = &output[after..end];
        let replacement = annotation_arg_at(construct, name, 0);
        output = format!("{}{}{}", &output[..start], replacement, &output[end + 2..]);
    }

    // {{annotation_arg("name", N)}}
    while let Some(start) = output.find("{{annotation_arg(\"") {
        let after = start + "{{annotation_arg(\"".len();
        let Some(name_end) = output[after..].find('"') else {
            break;
        };
        let name = &output[after..after + name_end];
        let rest = &output[after + name_end..];
        // ", N)}}
        let Some(close) = rest.find("}}") else {
            break;
        };
        let mid = &rest[..close]; // ", 0) or similar
        let idx: usize = mid
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        let replacement = annotation_arg_at(construct, name, idx);
        let abs_end = after + name_end + close + 2;
        output = format!("{}{}{}", &output[..start], replacement, &output[abs_end..]);
    }

    // {{annotation_value("name")}}
    while let Some(start) = output.find("{{annotation_value(\"") {
        let after = start + "{{annotation_value(\"".len();
        let Some(name_end) = output[after..].find('"') else {
            break;
        };
        let name = &output[after..after + name_end];
        let rest = &output[after + name_end..];
        let Some(close) = rest.find("}}") else {
            break;
        };
        let replacement = annotation_arg_at(construct, name, 0);
        let abs_end = after + name_end + close + 2;
        output = format!("{}{}{}", &output[..start], replacement, &output[abs_end..]);
    }

    output
}

/// Expand a {{for item in collection}}...{{end}} loop.
/// Handles multiple occurrences of the same loop in the template.
fn expand_for_loop<T, F>(
    template: &str,
    item_name: &str,
    collection_name: &str,
    items: &[&T],
    resolver: F,
) -> String
where
    F: Fn(&T, &str) -> String,
{
    let start_tag = format!("{{{{for {} in {}}}}}", item_name, collection_name);
    let end_tag = "{{end}}";
    let mut result = template.to_string();

    // Keep expanding until no more instances of this loop exist
    while let Some(start_idx) = result.find(&start_tag) {
        let after_start = start_idx + start_tag.len();

        let Some(end_idx) = result[after_start..].find(end_tag) else {
            break;
        };
        let end_abs = after_start + end_idx;

        let before = &result[..start_idx];
        let body = &result[after_start..end_abs];
        let after = &result[end_abs + end_tag.len()..];

        // Check for separator
        let (body_clean, separator) = if let Some(sep_idx) = body.find("{{sep ") {
            let sep_end = body[sep_idx..].find("}}").unwrap_or(body.len()) + sep_idx + 2;
            let sep_str = extract_quoted_value(&body[sep_idx..sep_end], "sep ").unwrap_or_default();
            let clean_body = format!("{}{}", &body[..sep_idx], &body[sep_end..]);
            (clean_body, sep_str)
        } else {
            (body.to_string(), String::new())
        };

        let expanded: Vec<String> = items
            .iter()
            .map(|item| {
                let mut item_result = body_clean.clone();
                // Replace all {{item_name.prop}} patterns
                let prefix = format!("{{{{{}.", item_name);
                while let Some(var_start) = item_result.find(&prefix) {
                    let var_end = item_result[var_start..].find("}}").unwrap_or(item_result.len()) + var_start;
                    let var_name = &item_result[var_start + 2..var_end].to_string();
                    let replacement = resolver(item, var_name);
                    item_result = format!("{}{}{}", &item_result[..var_start], replacement, &item_result[var_end + 2..]);
                }
                item_result
            })
            .collect();

        result = format!("{}{}{}", before, expanded.join(&separator), after);
    }

    result
}

/// Expand step loops with nested action iteration.
fn expand_step_loop(template: &str, steps: &[&FlowStep]) -> String {
    let start_tag = "{{for step in steps}}";
    let end_tag = "{{end}}";

    let Some(start_idx) = template.find(start_tag) else {
        return template.to_string();
    };
    let after_start = start_idx + start_tag.len();

    // Find the OUTERMOST end tag for the step loop
    // We need to skip inner {{end}} tags (from nested for loops)
    let mut depth = 1;
    let mut search_pos = after_start;
    let mut end_abs = template.len();
    while search_pos < template.len() {
        if template[search_pos..].starts_with("{{for ") {
            depth += 1;
            search_pos += 6;
        } else if template[search_pos..].starts_with("{{end}}") {
            depth -= 1;
            if depth == 0 {
                end_abs = search_pos;
                break;
            }
            search_pos += 7;
        } else {
            search_pos += 1;
        }
    }

    let before = &template[..start_idx];
    let body = &template[after_start..end_abs];
    let after = &template[end_abs + end_tag.len()..];

    let expanded: Vec<String> = steps
        .iter()
        .filter_map(|step| {
            match step {
                FlowStep::Step(s) => {
                    let mut result = body.to_string();
                    result = result.replace("{{step.name}}", &s.name);

                    // Handle nested {{for action in step.actions}}...{{end}}
                    if result.contains("{{for action in step.actions}}") {
                        let action_start = "{{for action in step.actions}}";
                        let action_end = "{{end}}";
                        if let Some(as_idx) = result.find(action_start) {
                            let as_after = as_idx + action_start.len();
                            if let Some(ae_idx) = result[as_after..].find(action_end) {
                                let ae_abs = as_after + ae_idx;
                                let action_body = result[as_after..ae_abs].to_string();
                                let action_after = result[ae_abs + action_end.len()..].to_string();
                                let action_before = result[..as_idx].to_string();

                                let actions_expanded: Vec<String> = s.body.iter().map(|expr| {
                                    let mut ab = action_body.clone();
                                    let expr_display = expr_to_display(expr);
                                    ab = ab.replace("{{emit_action(action)}}", &format!("    let {};", expr_display));
                                    ab
                                }).collect();

                                result = format!("{}{}{}", action_before, actions_expanded.join("\n"), action_after);
                            }
                        }
                    }

                    Some(result)
                }
                _ => None,
            }
        })
        .collect();

    format!("{}{}{}", before, expanded.join("\n"), after)
}

/// Extract a quoted argument from a function-call-like string.
/// e.g., extract_quoted_arg("has_annotation(\"dep\")", "has_annotation") -> Some("dep")
fn extract_quoted_arg(s: &str, fn_name: &str) -> Option<String> {
    let prefix = format!("{}(\"", fn_name);
    if let Some(start) = s.find(&prefix) {
        let after = &s[start + prefix.len()..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    None
}

/// Extract a quoted value from a comparison-like string.
/// e.g., extract_quoted_value("subkind == \"Screen\"", "subkind == ") -> Some("Screen")
fn extract_quoted_value(s: &str, prefix: &str) -> Option<String> {
    if let Some(start) = s.find(prefix) {
        let after = &s[start + prefix.len()..];
        let after = after.trim();
        if let Some(inner) = after.strip_prefix('"')
            && let Some(end) = inner.find('"') {
                return Some(inner[..end].to_string());
            }
    }
    None
}

fn target_extension(target: &str) -> &str {
    match target {
        "rust" => "rs",
        "typescript" => "ts",
        "swift" => "swift",
        "kotlin" => "kt",
        _ => "txt",
    }
}

/// Default `$state(...)` initializer from a VEIL type.
/// Map VEIL types to TypeScript-ish names for Svelte script blocks.
fn svelte_type_display(ty: &veil_ir::TypeExpr) -> String {
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
        TypeExpr::List(inner) => format!("{}[]", svelte_type_display(inner)),
        TypeExpr::Optional(inner) => format!("{} | null", svelte_type_display(inner)),
        TypeExpr::Map(_, v) => format!("Record<string, {}>", svelte_type_display(v)),
        _ => "any".into(),
    }
}

fn svelte_state_default(ty: &veil_ir::TypeExpr) -> String {
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
