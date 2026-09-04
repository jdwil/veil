//! Triggers — the "when" layer of the VEIL Execution Host.
//!
//! A **trigger** binds an incoming event (a schedule tick, a domain event, or
//! an on-demand call) to an execution artifact. The host resolves the target
//! artifact through the shared [`FunctionRegistry`](crate::function_invoke::FunctionRegistry)
//! and invokes it over the existing `CallableHandle` substrate.
//!
//! Per `veil-execution-host-design`, triggers live in a **separate table space**
//! (DDB `applications` single-table, `TRIGGER#{tenant}` PK) — NOT on the artifact
//! record — because triggers mutate independently of artifact versions and the
//! Scheduler queries them on their own cadence.
//!
//! ## Storage layout (single-table)
//! - PK = `TRIGGER#{tenant_id}`  SK = `T#{trigger_id}`  data = JSON(TriggerRecord)
//!
//! A per-tenant PK keeps a tenant's triggers in one partition for cheap `query`
//! listing while sharing the same physical table as the artifact registry.

pub mod resolver;
pub mod store;
pub mod toml_parse;

#[cfg(test)]
mod tests;

pub use resolver::TriggerResolver;
pub use store::TriggerStore;
pub use toml_parse::parse_triggers_from_file;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What kind of event fires a trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// Fired by an explicit RPC/HTTP call to the host (`fire` with a payload).
    OnDemand,
    /// Fired by the Scheduler on a cron/rate schedule. `schedule_expr` carries
    /// the expression; `timezone` its interpretation.
    Schedule,
    /// Fired by a domain event matching `event_type` (+ optional `filter`).
    Event,
}

/// A stored trigger binding an event to an execution artifact.
///
/// The `payload_template` (optional) is a JSON value merged over / substituted
/// with the incoming fire payload before invoke. v1 semantics: the template is
/// the base object and incoming fire fields are shallow-merged on top (fire
/// wins), so a template supplies defaults/constants the artifact expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerRecord {
    /// Stable trigger id (uuid or caller-supplied slug).
    pub id: String,
    /// Owning tenant.
    pub tenant_id: String,
    /// The execution artifact this trigger invokes (artifact/function id).
    pub artifact_id: String,
    /// What fires it.
    pub kind: TriggerKind,
    /// For `Schedule`: the cron/rate expression (e.g. `"rate(5 minutes)"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_expr: Option<String>,
    /// For `Schedule`: timezone the expression is interpreted in (IANA name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// For `Event`: the domain event type this trigger listens for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// For `Event`: an optional JSON filter the event must match (shallow
    /// equality on the listed keys). Absent ⇒ match any event of `event_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Value>,
    /// Optional base payload merged under the incoming fire payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_template: Option<serde_json::Value>,
    /// Whether the trigger is active. Disabled triggers are stored but never fire.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Execution topology of the target artifact — where a fire is routed
    /// (shared VEIL Execution Host vs. this project's dedicated
    /// `veil-<slug>-executor`). Persisted here so the fire-routing path invokes
    /// the RIGHT executor. `#[serde(default)]` ⇒ legacy rows (written before
    /// topology existed) deserialize as [`ExecutionTopology::Shared`].
    #[serde(default)]
    pub topology: crate::execution_topology::ExecutionTopology,
    /// When this record was created.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// When this record was last updated.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

fn default_enabled() -> bool {
    true
}

impl TriggerRecord {
    /// Merge the trigger's `payload_template` under an incoming fire payload.
    /// Fire fields win over template fields (template = defaults). If neither is
    /// an object, the fire payload is returned as-is (or template if fire is
    /// null).
    pub fn resolve_payload(&self, fire_payload: serde_json::Value) -> serde_json::Value {
        match (&self.payload_template, fire_payload) {
            (Some(serde_json::Value::Object(tpl)), serde_json::Value::Object(fire)) => {
                let mut merged = tpl.clone();
                for (k, v) in fire {
                    merged.insert(k, v);
                }
                serde_json::Value::Object(merged)
            }
            (Some(tpl), serde_json::Value::Null) => tpl.clone(),
            (_, fire) => fire,
        }
    }

    /// Whether an incoming event payload satisfies this trigger's `filter`.
    /// A `None` filter matches everything; a filter object requires shallow
    /// key/value equality for each listed key.
    // Retained: in-flight trigger-fire filtering (triggers subsystem).
    #[allow(dead_code)]
    pub fn matches_filter(&self, event: &serde_json::Value) -> bool {
        match &self.filter {
            None => true,
            Some(serde_json::Value::Object(filter)) => filter.iter().all(|(k, want)| {
                event.get(k).map(|got| got == want).unwrap_or(false)
            }),
            // A non-object filter is malformed; treat as non-matching (fail safe).
            Some(_) => false,
        }
    }
}

/// A trigger declaration parsed from a project's `veil.toml [[triggers]]` block.
/// Distinct from [`TriggerRecord`] (which is the stored, tenant-scoped, id'd
/// row) — a declaration is the *authoring* shape that flows into registration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerDeclaration {
    /// Optional stable id; a uuid is minted at registration if absent.
    #[serde(default)]
    pub id: Option<String>,
    /// What fires it.
    pub kind: TriggerKind,
    #[serde(default)]
    pub schedule_expr: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub filter: Option<serde_json::Value>,
    #[serde(default)]
    pub payload_template: Option<serde_json::Value>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl TriggerDeclaration {
    /// Promote a declaration into a stored [`TriggerRecord`] for `tenant` +
    /// `artifact_id`, minting an id if the declaration did not supply one and
    /// stamping the project's execution `topology` (shared vs dedicated) so the
    /// fire-routing path invokes the right executor.
    pub fn into_record(
        self,
        tenant_id: &str,
        artifact_id: &str,
        topology: crate::execution_topology::ExecutionTopology,
    ) -> TriggerRecord {
        let now = Utc::now();
        TriggerRecord {
            id: self
                .id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            tenant_id: tenant_id.to_string(),
            artifact_id: artifact_id.to_string(),
            kind: self.kind,
            schedule_expr: self.schedule_expr,
            timezone: self.timezone,
            event_type: self.event_type,
            filter: self.filter,
            payload_template: self.payload_template,
            enabled: self.enabled,
            topology,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Errors from the trigger layer.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TriggerError {
    #[error("trigger not found: {0}")]
    NotFound(String),
    #[error("trigger disabled: {0}")]
    Disabled(String),
    #[error("trigger storage error: {0}")]
    Storage(String),
    #[error("trigger invoke failed: {0}")]
    Invoke(String),
    #[error("invalid trigger: {0}")]
    // Retained: in-flight trigger-validation error (triggers subsystem).
    #[allow(dead_code)]
    Invalid(String),
}
