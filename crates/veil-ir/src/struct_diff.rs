//! Structural / semantic IR diff (UX-021).
//!
//! Compares two IR graphs by stable keys (parent path + kind + name) rather
//! than node ids (which are rebuild-unstable).

use serde::{Deserialize, Serialize};

use crate::ir::{IrGraph, IrNode, NodeKind};

/// One segment of a projection-aware container path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathSegment {
    /// Construct name (e.g. "Customer")
    pub name: String,
    /// Layer subkind if available (e.g. "Aggregate", "Context")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subkind: Option<String>,
}

/// One structural change between two IR snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffItem {
    Added {
        path: String,
        node_kind: String,
        name: String,
        subkind: Option<String>,
        /// Projection-aware container path (subkind + name segments).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        container_path: Vec<PathSegment>,
    },
    Removed {
        path: String,
        node_kind: String,
        name: String,
        subkind: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        container_path: Vec<PathSegment>,
    },
    Renamed {
        path: String,
        node_kind: String,
        from_name: String,
        to_name: String,
        subkind: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        container_path: Vec<PathSegment>,
    },
    SignatureChanged {
        path: String,
        node_kind: String,
        name: String,
        before: String,
        after: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        container_path: Vec<PathSegment>,
    },
    BodyChanged {
        path: String,
        node_kind: String,
        name: String,
        before_lines: usize,
        after_lines: usize,
        before_preview: Vec<String>,
        after_preview: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        container_path: Vec<PathSegment>,
    },
    AnnotationsChanged {
        path: String,
        node_kind: String,
        name: String,
        before: Vec<String>,
        after: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        container_path: Vec<PathSegment>,
    },
}

/// Snapshot of a construct for PR Wizard / review UI (fields, signature, body).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConstructPeek {
    /// `base` or `head` — which IR this snapshot came from.
    pub side: String,
    pub name: String,
    pub node_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subkind: Option<String>,
    /// path/parent path when known
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Field lines like `qty: Int`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_preview: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<String>,
    /// Agent intent / rationale when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StructDiff {
    pub base_label: String,
    pub head_label: String,
    pub items: Vec<DiffItem>,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    /// Per-item edit annotations (same length as `items` when populated).
    /// Populated by the server from the transient annotation cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_annotations: Option<Vec<Option<crate::edit::EditAnnotation>>>,
    /// Per-item construct peeks (same length as `items` when populated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_peeks: Option<Vec<Option<ConstructPeek>>>,
    /// Optional paired base peeks for modified items (same length as items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_peeks_base: Option<Vec<Option<ConstructPeek>>>,
}

fn kind_str(k: &NodeKind) -> String {
    format!("{:?}", k)
}

fn parent_path(graph: &IrGraph, node: &IrNode) -> String {
    let by_id: std::collections::HashMap<_, _> =
        graph.nodes.iter().map(|n| (n.id, n)).collect();
    let mut parts = Vec::new();
    let mut walk = node.metadata.parent;
    let mut guard = 0;
    while let Some(pid) = walk {
        if guard > 64 {
            break;
        }
        guard += 1;
        if let Some(p) = by_id.get(&pid) {
            if p.kind != NodeKind::Solution {
                parts.push(p.name.clone());
            }
            walk = p.metadata.parent;
        } else {
            break;
        }
    }
    parts.reverse();
    parts.join("/")
}

/// Build a projection-aware container path with subkind labels.
/// Returns segments like [PathSegment{name:"Identity", subkind:"Context"}, ...]
fn container_segments(graph: &IrGraph, node: &IrNode) -> Vec<PathSegment> {
    let by_id: std::collections::HashMap<_, _> =
        graph.nodes.iter().map(|n| (n.id, n)).collect();
    let mut segments = Vec::new();
    let mut walk = node.metadata.parent;
    let mut guard = 0;
    while let Some(pid) = walk {
        if guard > 64 {
            break;
        }
        guard += 1;
        if let Some(p) = by_id.get(&pid) {
            if p.kind != NodeKind::Solution {
                segments.push(PathSegment {
                    name: p.name.clone(),
                    subkind: p.metadata.subkind.clone(),
                });
            }
            walk = p.metadata.parent;
        } else {
            break;
        }
    }
    segments.reverse();
    segments
}

/// Key for matching constructs across rebuilds (exclude Action noise optionally).
fn stable_key(graph: &IrGraph, node: &IrNode) -> String {
    let path = parent_path(graph, node);
    let sk = node.metadata.subkind.as_deref().unwrap_or("");
    format!(
        "{}|{:?}|{}|{}",
        path,
        node.kind,
        sk,
        node.name
    )
}

/// Structural fingerprint for change detection (all shape-relevant props).
/// Not for UI — use [`display_signature`] in peeks / before·after text.
fn signature_of(node: &IrNode) -> String {
    let mut parts = Vec::new();
    if let Some(sk) = &node.metadata.subkind {
        parts.push(format!("subkind={}", sk));
    }
    for (k, v) in &node.metadata.properties {
        if matches!(
            k.as_str(),
            "signature"
                | "params"
                | "returns"
                | "methods"
                | "fields"
                | "implements"
                | "variants"
                | "transitions"
        ) || k.starts_with("fn:")
        {
            parts.push(format!("{}={}", k, v));
        }
    }
    parts.sort();
    parts.join("; ")
}

fn prop<'a>(node: &'a IrNode, key: &str) -> Option<&'a str> {
    node.metadata
        .properties
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Human-facing method/fn signature for PR Wizard peeks.
/// Prefer the IR `signature` property only — params/returns are redundant with it.
fn display_signature(node: &IrNode) -> Option<String> {
    if let Some(v) = prop(node, "signature") {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    // Fallback when only params/returns were stored (older IR)
    let params = prop(node, "params").map(str::trim).filter(|s| !s.is_empty());
    let returns = prop(node, "returns")
        .map(str::trim)
        .map(|s| s.trim_start_matches("->").trim())
        .filter(|s| !s.is_empty());
    match (params, returns) {
        (Some(p), Some(r)) => {
            let pnorm = if p.starts_with('(') {
                p.to_string()
            } else {
                format!("({p})")
            };
            Some(format!("{pnorm} -> {r}"))
        }
        (Some(p), None) => Some(p.to_string()),
        (None, Some(r)) => Some(format!("() -> {r}")),
        (None, None) => None,
    }
}

fn body_preview(graph: &IrGraph, parent: &IrNode) -> Vec<String> {
    let mut lines: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| n.metadata.parent == Some(parent.id) && n.kind == NodeKind::Action)
        .map(|n| n.name.clone())
        .collect();
    // Nested sub-blocks (compensate, etc.)
    for child in graph
        .nodes
        .iter()
        .filter(|n| n.metadata.parent == Some(parent.id) && n.kind == NodeKind::Step)
    {
        if child.metadata.annotations.iter().any(|a| a == "sub_block") {
            let nested = body_preview(graph, child);
            lines.push(format!("{}:", child.name));
            lines.extend(nested.into_iter().map(|l| format!("  {}", l)));
        }
    }
    lines
}

fn is_interesting(n: &IrNode) -> bool {
    !matches!(
        n.kind,
        NodeKind::Solution | NodeKind::Action | NodeKind::Inputs | NodeKind::Return | NodeKind::Field
    )
}

/// Diff two IR graphs structurally.
pub fn structural_diff(base: &IrGraph, head: &IrGraph, base_label: &str, head_label: &str) -> StructDiff {
    let base_nodes: Vec<_> = base.nodes.iter().filter(|n| is_interesting(n)).collect();
    let head_nodes: Vec<_> = head.nodes.iter().filter(|n| is_interesting(n)).collect();

    let mut base_map: std::collections::HashMap<String, &IrNode> = std::collections::HashMap::new();
    for n in &base_nodes {
        base_map.insert(stable_key(base, n), n);
    }
    let mut head_map: std::collections::HashMap<String, &IrNode> = std::collections::HashMap::new();
    for n in &head_nodes {
        head_map.insert(stable_key(head, n), n);
    }

    let mut items = Vec::new();

    // Unmatched nodes by (path, kind, subkind) buckets — rename only when
    // exactly one base and one head in the same bucket (unique 1:1).
    let mut removed_keys: Vec<String> = Vec::new();
    let mut added_keys: Vec<String> = Vec::new();
    for (key, _) in &base_map {
        if !head_map.contains_key(key) {
            removed_keys.push(key.clone());
        }
    }
    for (key, _) in &head_map {
        if !base_map.contains_key(key) {
            added_keys.push(key.clone());
        }
    }

    let bucket = |graph: &IrGraph, n: &IrNode| {
        format!(
            "{}|{:?}|{}",
            parent_path(graph, n),
            n.kind,
            n.metadata.subkind.as_deref().unwrap_or("")
        )
    };

    let mut base_buckets: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for k in &removed_keys {
        let n = base_map[k];
        base_buckets
            .entry(bucket(base, n))
            .or_default()
            .push(k.clone());
    }
    let mut head_buckets: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for k in &added_keys {
        let n = head_map[k];
        head_buckets
            .entry(bucket(head, n))
            .or_default()
            .push(k.clone());
    }

    let mut renamed_base = std::collections::HashSet::new();
    let mut renamed_head = std::collections::HashSet::new();
    for (bkt, bkeys) in &base_buckets {
        if bkeys.len() != 1 {
            continue;
        }
        let Some(hkeys) = head_buckets.get(bkt) else {
            continue;
        };
        if hkeys.len() != 1 {
            continue;
        }
        let bk = &bkeys[0];
        let hk = &hkeys[0];
        let bn = base_map[bk];
        let hn = head_map[hk];
        if bn.name == hn.name {
            continue;
        }
        items.push(DiffItem::Renamed {
            path: parent_path(base, bn),
            node_kind: kind_str(&bn.kind),
            from_name: bn.name.clone(),
            to_name: hn.name.clone(),
            subkind: bn.metadata.subkind.clone(),
            container_path: container_segments(base, bn),
        });
        renamed_base.insert(bk.clone());
        renamed_head.insert(hk.clone());
    }

    for k in &removed_keys {
        if renamed_base.contains(k) {
            continue;
        }
        let bn = base_map[k];
        items.push(DiffItem::Removed {
            path: parent_path(base, bn),
            node_kind: kind_str(&bn.kind),
            name: bn.name.clone(),
            subkind: bn.metadata.subkind.clone(),
            container_path: container_segments(base, bn),
        });
    }
    for k in &added_keys {
        if renamed_head.contains(k) {
            continue;
        }
        let hn = head_map[k];
        items.push(DiffItem::Added {
            path: parent_path(head, hn),
            node_kind: kind_str(&hn.kind),
            name: hn.name.clone(),
            subkind: hn.metadata.subkind.clone(),
            container_path: container_segments(head, hn),
        });
    }

    // Matched keys: signature / body / annotations
    for (key, bn) in &base_map {
        let Some(hn) = head_map.get(key) else {
            continue;
        };
        let path = parent_path(head, hn);
        let segments = container_segments(head, hn);
        let bsig = signature_of(bn);
        let hsig = signature_of(hn);
        if bsig != hsig {
            // Compare with full fingerprint; show clean signature text in the UI.
            let before = display_signature(bn).unwrap_or(bsig);
            let after = display_signature(hn).unwrap_or(hsig);
            items.push(DiffItem::SignatureChanged {
                path: path.clone(),
                node_kind: kind_str(&hn.kind),
                name: hn.name.clone(),
                before,
                after,
                container_path: segments.clone(),
            });
        }
        let bann: Vec<_> = bn
            .metadata
            .annotations
            .iter()
            .filter(|a| !a.starts_with("has_") && *a != "sub_block" && *a != "layer-provided")
            .cloned()
            .collect();
        let hann: Vec<_> = hn
            .metadata
            .annotations
            .iter()
            .filter(|a| !a.starts_with("has_") && *a != "sub_block" && *a != "layer-provided")
            .cloned()
            .collect();
        if bann != hann {
            items.push(DiffItem::AnnotationsChanged {
                path: path.clone(),
                node_kind: kind_str(&hn.kind),
                name: hn.name.clone(),
                before: bann,
                after: hann,
                container_path: segments.clone(),
            });
        }
        // Body: steps and methods
        if matches!(
            hn.kind,
            NodeKind::Step | NodeKind::InterfaceMethod | NodeKind::Flow
        ) {
            let bp = body_preview(base, bn);
            let hp = body_preview(head, hn);
            if bp != hp {
                items.push(DiffItem::BodyChanged {
                    path,
                    node_kind: kind_str(&hn.kind),
                    name: hn.name.clone(),
                    before_lines: bp.len(),
                    after_lines: hp.len(),
                    before_preview: bp.into_iter().take(6).collect(),
                    after_preview: hp.into_iter().take(6).collect(),
                    container_path: segments,
                });
            }
        }
    }

    let added = items
        .iter()
        .filter(|i| matches!(i, DiffItem::Added { .. }))
        .count();
    let removed = items
        .iter()
        .filter(|i| matches!(i, DiffItem::Removed { .. }))
        .count();
    let changed = items.len().saturating_sub(added + removed);

    let mut diff = StructDiff {
        base_label: base_label.to_string(),
        head_label: head_label.to_string(),
        items,
        added,
        removed,
        changed,
        item_annotations: None,
        item_peeks: None,
        item_peeks_base: None,
    };
    enrich_diff_peeks(&mut diff, base, head);
    // Stable high→low impact order (HashMap walk is otherwise random).
    sort_diff_for_review(&mut diff);
    diff
}

/// Change-kind impact: destructive / contract-breaking first, cosmetic last.
fn change_kind_rank(item: &DiffItem) -> u8 {
    match item {
        DiffItem::Removed { .. } => 0,
        DiffItem::Renamed { .. } => 1,
        DiffItem::SignatureChanged { .. } => 2,
        DiffItem::BodyChanged { .. } => 3,
        DiffItem::Added { .. } => 4,
        DiffItem::AnnotationsChanged { .. } => 5,
    }
}

fn item_subkind(item: &DiffItem) -> Option<&str> {
    match item {
        DiffItem::Added { subkind, .. }
        | DiffItem::Removed { subkind, .. }
        | DiffItem::Renamed { subkind, .. } => subkind.as_deref(),
        // Signature/body/annotation variants don't carry subkind yet.
        DiffItem::SignatureChanged { .. }
        | DiffItem::BodyChanged { .. }
        | DiffItem::AnnotationsChanged { .. } => None,
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

fn item_path(item: &DiffItem) -> &str {
    match item {
        DiffItem::Added { path, .. }
        | DiffItem::Removed { path, .. }
        | DiffItem::Renamed { path, .. }
        | DiffItem::SignatureChanged { path, .. }
        | DiffItem::BodyChanged { path, .. }
        | DiffItem::AnnotationsChanged { path, .. } => path.as_str(),
    }
}

fn item_container_depth(item: &DiffItem) -> usize {
    match item {
        DiffItem::Added { container_path, .. }
        | DiffItem::Removed { container_path, .. }
        | DiffItem::Renamed { container_path, .. }
        | DiffItem::SignatureChanged { container_path, .. }
        | DiffItem::BodyChanged { container_path, .. }
        | DiffItem::AnnotationsChanged { container_path, .. } => container_path.len(),
    }
}

/// Domain / host constructs before leaf methods/fields.
fn construct_rank(item: &DiffItem) -> u8 {
    let sk = item_subkind(item).unwrap_or("").to_ascii_lowercase();
    let nk = item_node_kind(item).to_ascii_lowercase();
    if matches!(
        sk.as_str(),
        "aggregate" | "entity" | "root" | "context" | "boundedcontext" | "module" | "package"
    ) || nk.contains("solution")
    {
        return 0;
    }
    if matches!(
        sk.as_str(),
        "service" | "interface" | "repo" | "repository" | "port" | "adapter" | "gateway" | "policy"
    ) || nk == "interface"
    {
        return 1;
    }
    if matches!(sk.as_str(), "flow" | "usecase" | "command" | "query" | "handler" | "process")
        || nk == "flow"
        || nk == "step"
    {
        return 2;
    }
    if sk.contains("method")
        || sk == "fn"
        || nk.contains("method")
        || nk == "interfacemethod"
        || nk == "action"
    {
        return 4;
    }
    if sk.contains("field") || nk == "field" {
        return 5;
    }
    3
}

fn annotation_criticality_rank(ann: Option<&crate::edit::EditAnnotation>) -> u8 {
    use crate::edit::Criticality;
    match ann.and_then(|a| a.criticality) {
        Some(Criticality::Critical) => 0,
        Some(Criticality::High) => 1,
        Some(Criticality::Normal) | None => 2,
        Some(Criticality::Low) => 3,
    }
}

/// Sort review items high-impact → low. Keeps peeks / annotations aligned by index.
///
/// Order keys (ascending = earlier in walk):
/// 1. Agent `criticality` when present (Critical → Low)
/// 2. Change kind (removed → renamed → signature → body → added → annotations)
/// 3. Construct role (aggregates/entities → services → flows → methods)
/// 4. Shallower container path first
/// 5. Path + name (stable)
pub fn sort_diff_for_review(diff: &mut StructDiff) {
    let n = diff.items.len();
    if n <= 1 {
        return;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ann_a = diff
            .item_annotations
            .as_ref()
            .and_then(|v| v.get(a))
            .and_then(|x| x.as_ref());
        let ann_b = diff
            .item_annotations
            .as_ref()
            .and_then(|v| v.get(b))
            .and_then(|x| x.as_ref());
        annotation_criticality_rank(ann_a)
            .cmp(&annotation_criticality_rank(ann_b))
            .then_with(|| change_kind_rank(&diff.items[a]).cmp(&change_kind_rank(&diff.items[b])))
            .then_with(|| construct_rank(&diff.items[a]).cmp(&construct_rank(&diff.items[b])))
            .then_with(|| {
                item_container_depth(&diff.items[a]).cmp(&item_container_depth(&diff.items[b]))
            })
            .then_with(|| item_path(&diff.items[a]).cmp(item_path(&diff.items[b])))
            .then_with(|| item_name(&diff.items[a]).cmp(item_name(&diff.items[b])))
    });

    fn permute<T: Clone>(src: Vec<T>, order: &[usize]) -> Vec<T> {
        order.iter().map(|&i| src[i].clone()).collect()
    }
    diff.items = permute(std::mem::take(&mut diff.items), &order);
    if let Some(peeks) = diff.item_peeks.take() {
        diff.item_peeks = Some(permute(peeks, &order));
    }
    if let Some(peeks) = diff.item_peeks_base.take() {
        diff.item_peeks_base = Some(permute(peeks, &order));
    }
    if let Some(anns) = diff.item_annotations.take() {
        diff.item_annotations = Some(permute(anns, &order));
    }
}

/// Build a review peek from an IR node (fields, methods, body actions, annotations).
pub fn construct_peek_from_node(graph: &IrGraph, node: &IrNode, side: &str) -> ConstructPeek {
    let path = parent_path(graph, node);
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    for (k, v) in &node.metadata.properties {
        if k == "fields" || k.starts_with("field:") {
            fields.push(if k.starts_with("field:") {
                format!("{}: {}", k.trim_start_matches("field:"), v)
            } else {
                v.clone()
            });
        } else if k == "methods" || k.starts_with("fn:") || k.starts_with("method:") {
            methods.push(if k.starts_with("fn:") {
                format!("{} {}", k.trim_start_matches("fn:"), v)
            } else {
                v.clone()
            });
        }
    }
    // Child Field nodes
    for child in graph
        .nodes
        .iter()
        .filter(|n| n.metadata.parent == Some(node.id) && n.kind == NodeKind::Field)
    {
        let ty = child
            .metadata
            .properties
            .iter()
            .find(|(k, _)| k == "type" || k == "ty")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let line = if ty.is_empty() {
            child.name.clone()
        } else {
            format!("{}: {}", child.name, ty)
        };
        if !fields.iter().any(|f| f.starts_with(&child.name)) {
            fields.push(line);
        }
    }
    // Child method-like TypeDef / Step under host
    for child in graph.nodes.iter().filter(|n| {
        n.metadata.parent == Some(node.id)
            && matches!(n.kind, NodeKind::Step | NodeKind::TypeDef)
            && n.metadata
                .subkind
                .as_deref()
                .map(|s| {
                    let l = s.to_ascii_lowercase();
                    l.contains("method") || l.contains("handler") || l == "fn"
                })
                .unwrap_or(false)
    }) {
        methods.push(format!(
            "{} {}",
            child.metadata.subkind.as_deref().unwrap_or("fn"),
            child.name
        ));
    }
    let body = body_preview(graph, node);
    let sig = display_signature(node);
    // Cap previews for UI
    fields.truncate(24);
    methods.truncate(16);
    let body_preview: Vec<String> = body.into_iter().take(20).collect();
    ConstructPeek {
        side: side.to_string(),
        name: node.name.clone(),
        node_kind: kind_str(&node.kind),
        subkind: node.metadata.subkind.clone(),
        path: if path.is_empty() { None } else { Some(path) },
        signature: sig,
        fields,
        methods,
        body_preview,
        annotations: node.metadata.annotations.clone(),
        intent: None,
    }
}

fn find_node_by_name<'a>(graph: &'a IrGraph, name: &str) -> Option<&'a IrNode> {
    // Prefer interesting nodes
    graph
        .nodes
        .iter()
        .filter(|n| is_interesting(n) && n.name == name)
        .max_by_key(|n| n.id)
        .or_else(|| graph.nodes.iter().find(|n| n.name == name))
}

/// Attach construct peeks to each diff item from base/head IR.
pub fn enrich_diff_peeks(diff: &mut StructDiff, base: &IrGraph, head: &IrGraph) {
    let mut peeks: Vec<Option<ConstructPeek>> = Vec::with_capacity(diff.items.len());
    let mut peeks_base: Vec<Option<ConstructPeek>> = Vec::with_capacity(diff.items.len());
    for item in &diff.items {
        match item {
            DiffItem::Added { name, .. } => {
                peeks.push(
                    find_node_by_name(head, name).map(|n| construct_peek_from_node(head, n, "head")),
                );
                peeks_base.push(None);
            }
            DiffItem::Removed { name, .. } => {
                peeks.push(
                    find_node_by_name(base, name).map(|n| construct_peek_from_node(base, n, "base")),
                );
                peeks_base.push(None);
            }
            DiffItem::Renamed {
                from_name, to_name, ..
            } => {
                peeks.push(
                    find_node_by_name(head, to_name)
                        .map(|n| construct_peek_from_node(head, n, "head")),
                );
                peeks_base.push(
                    find_node_by_name(base, from_name)
                        .map(|n| construct_peek_from_node(base, n, "base")),
                );
            }
            DiffItem::SignatureChanged { name, .. }
            | DiffItem::BodyChanged { name, .. }
            | DiffItem::AnnotationsChanged { name, .. } => {
                peeks.push(
                    find_node_by_name(head, name).map(|n| construct_peek_from_node(head, n, "head")),
                );
                peeks_base.push(
                    find_node_by_name(base, name).map(|n| construct_peek_from_node(base, n, "base")),
                );
            }
        }
    }
    if peeks.iter().any(|p| p.is_some()) {
        diff.item_peeks = Some(peeks);
    }
    if peeks_base.iter().any(|p| p.is_some()) {
        diff.item_peeks_base = Some(peeks_base);
    }
}

/// Apply agent intents onto peeks (by construct name).
pub fn apply_intents_to_peeks(diff: &mut StructDiff, intents: &std::collections::HashMap<String, String>) {
    if intents.is_empty() {
        return;
    }
    if let Some(peeks) = diff.item_peeks.as_mut() {
        for p in peeks.iter_mut().flatten() {
            if p.intent.is_none() {
                if let Some(i) = intents.get(&p.name).or_else(|| {
                    intents
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(&p.name))
                        .map(|(_, v)| v)
                }) {
                    p.intent = Some(i.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{EdgeKind, IrGraph, NodeKind};
    use crate::span::Span;

    fn node(g: &mut IrGraph, kind: NodeKind, name: &str, parent: Option<u64>) -> u64 {
        let id = g.add_node(kind, name.to_string(), Span::new(0, 0));
        if let Some(p) = parent {
            if let Some(n) = g.nodes.iter_mut().find(|n| n.id == id) {
                n.metadata.parent = Some(p);
            }
            g.add_edge(p, id, EdgeKind::Contains);
        }
        id
    }

    #[test]
    fn unique_pair_is_rename() {
        let mut base = IrGraph::new();
        let root = node(&mut base, NodeKind::Solution, "pkg", None);
        node(&mut base, NodeKind::TypeDef, "User", Some(root));

        let mut head = IrGraph::new();
        let root2 = node(&mut head, NodeKind::Solution, "pkg", None);
        node(&mut head, NodeKind::TypeDef, "Order", Some(root2));

        let d = structural_diff(&base, &head, "base", "head");
        assert!(d.items.iter().any(|i| matches!(i, DiffItem::Renamed { .. })));
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
    }

    #[test]
    fn detects_added_and_removed() {
        let mut base = IrGraph::new();
        let root = node(&mut base, NodeKind::Solution, "pkg", None);
        node(&mut base, NodeKind::TypeDef, "User", Some(root));
        node(&mut base, NodeKind::TypeDef, "Cart", Some(root));

        let mut head = IrGraph::new();
        let root2 = node(&mut head, NodeKind::Solution, "pkg", None);
        node(&mut head, NodeKind::TypeDef, "User", Some(root2));
        node(&mut head, NodeKind::Flow, "Checkout", Some(root2));

        let d = structural_diff(&base, &head, "base", "head");
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
    }

    #[test]
    fn detects_body_change() {
        let mut base = IrGraph::new();
        let root = node(&mut base, NodeKind::Solution, "pkg", None);
        let step = node(&mut base, NodeKind::Step, "Create", Some(root));
        node(&mut base, NodeKind::Action, "guard ok", Some(step));

        let mut head = IrGraph::new();
        let root2 = node(&mut head, NodeKind::Solution, "pkg", None);
        let step2 = node(&mut head, NodeKind::Step, "Create", Some(root2));
        node(&mut head, NodeKind::Action, "call Bus.dispatch", Some(step2));

        let d = structural_diff(&base, &head, "base", "head");
        assert!(d.items.iter().any(|i| matches!(i, DiffItem::BodyChanged { .. })));
    }

    #[test]
    fn review_order_removed_before_added() {
        // Two removals + two adds so rename pairing doesn't collapse pairs.
        let mut base = IrGraph::new();
        let root = node(&mut base, NodeKind::Solution, "pkg", None);
        node(&mut base, NodeKind::TypeDef, "LegacyA", Some(root));
        node(&mut base, NodeKind::TypeDef, "LegacyB", Some(root));

        let mut head = IrGraph::new();
        let root2 = node(&mut head, NodeKind::Solution, "pkg", None);
        node(&mut head, NodeKind::TypeDef, "NewA", Some(root2));
        node(&mut head, NodeKind::Flow, "Checkout", Some(root2));

        let d = structural_diff(&base, &head, "base", "head");
        assert!(d.removed >= 1 && d.added >= 1);
        // All removals should appear before any additions.
        let first_added = d
            .items
            .iter()
            .position(|i| matches!(i, DiffItem::Added { .. }))
            .expect("added");
        assert!(d.items[..first_added]
            .iter()
            .all(|i| matches!(i, DiffItem::Removed { .. })));
    }
}
