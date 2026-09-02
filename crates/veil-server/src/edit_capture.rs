//! Edit capture (Spec A) — synthesize durable [`crate::review::EditRecord`]s
//! from a whole-file write.
//!
//! The agent's primary editing tool is `write_source` (whole-file `content`),
//! which bypasses the structured [`veil_ir::EditOp`] machinery the viewer uses.
//! To give whole-file writes the same review payload as structured edits — the
//! topology delta, intent, category, **inferred criticality**, and VEIL body
//! before/after that the delta-on-map review UX consumes — we diff the previous
//! file against the new one (`veil_ir::structural_diff`) and turn each
//! per-construct [`veil_ir::DiffItem`] into a [`crate::review::EditRecordSpec`].
//!
//! The same `EditRecord` shape is produced by the true-`EditOp` path in
//! `POST /api/edit` (see `api::post_edit`), so both paths unify on one model.

use std::collections::HashMap;

use veil_ir::{
    infer_criticality, structural_diff, Criticality, DiffItem, EditAnnotation, EditCategory,
    EditOp, IrGraph, LayerRegistry, PathSegment,
};

use crate::review::EditRecordSpec;

/// Parse + build an IR graph for one file's source, or `None` on parse failure.
fn build_graph(source: &str, registry: &LayerRegistry) -> Option<IrGraph> {
    let tokens = veil_parser::lex(source);
    let sol = veil_parser::parse_with_registry(&tokens, registry.clone()).ok()?;
    Some(veil_ir::build_ir_with_registry(&sol, Some(registry)))
}

/// Join a projection-aware container path into `A/B` form.
fn join_container(path: &[PathSegment]) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    Some(
        path.iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Fold layer / stock infrastructure lines out of a VEIL body preview so the
/// review UX shows domain logic, never `stock.` wiring or generated noise.
fn clean_body(lines: &[String]) -> Option<String> {
    let kept: Vec<&String> = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("stock.") && !t.contains("::stock::")
        })
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(
        kept.iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// A representative `EditOp` for a diff kind so `infer_criticality` can run even
/// on the whole-file path (which carries no real EditOps). `span_start` is not
/// meaningful here — only the op *variant* drives inference.
fn representative_op(item: &DiffItem) -> EditOp {
    match item {
        DiffItem::Added { name, .. } => EditOp::CreateConstruct {
            parent_span: 0,
            keyword: String::new(),
            name: name.clone(),
            target: None,
        },
        DiffItem::Removed { .. } => EditOp::DeleteConstruct { span_start: 0 },
        DiffItem::Renamed { to_name, .. } => EditOp::Rename {
            span_start: 0,
            name: to_name.clone(),
        },
        DiffItem::SignatureChanged { .. } => EditOp::SetFields {
            span_start: 0,
            fields: Vec::new(),
        },
        DiffItem::BodyChanged { .. } => EditOp::SetBody {
            span_start: 0,
            body: Vec::new(),
        },
        DiffItem::AnnotationsChanged { .. } => EditOp::SetAnnotations {
            span_start: 0,
            annotations: Vec::new(),
        },
    }
}

/// Default review category for a diff kind (used when the agent gave none).
fn category_for(item: &DiffItem) -> EditCategory {
    match item {
        DiffItem::Added { .. }
        | DiffItem::Removed { .. }
        | DiffItem::Renamed { .. }
        | DiffItem::SignatureChanged { .. } => EditCategory::Structure,
        DiffItem::BodyChanged { .. } => EditCategory::Behavior,
        DiffItem::AnnotationsChanged { .. } => EditCategory::Cosmetic,
    }
}

fn item_name(item: &DiffItem) -> &str {
    match item {
        DiffItem::Added { name, .. }
        | DiffItem::Removed { name, .. }
        | DiffItem::SignatureChanged { name, .. }
        | DiffItem::BodyChanged { name, .. }
        | DiffItem::AnnotationsChanged { name, .. } => name.as_str(),
        DiffItem::Renamed { to_name, .. } => to_name.as_str(),
    }
}

fn item_node_kind(item: &DiffItem) -> &str {
    match item {
        DiffItem::Added { node_kind, .. }
        | DiffItem::Removed { node_kind, .. }
        | DiffItem::Renamed { node_kind, .. }
        | DiffItem::SignatureChanged { node_kind, .. }
        | DiffItem::BodyChanged { node_kind, .. }
        | DiffItem::AnnotationsChanged { node_kind, .. } => node_kind.as_str(),
    }
}

fn item_container(item: &DiffItem) -> Option<String> {
    let segs = match item {
        DiffItem::Added { container_path, .. }
        | DiffItem::Removed { container_path, .. }
        | DiffItem::Renamed { container_path, .. }
        | DiffItem::SignatureChanged { container_path, .. }
        | DiffItem::BodyChanged { container_path, .. }
        | DiffItem::AnnotationsChanged { container_path, .. } => container_path,
    };
    join_container(segs)
}

/// Case-insensitive intent lookup by construct name (from `write_source`
/// rationales). A `*` key is a package-level fallback applied to all records.
fn intent_for(name: &str, rats: &HashMap<String, String>) -> Option<String> {
    rats.get(name)
        .or_else(|| {
            rats.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v)
        })
        .or_else(|| rats.get("*"))
        .cloned()
}

/// Synthesize edit-capture specs from a whole-file change (`prev` → `content`).
///
/// Returns an empty vec when the new content fails to parse (the write's own
/// parse-gate handles reporting) or when there is no structural delta.
/// Count the DISTINCT top-level constructs changed between `prev` and `content`
/// (Added / Removed / Renamed / BodyChanged / …). Used by the write_source
/// rationale-enforcement guard: a multi-construct structural change must carry
/// rationales so the review can show intent per construct. Returns 0 when the
/// new content does not parse (the parse guard handles that separately).
pub fn changed_construct_count(prev: &str, content: &str, registry: &LayerRegistry) -> usize {
    let Some(head) = build_graph(content, registry) else {
        return 0;
    };
    let base = build_graph(prev, registry).unwrap_or_default();
    let diff = structural_diff(&base, &head, "before", "after");
    // Count DISTINCT TOP-LEVEL constructs only (empty container path). A single
    // new flow-with-steps is one top-level construct, so it is NOT treated as a
    // multi-construct change — only genuinely separate top-level constructs are.
    let mut top: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in &diff.items {
        let name = item_name(item);
        if name.is_empty() {
            continue;
        }
        let container = item_container(item).unwrap_or_default();
        if container.trim().is_empty() {
            top.insert(name.to_string());
        }
    }
    top.len()
}

pub fn synthesize_from_whole_file(
    prev: &str,
    content: &str,
    registry: &LayerRegistry,
    rats: &HashMap<String, String>,
    path: Option<&str>,
) -> Vec<EditRecordSpec> {
    let Some(head) = build_graph(content, registry) else {
        return Vec::new();
    };
    // A brand-new file (no prev) diffs against an empty graph → all Added.
    let base = build_graph(prev, registry).unwrap_or_default();

    let diff = structural_diff(&base, &head, "before", "after");
    synthesize_from_items(&diff.items, rats, path)
}

/// Synthesize edit-capture specs from already-computed `DiffItem`s. Shared by
/// the whole-file path and directly unit-testable without VEIL parsing.
pub fn synthesize_from_items(
    items: &[DiffItem],
    rats: &HashMap<String, String>,
    path: Option<&str>,
) -> Vec<EditRecordSpec> {
    let mut specs = Vec::with_capacity(items.len());
    for item in items {
        let name = item_name(item).to_string();
        if name.is_empty() {
            continue;
        }
        let op = representative_op(item);
        let criticality = infer_criticality(&op, &[]);

        // Bodies only for High/Critical body changes; VEIL only, stock folded.
        let (body_before, body_after) = match item {
            DiffItem::BodyChanged {
                before_preview,
                after_preview,
                ..
            } if criticality >= Criticality::High => {
                (clean_body(before_preview), clean_body(after_preview))
            }
            _ => (None, None),
        };

        let annotation = EditAnnotation {
            intent: intent_for(&name, rats),
            category: Some(category_for(item)),
            criticality: Some(criticality),
        };

        specs.push(EditRecordSpec {
            path: path.map(str::to_string),
            container_path: item_container(item),
            construct_name: name,
            construct_kind: item_node_kind(item).to_string(),
            // Whole-file path carries no real EditOps; the structural_delta
            // holds the shape. The true-EditOp path fills edit_ops instead.
            edit_ops: Vec::new(),
            annotation,
            criticality,
            structural_delta: vec![item.clone()],
            body_before,
            body_after,
        });
    }
    specs
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::PathSegment;

    fn reg() -> LayerRegistry {
        LayerRegistry::default()
    }

    #[test]
    fn added_construct_is_captured() {
        // `flow` is a core construct recognized without layer packages.
        let prev = "pkg Demo\n";
        let content = "pkg Demo\n\n  flow Checkout\n    step Validate\n      guard ok\n";
        let specs =
            synthesize_from_whole_file(prev, content, &reg(), &HashMap::new(), Some("main.veil"));
        assert!(
            specs.iter().any(|s| s.construct_name == "Checkout"
                || s.construct_name == "Validate"),
            "expected Checkout/Validate add, got {:?}",
            specs.iter().map(|s| &s.construct_name).collect::<Vec<_>>()
        );
        assert!(specs.iter().all(|s| s.path.as_deref() == Some("main.veil")));
        assert!(specs.iter().all(|s| !s.structural_delta.is_empty()));
    }

    #[test]
    fn no_change_yields_no_specs() {
        let src = "pkg Demo\n\n  rec A\n    x: Int\n";
        let specs = synthesize_from_whole_file(src, src, &reg(), &HashMap::new(), None);
        assert!(specs.is_empty(), "identical source must produce no edits: {specs:?}");
    }

    /// Rationale-enforcement guard (Part E): a multi-construct structural change
    /// reports >1 changed construct; a single add reports 1; no change reports 0.
    #[test]
    fn changed_construct_count_detects_multi() {
        let prev = "pkg Demo\n";
        let one = "pkg Demo\n\n  flow Checkout\n    step Validate\n      guard ok\n";
        let two = "pkg Demo\n\n  flow Checkout\n    step Validate\n      guard ok\n\n  flow Refund\n    step Issue\n      guard ok\n";
        assert_eq!(changed_construct_count(prev, prev, &reg()), 0);
        let one_n = changed_construct_count(prev, one, &reg());
        let two_n = changed_construct_count(prev, two, &reg());
        assert!(one_n >= 1, "one added flow → at least one changed construct");
        assert!(
            two_n > one_n,
            "a second flow must count more changed constructs than one ({two_n} vs {one_n})"
        );
    }

    // ── DiffItem-level synthesis (no VEIL parsing) ───────────────────────

    #[test]
    fn body_change_is_high_with_previews() {
        // A BodyChanged item → representative SetBody op → infer_criticality
        // returns High → body_before/after populated, stock lines folded out.
        let items = vec![DiffItem::BodyChanged {
            path: "Checkout".into(),
            node_kind: "Step".into(),
            name: "Validate".into(),
            before_lines: 1,
            after_lines: 2,
            before_preview: vec!["guard ok".into()],
            after_preview: vec!["stock.persist(x)".into(), "emit Validated{id}".into()],
            container_path: vec![PathSegment {
                name: "Checkout".into(),
                subkind: Some("Flow".into()),
            }],
        }];
        let mut rats = HashMap::new();
        rats.insert("Validate".to_string(), "tighten the guard".to_string());
        let specs = synthesize_from_items(&items, &rats, Some("main.veil"));
        assert_eq!(specs.len(), 1);
        let s = &specs[0];
        assert_eq!(s.construct_name, "Validate");
        assert_eq!(s.criticality, Criticality::High, "SetBody infers High");
        assert_eq!(s.body_before.as_deref(), Some("guard ok"));
        // stock.persist folded out; only the domain emit line remains.
        assert_eq!(s.body_after.as_deref(), Some("emit Validated{id}"));
        assert_eq!(s.annotation.intent.as_deref(), Some("tighten the guard"));
        assert_eq!(s.container_path.as_deref(), Some("Checkout"));
        assert!(!s.structural_delta.is_empty());
    }

    #[test]
    fn wildcard_intent_falls_back() {
        let items = vec![DiffItem::Added {
            path: String::new(),
            node_kind: "TypeDef".into(),
            name: "Widget".into(),
            subkind: None,
            container_path: vec![],
        }];
        let mut rats = HashMap::new();
        rats.insert("*".to_string(), "package-level why".to_string());
        let specs = synthesize_from_items(&items, &rats, None);
        assert_eq!(
            specs[0].annotation.intent.as_deref(),
            Some("package-level why")
        );
    }
}
