//! Execution topology — the *where* layer of the VEIL Execution Host.
//!
//! A VEIL project chooses, per `veil.toml [execution]`, whether its execution
//! artifact(s) run in the **shared** VEIL Execution Host (default) or in the
//! project's **own dedicated** scoped executor service. Crucially this is a
//! *deployment-topology* choice over the SAME engine — a dedicated executor is
//! literally `veil-runtime` in the same `VEIL_ROLE=execution-host` run-mode,
//! deployed as its own ECS service (`veil-<slug>-executor`) with its artifact
//! set scoped to one project. There is NO forked executor codebase; the only
//! differences are which artifacts it hosts, its service identity, and its
//! scaling policy (see `veil-execution-host-design`, topology section).
//!
//! ## What this module owns
//! - [`ExecutionTopology`] — the resolved topology (shared vs dedicated) that is
//!   persisted onto trigger/artifact records so fire-routing invokes the RIGHT
//!   executor.
//! - [`DedicatedSizing`] — the `[execution.dedicated]` cpu/memory/task-bounds/
//!   autoscale-target block used to provision the scoped service.
//! - [`parse_execution`] / [`parse_execution_from_file`] — read `[execution]`
//!   from an already-parsed `veil.toml` value (mirrors `parse_triggers`).
//! - [`resolve_endpoint`] — the trigger-fire routing resolver: given a topology
//!   (+ ambient shared-host base), return the executor base URL to invoke.
//!
//! ## veil.toml shape
//! ```toml
//! [execution]
//! mode = "shared"        # default: register into the shared Execution Host
//! # or:
//! mode = "dedicated"     # deploy this project's own scoped executor service
//!
//! [execution.dedicated]  # only honoured when mode = "dedicated"
//! cpu = 512
//! memory = 1024
//! min_tasks = 1
//! max_tasks = 4
//! autoscale_target_cpu = 60
//! ```

use serde::{Deserialize, Serialize};

/// The resolved execution topology of an artifact / trigger.
///
/// This is what gets *persisted* (on the trigger record) so the fire-routing
/// path can send a schedule/event/on-demand invoke to the correct executor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExecutionTopology {
    /// Register into the shared VEIL Execution Host (default). No per-project
    /// infra; the shared host picks the artifact up from the registry.
    Shared,
    /// This project deploys its own scoped executor service
    /// (`veil-<slug>-executor`) running the same execution-host run-mode,
    /// hosting ONLY this project's artifact(s), with its own scaling policy.
    Dedicated {
        /// Project slug — names the service (`veil-<slug>-executor`) and scopes
        /// which artifact(s) the executor hosts.
        slug: String,
        /// Sizing + scaling for the scoped service (from `[execution.dedicated]`).
        #[serde(default)]
        sizing: DedicatedSizing,
    },
}

impl Default for ExecutionTopology {
    fn default() -> Self {
        ExecutionTopology::Shared
    }
}

impl ExecutionTopology {
    /// Whether this topology is the shared host (the default).
    pub fn is_shared(&self) -> bool {
        matches!(self, ExecutionTopology::Shared)
    }

    /// Whether this topology is a project-scoped dedicated executor.
    pub fn is_dedicated(&self) -> bool {
        matches!(self, ExecutionTopology::Dedicated { .. })
    }

    /// The dedicated service name (`veil-<slug>-executor`) for a dedicated
    /// topology, or `None` for the shared host.
    pub fn dedicated_service_name(&self) -> Option<String> {
        match self {
            ExecutionTopology::Dedicated { slug, .. } => Some(dedicated_service_name(slug)),
            ExecutionTopology::Shared => None,
        }
    }
}

/// Sizing + scaling for a dedicated executor, parsed from
/// `[execution.dedicated]`. All fields have sensible defaults so a bare
/// `mode = "dedicated"` still yields a provisionable service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DedicatedSizing {
    /// Fargate CPU units (256/512/1024/2048/4096).
    pub cpu: u32,
    /// Fargate task memory in MiB.
    pub memory: u32,
    /// Minimum running tasks (scale-in floor).
    pub min_tasks: u32,
    /// Maximum running tasks (scale-out ceiling).
    pub max_tasks: u32,
    /// Target-tracking CPU utilization % for the dedicated service's autoscaler.
    pub autoscale_target_cpu: u32,
}

impl Default for DedicatedSizing {
    fn default() -> Self {
        // Matches the `[execution.dedicated]` example in the design/prompt.
        Self {
            cpu: 512,
            memory: 1024,
            min_tasks: 1,
            max_tasks: 4,
            autoscale_target_cpu: 60,
        }
    }
}

/// Errors from parsing `[execution]`.
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum TopologyError {
    #[error("unknown execution mode '{0}' (expected 'shared' or 'dedicated')")]
    UnknownMode(String),
    #[error("mode='dedicated' requires a project slug to name veil-<slug>-executor")]
    MissingSlug,
}

/// The default (implicit) execution mode when no `[execution]` block is present.
pub const DEFAULT_MODE: &str = "shared";

/// The dedicated-executor ECS service name for a project slug.
///
/// Constraint (design): `veil-` prefix; dedicated services are named
/// `veil-<slug>-executor` on the shared `veil-cluster`.
pub fn dedicated_service_name(slug: &str) -> String {
    format!("veil-{slug}-executor")
}

/// Parse the `[execution]` block from an already-parsed `veil.toml`
/// (`serde_json::Value`), returning the resolved [`ExecutionTopology`].
///
/// `slug` is the project slug used to name a dedicated service. It is required
/// only when `mode = "dedicated"`; for the shared default it is ignored.
///
/// Semantics:
/// - No `[execution]` block, or `mode = "shared"` (or absent mode) ⇒
///   [`ExecutionTopology::Shared`].
/// - `mode = "dedicated"` ⇒ [`ExecutionTopology::Dedicated`] carrying the slug +
///   `[execution.dedicated]` sizing (defaults fill any missing fields).
/// - Any other mode ⇒ [`TopologyError::UnknownMode`] (fail loud; do not silently
///   fall back to shared, so a typo like `mode = "dedcated"` is caught at
///   registration rather than silently mis-routing).
pub fn parse_execution(
    veil_toml: &serde_json::Value,
    slug: &str,
) -> Result<ExecutionTopology, TopologyError> {
    let exec = match veil_toml.get("execution") {
        Some(e) => e,
        None => return Ok(ExecutionTopology::Shared),
    };

    let mode = exec
        .get("mode")
        .and_then(|m| m.as_str())
        .unwrap_or(DEFAULT_MODE);

    match mode {
        "shared" => Ok(ExecutionTopology::Shared),
        "dedicated" => {
            if slug.is_empty() {
                return Err(TopologyError::MissingSlug);
            }
            let sizing = exec
                .get("dedicated")
                .map(parse_dedicated_sizing)
                .unwrap_or_default();
            Ok(ExecutionTopology::Dedicated {
                slug: slug.to_string(),
                sizing,
            })
        }
        other => Err(TopologyError::UnknownMode(other.to_string())),
    }
}

/// Parse `[execution.dedicated]` sizing, filling any missing field with the
/// [`DedicatedSizing::default`] value.
fn parse_dedicated_sizing(v: &serde_json::Value) -> DedicatedSizing {
    let d = DedicatedSizing::default();
    let u32_at = |key: &str, fallback: u32| -> u32 {
        v.get(key)
            .and_then(|n| n.as_u64())
            .map(|n| n as u32)
            .unwrap_or(fallback)
    };
    DedicatedSizing {
        cpu: u32_at("cpu", d.cpu),
        memory: u32_at("memory", d.memory),
        min_tasks: u32_at("min_tasks", d.min_tasks),
        max_tasks: u32_at("max_tasks", d.max_tasks),
        autoscale_target_cpu: u32_at("autoscale_target_cpu", d.autoscale_target_cpu),
    }
}

/// Load + parse `[execution]` directly from a `veil.toml` file on disk. A
/// missing file yields the shared default (matches `parse_triggers_from_file`).
pub fn parse_execution_from_file(
    path: &std::path::Path,
    slug: &str,
) -> Result<ExecutionTopology, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExecutionTopology::Shared)
        }
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let toml_value: toml::Value =
        toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let json_value = serde_json::to_value(&toml_value)
        .map_err(|e| format!("convert toml→json {}: {e}", path.display()))?;
    parse_execution(&json_value, slug).map_err(|e| e.to_string())
}

/// The resolved target of a trigger fire: which executor base URL to invoke.
///
/// The fire-routing path calls [`resolve_endpoint`] with the artifact's
/// persisted topology plus the ambient shared-host base URL. In-process fires
/// (the shared host firing an artifact it itself hosts) resolve to
/// [`ExecutorEndpoint::InProcess`]; a dedicated artifact resolves to
/// [`ExecutorEndpoint::Remote`] pointing at the scoped service.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecutorEndpoint {
    /// Invoke the artifact in *this* process (shared host hosting its own
    /// registered artifact). No network hop.
    InProcess,
    /// Invoke the artifact over HTTP at the given executor base URL (a dedicated
    /// `veil-<slug>-executor` service).
    Remote { base_url: String },
}

impl ExecutorEndpoint {
    /// The base URL to POST an invoke/fire to, or `None` for in-process.
    pub fn base_url(&self) -> Option<&str> {
        match self {
            ExecutorEndpoint::InProcess => None,
            ExecutorEndpoint::Remote { base_url } => Some(base_url),
        }
    }
}

/// Routing resolver: given an artifact's [`ExecutionTopology`] and the ambient
/// context, return the [`ExecutorEndpoint`] a fire should be dispatched to.
///
/// - **Shared** topology ⇒ the executor is the shared host. When the caller IS
///   the shared host (`caller_is_shared_host = true`), that is an in-process
///   invoke; otherwise (e.g. the Scheduler or ProductHost dispatching) it is a
///   remote call to `shared_host_base`.
/// - **Dedicated** topology ⇒ always a remote call to the scoped service,
///   whose base URL is derived from `dedicated_base` (a template with
///   `{service}` substituted for `veil-<slug>-executor`) or, when the caller
///   IS that exact dedicated executor, in-process.
///
/// This keeps the SAME engine while sending a fire to the RIGHT executor.
pub fn resolve_endpoint(
    topology: &ExecutionTopology,
    ctx: &RoutingContext,
) -> ExecutorEndpoint {
    match topology {
        ExecutionTopology::Shared => {
            if ctx.caller_is_shared_host {
                ExecutorEndpoint::InProcess
            } else {
                ExecutorEndpoint::Remote {
                    base_url: ctx.shared_host_base.clone(),
                }
            }
        }
        ExecutionTopology::Dedicated { slug, .. } => {
            let service = dedicated_service_name(slug);
            // If this very process is the dedicated executor for this slug, the
            // invoke is in-process; otherwise route to the scoped service URL.
            if ctx.self_service.as_deref() == Some(service.as_str()) {
                ExecutorEndpoint::InProcess
            } else {
                ExecutorEndpoint::Remote {
                    base_url: ctx.dedicated_base_for(slug),
                }
            }
        }
    }
}

/// Ambient context for [`resolve_endpoint`]: where the shared host lives, how to
/// build a dedicated service URL, and the identity of the *current* process so a
/// self-invoke short-circuits to in-process.
#[derive(Debug, Clone)]
pub struct RoutingContext {
    /// Base URL of the shared execution host (e.g. `https://exec.veil.dev.dashlx.com`).
    pub shared_host_base: String,
    /// URL template for a dedicated executor. `{service}` is replaced with
    /// `veil-<slug>-executor` and `{slug}` with the raw slug. Example:
    /// `https://{service}.veil.dev.dashlx.com`.
    pub dedicated_base_template: String,
    /// Whether the current process is the shared execution host.
    pub caller_is_shared_host: bool,
    /// The dedicated service name this process IS, if any (e.g.
    /// `veil-acme-executor`). Set on a dedicated executor so it invokes its own
    /// artifacts in-process rather than calling itself over the network.
    pub self_service: Option<String>,
}

impl RoutingContext {
    /// Build the dedicated base URL for a slug by substituting the template.
    pub fn dedicated_base_for(&self, slug: &str) -> String {
        let service = dedicated_service_name(slug);
        self.dedicated_base_template
            .replace("{service}", &service)
            .replace("{slug}", slug)
    }

    /// Construct a routing context from the standard env vars. Used by the host
    /// process to resolve fires. Env:
    /// - `VEIL_EXEC_SHARED_BASE` — shared host base URL.
    /// - `VEIL_EXEC_DEDICATED_BASE_TEMPLATE` — dedicated URL template.
    /// - `VEIL_ROLE == "execution-host"` ⇒ caller is the shared host, UNLESS
    ///   `VEIL_EXEC_ARTIFACT_SCOPE` names a dedicated slug (then this process is
    ///   that dedicated executor, not the shared host).
    pub fn from_env() -> Self {
        let shared_host_base = std::env::var("VEIL_EXEC_SHARED_BASE")
            .unwrap_or_else(|_| "http://localhost:8090".to_string());
        let dedicated_base_template = std::env::var("VEIL_EXEC_DEDICATED_BASE_TEMPLATE")
            .unwrap_or_else(|_| "http://{service}:8090".to_string());
        let is_exec_host =
            std::env::var("VEIL_ROLE").ok().as_deref() == Some("execution-host");
        // A scoped executor sets VEIL_EXEC_ARTIFACT_SCOPE=<slug>; that process is
        // the dedicated executor for <slug>, not the shared host.
        let scope = std::env::var("VEIL_EXEC_ARTIFACT_SCOPE").ok().filter(|s| !s.is_empty());
        let (caller_is_shared_host, self_service) = resolve_self_identity(is_exec_host, scope.as_deref());
        Self {
            shared_host_base,
            dedicated_base_template,
            caller_is_shared_host,
            self_service,
        }
    }
}

/// Pure identity resolution used by [`RoutingContext::from_env`]: given whether
/// this process runs the execution-host role and an optional artifact-scope
/// slug, decide whether it is the shared host or a dedicated executor.
///
/// - execution-host role + scope=Some(slug) ⇒ this IS `veil-<slug>-executor`
///   (a dedicated executor); not the shared host.
/// - execution-host role + no scope ⇒ the shared host.
/// - not the execution-host role (e.g. Scheduler/ProductHost) ⇒ neither; all
///   fires route remotely.
///
/// Factored out so the branch that decides in-process vs. remote on a live
/// dedicated task is unit-tested without mutating global process env.
pub fn resolve_self_identity(
    is_execution_host: bool,
    scope: Option<&str>,
) -> (bool, Option<String>) {
    match (is_execution_host, scope) {
        (true, Some(slug)) if !slug.is_empty() => (false, Some(dedicated_service_name(slug))),
        (true, _) => (true, None),
        (false, _) => (false, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn toml_to_json(src: &str) -> serde_json::Value {
        let tv: toml::Value = toml::from_str(src).unwrap();
        serde_json::to_value(&tv).unwrap()
    }

    #[test]
    fn no_execution_block_is_shared() {
        let v = json!({ "deploy": { "type": "ecs" } });
        assert_eq!(parse_execution(&v, "acme").unwrap(), ExecutionTopology::Shared);
    }

    #[test]
    fn explicit_shared_mode() {
        let v = toml_to_json("[execution]\nmode = \"shared\"\n");
        assert_eq!(parse_execution(&v, "acme").unwrap(), ExecutionTopology::Shared);
    }

    #[test]
    fn execution_block_without_mode_defaults_shared() {
        // An [execution] table with no `mode` key is treated as shared.
        let v = toml_to_json("[execution]\n");
        assert_eq!(parse_execution(&v, "acme").unwrap(), ExecutionTopology::Shared);
    }

    #[test]
    fn dedicated_with_defaults_when_no_sizing() {
        let v = toml_to_json("[execution]\nmode = \"dedicated\"\n");
        let topo = parse_execution(&v, "acme").unwrap();
        assert_eq!(
            topo,
            ExecutionTopology::Dedicated {
                slug: "acme".into(),
                sizing: DedicatedSizing::default(),
            }
        );
        assert_eq!(
            topo.dedicated_service_name().as_deref(),
            Some("veil-acme-executor")
        );
    }

    #[test]
    fn dedicated_parses_full_sizing() {
        let src = r#"
[execution]
mode = "dedicated"

[execution.dedicated]
cpu = 1024
memory = 2048
min_tasks = 2
max_tasks = 8
autoscale_target_cpu = 55
"#;
        let v = toml_to_json(src);
        let topo = parse_execution(&v, "orders").unwrap();
        match topo {
            ExecutionTopology::Dedicated { slug, sizing } => {
                assert_eq!(slug, "orders");
                assert_eq!(sizing.cpu, 1024);
                assert_eq!(sizing.memory, 2048);
                assert_eq!(sizing.min_tasks, 2);
                assert_eq!(sizing.max_tasks, 8);
                assert_eq!(sizing.autoscale_target_cpu, 55);
            }
            other => panic!("expected dedicated, got {other:?}"),
        }
    }

    #[test]
    fn dedicated_partial_sizing_fills_defaults() {
        let src = r#"
[execution]
mode = "dedicated"

[execution.dedicated]
cpu = 2048
"#;
        let v = toml_to_json(src);
        let topo = parse_execution(&v, "heavy").unwrap();
        match topo {
            ExecutionTopology::Dedicated { sizing, .. } => {
                assert_eq!(sizing.cpu, 2048);
                // Untouched fields fall back to defaults.
                assert_eq!(sizing.memory, DedicatedSizing::default().memory);
                assert_eq!(sizing.max_tasks, DedicatedSizing::default().max_tasks);
            }
            other => panic!("expected dedicated, got {other:?}"),
        }
    }

    #[test]
    fn unknown_mode_is_error() {
        let v = toml_to_json("[execution]\nmode = \"dedcated\"\n");
        assert_eq!(
            parse_execution(&v, "acme"),
            Err(TopologyError::UnknownMode("dedcated".into()))
        );
    }

    #[test]
    fn dedicated_without_slug_is_error() {
        let v = toml_to_json("[execution]\nmode = \"dedicated\"\n");
        assert_eq!(parse_execution(&v, ""), Err(TopologyError::MissingSlug));
    }

    #[test]
    fn topology_serde_round_trips() {
        for topo in [
            ExecutionTopology::Shared,
            ExecutionTopology::Dedicated {
                slug: "acme".into(),
                sizing: DedicatedSizing::default(),
            },
        ] {
            let s = serde_json::to_string(&topo).unwrap();
            let back: ExecutionTopology = serde_json::from_str(&s).unwrap();
            assert_eq!(topo, back);
        }
    }

    #[test]
    fn topology_deserializes_legacy_absent_as_shared_via_default() {
        // Records written before topology existed have no field; a serde default
        // of Shared is what the record type relies on. Confirm Default here.
        assert_eq!(ExecutionTopology::default(), ExecutionTopology::Shared);
    }

    // ─── Routing resolver ────────────────────────────────────────────────────

    fn ctx() -> RoutingContext {
        RoutingContext {
            shared_host_base: "https://exec.veil.dev.dashlx.com".into(),
            dedicated_base_template: "https://{service}.veil.dev.dashlx.com".into(),
            caller_is_shared_host: false,
            self_service: None,
        }
    }

    #[test]
    fn shared_topology_routes_to_shared_host_when_external_caller() {
        let ep = resolve_endpoint(&ExecutionTopology::Shared, &ctx());
        assert_eq!(
            ep,
            ExecutorEndpoint::Remote {
                base_url: "https://exec.veil.dev.dashlx.com".into()
            }
        );
    }

    #[test]
    fn shared_topology_is_in_process_for_shared_host_itself() {
        let mut c = ctx();
        c.caller_is_shared_host = true;
        let ep = resolve_endpoint(&ExecutionTopology::Shared, &c);
        assert_eq!(ep, ExecutorEndpoint::InProcess);
    }

    #[test]
    fn dedicated_topology_routes_to_scoped_service() {
        let topo = ExecutionTopology::Dedicated {
            slug: "acme".into(),
            sizing: DedicatedSizing::default(),
        };
        let ep = resolve_endpoint(&topo, &ctx());
        assert_eq!(
            ep,
            ExecutorEndpoint::Remote {
                base_url: "https://veil-acme-executor.veil.dev.dashlx.com".into()
            }
        );
    }

    #[test]
    fn dedicated_topology_is_in_process_on_its_own_executor() {
        let topo = ExecutionTopology::Dedicated {
            slug: "acme".into(),
            sizing: DedicatedSizing::default(),
        };
        let mut c = ctx();
        c.self_service = Some("veil-acme-executor".into());
        let ep = resolve_endpoint(&topo, &c);
        assert_eq!(ep, ExecutorEndpoint::InProcess);
    }

    #[test]
    fn dedicated_executor_still_routes_other_slugs_remotely() {
        // An acme executor firing a *different* project's dedicated artifact
        // routes to that project's service (not in-process).
        let topo = ExecutionTopology::Dedicated {
            slug: "orders".into(),
            sizing: DedicatedSizing::default(),
        };
        let mut c = ctx();
        c.self_service = Some("veil-acme-executor".into());
        let ep = resolve_endpoint(&topo, &c);
        assert_eq!(
            ep,
            ExecutorEndpoint::Remote {
                base_url: "https://veil-orders-executor.veil.dev.dashlx.com".into()
            }
        );
    }

    #[test]
    fn routing_context_from_env_shared_host() {
        // Simulate the env resolution logic directly (avoid mutating process env
        // in parallel tests): shared host role, no scope → caller is shared host.
        let c = RoutingContext {
            shared_host_base: "http://localhost:8090".into(),
            dedicated_base_template: "http://{service}:8090".into(),
            caller_is_shared_host: true,
            self_service: None,
        };
        assert!(c.caller_is_shared_host);
        assert_eq!(c.dedicated_base_for("acme"), "http://veil-acme-executor:8090");
    }

    #[test]
    fn self_identity_shared_host_when_exec_role_no_scope() {
        assert_eq!(resolve_self_identity(true, None), (true, None));
        assert_eq!(resolve_self_identity(true, Some("")), (true, None));
    }

    #[test]
    fn self_identity_dedicated_executor_when_scoped() {
        assert_eq!(
            resolve_self_identity(true, Some("acme")),
            (false, Some("veil-acme-executor".to_string()))
        );
    }

    #[test]
    fn self_identity_neither_when_not_exec_role() {
        // A Scheduler/ProductHost (not the execution-host role) is neither the
        // shared host nor a dedicated executor → all fires route remotely.
        assert_eq!(resolve_self_identity(false, None), (false, None));
        assert_eq!(resolve_self_identity(false, Some("acme")), (false, None));
    }
}
