//! Presentation layout + nest projection (LAY-006 / LAY-007).
//!
//! Pure functions that map a host + candidates + [`ViewSpec`] to display nodes.
//! Mirrors `veil-viewer/src/lib/presentation.ts`. Construct identity is by
//! **name** (subkind), never keyword.
//!
//! # Nest rules (LAY-007)
//!
//! | `when` | Meaning |
//! |--------|---------|
//! | `declared_in_parent` / `in_parent_type` | AST ancestor of child is parent construct |
//! | `same_source_group` | Child and parent share nearest Group ancestor name |
//! | `always` | Attach under a parent-type candidate (deterministic pick) |
//! | `implements` | IR `Implements` edge between child and parent (either dir) |
//!
//! **Type membership** (field-type links) is **not** in IR yet — deferred; see
//! `docs/PRESENTATION.md` §6.4.

use crate::presentation::ViewSpec;
use std::collections::{HashMap, HashSet};

/// MVP layout ids (strict at layer load; runtime may fall back).
pub const MVP_LAYOUTS: &[&str] = &["flat", "tabs", "tree", "flow"];

/// Known layouts including deferred ones still accepted at load.
pub const KNOWN_LAYOUTS: &[&str] = &["flat", "tabs", "tree", "flow", "bipartite"];

/// Nest `when` predicates supported by the projector.
pub const NEST_WHENS: &[&str] = &[
    "declared_in_parent",
    "in_parent_type",
    "same_source_group",
    "always",
    "implements",
];

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

/// Edge used by nest `when implements` (and future edge predicates).
#[derive(Debug, Clone)]
pub struct ProjectEdge {
    pub from: u32,
    pub to: u32,
    /// e.g. `Implements`, `Calls`, `References`
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOutput {
    /// Resolved layout after unknown→flat fallback.
    pub layout: String,
    /// True if the view's layout was unknown and remapped to `flat`.
    pub layout_fallback: bool,
    /// Top-level node ids to show (empty for `tabs` — use tab keys).
    /// Does **not** include nested children or (when bucket) orphans.
    pub node_ids: Vec<u32>,
    /// Tab keys for `tabs` layout.
    pub tabs: Vec<String>,
    /// tab name → optional group node id.
    pub tab_group_ids: Vec<(String, Option<u32>)>,
    /// For `flow`: suggested ELK direction (`LR` | `TB`).
    pub flow_direction: Option<String>,
    /// Nest attachments: (child_id, parent_id). Deterministic; cycle-free.
    pub nest_edges: Vec<(u32, u32)>,
    /// Orphan node ids (not roots, not nested) — for `list` also in `node_ids`.
    pub orphan_ids: Vec<u32>,
    /// When orphan_policy is `bucket` / `bucket:Name`, label for synthetic folder.
    pub orphan_bucket_label: Option<String>,
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

/// Project `host_id`'s children under `view` (no edges).
pub fn project_view(
    nodes: &[ProjectInputNode],
    host_id: u32,
    view: &ViewSpec,
    hide_layer_provided: bool,
) -> ProjectOutput {
    project_view_with_edges(nodes, &[], host_id, view, hide_layer_provided)
}

/// Project with IR edges available for `when implements` (LAY-007).
pub fn project_view_with_edges(
    nodes: &[ProjectInputNode],
    edges: &[ProjectEdge],
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
        "tree" => project_tree(nodes, edges, &candidates, view, layout_fallback),
        "flow" => ProjectOutput {
            layout: "flow".into(),
            layout_fallback,
            node_ids: candidates.iter().map(|n| n.id).collect(),
            tabs: Vec::new(),
            tab_group_ids: Vec::new(),
            flow_direction: Some("LR".into()),
            nest_edges: Vec::new(),
            orphan_ids: Vec::new(),
            orphan_bucket_label: None,
        },
        // flat (and bipartite fallback)
        _ => ProjectOutput {
            layout: "flat".into(),
            layout_fallback,
            node_ids: candidates.iter().map(|n| n.id).collect(),
            tabs: Vec::new(),
            tab_group_ids: Vec::new(),
            flow_direction: None,
            nest_edges: Vec::new(),
            orphan_ids: Vec::new(),
            orphan_bucket_label: None,
        },
    }
}

/// Parse orphan_policy string → (mode, optional bucket label).
/// Accepts `list`, `hide`, `bucket`, `bucket:Name`, `bucket Name`.
pub fn parse_orphan_policy(raw: &str) -> (String, Option<String>) {
    let s = raw.trim();
    if s.is_empty() {
        return ("list".into(), None);
    }
    if s == "list" || s == "hide" {
        return (s.into(), None);
    }
    if s == "bucket" {
        return ("bucket".into(), Some("Other".into()));
    }
    if let Some(name) = s.strip_prefix("bucket:") {
        let name = name.trim();
        return (
            "bucket".into(),
            Some(if name.is_empty() {
                "Other".into()
            } else {
                name.to_string()
            }),
        );
    }
    if let Some(name) = s.strip_prefix("bucket ") {
        let name = name.trim();
        return (
            "bucket".into(),
            Some(if name.is_empty() {
                "Other".into()
            } else {
                name.to_string()
            }),
        );
    }
    // Unknown — treat as list (validation should have caught at load)
    ("list".into(), None)
}

/// Whether an orphan_policy string is valid at layer load.
pub fn orphan_policy_valid(raw: &str) -> bool {
    let s = raw.trim();
    s.is_empty()
        || s == "list"
        || s == "hide"
        || s == "bucket"
        || s.starts_with("bucket:")
        || s.starts_with("bucket ")
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
    // Flatten Groups, then for tree include full subtrees so nest rules can
    // see Event under Aggregate under group domain (LAY-007).
    let top = flatten_groups(nodes, direct);
    if layout == "tree" {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for n in top {
            if seen.insert(n.id) {
                out.push(n);
            }
            for d in descendants(nodes, n.id) {
                if seen.insert(d.id) {
                    out.push(d);
                }
            }
        }
        out.sort_by_key(|n| n.id);
        return out;
    }
    top
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
        nest_edges: Vec::new(),
        orphan_ids: Vec::new(),
        orphan_bucket_label: None,
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

/// Nearest Group-shaped ancestor's **name** (source group bucket), if any.
fn nearest_group_name(nodes: &[ProjectInputNode], node: &ProjectInputNode) -> Option<String> {
    let map = by_id(nodes);
    let mut pid = node.parent;
    let mut seen = HashSet::new();
    while let Some(id) = pid {
        if !seen.insert(id) {
            break;
        }
        let p = map.get(&id)?;
        if p.is_group {
            return Some(p.name.clone());
        }
        pid = p.parent;
    }
    None
}

fn implements_edge(edges: &[ProjectEdge], a: u32, b: u32) -> bool {
    edges.iter().any(|e| {
        e.kind.eq_ignore_ascii_case("Implements")
            && ((e.from == a && e.to == b) || (e.from == b && e.to == a))
    })
}

/// Candidate parent ids of construct `parent_type` for child `c`, ordered for
/// ambiguity resolution (LAY-007 §6.3).
fn candidate_parents(
    nodes: &[ProjectInputNode],
    edges: &[ProjectEdge],
    candidates: &[&ProjectInputNode],
    candidate_ids: &HashSet<u32>,
    c: &ProjectInputNode,
    parent_type: &str,
    when: &str,
) -> Vec<u32> {
    let mut parents: Vec<u32> = candidates
        .iter()
        .filter(|p| p.construct == parent_type && p.id != c.id)
        .filter(|p| match when {
            "declared_in_parent" | "in_parent_type" => {
                ancestor_with_construct(nodes, c, parent_type) == Some(p.id)
            }
            "same_source_group" => {
                let cg = nearest_group_name(nodes, c);
                let pg = nearest_group_name(nodes, p);
                cg.is_some() && cg == pg && candidate_ids.contains(&p.id)
            }
            "always" => true,
            "implements" => implements_edge(edges, c.id, p.id),
            _ => ancestor_with_construct(nodes, c, parent_type) == Some(p.id),
        })
        .map(|p| p.id)
        .collect();

    // Deterministic ambiguity: AST parent first, then lowest id.
    let ast_pref = ancestor_with_construct(nodes, c, parent_type);
    parents.sort_by_key(|id| {
        let prefer = if Some(*id) == ast_pref { 0u8 } else { 1u8 };
        (prefer, *id)
    });
    parents.dedup();
    parents
}

/// Would attaching child→parent create a cycle given existing nest edges?
fn would_cycle(nest: &HashMap<u32, u32>, child: u32, parent: u32) -> bool {
    if child == parent {
        return true;
    }
    let mut walk = Some(parent);
    let mut seen = HashSet::new();
    while let Some(id) = walk {
        if id == child {
            return true;
        }
        if !seen.insert(id) {
            return true;
        }
        walk = nest.get(&id).copied();
    }
    false
}

fn project_tree(
    nodes: &[ProjectInputNode],
    edges: &[ProjectEdge],
    candidates: &[&ProjectInputNode],
    view: &ViewSpec,
    layout_fallback: bool,
) -> ProjectOutput {
    let candidate_ids: HashSet<u32> = candidates.iter().map(|n| n.id).collect();
    let root_names: HashSet<&str> = view.roots.iter().map(|s| s.as_str()).collect();

    // child_id → parent_id (first matching rule wins; cycle-free)
    let mut nest: HashMap<u32, u32> = HashMap::new();

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
            if nest.contains_key(&c.id) {
                continue; // already attached by earlier rule
            }
            let parents = candidate_parents(
                nodes,
                edges,
                candidates,
                &candidate_ids,
                c,
                &rule.parent,
                when,
            );
            if let Some(&pid) = parents.first() {
                if !would_cycle(&nest, c.id, pid) {
                    nest.insert(c.id, pid);
                }
            }
        }
    }

    let nested: HashSet<u32> = nest.keys().copied().collect();

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

    let (policy, bucket_label) = parse_orphan_policy(&view.orphan_policy);
    let mut node_ids = roots.clone();
    match policy.as_str() {
        "hide" => {}
        "bucket" => {
            // Orphans excluded from top-level; UI places under synthetic bucket.
        }
        _ => {
            // list (default)
            node_ids.extend(orphans.iter().copied());
        }
    }

    let mut nest_edges: Vec<(u32, u32)> = nest.into_iter().map(|(c, p)| (c, p)).collect();
    nest_edges.sort_by_key(|(c, p)| (*c, *p));

    ProjectOutput {
        layout: "tree".into(),
        layout_fallback,
        node_ids,
        tabs: Vec::new(),
        tab_group_ids: Vec::new(),
        flow_direction: None,
        nest_edges,
        orphan_ids: orphans,
        orphan_bucket_label: if policy == "bucket" {
            bucket_label
        } else {
            None
        },
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
        assert!(out.nest_edges.contains(&(11, 10)));
        assert!(out.node_ids.contains(&12), "orphan OrphanC listed");
        // Svc is under app group — ServiceType is not root and not nested → orphan
        assert!(out.node_ids.contains(&20));
        assert!(out.orphan_ids.contains(&12));
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

    // ─── LAY-007 nest rules ────────────────────────────────────────────

    #[test]
    fn nest_declared_in_parent_emits_edge() {
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
        assert!(out.nest_edges.contains(&(11, 10)));
        assert!(!out.node_ids.contains(&11));
    }

    #[test]
    fn nest_same_source_group() {
        // Two parents of ParentType; child shares group with one only.
        let nodes = vec![
            node(1, None, "H", "Host", false),
            node(2, Some(1), "domain", "Group", true),
            node(3, Some(1), "app", "Group", true),
            node(10, Some(2), "P1", "ParentType", false),
            node(11, Some(3), "P2", "ParentType", false),
            node(12, Some(2), "C", "ChildType", false), // domain group with P1
        ];
        let view = ViewSpec {
            id: "m".into(),
            label: "M".into(),
            layout: "tree".into(),
            is_default: false,
            members: "by_host_children".into(),
            roots: vec!["ParentType".into()],
            nest_rules: vec![NestRule {
                child: "ChildType".into(),
                parent: "ParentType".into(),
                when: "same_source_group".into(),
            }],
            orphan_policy: "list".into(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.nest_edges, vec![(12, 10)]); // under P1 in domain, not P2
    }

    #[test]
    fn nest_always_picks_lowest_id_when_ambiguous() {
        let nodes = vec![
            node(1, None, "H", "Host", false),
            node(10, Some(1), "P1", "ParentType", false),
            node(11, Some(1), "P2", "ParentType", false),
            node(12, Some(1), "C", "ChildType", false), // not under either AST
        ];
        let view = ViewSpec {
            id: "m".into(),
            label: "M".into(),
            layout: "tree".into(),
            is_default: false,
            members: "by_host_children".into(),
            roots: vec!["ParentType".into()],
            nest_rules: vec![NestRule {
                child: "ChildType".into(),
                parent: "ParentType".into(),
                when: "always".into(),
            }],
            orphan_policy: "hide".into(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        assert_eq!(out.nest_edges, vec![(12, 10)]); // lowest parent id
    }

    #[test]
    fn nest_implements_edge() {
        let nodes = vec![
            node(1, None, "H", "Host", false),
            node(10, Some(1), "PortA", "PortType", false),
            node(11, Some(1), "AdA", "AdapterType", false),
        ];
        let edges = vec![ProjectEdge {
            from: 11,
            to: 10,
            kind: "Implements".into(),
        }];
        let view = ViewSpec {
            id: "ports".into(),
            label: "P".into(),
            layout: "tree".into(),
            is_default: false,
            members: "by_host_children".into(),
            roots: vec!["PortType".into()],
            nest_rules: vec![NestRule {
                child: "AdapterType".into(),
                parent: "PortType".into(),
                when: "implements".into(),
            }],
            orphan_policy: "list".into(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view_with_edges(&nodes, &edges, 1, &view, true);
        assert_eq!(out.nest_edges, vec![(11, 10)]);
        assert!(!out.node_ids.contains(&11));
        assert!(out.node_ids.contains(&10));
    }

    #[test]
    fn nest_skips_cycle() {
        // A under B when always, B under A when always — second attach refused
        let nodes = vec![
            node(1, None, "H", "Host", false),
            node(10, Some(1), "A", "TypeA", false),
            node(11, Some(1), "B", "TypeB", false),
        ];
        let view = ViewSpec {
            id: "m".into(),
            label: "M".into(),
            layout: "tree".into(),
            is_default: false,
            members: "by_host_children".into(),
            roots: vec![],
            nest_rules: vec![
                NestRule {
                    child: "TypeA".into(),
                    parent: "TypeB".into(),
                    when: "always".into(),
                },
                NestRule {
                    child: "TypeB".into(),
                    parent: "TypeA".into(),
                    when: "always".into(),
                },
            ],
            orphan_policy: "list".into(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let out = project_view(&nodes, 1, &view, true);
        // First rule: A→B; second would cycle B→A — skipped
        assert_eq!(out.nest_edges, vec![(10, 11)]);
        assert!(!out.nest_edges.iter().any(|(c, p)| *c == 11 && *p == 10));
    }

    #[test]
    fn orphan_policy_hide_and_bucket() {
        let nodes = fixture();
        let base = |policy: &str| ViewSpec {
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
            orphan_policy: policy.into(),
            tabs: vec![],
            left: vec![],
            right: vec![],
            edge: None,
        };
        let hide = project_view(&nodes, 1, &base("hide"), true);
        assert!(hide.node_ids.contains(&10));
        assert!(!hide.node_ids.contains(&12));
        assert!(hide.orphan_ids.contains(&12));
        assert!(hide.orphan_bucket_label.is_none());

        let bucket = project_view(&nodes, 1, &base("bucket:Unplaced"), true);
        assert!(bucket.node_ids.contains(&10));
        assert!(!bucket.node_ids.contains(&12));
        assert!(bucket.orphan_ids.contains(&12));
        assert_eq!(bucket.orphan_bucket_label.as_deref(), Some("Unplaced"));
    }

    #[test]
    fn parse_orphan_policy_forms() {
        assert_eq!(parse_orphan_policy(""), ("list".into(), None));
        assert_eq!(parse_orphan_policy("hide"), ("hide".into(), None));
        assert_eq!(
            parse_orphan_policy("bucket"),
            ("bucket".into(), Some("Other".into()))
        );
        assert_eq!(
            parse_orphan_policy("bucket:Misc"),
            ("bucket".into(), Some("Misc".into()))
        );
        assert!(orphan_policy_valid("bucket:Foo"));
        assert!(!orphan_policy_valid("vanish"));
    }
}
