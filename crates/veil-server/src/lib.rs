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
pub mod aether_chat;
pub mod mind_palace_tools;
pub mod model;
pub mod agent_context;
pub mod rig_tools;
pub mod file_ops;
pub mod safety;
pub mod revision;
pub mod acp;
pub mod mcp;
pub mod devloop;
pub mod devloop_api;
pub mod layer_edit;
pub mod project_layout;
pub mod config;
pub mod product_host;

pub use provider::{FileInfo, FileKind, SourceProvider};
pub use provider::filesystem::FilesystemProvider;
pub use provider::remote::RemoteHttpProvider;
pub use api::{build_multi_router, build_router, ide_routes};
pub use provider::hub::{MultiProjectProvider, ProjectsHub};
pub use project_layout::{
    collect_project_files, create_project, create_project_with_opts, ensure_project_shape,
    ensure_projects_dir, has_package_sources, init_project, is_core_platform_layer, list_projects,
    project_display_name, ActiveProjectInfo, InitOptions, ProjectInfo,
};
pub use config::{
    complete_first_run, config_path, ensure_config, ensure_config_interactive,
    ensure_projects_dir_exists, is_noninteractive, load_config, load_config_or_default,
    needs_first_run, resolve_projects_dir, save_config, set_projects_dir, suggested_projects_dir,
    veil_home_dir, VeilConfig,
};
pub use product_host::{resolve_static_dir, ProductHost};

/// Projects directory: env → config.json → ~/veil-projects.
pub fn default_projects_dir() -> std::path::PathBuf {
    resolve_projects_dir()
}
pub use agent::{run_turn, AgentTurnRequest, AgentTurnResponse};
pub use model::{
    complete_with_env, ChatMessage, CompleteRequest, CompleteResponse, ModelConfig, ModelProvider,
};
