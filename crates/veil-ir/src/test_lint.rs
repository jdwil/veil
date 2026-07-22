//! Test-related lint rules (check pipeline).
//!
//! Detects common testing anti-patterns and missing pieces in the VEIL
//! testing framework AST:
//!
//! - Stubs declared but never exercised by `given`/`then`
//! - `wait <ms>` in unit/component tests (suggest `settles`)
//! - `spy` without corresponding `stub`
//! - Test cases with no `then` block (test does nothing)
//! - Fixtures defined but never referenced

use std::collections::HashSet;

use crate::ast::*;
use crate::diagnostics::{Diagnostic, Severity};
use crate::span::Span;

/// Diagnostic codes for test-lint rules (stable for filtering).
pub mod codes {
    pub const UNUSED_STUB: &str = "test_unused_stub";
    pub const WAIT_IN_UNIT: &str = "test_wait_in_unit";
    pub const SPY_WITHOUT_STUB: &str = "test_spy_without_stub";
    pub const NO_THEN: &str = "test_no_then";
    pub const UNUSED_FIXTURE: &str = "test_unused_fixture";
}

/// Run all test-related lint rules on a parsed solution.
pub fn check_tests(sol: &Solution) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Collect fixture names for the unused-fixture check.
    let mut fixture_names: HashSet<String> = HashSet::new();
    let mut fixture_spans: Vec<(String, Span)> = Vec::new();

    for item in &sol.items {
        match item {
            TopLevelItem::Fixture(fix) => {
                fixture_names.insert(fix.name.clone());
                fixture_spans.push((fix.name.clone(), fix.span));
            }
            TopLevelItem::TestBlock(tb) => {
                check_test_block(tb, &mut diagnostics);
            }
            TopLevelItem::Construct(c) => {
                lint_construct_tests(c, &mut diagnostics);
            }
            _ => {}
        }
    }

    // Check fixture usage across all test blocks.
    if !fixture_names.is_empty() {
        let referenced = collect_fixture_references(sol);
        for (name, span) in &fixture_spans {
            if !referenced.contains(name.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("fixture '{}' is defined but never referenced", name),
                    node_id: None,
                    node_name: Some(name.clone()),
                    code: codes::UNUSED_FIXTURE.into(),
                    constraint: codes::UNUSED_FIXTURE.into(),
                    parent: None,
                    hint: Some("Remove the unused fixture or reference it in a test case".into()),
                    span_start: Some(span.start),
                    span_end: Some(span.end),
                });
            }
        }
    }

    diagnostics
}

/// Check a single test block for lint issues.
fn check_test_block(tb: &TestBlock, diagnostics: &mut Vec<Diagnostic>) {
    for case in &tb.cases {
        check_test_case(case, diagnostics);
    }
}

/// Recursively lint test blocks inside constructs.
fn lint_construct_tests(c: &Construct, diagnostics: &mut Vec<Diagnostic>) {
    for tb in &c.test_blocks {
        check_test_block(tb, diagnostics);
    }
    for child in &c.children {
        lint_construct_tests(child, diagnostics);
    }
}

/// Check a single test case.
fn check_test_case(case: &TestCase, diagnostics: &mut Vec<Diagnostic>) {
    // Rule: test case with no `then` block (test does nothing).
    if case.then.is_empty() && case.spies.iter().all(|s| s.assertions.is_empty()) {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: format!("test case '{}' has no `then` block (test does nothing)", case.name),
            node_id: None,
            node_name: Some(case.name.clone()),
            code: codes::NO_THEN.into(),
            constraint: codes::NO_THEN.into(),
            parent: None,
            hint: Some("Add a `then` block with assertions to verify behavior".into()),
            span_start: Some(case.span.start),
            span_end: Some(case.span.end),
        });
    }

    // Collect stub targets for cross-reference.
    let stub_targets: HashSet<&str> = case.stubs.iter().map(|s| s.target.as_str()).collect();

    // Rule: spy without corresponding stub.
    for spy in &case.spies {
        if !stub_targets.contains(spy.target.as_str()) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!(
                    "spy on '{}' has no corresponding stub — spy cannot intercept without a stub",
                    spy.target
                ),
                node_id: None,
                node_name: Some(case.name.clone()),
                code: codes::SPY_WITHOUT_STUB.into(),
                constraint: codes::SPY_WITHOUT_STUB.into(),
                parent: None,
                hint: Some(format!("Add `stub {}` before the spy declaration", spy.target)),
                span_start: Some(spy.span.start),
                span_end: Some(spy.span.end),
            });
        }
    }

    // Rule: stub declared but never exercised by given/then.
    // A stub is "exercised" if it appears referenced in the given bindings or
    // then assertions — i.e., if the test actually calls through the stubbed path.
    // For simplicity: a stub is exercised if:
    //   - there's a `given` block (which calls the function under test), OR
    //   - there's a `then` block (assertions that would fail without the stub)
    // A stub with NEITHER given nor then is purely dead.
    let has_given = !case.given.is_empty();
    let has_then = !case.then.is_empty();
    if !has_given && !has_then {
        for stub in &case.stubs {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!(
                    "stub '{}' declared but never exercised (no `given` or `then` block)",
                    stub.target
                ),
                node_id: None,
                node_name: Some(case.name.clone()),
                code: codes::UNUSED_STUB.into(),
                constraint: codes::UNUSED_STUB.into(),
                parent: None,
                hint: Some("Add `given` inputs and `then` assertions that exercise this stub".into()),
                span_start: Some(stub.span.start),
                span_end: Some(stub.span.end),
            });
        }
    }

    // Rule: `wait <ms>` in a unit/component test (suggest `settles`).
    // A test is unit/component if it has NO scenario steps — it's a TestCase
    // inside a TestBlock, not a ScenarioBlock.
    let is_unit_or_component = true; // TestCase is always unit or component level
    if is_unit_or_component {
        for action in &case.actions {
            if let TestAction::Wait(ms) = action {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "use of `wait {}` in unit/component test — prefer `settles` for async resolution",
                        ms
                    ),
                    node_id: None,
                    node_name: Some(case.name.clone()),
                    code: codes::WAIT_IN_UNIT.into(),
                    constraint: codes::WAIT_IN_UNIT.into(),
                    parent: None,
                    hint: Some(
                        "`settles` flushes async without arbitrary delays; `wait` is for E2E scenarios".into(),
                    ),
                    span_start: Some(case.span.start),
                    span_end: Some(case.span.end),
                });
            }
        }
    }
}

/// Collect all fixture names referenced in test blocks (given bindings, assertions).
/// A fixture is "referenced" if its name appears as an identifier in any expression
/// within the test cases.
fn collect_fixture_references(sol: &Solution) -> HashSet<String> {
    let mut referenced = HashSet::new();
    for item in &sol.items {
        if let TopLevelItem::TestBlock(tb) = item {
            for case in &tb.cases {
                collect_refs_in_case(case, &mut referenced);
            }
        }
        if let TopLevelItem::Integration(integ) = item {
            collect_refs_in_exprs(&integ.setup, &mut referenced);
            collect_refs_in_exprs(&integ.verify, &mut referenced);
            collect_refs_in_exprs(&integ.teardown, &mut referenced);
        }
    }
    referenced
}

/// Scan a test case for fixture name references.
fn collect_refs_in_case(case: &TestCase, referenced: &mut HashSet<String>) {
    for binding in &case.given {
        collect_refs_in_expr(&binding.value, referenced);
    }
    for assertion in &case.then {
        match assertion {
            Assertion::ResultEq(expr) | Assertion::Expr(expr) => {
                collect_refs_in_expr(expr, referenced);
            }
            Assertion::FieldEq(_, expr) => {
                collect_refs_in_expr(expr, referenced);
            }
            _ => {}
        }
    }
}

/// Recursively collect identifiers from an expression (potential fixture refs).
fn collect_refs_in_expr(expr: &Expr, referenced: &mut HashSet<String>) {
    match expr {
        Expr::Ident(name) => {
            referenced.insert(name.clone());
        }
        Expr::Call(call) => {
            if !call.target.is_empty() {
                referenced.insert(call.target.clone());
            }
            if let Some(recv) = &call.receiver {
                collect_refs_in_expr(recv, referenced);
            }
            for arg in &call.args {
                collect_refs_in_expr(arg, referenced);
            }
        }
        Expr::FieldAccess(base, _) => {
            collect_refs_in_expr(base, referenced);
        }
        Expr::StructLit(name, fields) => {
            referenced.insert(name.clone());
            for (_, val) in fields {
                collect_refs_in_expr(val, referenced);
            }
        }
        Expr::ArrayLit(elems) => {
            for e in elems {
                collect_refs_in_expr(e, referenced);
            }
        }
        Expr::BinaryOp(binop) => {
            collect_refs_in_expr(&binop.left, referenced);
            collect_refs_in_expr(&binop.right, referenced);
        }
        Expr::UnaryOp(unop) => {
            collect_refs_in_expr(&unop.expr, referenced);
        }
        Expr::Await(inner) => {
            collect_refs_in_expr(inner, referenced);
        }
        Expr::Tuple(elems) => {
            for e in elems {
                collect_refs_in_expr(e, referenced);
            }
        }
        Expr::Index(base, idx) => {
            collect_refs_in_expr(base, referenced);
            collect_refs_in_expr(idx, referenced);
        }
        _ => {}
    }
}

/// Collect refs from a list of expressions (used for integration blocks).
fn collect_refs_in_exprs(exprs: &[Expr], referenced: &mut HashSet<String>) {
    for e in exprs {
        collect_refs_in_expr(e, referenced);
    }
}
