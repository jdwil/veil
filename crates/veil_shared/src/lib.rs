//! Shared types across all context crates — common errors and
//! layer-provided infrastructure traits (routing ports, etc.).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

/// Domain error type.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("Not found")]
    NotFound,
    #[error("Validation failed: {0}")]
    Validation(String),
    #[error("External service error: {0}")]
    External(String),
}

/// Validation error type.
#[derive(Debug, thiserror::Error)]
#[error("Validation error: {0}")]
pub struct ValidationError(pub String);

impl From<ValidationError> for DomainError {
    fn from(e: ValidationError) -> Self {
        DomainError::Validation(e.0)
    }
}

impl From<serde_json::Error> for DomainError {
    fn from(e: serde_json::Error) -> Self {
        DomainError::External(e.to_string())
    }
}

impl From<String> for DomainError {
    fn from(e: String) -> Self {
        DomainError::External(e)
    }
}

/// trait: Bus
#[async_trait]
pub trait Bus: Send + Sync {
    async fn dispatch(&self, evt: serde_json::Value) -> Result<(), DomainError>;
    async fn invoke(&self, cmd: serde_json::Value) -> Result<serde_json::Value, DomainError>;
    async fn request(&self, qry: serde_json::Value) -> Result<serde_json::Value, DomainError>;
}

/// trait: EntityRepo
#[async_trait]
pub trait EntityRepo<T>: Send + Sync
where
    T: Send + Sync + 'static,
{
    async fn find(&self, id: Uuid) -> Result<Option<T>, DomainError>;
    async fn list_by_tenant(&self, tenant_id: Uuid) -> Result<Vec<T>, DomainError>;
    async fn save(&self, entity: T) -> Result<(), DomainError>;
    async fn delete(&self, id: Uuid) -> Result<(), DomainError>;
}

/// trait: AuthService
#[async_trait]
pub trait AuthService: Send + Sync {
    async fn validate_token(&self, token: String) -> Result<Principal, DomainError>;
    async fn check_permission(
        &self,
        principal: Principal,
        permission: String,
    ) -> Result<bool, DomainError>;
}

/// trait: SagaStep
#[async_trait]
pub trait SagaStep: Send + Sync {
    async fn action(
        &self,
        bus: &(dyn Bus + Send + Sync),
        state: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError>;
    async fn compensate(
        &self,
        bus: &(dyn Bus + Send + Sync),
        state: serde_json::Value,
    ) -> Result<(), DomainError>;
}

// ─── InProcessBus (local harness, RT-001 / RT-004) ─────────────────────────
// Methods generated from the layer-declared routing trait surface.
use futures::FutureExt;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;

type BusHandler = Arc<
    dyn Fn(serde_json::Value) -> BoxFuture<'static, Result<serde_json::Value, DomainError>>
        + Send
        + Sync,
>;

/// In-process message bus for local multi-context runs.
#[derive(Clone, Default)]
pub struct InProcessBus {
    handlers: Arc<std::sync::Mutex<HashMap<String, BusHandler>>>,
}

impl InProcessBus {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Register a handler for a message type name (manifest `handlers` keys).
    pub fn register<F, Fut>(&self, name: impl Into<String>, f: F)
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, DomainError>> + Send + 'static,
    {
        let name = name.into();
        let handler: BusHandler = Arc::new(move |v| f(v).boxed());
        self.handlers
            .lock()
            .expect("bus lock")
            .insert(name, handler);
    }

    fn lookup(&self, type_name: &str) -> Option<BusHandler> {
        self.handlers
            .lock()
            .expect("bus lock")
            .get(type_name)
            .cloned()
    }
}

#[async_trait]
impl Bus for InProcessBus {
    async fn dispatch(&self, evt: serde_json::Value) -> Result<(), DomainError> {
        let type_name = evt
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(handler) = self.lookup(&type_name) {
            let payload = evt.clone();
            tokio::spawn(async move {
                let _ = handler(payload).await;
            });
        }
        Ok(())
    }

    async fn invoke(&self, cmd: serde_json::Value) -> Result<serde_json::Value, DomainError> {
        let type_name = cmd
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let handler = self.lookup(&type_name).ok_or(DomainError::NotFound)?;
        handler(cmd).await
    }

    async fn request(&self, qry: serde_json::Value) -> Result<serde_json::Value, DomainError> {
        let type_name = qry
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let handler = self.lookup(&type_name).ok_or(DomainError::NotFound)?;
        handler(qry).await
    }
}
/// Dev/local AuthService — allows all tokens and permissions (RT-008).
/// Host harnesses replace this with Cognito/Auth0/etc. via `provided_by: runtime`.
pub struct AllowAllAuth;

#[async_trait]
impl AuthService for AllowAllAuth {
    async fn validate_token(&self, token: String) -> Result<Principal, DomainError> {
        Ok(Principal {
            id: if token.is_empty() {
                "anonymous".into()
            } else {
                token
            },
            roles: vec!["local".into()],
            claims: std::collections::HashMap::new(),
        })
    }

    async fn check_permission(
        &self,
        _principal: Principal,
        _permission: String,
    ) -> Result<bool, DomainError> {
        Ok(true)
    }
}
/// Layer-provided struct: Principal
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Principal {
    pub id: String,
    pub roles: Vec<String>,
    pub claims: std::collections::HashMap<String, String>,
}

/// Layer-provided struct: ToolDefinition
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Layer-declared coordinator.
pub async fn unwind(
    bus: &(dyn Bus + Send + Sync),
    steps: &[Box<dyn SagaStep + Send + Sync>],
    upto: i64,
    state: serde_json::Value,
) -> Result<(), DomainError> {
    let mut i = upto;
    while i > 0 {
        i = i - 1;
        steps[(i) as usize]
            .compensate(bus.clone(), state.clone())
            .await?;
    }
    return Ok(());
}

/// Layer-declared coordinator.
pub async fn run_saga(
    bus: &(dyn Bus + Send + Sync),
    steps: &[Box<dyn SagaStep + Send + Sync>],
) -> Result<(), DomainError> {
    let mut state = serde_json::json!({});
    let mut i = 0;
    while i < (steps.len() as i64) {
        match steps[(i) as usize].action(bus.clone(), state.clone()).await {
            Ok(next) => {
                state = next;
                i = i + 1;
            }
            Err(e) => {
                unwind(bus.clone(), steps.clone(), i, state.clone()).await?;
                return Err(e);
            }
        };
    }
    return Ok(());
}
