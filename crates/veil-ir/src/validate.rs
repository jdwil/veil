//! VEIL Validation — enforces layer constraints on the parsed AST.
//!
//! Constraints are declared in `.layer` files with a small generic grammar:
//!
//! - `only <Name>`            — children may only be constructs named <Name> (groups always allowed)
//! - `deny <Name>`            — constructs named <Name> may not appear as children
//! - `must_have <block>`      — a named sub-block (e.g. `root`) must be present
//! - `requires_groups`        — direct children must be groups
//!
//! Free-form constraint words the engine does not recognize emit a **one-shot
//! warning-style ValidationError** (INV-004) so layer authors notice missing
//! handlers. Preferred generic forms: `must_have_methods`, `must_have`, …

use crate::ast::*;
use crate::layer::{LayerRegistry, Shape};

/// A validation error with context.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Machine-stable rule id (e.g. `must_have`, `deny`).
    pub code: String,
    pub message: String,
    pub construct: String,
    pub parent: String,
    pub hint: Option<String>,
}

impl ValidationError {
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        construct: impl Into<String>,
        parent: impl Into<String>,
        hint: Option<String>,
    ) -> Self {
        ValidationError {
            code: code.into(),
            message: message.into(),
            construct: construct.into(),
            parent: parent.into(),
            hint,
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: [{}] in {}: {}",
            self.code, self.construct, self.parent, self.message
        )?;
        if let Some(hint) = &self.hint {
            write!(f, " (hint: {})", hint)?;
        }
        Ok(())
    }
}

/// Validate a parsed solution against the layer registry.
pub fn validate_solution(sol: &Solution, registry: &LayerRegistry) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    // Index all constructs by name for cross-reference rules (e.g. must_implement_port).
    let mut by_name: std::collections::HashMap<String, &Construct> =
        std::collections::HashMap::new();
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            index_constructs(c, &mut by_name);
        }
    }
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            validate_construct(c, "Solution", registry, &by_name, &mut errors);
        }
    }
    errors
}

fn index_constructs<'a>(
    c: &'a Construct,
    by_name: &mut std::collections::HashMap<String, &'a Construct>,
) {
    by_name.insert(c.name.clone(), c);
    for child in &c.children {
        index_constructs(child, by_name);
    }
}

fn validate_construct(
    c: &Construct,
    parent_name: &str,
    registry: &LayerRegistry,
    by_name: &std::collections::HashMap<String, &Construct>,
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
                            errors.push(ValidationError::new(
                                "only",
                                format!(
                                    "'{}' only allows {}, found '{}'",
                                    spec.name,
                                    allowed.join(", "),
                                    child.subkind
                                ),
                                child.name.clone(),
                                c.name.clone(),
                                Some(format!(
                                    "Move the '{}' to a construct that allows it",
                                    child.subkind
                                )),
                            ));
                        }
                    }
                }
                Some("deny") => {
                    let denied: Vec<&str> = words.collect();
                    for child in &effective {
                        if denied.iter().any(|d| registry.is_a(&child.keyword, d)) {
                            errors.push(ValidationError::new(
                                "deny",
                                format!(
                                    "'{}' is not allowed in '{}'",
                                    child.subkind, spec.name
                                ),
                                child.name.clone(),
                                c.name.clone(),
                                None,
                            ));
                        }
                    }
                }
                Some("must_have") => {
                    if let Some(block_kw) = words.next() {
                        let has = c.blocks.iter().any(|b| b.keyword == block_kw);
                        if !has {
                            errors.push(ValidationError::new(
                                "must_have",
                                format!(
                                    "'{}' must define a '{}' block",
                                    spec.name, block_kw
                                ),
                                c.name.clone(),
                                parent_name.to_string(),
                                Some(format!(
                                    "Add a '{}' block with the required fields",
                                    block_kw
                                )),
                            ));
                        }
                    }
                }
                Some("requires_groups") => {
                    for child in &direct {
                        if child.shape != Shape::Group {
                            errors.push(ValidationError::new(
                                "requires_groups",
                                format!(
                                    "'{}' must be inside a group, not directly in '{}'",
                                    child.subkind, spec.name
                                ),
                                child.name.clone(),
                                c.name.clone(),
                                Some("Wrap it in a 'group <name>' block".to_string()),
                            ));
                        }
                    }
                }
                // `deny_calls <shape|kind>` — check that function bodies
                // do NOT call constructs of the given shape/kind.
                // Used by functional.layer to enforce purity.
                Some("deny_calls") => {
                    let denied_targets: Vec<&str> = words.collect();
                    let mut call_targets = Vec::new();
                    collect_call_targets_from_construct(c, &mut call_targets);
                    for (target_name, call_location) in &call_targets {
                        if let Some(target_spec) = registry.construct(target_name) {
                            let shape_name = format!("{:?}", target_spec.shape).to_lowercase();
                            let is_denied = denied_targets.iter().any(|d| {
                                *d == shape_name
                                    || *d == target_spec.name.to_lowercase()
                                    || registry.is_a(&target_spec.keyword, d)
                            });
                            if is_denied {
                                errors.push(ValidationError::new(
                                    "deny_calls",
                                    format!(
                                        "'{}' in '{}' calls '{}' ({}) which is not allowed (deny_calls {})",
                                        call_location, c.name, target_name, target_spec.name,
                                        denied_targets.join(" ")
                                    ),
                                    c.name.clone(),
                                    parent_name.to_string(),
                                    Some(format!(
                                        "'{}' constructs cannot call {} targets",
                                        spec.name, denied_targets.join("/")
                                    )),
                                ));
                            }
                        }
                    }
                }
                // `has_identity <field>` — the construct must have the named field.
                Some("has_identity") => {
                    if let Some(id_field) = words.next() {
                        let all_fields: Vec<&str> = c.fields.iter()
                            .map(|f| f.name.as_str())
                            .chain(c.blocks.iter().flat_map(|b| b.fields.iter().map(|f| f.name.as_str())))
                            .collect();
                        if !all_fields.iter().any(|f| *f == id_field) {
                            errors.push(ValidationError::new(
                                "has_identity",
                                format!(
                                    "'{}' requires an identity field '{}' but it was not found",
                                    spec.name, id_field
                                ),
                                c.name.clone(),
                                parent_name.to_string(),
                                Some(format!("Add a '{}' field to the construct", id_field)),
                            ));
                        }
                    }
                }
                // `equality_by_value` / `no_identity` — no identity field (INV-006 policy name).
                Some("equality_by_value") | Some("no_identity") => {
                    let id_field = registry
                        .identity_policy
                        .identity_field
                        .as_deref()
                        .unwrap_or("id");
                    let all_fields: Vec<&str> = c.fields.iter()
                        .map(|f| f.name.as_str())
                        .chain(c.blocks.iter().flat_map(|b| b.fields.iter().map(|f| f.name.as_str())))
                        .collect();
                    if all_fields.iter().any(|f| *f == id_field) {
                        errors.push(ValidationError::new(
                            "equality_by_value",
                            format!(
                                "'{}' has equality_by_value but contains '{}' field (implies identity)",
                                c.name, id_field
                            ),
                            c.name.clone(),
                            parent_name.to_string(),
                            Some(format!(
                                "Value objects should not have a '{}' field — compared by all fields",
                                id_field
                            )),
                        ));
                    }
                }
                // `immutable` — no mut assignments / field assigns in methods.
                Some("immutable") => {
                    for f in &c.fns {
                        for expr in &f.body {
                            if let Expr::MutAssign(_, _, _) = expr {
                                errors.push(ValidationError::new(
                                    "immutable",
                                    format!(
                                        "'{}' is immutable but fn '{}' contains a mutable assignment",
                                        c.name, f.name
                                    ),
                                    c.name.clone(),
                                    parent_name.to_string(),
                                    Some(format!("'{}' constructs cannot mutate state", spec.name)),
                                ));
                            }
                            if let Expr::Assign(name, _, _) = expr {
                                let all_fields: Vec<&str> = c.fields.iter()
                                    .map(|f| f.name.as_str())
                                    .chain(c.blocks.iter().flat_map(|b| b.fields.iter().map(|f| f.name.as_str())))
                                    .collect();
                                if all_fields.contains(&name.as_str()) {
                                    errors.push(ValidationError::new(
                                        "immutable",
                                        format!(
                                            "'{}' is immutable but fn '{}' assigns to field '{}'",
                                            c.name, f.name, name
                                        ),
                                        c.name.clone(),
                                        parent_name.to_string(),
                                        Some(format!("'{}' constructs cannot mutate fields", spec.name)),
                                    ));
                                }
                            }
                        }
                    }
                }
                Some("mutations_through_methods") => {
                    let has_state = c.blocks.iter().any(|b| b.shape == Shape::Enum);
                    if has_state && c.fns.is_empty() {
                        errors.push(ValidationError::new(
                            "mutations_through_methods",
                            format!(
                                "'{}' has state but no methods to mutate it",
                                c.name
                            ),
                            c.name.clone(),
                            parent_name.to_string(),
                            Some("Add fn methods that transition state".to_string()),
                        ));
                    }
                }
                // `must_implement_port` — impl must cover all methods of its target trait.
                // Trait is resolved by name across the package (siblings, groups, etc.).
                // Empty monomorphized adapters (`adapter Foo for Trait<WearTest>`) may
                // inherit bodies from a pure generic template (`adapter Bar<T> for Trait<T>`)
                // — treat those template methods as covering the port.
                Some("must_implement_port") => {
                    if c.shape == Shape::Impl {
                        if let Some(target_name) = &c.target {
                            // Resolve target trait by name anywhere in the package.
                            let target_trait = by_name
                                .get(target_name)
                                .copied()
                                .filter(|t| t.shape == Shape::Trait);
                            if let Some(trait_construct) = target_trait {
                                let mut impl_methods: std::collections::HashSet<String> = c
                                    .impls
                                    .iter()
                                    .map(|m| m.method_name.clone())
                                    .collect();
                                // Inherit methods from a pure-generic template adapter
                                // for the same trait (codegen monomorphizes those bodies).
                                if !c.target_type_args.is_empty() {
                                    for other in by_name.values() {
                                        if other.name == c.name
                                            || other.shape != Shape::Impl
                                            || other.target.as_deref() != Some(target_name.as_str())
                                            || other.type_params.is_empty()
                                        {
                                            continue;
                                        }
                                        // Pure generic: all target args are type params.
                                        let tp: std::collections::HashSet<&str> = other
                                            .type_params
                                            .iter()
                                            .map(|p| p.split(':').next().unwrap_or(p).trim())
                                            .collect();
                                        let pure = other.target_type_args.is_empty()
                                            || other.target_type_args.iter().all(|a| {
                                                matches!(
                                                    a,
                                                    crate::ast::TypeExpr::Named(n)
                                                        if tp.contains(n.as_str())
                                                )
                                            });
                                        if pure {
                                            for m in &other.impls {
                                                if !m.body.is_empty() {
                                                    impl_methods.insert(m.method_name.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                                for method in &trait_construct.methods {
                                    let name = method.name.as_str();
                                    let covered = impl_methods.contains(name)
                                        || impl_methods.iter().any(|m| {
                                            m.trim_end_matches(['!', '?'])
                                                == name.trim_end_matches(['!', '?'])
                                        });
                                    if !covered {
                                        errors.push(ValidationError::new(
                                            "must_implement_port",
                                            format!(
                                                "'{}' does not implement method '{}' from '{}'",
                                                c.name, method.name, target_name
                                            ),
                                            c.name.clone(),
                                            parent_name.to_string(),
                                            Some(format!("Add 'impl {}(...)' to the adapter", method.name)),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                // INV-004: generic method-list constraint.
                // `must_have_methods find|save|delete` — preferred form.
                // `crud_for_aggregate` remains as a legacy alias for the same
                // default method list so existing layers keep working.
                Some("must_have_methods") => {
                    let required: Vec<String> = words
                        .flat_map(|s| s.split(|ch| ch == '|' || ch == ','))
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                    let method_names: Vec<&str> =
                        c.methods.iter().map(|m| m.name.as_str()).collect();
                    for req in &required {
                        if !method_names.iter().any(|m| m.starts_with(req.as_str())) {
                            errors.push(ValidationError::new(
                                "must_have_methods",
                                format!("'{}' requires a '{}' method", c.name, req),
                                c.name.clone(),
                                parent_name.to_string(),
                                Some(format!("Add a '{}' method to the construct", req)),
                            ));
                        }
                    }
                }
                Some("crud_for_aggregate") => {
                    // Legacy alias → same as must_have_methods find|save|delete
                    let required = ["find", "save", "delete"];
                    let method_names: Vec<&str> =
                        c.methods.iter().map(|m| m.name.as_str()).collect();
                    for req in &required {
                        if !method_names.iter().any(|m| m.starts_with(req)) {
                            errors.push(ValidationError::new(
                                "must_have_methods",
                                format!("'{}' requires a '{}' method", c.name, req),
                                c.name.clone(),
                                parent_name.to_string(),
                                Some(format!("Add a '{}' method (must_have_methods)", req)),
                            ));
                        }
                    }
                }
                Some("spans_contexts") => {
                    let has_refs = c.steps.iter().any(|s| {
                        if let FlowStep::Step(sd) = s { !sd.refs.is_empty() } else { false }
                    });
                    if !has_refs && c.refs.is_empty() {
                        errors.push(ValidationError::new(
                            "spans_contexts",
                            format!(
                                "'{}' must span multiple contexts (spans_contexts)",
                                c.name
                            ),
                            c.name.clone(),
                            parent_name.to_string(),
                            Some("Add 'contexts X, Y' or 'ctx X' references to steps".to_string()),
                        ));
                    }
                }
                Some("steps_have_compensation") => {
                    for step in &c.steps {
                        if let FlowStep::Step(sd) = step {
                            let has_compensate = sd.sub_blocks.iter()
                                .any(|sb| sb.keyword == "compensate");
                            if !has_compensate {
                                errors.push(ValidationError::new(
                                    "steps_have_compensation",
                                    format!(
                                        "Step '{}' in '{}' must have a 'compensate' block",
                                        sd.name, c.name
                                    ),
                                    c.name.clone(),
                                    parent_name.to_string(),
                                    Some("Add a 'compensate' sub-block with rollback logic".to_string()),
                                ));
                            }
                        }
                    }
                }
                // INV-004: unknown constraints warn once (not silent skip).
                // Skip constraints handled elsewhere (e.g. diagnostics::analyze).
                Some(other)
                    if !matches!(
                        other,
                        "requires_implementation"
                            | "immutable"
                            | "equality_by_value"
                            | "no_identity"
                            | "has_identity"
                    ) =>
                {
                    let key = format!("unknown_constraint:{}", other);
                    if !errors.iter().any(|e| e.code == key) {
                        errors.push(ValidationError::new(
                            &key,
                            format!(
                                "unknown constraint '{}' on '{}' — not enforced (layer debt)",
                                other, spec.name
                            ),
                            c.name.clone(),
                            parent_name.to_string(),
                            Some(
                                "Implement via generic primitives (must_have_methods, must_have, …) or register a handler"
                                    .into(),
                            ),
                        ));
                    }
                }
                Some(_) | None => {}
            }
        }

        // `contains` allow-list
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
                    errors.push(ValidationError::new(
                        "contains",
                        format!(
                            "'{}' is not allowed directly inside '{}'",
                            child.subkind, spec.name
                        ),
                        child.name.clone(),
                        c.name.clone(),
                        Some(format!("Allowed children: {}", spec.contains.join(", "))),
                    ));
                }
            }
        }
    }

    // Recurse (through groups too).
    for child in &c.children {
        validate_construct(child, &c.name, registry, by_name, errors);
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
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) | Expr::Return(rhs)
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
