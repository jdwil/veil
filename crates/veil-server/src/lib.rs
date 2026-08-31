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

pub mod acp;
pub mod aether_chat;
pub mod agent;
pub mod agent_context;
pub mod agent_runtime_tools;
pub mod agent_scope;
pub mod agent_stream;
pub mod api;
pub mod chat_attachments;
pub mod chat_log;
pub mod coding_gates;
pub mod coding_orchestrator;
pub mod coding_resolve;
pub mod config;
pub mod devloop;
pub mod devloop_api;
pub mod edit_capture;
pub mod file_ops;
pub mod focus;
pub mod git_origin;
pub mod git_provider;
pub mod layer_edit;
pub mod layer_ops;
pub mod mcp;
pub mod mind_palace_tools;
pub mod model;
pub mod platform_tools;
pub mod pr_writeback;
pub mod product_host;
pub mod project_layout;
pub mod protocol;
pub mod provider;
pub mod reference_fs;
pub mod review;
pub mod revision;
pub mod rig_tools;
pub mod safety;
pub mod session;
pub mod session_api;
pub mod stub_ops;

pub use api::{build_multi_router, build_router, ide_routes};
pub use config::{
    VeilConfig, complete_first_run, config_path, ensure_config, ensure_config_interactive,
    ensure_projects_dir_exists, is_noninteractive, load_config, load_config_or_default,
    local_catalog_path, needs_first_run, platform_local, resolve_projects_dir, save_config,
    set_projects_dir, suggested_projects_dir, veil_home_dir,
};
pub use product_host::{ProductHost, resolve_ui_dir};
pub use project_layout::{
    ActiveProjectInfo, InitOptions, MISSION_MAX_INJECT_CHARS, ProjectInfo, collect_project_files,
    create_project, create_project_with_opts, ensure_project_shape, ensure_projects_dir,
    has_package_sources, init_project, is_core_platform_layer, list_projects, mission_md_template,
    project_display_name, read_mission_for_agent,
};
pub use provider::filesystem::FilesystemProvider;
pub use provider::hub::{MultiProjectProvider, ProjectsHub};
pub use provider::remote::RemoteHttpProvider;
pub use provider::{FileInfo, FileKind, SourceProvider};

/// Projects directory: env → config.json → ~/veil-projects.
pub fn default_projects_dir() -> std::path::PathBuf {
    resolve_projects_dir()
}
pub use agent::{AgentTurnRequest, AgentTurnResponse, run_turn};
pub use model::{
    ChatMessage, CompleteRequest, CompleteResponse, ModelConfig, ModelProvider, complete_with_env,
};
