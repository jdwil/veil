//! Static coverage analysis for VEIL testing framework.
//!
//! Analyzes the VEIL AST to determine which functions, branches, and expose
//! nodes have corresponding test blocks. No code execution needed — this is
//! purely static analysis.

use std::collections::HashSet;

use crate::ast::*;
use crate::layer::Shape;

/// Coverage report computed from static AST analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageReport {
    pub functions: CoverageMetric,
    pub branches: CoverageMetric,
    pub nodes: CoverageMetric,
    pub uncovered: Vec<UncoveredItem>,
}

/// A single coverage metric.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageMetric {
    pub covered: usize,
    pub total: usize,
    pub percent: f64,
}

/// An item that lacks test coverage.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UncoveredItem {
    pub kind: String,
    pub name: String,
    pub line: usize,
}

/// Compute static coverage from the VEIL AST.
///
/// Inspects:
/// - Function coverage: fn/svc/handler-shaped constructs that have a TestBlock
/// - Branch coverage: if/match arms in tested functions, checking if different
///   `given` inputs would exercise different branches
/// - Node coverage: expose block nodes that have at least one test path
pub fn compute_coverage(sol: &Solution) -> CoverageReport {
    // Collect all test targets (names referenced by TestBlock targets).
    let tested_targets: HashSet<String> = sol
        .items
        .iter()
        .filter_map(|item| {
            if let TopLevelItem::TestBlock(tb) = item {
                tb.target.clone()
            } else {
                None
            }
        })
        .collect();

    // Collect the number of given variants per tested target.
    let mut given_count_per_target: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for item in &sol.items {
        if let TopLevelItem::TestBlock(tb) = item {
            if let Some(target) = &tb.target {
                let count = given_count_per_target.entry(target.clone()).or_insert(0);
                *count += tb.cases.len();
            }
        }
    }

    // ─── Function coverage ──────────────────────────────────────────────
    let mut fn_total = 0usize;
    let mut fn_covered = 0usize;
    let mut uncovered = Vec::new();

    // Count fn-shaped constructs and free functions.
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) if c.shape == Shape::Fn => {
                fn_total += 1;
                if tested_targets.contains(&c.name) {
                    fn_covered += 1;
                } else {
                    uncovered.push(UncoveredItem {
                        kind: "fn".into(),
                        name: c.name.clone(),
                        line: c.span.start,
                    });
                }
            }
            TopLevelItem::Construct(c) if c.shape == Shape::Struct || c.shape == Shape::Mod => {
                // Check nested fns inside struct-shaped constructs (services, handlers).
                for f in &c.fns {
                    fn_total += 1;
                    let qualified = format!("{}.{}", c.name, f.name);
                    if tested_targets.contains(&c.name) || tested_targets.contains(&qualified) {
                        fn_covered += 1;
                    } else {
                        uncovered.push(UncoveredItem {
                            kind: "fn".into(),
                            name: qualified,
                            line: f.span.start,
                        });
                    }
                }
            }
            TopLevelItem::Function(f) => {
                fn_total += 1;
                if tested_targets.contains(&f.name) {
                    fn_covered += 1;
                } else {
                    uncovered.push(UncoveredItem {
                        kind: "fn".into(),
                        name: f.name.clone(),
                        line: f.span.start,
                    });
                }
            }
            _ => {}
        }
    }

    // ─── Branch coverage ────────────────────────────────────────────────
    let mut branch_total = 0usize;
    let mut branch_covered = 0usize;

    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) if c.shape == Shape::Fn => {
                let is_tested = tested_targets.contains(&c.name);
                let test_count = given_count_per_target.get(&c.name).copied().unwrap_or(0);
                count_branches_in_steps(&c.fns, &c.name, is_tested, test_count,
                    &tested_targets, &given_count_per_target,
                    &mut branch_total, &mut branch_covered, &mut uncovered);
            }
            TopLevelItem::Construct(c) if c.shape == Shape::Struct || c.shape == Shape::Mod => {
                count_branches_in_steps(&c.fns, &c.name, false, 0,
                    &tested_targets, &given_count_per_target,
                    &mut branch_total, &mut branch_covered, &mut uncovered);
            }
            TopLevelItem::Function(f) => {
                let is_tested = tested_targets.contains(&f.name);
                let test_count = given_count_per_target.get(&f.name).copied().unwrap_or(0);
                let branches = count_branches_in_exprs(&f.body);
                branch_total += branches;
                if is_tested && test_count >= branches && branches > 0 {
                    branch_covered += branches;
                } else if is_tested && branches > 0 {
                    // Partially covered: at least one branch exercised per test case.
                    branch_covered += test_count.min(branches);
                    if test_count < branches {
                        uncovered.push(UncoveredItem {
                            kind: "branch".into(),
                            name: format!("{} ({} of {} branches)", f.name, test_count, branches),
                            line: f.span.start,
                        });
                    }
                } else if branches > 0 {
                    uncovered.push(UncoveredItem {
                        kind: "branch".into(),
                        name: f.name.clone(),
                        line: f.span.start,
                    });
                }
            }
            _ => {}
        }
    }

    // ─── Node coverage (expose block) ───────────────────────────────────
    let mut node_total = 0usize;
    let mut node_covered = 0usize;

    if let Some(expose) = &sol.expose {
        for node in &expose.nodes {
            node_total += 1;
            if tested_targets.contains(&node.name) {
                node_covered += 1;
            } else {
                uncovered.push(UncoveredItem {
                    kind: "node".into(),
                    name: node.name.clone(),
                    line: node.span.start,
                });
            }
        }
    }

    CoverageReport {
        functions: CoverageMetric {
            covered: fn_covered,
            total: fn_total,
            percent: if fn_total == 0 { 100.0 } else { (fn_covered as f64 / fn_total as f64) * 100.0 },
        },
        branches: CoverageMetric {
            covered: branch_covered,
            total: branch_total,
            percent: if branch_total == 0 { 100.0 } else { (branch_covered as f64 / branch_total as f64) * 100.0 },
        },
        nodes: CoverageMetric {
            covered: node_covered,
            total: node_total,
            percent: if node_total == 0 { 100.0 } else { (node_covered as f64 / node_total as f64) * 100.0 },
        },
        uncovered,
    }
}

/// Count branches in a list of FnDef items.
fn count_branches_in_steps(
    fns: &[FnDef],
    parent_name: &str,
    parent_tested: bool,
    parent_test_count: usize,
    tested_targets: &HashSet<String>,
    given_counts: &std::collections::HashMap<String, usize>,
    branch_total: &mut usize,
    branch_covered: &mut usize,
    uncovered: &mut Vec<UncoveredItem>,
) {
    for f in fns {
        let qualified = format!("{}.{}", parent_name, f.name);
        let is_tested = parent_tested || tested_targets.contains(&qualified);
        let test_count = given_counts.get(&qualified).copied().unwrap_or(parent_test_count);
        let branches = count_branches_in_exprs(&f.body)
            + count_branches_in_flow_steps(&f.steps);
        *branch_total += branches;
        if is_tested && test_count >= branches && branches > 0 {
            *branch_covered += branches;
        } else if is_tested && branches > 0 {
            *branch_covered += test_count.min(branches);
            if test_count < branches {
                uncovered.push(UncoveredItem {
                    kind: "branch".into(),
                    name: format!("{} ({} of {} branches)", qualified, test_count, branches),
                    line: f.span.start,
                });
            }
        } else if branches > 0 {
            uncovered.push(UncoveredItem {
                kind: "branch".into(),
                name: qualified,
                line: f.span.start,
            });
        }
    }
}

/// Count branches (if/match arms) in a list of expressions.
fn count_branches_in_exprs(exprs: &[Expr]) -> usize {
    let mut count = 0;
    for expr in exprs {
        count += count_branches_in_expr(expr);
    }
    count
}

/// Count branches in a single expression (recursive).
fn count_branches_in_expr(expr: &Expr) -> usize {
    match expr {
        Expr::IfExpr(data) => {
            // An if/else has 2 branches.
            let mut count = 2;
            count += count_branches_in_exprs(&data.then_body);
            if let Some(else_body) = &data.else_body {
                count += count_branches_in_exprs(else_body);
            }
            count
        }
        Expr::Match(_, arms) => {
            let mut count = arms.len();
            for arm in arms {
                count += count_branches_in_exprs(&arm.body);
            }
            count
        }
        Expr::ForLoop { body, .. } => count_branches_in_exprs(body),
        Expr::WhileLoop { body, .. } => count_branches_in_exprs(body),
        Expr::Loop(body) => count_branches_in_exprs(body),
        Expr::Closure { body, .. } => count_branches_in_exprs(body),
        _ => 0,
    }
}

/// Count branches in flow steps (match blocks within steps).
fn count_branches_in_flow_steps(steps: &[FlowStep]) -> usize {
    let mut count = 0;
    for step in steps {
        match step {
            FlowStep::Match(mb) => {
                count += mb.arms.len();
                for arm in &mb.arms {
                    count += count_branches_in_exprs(&arm.body);
                }
            }
            _ => {}
        }
    }
    count
}
