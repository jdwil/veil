//! Unified check pipeline — the single entry point for structural validation
//! and graph diagnostics.
//!
//! Used by the CLI (`veil check`), the HTTP server (`/api/diagnostics`), and
//! post-edit validation so agents and humans always see the same truth.
//!
//! # Severity
//!
//! - **Error** — structural constraint violations; process exit ≠ 0; edits rejected
//! - **Warning** — advisory (e.g. missing implementation); printed but do not fail check

use crate::ast::Solution;
use crate::builder::build_ir_with_registry;
use crate::diagnostics::{self, Diagnostic, Severity};
use crate::ir::IrGraph;
use crate::layer::LayerRegistry;
use crate::validate::{self, ValidationError};
use serde::Serialize;

/// Result of checking a solution.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
    /// IR graph built during check (also used by the viewer).
    #[serde(skip)]
    pub graph: IrGraph,
}

impl CheckResult {
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Warning))
            .count()
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    /// Diagnostics with error severity only.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
    }
}

/// Run the full check pipeline: structural AST validation + IR graph analysis.
///
/// This is the **only** entry point consumers should call for “is this package OK?”
pub fn check_solution(sol: &Solution, registry: &LayerRegistry) -> CheckResult {
    let graph = build_ir_with_registry(sol, Some(registry));

    let mut diagnostics: Vec<Diagnostic> = validate::validate_solution(sol, registry)
        .into_iter()
        .map(validation_to_diagnostic)
        .collect();

    diagnostics.extend(diagnostics::analyze(&graph, registry));
    diagnostics.extend(crate::names::check_names(sol, registry));
    diagnostics.extend(crate::names::check_stub_type_refs(sol, registry));
    diagnostics.extend(crate::names::check_layer_declare_shadows(sol, registry));
    diagnostics.extend(crate::harness_check::check_harness(sol, registry));
    diagnostics.extend(crate::typecheck::check_types(sol, registry));
    diagnostics.extend(crate::escape::check_escape_hatches(sol, registry));
    diagnostics.extend(crate::test_lint::check_tests(sol));
    diagnostics.extend(crate::version_pin::check_version_pins(sol, registry));
    diagnostics.extend(check_error_model(sol, registry));

    // Parser-emitted notes. `sol` is removed — treat as a hard error so agents
    // cannot flip-flop Sol vs Pkg. Other guidance stays non-blocking.
    for g in &sol.guidance {
        let hard_guidance = g.code == "sol_removed"
            || g.code == "prefer_terse"
            || g.code == "pkg_body_unindented"
            || g.code == "unattached_source";
        diagnostics.push(Diagnostic {
            severity: if hard_guidance {
                Severity::Error
            } else {
                Severity::Guidance
            },
            message: g.message.clone(),
            node_id: None,
            node_name: None,
            code: if g.code == "prefer_terse" {
                "sol_removed".into()
            } else {
                g.code.clone()
            },
            constraint: g.code.clone(),
            parent: None,
            hint: Some(g.hint.clone()),
            span_start: Some(g.span.start),
            span_end: Some(g.span.end),
        });
    }

    sort_diagnostics(&mut diagnostics);

    CheckResult {
        diagnostics,
        graph,
    }
}

/// Re-sort diagnostics (errors first, then code, message).
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|a, b| {
        let sa = severity_rank(&a.severity);
        let sb = severity_rank(&b.severity);
        sa.cmp(&sb)
            .then_with(|| a.code.cmp(&b.code))
            .then_with(|| a.message.cmp(&b.message))
    });
}

fn severity_rank(s: &Severity) -> u8 {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Guidance => 2,
    }
}

fn validation_to_diagnostic(err: ValidationError) -> Diagnostic {
    // INV-004: unknown constraint notices are warnings, not hard errors.
    let severity = if err.code.starts_with("unknown_constraint:") {
        Severity::Warning
    } else {
        Severity::Error
    };
    Diagnostic {
        severity,
        message: err.message.clone(),
        node_id: None,
        node_name: Some(err.construct.clone()),
        code: err.code.clone(),
        constraint: err.code.clone(),
        parent: Some(err.parent),
        hint: err.hint,
        span_start: None,
        span_end: None,
    }
}

/// Format a diagnostic as a single compact line for CLI output.
///
/// ```text
/// error[must_have] Customer: 'Aggregate' must define a 'root' block
/// warning[requires_implementation] UserRepo: Port 'UserRepo' has no implementation
/// ```
pub fn format_diagnostic_line(d: &Diagnostic) -> String {
    let sev = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Guidance => "guidance",
    };
    let where_ = d
        .node_name
        .as_deref()
        .or(d.parent.as_deref())
        .unwrap_or("?");
    let mut line = format!("{}[{}] {}: {}", sev, d.code, where_, d.message);
    if let Some(parent) = &d.parent {
        if d.node_name.as_deref() != Some(parent.as_str()) {
            line.push_str(&format!(" (in {})", parent));
        }
    }
    if let Some(id) = d.node_id {
        line.push_str(&format!(" [node:{}]", id));
    }
    if let (Some(s), Some(e)) = (d.span_start, d.span_end) {
        line.push_str(&format!(" [span:{}..{}]", s, e));
    }
    if let Some(hint) = &d.hint {
        line.push_str(&format!(" — hint: {}", hint));
    }
    line
}

/// Machine-friendly diagnostic item (ACS-008).
///
/// Shape: `{ code, severity, message, span?, hint?, node_name? }`.
#[derive(Debug, Clone, Serialize)]
pub struct StructuredDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<StructuredSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
}

/// Byte-offset span for agent-targeted fixes.
#[derive(Debug, Clone, Serialize)]
pub struct StructuredSpan {
    pub start: usize,
    pub end: usize,
}

impl From<&Diagnostic> for StructuredDiagnostic {
    fn from(d: &Diagnostic) -> Self {
        let span = match (d.span_start, d.span_end) {
            (Some(start), Some(end)) => Some(StructuredSpan { start, end }),
            _ => None,
        };
        StructuredDiagnostic {
            code: d.code.clone(),
            severity: match d.severity {
                Severity::Error => "error".into(),
                Severity::Warning => "warning".into(),
                Severity::Guidance => "guidance".into(),
            },
            message: d.message.clone(),
            span,
            hint: d.hint.clone(),
            node_name: d.node_name.clone(),
        }
    }
}

/// Full structured check report for agents (ACS-008).
#[derive(Debug, Clone, Serialize)]
pub struct StructuredCheckReport {
    pub ok: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub diagnostics: Vec<StructuredDiagnostic>,
}

impl StructuredCheckReport {
    pub fn from_diagnostics(diagnostics: &[Diagnostic]) -> Self {
        let error_count = diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Warning))
            .count();
        Self {
            ok: error_count == 0,
            error_count,
            warning_count,
            diagnostics: diagnostics.iter().map(StructuredDiagnostic::from).collect(),
        }
    }

    pub fn from_check_result(result: &CheckResult) -> Self {
        Self::from_diagnostics(&result.diagnostics)
    }

    /// Pretty JSON for tool / CLI output.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Compact one-line summary (optional prefix before JSON body).
    pub fn summary_line(&self) -> String {
        format!(
            "check: {} error(s), {} warning(s) — {}",
            self.error_count,
            self.warning_count,
            if self.ok { "OK" } else { "FAIL" }
        )
    }
}

/// Build a parse-error diagnostic with span when available (ACS-008).
pub fn parse_error_diagnostic(message: impl Into<String>, start: usize, end: usize) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message: message.into(),
        node_id: None,
        node_name: None,
        code: "parse_error".into(),
        constraint: "parse_error".into(),
        parent: None,
        hint: Some("Fix syntax at the reported span before re-running check.".into()),
        span_start: Some(start),
        span_end: Some(end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::layer::{ConstructSpec, LayerRegistry, Shape, Visual};
    use crate::span::Span;

    fn empty_visual() -> Visual {
        Visual {
            icon: String::new(),
            color: String::new(),
            label: String::new(),
        }
    }

    fn spec(kw: &str, name: &str, shape: Shape, constraints: &[&str]) -> ConstructSpec {
        ConstructSpec {
            name: name.to_string(),
            keyword: kw.to_string(),
            maps_to: shape.name().to_string(),
            shape,
            layer: "test".to_string(),
            desc: String::new(),
            contains: Vec::new(),
            blocks: Vec::new(),
            raw_block_keywords: Vec::new(),
            constraints: constraints.iter().map(|s| s.to_string()).collect(),
            allowed_in: "any".to_string(),
            group: String::new(),
            visual: empty_visual(),
            au: false,
                is_step: false,
                step_fields: Vec::new(),
            annotations: Vec::new(),
            runtime: None,
            tgt: String::new(),
            dg: String::new(),
            presentation: Default::default(),
            roles: Vec::new(),
            config_keys: Vec::new(),
            required_fields: Vec::new(),
            lowers_to: std::collections::HashMap::new(),
        }
    }

    fn test_registry(extra: Vec<ConstructSpec>) -> LayerRegistry {
        let mut reg = LayerRegistry::builtin();
        for s in extra {
            if let Some(i) = reg.constructs.iter().position(|c| c.keyword == s.keyword) {
                reg.constructs[i] = s;
            } else {
                reg.constructs.push(s);
            }
        }
        reg.layers.push("test".to_string());
        reg
    }

    fn sol_with(root: Construct) -> Solution {
        Solution {
            name: "Test".to_string(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(root)],
            expose: None,
            guidance: Vec::new(),
        }
    }

    #[test]
    fn sol_keyword_is_hard_error() {
        let mut sol = sol_with(Construct::new(
            "struct",
            "Struct",
            Shape::Struct,
            "User".into(),
            Span::new(0, 0),
        ));
        sol.guidance.push(crate::ast::GuidanceDiagnostic {
            code: "sol_removed".into(),
            message: "`sol` is removed — use `pkg`".into(),
            hint: "Replace `sol` with `pkg`".into(),
            span: Span::new(0, 3),
        });
        let r = check_solution(&sol, &LayerRegistry::builtin());
        assert!(
            r.diagnostics.iter().any(|d| {
                d.code == "sol_removed" && matches!(d.severity, Severity::Error)
            }),
            "{:?}",
            r.diagnostics
        );
    }

    #[test]
    fn structured_report_includes_code_severity_span() {
        let d = Diagnostic {
            severity: Severity::Error,
            message: "expected Int, found Str".into(),
            node_id: None,
            node_name: Some("svc".into()),
            code: "type_mismatch".into(),
            constraint: "type_mismatch".into(),
            parent: None,
            hint: Some("coerce or change the type".into()),
            span_start: Some(10),
            span_end: Some(20),
        };
        let report = StructuredCheckReport::from_diagnostics(&[d]);
        assert!(!report.ok);
        assert_eq!(report.error_count, 1);
        let json = report.to_json_pretty();
        assert!(json.contains("type_mismatch"));
        assert!(json.contains("\"start\": 10"));
        assert!(json.contains("\"end\": 20"));
        assert!(json.contains("coerce or change"));
        let pe = parse_error_diagnostic("unexpected token", 5, 6);
        assert_eq!(pe.code, "parse_error");
        assert_eq!(pe.span_start, Some(5));
    }

    #[test]
    fn unindented_pkg_guidance_is_error() {
        let sol = Solution {
            name: "Empty".into(),
            span: Span::new(0, 0),
            uses: vec![],
            links: vec![],
            items: vec![],
            expose: None,
            guidance: vec![crate::ast::GuidanceDiagnostic {
                code: "pkg_body_unindented".into(),
                message: "Package body is not indented under `pkg`".into(),
                hint: "Indent by 2 spaces".into(),
                span: Span::new(0, 4),
            }],
        };
        let result = check_solution(&sol, &LayerRegistry::builtin());
        assert!(result.has_errors(), "{:?}", result.diagnostics);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "pkg_body_unindented"
                    && matches!(d.severity, Severity::Error)),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn requires_groups_emits_error() {
        let reg = test_registry(vec![spec("box", "Box", Shape::Mod, &["requires_groups"])]);
        let mut root = Construct::new("box", "Box", Shape::Mod, "Outer".into(), Span::new(0, 0));
        root.children.push(Construct::new(
            "struct",
            "Struct",
            Shape::Struct,
            "Bare".into(),
            Span::new(0, 0),
        ));
        let result = check_solution(&sol_with(root), &reg);
        assert!(result.has_errors(), "{:?}", result.diagnostics);
        assert!(
            result.diagnostics.iter().any(|d| d.code == "requires_groups"),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn must_have_emits_error() {
        let reg = test_registry(vec![spec(
            "agg",
            "Aggregate",
            Shape::Struct,
            &["must_have root"],
        )]);
        let root = Construct::new(
            "agg",
            "Aggregate",
            Shape::Struct,
            "Customer".into(),
            Span::new(0, 0),
        );
        let result = check_solution(&sol_with(root), &reg);
        assert!(result.has_errors());
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.code == "must_have")
            .expect("must_have diagnostic");
        assert!(d.message.contains("root"), "{}", d.message);
        assert_eq!(d.node_name.as_deref(), Some("Customer"));
    }

    #[test]
    fn deny_emits_error() {
        let reg = test_registry(vec![spec("box", "Box", Shape::Mod, &["deny struct"])]);
        let mut root = Construct::new("box", "Box", Shape::Mod, "Outer".into(), Span::new(0, 0));
        let mut g = Construct::new(
            "group",
            "Group",
            Shape::Group,
            "domain".into(),
            Span::new(0, 0),
        );
        g.children.push(Construct::new(
            "struct",
            "Struct",
            Shape::Struct,
            "Nope".into(),
            Span::new(0, 0),
        ));
        root.children.push(g);
        let result = check_solution(&sol_with(root), &reg);
        assert!(result.has_errors(), "{:?}", result.diagnostics);
        assert!(result.diagnostics.iter().any(|d| d.code == "deny"));
    }

    #[test]
    fn must_implement_port_emits_error() {
        let reg = test_registry(vec![
            spec("port", "Port", Shape::Trait, &[]),
            spec("adapter", "Adapter", Shape::Impl, &["must_implement_port"]),
        ]);
        let mut root = Construct::new("mod", "Module", Shape::Mod, "Pkg".into(), Span::new(0, 0));
        let mut trait_c = Construct::new(
            "port",
            "Port",
            Shape::Trait,
            "Repo".into(),
            Span::new(0, 0),
        );
        trait_c.methods.push(Method {
            name: "save".into(),
            params: Vec::new(),
            return_type: None,
            span: Span::new(0, 0),
            is_async: false,
        });
        let mut impl_c = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "PgRepo".into(),
            Span::new(0, 0),
        );
        impl_c.target = Some("Repo".into());
        // intentionally empty impls
        root.children.push(trait_c);
        root.children.push(impl_c);
        let result = check_solution(&sol_with(root), &reg);
        assert!(result.has_errors(), "{:?}", result.diagnostics);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "must_implement_port" && d.message.contains("save")),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn requires_implementation_emits_warning() {
        let reg = test_registry(vec![spec(
            "port",
            "Port",
            Shape::Trait,
            &["requires_implementation"],
        )]);
        let root = Construct::new(
            "port",
            "Port",
            Shape::Trait,
            "UserRepo".into(),
            Span::new(0, 0),
        );
        let result = check_solution(&sol_with(root), &reg);
        assert!(!result.has_errors(), "should be warning not error");
        assert_eq!(result.warning_count(), 1);
        let d = &result.diagnostics[0];
        assert!(matches!(d.severity, Severity::Warning));
        assert_eq!(d.code, "requires_implementation");
        assert_eq!(d.node_name.as_deref(), Some("UserRepo"));
        assert!(d.node_id.is_some());
    }

    #[test]
    fn clean_solution_has_no_diagnostics() {
        let reg = test_registry(vec![spec(
            "agg",
            "Aggregate",
            Shape::Struct,
            &["must_have root"],
        )]);
        let mut root = Construct::new(
            "agg",
            "Aggregate",
            Shape::Struct,
            "Customer".into(),
            Span::new(0, 0),
        );
        root.blocks.push(NamedBlock {
            keyword: "root".into(),
            shape: Shape::Struct,
            name: None,
            fields: Vec::new(),
            variants: Vec::new(),
            transitions: Vec::new(),
            span: Span::new(0, 0),
        });
        let result = check_solution(&sol_with(root), &reg);
        assert!(
            !result.has_errors() && result.warning_count() == 0,
            "{:?}",
            result.diagnostics
        );
    }

    // ─── Required fields (has field: Type) ─────────────────────────────

    #[test]
    fn entity_without_required_id_emits_diagnostic() {
        let mut s = spec("ent", "Entity", Shape::Struct, &[]);
        s.required_fields = vec![("id".to_string(), "Id".to_string())];
        let reg = test_registry(vec![s]);
        // Entity with no id field
        let ent = Construct::new(
            "ent",
            "Entity",
            Shape::Struct,
            "Customer".into(),
            Span::new(0, 0),
        );
        let result = check_solution(&sol_with(ent), &reg);
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.code == "required_field")
            .expect("required_field diagnostic should be emitted");
        assert!(d.message.contains("Customer"), "{}", d.message);
        assert!(d.message.contains("id: Id"), "{}", d.message);
    }

    #[test]
    fn entity_with_required_id_passes() {
        let mut s = spec("ent", "Entity", Shape::Struct, &[]);
        s.required_fields = vec![("id".to_string(), "Id".to_string())];
        let reg = test_registry(vec![s]);
        // Entity with id field
        let mut ent = Construct::new(
            "ent",
            "Entity",
            Shape::Struct,
            "Customer".into(),
            Span::new(0, 0),
        );
        ent.fields.push(Field {
            annotations: Vec::new(),
            name: "id".to_string(),
            type_expr: TypeExpr::Named("Id".into()),
            default_expr: None,
            span: Span::new(0, 0),
        });
        let result = check_solution(&sol_with(ent), &reg);
        assert!(
            !result.diagnostics.iter().any(|d| d.code == "required_field"),
            "should not emit required_field diagnostic when field is present: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn aggregate_with_id_in_block_passes() {
        let mut s = spec("agg", "Aggregate", Shape::Struct, &[]);
        s.required_fields = vec![("id".to_string(), "Id".to_string())];
        let reg = test_registry(vec![s]);
        // Aggregate with id in root block (not top-level fields)
        let mut agg = Construct::new(
            "agg",
            "Aggregate",
            Shape::Struct,
            "Order".into(),
            Span::new(0, 0),
        );
        agg.blocks.push(NamedBlock {
            keyword: "root".into(),
            shape: Shape::Struct,
            name: None,
            fields: vec![Field {
                annotations: Vec::new(),
                name: "id".to_string(),
                type_expr: TypeExpr::Named("Id".into()),
                default_expr: None,
                span: Span::new(0, 0),
            }],
            variants: Vec::new(),
            transitions: Vec::new(),
            span: Span::new(0, 0),
        });
        let result = check_solution(&sol_with(agg), &reg);
        assert!(
            !result.diagnostics.iter().any(|d| d.code == "required_field"),
            "should find id in named block: {:?}",
            result.diagnostics
        );
    }

    // ─── Error model tests (CHK-EM) ───────────────────────────────────────────

    #[test]
    fn no_error_model_with_res_emits_diagnostic() {
        let reg = LayerRegistry::builtin();
        let mut svc = Construct::new("trait", "Trait", Shape::Trait, "OrderRepo".into(), Span::new(0, 0));
        svc.methods.push(crate::ast::Method {
            name: "find".into(),
            params: Vec::new(),
            return_type: Some(TypeExpr::Result(Some(Box::new(TypeExpr::Named("Order".into()))))),
            span: Span::new(0, 0),
            is_async: false,
        });
        let sol = sol_with(svc);
        // Direct check of our function:
        let em_diags = super::check_error_model(&sol, &reg);
        assert!(
            em_diags.iter().any(|d| d.code == "missing_error_model"),
            "check_error_model should emit diagnostic, got: {:?}",
            em_diags
        );
        // Full check pipeline should also contain it:
        let result = check_solution(&sol, &reg);
        assert!(
            result.diagnostics.iter().any(|d| d.code == "missing_error_model"),
            "expected missing_error_model diagnostic in full pipeline: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn error_model_present_no_diagnostic() {
        use crate::layer::ErrorModelPolicy;
        let mut reg = LayerRegistry::builtin();
        reg.error_model = Some(ErrorModelPolicy {
            type_name: "AppError".into(),
            variants: vec![
                ("external".into(), "Upstream".into()),
                ("not_found".into(), "Missing".into()),
                ("validation".into(), "BadInput".into()),
            ],
        });
        let mut svc = Construct::new("trait", "Trait", Shape::Trait, "OrderRepo".into(), Span::new(0, 0));
        svc.methods.push(crate::ast::Method {
            name: "find".into(),
            params: Vec::new(),
            return_type: Some(TypeExpr::Result(Some(Box::new(TypeExpr::Named("Order".into()))))),
            span: Span::new(0, 0),
            is_async: false,
        });
        let result = check_solution(&sol_with(svc), &reg);
        assert!(
            !result.diagnostics.iter().any(|d| d.code == "missing_error_model"),
            "should not emit missing_error_model when error model is declared: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn no_res_usage_no_diagnostic() {
        let reg = LayerRegistry::builtin();
        let mut svc = Construct::new("trait", "Trait", Shape::Trait, "Notifier".into(), Span::new(0, 0));
        svc.methods.push(crate::ast::Method {
            name: "send".into(),
            params: Vec::new(),
            return_type: Some(TypeExpr::Named("String".into())),
            span: Span::new(0, 0),
            is_async: false,
        });
        let result = check_solution(&sol_with(svc), &reg);
        assert!(
            !result.diagnostics.iter().any(|d| d.code == "missing_error_model"),
            "should not emit diagnostic when no Res! usage: {:?}",
            result.diagnostics
        );
    }
}

/// CHK-EM: If any construct/method uses `Res!` (TypeExpr::Result) in return types,
/// the package must have a layer that declares an `error_model`. Otherwise codegen
/// would need to invent an error type — which violates the zero-domain-knowledge invariant.
fn check_error_model(sol: &Solution, registry: &LayerRegistry) -> Vec<Diagnostic> {
    use crate::ast::{Construct, TopLevelItem, TypeExpr};

    // If a layer already provides an error model, nothing to check.
    if registry.error_model.is_some() {
        return Vec::new();
    }

    // Scan for any usage of Res! (TypeExpr::Result) in the solution.
    fn type_uses_result(ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Result(_))
    }

    fn construct_uses_result(c: &Construct) -> Option<&str> {
        for m in &c.methods {
            if let Some(rt) = &m.return_type {
                if type_uses_result(rt) {
                    return Some(&c.name);
                }
            }
        }
        for f in &c.fns {
            if let Some(rt) = &f.return_type {
                if type_uses_result(rt) {
                    return Some(&c.name);
                }
            }
        }
        for child in &c.children {
            if let Some(name) = construct_uses_result(child) {
                return Some(name);
            }
        }
        None
    }

    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => {
                if let Some(name) = construct_uses_result(c) {
                    return vec![Diagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "no error model declared — Res! in '{}' requires a layer that provides \
                             error_model {{ ... }}",
                            name
                        ),
                        node_id: None,
                        node_name: Some(name.to_string()),
                        code: "missing_error_model".into(),
                        constraint: "missing_error_model".into(),
                        parent: None,
                        hint: Some(
                            "Add `use ddd` (which declares DomainError) or declare a custom \
                             error_model in your layer."
                                .into(),
                        ),
                        span_start: None,
                        span_end: None,
                    }];
                }
            }
            TopLevelItem::Function(f) if !f.layer_provided => {
                if let Some(rt) = &f.return_type {
                    if type_uses_result(rt) {
                        return vec![Diagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "no error model declared — Res! in function '{}' requires a layer \
                                 that provides error_model {{ ... }}",
                                f.name
                            ),
                            node_id: None,
                            node_name: Some(f.name.clone()),
                            code: "missing_error_model".into(),
                            constraint: "missing_error_model".into(),
                            parent: None,
                            hint: Some(
                                "Add `use ddd` (which declares DomainError) or declare a \
                                 custom error_model in your layer."
                                    .into(),
                            ),
                            span_start: None,
                            span_end: None,
                        }];
                    }
                }
            }
            _ => {}
        }
    }

    Vec::new()
}
