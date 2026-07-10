//! veil-server — shared dev server logic for the VEIL visual editor.
//!
//! This crate provides the HTTP API that the veil-viewer connects to.
//! It's parameterized by a [`SourceProvider`] trait so the same API works
//! whether the source lives on the local filesystem (veil-cli) or in a
//! remote VCS (veil-runtime).
//!
//! # Usage
//!
//! ```rust,ignore
//! use veil_server::{build_router, FilesystemProvider};
//!
//! let provider = FilesystemProvider::new("path/to/app.veil");
//! let app = build_router(provider);
//! // serve with axum...
//! ```

pub mod provider;
pub mod api;
pub mod protocol;
pub mod agent;
pub mod agent_stream;
pub mod model;
pub mod agent_context;
pub mod rig_tools;
pub mod safety;
pub mod revision;
pub mod acp;
pub mod layer_edit;

pub use provider::{FileInfo, FileKind, SourceProvider};
pub use provider::filesystem::FilesystemProvider;
pub use provider::remote::RemoteHttpProvider;
pub use api::build_router;
pub use agent::{run_turn, AgentTurnRequest, AgentTurnResponse};
pub use model::{
    complete_with_env, ChatMessage, CompleteRequest, CompleteResponse, ModelConfig, ModelProvider,
};
