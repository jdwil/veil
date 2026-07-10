//! Structural / semantic IR diff (UX-021).
//!
//! Compares two IR graphs by stable keys (parent path + kind + name) rather
//! than node ids (which are rebuild-unstable).

use serde::{Deserialize, Serialize};

use crate::ir::{IrGraph, IrNode, NodeKind};

/// One structural change between two IR snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffItem {
    Added {
        path: String,
        node_kind: String,
        name: String,
        subkind: Option<String>,
    },
    Removed {
        path: String,
        node_kind: String,
        name: String,
        subkind: Option<String>,
    },
    Renamed {
        path: String,
        node_kind: String,
        from_name: String,
        to_name: String,
        subkind: Option<String>,
    },
    SignatureChanged {
        path: String,
        node_kind: String,
        name: String,
        before: String,
        after: String,
    },
    BodyChanged {
        path: String,
        node_kind: String,
        name: String,
        before_lines: usize,
        after_lines: usize,
        before_preview: Vec<String>,
        after_preview: Vec<String>,
    },
    AnnotationsChanged {
        path: String,
        node_kind: String,
        name: String,
        before: Vec<String>,
        after: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StructDiff {
    pub base_label: String,
    pub head_label: String,
    pub items: Vec<DiffItem>,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
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
        });
    }

    // Matched keys: signature / body / annotations
    for (key, bn) in &base_map {
        let Some(hn) = head_map.get(key) else {
            continue;
        };
        let path = parent_path(head, hn);
        let bsig = signature_of(bn);
        let hsig = signature_of(hn);
        if bsig != hsig {
            items.push(DiffItem::SignatureChanged {
                path: path.clone(),
                node_kind: kind_str(&hn.kind),
                name: hn.name.clone(),
                before: bsig,
                after: hsig,
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

    StructDiff {
        base_label: base_label.to_string(),
        head_label: head_label.to_string(),
        items,
        added,
        removed,
        changed,
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
}
