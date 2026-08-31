//! Trigger resolver — turns an incoming fire into an artifact invocation.
//!
//! The resolver is the execution seam of the host: given a fire (a resolved
//! [`TriggerRecord`] or a direct on-demand call plus a payload), it
//!   1. checks the trigger is enabled,
//!   2. resolves the target artifact through the shared
//!      [`FunctionRegistry`](crate::function_invoke::FunctionRegistry)
//!      (which applies the toolchain-fingerprint gate, hash verification, tenant
//!      visibility, and sign-off), and
//!   3. invokes it over the existing `CallableHandle` substrate.
//!
//! Two host-level safety knobs are applied here (SOLVE ONCE):
//! - **Concurrency limit** — a [`tokio::sync::Semaphore`] bounds the number of
//!   in-flight invocations so a burst of fires cannot exhaust the process.
//! - **Feedback event seam** — a [`tokio::sync::broadcast`] channel every invoke
//!   emits lifecycle events into; Phase 3 wires a websocket to the receiver.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{broadcast, Semaphore};

use super::{TriggerError, TriggerRecord, TriggerStore};
use crate::function_invoke::FunctionRegistry;
use crate::tenancy::TenantId;

/// A lifecycle event emitted by the host as an artifact runs. This is the
/// **feedback seam** stub — Phase 3 subscribes a websocket to it for live
/// streaming. Deliberately small; artifacts can emit richer step events once the
/// capability channel is threaded across the FFI seam.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum FeedbackEvent {
    /// An invocation started.
    Started {
        tenant_id: String,
        artifact_id: String,
        trigger_id: Option<String>,
    },
    /// A progress/step note from the host (or, later, the artifact).
    Step {
        tenant_id: String,
        artifact_id: String,
        message: String,
    },
    /// An invocation finished.
    Finished {
        tenant_id: String,
        artifact_id: String,
        trigger_id: Option<String>,
        ok: bool,
    },
}

/// An incoming request to fire an artifact. Either references a stored trigger
/// (schedule/event/on-demand) or names an artifact directly (bare on-demand
/// invoke via the host RPC surface).
#[derive(Debug, Clone)]
pub struct TriggerFire {
    pub tenant_id: String,
    /// The stored trigger to fire, if this fire came from one. `None` for a
    /// direct on-demand artifact invoke.
    pub trigger: Option<TriggerRecord>,
    /// The target artifact id (taken from the trigger when present, else the
    /// direct invoke target).
    pub artifact_id: String,
    /// The incoming payload (merged under the trigger's `payload_template`).
    pub payload: Value,
}

/// The result of a fire.
#[derive(Debug, Clone)]
pub struct FireOutcome {
    pub artifact_id: String,
    pub result: Value,
}

/// Resolves + invokes fires against the shared registry, with a concurrency
/// bound and a feedback broadcast seam.
#[derive(Clone)]
pub struct TriggerResolver {
    registry: FunctionRegistry,
    store: Arc<TriggerStore>,
    /// Bounds concurrent in-flight invocations.
    concurrency: Arc<Semaphore>,
    /// Feedback event fan-out (Phase 3 websocket subscribes here).
    feedback: broadcast::Sender<FeedbackEvent>,
}

impl TriggerResolver {
    /// Build a resolver. `max_concurrency` bounds simultaneous invocations
    /// (default via [`default_max_concurrency`] when constructed from env).
    pub fn new(
        registry: FunctionRegistry,
        store: Arc<TriggerStore>,
        max_concurrency: usize,
    ) -> Self {
        let (feedback, _rx) = broadcast::channel(1024);
        Self {
            registry,
            store,
            concurrency: Arc::new(Semaphore::new(max_concurrency.max(1))),
            feedback,
        }
    }

    /// Subscribe to the feedback event stream (Phase 3 seam).
    pub fn subscribe(&self) -> broadcast::Receiver<FeedbackEvent> {
        self.feedback.subscribe()
    }

    /// Access the underlying trigger store (for the host's CRUD handlers).
    pub fn store(&self) -> &Arc<TriggerStore> {
        &self.store
    }

    /// Current number of invocation permits available (for health/metrics).
    pub fn available_permits(&self) -> usize {
        self.concurrency.available_permits()
    }

    /// Fire a stored trigger by tenant + id: load it, merge its payload template,
    /// and invoke the target artifact.
    pub async fn fire_trigger(
        &self,
        tenant_id: &str,
        trigger_id: &str,
        payload: Value,
    ) -> Result<FireOutcome, TriggerError> {
        let trigger = self.store.get(tenant_id, trigger_id).await?;
        if !trigger.enabled {
            return Err(TriggerError::Disabled(trigger_id.to_string()));
        }
        let merged = trigger.resolve_payload(payload);
        let fire = TriggerFire {
            tenant_id: tenant_id.to_string(),
            artifact_id: trigger.artifact_id.clone(),
            trigger: Some(trigger),
            payload: merged,
        };
        self.fire(fire).await
    }

    /// Invoke an artifact directly (bare on-demand — no stored trigger row).
    pub async fn invoke_on_demand(
        &self,
        tenant_id: &str,
        artifact_id: &str,
        payload: Value,
    ) -> Result<FireOutcome, TriggerError> {
        let fire = TriggerFire {
            tenant_id: tenant_id.to_string(),
            trigger: None,
            artifact_id: artifact_id.to_string(),
            payload,
        };
        self.fire(fire).await
    }

    /// Core fire path: acquire a concurrency permit, resolve the artifact via the
    /// shared registry, invoke it, and emit feedback events.
    pub async fn fire(&self, fire: TriggerFire) -> Result<FireOutcome, TriggerError> {
        // Bound concurrency: a burst of fires waits here rather than spawning
        // unbounded native invocations.
        let _permit = self
            .concurrency
            .acquire()
            .await
            .map_err(|e| TriggerError::Invoke(format!("semaphore closed: {e}")))?;

        let trigger_id = fire.trigger.as_ref().map(|t| t.id.clone());
        // Feedback seam: emit start (ignore no-subscriber errors).
        let _ = self.feedback.send(FeedbackEvent::Started {
            tenant_id: fire.tenant_id.clone(),
            artifact_id: fire.artifact_id.clone(),
            trigger_id: trigger_id.clone(),
        });

        let tenant = TenantId::new(&fire.tenant_id);
        let outcome = async {
            let handle = self
                .registry
                .resolve(&tenant, &fire.artifact_id)
                .await
                .map_err(|e| TriggerError::Invoke(format!("resolve '{}': {e}", fire.artifact_id)))?;
            handle
                .invoke(fire.payload.clone())
                .await
                .map_err(|e| TriggerError::Invoke(format!("invoke '{}': {e}", fire.artifact_id)))
        }
        .await;

        let ok = outcome.is_ok();
        let _ = self.feedback.send(FeedbackEvent::Finished {
            tenant_id: fire.tenant_id.clone(),
            artifact_id: fire.artifact_id.clone(),
            trigger_id,
            ok,
        });

        outcome.map(|result| FireOutcome {
            artifact_id: fire.artifact_id,
            result,
        })
    }
}

/// Default max concurrent invocations. Override via `VEIL_FFI_MAX_CONCURRENCY`.
pub fn default_max_concurrency() -> usize {
    std::env::var("VEIL_FFI_MAX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(64)
}
