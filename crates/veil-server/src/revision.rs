//! Live source revision bus (AGT-002) — mid-turn agent writes publish here;
//! `GET /api/events` streams them to the viewer.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use tokio::sync::broadcast;

/// Payload for SSE `revision` events.
#[derive(Debug, Clone, Serialize)]
pub struct RevisionEvent {
    pub revision: u64,
    pub bytes: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Optional hint: `"agent"`, `"edit"`, `"source"`, …
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Process-wide bus so any write path can notify without threading AppState.
#[derive(Clone)]
pub struct RevisionBus {
    rev: Arc<AtomicU64>,
    tx: broadcast::Sender<RevisionEvent>,
}

impl RevisionBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(128);
        Self {
            rev: Arc::new(AtomicU64::new(1)),
            tx,
        }
    }

    pub fn current(&self) -> u64 {
        self.rev.load(Ordering::SeqCst)
    }

    pub fn publish(&self, bytes: usize, path: &str, reason: &str) -> RevisionEvent {
        let revision = self.rev.fetch_add(1, Ordering::SeqCst) + 1;
        let ev = RevisionEvent {
            revision,
            bytes,
            path: path.to_string(),
            reason: reason.to_string(),
        };
        let _ = self.tx.send(ev.clone());
        ev
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RevisionEvent> {
        self.tx.subscribe()
    }
}

impl Default for RevisionBus {
    fn default() -> Self {
        Self::new()
    }
}

static BUS: OnceLock<RevisionBus> = OnceLock::new();

/// Global revision bus used by provider writes and SSE.
pub fn bus() -> &'static RevisionBus {
    BUS.get_or_init(RevisionBus::new)
}
