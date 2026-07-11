//! Layer-driven presentation model (LAY-001 / LAY-002).
//!
//! Normative grammar: `docs/PRESENTATION.md`. Types are stored on
//! [`crate::layer::ConstructSpec`] and exposed via
//! [`presentation_from_registry`] for the viewer API.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::layer::LayerRegistry;

// ─── Spec types (stored on constructs) ─────────────────────────────────────

/// Presentation metadata for one construct type (`present` block).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConstructPresentation {
    /// Named views when this construct is a drill host.
    #[serde(default)]
    pub views: Vec<ViewSpec>,
    /// `container` | `leaf` | `edge_endpoint`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Default view id when this type is the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_view: Option<String>,
    /// Optional nestable hints (sugar; view nest_rules are authoritative).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nestable: Vec<NestableHint>,
    /// Review lens tags (`critical`, `integration`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lenses: Vec<String>,
}

impl ConstructPresentation {
    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
            && self.role.is_none()
            && self.default_view.is_none()
            && self.nestable.is_empty()
            && self.lenses.is_empty()
    }
}

/// One named view under a host construct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewSpec {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// `flat` | `tabs` | `tree` | `flow` | `bipartite`
    pub layout: String,
    /// Selected by default when drilling into the host.
    #[serde(default)]
    pub is_default: bool,
    /// Empty → default from layout (see PRESENTATION.md).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub members: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nest_rules: Vec<NestRule>,
    /// Empty → default `list` for tree.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub orphan_policy: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub right: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<String>,
}

impl ViewSpec {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: String::new(),
            layout: String::new(),
            is_default: false,
            members: String::new(),
            roots: Vec::new(),
            nest_rules: Vec::new(),
            orphan_policy: String::new(),
            tabs: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
            edge: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NestRule {
    pub child: String,
    pub parent: String,
    /// Empty → `declared_in_parent`
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NestableHint {
    pub view_id: String,
    /// `root` or parent construct name
    pub under: String,
}

// ─── API / machine IR ──────────────────────────────────────────────────────

/// Serializable presentation model for the IDE (PRESENTATION.md §8).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresentationModel {
    pub version: u32,
    /// Host construct name → views + default.
    pub hosts: BTreeMap<String, HostPresentation>,
    /// Per-construct roles / lenses.
    pub constructs: BTreeMap<String, ConstructRoleDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_view: Option<String>,
    pub views: Vec<ViewSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConstructRoleDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lenses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_view: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nestable: Vec<NestableHint>,
}

/// Build API presentation IR from the loaded registry.
pub fn presentation_from_registry(reg: &LayerRegistry) -> PresentationModel {
    let mut hosts = BTreeMap::new();
    let mut constructs = BTreeMap::new();

    for c in &reg.constructs {
        let p = &c.presentation;
        if p.is_empty() {
            continue;
        }

        if !p.views.is_empty() {
            let default_view = p
                .default_view
                .clone()
                .or_else(|| {
                    p.views
                        .iter()
                        .find(|v| v.is_default)
                        .map(|v| v.id.clone())
                });
            hosts.insert(
                c.name.clone(),
                HostPresentation {
                    default_view,
                    views: p.views.clone(),
                },
            );
        }

        if p.role.is_some()
            || !p.lenses.is_empty()
            || p.default_view.is_some()
            || !p.nestable.is_empty()
        {
            constructs.insert(
                c.name.clone(),
                ConstructRoleDto {
                    role: p.role.clone(),
                    lenses: p.lenses.clone(),
                    default_view: p.default_view.clone(),
                    nestable: p.nestable.clone(),
                },
            );
        }
    }

    PresentationModel {
        version: 1,
        hosts,
        constructs,
    }
}

// ─── Validation ────────────────────────────────────────────────────────────

const LAYOUTS: &[&str] = &["flat", "tabs", "tree", "flow", "bipartite"];
const MEMBERS: &[&str] = &[
    "by_host_children",
    "by_source_group",
    "by_construct",
    "all_descendants",
];
const WHENS: &[&str] = &[
    "declared_in_parent",
    "in_parent_type",
    "same_source_group",
    "always",
    "implements",
    "references",
];
const ROLES: &[&str] = &["container", "leaf", "edge_endpoint"];

/// Validate presentation on all constructs; `known_names` = construct names.
pub fn validate_presentations(
    constructs: &[(String, &ConstructPresentation)],
    known_names: &std::collections::HashSet<String>,
) -> Result<(), String> {
    for (cname, p) in constructs {
        if let Some(role) = &p.role {
            if !ROLES.contains(&role.as_str()) {
                return Err(format!(
                    "construct '{cname}': unknown present role '{role}' (expected one of {})",
                    ROLES.join(", ")
                ));
            }
        }
        for v in &p.views {
            if v.layout.is_empty() {
                return Err(format!(
                    "construct '{cname}' view '{}': layout is required",
                    v.id
                ));
            }
            if !LAYOUTS.contains(&v.layout.as_str()) {
                return Err(format!(
                    "construct '{cname}' view '{}': unknown layout '{}' (expected one of {})",
                    v.id,
                    v.layout,
                    LAYOUTS.join(", ")
                ));
            }
            if !v.members.is_empty() && !MEMBERS.contains(&v.members.as_str()) {
                return Err(format!(
                    "construct '{cname}' view '{}': unknown members '{}' (expected one of {})",
                    v.id,
                    v.members,
                    MEMBERS.join(", ")
                ));
            }
            if !v.orphan_policy.is_empty()
                && !crate::project::orphan_policy_valid(&v.orphan_policy)
            {
                return Err(format!(
                    "construct '{cname}' view '{}': unknown orphan_policy '{}' (expected list|hide|bucket[:Name])",
                    v.id, v.orphan_policy
                ));
            }
            for r in &v.roots {
                if !known_names.contains(r) {
                    return Err(format!(
                        "construct '{cname}' view '{}': unknown root construct '{r}'",
                        v.id
                    ));
                }
            }
            for rule in &v.nest_rules {
                if !known_names.contains(&rule.child) {
                    return Err(format!(
                        "construct '{cname}' view '{}': nest child '{}' is not a known construct",
                        v.id, rule.child
                    ));
                }
                if !known_names.contains(&rule.parent) {
                    return Err(format!(
                        "construct '{cname}' view '{}': nest parent '{}' is not a known construct",
                        v.id, rule.parent
                    ));
                }
                let when = if rule.when.is_empty() {
                    "declared_in_parent"
                } else {
                    rule.when.as_str()
                };
                if !WHENS.contains(&when) {
                    return Err(format!(
                        "construct '{cname}' view '{}': unknown nest when '{when}'",
                        v.id
                    ));
                }
            }
        }
    }
    Ok(())
}

// ─── Line parsers (used by layer file loader) ──────────────────────────────

/// Apply a line that belongs inside a `view` block. Returns Err on bad syntax.
pub fn apply_view_line(view: &mut ViewSpec, trimmed: &str) -> Result<(), String> {
    if let Some(rest) = trimmed.strip_prefix("label ") {
        view.label = unquote(rest);
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("layout ") {
        view.layout = rest.trim().to_string();
        return Ok(());
    }
    if trimmed == "default" {
        view.is_default = true;
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("members ") {
        view.members = rest.trim().to_string();
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("roots ") {
        view.roots = split_names(rest);
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("orphan_policy ") {
        view.orphan_policy = rest.trim().to_string();
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("tabs ") {
        view.tabs = split_names(rest);
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("left ") {
        view.left = split_names(rest);
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("right ") {
        view.right = split_names(rest);
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("edge ") {
        view.edge = Some(rest.trim().to_string());
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("nest ") {
        view.nest_rules.push(parse_nest_rule(rest)?);
        return Ok(());
    }
    Err(format!("unknown view property: '{trimmed}'"))
}

/// Whether `trimmed` is a property that belongs inside a `view` block.
pub fn is_view_property_line(trimmed: &str) -> bool {
    trimmed.starts_with("label ")
        || trimmed.starts_with("layout ")
        || trimmed == "default"
        || trimmed.starts_with("members ")
        || trimmed.starts_with("roots ")
        || trimmed.starts_with("orphan_policy ")
        || trimmed.starts_with("tabs ")
        || trimmed.starts_with("left ")
        || trimmed.starts_with("right ")
        || trimmed.starts_with("edge ")
        || trimmed.starts_with("nest ")
}

/// Apply a construct-level `present` line (role, lens, nestable_in, …).
pub fn apply_construct_present_line(
    p: &mut ConstructPresentation,
    trimmed: &str,
) -> Result<(), String> {
    if let Some(rest) = trimmed.strip_prefix("role ") {
        p.role = Some(rest.trim().to_string());
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("default_view ") {
        p.default_view = Some(rest.trim().to_string());
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("lens ") {
        p.lenses.push(rest.trim().to_string());
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("nestable_in ") {
        // nestable_in <view> as root  |  nestable_in <view> under <Parent>
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 3 && parts[1] == "as" && parts[2] == "root" {
            p.nestable.push(NestableHint {
                view_id: parts[0].to_string(),
                under: "root".into(),
            });
            return Ok(());
        }
        if parts.len() == 3 && parts[1] == "under" {
            p.nestable.push(NestableHint {
                view_id: parts[0].to_string(),
                under: parts[2].to_string(),
            });
            return Ok(());
        }
        return Err(format!(
            "invalid nestable_in (expected 'nestable_in <view> as root' or 'nestable_in <view> under <Parent>'): '{trimmed}'"
        ));
    }
    Err(format!("unknown present property: '{trimmed}'"))
}

fn parse_nest_rule(rest: &str) -> Result<NestRule, String> {
    // <Child> under <Parent> [when <pred>]
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() < 3 || parts[1] != "under" {
        return Err(format!(
            "invalid nest rule (expected 'nest <Child> under <Parent> [when <pred>]'): nest {rest}"
        ));
    }
    let child = parts[0].to_string();
    let parent = parts[2].to_string();
    let when = if parts.len() >= 5 && parts[3] == "when" {
        parts[4].to_string()
    } else if parts.len() > 3 && parts[3] != "when" {
        return Err(format!("invalid nest rule trailing tokens: nest {rest}"));
    } else {
        String::new()
    };
    Ok(NestRule {
        child,
        parent,
        when,
    })
}

fn split_names(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nest_rule_with_when() {
        let r = parse_nest_rule("Event under Aggregate when declared_in_parent").unwrap();
        assert_eq!(r.child, "Event");
        assert_eq!(r.parent, "Aggregate");
        assert_eq!(r.when, "declared_in_parent");
    }

    #[test]
    fn apply_view_line_roots_and_tabs() {
        let mut v = ViewSpec::new("model");
        apply_view_line(&mut v, "layout tree").unwrap();
        apply_view_line(&mut v, "roots Aggregate, Entity").unwrap();
        apply_view_line(&mut v, "tabs domain, application").unwrap();
        assert_eq!(v.layout, "tree");
        assert_eq!(v.roots, vec!["Aggregate", "Entity"]);
        assert_eq!(v.tabs, vec!["domain", "application"]);
    }
}
