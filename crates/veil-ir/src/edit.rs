//! Structured edits applied to a parsed `Solution` AST.
//!
//! The viewer never writes raw text. Instead it sends a structured [`EditOp`]
//! keyed by **AST span start** (`node.span.start` in the IR graph). The IR
//! builder stamps each construct, step, method-impl, and free function with its
//! parse span, so `span_start` uniquely identifies the target across IR rebuilds
//! for a given source revision. Edits are **not** keyed by ephemeral IR node ids.
//!
//! The server applies the edit to the AST, re-serializes, re-parses/checks, and
//! only then writes back — a failed body parse or validation never corrupts the
//! file on disk.
//!
//! # Body expressions ([`EditOp::SetBody`])
//!
//! Each string in `body` is a VEIL expression (possibly multi-line for `if` /
//! `match`). Callers that can parse real expressions should use
//! [`apply_edits_with`]; the server/CLI pass `veil_parser::parse_expr_str`.
//! Opaque `Ident` fallback is intentionally **not** used — invalid lines fail
//! the edit with [`EditError::InvalidBody`].
//!
//! This module is generic: it edits by core shape and never encodes domain
//! vocabulary. Field/method types use `parse_type_str`.

use serde::{Deserialize, Serialize};

use crate::ast::*;
use crate::span::Span;

/// A single structured edit targeting a node whose AST span starts at
/// `span_start` (or `parent_span` for create).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    /// Rename the construct (and, where relevant, its `name`).
    Rename { span_start: usize, name: String },
    /// Replace the full annotation set on the construct.
    SetAnnotations { span_start: usize, annotations: Vec<String> },
    /// Replace the fields of a struct-shaped construct.
    SetFields { span_start: usize, fields: Vec<FieldSpec> },
    /// Replace the method signatures of a trait-shaped construct.
    SetMethods { span_start: usize, methods: Vec<MethodSpec> },
    /// Create a new child construct inside the parent at `parent_span`.
    /// The construct is inserted as a child with the given keyword, name, and
    /// optional target (for impl-shaped constructs).
    CreateConstruct {
        parent_span: usize,
        keyword: String,
        name: String,
        /// For impl-shaped: the name of the trait being implemented.
        #[serde(default)]
        target: Option<String>,
    },
    /// Replace the expression body of a step, free fn, method-impl, or
    /// fn-shaped construct identified by `span_start`.
    ///
    /// `body` is a list of VEIL expression source strings (one statement each)
    /// that are parsed into real [`Expr`] nodes via [`apply_edits_with`].
    SetBody {
        span_start: usize,
        body: Vec<String>,
    },
    /// Remove a construct (and its children), free function, flow, or step
    /// identified by `span_start`. Layer-provided infrastructure cannot be
    /// deleted. Keyed by AST span start (not IR node id) — see module docs.
    DeleteConstruct { span_start: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSpec {
    pub name: String,
    /// Type in VEIL display form (e.g. "Str", "UUID", "Opt<Customer>").
    #[serde(rename = "type")]
    pub type_str: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodSpec {
    pub name: String,
    pub params: Vec<FieldSpec>,
    /// Return type in VEIL display form; empty for none.
    #[serde(default)]
    pub return_type: String,
}

/// Error applying an edit.
#[derive(Debug)]
pub enum EditError {
    /// No construct / body target found at the given span start.
    TargetNotFound(usize),
    /// The target construct's shape does not support this edit.
    ShapeMismatch { span_start: usize, expected: &'static str },
    /// A construct with this name already exists in the parent.
    DuplicateName(String),
    /// A body expression line failed to parse into a real [`Expr`].
    InvalidBody {
        span_start: usize,
        line: usize,
        message: String,
    },
    /// Delete refused (e.g. layer-provided infrastructure).
    RefuseDelete { span_start: usize, reason: String },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::TargetNotFound(s) => write!(f, "no construct found at span {}", s),
            EditError::ShapeMismatch { span_start, expected } => {
                write!(f, "construct at span {} is not {}", span_start, expected)
            }
            EditError::DuplicateName(name) => {
                write!(f, "a construct named '{}' already exists in this scope", name)
            }
            EditError::InvalidBody {
                span_start,
                line,
                message,
            } => write!(
                f,
                "invalid body at span {} line {}: {}",
                span_start, line, message
            ),
            EditError::RefuseDelete { span_start, reason } => {
                write!(f, "cannot delete span {}: {}", span_start, reason)
            }
        }
    }
}

impl std::error::Error for EditError {}

/// Apply a batch of edits. For [`EditOp::SetBody`], expression strings are
/// parsed with a **no-op failure** parser — use [`apply_edits_with`] (or
/// `veil_parser::apply_edits`) so bodies become real AST.
pub fn apply_edits(sol: &mut Solution, ops: &[EditOp]) -> Result<(), EditError> {
    apply_edits_with(sol, ops, |src| {
        Err(format!(
            "body expression parse requires apply_edits_with / veil_parser::apply_edits ({src:?})"
        ))
    })
}

/// Apply edits, parsing each [`EditOp::SetBody`] line with `parse_expr`.
///
/// `parse_expr` receives one body string (trimmed); on success it returns a
/// real [`Expr`]. On failure the whole edit aborts and the solution must be
/// discarded (server re-parses from disk source on the next request).
pub fn apply_edits_with<F>(
    sol: &mut Solution,
    ops: &[EditOp],
    mut parse_expr: F,
) -> Result<(), EditError>
where
    F: FnMut(&str) -> Result<Expr, String>,
{
    for op in ops {
        apply_edit_with(sol, op, &mut parse_expr)?;
    }
    Ok(())
}

/// Apply a single edit (uses the failing body parser — prefer [`apply_edit_with`]).
pub fn apply_edit(sol: &mut Solution, op: &EditOp) -> Result<(), EditError> {
    apply_edit_with(sol, op, &mut |src| {
        Err(format!(
            "body expression parse requires apply_edit_with ({src:?})"
        ))
    })
}

/// Apply a single edit with a body expression parser.
pub fn apply_edit_with<F>(
    sol: &mut Solution,
    op: &EditOp,
    parse_expr: &mut F,
) -> Result<(), EditError>
where
    F: FnMut(&str) -> Result<Expr, String>,
{
    // CreateConstruct targets a parent, not the construct itself.
    if let EditOp::CreateConstruct {
        parent_span,
        keyword,
        name,
        target,
    } = op
    {
        let trait_methods: Vec<Method> = if let Some(target_name) = target {
            find_trait_methods_in_solution(&sol.items, target_name)
        } else {
            Vec::new()
        };

        let parent = find_construct_mut(&mut sol.items, *parent_span)
            .ok_or(EditError::TargetNotFound(*parent_span))?;
        let shape = if target.is_some() {
            Shape::Impl
        } else {
            Shape::from_name(keyword).unwrap_or(Shape::Struct)
        };
        // New nodes get a zero span until re-parse; subsequent edits key by the
        // span assigned on the next parse (server returns fresh IR after save).
        let mut child = Construct::new(keyword, keyword, shape, name.clone(), Span::new(0, 0));
        child.target = target.clone();
        for m in trait_methods {
            child.impls.push(MethodImpl {
                method_name: m.name.clone(),
                params: m.params.iter().map(|p| p.name.clone()).collect(),
                span: Span::new(0, 0),
                body: Vec::new(),
            });
        }
        if parent.children.iter().any(|c| c.name == *name) {
            return Err(EditError::DuplicateName(name.clone()));
        }
        parent.children.push(child);
        return Ok(());
    }

    if let EditOp::SetBody { body, span_start } = op {
        let mut exprs = Vec::new();
        for (i, line) in body.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match parse_expr(trimmed) {
                Ok(e) => exprs.push(e),
                Err(message) => {
                    return Err(EditError::InvalidBody {
                        span_start: *span_start,
                        line: i + 1,
                        message,
                    });
                }
            }
        }
        let slot = find_body_mut(&mut sol.items, *span_start)
            .ok_or(EditError::TargetNotFound(*span_start))?;
        *slot = exprs;
        return Ok(());
    }

    if let EditOp::DeleteConstruct { span_start } = op {
        return delete_by_span(&mut sol.items, *span_start);
    }

    let span_start = op.span_start();
    let target = find_construct_mut(&mut sol.items, span_start)
        .ok_or(EditError::TargetNotFound(span_start))?;
    match op {
        EditOp::Rename { name, .. } => {
            target.name = name.clone();
            Ok(())
        }
        EditOp::SetAnnotations { annotations, .. } => {
            target.annotations = annotations
                .iter()
                .map(|a| parse_annotation_str(a))
                .collect();
            Ok(())
        }
        EditOp::SetFields { fields, span_start } => {
            if target.shape != Shape::Struct {
                return Err(EditError::ShapeMismatch {
                    span_start: *span_start,
                    expected: "a struct-shaped construct",
                });
            }
            target.fields = fields
                .iter()
                .map(|f| Field {
                    annotations: Vec::new(),
                    name: f.name.clone(),
                    type_expr: parse_type_str(&f.type_str),
                    default_expr: None,
                    span: Span::new(0, 0),
                })
                .collect();
            Ok(())
        }
        EditOp::SetMethods { methods, span_start } => {
            if target.shape != Shape::Trait {
                return Err(EditError::ShapeMismatch {
                    span_start: *span_start,
                    expected: "a trait-shaped construct",
                });
            }
            target.methods = methods
                .iter()
                .map(|m| Method {
                    name: m.name.clone(),
                    params: m
                        .params
                        .iter()
                        .map(|p| Param {
                            name: p.name.clone(),
                            type_expr: parse_type_str(&p.type_str),
                            span: Span::new(0, 0),
                        })
                        .collect(),
                    return_type: if m.return_type.is_empty() {
                        None
                    } else {
                        Some(parse_type_str(&m.return_type))
                    },
                    span: Span::new(0, 0),
                })
                .collect();
            Ok(())
        }
        EditOp::CreateConstruct { .. }
        | EditOp::SetBody { .. }
        | EditOp::DeleteConstruct { .. } => unreachable!("handled above"),
    }
}

impl EditOp {
    pub fn span_start(&self) -> usize {
        match self {
            EditOp::Rename { span_start, .. }
            | EditOp::SetAnnotations { span_start, .. }
            | EditOp::SetFields { span_start, .. }
            | EditOp::SetMethods { span_start, .. }
            | EditOp::SetBody { span_start, .. }
            | EditOp::DeleteConstruct { span_start } => *span_start,
            EditOp::CreateConstruct { parent_span, .. } => *parent_span,
        }
    }
}

use crate::layer::Shape;

/// Depth-first search for a construct whose span starts at `span_start`.
fn find_construct_mut(items: &mut [TopLevelItem], span_start: usize) -> Option<&mut Construct> {
    for item in items.iter_mut() {
        if let TopLevelItem::Construct(c) = item {
            if let Some(found) = find_in_construct(c, span_start) {
                return Some(found);
            }
        }
    }
    None
}

/// Remove a node by AST span start from the solution item tree.
fn delete_by_span(items: &mut Vec<TopLevelItem>, span_start: usize) -> Result<(), EditError> {
    // Top-level construct / free function / flow.
    if let Some(i) = items.iter().position(|item| match item {
        TopLevelItem::Construct(c) if c.span.start == span_start => true,
        TopLevelItem::Function(f) if f.span.start == span_start => true,
        TopLevelItem::Flow(f) if f.span.start == span_start => true,
        _ => false,
    }) {
        if let TopLevelItem::Construct(c) = &items[i] {
            if c.layer_provided {
                return Err(EditError::RefuseDelete {
                    span_start,
                    reason: format!(
                        "'{}' is layer-provided infrastructure and cannot be deleted",
                        c.name
                    ),
                });
            }
        }
        items.remove(i);
        return Ok(());
    }

    // Nested: children, steps, free-fns, method-impls inside constructs.
    for item in items.iter_mut() {
        match item {
            TopLevelItem::Construct(c) => {
                if try_delete_in_construct(c, span_start)? {
                    return Ok(());
                }
            }
            TopLevelItem::Flow(flow) => {
                if try_delete_in_steps(&mut flow.steps, span_start) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }

    Err(EditError::TargetNotFound(span_start))
}

/// Returns Ok(true) if something was removed, Ok(false) if not found here.
fn try_delete_in_construct(c: &mut Construct, span_start: usize) -> Result<bool, EditError> {
    // Direct child construct.
    if let Some(i) = c.children.iter().position(|ch| ch.span.start == span_start) {
        if c.children[i].layer_provided {
            return Err(EditError::RefuseDelete {
                span_start,
                reason: format!(
                    "'{}' is layer-provided infrastructure and cannot be deleted",
                    c.children[i].name
                ),
            });
        }
        c.children.remove(i);
        return Ok(true);
    }
    // Nested free function on this construct.
    if let Some(i) = c.fns.iter().position(|f| f.span.start == span_start) {
        c.fns.remove(i);
        return Ok(true);
    }
    // Method implementation.
    if let Some(i) = c.impls.iter().position(|imp| imp.span.start == span_start) {
        c.impls.remove(i);
        return Ok(true);
    }
    // Flow step (or parallel sub-step / match arm — arms are not full constructs;
    // only named steps are deleted as first-class nodes on the canvas).
    if try_delete_in_steps(&mut c.steps, span_start) {
        return Ok(true);
    }
    // Recurse into children.
    for child in c.children.iter_mut() {
        if try_delete_in_construct(child, span_start)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn try_delete_in_steps(steps: &mut Vec<FlowStep>, span_start: usize) -> bool {
    if let Some(i) = steps.iter().position(|s| match s {
        FlowStep::Step(sd) if sd.span.start == span_start => true,
        FlowStep::Parallel(p) if p.span.start == span_start => true,
        FlowStep::Match(m) if m.span.start == span_start => true,
        _ => false,
    }) {
        steps.remove(i);
        return true;
    }
    // Parallel sub-steps.
    for step in steps.iter_mut() {
        if let FlowStep::Parallel(par) = step {
            if let Some(j) = par.steps.iter().position(|s| s.span.start == span_start) {
                par.steps.remove(j);
                return true;
            }
        }
    }
    false
}

fn find_in_construct(c: &mut Construct, span_start: usize) -> Option<&mut Construct> {
    if c.span.start == span_start {
        return Some(c);
    }
    for child in c.children.iter_mut() {
        if let Some(found) = find_in_construct(child, span_start) {
            return Some(found);
        }
    }
    None
}

/// Locate a mutable expression body by AST span start.
///
/// Matches (in order of search): flow steps, method implementations, nested
/// free-fns on constructs, top-level free functions, and — as a fallback for
/// single-body fn-shaped constructs — the first step or first nested fn body
/// when the construct's own span matches.
fn find_body_mut(items: &mut [TopLevelItem], span_start: usize) -> Option<&mut Vec<Expr>> {
    for item in items.iter_mut() {
        match item {
            TopLevelItem::Construct(c) => {
                if let Some(body) = find_body_in_construct(c, span_start) {
                    return Some(body);
                }
            }
            TopLevelItem::Function(f) if f.span.start == span_start => {
                return Some(&mut f.body);
            }
            TopLevelItem::Flow(flow) => {
                if let Some(body) = find_body_in_steps(&mut flow.steps, span_start) {
                    return Some(body);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_body_in_construct(c: &mut Construct, span_start: usize) -> Option<&mut Vec<Expr>> {
    // Resolve the path with shared refs first, then take a single &mut.
    if let Some(i) = c.fns.iter().position(|f| f.span.start == span_start) {
        return Some(&mut c.fns[i].body);
    }
    if let Some(i) = c.impls.iter().position(|imp| imp.span.start == span_start) {
        return Some(&mut c.impls[i].body);
    }
    if let Some(path) = locate_step_body(&c.steps, span_start) {
        return body_from_step_path(&mut c.steps, path);
    }
    if let Some(i) = c
        .children
        .iter()
        .position(|ch| body_exists_in_construct(ch, span_start))
    {
        return find_body_in_construct(&mut c.children[i], span_start);
    }
    // Fallback: body edit on the construct itself (fn-shaped single body).
    if c.span.start == span_start {
        if !c.fns.is_empty() {
            return Some(&mut c.fns[0].body);
        }
        if let Some(i) = c.steps.iter().position(|s| matches!(s, FlowStep::Step(_))) {
            if let FlowStep::Step(s) = &mut c.steps[i] {
                return Some(&mut s.body);
            }
        }
        if !c.impls.is_empty() {
            return Some(&mut c.impls[0].body);
        }
    }
    None
}

/// Read-only probe used to pick a child index before re-borrowing mutably.
fn body_exists_in_construct(c: &Construct, span_start: usize) -> bool {
    if c.fns.iter().any(|f| f.span.start == span_start) {
        return true;
    }
    if c.impls.iter().any(|imp| imp.span.start == span_start) {
        return true;
    }
    if steps_contain_span(&c.steps, span_start) {
        return true;
    }
    if c.children.iter().any(|ch| body_exists_in_construct(ch, span_start)) {
        return true;
    }
    c.span.start == span_start
        && (!c.fns.is_empty()
            || c.steps.iter().any(|s| matches!(s, FlowStep::Step(_)))
            || !c.impls.is_empty())
}

fn steps_contain_span(steps: &[FlowStep], span_start: usize) -> bool {
    for step in steps {
        match step {
            FlowStep::Step(s) if s.span.start == span_start => return true,
            FlowStep::Parallel(par) => {
                if par.steps.iter().any(|s| s.span.start == span_start) {
                    return true;
                }
            }
            FlowStep::Match(m) => {
                if m.arms.iter().any(|a| a.span.start == span_start) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

enum StepBodyPath {
    Step(usize),
    Parallel { step: usize, sub: usize },
    MatchArm { step: usize, arm: usize },
}

fn locate_step_body(steps: &[FlowStep], span_start: usize) -> Option<StepBodyPath> {
    for (i, step) in steps.iter().enumerate() {
        match step {
            FlowStep::Step(s) if s.span.start == span_start => {
                return Some(StepBodyPath::Step(i));
            }
            FlowStep::Parallel(par) => {
                if let Some(j) = par.steps.iter().position(|s| s.span.start == span_start) {
                    return Some(StepBodyPath::Parallel { step: i, sub: j });
                }
            }
            FlowStep::Match(m) => {
                if let Some(j) = m.arms.iter().position(|a| a.span.start == span_start) {
                    return Some(StepBodyPath::MatchArm { step: i, arm: j });
                }
            }
            _ => {}
        }
    }
    None
}

fn body_from_step_path(steps: &mut [FlowStep], path: StepBodyPath) -> Option<&mut Vec<Expr>> {
    match path {
        StepBodyPath::Step(i) => match &mut steps[i] {
            FlowStep::Step(s) => Some(&mut s.body),
            _ => None,
        },
        StepBodyPath::Parallel { step, sub } => match &mut steps[step] {
            FlowStep::Parallel(par) => Some(&mut par.steps[sub].body),
            _ => None,
        },
        StepBodyPath::MatchArm { step, arm } => match &mut steps[step] {
            FlowStep::Match(m) => Some(&mut m.arms[arm].body),
            _ => None,
        },
    }
}

fn find_body_in_steps(steps: &mut [FlowStep], span_start: usize) -> Option<&mut Vec<Expr>> {
    let path = locate_step_body(steps, span_start)?;
    body_from_step_path(steps, path)
}

/// Recursively search for a trait-shaped construct by name and return its methods.
fn find_trait_methods(constructs: &[Construct], name: &str) -> Option<Vec<Method>> {
    for c in constructs {
        if c.name == name && c.shape == Shape::Trait {
            return Some(c.methods.clone());
        }
        if let Some(methods) = find_trait_methods(&c.children, name) {
            return Some(methods);
        }
    }
    None
}

/// Search the entire solution for a trait-shaped construct by name.
fn find_trait_methods_in_solution(items: &[TopLevelItem], name: &str) -> Vec<Method> {
    for item in items {
        if let TopLevelItem::Construct(c) = item {
            if let Some(methods) = find_trait_methods(std::slice::from_ref(c), name) {
                return methods;
            }
        }
    }
    Vec::new()
}

/// Parse a VEIL display type string into a `TypeExpr`, mirroring the display
/// forms produced by the builder/serializer (`Res!<T>`, `Opt<T>`, `List<T>`,
/// `Map<K, V>`, `Set<T>`, `(A, B)`). Unknown/plain names become `Named`.
pub fn parse_type_str(s: &str) -> TypeExpr {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("Res!<").and_then(|x| x.strip_suffix('>')) {
        return TypeExpr::Result(Some(Box::new(parse_type_str(inner))));
    }
    if s == "Res!" {
        return TypeExpr::Result(None);
    }
    if let Some(inner) = s.strip_prefix("Opt<").and_then(|x| x.strip_suffix('>')) {
        return TypeExpr::Optional(Box::new(parse_type_str(inner)));
    }
    if let Some(inner) = s.strip_prefix("List<").and_then(|x| x.strip_suffix('>')) {
        return TypeExpr::List(Box::new(parse_type_str(inner)));
    }
    if let Some(inner) = s.strip_prefix("Set<").and_then(|x| x.strip_suffix('>')) {
        return TypeExpr::Set(Box::new(parse_type_str(inner)));
    }
    if let Some(inner) = s.strip_prefix("Map<").and_then(|x| x.strip_suffix('>')) {
        if let Some((k, v)) = split_top_level_comma(inner) {
            return TypeExpr::Map(Box::new(parse_type_str(&k)), Box::new(parse_type_str(&v)));
        }
    }
    // Generic form Name<A, B>
    if let Some(open) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..open].to_string();
            let args_str = &s[open + 1..s.len() - 1];
            let args = split_all_top_level(args_str)
                .into_iter()
                .map(|a| parse_type_str(&a))
                .collect();
            return TypeExpr::Generic(name, args);
        }
    }
    TypeExpr::Named(s.to_string())
}

/// Split a string on the first top-level comma (not nested inside `<>`).
fn split_top_level_comma(s: &str) -> Option<(String, String)> {
    let mut depth = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => {
                return Some((s[..i].trim().to_string(), s[i + 1..].trim().to_string()));
            }
            _ => {}
        }
    }
    None
}

/// Split a string on all top-level commas.
fn split_all_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

/// Parse an annotation string (with or without leading `@`) into an `Annotation`.
/// e.g. "@invariant(status == Pending)" → name "invariant", args ["status == Pending"].
fn parse_annotation_str(s: &str) -> Annotation {
    let s = s.trim().trim_start_matches('@');
    if let Some(open) = s.find('(') {
        if s.ends_with(')') {
            let name = s[..open].to_string();
            let args_str = &s[open + 1..s.len() - 1];
            let args = if args_str.trim().is_empty() {
                Vec::new()
            } else {
                split_all_top_level(args_str)
            };
            return Annotation { name, args, span: Span::new(0, 0) };
        }
    }
    Annotation { name: s.to_string(), args: Vec::new(), span: Span::new(0, 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_type_str_handles_display_forms() {
        assert!(matches!(parse_type_str("Str"), TypeExpr::Named(n) if n == "Str"));
        assert!(matches!(parse_type_str("Res!"), TypeExpr::Result(None)));
        match parse_type_str("Res!<Customer>") {
            TypeExpr::Result(Some(inner)) => assert!(matches!(*inner, TypeExpr::Named(n) if n == "Customer")),
            other => panic!("expected Result, got {:?}", other),
        }
        match parse_type_str("Opt<Str>") {
            TypeExpr::Optional(inner) => assert!(matches!(*inner, TypeExpr::Named(n) if n == "Str")),
            other => panic!("expected Optional, got {:?}", other),
        }
        match parse_type_str("List<Customer>") {
            TypeExpr::List(inner) => assert!(matches!(*inner, TypeExpr::Named(n) if n == "Customer")),
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn parse_type_str_handles_nested_map() {
        match parse_type_str("Map<Str, List<Customer>>") {
            TypeExpr::Map(k, v) => {
                assert!(matches!(*k, TypeExpr::Named(n) if n == "Str"));
                assert!(matches!(*v, TypeExpr::List(_)));
            }
            other => panic!("expected Map, got {:?}", other),
        }
    }

    #[test]
    fn parse_annotation_str_extracts_args() {
        let a = parse_annotation_str("@invariant(status == Pending)");
        assert_eq!(a.name, "invariant");
        assert_eq!(a.args, vec!["status == Pending".to_string()]);
        let b = parse_annotation_str("env");
        assert_eq!(b.name, "env");
        assert!(b.args.is_empty());
    }

    #[test]
    fn edit_unknown_span_errors() {
        let mut sol = Solution {
            name: "T".to_string(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            items: Vec::new(),
            expose: None,
        };
        let err = apply_edit(&mut sol, &EditOp::Rename { span_start: 42, name: "X".to_string() });
        assert!(matches!(err, Err(EditError::TargetNotFound(42))));
    }

    fn sample_svc_with_step() -> Solution {
        let step = StepDef {
            name: "go".into(),
            span: Span::new(100, 200),
            body: vec![Expr::Ident("old".into())],
            refs: Vec::new(),
            sub_blocks: Vec::new(),
        };
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "Do".into(), Span::new(10, 300));
        svc.steps.push(FlowStep::Step(step));
        Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            items: vec![TopLevelItem::Construct(svc)],
            expose: None,
        }
    }

    #[test]
    fn set_body_replaces_step_with_parsed_exprs() {
        let mut sol = sample_svc_with_step();
        apply_edits_with(
            &mut sol,
            &[EditOp::SetBody {
                span_start: 100,
                body: vec!["x = 1".into(), "Repo.save(x)".into()],
            }],
            |src| {
                // Minimal stand-in: Ident for bare names; Assign for `a = b`.
                if let Some((lhs, rhs)) = src.split_once('=') {
                    Ok(Expr::Assign(
                        lhs.trim().into(),
                        Box::new(Expr::Ident(rhs.trim().into())),
                        None,
                    ))
                } else {
                    Ok(Expr::Ident(src.to_string()))
                }
            },
        )
        .expect("set_body");
        let TopLevelItem::Construct(svc) = &sol.items[0] else {
            panic!("expected construct");
        };
        let FlowStep::Step(step) = &svc.steps[0] else {
            panic!("expected step");
        };
        assert_eq!(step.body.len(), 2);
        assert!(matches!(&step.body[0], Expr::Assign(n, _, None) if n == "x"));
        assert!(matches!(&step.body[1], Expr::Ident(s) if s == "Repo.save(x)"));
        // Must not store opaque full-line Idents for the assign line.
        assert!(!matches!(&step.body[0], Expr::Ident(_)));
    }

    #[test]
    fn set_body_invalid_line_does_not_mutate() {
        let mut sol = sample_svc_with_step();
        let before = format!("{:?}", sol);
        let err = apply_edits_with(
            &mut sol,
            &[EditOp::SetBody {
                span_start: 100,
                body: vec!["x = 1".into(), "!!! bad".into()],
            }],
            |src| {
                if src.contains('!') {
                    Err("boom".into())
                } else {
                    Ok(Expr::Ident(src.into()))
                }
            },
        );
        assert!(matches!(
            err,
            Err(EditError::InvalidBody {
                span_start: 100,
                line: 2,
                ..
            })
        ));
        // On parse failure of a later line, earlier successful parses are not
        // committed — we build the vec then assign. Verify body unchanged.
        let TopLevelItem::Construct(svc) = &sol.items[0] else {
            panic!();
        };
        let FlowStep::Step(step) = &svc.steps[0] else {
            panic!();
        };
        assert!(
            matches!(&step.body[..], [Expr::Ident(s)] if s == "old"),
            "body must be unchanged on InvalidBody: {:?}",
            step.body
        );
        let _ = before;
    }

    #[test]
    fn set_body_without_parser_errors() {
        let mut sol = sample_svc_with_step();
        let err = apply_edits(
            &mut sol,
            &[EditOp::SetBody {
                span_start: 100,
                body: vec!["x = 1".into()],
            }],
        );
        assert!(matches!(err, Err(EditError::InvalidBody { .. })));
    }

    #[test]
    fn delete_construct_removes_nested_child() {
        let mut parent =
            Construct::new("ctx", "Context", Shape::Mod, "Identity".into(), Span::new(10, 500));
        let child =
            Construct::new("val", "ValueObject", Shape::Struct, "Email".into(), Span::new(50, 100));
        parent.children.push(child);
        let mut sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            items: vec![TopLevelItem::Construct(parent)],
            expose: None,
        };
        apply_edit(
            &mut sol,
            &EditOp::DeleteConstruct { span_start: 50 },
        )
        .expect("delete");
        let TopLevelItem::Construct(ctx) = &sol.items[0] else {
            panic!();
        };
        assert!(ctx.children.is_empty(), "child should be gone");
    }

    #[test]
    fn delete_step_by_span() {
        let mut sol = sample_svc_with_step();
        apply_edit(
            &mut sol,
            &EditOp::DeleteConstruct { span_start: 100 },
        )
        .expect("delete step");
        let TopLevelItem::Construct(svc) = &sol.items[0] else {
            panic!();
        };
        assert!(svc.steps.is_empty());
    }

    #[test]
    fn delete_layer_provided_refused() {
        let mut bus =
            Construct::new("trait", "Trait", Shape::Trait, "Bus".into(), Span::new(20, 40));
        bus.layer_provided = true;
        let mut sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            items: vec![TopLevelItem::Construct(bus)],
            expose: None,
        };
        let err = apply_edit(
            &mut sol,
            &EditOp::DeleteConstruct { span_start: 20 },
        );
        assert!(matches!(err, Err(EditError::RefuseDelete { .. })), "{:?}", err);
        assert_eq!(sol.items.len(), 1, "must not remove on refuse");
    }

    #[test]
    fn delete_unknown_span_errors() {
        let mut sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            items: Vec::new(),
            expose: None,
        };
        let err = apply_edit(&mut sol, &EditOp::DeleteConstruct { span_start: 99 });
        assert!(matches!(err, Err(EditError::TargetNotFound(99))));
    }
}
