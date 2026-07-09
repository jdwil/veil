//! Template execution engine — evaluates layer-declared codegen templates
//! against the full IR to produce target-language output.
//!
//! The engine has NO domain knowledge. It executes templates from the
//! LayerRegistry, matching them against constructs in the Solution AST.

use std::collections::HashMap;

use veil_ir::ast::{Construct, Expr, FlowStep, Solution, StepDef, Field, TypeExpr};
use veil_ir::builder::{expr_to_display, type_to_display};
use veil_ir::layer::{CodegenRule, CodegenTemplate, LayerRegistry, Shape};

/// Result of executing all templates for a target.
pub struct TemplateOutput {
    /// Files emitted directly (path → content).
    pub files: Vec<TemplateFile>,
    /// Named sections (section name → ordered contributions).
    pub sections: HashMap<String, Vec<SectionContribution>>,
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
    let mut file_fragments: Vec<String> = Vec::new();

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
        };
    }

    // Walk all top-level constructs in the solution
    for item in &solution.items {
        let construct = match item {
            veil_ir::ast::TopLevelItem::Construct(c) => c,
            _ => continue,
        };
        for template in &templates {
            for rule in &template.rules {
                if matches_construct(construct, rule) {
                    let output = render_template(construct, rule);

                    if let Some(section_name) = &rule.emit_to {
                        // Contribute to a named section
                        sections
                            .entry(section_name.clone())
                            .or_insert_with(Vec::new)
                            .push(SectionContribution {
                                priority: rule.priority,
                                content: output,
                                source_layer: template.layer.clone(),
                                source_rule: format!("match {} where {}", rule.match_shape, rule.condition),
                            });
                    } else {
                        // Direct emit to file
                        file_fragments.push(output);
                    }
                }
            }
        }
    }

    // Sort section contributions by priority
    for contributions in sections.values_mut() {
        contributions.sort_by_key(|c| c.priority);
    }

    let mut files = Vec::new();
    if !file_fragments.is_empty() {
        files.push(TemplateFile {
            path: format!("{}_generated.{}", solution.name, target_extension(target)),
            content: file_fragments.join("\n\n"),
        });
    }

    TemplateOutput { files, sections }
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

/// Check if a construct matches a rule's conditions.
fn matches_construct(construct: &Construct, rule: &CodegenRule) -> bool {
    // Check shape match
    let shape_name = construct.shape.name();
    if rule.match_shape != shape_name && rule.match_shape != "*" {
        return false;
    }

    // Check condition
    if rule.condition.is_empty() {
        return true; // No condition = match all of this shape
    }

    // Parse simple conditions
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
            // subkind comes from the construct's layer-declared name
            return construct.name == sk;
        }
    }

    // Unknown condition — don't match
    false
}

/// Render a template body with interpolation against a construct.
fn render_template(construct: &Construct, rule: &CodegenRule) -> String {
    let mut output = rule.emit_body.clone();

    // Simple interpolations
    output = output.replace("{{name}}", &construct.name);

    // Handle {{for field in dep_fields}}...{{end}}
    if output.contains("{{for field in dep_fields}}") {
        let dep_fields: Vec<&veil_ir::ast::Field> = construct
            .fields
            .iter()
            .filter(|f| {
                construct.annotations.iter().any(|a| a.name == "dep")
            })
            .collect();

        output = expand_for_loop(&output, "field", "dep_fields", &dep_fields, |field, var| {
            match var {
                "field.name" => field.name.clone(),
                "field.type" => veil_ir::builder::type_to_display(&field.type_expr),
                _ => format!("{{{{{}}}}}",var),
            }
        });
    }

    // Handle {{for field in fields}}...{{end}}
    if output.contains("{{for field in fields}}") {
        let fields: Vec<&veil_ir::ast::Field> = construct.fields.iter().collect();

        output = expand_for_loop(&output, "field", "fields", &fields, |field, var| {
            match var {
                "field.name" => field.name.clone(),
                "field.type" => veil_ir::builder::type_to_display(&field.type_expr),
                _ => format!("{{{{{}}}}}", var),
            }
        });
    }

    // Handle {{for step in steps}}...{{end}}
    if output.contains("{{for step in steps}}") {
        let steps: Vec<&veil_ir::ast::FlowStep> = construct.steps.iter().collect();

        output = expand_step_loop(&output, &steps);
    }

    output
}

/// Expand a {{for item in collection}}...{{end}} loop.
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

    let Some(start_idx) = template.find(&start_tag) else {
        return template.to_string();
    };
    let after_start = start_idx + start_tag.len();

    let Some(end_idx) = template[after_start..].find(end_tag) else {
        return template.to_string();
    };
    let end_abs = after_start + end_idx;

    let before = &template[..start_idx];
    let body = &template[after_start..end_abs];
    let after = &template[end_abs + end_tag.len()..];

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
            let mut result = body_clean.clone();
            // Replace all {{item_name.prop}} patterns
            let prefix = format!("{{{{{}.", item_name);
            while let Some(var_start) = result.find(&prefix) {
                let var_end = result[var_start..].find("}}").unwrap_or(result.len()) + var_start;
                let var_name = &result[var_start + 2..var_end];
                let replacement = resolver(item, var_name);
                result = format!("{}{}{}", &result[..var_start], replacement, &result[var_end + 2..]);
            }
            result
        })
        .collect();

    format!("{}{}{}", before, expanded.join(&separator), after)
}

/// Expand step loops with nested action iteration.
fn expand_step_loop(template: &str, steps: &[&veil_ir::ast::FlowStep]) -> String {
    let start_tag = "{{for step in steps}}";
    let end_tag = "{{end}}";

    let Some(start_idx) = template.find(start_tag) else {
        return template.to_string();
    };
    let after_start = start_idx + start_tag.len();

    // Find the OUTERMOST end tag for the step loop
    let Some(end_idx) = template[after_start..].find(end_tag) else {
        return template.to_string();
    };
    let end_abs = after_start + end_idx;

    let before = &template[..start_idx];
    let body = &template[after_start..end_abs];
    let after = &template[end_abs + end_tag.len()..];

    let expanded: Vec<String> = steps
        .iter()
        .filter_map(|step| {
            match step {
                veil_ir::ast::FlowStep::Step(s) => {
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
                                let action_after = &result[ae_abs + action_end.len()..].to_string();
                                let action_before = &result[..as_idx].to_string();

                                let actions_expanded: Vec<String> = s.body.iter().map(|expr| {
                                    let mut ab = action_body.clone();
                                    let expr_display = veil_ir::builder::expr_to_display(expr);
                                    ab = ab.replace("{{emit_action(action)}}", &format!("    {};", expr_display));
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
        if after.starts_with('"') {
            let inner = &after[1..];
            if let Some(end) = inner.find('"') {
                return Some(inner[..end].to_string());
            }
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
