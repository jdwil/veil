//! Structured edits applied to a parsed `Solution` AST.
//!
//! The viewer never writes raw text. Instead it sends a structured `EditOp`
//! keyed by the IR node's span (the IR builder stamps each construct node with
//! its AST span, so the span uniquely identifies the target construct). The
//! server applies the edit to the AST, re-serializes, and writes back — the
//! serializer is idempotent, so a load→edit→save cycle is stable.
//!
//! This module is generic: it edits by core shape and never encodes domain
//! vocabulary. Field/method types are stored as raw strings and parsed with the
//! same `TypeExpr` grammar the parser uses (via `parse_type_str`).

use serde::{Deserialize, Serialize};

use crate::ast::*;
use crate::span::Span;

/// A single structured edit targeting the construct whose AST span starts at
/// `span_start`. Using the span start (rather than an IR node id) keeps the
/// edit stable across IR rebuilds.
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
    /// No construct found at the given span start.
    TargetNotFound(usize),
    /// The target construct's shape does not support this edit.
    ShapeMismatch { span_start: usize, expected: &'static str },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::TargetNotFound(s) => write!(f, "no construct found at span {}", s),
            EditError::ShapeMismatch { span_start, expected } => {
                write!(f, "construct at span {} is not {}", span_start, expected)
            }
        }
    }
}

impl std::error::Error for EditError {}

/// Apply a batch of edits to the solution, in order.
pub fn apply_edits(sol: &mut Solution, ops: &[EditOp]) -> Result<(), EditError> {
    for op in ops {
        apply_edit(sol, op)?;
    }
    Ok(())
}

/// Apply a single edit to the solution.
pub fn apply_edit(sol: &mut Solution, op: &EditOp) -> Result<(), EditError> {
    let span_start = op.span_start();
    let target = find_construct_mut(&mut sol.items, span_start)
        .ok_or(EditError::TargetNotFound(span_start))?;
    match op {
        EditOp::Rename { name, .. } => {
            target.name = name.clone();
            Ok(())
        }
        EditOp::SetAnnotations { annotations, .. } => {
            target.annotations = annotations.iter().map(|a| parse_annotation_str(a)).collect();
            Ok(())
        }
        EditOp::SetFields { fields, span_start } => {
            if target.shape != Shape::Struct {
                return Err(EditError::ShapeMismatch { span_start: *span_start, expected: "a struct-shaped construct" });
            }
            target.fields = fields.iter().map(|f| Field {
                name: f.name.clone(),
                type_expr: parse_type_str(&f.type_str),
                span: Span::new(0, 0),
            }).collect();
            Ok(())
        }
        EditOp::SetMethods { methods, span_start } => {
            if target.shape != Shape::Trait {
                return Err(EditError::ShapeMismatch { span_start: *span_start, expected: "a trait-shaped construct" });
            }
            target.methods = methods.iter().map(|m| Method {
                name: m.name.clone(),
                params: m.params.iter().map(|p| Param {
                    name: p.name.clone(),
                    type_expr: parse_type_str(&p.type_str),
                    span: Span::new(0, 0),
                }).collect(),
                return_type: if m.return_type.is_empty() { None } else { Some(parse_type_str(&m.return_type)) },
                span: Span::new(0, 0),
            }).collect();
            Ok(())
        }
    }
}

impl EditOp {
    pub fn span_start(&self) -> usize {
        match self {
            EditOp::Rename { span_start, .. }
            | EditOp::SetAnnotations { span_start, .. }
            | EditOp::SetFields { span_start, .. }
            | EditOp::SetMethods { span_start, .. } => *span_start,
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
        };
        let err = apply_edit(&mut sol, &EditOp::Rename { span_start: 42, name: "X".to_string() });
        assert!(matches!(err, Err(EditError::TargetNotFound(42))));
    }
}
