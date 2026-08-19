//! Generic pre-deploy hooks (`role:deploy_hook`).
//!
//! The engine matches the role, never the keyword `hook` (INV-001). Product
//! hooks iterate typed `DeployContext.constructs`; this module only dumps IR.

use serde::Serialize;

use crate::ast::{Construct, Solution, TopLevelItem};
use crate::layer::{LayerRegistry, Shape};

/// INV-001 role for provisioner hooks (after compile, before code publish).
pub const DEPLOY_HOOK_ROLE: &str = "deploy_hook";

/// One construct in the deploy inventory (host → hook JSON).
#[derive(Debug, Clone, Serialize)]
pub struct DeployedConstructDump {
    pub name: String,
    pub keyword: String,
    pub package: String,
    pub annotations: Vec<DeployedAnnotationDump>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeployedAnnotationDump {
    pub name: String,
    pub args: Vec<String>,
    pub roles: Vec<String>,
}

/// Fn-shaped constructs whose layer spec carries `role:deploy_hook`.
pub fn collect_deploy_hooks<'a>(
    sol: &'a Solution,
    registry: &'a LayerRegistry,
) -> Vec<&'a Construct> {
    registry.constructs_with_role(sol, DEPLOY_HOOK_ROLE)
}

/// True when `c` is a deploy hook (role only).
pub fn is_deploy_hook(c: &Construct, registry: &LayerRegistry) -> bool {
    registry.construct_has_role(c, DEPLOY_HOOK_ROLE)
}

/// Walk every construct in `sol` and dump name / keyword / annotations + roles.
pub fn collect_construct_inventory(
    sol: &Solution,
    registry: &LayerRegistry,
    package: &str,
) -> Vec<DeployedConstructDump> {
    let mut out = Vec::new();
    fn walk(
        c: &Construct,
        registry: &LayerRegistry,
        package: &str,
        out: &mut Vec<DeployedConstructDump>,
    ) {
        if c.shape != Shape::Group {
            out.push(dump_construct(c, registry, package));
        }
        for child in &c.children {
            walk(child, registry, package, out);
        }
    }
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            walk(c, registry, package, &mut out);
        }
    }
    out
}

fn dump_construct(
    c: &Construct,
    registry: &LayerRegistry,
    package: &str,
) -> DeployedConstructDump {
    let annotations = c
        .annotations
        .iter()
        .map(|a| DeployedAnnotationDump {
            name: a.name.clone(),
            args: a.args.clone(),
            roles: registry.annotation_roles(&a.name),
        })
        .collect();
    DeployedConstructDump {
        name: c.name.clone(),
        keyword: c.keyword.clone(),
        package: package.to_string(),
        annotations,
    }
}

/// JSON text of the construct inventory (host embeds this as `constructs`).
pub fn constructs_json(
    sol: &Solution,
    registry: &LayerRegistry,
    package: &str,
) -> Result<String, String> {
    let inv = collect_construct_inventory(sol, registry, package);
    serde_json::to_string(&inv).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// Platform `deploy.layer` teaches inventory + language, never product
    /// annotation names. Those live in the product layers that declare them.
    #[test]
    fn deploy_layer_prompt_is_product_agnostic() {
        let src = include_str!("../../../layers/deploy.layer");
        for leak in [
            "bus_event_listener",
            "bus_command_handler",
            "bus_request_handler",
            "iaaa.UserCreated",
            "SubscriptionManager",
            "@on",
            "@command",
            "@request",
        ] {
            assert!(
                !src.contains(leak),
                "deploy.layer must not name product annotations ({leak})"
            );
        }
        assert!(
            src.contains("layer that declared"),
            "deploy.layer must send the agent to the declaring layer"
        );
    }
}
