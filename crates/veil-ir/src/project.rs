//! Presentation layout projection (LAY-006).
//!
//! Pure functions that map a host + candidates + [`ViewSpec`] to display nodes.
//! Mirrors `veil-viewer/src/lib/presentation.ts` so algorithms are unit-tested
//! without a browser. Construct identity is by **name** (subkind), never keyword.

use crate::presentation::ViewSpec;
use std::collections::{HashMap, HashSet};

/// MVP layout ids (strict at layer load; runtime may fall back).
pub const MVP_LAYOUTS: &[&str] = &["flat", "tabs", "tree", "flow"];

/// Known layouts including deferred ones still accepted at load.
pub const KNOWN_LAYOUTS: &[&str] = &["flat", "tabs", "tree", "flow", "bipartite"];

/// Minimal node for projection tests / shared logic.
#[derive(Debug, Clone)]
pub struct ProjectInputNode {
    pub id: u32,
    pub parent: Option<u32>,
    pub name: String,
    /// Layer construct name / IR subkind.
    pub construct: String,
    /// Core Group shape — organizational bucket.
    pub is_group: bool,
    pub layer_provided: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOutput {
    /// Resolved layout after unknown→flat fallback.
    pub layout: String,
    /// True if the view's layout was unknown and remapped to `flat`.
    pub layout_fallback: bool,
    /// Top-level node ids to show (empty for `tabs` — use tab keys).
    pub node_ids: Vec<u32>,
    /// Tab keys for `tabs` layout.
    pub tabs: Vec<String>,
    /// tab name → optional group node id.
    pub tab_group_ids: Vec<(String, Option<u32>)>,
    /// For `flow`: suggested ELK direction (`LR` | `TB`).
    pub flow_direction: Option<String>,
}

/// Resolve layout string: MVP/known ids pass through; unknown → `flat` + fallback flag.
pub fn resolve_layout(layout: &str) -> (String, bool) {
    let l = layout.trim();
    if l.is_empty() {
        return ("flat".into(), true);
    }
    if KNOWN_LAYOUTS.contains(&l) {
        // bipartite not fully implemented — project as flat with fallback note
        if l == "bipartite" {
            return ("flat".into(), true);
        }
        return (l.to_string(), false);
    }
    ("flat".into(), true)
}

pub fn default_members_for_layout(layout: &str) -> &'static str {
    match layout {
        "tabs" => "by_source_group",
        _ => "by_host_children",
    }
}

/// Project `host_id`'s children under `view`.
pub fn project_view(
    nodes: &[ProjectInputNode],
    host_id: u32,
    view: &ViewSpec,
    hide_layer_provided: bool,
) -> ProjectOutput {
    let (layout, layout_fallback) = resolve_layout(&view.layout);
    let members = if view.members.is_empty() {
        default_members_for_layout(&layout).to_string()
    } else {
        view.members.clone()
    };

    let mut candidates = collect_candidates(nodes, host_id, &members, &layout, view);
    if hide_layer_provided {
        candidates.retain(|n| !n.layer_provided);
    }

    match layout.as_str() {
        "tabs" => project_tabs(&candidates, view, layout_fallback),
        "tree" => project_tree(nodes, &candidates, view, layout_fallback),
        "flow" => ProjectOutput {
            layout: "flow".into(),
            layout_fallback,
            node_ids: candidates.iter().map(|n| n.id).collect(),
            tabs: Vec::new(),
            tab_group_ids: Vec::new(),
            flow_direction: Some("LR".into()),
        },
        // flat (and bipartite fallback)
        _ => ProjectOutput {
            layout: "flat".into(),
            layout_fallback,
            node_ids: candidates.iter().map(|n| n.id).collect(),
            tabs: Vec::new(),
            tab_group_ids: Vec::new(),
            flow_direction: None,
        },
    }
}

fn by_id(nodes: &[ProjectInputNode]) -> HashMap<u32, &ProjectInputNode> {
    nodes.iter().map(|n| (n.id, n)).collect()
}

fn children_of(nodes: &[ProjectInputNode], parent: u32) -> Vec<&ProjectInputNode> {
    let mut kids: Vec<_> = nodes.iter().filter(|n| n.parent == Some(parent)).collect();
    kids.sort_by_key(|n| (n.id, n.name.as_str()));
    kids
}

fn flatten_groups<'a>(nodes: &'a [ProjectInputNode], list: Vec<&'a ProjectInputNode>) -> Vec<&'a ProjectInputNode> {
    let mut out = Vec::new();
    for n in list {
        if n.is_group {
            out.extend(flatten_groups(nodes, children_of(nodes, n.id)));
        } else {
            out.push(n);
        }
    }
    out
}

fn descendants(nodes: &[ProjectInputNode], host: u32) -> Vec<&ProjectInputNode> {
    let mut out = Vec::new();
    let mut queue = vec![host];
    let mut seen = HashSet::from([host]);
    while let Some(id) = queue.pop() {
        for c in children_of(nodes, id) {
            if seen.insert(c.id) {
                out.push(c);
                queue.push(c.id);
            }
        }
    }
    out.sort_by_key(|n| n.id);
    out
}

fn collect_candidates<'a>(
    nodes: &'a [ProjectInputNode],
    host_id: u32,
    members: &str,
    layout: &str,
    view: &ViewSpec,
) -> Vec<&'a ProjectInputNode> {
    if members == "all_descendants" {
        return descendants(nodes, host_id);
    }
    if members == "by_construct" {
        let mut names: HashSet<&str> = view.roots.iter().map(|s| s.as_str()).collect();
        for r in &view.nest_rules {
            names.insert(r.child.as_str());
            names.insert(r.parent.as_str());
        }
        return descendants(nodes, host_id)
            .into_iter()
            .filter(|n| names.contains(n.construct.as_str()))
            .collect();
    }
    let direct = children_of(nodes, host_id);
    if layout == "tabs" || members == "by_source_group" {
        return direct;
    }
    flatten_groups(nodes, direct)
}

fn project_tabs(
    candidates: &[&ProjectInputNode],
    view: &ViewSpec,
    layout_fallback: bool,
) -> ProjectOutput {
    let groups: Vec<_> = candidates.iter().filter(|n| n.is_group).copied().collect();
    let mut tabs = if view.tabs.is_empty() {
        groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>()
    } else {
        view.tabs.clone()
    };
    for g in &groups {
        if !tabs.contains(&g.name) {
            tabs.push(g.name.clone());
        }
    }
    let tab_group_ids = tabs
        .iter()
        .map(|t| {
            let id = groups.iter().find(|g| g.name == *t).map(|g| g.id);
            (t.clone(), id)
        })
        .collect();
    ProjectOutput {
        layout: "tabs".into(),
        layout_fallback,
        node_ids: Vec::new(),
        tabs,
        tab_group_ids,
        flow_direction: None,
    }
}

fn ancestor_with_construct(
    nodes: &[ProjectInputNode],
    node: &ProjectInputNode,
    want: &str,
) -> Option<u32> {
    let map = by_id(nodes);
    let mut pid = node.parent;
    let mut seen = HashSet::new();
    while let Some(id) = pid {
        if !seen.insert(id) {
            break;
        }
        let p = map.get(&id)?;
        if p.construct == want {
            return Some(p.id);
        }
        pid = p.parent;
    }
    None
}

fn project_tree(
    nodes: &[ProjectInputNode],
    candidates: &[&ProjectInputNode],
    view: &ViewSpec,
    layout_fallback: bool,
) -> ProjectOutput {
    let candidate_ids: HashSet<u32> = candidates.iter().map(|n| n.id).collect();
    let root_names: HashSet<&str> = view.roots.iter().map(|s| s.as_str()).collect();

    let mut nested = HashSet::new();
    for rule in &view.nest_rules {
        let when = if rule.when.is_empty() {
            "declared_in_parent"
        } else {
            rule.when.as_str()
        };
        for c in candidates {
            if c.construct != rule.child {
                continue;
            }
            let attach = match when {
                "always" => candidates.iter().any(|n| n.construct == rule.parent),
                _ => ancestor_with_construct(nodes, c, &rule.parent)
                    .map(|id| candidate_ids.contains(&id))
                    .unwrap_or(false),
            };
            if attach {
                nested.insert(c.id);
            }
        }
    }

    let is_root = |n: &ProjectInputNode| {
        if root_names.is_empty() {
            !nested.contains(&n.id)
        } else {
            root_names.contains(n.construct.as_str())
        }
    };

    let roots: Vec<u32> = candidates
        .iter()
        .filter(|n| is_root(n) && !nested.contains(&n.id))
        .map(|n| n.id)
        .collect();
    let orphans: Vec<u32> = candidates
        .iter()
        .filter(|n| !is_root(n) && !nested.contains(&n.id))
        .map(|n| n.id)
        .collect();

    let policy = if view.orphan_policy.is_empty() {
        "list"
    } else {
        view.orphan_policy.as_str()
    };
    let mut node_ids = roots;
    if policy == "list" || policy == "bucket" {
        node_ids.extend(orphans);
    }
    // hide: roots only

    ProjectOutput {
        layout: "tree".into(),
        layout_fallback,
        node_ids,
        tabs: Vec::new(),
        tab_group_ids: Vec::new(),
        flow_direction: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::{NestRule, ViewSpec};

    fn node(
        id: u32,
        parent: Option<u32>,
        name: &str,
        construct: &str,
        is_group: bool,
    ) -> ProjectInputNode {
        ProjectInputNode {
            id,
            parent,
            name: name.into(),
            construct: construct.into(),
            is_group,
            layer_provided: false,
        }
    }

    fn fixture() -> Vec<ProjectInputNode> {
        vec![
            node(1, None, "H", "Host", false),
            node(2, Some(1), "domain", "Group", true),
            node(3, Some(1), "app", "Group", true),
            node(10, Some(2), "RootA", "RootType", false),
            node(11, Some(10), "ChildB", "ChildType", false),
            node(12, Some(2), "OrphanC", "OtherType", false),
            node(20, Some(3), "Svc", "ServiceType", false),
        ]
    }

    #[test]
    fn layout_flat_lists_flattened_children() {
        let nodes = fixture();
        let view = ViewSpec {
            id: "all".into(),
            label: "All".into(),
            layout: "flat".into(),
            is_default: true,
            members: "by_host_children".into(),
            roots: vec![],
            nest_rules: vec![],
            orphan_policy: String::new(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.layout, "flat");
        assert!(!out.layout_fallback);
        // Flattened through groups: RootA, ChildB, OrphanC, Svc
        assert!(out.node_ids.contains(&10));
        assert!(out.node_ids.contains(&20));
        assert!(out.tabs.is_empty());
    }

    #[test]
    fn layout_tabs_partitions_groups() {
        let nodes = fixture();
        let view = ViewSpec {
            id: "g".into(),
            label: "G".into(),
            layout: "tabs".into(),
            is_default: true,
            members: "by_source_group".into(),
            roots: vec![],
            nest_rules: vec![],
            orphan_policy: String::new(),
            tabs: vec!["domain".into(), "app".into()],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.layout, "tabs");
        assert_eq!(out.tabs, vec!["domain", "app"]);
        assert_eq!(out.tab_group_ids[0], ("domain".into(), Some(2)));
        assert!(out.node_ids.is_empty());
    }

    #[test]
    fn layout_tree_roots_and_nests() {
        let nodes = fixture();
        let view = ViewSpec {
            id: "model".into(),
            label: "M".into(),
            layout: "tree".into(),
            is_default: false,
            members: "by_host_children".into(),
            roots: vec!["RootType".into()],
            nest_rules: vec![NestRule {
                child: "ChildType".into(),
                parent: "RootType".into(),
                when: "declared_in_parent".into(),
            }],
            orphan_policy: "list".into(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.layout, "tree");
        assert!(out.node_ids.contains(&10), "root RootA");
        assert!(!out.node_ids.contains(&11), "ChildB nested away");
        assert!(out.node_ids.contains(&12), "orphan OrphanC listed");
        // Svc is under app group, construct OtherType not root — orphan-ish
        // ServiceType is not root and not nested → orphan listed
        assert!(out.node_ids.contains(&20));
    }

    #[test]
    fn layout_flow_sets_lr_direction() {
        let nodes = fixture();
        let view = ViewSpec {
            id: "f".into(),
            label: "F".into(),
            layout: "flow".into(),
            is_default: false,
            members: "by_host_children".into(),
            roots: vec![],
            nest_rules: vec![],
            orphan_policy: String::new(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.layout, "flow");
        assert_eq!(out.flow_direction.as_deref(), Some("LR"));
        assert!(!out.node_ids.is_empty());
    }

    #[test]
    fn unknown_layout_falls_back_to_flat() {
        let nodes = fixture();
        let view = ViewSpec {
            id: "x".into(),
            label: "X".into(),
            layout: "spiral_galaxy".into(),
            is_default: false,
            members: String::new(),
            roots: vec![],
            nest_rules: vec![],
            orphan_policy: String::new(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.layout, "flat");
        assert!(out.layout_fallback);
    }

    #[test]
    fn mvp_layouts_documented() {
        assert!(MVP_LAYOUTS.contains(&"flat"));
        assert!(MVP_LAYOUTS.contains(&"tabs"));
        assert!(MVP_LAYOUTS.contains(&"tree"));
        assert!(MVP_LAYOUTS.contains(&"flow"));
    }
}
