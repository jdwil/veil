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
pub fn compose_main_section(output: &TemplateOutput, target: &str, registry: Option<&veil_ir::layer::LayerRegistry>) -> Option<String> {
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
        "rust" => {
            // Use layer-provided main wrapper if available
            if let Some(reg) = registry {
                if let Some(tpl) = reg.harness_render_templates.get("rust_bin_main_wrapper") {
                    return Some(tpl.replace("{body}", &body));
                }
            }
            Some(format!(
                "fn main() {{\n{}\n}}",
                body
            ))
        },
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
///
/// This is the primary template engine for layer-declared codegen templates.
/// It supports:
/// - Simple variable interpolation: `{{name}}`, `{{subkind}}`, etc.
/// - Conditionals: `{{if CONDITION}}...{{else}}...{{end}}`
/// - Child iteration: `{{for child in children}}` with optional `where` filters
/// - Field iteration: `{{for field in fields}}` with type introspection
/// - Method iteration: `{{for method in methods}}` with params/return_type
/// - Helper variables: `{{name_snake}}`, `{{name_camel}}`, `{{count_children}}`
/// - Error model access: `{{error_type}}`, `{{error_variant("role")}}`
/// - Nested template resolution: `{{child.lowers_to}}`
fn render_template(construct: &Construct, rule: &CodegenRule, registry: &LayerRegistry, target: &str) -> String {
    render_template_for_construct(construct, &rule.emit_body, registry, target)
}

/// Inner template rendering against a specific construct. Factored out so that
/// child-iteration loops can recursively render templates against child constructs.
fn render_template_for_construct(
    construct: &Construct,
    template_body: &str,
    registry: &LayerRegistry,
    target: &str,
) -> String {
    let mut output = template_body.to_string();

    // ─── Phase 1: Conditionals ───────────────────────────────────────────────
    // Process {{if ...}}...{{else}}...{{end}} blocks from innermost outward.
    output = expand_conditionals(&output, construct, registry, target);

    // ─── Phase 2: Iteration (for loops) ──────────────────────────────────────
    // Child iteration: {{for child in children}}...{{end}} with optional where clause
    output = expand_child_loops(&output, construct, registry, target);

    // Method iteration: {{for method in methods}}...{{end}}
    output = expand_method_loops(&output, construct, target);

    // Field iteration (enhanced): {{for field in fields}}...{{end}} with type introspection
    output = expand_field_loops_enhanced(&output, construct, registry, target);

    // Dep-field iteration (legacy, kept for backward compat)
    output = expand_dep_field_loops(&output, construct, registry);

    // Step iteration (legacy)
    if output.contains("{{for step in steps}}") {
        let steps: Vec<&FlowStep> = construct.steps.iter().collect();
        output = expand_step_loop(&output, &steps);
    }

    // ─── Phase 3: Simple interpolations ──────────────────────────────────────
    output = output.replace("{{name}}", &construct.name);
    output = output.replace("{{subkind}}", &construct.subkind);
    output = output.replace("{{keyword}}", &construct.keyword);

    // Helper variables
    output = output.replace("{{name_snake}}", &to_snake_case(&construct.name));
    output = output.replace("{{name_camel}}", &to_camel_case(&construct.name));
    output = output.replace("{{count_children}}", &construct.children.len().to_string());

    // Error model access
    if output.contains("{{error_type}}") {
        let error_type = registry
            .error_model
            .as_ref()
            .map(|e| e.type_name.as_str())
            .unwrap_or("Error");
        output = output.replace("{{error_type}}", error_type);
    }
    // {{error_variant("role")}} — e.g. {{error_variant("not_found")}} → "NotFound"
    while let Some(start) = output.find("{{error_variant(\"") {
        let after = start + "{{error_variant(\"".len();
        let Some(quote_end) = output[after..].find('"') else { break };
        let role = output[after..after + quote_end].to_string();
        let Some(close) = output[after + quote_end..].find("}}") else { break };
        let end = after + quote_end + close + 2;
        let variant = registry
            .error_model
            .as_ref()
            .and_then(|e| e.variant(&role))
            .unwrap_or("");
        output = format!("{}{}{}", &output[..start], variant, &output[end..]);
    }

    // {{route}} — role:ui_route (page/layout) or leftover role:http_route
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

    // {{props_decl}} — reactive props declaration from props block (pattern from layer reactivity_policy)
    if output.contains("{{props_decl}}") {
        let props_block = construct.blocks.iter().find(|b| b.keyword == "props");
        let props_call = &registry.reactivity_policy.props_call;
        let props_script = if let Some(props) = props_block {
            let mut s = String::new();
            s.push_str("  interface Props {\n");
            for field in &props.fields {
                let ty = ts_type_display(&field.type_expr);
                s.push_str(&format!("    {}: {};\n", field.name, ty));
            }
            s.push_str("  }\n");
            let names: Vec<&str> = props.fields.iter().map(|f| f.name.as_str()).collect();
            if !names.is_empty() {
                s.push_str(&format!("  let {{ {} }}: Props = {};\n", names.join(", "), props_call));
            } else {
                s.push_str(&format!("  let {{}}: Props = {};\n", props_call));
            }
            s
        } else {
            String::new()
        };
        output = output.replace("{{props_decl}}", &props_script);
    }

    // {{state_decl}} — reactive state fields from state block (pattern from layer reactivity_policy)
    if output.contains("{{state_decl}}") {
        let state_block = construct.blocks.iter().find(|b| b.keyword == "state");
        let state_line_pattern = &registry.reactivity_policy.state_line;
        let state_script = if let Some(state) = state_block {
            let mut s = String::new();
            for field in &state.fields {
                let default = ts_default_value(&field.type_expr);
                let line = state_line_pattern
                    .replace("{name}", &field.name)
                    .replace("{type}", &ts_type_display(&field.type_expr))
                    .replace("{default}", &default);
                s.push_str(&format!("  {}\n", line));
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
                            .map(|p| format!("{}: {}", p.name, ts_type_display(&p.type_expr)))
                            .collect::<Vec<_>>().join(", ");
                        let is_async = f.return_type.as_ref()
                            .map(|t| matches!(t, veil_ir::TypeExpr::Result(_)))
                            .unwrap_or(false);
                        let ret_type = match &f.return_type {
                            Some(veil_ir::TypeExpr::Result(Some(inner))) =>
                                format!(": Promise<{}>", ts_type_display(inner)),
                            Some(veil_ir::TypeExpr::Result(None)) => ": Promise<void>".into(),
                            Some(ty) => format!(": {}", ts_type_display(ty)),
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
            .map(|name| {
                // Import pattern: layer can override via {{import_pattern}} in template.
                // Default: framework-agnostic relative import.
                format!("  import {name} from './{name}';")
            })
            .collect::<Vec<_>>()
            .join("\n");
        output = output.replace("{{imports}}", &import_stmts);
    }

    output
}

// ─── Conditional Blocks ──────────────────────────────────────────────────────

/// Process all `{{if CONDITION}}...{{else}}...{{end}}` blocks in a template.
/// Handles nested conditionals by processing from innermost to outermost.
/// Skips conditionals that are inside `{{for` loops — those are handled by
/// the loop expanders themselves (e.g. field-level conditionals).
fn expand_conditionals(
    template: &str,
    construct: &Construct,
    registry: &LayerRegistry,
    _target: &str,
) -> String {
    let mut result = template.to_string();

    // Process conditionals iteratively until none remain.
    // Each pass finds the innermost {{if}} that is NOT inside a {{for}} block.
    loop {
        let Some(if_start) = find_toplevel_innermost_if(&result) else {
            break;
        };

        let after_if = &result[if_start + "{{if ".len()..];
        let Some(cond_end) = after_if.find("}}") else { break };
        let condition = after_if[..cond_end].trim().to_string();
        let body_start = if_start + "{{if ".len() + cond_end + "}}".len();

        // Find matching {{end}} — since we found the innermost if, there are
        // no nested ifs between here and our end tag.
        let rest = &result[body_start..];
        let Some(end_offset) = rest.find("{{end}}") else { break };
        let end_abs = body_start + end_offset + "{{end}}".len();

        let body = &result[body_start..body_start + end_offset];

        // Split on {{else}} if present
        let (then_branch, else_branch) = if let Some(else_pos) = body.find("{{else}}") {
            (&body[..else_pos], &body[else_pos + "{{else}}".len()..])
        } else {
            (body, "")
        };

        // Evaluate condition
        let cond_result = evaluate_condition(&condition, construct, registry);
        let replacement = if cond_result {
            then_branch
        } else {
            else_branch
        };

        result = format!("{}{}{}", &result[..if_start], replacement, &result[end_abs..]);
    }

    result
}

/// Find the start position of the innermost `{{if ...}}` that is NOT nested
/// inside any `{{for ...}}` block. This ensures we only process top-level
/// conditionals in Phase 1 — loop-internal conditionals are handled by the
/// loop expanders.
fn find_toplevel_innermost_if(text: &str) -> Option<usize> {
    // Find all positions, filtering out those inside for-loops.
    let mut candidates = Vec::new();
    let mut pos = 0;
    let mut for_depth = 0;

    while pos < text.len() {
        if text[pos..].starts_with("{{for ") {
            for_depth += 1;
            pos += 6;
        } else if text[pos..].starts_with("{{end}}") {
            if for_depth > 0 {
                for_depth -= 1;
            }
            pos += 7;
        } else if text[pos..].starts_with("{{if ") {
            if for_depth == 0 {
                candidates.push(pos);
            }
            pos += 5;
        } else {
            // Advance by one char (handles multi-byte UTF-8)
            pos += text[pos..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }

    // From candidates, find the innermost (last one whose body has no nested {{if at top level)
    // Process from last to first — the last candidate is most likely innermost.
    for &candidate in candidates.iter().rev() {
        let after = &text[candidate + 5..];
        let Some(cond_close) = after.find("}}") else { continue };
        let body_start = candidate + 5 + cond_close + 2;
        let rest = &text[body_start..];
        let Some(end_pos) = rest.find("{{end}}") else { continue };
        let body = &rest[..end_pos];
        // Check that body has no top-level {{if (not inside a {{for)
        if !has_toplevel_if(body) {
            return Some(candidate);
        }
    }

    None
}

/// Check if text contains a `{{if` that is not inside a `{{for` block.
fn has_toplevel_if(text: &str) -> bool {
    let mut pos = 0;
    let mut for_depth = 0;
    while pos < text.len() {
        if text[pos..].starts_with("{{for ") {
            for_depth += 1;
            pos += 6;
        } else if text[pos..].starts_with("{{end}}") {
            if for_depth > 0 {
                for_depth -= 1;
            }
            pos += 7;
        } else if text[pos..].starts_with("{{if ") {
            if for_depth == 0 {
                return true;
            }
            pos += 5;
        } else {
            pos += text[pos..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }
    false
}

/// Find the start position of the innermost `{{if ...}}` — one that has no
/// nested `{{if` between itself and its matching `{{end}}`.
fn find_innermost_if(text: &str) -> Option<usize> {
    // Find all {{if positions, pick the last one that comes before any {{end}}
    // that doesn't have another {{if between them.
    let mut last_if_pos = None;
    let mut search_from = 0;

    while let Some(pos) = text[search_from..].find("{{if ") {
        let abs_pos = search_from + pos;
        last_if_pos = Some(abs_pos);
        search_from = abs_pos + 5;
    }

    // Verify: from last_if_pos, there should be a {{end}} before any other {{if
    if let Some(pos) = last_if_pos {
        let after = &text[pos + 5..];
        let next_if = after.find("{{if ");
        let next_end = after.find("{{end}}");
        match (next_end, next_if) {
            (Some(end_pos), Some(if_pos)) if if_pos < end_pos => {
                // There's a nested if before our end — go back to find a
                // non-nested one. Walk backward through all {{if positions.
                find_innermost_if_scan(text)
            }
            (Some(_), _) => Some(pos),
            (None, _) => None, // malformed template
        }
    } else {
        None
    }
}

/// Scan for the innermost if by finding an {{if whose body has no nested {{if.
fn find_innermost_if_scan(text: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find("{{if ") {
        let abs_pos = search_from + pos;
        // Check if this if's body (up to the next {{end}}) contains another {{if
        let after_cond = &text[abs_pos + 5..];
        let Some(cond_close) = after_cond.find("}}") else {
            search_from = abs_pos + 5;
            continue;
        };
        let body_start = abs_pos + 5 + cond_close + 2;
        let rest = &text[body_start..];
        let Some(end_pos) = rest.find("{{end}}") else {
            search_from = abs_pos + 5;
            continue;
        };
        let body = &rest[..end_pos];
        if !body.contains("{{if ") {
            return Some(abs_pos);
        }
        search_from = abs_pos + 5;
    }
    None
}

/// Evaluate a template condition against a construct.
///
/// Supported conditions:
/// - `has_annotation("name")` — construct has annotation @name
/// - `has_role("name")` — construct has a role (via registry)
/// - `has_children` — construct.children is non-empty
/// - `field.type == "X"` — field type check (set by field loop context)
/// - `method == "X"` — string equality (set by loop variable context)
/// - `!condition` — negation of any of the above
fn evaluate_condition(condition: &str, construct: &Construct, registry: &LayerRegistry) -> bool {
    let trimmed = condition.trim();

    // Negation
    if let Some(inner) = trimmed.strip_prefix('!') {
        return !evaluate_condition(inner.trim(), construct, registry);
    }

    // has_annotation("name")
    if let Some(name) = extract_quoted_arg(trimmed, "has_annotation") {
        return construct.annotations.iter().any(|a| a.name == name);
    }

    // has_role("name")
    if let Some(role) = extract_quoted_arg(trimmed, "has_role") {
        return construct
            .annotations
            .iter()
            .any(|a| registry.annotation_has_role(&a.name, &role));
    }

    // has_children
    if trimmed == "has_children" {
        return !construct.children.is_empty();
    }

    // field.type == "X" — this is evaluated at the CONSTRUCT level as a fallback.
    // The real field-level check happens inside expand_field_loops_enhanced when
    // conditionals are nested inside field loops. At construct level, this is always false.
    if trimmed.starts_with("field.type == ") {
        return false;
    }

    // Generic equality: `varname == "value"` — falls through to false at top level.
    // Inside loops, conditions are evaluated with loop-scoped context.
    if trimmed.contains(" == ") {
        return false;
    }

    false
}

// ─── Child Iteration ─────────────────────────────────────────────────────────

/// Expand `{{for child in children}}...{{end}}` and
/// `{{for child in children where FILTER}}...{{end}}` loops.
///
/// Inside the loop body, the template context shifts to the child construct,
/// so `{{name}}`, `{{annotation_value:X}}`, etc. resolve against the child.
fn expand_child_loops(
    template: &str,
    construct: &Construct,
    registry: &LayerRegistry,
    target: &str,
) -> String {
    let mut result = template.to_string();
    let prefix = "{{for child in children";

    loop {
        let Some(start) = result.find(prefix) else { break };
        let after_prefix = &result[start + prefix.len()..];

        // Parse the rest of the opening tag: either `}}` or ` where CONDITION}}`
        let Some(tag_close) = after_prefix.find("}}") else { break };
        let tag_content = after_prefix[..tag_close].trim();
        let filter = if let Some(where_clause) = tag_content.strip_prefix("where ") {
            Some(where_clause.trim().to_string())
        } else {
            // tag_content should be empty (plain `{{for child in children}}`)
            None
        };

        let body_start = start + prefix.len() + tag_close + "}}".len();

        // Find matching {{end}} accounting for nesting
        let Some(end_offset) = find_matching_end(&result[body_start..]) else { break };
        let end_abs = body_start + end_offset + "{{end}}".len();
        let body = result[body_start..body_start + end_offset].to_string();

        // Filter children
        let children: Vec<&Construct> = construct
            .children
            .iter()
            .filter(|child| match_child_filter(child, filter.as_deref(), registry))
            .collect();

        // Expand body for each matching child
        let mut expanded = String::new();
        for child in &children {
            let mut child_body = body.clone();

            // Replace {{child.lowers_to}} with recursive template rendering
            if child_body.contains("{{child.lowers_to}}") {
                let child_template = registry
                    .construct_lowers_to(child, target)
                    .unwrap_or("");
                let rendered = if child_template.is_empty() {
                    String::new()
                } else {
                    render_template_for_construct(child, child_template, registry, target)
                };
                child_body = child_body.replace("{{child.lowers_to}}", &rendered);
            }

            // Replace {{child.X}} accessors
            child_body = child_body.replace("{{child.name}}", &child.name);
            child_body = child_body.replace("{{child.name_snake}}", &to_snake_case(&child.name));
            child_body = child_body.replace("{{child.name_camel}}", &to_camel_case(&child.name));
            child_body = child_body.replace("{{child.subkind}}", &child.subkind);
            child_body = child_body.replace("{{child.keyword}}", &child.keyword);

            // {{child.annotation_value("name")}} / {{child.annotation_arg("name", N)}}
            child_body = expand_prefixed_annotation_placeholders(child, &child_body, "child.");

            expanded.push_str(&child_body);
        }

        result = format!("{}{}{}", &result[..start], expanded, &result[end_abs..]);
    }

    result
}

/// Check if a child matches a where-filter.
///
/// Supported filters:
/// - `role == "X"` — child has role X (via registry)
/// - `has_annotation("X")` — child has annotation @X
/// - `keyword == "X"` — child.keyword equals X
/// - `subkind == "X"` — child.subkind equals X
fn match_child_filter(child: &Construct, filter: Option<&str>, registry: &LayerRegistry) -> bool {
    let Some(filter) = filter else { return true };
    let filter = filter.trim();

    if let Some(role) = extract_equality_value(filter, "role") {
        return registry.construct_has_role(child, &role);
    }
    if let Some(name) = extract_quoted_arg(filter, "has_annotation") {
        return child.annotations.iter().any(|a| a.name == name);
    }
    if let Some(kw) = extract_equality_value(filter, "keyword") {
        return child.keyword.eq_ignore_ascii_case(&kw);
    }
    if let Some(sk) = extract_equality_value(filter, "subkind") {
        return child.subkind.eq_ignore_ascii_case(&sk);
    }

    // Unknown filter — include all
    true
}

/// Extract value from `key == "value"` pattern.
fn extract_equality_value(s: &str, key: &str) -> Option<String> {
    let prefix = format!("{} == \"", key);
    if let Some(start) = s.find(&prefix) {
        let after = &s[start + prefix.len()..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    None
}

// ─── Method Iteration ────────────────────────────────────────────────────────

/// Expand `{{for method in methods}}...{{end}}` loops.
///
/// Available inside the loop:
/// - `{{method.name}}` — method name
/// - `{{method.params}}` — formatted parameter list
/// - `{{method.return_type}}` — return type string
fn expand_method_loops(template: &str, construct: &Construct, target: &str) -> String {
    let tag = "{{for method in methods}}";
    let mut result = template.to_string();

    while let Some(start) = result.find(tag) {
        let body_start = start + tag.len();
        let Some(end_offset) = find_matching_end(&result[body_start..]) else { break };
        let end_abs = body_start + end_offset + "{{end}}".len();
        let body = &result[body_start..body_start + end_offset];

        let mut expanded = String::new();
        for method in &construct.methods {
            let mut line = body.to_string();
            line = line.replace("{{method.name}}", &method.name);

            let params = method
                .params
                .iter()
                .map(|p| {
                    let ty = match target {
                        "rust" => crate::rust::type_to_rust(&p.type_expr),
                        "typescript" => crate::ts::lower::type_to_ts(&p.type_expr),
                        _ => type_to_display(&p.type_expr),
                    };
                    format!("{}: {}", p.name, ty)
                })
                .collect::<Vec<_>>()
                .join(", ");
            line = line.replace("{{method.params}}", &params);

            let ret = match &method.return_type {
                Some(t) => match target {
                    "rust" => crate::rust::type_to_rust(t),
                    "typescript" => crate::ts::lower::type_to_ts(t),
                    _ => type_to_display(t),
                },
                None => String::new(),
            };
            line = line.replace("{{method.return_type}}", &ret);

            expanded.push_str(&line);
        }

        result = format!("{}{}{}", &result[..start], expanded, &result[end_abs..]);
    }

    result
}

// ─── Enhanced Field Iteration ────────────────────────────────────────────────

/// Expand `{{for field in fields}}...{{end}}` with enhanced type introspection.
///
/// Available inside the loop:
/// - `{{field.name}}` — field name
/// - `{{field.type}}` — VEIL type as-is
/// - `{{field.rust_type}}` — Rust type via type_to_rust
/// - `{{field.ts_type}}` — TS type via type_to_ts
///
/// Supports optional where clause:
/// - `{{for field in fields where annotation == "dep"}}` — only fields with @dep
fn expand_field_loops_enhanced(
    template: &str,
    construct: &Construct,
    registry: &LayerRegistry,
    target: &str,
) -> String {
    let prefix = "{{for field in fields";
    let mut result = template.to_string();

    loop {
        let Some(start) = result.find(prefix) else { break };
        let after_prefix = &result[start + prefix.len()..];

        let Some(tag_close) = after_prefix.find("}}") else { break };
        let tag_content = after_prefix[..tag_close].trim();
        let filter = if let Some(where_clause) = tag_content.strip_prefix("where ") {
            Some(where_clause.trim().to_string())
        } else {
            None
        };

        let body_start = start + prefix.len() + tag_close + "}}".len();
        let Some(end_offset) = find_matching_end(&result[body_start..]) else { break };
        let end_abs = body_start + end_offset + "{{end}}".len();
        let body = &result[body_start..body_start + end_offset];

        // Collect fields, applying filter
        let fields: Vec<&Field> = construct
            .fields
            .iter()
            .filter(|f| match_field_filter(f, filter.as_deref(), registry))
            .collect();

        let mut expanded = String::new();
        for field in &fields {
            let mut line = body.to_string();

            // Process field-level conditionals (e.g. {{if field.type == "Uuid"}}...{{end}})
            line = expand_field_conditionals(&line, field, construct, registry, target);

            line = line.replace("{{field.name}}", &field.name);
            line = line.replace("{{field.type}}", &type_to_display(&field.type_expr));
            line = line.replace("{{field.rust_type}}", &crate::rust::type_to_rust(&field.type_expr));
            line = line.replace("{{field.ts_type}}", &crate::ts::lower::type_to_ts(&field.type_expr));

            expanded.push_str(&line);
        }

        result = format!("{}{}{}", &result[..start], expanded, &result[end_abs..]);
    }

    result
}

/// Check if a field matches a where-filter.
///
/// Supported filters:
/// - `annotation == "X"` — field has annotation @X
/// - `type == "X"` — field type name equals X
fn match_field_filter(field: &Field, filter: Option<&str>, _registry: &LayerRegistry) -> bool {
    let Some(filter) = filter else { return true };
    let filter = filter.trim();

    if let Some(ann_name) = extract_equality_value(filter, "annotation") {
        return field.annotations.iter().any(|a| a.name == ann_name);
    }
    if let Some(type_name) = extract_equality_value(filter, "type") {
        return type_to_display(&field.type_expr) == type_name;
    }

    true
}

/// Expand conditionals inside a field loop body — these can reference field.type.
fn expand_field_conditionals(
    body: &str,
    field: &Field,
    construct: &Construct,
    registry: &LayerRegistry,
    _target: &str,
) -> String {
    let mut result = body.to_string();

    loop {
        let Some(if_start) = find_innermost_if(&result) else { break };

        let after_if = &result[if_start + "{{if ".len()..];
        let Some(cond_end) = after_if.find("}}") else { break };
        let condition = after_if[..cond_end].trim().to_string();
        let body_start = if_start + "{{if ".len() + cond_end + "}}".len();

        let rest = &result[body_start..];
        let Some(end_offset) = rest.find("{{end}}") else { break };
        let end_abs = body_start + end_offset + "{{end}}".len();

        let inner_body = &result[body_start..body_start + end_offset];
        let (then_branch, else_branch) = if let Some(else_pos) = inner_body.find("{{else}}") {
            (&inner_body[..else_pos], &inner_body[else_pos + "{{else}}".len()..])
        } else {
            (inner_body, "")
        };

        let cond_result = evaluate_field_condition(&condition, field, construct, registry);
        let replacement = if cond_result { then_branch } else { else_branch };

        result = format!("{}{}{}", &result[..if_start], replacement, &result[end_abs..]);
    }

    result
}

/// Evaluate a condition in the context of a field loop iteration.
fn evaluate_field_condition(
    condition: &str,
    field: &Field,
    construct: &Construct,
    registry: &LayerRegistry,
) -> bool {
    let trimmed = condition.trim();

    // Negation
    if let Some(inner) = trimmed.strip_prefix('!') {
        return !evaluate_field_condition(inner.trim(), field, construct, registry);
    }

    // field.type == "X"
    if let Some(type_name) = extract_equality_value(trimmed, "field.type") {
        return type_to_display(&field.type_expr) == type_name;
    }

    // Fall back to construct-level conditions
    evaluate_condition(trimmed, construct, registry)
}

// ─── Dep-Field Iteration (legacy) ───────────────────────────────────────────

/// Legacy dep_fields loop for backward compatibility.
fn expand_dep_field_loops(
    template: &str,
    construct: &Construct,
    registry: &LayerRegistry,
) -> String {
    if !template.contains("{{for field in dep_fields}}") {
        return template.to_string();
    }

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

    expand_for_loop(template, "field", "dep_fields", &dep_fields, |field, var| {
        match var {
            "field.name" => field.name.clone(),
            "field.type" => type_to_display(&field.type_expr),
            _ => format!("{{{{{}}}}}", var),
        }
    })
}

// ─── Utility: Find matching {{end}} with nesting ────────────────────────────

/// Find the offset of the matching `{{end}}` tag, accounting for nested
/// `{{for` and `{{if` blocks.
fn find_matching_end(text: &str) -> Option<usize> {
    let mut depth = 1;
    let mut pos = 0;
    while pos < text.len() {
        if text[pos..].starts_with("{{for ") || text[pos..].starts_with("{{if ") {
            depth += 1;
            pos += 5;
        } else if text[pos..].starts_with("{{end}}") {
            depth -= 1;
            if depth == 0 {
                return Some(pos);
            }
            pos += 7;
        } else {
            pos += text[pos..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }
    None
}

// ─── Helper: Annotation placeholders with prefix ────────────────────────────

/// Expand annotation placeholders with a prefix (e.g. "child." for {{child.annotation_value("X")}}).
fn expand_prefixed_annotation_placeholders(construct: &Construct, output: &str, prefix: &str) -> String {
    let mut result = output.to_string();

    // {{prefix.annotation_value("name")}}
    let pattern = format!("{{{{{prefix}annotation_value(\"");
    while let Some(start) = result.find(&pattern) {
        let after = start + pattern.len();
        let Some(name_end) = result[after..].find('"') else { break };
        let name = result[after..after + name_end].to_string();
        let rest = &result[after + name_end..];
        let Some(close) = rest.find("}}") else { break };
        let replacement = annotation_arg_at(construct, &name, 0);
        let abs_end = after + name_end + close + 2;
        result = format!("{}{}{}", &result[..start], replacement, &result[abs_end..]);
    }

    // {{prefix.annotation_arg("name", N)}}
    let pattern = format!("{{{{{prefix}annotation_arg(\"");
    while let Some(start) = result.find(&pattern) {
        let after = start + pattern.len();
        let Some(name_end) = result[after..].find('"') else { break };
        let name = result[after..after + name_end].to_string();
        let rest = &result[after + name_end..];
        let Some(close) = rest.find("}}") else { break };
        let mid = &rest[..close];
        let idx: usize = mid
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        let replacement = annotation_arg_at(construct, &name, idx);
        let abs_end = after + name_end + close + 2;
        result = format!("{}{}{}", &result[..start], replacement, &result[abs_end..]);
    }

    result
}

// ─── Case conversion helpers ─────────────────────────────────────────────────

/// Convert PascalCase/camelCase to snake_case.
fn to_snake_case(name: &str) -> String {
    crate::rust::to_snake(name)
}

/// Convert PascalCase/snake_case to camelCase.
fn to_camel_case(name: &str) -> String {
    crate::ts::lower::to_camel(name)
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

/// File-path segment for framework route paths (e.g. `src/routes/{{route}}/+page`).
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
            search_pos += template[search_pos..].chars().next().map_or(1, |c| c.len_utf8());
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

/// Default state initializer from a VEIL type.
/// Map VEIL types to TypeScript-ish names for reactive state declarations.
fn ts_type_display(ty: &veil_ir::TypeExpr) -> String {
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
        TypeExpr::List(inner) => format!("{}[]", ts_type_display(inner)),
        TypeExpr::Optional(inner) => format!("{} | null", ts_type_display(inner)),
        TypeExpr::Map(_, v) => format!("Record<string, {}>", ts_type_display(v)),
        _ => "any".into(),
    }
}

fn ts_default_value(ty: &veil_ir::TypeExpr) -> String {
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


// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::ast::{Annotation, Construct, Method, Param};
    use veil_ir::layer::{CodegenRule, LayerRegistry, Shape};
    use veil_ir::span::Span;
    use veil_ir::TypeExpr;

    fn span() -> Span {
        Span::default()
    }

    fn empty_construct(name: &str) -> Construct {
        Construct {
            keyword: "struct".into(),
            subkind: "Entity".into(),
            shape: Shape::Struct,
            name: name.into(),
            type_params: Vec::new(),
            span: span(),
            annotations: Vec::new(),
            exported: false,
            visibility: "pub".into(),
            where_clause: Vec::new(),
            deployment_unit: false,
            layer_provided: false,
            fields: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            raw_blocks: Vec::new(),
            effects: Vec::new(),
            fns: Vec::new(),
            test_blocks: Vec::new(),
            variants: Vec::new(),
            rich_variants: Vec::new(),
            transitions: Vec::new(),
            methods: Vec::new(),
            associated_types: Vec::new(),
            target: None,
            target_type_args: Vec::new(),
            impls: Vec::new(),
            inputs: Vec::new(),
            steps: Vec::new(),
            return_expr: None,
            refs: Vec::new(),
            children: Vec::new(),
            pass_annotations: std::collections::HashMap::new(),
        }
    }

    fn make_rule(body: &str) -> CodegenRule {
        CodegenRule {
            match_shape: "struct".into(),
            condition: String::new(),
            emit_body: body.into(),
            emit_to: None,
            emit_file: None,
            priority: 100,
        }
    }

    fn render(construct: &Construct, body: &str) -> String {
        let registry = LayerRegistry::builtin();
        let rule = make_rule(body);
        render_template(construct, &rule, &registry, "rust")
    }

    // ─── Conditionals ────────────────────────────────────────────────────────

    #[test]
    fn conditional_has_annotation_true() {
        let mut c = empty_construct("Order");
        c.annotations.push(Annotation {
            name: "route".into(),
            args: vec!["\"/orders\"".into()],
            span: span(),
        });
        let result = render(&c, "{{if has_annotation(\"route\")}}yes{{end}}");
        assert_eq!(result, "yes");
    }

    #[test]
    fn conditional_has_annotation_false() {
        let c = empty_construct("Order");
        let result = render(&c, "{{if has_annotation(\"route\")}}yes{{end}}");
        assert_eq!(result, "");
    }

    #[test]
    fn conditional_negation() {
        let c = empty_construct("Order");
        let result = render(&c, "{{if !has_annotation(\"route\")}}no route{{end}}");
        assert_eq!(result, "no route");
    }

    #[test]
    fn conditional_with_else_branch() {
        let c = empty_construct("Order");
        let result = render(&c, "{{if has_annotation(\"route\")}}routed{{else}}unrouted{{end}}");
        assert_eq!(result, "unrouted");
    }

    #[test]
    fn conditional_has_children_true() {
        let mut c = empty_construct("Widget");
        c.children.push(empty_construct("Endpoint"));
        let result = render(&c, "{{if has_children}}has kids{{end}}");
        assert_eq!(result, "has kids");
    }

    #[test]
    fn conditional_has_children_false() {
        let c = empty_construct("Widget");
        let result = render(&c, "{{if has_children}}has kids{{end}}");
        assert_eq!(result, "");
    }

    #[test]
    fn conditional_nested() {
        let mut c = empty_construct("Order");
        c.annotations.push(Annotation {
            name: "route".into(),
            args: vec!["\"/orders\"".into()],
            span: span(),
        });
        c.children.push(empty_construct("Item"));
        let tpl = "{{if has_annotation(\"route\")}}R{{if has_children}}C{{end}}{{end}}";
        let result = render(&c, tpl);
        assert_eq!(result, "RC");
    }

    // ─── Child Iteration ─────────────────────────────────────────────────────

    #[test]
    fn for_child_basic() {
        let mut c = empty_construct("Widget");
        c.children.push(empty_construct("Alpha"));
        c.children.push(empty_construct("Beta"));
        let result = render(&c, "{{for child in children}}{{child.name}},{{end}}");
        assert_eq!(result, "Alpha,Beta,");
    }

    #[test]
    fn for_child_name_snake() {
        let mut c = empty_construct("Widget");
        c.children.push(empty_construct("GetOrders"));
        let result = render(&c, "{{for child in children}}{{child.name_snake}}{{end}}");
        assert_eq!(result, "get_orders");
    }

    #[test]
    fn for_child_where_keyword() {
        let mut c = empty_construct("Widget");
        let mut ep = empty_construct("ListItems");
        ep.keyword = "endpoint".into();
        c.children.push(ep);
        let mut other = empty_construct("Other");
        other.keyword = "struct".into();
        c.children.push(other);
        let result = render(
            &c,
            "{{for child in children where keyword == \"endpoint\"}}{{child.name}}{{end}}",
        );
        assert_eq!(result, "ListItems");
    }

    #[test]
    fn for_child_annotation_value() {
        let mut c = empty_construct("Widget");
        let mut ep = empty_construct("GetOrder");
        ep.annotations.push(Annotation {
            name: "route".into(),
            args: vec!["\"/orders/:id\"".into()],
            span: span(),
        });
        c.children.push(ep);
        let result = render(
            &c,
            "{{for child in children}}{{child.annotation_value(\"route\")}}{{end}}",
        );
        assert_eq!(result, "/orders/:id");
    }

    // ─── Field Iteration ─────────────────────────────────────────────────────

    #[test]
    fn for_field_basic() {
        let mut c = empty_construct("Order");
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "id".into(),
            type_expr: TypeExpr::Named("Uuid".into()),
            default_expr: None,
            span: span(),
        });
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "total".into(),
            type_expr: TypeExpr::Named("F64".into()),
            default_expr: None,
            span: span(),
        });
        let result = render(&c, "{{for field in fields}}{{field.name}}: {{field.rust_type}},\n{{end}}");
        assert_eq!(result, "id: Uuid,\ntotal: f64,\n");
    }

    #[test]
    fn for_field_ts_type() {
        let mut c = empty_construct("Order");
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "name".into(),
            type_expr: TypeExpr::Named("Str".into()),
            default_expr: None,
            span: span(),
        });
        let registry = LayerRegistry::builtin();
        let result = render_template_for_construct(
            &c,
            "{{for field in fields}}{{field.ts_type}}{{end}}",
            &registry,
            "typescript",
        );
        assert_eq!(result, "string");
    }

    #[test]
    fn for_field_with_annotation_filter() {
        let mut c = empty_construct("Handler");
        c.fields.push(Field {
            annotations: vec![Annotation {
                name: "dep".into(),
                args: Vec::new(),
                span: span(),
            }],
            name: "repo".into(),
            type_expr: TypeExpr::Named("OrderRepo".into()),
            default_expr: None,
            span: span(),
        });
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "count".into(),
            type_expr: TypeExpr::Named("Int".into()),
            default_expr: None,
            span: span(),
        });
        let result = render(
            &c,
            "{{for field in fields where annotation == \"dep\"}}{{field.name}}{{end}}",
        );
        assert_eq!(result, "repo");
    }

    #[test]
    fn for_field_with_type_conditional() {
        let mut c = empty_construct("Handler");
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "id".into(),
            type_expr: TypeExpr::Named("Uuid".into()),
            default_expr: None,
            span: span(),
        });
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "name".into(),
            type_expr: TypeExpr::Named("Str".into()),
            default_expr: None,
            span: span(),
        });
        let result = render(
            &c,
            "{{for field in fields}}{{if field.type == \"Uuid\"}}parse({{field.name}}){{else}}{{field.name}}{{end}},{{end}}",
        );
        assert_eq!(result, "parse(id),name,");
    }

    // ─── Method Iteration ────────────────────────────────────────────────────

    #[test]
    fn for_method_basic() {
        let mut c = empty_construct("Repository");
        c.shape = Shape::Trait;
        c.methods.push(Method {
            name: "find_by_id".into(),
            span: span(),
            params: vec![Param {
                name: "id".into(),
                type_expr: TypeExpr::Named("Uuid".into()),
                span: span(),
            }],
            return_type: Some(TypeExpr::Named("Order".into())),
        });
        let result = render(
            &c,
            "{{for method in methods}}fn {{method.name}}({{method.params}}) -> {{method.return_type}};{{end}}",
        );
        assert_eq!(result, "fn find_by_id(id: Uuid) -> Order;");
    }

    #[test]
    fn for_method_multiple_params() {
        let mut c = empty_construct("Service");
        c.shape = Shape::Trait;
        c.methods.push(Method {
            name: "update".into(),
            span: span(),
            params: vec![
                Param {
                    name: "id".into(),
                    type_expr: TypeExpr::Named("Uuid".into()),
                    span: span(),
                },
                Param {
                    name: "data".into(),
                    type_expr: TypeExpr::Named("Str".into()),
                    span: span(),
                },
            ],
            return_type: None,
        });
        let result = render(
            &c,
            "{{for method in methods}}{{method.name}}({{method.params}}){{end}}",
        );
        assert_eq!(result, "update(id: Uuid, data: String)");
    }

    // ─── Helper Variables ────────────────────────────────────────────────────

    #[test]
    fn helper_name_snake() {
        let c = empty_construct("OrderService");
        let result = render(&c, "{{name_snake}}");
        assert_eq!(result, "order_service");
    }

    #[test]
    fn helper_name_camel() {
        let c = empty_construct("OrderService");
        let result = render(&c, "{{name_camel}}");
        // to_camel on PascalCase → keeps as-is or lowercases first char depending on impl
        // Our to_camel converts "OrderService" → it depends on implementation
        let result_val = to_camel_case("OrderService");
        assert_eq!(result, result_val);
    }

    #[test]
    fn helper_count_children() {
        let mut c = empty_construct("Widget");
        c.children.push(empty_construct("A"));
        c.children.push(empty_construct("B"));
        c.children.push(empty_construct("C"));
        let result = render(&c, "{{count_children}}");
        assert_eq!(result, "3");
    }

    #[test]
    fn helper_error_type() {
        let mut registry = LayerRegistry::builtin();
        registry.error_model = Some(veil_ir::layer::ErrorModelPolicy {
            type_name: "AppError".into(),
            variants: vec![
                ("not_found".into(), "NotFound".into()),
                ("validation".into(), "Validation".into()),
            ],
        });
        let c = empty_construct("Handler");
        let rule = make_rule("{{error_type}}");
        let result = render_template(&c, &rule, &registry, "rust");
        assert_eq!(result, "AppError");
    }

    #[test]
    fn helper_error_variant() {
        let mut registry = LayerRegistry::builtin();
        registry.error_model = Some(veil_ir::layer::ErrorModelPolicy {
            type_name: "AppError".into(),
            variants: vec![
                ("not_found".into(), "NotFound".into()),
                ("validation".into(), "Validation".into()),
            ],
        });
        let c = empty_construct("Handler");
        let rule = make_rule("{{error_variant(\"not_found\")}}");
        let result = render_template(&c, &rule, &registry, "rust");
        assert_eq!(result, "NotFound");
    }

    // ─── Nested Template Resolution ──────────────────────────────────────────

    #[test]
    fn child_lowers_to_resolution() {
        // Setup: a registry with a lowers_to template for the child's construct spec.
        use veil_ir::layer::ConstructSpec;
        use std::collections::HashMap;

        let mut registry = LayerRegistry::builtin();
        // Directly inject a construct spec with lowers_to for "endpoint" keyword.
        let mut lowers_to = HashMap::new();
        lowers_to.insert("rust".into(), "fn {{name_snake}}_handler() {}".into());
        registry.constructs.push(ConstructSpec {
            name: "Endpoint".into(),
            keyword: "endpoint".into(),
            maps_to: "struct".into(),
            shape: Shape::Struct,
            layer: "test".into(),
            desc: String::new(),
            contains: Vec::new(),
            blocks: Vec::new(),
            raw_block_keywords: Vec::new(),
            constraints: Vec::new(),
            allowed_in: String::new(),
            group: String::new(),
            visual: veil_ir::layer::Visual::default(),
            runtime: None,
            au: false,
            is_step: false,
            step_fields: Vec::new(),
            annotations: Vec::new(),
            tgt: String::new(),
            dg: String::new(),
            presentation: Default::default(),
            roles: Vec::new(),
            config_keys: Vec::new(),
            required_fields: Vec::new(),
            lowers_to,
        });

        let mut parent = empty_construct("Widget");
        let mut child = empty_construct("GetItems");
        child.keyword = "endpoint".into();
        child.subkind = "Endpoint".into();
        parent.children.push(child);

        let rule = make_rule("{{for child in children}}{{child.lowers_to}}\n{{end}}");
        let result = render_template(&parent, &rule, &registry, "rust");
        assert_eq!(result.trim(), "fn get_items_handler() {}");
    }

    // ─── Integration: Combined Features ──────────────────────────────────────

    #[test]
    fn combined_conditional_and_child_loop() {
        let mut c = empty_construct("Widget");
        c.children.push(empty_construct("A"));
        c.children.push(empty_construct("B"));
        let tpl = "{{if has_children}}routes:\n{{for child in children}}- {{child.name_snake}}\n{{end}}{{end}}";
        let result = render(&c, tpl);
        assert_eq!(result, "routes:\n- a\n- b\n");
    }

    #[test]
    fn combined_fields_and_helpers() {
        let mut c = empty_construct("UserService");
        c.fields.push(Field {
            annotations: Vec::new(),
            name: "name".into(),
            type_expr: TypeExpr::Named("Str".into()),
            default_expr: None,
            span: span(),
        });
        let result = render(
            &c,
            "struct {{name}} {\n{{for field in fields}}  {{field.name}}: {{field.rust_type}},\n{{end}}}\nmod {{name_snake}};",
        );
        assert_eq!(
            result,
            "struct UserService {\n  name: String,\n}\nmod user_service;"
        );
    }

    // ─── find_matching_end ───────────────────────────────────────────────────

    #[test]
    fn find_matching_end_simple() {
        let text = "body text{{end}}";
        assert_eq!(find_matching_end(text), Some(9));
    }

    #[test]
    fn find_matching_end_nested_for() {
        let text = "{{for x in y}}inner{{end}}outer{{end}}";
        assert_eq!(find_matching_end(text), Some(31));
    }

    #[test]
    fn find_matching_end_nested_if() {
        let text = "{{if cond}}inner{{end}}outer{{end}}";
        assert_eq!(find_matching_end(text), Some(28));
    }

    // ─── find_innermost_if ───────────────────────────────────────────────────

    #[test]
    fn find_innermost_no_if() {
        assert_eq!(find_innermost_if("no conditionals here"), None);
    }

    #[test]
    fn find_innermost_single_if() {
        let text = "before{{if x}}body{{end}}after";
        assert_eq!(find_innermost_if(text), Some(6));
    }

    #[test]
    fn find_innermost_nested_ifs() {
        let text = "{{if a}}{{if b}}inner{{end}}outer{{end}}";
        // Innermost is the second {{if b}}
        let pos = find_innermost_if(text).unwrap();
        assert!(text[pos..].starts_with("{{if b}}"));
    }

    // ─── evaluate_condition ──────────────────────────────────────────────────

    #[test]
    fn eval_has_annotation_present() {
        let mut c = empty_construct("X");
        c.annotations.push(Annotation {
            name: "auth".into(),
            args: Vec::new(),
            span: span(),
        });
        let registry = LayerRegistry::builtin();
        assert!(evaluate_condition("has_annotation(\"auth\")", &c, &registry));
    }

    #[test]
    fn eval_has_annotation_absent() {
        let c = empty_construct("X");
        let registry = LayerRegistry::builtin();
        assert!(!evaluate_condition("has_annotation(\"auth\")", &c, &registry));
    }

    #[test]
    fn eval_negation() {
        let c = empty_construct("X");
        let registry = LayerRegistry::builtin();
        assert!(evaluate_condition("!has_annotation(\"auth\")", &c, &registry));
    }

    #[test]
    fn eval_has_children_empty() {
        let c = empty_construct("X");
        let registry = LayerRegistry::builtin();
        assert!(!evaluate_condition("has_children", &c, &registry));
    }

    #[test]
    fn eval_has_children_nonempty() {
        let mut c = empty_construct("X");
        c.children.push(empty_construct("Y"));
        let registry = LayerRegistry::builtin();
        assert!(evaluate_condition("has_children", &c, &registry));
    }

    // ─── extract_equality_value ──────────────────────────────────────────────

    #[test]
    fn extract_eq_role() {
        let v = extract_equality_value("role == \"http_endpoint\"", "role");
        assert_eq!(v, Some("http_endpoint".to_string()));
    }

    #[test]
    fn extract_eq_keyword() {
        let v = extract_equality_value("keyword == \"endpoint\"", "keyword");
        assert_eq!(v, Some("endpoint".to_string()));
    }

    #[test]
    fn extract_eq_missing() {
        let v = extract_equality_value("something else", "role");
        assert_eq!(v, None);
    }
}
