//! Product dependencies (R20) — declared in `veil.toml` for hub + cloud resolve.
//!
//! ```toml
//! [dependencies]
//! designkit = { project = "dlx-designkit" }
//! application = { path = "../application" }
//! # future: mylib = { git = "https://…", rev = "main" }
//! ```
//!
//! Resolved roots are added to adapt package search and layer search so
//! `use designkit` works without relying only on ambient sibling discovery.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One product dependency keyed by **use name** (layer / package stem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductDep {
    /// Name used in `use <name>` / `adapt <name>` (map key in toml).
    pub use_name: String,
    /// Hub project directory name under `projects_dir` (e.g. `dlx-designkit`).
    pub project: Option<String>,
    /// Explicit path (absolute or relative to the depending project root).
    pub path: Option<PathBuf>,
    /// Optional git URL (materialized into cache when path/project missing).
    pub git: Option<String>,
    /// Git rev/branch/tag when `git` is set.
    pub rev: Option<String>,
}

/// Flexible toml value for a single dependency entry.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DepToml {
    /// `designkit = "../dlx-designkit"`
    Path(String),
    /// `designkit = { project = "dlx-designkit", path = "…", git = "…", rev = "…" }`
    Table {
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        git: Option<String>,
        #[serde(default)]
        rev: Option<String>,
        /// Optional override of use-name (defaults to table key).
        #[serde(default, rename = "use")]
        use_name: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct VeilTomlFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    package: Option<PackageToml>,
    #[serde(default)]
    dependencies: BTreeMap<String, DepToml>,
    /// Product codegen policy overrides (INV-001). Applied after layers load.
    #[serde(default)]
    codegen: Option<CodegenToml>,
    /// Product harness knobs (INV-001). Applied after layers; same tokens as
    /// layer `harness_policy`. Codegen does not emit from this yet.
    #[serde(default)]
    harness: Option<HarnessToml>,
    /// Infrastructure deployment configuration — links to a Terraform template
    /// project and provides variable overrides.
    #[serde(default)]
    deploy: Option<DeployToml>,
    /// Multi-project repo workspace section (Cargo-workspace shape). A root
    /// `veil.toml` with `[workspace] members = [...]` lists subproject dirs.
    /// Parsing only here (Spec 1 shared contract); Spec 2 consumes it.
    #[serde(default)]
    workspace: Option<WorkspaceToml>,
}

/// `[workspace]` section in a root `veil.toml` for multi-project VEIL repos.
///
/// Shape mirrors a Cargo workspace: a generated convenience list of subproject
/// directories. The authority for each member is its own `veil.toml`; this list
/// is written when a subproject is added and is used by registry crawlers /
/// resolution-point registration to enumerate members.
///
/// A root `veil.toml` that has `[workspace]` and NO `[package]` parses without
/// error and is NOT a compilable project (it declares no packages).
#[derive(Debug, Deserialize, Default)]
struct WorkspaceToml {
    #[serde(default)]
    members: Vec<String>,
}

/// `[codegen]` section in `veil.toml` — product knobs over layer policies.
///
/// Absent keys leave layer policy alone. Empty string or `"none"` clears an
/// optional field (e.g. disable bus strip prefix without forking ddd.layer).
///
/// ```toml
/// [codegen]
/// bus_strip_prefix = "Handle"
/// auth_service_trait = "AuthService"
/// http_path_prefix = "/api/v1/"
/// http_list_prefix = "List"
/// http_get_prefix = "Get"
/// http_create_prefix = "Create"
/// http_update_prefix = "Update"
/// http_delete_prefix = "Delete"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodegenToml {
    #[serde(default)]
    pub bus_strip_prefix: Option<String>,
    #[serde(default)]
    pub auth_service_trait: Option<String>,
    #[serde(default)]
    pub http_path_prefix: Option<String>,
    #[serde(default)]
    pub http_list_prefix: Option<String>,
    #[serde(default)]
    pub http_get_prefix: Option<String>,
    #[serde(default)]
    pub http_create_prefix: Option<String>,
    #[serde(default)]
    pub http_update_prefix: Option<String>,
    #[serde(default)]
    pub http_delete_prefix: Option<String>,
    /// Output type: "bin" (default) or "cdylib" (shared library for `link` consumers).
    /// When "cdylib", codegen adds `[lib]\ncrate-type = ["cdylib"]` and generates
    /// factory functions for each adapter.
    #[serde(default)]
    pub output_type: Option<String>,
}

impl CodegenToml {
    /// True when at least one override key was present in toml.
    pub fn is_empty(&self) -> bool {
        self.bus_strip_prefix.is_none()
            && self.auth_service_trait.is_none()
            && self.http_path_prefix.is_none()
            && self.http_list_prefix.is_none()
            && self.http_get_prefix.is_none()
            && self.http_create_prefix.is_none()
            && self.http_update_prefix.is_none()
            && self.http_delete_prefix.is_none()
            && self.output_type.is_none()
    }

    /// Normalize a optional string field: empty / `-` / `none` → clear (None).
    pub fn normalize_opt(s: &Option<String>) -> Option<Option<String>> {
        match s {
            None => None, // key absent — do not override
            Some(v) => {
                let t = v.trim();
                if t.is_empty() || t == "-" || t.eq_ignore_ascii_case("none") {
                    Some(None) // explicit clear
                } else {
                    Some(Some(t.to_string()))
                }
            }
        }
    }
}

/// Load `[codegen]` from a product root’s `veil.toml` (None if missing/empty).
pub fn load_codegen_overrides(project_root: &Path) -> Result<Option<CodegenToml>, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read {}: {e}", toml_path.display()))?;
    let parsed: VeilTomlFile =
        toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))?;
    Ok(parsed.codegen.filter(|c| !c.is_empty()))
}

/// Walk from a `.veil` path to project root and load `[codegen]` if present.
pub fn load_codegen_overrides_for(veil_path: &Path) -> Option<CodegenToml> {
    let root = find_project_root(veil_path)?;
    load_codegen_overrides(&root).ok().flatten()
}

/// `[harness]` section in `veil.toml` — project knobs over layer `harness_policy`.
///
/// Tokens match `docs/POLICY_ROLES.md` / design §5.3. Absent keys leave layer
/// policy alone. `"none"` / `"-"` / `""` clears optional strings (same as
/// [`CodegenToml::normalize_opt`]).
///
/// ```toml
/// [harness]
/// profile = "axum_http"
/// compat = "auto"
/// cors = "localhost"
/// auth = "api_key"
/// emit_bin = "on_entry"
///
/// [harness.wire]
/// item_repo = "PgItemRepo"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarnessToml {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub listen: Option<String>,
    #[serde(default)]
    pub listen_env: Option<String>,
    #[serde(default)]
    pub listen_default: Option<u16>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub health: Option<String>,
    #[serde(default)]
    pub cors: Option<String>,
    #[serde(default)]
    pub cors_outside_auth: Option<bool>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub collide: Option<String>,
    #[serde(default)]
    pub emit_bin: Option<String>,
    #[serde(default)]
    pub bus_wire: Option<String>,
    #[serde(default)]
    pub bind_defaults: Option<String>,
    #[serde(default)]
    pub delete_extras: Option<String>,
    #[serde(default)]
    pub compat: Option<String>,
    #[serde(default)]
    pub wire: BTreeMap<String, String>,
}

impl HarnessToml {
    pub fn is_empty(&self) -> bool {
        self.profile.is_none()
            && self.bin.is_none()
            && self.listen.is_none()
            && self.listen_env.is_none()
            && self.listen_default.is_none()
            && self.path_prefix.is_none()
            && self.health.is_none()
            && self.cors.is_none()
            && self.cors_outside_auth.is_none()
            && self.auth.is_none()
            && self.collide.is_none()
            && self.emit_bin.is_none()
            && self.bus_wire.is_none()
            && self.bind_defaults.is_none()
            && self.delete_extras.is_none()
            && self.compat.is_none()
            && self.wire.is_empty()
    }

    /// Convert set keys to a [`crate::harness::HarnessPolicy`] overlay.
    ///
    /// Absent toml keys stay `None` (keep layer). `"none"` becomes
    /// [`crate::harness::HARNESS_CLEAR`] so merge actually drops the layer value.
    pub fn to_policy(&self) -> crate::harness::HarnessPolicy {
        use crate::harness::{
            AuthMode, BindDefaults, BusWire, CollideMode, CompatMode, CorsMode, DeleteExtras,
            EmitBin, HarnessPolicy, HARNESS_CLEAR,
        };
        fn overlay_str(src: &Option<String>) -> Option<String> {
            match CodegenToml::normalize_opt(src) {
                None => None,
                Some(None) => Some(HARNESS_CLEAR.to_string()),
                Some(Some(v)) => Some(v),
            }
        }
        HarnessPolicy {
            profile: overlay_str(&self.profile),
            bin: overlay_str(&self.bin),
            listen_env: overlay_str(&self.listen_env),
            listen_default: self.listen_default,
            listen: overlay_str(&self.listen),
            health: overlay_str(&self.health),
            cors: self.cors.as_deref().and_then(CorsMode::parse),
            cors_outside_auth: self.cors_outside_auth,
            auth: self.auth.as_deref().and_then(AuthMode::parse),
            emit_bin: self.emit_bin.as_deref().and_then(EmitBin::parse),
            bus_wire: self.bus_wire.as_deref().and_then(BusWire::parse),
            collide: self.collide.as_deref().and_then(CollideMode::parse),
            bind_defaults: self.bind_defaults.as_deref().and_then(BindDefaults::parse),
            delete_extras: self.delete_extras.as_deref().and_then(DeleteExtras::parse),
            compat: self.compat.as_deref().and_then(CompatMode::parse),
            path_prefix: overlay_str(&self.path_prefix),
            provided_runtime_traits: Vec::new(),
            wire: self.wire.clone(),
        }
    }
}

/// Load `[harness]` from a product root’s `veil.toml` (None if missing/empty).
pub fn load_harness_overrides(project_root: &Path) -> Result<Option<HarnessToml>, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read {}: {e}", toml_path.display()))?;
    let parsed: VeilTomlFile =
        toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))?;
    Ok(parsed.harness.filter(|h| !h.is_empty()))
}

/// Walk from a `.veil` path to project root and load `[harness]` if present.
pub fn load_harness_overrides_for(veil_path: &Path) -> Option<HarnessToml> {
    let root = find_project_root(veil_path)?;
    load_harness_overrides(&root).ok().flatten()
}

// ─── Deploy Configuration ────────────────────────────────────────────────────

/// `[deploy]` section in `veil.toml` — infrastructure template reference + variable overrides.
///
/// ```toml
/// [deploy]
/// template = "dlx-service-template"    # project slug of the infra template
///
/// [deploy.uses.ecs_cluster]
/// project = "veil-ecs-cluster"
///
/// [deploy.uses.dlx_bus]
/// project = "dlx-bus"
/// vars.ecs_cluster_arn = "{{ecs_cluster.outputs.cluster_arn}}"
///
/// [deploy.vars]
/// service_name = "{{slug}}"            # interpolated at deploy time
/// environment = "{{env}}"
/// enable_daemon = true
///
/// [deploy.suppress]
/// sns = true
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeployToml {
    /// Project slug of the Terraform template to use.
    #[serde(default)]
    pub template: Option<String>,
    /// Infrastructure dependencies on other projects. Each key is an alias used
    /// in `{{alias.outputs.output_name}}` references.
    #[serde(default)]
    pub uses: BTreeMap<String, DeployUsesToml>,
    /// Variable overrides passed to terraform. Values can be strings (with
    /// `{{token}}` or `{{alias.outputs.name}}` interpolation), booleans, or numbers.
    #[serde(default)]
    pub vars: BTreeMap<String, toml::Value>,
    /// Suppress inherited infra resources (e.g. `sns = true` to skip SNS topic).
    #[serde(default)]
    pub suppress: BTreeMap<String, bool>,
}

/// One `[deploy.uses.<alias>]` entry — an infrastructure dependency on another project.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeployUsesToml {
    /// Project slug that provides the infrastructure (e.g. "veil-ecs-cluster").
    pub project: String,
    /// Variable overrides to pass when applying that project's shared terraform.
    /// These may reference outputs from OTHER uses entries: `{{other_alias.outputs.x}}`.
    #[serde(default)]
    pub vars: BTreeMap<String, toml::Value>,
}

impl DeployToml {
    /// True when at least one meaningful key is present.
    pub fn is_empty(&self) -> bool {
        self.template.is_none()
            && self.uses.is_empty()
            && self.vars.is_empty()
            && self.suppress.is_empty()
    }

    /// Render vars with interpolation context and resolved outputs.
    ///
    /// `outputs` maps `alias → output_name → value` (from prior terraform applies).
    /// Falls back to `""` for unresolved `{{alias.outputs.x}}` references.
    pub fn render_vars(
        &self,
        ctx: &DeployContext,
        outputs: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (k, v) in &self.vars {
            let rendered = match v {
                toml::Value::String(s) => render_deploy_value(s, ctx, outputs),
                toml::Value::Boolean(b) => b.to_string(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Float(f) => f.to_string(),
                _ => v.to_string(),
            };
            out.insert(k.clone(), rendered);
        }
        out
    }

    /// Extract which `uses` aliases this deploy config depends on (by scanning
    /// `{{alias.outputs.x}}` references in vars).
    pub fn output_dependencies(&self) -> Vec<String> {
        let mut deps = Vec::new();
        for v in self.vars.values() {
            if let toml::Value::String(s) = v {
                collect_output_refs(s, &mut deps);
            }
        }
        // Also scan uses.*.vars for inter-uses dependencies.
        for entry in self.uses.values() {
            for v in entry.vars.values() {
                if let toml::Value::String(s) = v {
                    collect_output_refs(s, &mut deps);
                }
            }
        }
        deps.sort();
        deps.dedup();
        deps
    }
}

impl DeployUsesToml {
    /// Extract which other `uses` aliases this entry depends on (by scanning
    /// `{{alias.outputs.x}}` references in its vars).
    pub fn output_dependencies(&self) -> Vec<String> {
        let mut deps = Vec::new();
        for v in self.vars.values() {
            if let toml::Value::String(s) = v {
                collect_output_refs(s, &mut deps);
            }
        }
        deps.sort();
        deps.dedup();
        deps
    }
}

/// Context available for `{{token}}` interpolation in deploy vars.
#[derive(Debug, Clone, Default)]
pub struct DeployContext {
    /// Project slug (e.g. "agent-core").
    pub slug: String,
    /// Deploy environment (e.g. "dev", "staging", "prod").
    pub env: String,
    /// AWS region (e.g. "us-west-2").
    pub region: String,
    /// AWS account ID (e.g. "086261225885").
    pub account_id: String,
    /// Runtime S3 bucket name.
    pub bucket: String,
    /// Runtime DynamoDB table name.
    pub table: String,
}

/// Render a deploy value string with context interpolation AND output references.
///
/// Handles both simple tokens (`{{slug}}`) and output references (`{{alias.outputs.name}}`).
pub fn render_deploy_value(
    template: &str,
    ctx: &DeployContext,
    outputs: &BTreeMap<String, BTreeMap<String, String>>,
) -> String {
    // First pass: resolve output references (they may contain dots that look like tokens).
    let mut result = resolve_output_refs(template, outputs);
    // Second pass: resolve simple context tokens.
    result = result.replace("{{slug}}", &ctx.slug);
    result = result.replace("{{env}}", &ctx.env);
    result = result.replace("{{region}}", &ctx.region);
    result = result.replace("{{account_id}}", &ctx.account_id);
    result = result.replace("{{bucket}}", &ctx.bucket);
    result = result.replace("{{table}}", &ctx.table);
    result
}

/// Interpolate simple `{{token}}` patterns (no output refs). Legacy compat.
pub fn interpolate_deploy_var(template: &str, ctx: &DeployContext) -> String {
    render_deploy_value(template, ctx, &BTreeMap::new())
}

/// Resolve `{{alias.outputs.output_name}}` patterns from a resolved outputs map.
fn resolve_output_refs(
    template: &str,
    outputs: &BTreeMap<String, BTreeMap<String, String>>,
) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let token = &after_open[..end];
            // Check if this is an output reference: alias.outputs.name
            if let Some(resolved) = try_resolve_output_ref(token, outputs) {
                result.push_str(&resolved);
            } else {
                // Not an output ref — leave it for the context pass.
                result.push_str("{{");
                result.push_str(token);
                result.push_str("}}");
            }
            rest = &after_open[end + 2..];
        } else {
            // No closing }} — pass through literally.
            result.push_str("{{");
            rest = after_open;
        }
    }
    result.push_str(rest);
    result
}

/// Try to resolve a single token as an output reference (`alias.outputs.name`).
/// Returns None if it's not an output reference pattern.
fn try_resolve_output_ref(
    token: &str,
    outputs: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() == 3 && parts[1] == "outputs" {
        let alias = parts[0];
        let output_name = parts[2];
        let value = outputs
            .get(alias)
            .and_then(|m| m.get(output_name))
            .cloned()
            .unwrap_or_default();
        Some(value)
    } else {
        None
    }
}

/// Collect alias names from `{{alias.outputs.x}}` references in a string.
fn collect_output_refs(s: &str, out: &mut Vec<String>) {
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let token = &after_open[..end];
            let parts: Vec<&str> = token.splitn(3, '.').collect();
            if parts.len() == 3 && parts[1] == "outputs" {
                let alias = parts[0].to_string();
                if !out.contains(&alias) {
                    out.push(alias);
                }
            }
            rest = &after_open[end + 2..];
        } else {
            break;
        }
    }
}

// ─── Deploy DAG ──────────────────────────────────────────────────────────────

/// A node in the deploy DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployNode {
    /// Alias name (key in `[deploy.uses]`).
    pub alias: String,
    /// Project slug being deployed.
    pub project: String,
    /// Aliases this node depends on (from `{{alias.outputs.x}}` references).
    pub depends_on: Vec<String>,
}

/// Error from DAG construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployDagError {
    /// Circular dependency detected. Contains the cycle path.
    CyclicDependency(Vec<String>),
    /// A `{{alias.outputs.x}}` reference points to an alias not declared in `[deploy.uses]`.
    UnknownAlias { reference: String, available: Vec<String> },
}

impl std::fmt::Display for DeployDagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployDagError::CyclicDependency(path) => {
                write!(
                    f,
                    "circular infrastructure dependency detected: {}",
                    path.join(" → ")
                )
            }
            DeployDagError::UnknownAlias { reference, available } => {
                write!(
                    f,
                    "unknown deploy alias '{}' referenced in outputs; available: [{}]",
                    reference,
                    available.join(", ")
                )
            }
        }
    }
}

/// Build a deploy DAG from the `[deploy.uses]` entries. Returns nodes in
/// topological order (dependencies first). Errors on cycles or unknown aliases.
pub fn build_deploy_dag(deploy: &DeployToml) -> Result<Vec<DeployNode>, DeployDagError> {
    if deploy.uses.is_empty() {
        return Ok(Vec::new());
    }

    let aliases: Vec<String> = deploy.uses.keys().cloned().collect();

    // Build nodes with their dependencies.
    let mut nodes: BTreeMap<String, DeployNode> = BTreeMap::new();
    for (alias, entry) in &deploy.uses {
        let deps = entry.output_dependencies();
        // Validate that all referenced aliases exist.
        for dep in &deps {
            if !aliases.contains(dep) {
                return Err(DeployDagError::UnknownAlias {
                    reference: dep.clone(),
                    available: aliases.clone(),
                });
            }
        }
        nodes.insert(
            alias.clone(),
            DeployNode {
                alias: alias.clone(),
                project: entry.project.clone(),
                depends_on: deps,
            },
        );
    }

    // Also check top-level vars for output refs that point to uses entries.
    // These don't create extra nodes, but validate alias references.
    for v in deploy.vars.values() {
        if let toml::Value::String(s) = v {
            let mut refs = Vec::new();
            collect_output_refs(s, &mut refs);
            for r in &refs {
                if !aliases.contains(r) {
                    return Err(DeployDagError::UnknownAlias {
                        reference: r.clone(),
                        available: aliases.clone(),
                    });
                }
            }
        }
    }

    // Topological sort with cycle detection (Kahn's algorithm).
    topological_sort(&nodes)
}

/// Topological sort via Kahn's algorithm. Returns nodes in dependency order.
fn topological_sort(
    nodes: &BTreeMap<String, DeployNode>,
) -> Result<Vec<DeployNode>, DeployDagError> {
    let mut in_deg: BTreeMap<String, usize> = nodes.keys().map(|k| (k.clone(), 0)).collect();
    for node in nodes.values() {
        for dep in &node.depends_on {
            if nodes.contains_key(dep) {
                *in_deg.entry(node.alias.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for (alias, &deg) in &in_deg {
        if deg == 0 {
            queue.push_back(alias.clone());
        }
    }

    let mut sorted = Vec::new();
    while let Some(alias) = queue.pop_front() {
        sorted.push(nodes[&alias].clone());
        // For each node that depends on `alias`, reduce its in-degree.
        for (other_alias, node) in nodes {
            if node.depends_on.contains(&alias) {
                let deg = in_deg.get_mut(other_alias).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(other_alias.clone());
                }
            }
        }
    }

    if sorted.len() != nodes.len() {
        // Cycle detected — find the cycle for error reporting.
        let remaining: Vec<String> = nodes
            .keys()
            .filter(|k| !sorted.iter().any(|n| &n.alias == *k))
            .cloned()
            .collect();
        let cycle = find_cycle(nodes, &remaining);
        return Err(DeployDagError::CyclicDependency(cycle));
    }

    Ok(sorted)
}

/// Find a cycle in the remaining (unsorted) nodes for error reporting.
fn find_cycle(nodes: &BTreeMap<String, DeployNode>, remaining: &[String]) -> Vec<String> {
    if remaining.is_empty() {
        return Vec::new();
    }
    // DFS from the first remaining node to find the cycle.
    let start = &remaining[0];
    let mut path = vec![start.clone()];
    let mut visited = std::collections::BTreeSet::new();
    visited.insert(start.clone());

    let mut current = start.clone();
    loop {
        let node = match nodes.get(&current) {
            Some(n) => n,
            None => break,
        };
        let next = node
            .depends_on
            .iter()
            .find(|d| remaining.contains(d));
        match next {
            Some(n) => {
                if visited.contains(n) {
                    path.push(n.clone());
                    break;
                }
                visited.insert(n.clone());
                path.push(n.clone());
                current = n.clone();
            }
            None => break,
        }
    }
    path
}

/// Load `[deploy]` from a product root's `veil.toml` (None if missing/empty).
pub fn load_deploy_config(project_root: &Path) -> Result<Option<DeployToml>, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read {}: {e}", toml_path.display()))?;
    let parsed: VeilTomlFile =
        toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))?;
    Ok(parsed.deploy.filter(|d| !d.is_empty()))
}

/// Walk from a `.veil` path to project root and load `[deploy]` if present.
pub fn load_deploy_config_for(veil_path: &Path) -> Option<DeployToml> {
    let root = find_project_root(veil_path)?;
    load_deploy_config(&root).ok().flatten()
}

/// `[package]` entry in veil.toml (R21).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PackageToml {
    /// Language/`use` name (defaults to top-level `name`).
    #[serde(default)]
    pub name: Option<String>,
    /// Primary package source relative to project root (default: `main.veil` if present).
    #[serde(default)]
    pub veil: Option<String>,
    /// Primary layer relative to project root (default: `layers/main.layer` if present).
    #[serde(default)]
    pub layer: Option<String>,
}

/// Resolved primary package entry for a product root (R21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    /// Project directory name / top-level `name` in veil.toml.
    pub project_name: Option<String>,
    /// `use` / language name (`package.name` or project name).
    pub use_name: String,
    /// Relative path to primary .veil (e.g. `main.veil`).
    pub veil: PathBuf,
    /// Relative path to primary .layer (e.g. `layers/main.layer`).
    pub layer: PathBuf,
}

impl PackageEntry {
    pub fn provides_use(&self, name: &str) -> bool {
        self.use_name == name
            || self
                .project_name
                .as_ref()
                .map(|p| p == name)
                .unwrap_or(false)
    }

    pub fn veil_abs(&self, root: &Path) -> PathBuf {
        root.join(&self.veil)
    }

    pub fn layer_abs(&self, root: &Path) -> PathBuf {
        root.join(&self.layer)
    }
}

/// Load `[package]` + defaults for a product root. Returns None if no veil.toml.
pub fn load_package_entry(project_root: &Path) -> Option<PackageEntry> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(&toml_path).ok()?;
    let parsed: VeilTomlFile = toml::from_str(&content).ok()?;
    let project_name = parsed.name.clone();
    let pkg = parsed.package.unwrap_or_default();
    let use_name = pkg
        .name
        .or_else(|| project_name.clone())
        .unwrap_or_else(|| {
            project_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("app")
                .to_string()
        });

    let veil = if let Some(v) = pkg.veil {
        PathBuf::from(v)
    } else if project_root.join("main.veil").is_file() {
        PathBuf::from("main.veil")
    } else if project_root.join(format!("{use_name}.veil")).is_file() {
        PathBuf::from(format!("{use_name}.veil"))
    } else {
        // Prefer main.veil as the written convention even if not yet created
        PathBuf::from("main.veil")
    };

    let layer = if let Some(l) = pkg.layer {
        PathBuf::from(l)
    } else if project_root.join("layers/main.layer").is_file() {
        PathBuf::from("layers/main.layer")
    } else if project_root.join(format!("layers/{use_name}.layer")).is_file() {
        PathBuf::from(format!("layers/{use_name}.layer"))
    } else if project_root.join("main.layer").is_file() {
        PathBuf::from("main.layer")
    } else {
        PathBuf::from("layers/main.layer")
    };

    Some(PackageEntry {
        project_name,
        use_name,
        veil,
        layer,
    })
}

/// Whether this product root provides the given `use` name (entry or legacy files).
pub fn product_provides_use(root: &Path, use_name: &str) -> bool {
    if let Some(entry) = load_package_entry(root) {
        if entry.provides_use(use_name) {
            return true;
        }
    }
    // Legacy: folder or file stem matches
    root.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == use_name)
        .unwrap_or(false)
        || root.join(format!("{use_name}.veil")).is_file()
        || root.join("layers").join(format!("{use_name}.layer")).is_file()
}

/// Convert PascalCase to snake_case (e.g. "DlxAuth" → "dlx_auth").
fn pascal_to_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Absolute path to primary package source for `use_name` inside a product root.
pub fn package_source_in_root(root: &Path, use_name: &str) -> Option<PathBuf> {
    if let Some(entry) = load_package_entry(root) {
        if entry.provides_use(use_name) {
            let p = entry.veil_abs(root);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // main.veil when package declares this use name via pkg line (peek)
    let main = root.join("main.veil");
    if main.is_file() {
        if let Some(pkg_name) = package_file_use_name(&main) {
            // Case-insensitive + snake_case normalization (use dlx_auth matches pkg DlxAuth)
            let norm_use = use_name.to_lowercase().replace('-', "_");
            let norm_pkg = pkg_name.to_lowercase().replace('-', "_");
            // Also try converting PascalCase to snake_case for pkg name
            let norm_pkg_snake = pascal_to_snake(&pkg_name);
            if norm_pkg == norm_use || norm_pkg_snake == norm_use || pkg_name == use_name {
                return Some(main);
            }
        }
    }
    let legacy = root.join(format!("{use_name}.veil"));
    if legacy.is_file() {
        return Some(legacy);
    }
    None
}

/// Absolute path to primary layer for `use_name` inside a product root.
///
/// Never returns another product’s `main.layer` for a different use name
/// (that caused infinite load_layer recursion when `use ddd` resolved to a
/// product main.layer that itself `use ddd`).
pub fn layer_source_in_root(root: &Path, use_name: &str) -> Option<PathBuf> {
    if let Some(entry) = load_package_entry(root) {
        if entry.provides_use(use_name) {
            let p = entry.layer_abs(root);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // Named layer only — do not fall back to main.layer unless entry matched above
    for rel in [
        format!("layers/{use_name}.layer"),
        format!("{use_name}.layer"),
    ] {
        let p = root.join(&rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Peek `pkg Name` from a .veil file.
fn package_file_use_name(veil_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(veil_path).ok()?;
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("pkg ") {
            let name = rest.split_whitespace().next()?.to_string();
            return Some(name);
        }
    }
    None
}

/// Walk parents of `start` looking for a directory that contains `veil.toml`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent().map(|p| p.to_path_buf())
    } else {
        Some(start.to_path_buf())
    };
    while let Some(dir) = cur {
        if dir.join("veil.toml").is_file() {
            return Some(dir);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Substring every `missing veil.toml` diagnostic MUST contain (tests assert this).
pub const MISSING_VEIL_TOML: &str = "missing veil.toml";

/// Enforce that a `.veil` leaf (or directory) belongs to a project root that
/// has a `veil.toml` (Decision 3, `decision-registry-repo-structure`).
///
/// Returns the resolved project root on success. On failure returns a
/// diagnostic string containing the [`MISSING_VEIL_TOML`] substring and the
/// offending absolute path. Never panics — Law 11 (no silent miscompile) and
/// Law 7 (diagnostics outrank terseness).
///
/// Enforcement fires whenever a `.veil` has no ancestor `veil.toml`: previously
/// tolerated, now a hard error. Bare single-file parser fixtures must be given a
/// `veil.toml` (or a `[workspace]` root) rather than weakening this check.
pub fn require_project_root(leaf_path: &Path) -> Result<PathBuf, String> {
    if let Some(root) = find_project_root(leaf_path) {
        return Ok(root);
    }
    // No ancestor veil.toml — surface the directory that should hold one.
    let offending = if leaf_path.is_dir() {
        leaf_path.to_path_buf()
    } else {
        leaf_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| leaf_path.to_path_buf())
    };
    let abs = offending
        .canonicalize()
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(&offending))
                .unwrap_or(offending.clone())
        });
    Err(format!(
        "{MISSING_VEIL_TOML}: {} has no veil.toml (a VEIL project root must contain one)",
        abs.display()
    ))
}

/// Read `[workspace] members` from a root `veil.toml` (empty if absent or no
/// `[workspace]`).
///
/// Members are subdir paths relative to the workspace root, normalized: no
/// leading `./`, no trailing slash, backslashes folded to `/`. Entries with
/// `..`/`.` traversal segments are REJECTED (dropped) so a `members` list can
/// never point outside the workspace — same safety posture as
/// `git_origin::normalize_subpath` (Spec 2, `decision-registry-repo-structure`).
pub fn load_workspace_members(project_root: &Path) -> Vec<String> {
    let toml_path = project_root.join("veil.toml");
    let Ok(content) = std::fs::read_to_string(&toml_path) else {
        return Vec::new();
    };
    match toml::from_str::<VeilTomlFile>(&content) {
        Ok(parsed) => parsed
            .workspace
            .map(|w| {
                w.members
                    .iter()
                    .filter_map(|m| normalize_member(m))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// True if `path/veil.toml` has a `[workspace]` section (i.e. `path` is a
/// multi-project VEIL repo root). A missing or unparseable `veil.toml` is false.
pub fn is_workspace_root(path: &Path) -> bool {
    let toml_path = path.join("veil.toml");
    let Ok(content) = std::fs::read_to_string(&toml_path) else {
        return false;
    };
    matches!(
        toml::from_str::<VeilTomlFile>(&content),
        Ok(parsed) if parsed.workspace.is_some()
    )
}

/// Normalize a workspace `members` entry: trimmed, backslashes → `/`, no
/// leading/trailing slashes, `None` if empty. Rejects `..`/`.` traversal
/// segments (returns `None`) so a member can never escape the workspace root.
/// Mirrors `git_origin::normalize_subpath` (single safety posture).
pub fn normalize_member(raw: &str) -> Option<String> {
    let s = raw.trim().replace('\\', "/");
    let s = s.trim_matches('/');
    if s.is_empty() {
        return None;
    }
    if s.split('/').any(|seg| seg == ".." || seg == ".") {
        return None;
    }
    Some(s.to_string())
}

/// Projects hub directory for resolving `{ project = "…" }` deps.
///
/// Order: `VEIL_PROJECTS_DIR` → parent of project root (hub) → project root.
/// Canonicalizes `project_root` when possible so relative paths like
/// `main.veil` (parent `""`) still resolve hub as the parent of the real
/// product directory under CWD.
pub fn projects_hub(project_root: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("VEIL_PROJECTS_DIR") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| {
            // Relative / empty path: resolve against CWD
            if project_root.as_os_str().is_empty() || project_root == Path::new(".") {
                std::env::current_dir().unwrap_or_else(|_| project_root.to_path_buf())
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(project_root))
                    .ok()
                    .and_then(|p| p.canonicalize().ok())
                    .unwrap_or_else(|| project_root.to_path_buf())
            }
        });
    root.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(root)
}

/// Cache dir for git-materialized deps: `$hub/.veil-deps/`.
pub fn deps_cache_dir(hub: &Path) -> PathBuf {
    hub.join(".veil-deps")
}

/// Parse `[dependencies]` from a project’s `veil.toml` (empty if missing).
pub fn load_product_deps(project_root: &Path) -> Result<Vec<ProductDep>, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read {}: {e}", toml_path.display()))?;
    let parsed: VeilTomlFile =
        toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))?;
    let mut out = Vec::new();
    for (key, val) in parsed.dependencies {
        let dep = match val {
            DepToml::Path(p) => ProductDep {
                use_name: key,
                project: None,
                path: Some(PathBuf::from(p)),
                git: None,
                rev: None,
            },
            DepToml::Table {
                project,
                path,
                git,
                rev,
                use_name,
            } => ProductDep {
                use_name: use_name.unwrap_or(key),
                project,
                path: path.map(PathBuf::from),
                git,
                rev,
            },
        };
        out.push(dep);
    }
    out.sort_by(|a, b| a.use_name.cmp(&b.use_name));
    Ok(out)
}

fn looks_like_product_root(dir: &Path) -> bool {
    dir.join("veil.toml").is_file()
        || dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| {
                // application/main.veil or designkit.veil at root of dlx-designkit
                dir.join(format!("{name}.veil")).is_file()
            })
            .unwrap_or(false)
        || dir
            .read_dir()
            .ok()
            .map(|rd| {
                rd.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        == Some("veil")
                })
            })
            .unwrap_or(false)
}

/// Resolve one dep to an on-disk product root (materialize git if needed).
pub fn resolve_dep_root(
    project_root: &Path,
    dep: &ProductDep,
    hub: &Path,
) -> Result<PathBuf, String> {
    // 1. Explicit path
    if let Some(ref p) = dep.path {
        let resolved = if p.is_absolute() {
            p.clone()
        } else {
            project_root.join(p)
        };
        let resolved = resolved
            .canonicalize()
            .unwrap_or(resolved);
        if looks_like_product_root(&resolved) || resolved.is_dir() {
            return Ok(resolved);
        }
        return Err(format!(
            "dependency '{}': path {} not found or not a VEIL product",
            dep.use_name,
            resolved.display()
        ));
    }

    // 2. Hub project id
    if let Some(ref proj) = dep.project {
        let candidate = hub.join(proj);
        if looks_like_product_root(&candidate) || candidate.is_dir() {
            return Ok(candidate
                .canonicalize()
                .unwrap_or(candidate));
        }
        // Fall through to git if also specified
        if dep.git.is_none() {
            return Err(format!(
                "dependency '{}': project '{}' not found under hub {} — \
                 clone it there or set path = \"…\" in veil.toml [dependencies]",
                dep.use_name,
                proj,
                hub.display()
            ));
        }
    }

    // 3. Git materialize
    if let Some(ref url) = dep.git {
        return materialize_git_dep(dep, url, hub);
    }

    Err(format!(
        "dependency '{}': need path, project, or git in veil.toml [dependencies]",
        dep.use_name
    ))
}

fn materialize_git_dep(dep: &ProductDep, url: &str, hub: &Path) -> Result<PathBuf, String> {
    let cache = deps_cache_dir(hub);
    std::fs::create_dir_all(&cache)
        .map_err(|e| format!("cannot create deps cache {}: {e}", cache.display()))?;
    let dest = cache.join(&dep.use_name);
    if looks_like_product_root(&dest) {
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    if dest.exists() {
        // Incomplete prior clone — remove and retry
        let _ = std::fs::remove_dir_all(&dest);
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("clone");
    if let Some(ref rev) = dep.rev {
        cmd.args(["--branch", rev]);
    }
    cmd.args(["--depth", "1", url]);
    cmd.arg(&dest);
    let out = cmd
        .output()
        .map_err(|e| format!("dependency '{}': git clone failed to start: {e}", dep.use_name))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "dependency '{}': git clone {} failed: {stderr}",
            dep.use_name, url
        ));
    }
    Ok(dest.canonicalize().unwrap_or(dest))
}

/// Resolve all declared deps to product roots (errors are collected).
pub fn resolve_dependency_roots(project_root: &Path) -> Result<Vec<PathBuf>, String> {
    let deps = load_product_deps(project_root)?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let hub = projects_hub(project_root);
    let mut roots = Vec::new();
    let mut errors = Vec::new();
    for dep in &deps {
        match resolve_dep_root(project_root, dep, &hub) {
            Ok(r) => {
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() && roots.is_empty() {
        return Err(errors.join("; "));
    }
    // Soft-warn partial failures via stderr (gen continues with what resolved)
    for e in errors {
        eprintln!("veil: {e}");
    }
    Ok(roots)
}

/// Transitive product-dep graph, **dependencies first**, `project_root` last.
/// Cycles are skipped (the already-visiting node is not re-entered).
pub fn resolve_dependency_graph(project_root: &Path) -> Result<Vec<PathBuf>, String> {
    let hub = projects_hub(project_root);
    let mut seen: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    let mut visiting: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    walk_dep_graph(project_root, &hub, &mut seen, &mut visiting, &mut out)?;
    Ok(out)
}

fn walk_dep_graph(
    root: &Path,
    hub: &Path,
    seen: &mut std::collections::BTreeSet<PathBuf>,
    visiting: &mut std::collections::BTreeSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if seen.contains(&canon) {
        return Ok(());
    }
    if !visiting.insert(canon.clone()) {
        return Ok(()); // cycle
    }
    let deps = load_product_deps(root).unwrap_or_default();
    for dep in &deps {
        match resolve_dep_root(root, dep, hub) {
            Ok(child) => walk_dep_graph(&child, hub, seen, visiting, out)?,
            Err(e) => eprintln!("veil: {e}"),
        }
    }
    visiting.remove(&canon);
    seen.insert(canon.clone());
    if !out.iter().any(|p| p == &canon) {
        out.push(canon);
    }
    Ok(())
}

/// Resolve dependency roots for any path under a product (file or dir).
pub fn resolve_dependency_roots_for(path: &Path) -> Vec<PathBuf> {
    let Some(root) = find_project_root(path) else {
        return Vec::new();
    };
    match resolve_dependency_roots(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("veil: dependencies: {e}");
            Vec::new()
        }
    }
}

/// Adapt package search paths: defaults + declared product deps.
pub fn adapt_search_paths_for_file(leaf_path: &Path) -> Vec<PathBuf> {
    let extra = resolve_dependency_roots_for(leaf_path);
    crate::adapt::default_adapt_search_paths(leaf_path, &extra)
}

/// Human-readable hint when a use/adapt target is missing.
pub fn missing_package_hint(use_name: &str, project_root: Option<&Path>) -> String {
    let mut msg = format!(
        "package '{use_name}' not found for use/adapt.\n\
         Searched: project dir, hub siblings, and [dependencies] roots."
    );
    if let Some(root) = project_root {
        let hub = projects_hub(root);
        msg.push_str(&format!(
            "\n\nDeclare it in {}:\n\n\
             [dependencies]\n\
             {use_name} = {{ project = \"{use_name}\" }}\n\
             # or: {use_name} = {{ path = \"../other-product\" }}\n\
             # or: {use_name} = {{ git = \"https://…\", rev = \"main\" }}\n\n\
             Hub for project= is {} (VEIL_PROJECTS_DIR or parent of project).",
            root.join("veil.toml").display(),
            hub.display()
        ));
    } else {
        msg.push_str(&format!(
            "\n\nAdd to veil.toml:\n[dependencies]\n{use_name} = {{ project = \"…\" }}"
        ));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_path_and_project_deps() {
        let dir = tempfile_dir();
        let mut f = std::fs::File::create(dir.join("veil.toml")).unwrap();
        writeln!(
            f,
            r#"
name = "app"
[dependencies]
designkit = {{ project = "dlx-designkit" }}
application = "../application"
mylib = {{ path = "/tmp/mylib", use = "lib" }}
"#
        )
        .unwrap();
        let deps = load_product_deps(&dir).unwrap();
        assert_eq!(deps.len(), 3);
        let dk = deps.iter().find(|d| d.use_name == "designkit").unwrap();
        assert_eq!(dk.project.as_deref(), Some("dlx-designkit"));
        let eng = deps.iter().find(|d| d.use_name == "application").unwrap();
        assert_eq!(
            eng.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            Some("../application".into())
        );
        let lib = deps.iter().find(|d| d.use_name == "lib").unwrap();
        assert!(lib.path.is_some());
    }

    #[test]
    fn parse_codegen_overrides() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "app"
[codegen]
bus_strip_prefix = "Cmd"
auth_service_trait = "AuthService"
http_path_prefix = "/api/v1/"
http_list_prefix = "List"
"#,
        )
        .unwrap();
        let o = load_codegen_overrides(&dir).unwrap().expect("codegen");
        assert_eq!(o.bus_strip_prefix.as_deref(), Some("Cmd"));
        assert_eq!(o.auth_service_trait.as_deref(), Some("AuthService"));
        assert_eq!(o.http_path_prefix.as_deref(), Some("/api/v1/"));
        assert_eq!(o.http_list_prefix.as_deref(), Some("List"));
        assert!(o.http_get_prefix.is_none());
    }

    #[test]
    fn parse_harness_overrides() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "app"
[harness]
profile = "axum_rpc"
cors = "localhost"
auth = "api_key"
emit_bin = "never"
health = "none"
compat = "off"

[harness.wire]
item_repo = "PgItemRepo"
"#,
        )
        .unwrap();
        let o = load_harness_overrides(&dir).unwrap().expect("harness");
        assert_eq!(o.profile.as_deref(), Some("axum_rpc"));
        assert_eq!(o.cors.as_deref(), Some("localhost"));
        assert_eq!(o.emit_bin.as_deref(), Some("never"));
        assert_eq!(o.health.as_deref(), Some("none"));
        assert_eq!(o.compat.as_deref(), Some("off"));
        assert_eq!(o.wire.get("item_repo").map(String::as_str), Some("PgItemRepo"));
        let pol = o.to_policy();
        assert_eq!(pol.emit_bin, Some(crate::harness::EmitBin::Never));
        assert_eq!(pol.health.as_deref(), Some(crate::harness::HARNESS_CLEAR));
    }

    #[test]
    fn parse_deploy_config() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "agent-core"

[deploy]
template = "dlx-service-template"

[deploy.vars]
service_name = "{{slug}}"
environment = "{{env}}"
enable_daemon = true
enable_daemon_queue = true
api_memory = 256
api_timeout = 30
consumer_timeout = 900

[deploy.suppress]
sns = true
"#,
        )
        .unwrap();
        let deploy = load_deploy_config(&dir).unwrap().expect("deploy");
        assert_eq!(deploy.template.as_deref(), Some("dlx-service-template"));
        assert_eq!(deploy.vars.len(), 7);
        assert_eq!(
            deploy.vars.get("service_name").and_then(|v| v.as_str()),
            Some("{{slug}}")
        );
        assert_eq!(
            deploy.vars.get("enable_daemon").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            deploy.vars.get("api_memory").and_then(|v| v.as_integer()),
            Some(256)
        );
        assert_eq!(deploy.suppress.get("sns"), Some(&true));
    }

    #[test]
    fn deploy_variable_interpolation() {
        let ctx = DeployContext {
            slug: "agent-core".into(),
            env: "dev".into(),
            region: "us-west-2".into(),
            account_id: "123456789012".into(),
            bucket: "veil-runtime-dev".into(),
            table: "veil-runtime-dev".into(),
        };

        assert_eq!(interpolate_deploy_var("{{slug}}", &ctx), "agent-core");
        assert_eq!(interpolate_deploy_var("{{env}}", &ctx), "dev");
        assert_eq!(
            interpolate_deploy_var("veil-{{slug}}-{{env}}", &ctx),
            "veil-agent-core-dev"
        );
        assert_eq!(
            interpolate_deploy_var("no tokens here", &ctx),
            "no tokens here"
        );
        assert_eq!(
            interpolate_deploy_var("arn:aws:lambda:{{region}}:{{account_id}}:function:test", &ctx),
            "arn:aws:lambda:us-west-2:123456789012:function:test"
        );
    }

    #[test]
    fn deploy_render_vars_mixed_types() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "myapp"

[deploy]
template = "dlx-service-template"

[deploy.vars]
service_name = "{{slug}}"
environment = "{{env}}"
enable_daemon = true
api_memory = 512
"#,
        )
        .unwrap();
        let deploy = load_deploy_config(&dir).unwrap().expect("deploy");
        let ctx = DeployContext {
            slug: "myapp".into(),
            env: "prod".into(),
            region: "us-east-1".into(),
            ..Default::default()
        };
        let rendered = deploy.render_vars(&ctx, &BTreeMap::new());
        assert_eq!(rendered.get("service_name").unwrap(), "myapp");
        assert_eq!(rendered.get("environment").unwrap(), "prod");
        assert_eq!(rendered.get("enable_daemon").unwrap(), "true");
        assert_eq!(rendered.get("api_memory").unwrap(), "512");
    }

    #[test]
    fn parse_deploy_uses() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "agent-core"

[deploy]
template = "dlx-service-template"

[deploy.uses.ecs_cluster]
project = "veil-ecs-cluster"

[deploy.uses.dlx_bus]
project = "dlx-bus"
vars.ecs_cluster_arn = "{{ecs_cluster.outputs.cluster_arn}}"
vars.api_gateway_id = "{{ecs_cluster.outputs.api_gateway_id}}"

[deploy.vars]
service_name = "{{slug}}"
environment = "{{env}}"
enable_daemon = true
"#,
        )
        .unwrap();
        let deploy = load_deploy_config(&dir).unwrap().expect("deploy");
        assert_eq!(deploy.uses.len(), 2);
        let ecs = deploy.uses.get("ecs_cluster").unwrap();
        assert_eq!(ecs.project, "veil-ecs-cluster");
        assert!(ecs.vars.is_empty());
        let bus = deploy.uses.get("dlx_bus").unwrap();
        assert_eq!(bus.project, "dlx-bus");
        assert_eq!(bus.vars.len(), 2);
        assert_eq!(
            bus.vars.get("ecs_cluster_arn").and_then(|v| v.as_str()),
            Some("{{ecs_cluster.outputs.cluster_arn}}")
        );
    }

    #[test]
    fn deploy_output_interpolation() {
        let mut outputs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut ecs_outputs = BTreeMap::new();
        ecs_outputs.insert("cluster_arn".into(), "arn:aws:ecs:us-west-2:123:cluster/my-cluster".into());
        ecs_outputs.insert("api_gateway_id".into(), "gw-abc123".into());
        outputs.insert("ecs_cluster".into(), ecs_outputs);

        let ctx = DeployContext {
            slug: "agent-core".into(),
            env: "dev".into(),
            region: "us-west-2".into(),
            ..Default::default()
        };

        // Output ref resolves
        assert_eq!(
            render_deploy_value("{{ecs_cluster.outputs.cluster_arn}}", &ctx, &outputs),
            "arn:aws:ecs:us-west-2:123:cluster/my-cluster"
        );
        // Mixed: output ref + context token
        assert_eq!(
            render_deploy_value("{{slug}}-{{ecs_cluster.outputs.api_gateway_id}}", &ctx, &outputs),
            "agent-core-gw-abc123"
        );
        // Unknown output ref resolves to empty string
        assert_eq!(
            render_deploy_value("{{unknown.outputs.foo}}", &ctx, &outputs),
            ""
        );
        // Simple context tokens still work
        assert_eq!(
            render_deploy_value("{{slug}}-{{env}}", &ctx, &outputs),
            "agent-core-dev"
        );
    }

    #[test]
    fn deploy_dag_ordering() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "agent-core"

[deploy]
template = "dlx-service-template"

[deploy.uses.ecs_cluster]
project = "veil-ecs-cluster"

[deploy.uses.dlx_bus]
project = "dlx-bus"
vars.ecs_cluster_arn = "{{ecs_cluster.outputs.cluster_arn}}"

[deploy.vars]
service_name = "{{slug}}"
"#,
        )
        .unwrap();
        let deploy = load_deploy_config(&dir).unwrap().expect("deploy");
        let dag = build_deploy_dag(&deploy).unwrap();
        assert_eq!(dag.len(), 2);
        // ecs_cluster has no deps → comes first
        assert_eq!(dag[0].alias, "ecs_cluster");
        assert_eq!(dag[0].project, "veil-ecs-cluster");
        assert!(dag[0].depends_on.is_empty());
        // dlx_bus depends on ecs_cluster → comes second
        assert_eq!(dag[1].alias, "dlx_bus");
        assert_eq!(dag[1].project, "dlx-bus");
        assert_eq!(dag[1].depends_on, vec!["ecs_cluster"]);
    }

    #[test]
    fn deploy_dag_cycle_detection() {
        let mut deploy = DeployToml::default();
        deploy.uses.insert(
            "a".into(),
            DeployUsesToml {
                project: "proj-a".into(),
                vars: {
                    let mut m = BTreeMap::new();
                    m.insert("x".into(), toml::Value::String("{{b.outputs.foo}}".into()));
                    m
                },
            },
        );
        deploy.uses.insert(
            "b".into(),
            DeployUsesToml {
                project: "proj-b".into(),
                vars: {
                    let mut m = BTreeMap::new();
                    m.insert("y".into(), toml::Value::String("{{a.outputs.bar}}".into()));
                    m
                },
            },
        );
        let result = build_deploy_dag(&deploy);
        assert!(result.is_err());
        match result {
            Err(DeployDagError::CyclicDependency(path)) => {
                // The cycle should mention both a and b
                assert!(path.contains(&"a".to_string()) || path.contains(&"b".to_string()));
            }
            _ => panic!("expected CyclicDependency error"),
        }
    }

    #[test]
    fn deploy_dag_unknown_alias() {
        let mut deploy = DeployToml::default();
        deploy.uses.insert(
            "a".into(),
            DeployUsesToml {
                project: "proj-a".into(),
                vars: {
                    let mut m = BTreeMap::new();
                    m.insert("x".into(), toml::Value::String("{{nonexistent.outputs.foo}}".into()));
                    m
                },
            },
        );
        let result = build_deploy_dag(&deploy);
        assert!(result.is_err());
        match result {
            Err(DeployDagError::UnknownAlias { reference, .. }) => {
                assert_eq!(reference, "nonexistent");
            }
            _ => panic!("expected UnknownAlias error"),
        }
    }

    #[test]
    fn deploy_dag_three_level_chain() {
        let mut deploy = DeployToml::default();
        deploy.uses.insert(
            "network".into(),
            DeployUsesToml {
                project: "veil-network".into(),
                vars: BTreeMap::new(),
            },
        );
        deploy.uses.insert(
            "cluster".into(),
            DeployUsesToml {
                project: "veil-ecs-cluster".into(),
                vars: {
                    let mut m = BTreeMap::new();
                    m.insert("vpc_id".into(), toml::Value::String("{{network.outputs.vpc_id}}".into()));
                    m
                },
            },
        );
        deploy.uses.insert(
            "bus".into(),
            DeployUsesToml {
                project: "dlx-bus".into(),
                vars: {
                    let mut m = BTreeMap::new();
                    m.insert("cluster_arn".into(), toml::Value::String("{{cluster.outputs.cluster_arn}}".into()));
                    m
                },
            },
        );
        let dag = build_deploy_dag(&deploy).unwrap();
        assert_eq!(dag.len(), 3);
        assert_eq!(dag[0].alias, "network");
        assert_eq!(dag[1].alias, "cluster");
        assert_eq!(dag[2].alias, "bus");
    }

    #[test]
    fn codegen_none_string_normalizes_to_clear() {
        assert_eq!(
            CodegenToml::normalize_opt(&Some("none".into())),
            Some(None)
        );
        assert_eq!(CodegenToml::normalize_opt(&Some("".into())), Some(None));
        assert_eq!(
            CodegenToml::normalize_opt(&Some("Handle".into())),
            Some(Some("Handle".into()))
        );
        assert_eq!(CodegenToml::normalize_opt(&None), None);
    }

    #[test]
    fn resolve_dependency_graph_is_deps_first() {
        let hub = tempfile_dir();
        let c = hub.join("c");
        let b = hub.join("b");
        let a = hub.join("a");
        for p in [&c, &b, &a] {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(c.join("veil.toml"), "name = \"c\"\n").unwrap();
        std::fs::write(c.join("main.veil"), "pkg c\n").unwrap();
        std::fs::write(
            b.join("veil.toml"),
            "[dependencies]\nc = { path = \"../c\" }\n",
        )
        .unwrap();
        std::fs::write(b.join("main.veil"), "pkg b\n").unwrap();
        std::fs::write(
            a.join("veil.toml"),
            "[dependencies]\nb = { path = \"../b\" }\n",
        )
        .unwrap();
        std::fs::write(a.join("main.veil"), "pkg a\n").unwrap();

        let graph = resolve_dependency_graph(&a).unwrap();
        let names: Vec<String> = graph
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(names, vec!["c", "b", "a"]);
    }

    #[test]
    fn resolve_dependency_graph_ignores_cycle() {
        let hub = tempfile_dir();
        let a = hub.join("a");
        let b = hub.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(
            a.join("veil.toml"),
            "[dependencies]\nb = { path = \"../b\" }\n",
        )
        .unwrap();
        std::fs::write(a.join("main.veil"), "pkg a\n").unwrap();
        std::fs::write(
            b.join("veil.toml"),
            "[dependencies]\na = { path = \"../a\" }\n",
        )
        .unwrap();
        std::fs::write(b.join("main.veil"), "pkg b\n").unwrap();
        let graph = resolve_dependency_graph(&a).unwrap();
        assert_eq!(graph.len(), 2);
    }

    #[test]
    fn resolve_path_dep() {
        let hub = tempfile_dir();
        let eng = hub.join("application");
        std::fs::create_dir_all(&eng).unwrap();
        std::fs::write(eng.join("veil.toml"), "name = \"application\"\n").unwrap();
        std::fs::write(eng.join("main.veil"), "pkg application\n  use ddd\n").unwrap();

        let app = hub.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("veil.toml"),
            "[dependencies]\napplication = { path = \"../application\" }\n",
        )
        .unwrap();

        let roots = resolve_dependency_roots(&app).unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].ends_with("application") || roots[0].file_name().unwrap() == "application");
    }

    #[test]
    fn resolve_project_dep_via_hub() {
        let hub = tempfile_dir();
        let eng = hub.join("application");
        std::fs::create_dir_all(eng.join("layers")).unwrap();
        std::fs::write(eng.join("veil.toml"), "name = \"application\"\n").unwrap();
        std::fs::write(
            eng.join("layers").join("main.layer"),
            "pkg application v1\n  use ddd\n",
        )
        .unwrap();

        let app = hub.join("wear_test");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("veil.toml"),
            "[dependencies]\napplication = { project = \"application\" }\n",
        )
        .unwrap();

        // Hub is parent of app
        let roots = resolve_dependency_roots(&app).unwrap();
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn package_entry_main_veil_and_layer() {
        let root = tempfile_dir();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(
            root.join("veil.toml"),
            r#"
name = "dlx-designkit"
[package]
name = "designkit"
veil = "main.veil"
layer = "layers/main.layer"
"#,
        )
        .unwrap();
        std::fs::write(root.join("main.veil"), "pkg designkit\n  use ddd\n").unwrap();
        std::fs::write(
            root.join("layers/main.layer"),
            "pkg designkit v1\n  use sveltekit5\n",
        )
        .unwrap();

        let entry = load_package_entry(&root).unwrap();
        assert_eq!(entry.use_name, "designkit");
        assert_eq!(entry.veil, PathBuf::from("main.veil"));
        assert_eq!(entry.layer, PathBuf::from("layers/main.layer"));
        assert!(entry.provides_use("designkit"));
        assert!(package_source_in_root(&root, "designkit")
            .unwrap()
            .ends_with("main.veil"));
        assert!(layer_source_in_root(&root, "designkit")
            .unwrap()
            .ends_with("main.layer"));
    }

    #[test]
    fn find_package_via_main_veil_in_search_path() {
        let root = tempfile_dir();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(
            root.join("veil.toml"),
            "name = \"application\"\n[package]\nname = \"application\"\n",
        )
        .unwrap();
        // Defaults prefer main.veil when present
        std::fs::write(root.join("main.veil"), "pkg application\n  use ddd\n").unwrap();
        std::fs::write(
            root.join("layers/main.layer"),
            "pkg application v1\n  use ddd\n",
        )
        .unwrap();

        let found = crate::adapt::find_package_source("application", &[root.clone()]);
        assert!(found.unwrap().ends_with("main.veil"));
    }

    #[test]
    fn require_project_root_fails_without_veil_toml() {
        let dir = tempfile_dir();
        let leaf = dir.join("app.veil");
        std::fs::write(&leaf, "pkg App\n").unwrap();
        let err = require_project_root(&leaf).expect_err("must fail without veil.toml");
        assert!(
            err.contains(MISSING_VEIL_TOML),
            "diagnostic must contain '{MISSING_VEIL_TOML}': {err}"
        );
        // Offending absolute path is surfaced.
        assert!(err.contains(&dir.to_string_lossy().to_string()) || err.contains("app.veil") || err.contains("veil"), "{err}");
    }

    #[test]
    fn require_project_root_succeeds_with_veil_toml() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("veil.toml"), "[package]\nname = \"app\"\n").unwrap();
        let leaf = dir.join("app.veil");
        std::fs::write(&leaf, "pkg App\n").unwrap();
        let root = require_project_root(&leaf).expect("must succeed with veil.toml");
        assert_eq!(root, dir);
    }

    #[test]
    fn workspace_only_root_parses_and_lists_members() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            "[workspace]\nmembers = [\"ddd\", \"di\", \"bus\"]\n",
        )
        .unwrap();
        // Parses without error (no [package]) and yields members.
        let members = load_workspace_members(&dir);
        assert_eq!(members, vec!["ddd", "di", "bus"]);
        // A [workspace]-only root has no packages: load_product_deps parses cleanly.
        let deps = load_product_deps(&dir).expect("workspace-only veil.toml must parse");
        assert!(deps.is_empty());
        // require_project_root treats it as a valid root (has veil.toml).
        let leaf = dir.join("sub.veil");
        std::fs::write(&leaf, "pkg Sub\n").unwrap();
        assert_eq!(require_project_root(&leaf).unwrap(), dir);
    }

    #[test]
    fn workspace_members_empty_when_no_workspace_section() {
        let dir = tempfile_dir();
        std::fs::write(dir.join("veil.toml"), "[package]\nname = \"app\"\n").unwrap();
        assert!(load_workspace_members(&dir).is_empty());
    }

    #[test]
    fn is_workspace_root_detects_workspace_section() {
        let ws = tempfile_dir();
        std::fs::write(ws.join("veil.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert!(is_workspace_root(&ws));

        let pkg = tempfile_dir();
        std::fs::write(pkg.join("veil.toml"), "[package]\nname = \"app\"\n").unwrap();
        assert!(!is_workspace_root(&pkg));

        // Missing veil.toml → not a workspace root.
        let empty = tempfile_dir();
        assert!(!is_workspace_root(&empty));
    }

    #[test]
    fn normalize_member_trims_and_rejects_traversal() {
        assert_eq!(normalize_member("  ddd/ ").as_deref(), Some("ddd"));
        assert_eq!(normalize_member("a/b/c").as_deref(), Some("a/b/c"));
        assert_eq!(normalize_member("libs\\di").as_deref(), Some("libs/di"));
        // Reuses normalize_subpath posture: `.` and `..` segments are rejected.
        assert_eq!(normalize_member("./ddd"), None);
        assert_eq!(normalize_member(""), None);
        assert_eq!(normalize_member("   "), None);
        // Traversal is refused.
        assert_eq!(normalize_member("../evil"), None);
        assert_eq!(normalize_member("a/../b"), None);
        assert_eq!(normalize_member("a/./b"), None);
    }

    #[test]
    fn load_workspace_members_drops_traversal_and_normalizes() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            "[workspace]\nmembers = [\"ddd/\", \"../evil\", \"libs\\\\di\", \"a/../b\"]\n",
        )
        .unwrap();
        // `../evil` and `a/../b` dropped; `ddd/` trimmed; backslash folded.
        assert_eq!(load_workspace_members(&dir), vec!["ddd", "libs/di"]);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "veil-deps-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}








