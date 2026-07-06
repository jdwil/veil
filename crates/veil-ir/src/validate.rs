//! VEIL Validation — enforces layer constraints on the parsed AST.
//!
//! Constraints are declared in `.layer` files with a small generic grammar:
//!
//! - `only <Name>`            — children may only be constructs named <Name> (groups always allowed)
//! - `deny <Name>`            — constructs named <Name> may not appear as children
//! - `must_have <block>`      — a named sub-block (e.g. `root`) must be present
//! - `requires_groups`        — direct children must be groups
//!
//! Free-form constraint words the engine does not recognize (e.g.
//! `immutable`, `equality_by_value`) are treated as documentation/semantic
//! hints and skipped — they carry meaning for codegen plugins or humans, not
//! for the structural validator.

use crate::ast::*;
use crate::layer::{LayerRegistry, Shape};

/// A validation error with context.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub message: String,
    pub construct: String,
    pub parent: String,
    pub hint: Option<String>,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] in {}: {}", self.construct, self.parent, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " (hint: {})", hint)?;
        }
        Ok(())
    }
}

/// Validate a parsed solution against the layer registry.
pub fn validate_solution(sol: &Solution, registry: &LayerRegistry) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            validate_construct(c, "Solution", registry, &mut errors);
        }
    }
    errors
}

fn validate_construct(
    c: &Construct,
    parent_name: &str,
    registry: &LayerRegistry,
    errors: &mut Vec<ValidationError>,
) {
    let spec = registry.construct(&c.keyword);

    if let Some(spec) = spec {
        // Effective children: direct children, plus children inside groups
        // (groups are structural, constraints see through them).
        let direct: Vec<&Construct> = c.children.iter().collect();
        let effective: Vec<&Construct> = c
            .children
            .iter()
            .flat_map(|ch| {
                if ch.shape == Shape::Group {
                    ch.children.iter().collect::<Vec<_>>()
                } else {
                    vec![ch]
                }
            })
            .collect();

        for constraint in &spec.constraints {
            let mut words = constraint.split_whitespace();
            match words.next() {
                Some("only") => {
                    // Allowance follows the maps_to chain: a stacked construct
                    // (e.g. crm Playbook -> ddd Saga) satisfies `only Saga`.
                    let allowed: Vec<&str> = words.collect();
                    for child in &effective {
                        if !allowed
                            .iter()
                            .any(|a| registry.is_a(&child.keyword, a))
                        {
                            errors.push(ValidationError {
                                message: format!(
                                    "'{}' only allows {}, found '{}'",
                                    spec.name,
                                    allowed.join(", "),
                                    child.subkind
                                ),
                                construct: child.name.clone(),
                                parent: c.name.clone(),
                                hint: Some(format!(
                                    "Move the '{}' to a construct that allows it",
                                    child.subkind
                                )),
                            });
                        }
                    }
                }
                Some("deny") => {
                    let denied: Vec<&str> = words.collect();
                    for child in &effective {
                        if denied.iter().any(|d| registry.is_a(&child.keyword, d)) {
                            errors.push(ValidationError {
                                message: format!(
                                    "'{}' is not allowed in '{}'",
                                    child.subkind, spec.name
                                ),
                                construct: child.name.clone(),
                                parent: c.name.clone(),
                                hint: None,
                            });
                        }
                    }
                }
                Some("must_have") => {
                    if let Some(block_kw) = words.next() {
                        let has = c.blocks.iter().any(|b| b.keyword == block_kw);
                        if !has {
                            errors.push(ValidationError {
                                message: format!(
                                    "'{}' must define a '{}' block",
                                    spec.name, block_kw
                                ),
                                construct: c.name.clone(),
                                parent: parent_name.to_string(),
                                hint: Some(format!(
                                    "Add a '{}' block with the required fields",
                                    block_kw
                                )),
                            });
                        }
                    }
                }
                Some("requires_groups") => {
                    for child in &direct {
                        if child.shape != Shape::Group {
                            errors.push(ValidationError {
                                message: format!(
                                    "'{}' must be inside a group, not directly in '{}'",
                                    child.subkind, spec.name
                                ),
                                construct: child.name.clone(),
                                parent: c.name.clone(),
                                hint: Some("Wrap it in a 'group <name>' block".to_string()),
                            });
                        }
                    }
                }
                // `deny_calls <shape|kind>` — check that function bodies
                // do NOT call constructs of the given shape/kind.
                // Used by functional.layer to enforce purity.
                Some("deny_calls") => {
                    let denied_targets: Vec<&str> = words.collect();
                    // Check all fn bodies in this construct
                    let mut call_targets = Vec::new();
                    collect_call_targets_from_construct(c, &mut call_targets);
                    for (target_name, call_location) in &call_targets {
                        // Check if the target is a known construct of a denied shape
                        if let Some(target_spec) = registry.construct(target_name) {
                            let shape_name = format!("{:?}", target_spec.shape).to_lowercase();
                            let is_denied = denied_targets.iter().any(|d| {
                                *d == shape_name
                                    || *d == target_spec.name.to_lowercase()
                                    || registry.is_a(&target_spec.keyword, d)
                            });
                            if is_denied {
                                errors.push(ValidationError {
                                    message: format!(
                                        "'{}' in '{}' calls '{}' ({}) which is not allowed (deny_calls {})",
                                        call_location, c.name, target_name, target_spec.name,
                                        denied_targets.join(" ")
                                    ),
                                    construct: c.name.clone(),
                                    parent: parent_name.to_string(),
                                    hint: Some(format!(
                                        "'{}' constructs cannot call {} targets",
                                        spec.name, denied_targets.join("/")
                                    )),
                                });
                            }
                        }
                    }
                }
                // Unrecognized constraint words are semantic hints, not
                // structural rules — skip.
                _ => {}
            }
        }

        // `contains` allow-list: when declared, children must match one of
        // the entries (by construct name, block keyword, or shape name).
        if !spec.contains.is_empty() {
            for child in &effective {
                let allowed = spec.contains.iter().any(|entry| {
                    let e = entry.trim_end_matches("[]");
                    registry.is_a(&child.keyword, e)
                        || e == child.shape.name()
                        || entry.starts_with("group ")
                        || e.split(':').next().map(|s| s.trim()) == Some(child.keyword.as_str())
                });
                if !allowed && child.shape != Shape::Group {
                    errors.push(ValidationError {
                        message: format!(
                            "'{}' is not allowed directly inside '{}'",
                            child.subkind, spec.name
                        ),
                        construct: child.name.clone(),
                        parent: c.name.clone(),
                        hint: Some(format!("Allowed children: {}", spec.contains.join(", "))),
                    });
                }
            }
        }
    }

    // Recurse (through groups too).
    for child in &c.children {
        validate_construct(child, &c.name, registry, errors);
    }
}

/// Collect all call target names from a construct's function bodies.
/// Returns (target_name, location_description) pairs.
fn collect_call_targets_from_construct(c: &Construct, targets: &mut Vec<(String, String)>) {
    // Check step bodies
    for step in &c.steps {
        if let FlowStep::Step(s) = step {
            for expr in &s.body {
                collect_call_targets_from_expr(expr, &s.name, targets);
            }
        }
    }
    // Check fn bodies
    for f in &c.fns {
        for expr in &f.body {
            collect_call_targets_from_expr(expr, &f.name, targets);
        }
    }
}

/// Recursively collect call targets from an expression.
fn collect_call_targets_from_expr(expr: &Expr, location: &str, targets: &mut Vec<(String, String)>) {
    match expr {
        Expr::Call(call) => {
            if !call.target.is_empty() {
                targets.push((call.target.clone(), location.to_string()));
            }
            for arg in &call.args {
                collect_call_targets_from_expr(arg, location, targets);
            }
        }
        Expr::Assign(_, rhs) | Expr::MutAssign(_, rhs) | Expr::Return(rhs)
        | Expr::Await(rhs) | Expr::Try(rhs) => {
            collect_call_targets_from_expr(rhs, location, targets);
        }
        Expr::BinaryOp(op) => {
            collect_call_targets_from_expr(&op.left, location, targets);
            collect_call_targets_from_expr(&op.right, location, targets);
        }
        Expr::IfExpr(ie) => {
            collect_call_targets_from_expr(&ie.condition, location, targets);
            for e in &ie.then_body { collect_call_targets_from_expr(e, location, targets); }
            if let Some(eb) = &ie.else_body {
                for e in eb { collect_call_targets_from_expr(e, location, targets); }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            collect_call_targets_from_expr(iterable, location, targets);
            for e in body { collect_call_targets_from_expr(e, location, targets); }
        }
        Expr::WhileLoop { condition, body } => {
            collect_call_targets_from_expr(condition, location, targets);
            for e in body { collect_call_targets_from_expr(e, location, targets); }
        }
        Expr::Loop(body) => {
            for e in body { collect_call_targets_from_expr(e, location, targets); }
        }
        Expr::Closure { body, .. } => {
            for e in body { collect_call_targets_from_expr(e, location, targets); }
        }
        Expr::Action(a) => {
            if !a.target.is_empty() {
                targets.push((a.target.clone(), location.to_string()));
            }
        }
        _ => {}
    }
}
