//! Pass executor — walks the AST and applies layer-declared pre/post pass rules.
//!
//! Pre-passes annotate AST nodes before the engine backend runs.
//! Post-passes augment output after the engine backend runs.

use std::collections::HashMap;

use veil_ir::ast::{Construct, Solution, TopLevelItem};
use veil_ir::layer::{LayerRegistry, PassPhase, PassSpec, RuleAction, RuleSpec};

use crate::pass_eval::{evaluate_predicate, NodeContext};

/// Trace entry: records what a pass/rule did to a node.
#[derive(Debug, Clone)]
pub struct PassTraceEntry {
    pub pass_name: String,
    pub rule_name: String,
    pub construct_name: String,
    pub construct_kind: String,
    pub action_desc: String,
}

/// Execute all pre-passes: runs before the engine backend.
/// Walks the AST, evaluates predicates, and applies annotations.
/// Returns trace entries if tracing is enabled.
pub fn execute_pre_passes(
    solution: &mut Solution,
    registry: &LayerRegistry,
    trace: bool,
) -> Vec<PassTraceEntry> {
    execute_passes(solution, registry, PassPhase::Pre, trace)
}

/// Execute all post-passes: runs after the engine backend.
/// Currently only annotates (post-pass output transformation is future work).
pub fn execute_post_passes(
    solution: &mut Solution,
    registry: &LayerRegistry,
    trace: bool,
) -> Vec<PassTraceEntry> {
    execute_passes(solution, registry, PassPhase::Post, trace)
}

fn execute_passes(
    solution: &mut Solution,
    registry: &LayerRegistry,
    phase: PassPhase,
    trace: bool,
) -> Vec<PassTraceEntry> {
    let mut traces = Vec::new();

    // Collect and sort passes by priority (lower = first).
    let mut passes: Vec<&PassSpec> = registry.passes.iter()
        .filter(|p| p.phase == phase)
        .collect();
    passes.sort_by_key(|p| p.priority);

    for pass in passes {
        for rule in &pass.rules {
            for item in &mut solution.items {
                if let TopLevelItem::Construct(c) = item {
                    apply_rule_to_construct(c, pass, rule, registry, trace, &mut traces);
                }
            }
        }
    }

    traces
}

/// Recursively walk a construct and its children, applying a rule.
fn apply_rule_to_construct(
    construct: &mut Construct,
    pass: &PassSpec,
    rule: &RuleSpec,
    registry: &LayerRegistry,
    trace: bool,
    traces: &mut Vec<PassTraceEntry>,
) {
    // Build context for this construct
    let ctx = build_construct_context(construct, registry);

    // Evaluate the predicate
    if evaluate_predicate(&rule.when, &ctx) {
        // Apply all actions
        for action in &rule.actions {
            apply_action(construct, action);
            if trace {
                traces.push(PassTraceEntry {
                    pass_name: pass.name.clone(),
                    rule_name: rule.name.clone(),
                    construct_name: construct.name.clone(),
                    construct_kind: construct.subkind.clone(),
                    action_desc: describe_action(action),
                });
            }
        }
    }

    // Recurse into children
    for child in &mut construct.children {
        apply_rule_to_construct(child, pass, rule, registry, trace, traces);
    }
}

/// Build a NodeContext from a Construct for predicate evaluation.
fn build_construct_context(construct: &Construct, registry: &LayerRegistry) -> NodeContext {
    let mut ctx = NodeContext::new();

    // construct.kind — the resolved shape name
    ctx.set_str("construct.kind", construct.shape.name());
    // construct.keyword
    ctx.set_str("construct.keyword", &construct.keyword);
    // construct.subkind (layer name like "Aggregate")
    ctx.set_str("construct.subkind", &construct.subkind);
    // construct.name (instance name like "Customer")
    ctx.set_str("construct.name", &construct.name);

    // construct.has_annotation("X") — check authored annotations
    for ann in &construct.annotations {
        let key = format!("construct.has_annotation(\"{}\")", ann.name);
        ctx.set_bool(&key, true);
    }

    // construct.has_role("X") — check layer-declared roles
    if let Some(spec) = registry.spec_for_construct(construct) {
        for role in &spec.roles {
            let key = format!("construct.has_role(\"{}\")", role);
            ctx.set_bool(&key, true);
        }
    }

    // Boolean properties
    ctx.set_bool("construct.exported", construct.exported);
    ctx.set_bool("construct.layer_provided", construct.layer_provided);
    ctx.set_bool("construct.deployment_unit", construct.deployment_unit);

    // Numeric properties
    ctx.set_num("construct.field_count", construct.fields.len() as i64);
    ctx.set_num("construct.method_count", construct.methods.len() as i64);
    ctx.set_num("construct.fn_count", construct.fns.len() as i64);
    ctx.set_num("construct.child_count", construct.children.len() as i64);

    ctx
}

/// Apply a rule action to a construct.
fn apply_action(construct: &mut Construct, action: &RuleAction) {
    match action {
        RuleAction::Annotate { key, value } => {
            construct.pass_annotations.insert(key.clone(), value.clone());
        }
        RuleAction::Wrap(_kind) => {
            // Wrap actions apply to expressions, not constructs.
            // For now, store as a pass annotation hint.
            construct.pass_annotations.insert(
                "__wrap".to_string(),
                format!("{:?}", _kind),
            );
        }
        RuleAction::Remove => {
            construct.pass_annotations.insert("__remove".to_string(), "true".to_string());
        }
    }
}

/// Human-readable description of a rule action for tracing.
fn describe_action(action: &RuleAction) -> String {
    match action {
        RuleAction::Annotate { key, value } => format!("annotate: {} = \"{}\"", key, value),
        RuleAction::Wrap(kind) => format!("wrap: {:?}", kind),
        RuleAction::Remove => "remove".to_string(),
    }
}

/// Get the effective value of a pass annotation for a construct.
/// Checks the construct's pass_annotations first (per-construct override from passes),
/// then falls back to the provided global default (from template system).
///
/// This unifies emit_to/fn_attrs/derives threading: both the template system
/// and the pass system can contribute, with pass annotations taking precedence.
pub fn effective_annotation<'a>(construct: &'a Construct, key: &str, global_default: Option<&'a str>) -> Option<&'a str> {
    construct.pass_annotations.get(key).map(|s| s.as_str()).or(global_default)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::ast::*;
    use veil_ir::layer::*;
    use veil_ir::span::Span;

    fn span() -> Span { Span::new(0, 0) }

    fn make_solution_with_construct(keyword: &str, subkind: &str, name: &str) -> Solution {
        Solution {
            name: "test".into(),
            span: span(),
            uses: Vec::new(),
            links: Vec::new(),
            items: vec![TopLevelItem::Construct(
                Construct::new(keyword, subkind, Shape::Struct, name.into(), span()),
            )],
            expose: None,
            guidance: Vec::new(),
        }
    }

    #[test]
    fn pre_pass_applies_annotation() {
        let mut sol = make_solution_with_construct("agg", "Aggregate", "Customer");

        let mut reg = LayerRegistry::builtin();
        reg.passes.push(PassSpec {
            name: "test_pass".into(),
            priority: 10,
            phase: PassPhase::Pre,
            rules: vec![RuleSpec {
                name: "mark_structs".into(),
                when: r#"construct.kind == "struct""#.into(),
                actions: vec![RuleAction::Annotate {
                    key: "ownership".into(),
                    value: "value".into(),
                }],
            }],
            layer: "test".into(),
        });

        let traces = execute_pre_passes(&mut sol, &reg, true);

        // Should have applied the annotation
        if let TopLevelItem::Construct(c) = &sol.items[0] {
            assert_eq!(c.pass_annotations.get("ownership"), Some(&"value".to_string()));
        } else {
            panic!("expected construct");
        }

        // Trace should record the action
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].pass_name, "test_pass");
        assert_eq!(traces[0].rule_name, "mark_structs");
        assert_eq!(traces[0].construct_name, "Customer");
    }

    #[test]
    fn predicate_filters_correctly() {
        let mut sol = make_solution_with_construct("port", "Port", "UserPort");
        // Change shape to Trait
        if let TopLevelItem::Construct(c) = &mut sol.items[0] {
            c.shape = Shape::Trait;
        }

        let mut reg = LayerRegistry::builtin();
        reg.passes.push(PassSpec {
            name: "struct_only".into(),
            priority: 10,
            phase: PassPhase::Pre,
            rules: vec![RuleSpec {
                name: "only_structs".into(),
                when: r#"construct.kind == "struct""#.into(),
                actions: vec![RuleAction::Annotate {
                    key: "matched".into(),
                    value: "yes".into(),
                }],
            }],
            layer: "test".into(),
        });

        execute_pre_passes(&mut sol, &reg, false);

        // Should NOT have applied (port is trait-shaped, not struct)
        if let TopLevelItem::Construct(c) = &sol.items[0] {
            assert!(c.pass_annotations.is_empty());
        }
    }

    #[test]
    fn priority_ordering() {
        let mut sol = make_solution_with_construct("agg", "Aggregate", "Order");

        let mut reg = LayerRegistry::builtin();
        // Lower priority = runs first
        reg.passes.push(PassSpec {
            name: "second".into(),
            priority: 20,
            phase: PassPhase::Pre,
            rules: vec![RuleSpec {
                name: "r".into(),
                when: r#"construct.kind == "struct""#.into(),
                actions: vec![RuleAction::Annotate { key: "order".into(), value: "second".into() }],
            }],
            layer: "test".into(),
        });
        reg.passes.push(PassSpec {
            name: "first".into(),
            priority: 10,
            phase: PassPhase::Pre,
            rules: vec![RuleSpec {
                name: "r".into(),
                when: r#"construct.kind == "struct""#.into(),
                actions: vec![RuleAction::Annotate { key: "order".into(), value: "first".into() }],
            }],
            layer: "test".into(),
        });

        execute_pre_passes(&mut sol, &reg, false);

        // "second" (priority 20) overwrites "first" (priority 10) since it runs later
        if let TopLevelItem::Construct(c) = &sol.items[0] {
            assert_eq!(c.pass_annotations.get("order"), Some(&"second".to_string()));
        }
    }

    #[test]
    fn post_pass_only_runs_in_post_phase() {
        let mut sol = make_solution_with_construct("svc", "Service", "UserService");

        let mut reg = LayerRegistry::builtin();
        reg.passes.push(PassSpec {
            name: "post_only".into(),
            priority: 10,
            phase: PassPhase::Post, // post phase!
            rules: vec![RuleSpec {
                name: "r".into(),
                when: r#"construct.kind == "struct""#.into(),
                actions: vec![RuleAction::Annotate { key: "post".into(), value: "yes".into() }],
            }],
            layer: "test".into(),
        });

        // Pre-passes should not execute post-phase passes
        execute_pre_passes(&mut sol, &reg, false);
        if let TopLevelItem::Construct(c) = &sol.items[0] {
            assert!(c.pass_annotations.is_empty());
        }

        // Post-passes should execute them
        execute_post_passes(&mut sol, &reg, false);
        if let TopLevelItem::Construct(c) = &sol.items[0] {
            assert_eq!(c.pass_annotations.get("post"), Some(&"yes".to_string()));
        }
    }
}
