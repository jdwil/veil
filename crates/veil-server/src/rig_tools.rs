//! VEIL agent tools for the Rig SDK (AGT-005 / AGT-006).
//!
//! Tools operate on an in-memory workspace snapshot. When a
//! [`LiveWriter`] is attached, each successful edit is flushed to the
//! host immediately (and SSE revision events fire) so the IDE badge
//! updates mid-turn — not only after the model finishes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};
use veil_ir::{check_solution, build_ir_with_registry, LayerRegistry};
use veil_parser::TokenKind;

use crate::agent::AgentToolCall;
use crate::provider::{FileInfo, FileKind};

/// Callback to persist source mid-turn (async, host-owned).
pub type LiveWriter = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Host project ops that mirror IDE front-end capabilities (files, select, …).
#[async_trait]
pub trait AgentHost: Send + Sync {
    async fn list_files(&self) -> Vec<FileInfo>;
    async fn create_file(
        &self,
        name: &str,
        kind: Option<&str>,
        content: Option<String>,
    ) -> Result<crate::file_ops::CreatedFile, String>;
    async fn select_file(&self, index: usize) -> Result<(), String>;
    async fn read_active_source(&self) -> Result<String, String>;
    /// Active file layer registry (async so multi-project can re-scope).
    async fn registry(&self) -> LayerRegistry;
    async fn reload_from_disk(&self) -> Result<usize, String>;
    /// Project root for dual-loop / generated tree tools (AGT-020+).
    fn project_root(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// Multi-project name when known.
    fn project_name(&self) -> Option<String> {
        None
    }
}

/// Shared mutable workspace for a single agent turn.
#[derive(Clone)]
pub struct Workspace {
    pub source: Arc<Mutex<String>>,
    pub registry: Arc<Mutex<LayerRegistry>>,
    pub source_changed: Arc<AtomicBool>,
    pub tool_log: Arc<Mutex<Vec<AgentToolCall>>>,
    pub confirm_writes: bool,
    /// When set, edits flush to the SourceProvider immediately.
    pub live_writer: Option<LiveWriter>,
    /// Project host (create/list/select files). Optional for unit tests.
    pub host: Option<Arc<dyn AgentHost>>,
}

impl Workspace {
    pub fn new(source: String, registry: LayerRegistry, confirm_writes: bool) -> Self {
        Self {
            source: Arc::new(Mutex::new(source)),
            registry: Arc::new(Mutex::new(registry)),
            source_changed: Arc::new(AtomicBool::new(false)),
            tool_log: Arc::new(Mutex::new(Vec::new())),
            confirm_writes,
            live_writer: None,
            host: None,
        }
    }

    pub fn with_live_writer(mut self, writer: LiveWriter) -> Self {
        self.live_writer = Some(writer);
        self
    }

    pub fn with_host(mut self, host: Arc<dyn AgentHost>) -> Self {
        self.host = Some(host);
        self
    }

    pub fn registry_snapshot(&self) -> LayerRegistry {
        self.registry
            .lock()
            .map(|r| r.clone())
            .unwrap_or_else(|_| LayerRegistry::builtin())
    }

    async fn apply_source(&self, new_src: String) -> Result<(), String> {
        if let Ok(mut g) = self.source.lock() {
            *g = new_src.clone();
        }
        self.source_changed.store(true, Ordering::SeqCst);
        if let Some(ref w) = self.live_writer {
            w(new_src).await?;
        }
        Ok(())
    }

    /// Switch workspace snapshot after host selects/creates a different file.
    async fn adopt_active_file(&self) -> Result<(), String> {
        let Some(host) = &self.host else {
            return Err("no project host attached".into());
        };
        let src = host.read_active_source().await?;
        let reg = host.registry().await;
        if let Ok(mut g) = self.source.lock() {
            *g = src;
        }
        if let Ok(mut r) = self.registry.lock() {
            *r = reg;
        }
        Ok(())
    }

    fn log(&self, name: &str, detail: impl Into<String>) {
        if let Ok(mut log) = self.tool_log.lock() {
            log.push(AgentToolCall {
                name: name.into(),
                detail: detail.into(),
            });
        }
    }

    pub fn take_log(&self) -> Vec<AgentToolCall> {
        self.tool_log.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
    }

    pub fn source_snapshot(&self) -> String {
        self.source.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn changed(&self) -> bool {
        self.source_changed.load(Ordering::SeqCst)
    }
}

fn parse_source(source: &str, registry: &LayerRegistry) -> Result<veil_ir::Solution, String> {
    let tokens = veil_parser::lex(source);
    veil_parser::parse_with_registry(&tokens, registry.clone())
        .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
}

// ─── check ─────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolErr(pub String);

#[derive(Deserialize, Serialize, Default)]
pub struct EmptyArgs {}

#[derive(Clone)]
pub struct CheckTool {
    pub ws: Workspace,
}

impl Tool for CheckTool {
    const NAME: &'static str = "veil_check";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Run the VEIL dual-loop check pipeline. Returns summary + JSON diagnostics [{ code, severity, message, span?, hint? }]. Prefer fixing by code+span after any edit.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("veil_check", "target=rust");
        let src = self.ws.source_snapshot();
        let reg = self.ws.registry_snapshot();
        Ok(run_check(&src, &reg))
    }
}

// ─── outline ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct OutlineTool {
    pub ws: Workspace,
}

impl Tool for OutlineTool {
    const NAME: &'static str = "veil_outline";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Return a compact IR construct outline (topology) for the active package. Use for navigation before editing.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("veil_outline", "outline");
        let src = self.ws.source_snapshot();
        let reg = self.ws.registry_snapshot();
        Ok(run_outline(&src, &reg))
    }
}

// ─── read_source ───────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize, Default)]
pub struct ReadArgs {
    /// Optional max characters (default 8000).
    #[serde(default)]
    pub max_chars: Option<usize>,
}

#[derive(Clone)]
pub struct ReadSourceTool {
    pub ws: Workspace,
}

impl Tool for ReadSourceTool {
    const NAME: &'static str = "read_source";
    type Error = ToolErr;
    type Args = ReadArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Read the active .veil source text (truncated if large). Prefer outline + check for overview.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_chars": {
                        "type": "integer",
                        "description": "Max characters to return (default 8000)"
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let max = args.max_chars.unwrap_or(8000);
        self.ws.log("read_source", format!("max_chars={max}"));
        let src = self.ws.source_snapshot();
        if src.len() <= max {
            Ok(src)
        } else {
            Ok(format!(
                "{}…\n\n[truncated {} / {} chars]",
                &src[..max],
                max,
                src.len()
            ))
        }
    }
}

// ─── rename ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
pub struct RenameArgs {
    pub from: String,
    pub to: String,
    /// Set true when user confirmed (or when confirm mode is off).
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Clone)]
pub struct RenameTool {
    pub ws: Workspace,
}

impl Tool for RenameTool {
    const NAME: &'static str = "rename_construct";
    type Error = ToolErr;
    type Args = RenameArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Rename a construct by name via structured EditOp (preferred over raw text rewrite). Requires confirmed=true when write confirmation is enabled.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "Current construct name" },
                    "to": { "type": "string", "description": "New construct name" },
                    "confirmed": { "type": "boolean", "description": "User confirmed the write" }
                },
                "required": ["from", "to"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if self.ws.confirm_writes && !args.confirmed {
            self.ws.log("permission_check", "confirm required");
            return Ok(format!(
                "Write blocked (VEIL_AGENT_CONFIRM_WRITES). Ask the user to confirm rename '{}' → '{}', then call again with confirmed=true.",
                args.from, args.to
            ));
        }
        self.ws.log(
            "rename_construct",
            format!("{} → {}", args.from, args.to),
        );
        let src = self.ws.source_snapshot();
        let reg = self.ws.registry_snapshot();
        match apply_rename(&src, &reg, &args.from, &args.to) {
            Ok((new_src, summary)) => {
                if let Err(e) = self.ws.apply_source(new_src.clone()).await {
                    return Err(ToolErr(format!("live write failed: {e}")));
                }
                let check = run_check(&new_src, &reg);
                self.ws.log("veil_check", "post-rename");
                Ok(format!("{summary}\n\n{check}"))
            }
            Err(e) => Err(ToolErr(e)),
        }
    }
}

// ─── list_files ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ListFilesTool {
    pub ws: Workspace,
}

impl Tool for ListFilesTool {
    const NAME: &'static str = "list_files";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List packages/layers in the IDE project (same as the file picker). Use before create_file or select_file.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("list_files", "list");
        let Some(host) = &self.ws.host else {
            return Err(ToolErr("list_files: no project host".into()));
        };
        let files = host.list_files().await;
        if files.is_empty() {
            return Ok("No files loaded in this project.".into());
        }
        let mut lines = vec!["files:".to_string()];
        for f in &files {
            let mark = if f.active { " ●" } else { "" };
            let kind = f.kind.as_str();
            let adapts = f
                .adapts
                .as_ref()
                .map(|a| format!(" adapts:{a}"))
                .unwrap_or_default();
            lines.push(format!(
                "  [{idx}] {name} ({kind}){mark}{adapts}",
                idx = f.index,
                name = f.name,
            ));
        }
        Ok(lines.join("\n"))
    }
}

// ─── select_file ───────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
pub struct SelectFileArgs {
    /// File index from list_files, or basename (e.g. `wear_test.veil`).
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone)]
pub struct SelectFileTool {
    pub ws: Workspace,
}

impl Tool for SelectFileTool {
    const NAME: &'static str = "select_file";
    type Error = ToolErr;
    type Args = SelectFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Switch the active IDE file (same as the file picker). Subsequent tools operate on this file. Pass index from list_files or name.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "description": "File index from list_files" },
                    "name": { "type": "string", "description": "Basename e.g. client.veil" }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let Some(host) = &self.ws.host else {
            return Err(ToolErr("select_file: no project host".into()));
        };
        let files = host.list_files().await;
        let idx = if let Some(i) = args.index {
            i
        } else if let Some(ref name) = args.name {
            files
                .iter()
                .find(|f| f.name == *name || f.name.trim_end_matches(".veil") == name.as_str())
                .map(|f| f.index)
                .ok_or_else(|| ToolErr(format!("no file named '{name}'")))?
        } else {
            return Err(ToolErr("select_file requires index or name".into()));
        };
        self.ws.log("select_file", format!("index={idx}"));
        host.select_file(idx)
            .await
            .map_err(ToolErr)?;
        self.ws
            .adopt_active_file()
            .await
            .map_err(ToolErr)?;
        let name = files
            .iter()
            .find(|f| f.index == idx)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| format!("#{idx}"));
        Ok(format!("Active file is now {name}. Use read_source / veil_check / rename_construct on it."))
    }
}

// ─── create_file ───────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
pub struct CreateFileArgs {
    /// Basename or stem: `AcmeWear`, `AcmeWear.veil`, or `wear_test.layer`.
    pub name: String,
    /// `package` (default) or `layer`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Optional full starter source; default is a minimal pkg/layer scaffold.
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub confirmed: bool,
}

#[derive(Clone)]
pub struct CreateFileTool {
    pub ws: Workspace,
}

impl Tool for CreateFileTool {
    const NAME: &'static str = "create_file";
    type Error = ToolErr;
    type Args = CreateFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Create a new package (.veil) or layer (.layer) in the project (same as the IDE + button). Writes to disk, registers, and selects it as the active file.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "File stem or name.ext" },
                    "kind": { "type": "string", "enum": ["package", "layer"], "description": "package (default) or layer" },
                    "content": { "type": "string", "description": "Optional full file body" },
                    "confirmed": { "type": "boolean", "description": "User confirmed when write confirmation is on" }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if self.ws.confirm_writes && !args.confirmed {
            self.ws.log("permission_check", "confirm create_file");
            return Ok(format!(
                "Write blocked (VEIL_AGENT_CONFIRM_WRITES). Confirm create_file name='{}', then call again with confirmed=true.",
                args.name
            ));
        }
        let Some(host) = &self.ws.host else {
            return Err(ToolErr("create_file: no project host".into()));
        };
        self.ws.log(
            "create_file",
            format!(
                "{} kind={}",
                args.name,
                args.kind.as_deref().unwrap_or("package")
            ),
        );
        let created = host
            .create_file(&args.name, args.kind.as_deref(), args.content)
            .await
            .map_err(ToolErr)?;
        self.ws
            .adopt_active_file()
            .await
            .map_err(ToolErr)?;
        self.ws.source_changed.store(true, Ordering::SeqCst);
        Ok(format!(
            "Created {} ({}) at {} — now active. Edit with rename_construct or write_source; run veil_check when ready.",
            created.name,
            created.kind.as_str(),
            created.path
        ))
    }
}

// ─── write_source ──────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
pub struct WriteSourceArgs {
    /// Full file body for the active package/layer.
    pub content: String,
    #[serde(default)]
    pub confirmed: bool,
    /// Construct name → short why (PR Wizard).
    #[serde(default)]
    pub rationales: Option<std::collections::HashMap<String, String>>,
    /// Package-level why when rationales omitted.
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Clone)]
pub struct WriteSourceTool {
    pub ws: Workspace,
}

impl Tool for WriteSourceTool {
    const NAME: &'static str = "write_source";
    type Error = ToolErr;
    type Args = WriteSourceArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Replace the entire active file source. Prefer rename_construct for renames. Pass rationales: {ConstructName: short why} for the PR Wizard.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "Full new source text" },
                    "rationales": {
                        "type": "object",
                        "description": "constructName → short intent/why for PR Wizard",
                        "additionalProperties": { "type": "string" }
                    },
                    "rationale": { "type": "string", "description": "Package-level why" },
                    "confirmed": { "type": "boolean" }
                },
                "required": ["content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if self.ws.confirm_writes && !args.confirmed {
            self.ws.log("permission_check", "confirm write_source");
            return Ok(
                "Write blocked (VEIL_AGENT_CONFIRM_WRITES). Confirm with user, then call write_source again with confirmed=true."
                    .into(),
            );
        }
        let mut rats = args.rationales.unwrap_or_default();
        if let Some(one) = args.rationale {
            if !one.trim().is_empty() {
                rats.entry("*".into()).or_insert_with(|| one.trim().to_string());
            }
        }
        if !rats.is_empty() {
            crate::api::record_rationales(rats);
        }
        let len = args.content.len();
        self.ws.log("write_source", format!("bytes={len}"));
        if let Err(e) = self.ws.apply_source(args.content.clone()).await {
            return Err(ToolErr(format!("write failed: {e}")));
        }
        let reg = self.ws.registry_snapshot();
        let check = run_check(&args.content, &reg);
        Ok(format!("Wrote {len} bytes to active file.\n\n{check}"))
    }
}

// ─── Runtime observability (AGT-020–028) ───────────────────────────────────

fn host_project(ws: &Workspace) -> Result<(std::path::PathBuf, Option<String>), ToolErr> {
    let host = ws
        .host
        .as_ref()
        .ok_or_else(|| ToolErr("no project host — open a project".into()))?;
    let root = host
        .project_root()
        .ok_or_else(|| ToolErr("no project root".into()))?;
    Ok((root, host.project_name()))
}

#[derive(Deserialize, Serialize, Default)]
pub struct DevStatusArgs {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone)]
pub struct DevStatusTool {
    pub ws: Workspace,
}

impl Tool for DevStatusTool {
    const NAME: &'static str = "dev_status";
    type Error = ToolErr;
    type Args = DevStatusArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Dual-loop target status (ports, running/stopped, last_error).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Optional target filter" }
                }
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, proj) = host_project(&self.ws)?;
        self.ws.log("dev_status", args.name.as_deref().unwrap_or("*"));
        crate::agent_runtime_tools::tool_dev_status(
            &root,
            args.name.as_deref(),
            proj.as_deref(),
        )
        .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct DevLogsArgs {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tail: Option<usize>,
}

#[derive(Clone)]
pub struct DevLogsTool {
    pub ws: Workspace,
}

impl Tool for DevLogsTool {
    const NAME: &'static str = "dev_logs";
    type Error = ToolErr;
    type Args = DevLogsArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Dual-loop gen/check/smoke logs. Use after WRITE REJECTED or 404.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "tail": { "type": "integer" }
                }
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, proj) = host_project(&self.ws)?;
        self.ws.log("dev_logs", args.name.as_deref().unwrap_or("*"));
        crate::agent_runtime_tools::tool_dev_logs(
            &root,
            args.name.as_deref(),
            args.tail,
            proj.as_deref(),
        )
        .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct ReadGeneratedArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub what: Option<String>,
    #[serde(default)]
    pub max_chars: Option<usize>,
    #[serde(default)]
    pub list: bool,
}

#[derive(Clone)]
pub struct ReadGeneratedTool {
    pub ws: Workspace,
}

impl Tool for ReadGeneratedTool {
    const NAME: &'static str = "read_generated";
    type Error = ToolErr;
    type Args = ReadGeneratedArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Read codegen output (what=harness|routes or path under target output).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "what": { "type": "string", "enum": ["harness", "routes"] },
                    "max_chars": { "type": "integer" },
                    "list": { "type": "boolean" }
                }
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, _) = host_project(&self.ws)?;
        self.ws.log(
            "read_generated",
            args.what
                .clone()
                .or(args.path.clone())
                .unwrap_or_else(|| "—".into()),
        );
        crate::agent_runtime_tools::tool_read_generated(
            &root,
            args.path.as_deref(),
            args.what.as_deref(),
            args.max_chars,
            args.list,
        )
        .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct ListRoutesArgs {
    /// `auto` (default) | `generated` | `ir` — ACS-011
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone)]
pub struct ListRoutesTool {
    pub ws: Workspace,
}

impl Tool for ListRoutesTool {
    const NAME: &'static str = "list_routes";
    type Error = ToolErr;
    type Args = ListRoutesArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "JSON routes: source=auto|generated|ir. Prefer generated when present; use ir after bad gen to see @route intent (ACS-011).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "enum": ["auto", "generated", "ir"],
                        "description": "auto (default): generated if present else IR; ir derives from active package"
                    }
                }
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, _) = host_project(&self.ws)?;
        let mode = crate::agent_runtime_tools::ListRoutesSource::parse(args.source.as_deref())
            .map_err(ToolErr)?;
        self.ws.log(
            "list_routes",
            match mode {
                crate::agent_runtime_tools::ListRoutesSource::Auto => "auto",
                crate::agent_runtime_tools::ListRoutesSource::Generated => "generated",
                crate::agent_runtime_tools::ListRoutesSource::Ir => "ir",
            },
        );
        let src = self.ws.source_snapshot();
        let reg = self.ws.registry_snapshot();
        crate::agent_runtime_tools::tool_list_routes_with(
            &root,
            Some(&src),
            Some(&reg),
            mode,
        )
        .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct HttpRequestArgs {
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub struct HttpRequestTool {
    pub ws: Workspace,
}

impl Tool for HttpRequestTool {
    const NAME: &'static str = "http_request";
    type Error = ToolErr;
    type Args = HttpRequestArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "HTTP to local dual-loop ports only (127.0.0.1 + dev_port).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "method": { "type": "string" },
                    "path": { "type": "string" },
                    "target": { "type": "string" },
                    "url": { "type": "string" },
                    "body": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                }
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, _) = host_project(&self.ws)?;
        self.ws.log(
            "http_request",
            format!(
                "{} {}",
                args.method.as_deref().unwrap_or("GET"),
                args.path.as_deref().or(args.url.as_deref()).unwrap_or("/health")
            ),
        );
        crate::agent_runtime_tools::tool_http_request(
            &root,
            args.method.as_deref(),
            args.path.as_deref(),
            args.target.as_deref(),
            args.url.as_deref(),
            args.body.as_deref(),
            args.timeout_ms,
        )
        .await
        .map_err(ToolErr)
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct DevRestartArgs {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone)]
pub struct DevRestartTool {
    pub ws: Workspace,
}

impl Tool for DevRestartTool {
    const NAME: &'static str = "dev_restart";
    type Error = ToolErr;
    type Args = DevRestartArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Restart dual-loop target so cargo run loads new gen.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } }
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, proj) = host_project(&self.ws)?;
        self.ws.log("dev_restart", args.name.as_deref().unwrap_or("*"));
        crate::agent_runtime_tools::tool_dev_restart(
            &root,
            args.name.as_deref(),
            proj.as_deref(),
        )
        .map_err(ToolErr)
    }
}

#[derive(Clone)]
pub struct SmokeStatusTool {
    pub ws: Workspace,
}

impl Tool for SmokeStatusTool {
    const NAME: &'static str = "smoke_status";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Last smoke/check status for dual-loop targets.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }
    }
    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let (root, proj) = host_project(&self.ws)?;
        self.ws.log("smoke_status", "query");
        crate::agent_runtime_tools::tool_smoke_status(&root, proj.as_deref()).map_err(ToolErr)
    }
}

// ─── stubs (catalog / gen / install) ───────────────────────────────────────

#[derive(Clone)]
pub struct StubListTool {
    pub ws: Workspace,
}

impl Tool for StubListTool {
    const NAME: &'static str = "stub_list";
    type Error = ToolErr;
    type Args = EmptyArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "List project + platform .stub catalog (name, version, origin, sparse). Prefer stub_install or stub_gen over hand-writing stubs.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }
    }
    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("stub_list", "catalog");
        let root = self.ws.host.as_ref().and_then(|h| h.project_root());
        Ok(crate::stub_ops::tool_list_text(root.as_deref()))
    }
}

#[derive(Deserialize, Serialize)]
pub struct StubNameArgs {
    /// Crate / stub use-name (e.g. reqwest, aws_sdk_s3).
    pub name: String,
}

#[derive(Clone)]
pub struct StubGetTool {
    pub ws: Workspace,
}

impl Tool for StubGetTool {
    const NAME: &'static str = "stub_get";
    type Error = ToolErr;
    type Args = StubNameArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Resolve a .stub by name (project stubs/ first, then platform catalog). Returns metadata + content.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Crate use-name" }
                },
                "required": ["name"]
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("stub_get", &args.name);
        let root = self.ws.host.as_ref().and_then(|h| h.project_root());
        let mut r = crate::stub_ops::get_stub(root.as_deref(), &args.name).map_err(ToolErr)?;
        r.entry.path = Some(crate::stub_ops::agent_visible_stub_ref(&r.entry));
        serde_json::to_string_pretty(&r).map_err(|e| ToolErr(e.to_string()))
    }
}

#[derive(Deserialize, Serialize)]
pub struct StubGenArgs {
    pub crate_name: String,
    #[serde(default)]
    pub features: Vec<String>,
    /// Write into project stubs/ (default true).
    #[serde(default = "stub_gen_write_default")]
    pub write: bool,
}

fn stub_gen_write_default() -> bool {
    true
}

#[derive(Clone)]
pub struct StubGenTool {
    pub ws: Workspace,
}

impl Tool for StubGenTool {
    const NAME: &'static str = "stub_gen";
    type Error = ToolErr;
    type Args = StubGenArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Generate a .stub from crates.io via rustdoc (veil stub-gen). REQUIRED instead of hand-writing SDK stubs. Writes to project stubs/ by default.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "crate_name": { "type": "string", "description": "Cargo crate name (reqwest, aws-sdk-s3)" },
                    "features": { "type": "array", "items": { "type": "string" } },
                    "write": { "type": "boolean", "description": "Write to project stubs/ (default true)" }
                },
                "required": ["crate_name"]
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("stub_gen", &args.crate_name);
        let root = self.ws.host.as_ref().and_then(|h| h.project_root());
        let r = crate::stub_ops::generate_stub(
            root.as_deref(),
            &args.crate_name,
            &args.features,
            args.write,
        )
        .map_err(ToolErr)?;
        Ok(format!(
            "Generated stub {} @ {} → {}\nnotes: {}\n\n{}",
            r.entry.name,
            r.entry.version,
            crate::stub_ops::agent_visible_stub_ref(&r.entry),
            r.entry.notes.join("; "),
            if r.content.len() > 4000 {
                format!("{}…\n[truncated {} chars]", &r.content[..4000], r.content.len())
            } else {
                r.content
            }
        ))
    }
}

#[derive(Clone)]
pub struct StubInstallTool {
    pub ws: Workspace,
}

impl Tool for StubInstallTool {
    const NAME: &'static str = "stub_install";
    type Error = ToolErr;
    type Args = StubNameArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Copy a platform catalog stub into the project stubs/ directory (pin/override). Prefer this before stub_gen for common SDKs.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Platform stub name" }
                },
                "required": ["name"]
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("stub_install", &args.name);
        let root = self
            .ws
            .host
            .as_ref()
            .and_then(|h| h.project_root())
            .ok_or_else(|| ToolErr("no project root".into()))?;
        let r = crate::stub_ops::install_stub_to_project(&root, &args.name).map_err(ToolErr)?;
        Ok(format!(
            "Installed {} @ {} → {} (use stub_search / stub_get; do not open host disk)",
            r.entry.name,
            r.entry.version,
            crate::stub_ops::agent_visible_stub_ref(&r.entry)
        ))
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct StubSearchArgs {
    pub query: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone)]
pub struct StubSearchTool {
    pub ws: Workspace,
}

impl Tool for StubSearchTool {
    const NAME: &'static str = "stub_search";
    type Error = ToolErr;
    type Args = StubSearchArgs;
    type Output = String;
    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.into(),
            description: "Search any .stub contract for types/methods without dumping the full file. Call those names via @field — never invent aws_sns / http.post.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Method or type substring" },
                    "name": { "type": "string", "description": "Optional crate/stub name" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }),
        }
    }
    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("stub_search", &args.query);
        let root = self.ws.host.as_ref().and_then(|h| h.project_root());
        Ok(crate::stub_ops::tool_search_text(
            root.as_deref(),
            args.name.as_deref(),
            &args.query,
            args.limit.unwrap_or(24) as usize,
        ))
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn looks_like_layer(source: &str) -> bool {
    source.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("construct ") || (t.starts_with("pkg ") && source.contains("\n  construct "))
    }) && !source.contains("\n  ctx ") && !source.contains("\n    group ")
}

/// Decide layer vs package check the same way the IDE does (`FileKind` from
/// the path). Never use `parse_layer_file(src).is_ok()` — that parser is
/// permissive and returns Ok for ordinary `.veil` packages, which made
/// `veil_check` report 0/0 while GET /api/check still showed warnings.
fn check_as_layer(source: &str, kind: Option<FileKind>) -> bool {
    match kind {
        Some(FileKind::Layer) => true,
        Some(FileKind::Package) | Some(FileKind::Stub) => false,
        None => looks_like_layer(source),
    }
}

/// Run check and return **structured JSON** diagnostics (ACS-008).
///
/// Agents should parse the JSON body: `{ ok, error_count, warning_count, diagnostics[] }`
/// where each diagnostic is `{ code, severity, message, span?, hint?, node_name? }`.
/// Prefer fixing by `code` + `span` rather than rewriting whole files.
pub fn run_check(source: &str, registry: &LayerRegistry) -> String {
    run_check_kind(source, registry, None)
}

/// Same as [`run_check`], but honor the active file kind (IDE / MCP).
pub fn run_check_kind(source: &str, registry: &LayerRegistry, kind: Option<FileKind>) -> String {
    if check_as_layer(source, kind) {
        let name = source
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("pkg ")
                    .map(|r| r.split_whitespace().next().unwrap_or("layer").to_string())
            })
            .unwrap_or_else(|| "layer".into());
        let diags = veil_ir::check_layer(source, &name);
        let report = veil_ir::StructuredCheckReport::from_diagnostics(&diags);
        return format_structured_check("layer", &report);
    }
    // Prefer parse errors with spans (do not collapse to a bare string).
    let tokens = veil_parser::lex(source);
    match veil_parser::parse_with_registry(&tokens, registry.clone()) {
        Ok(sol) => {
            let result = check_solution(&sol, registry);
            let report = veil_ir::StructuredCheckReport::from_check_result(&result);
            format_structured_check("package", &report)
        }
        Err(errs) => {
            let diags: Vec<veil_ir::Diagnostic> = errs
                .iter()
                .map(|e| {
                    veil_ir::parse_error_diagnostic(
                        e.message.clone(),
                        e.span.start,
                        e.span.end,
                    )
                })
                .collect();
            let report = veil_ir::StructuredCheckReport::from_diagnostics(&diags);
            format_structured_check("package", &report)
        }
    }
}

fn format_structured_check(kind: &str, report: &veil_ir::StructuredCheckReport) -> String {
    // One human line + JSON body so logs stay scannable and agents get machine fields.
    format!(
        "{kind} {}\n{}",
        report.summary_line(),
        report.to_json_pretty()
    )
}

pub fn run_outline(source: &str, registry: &LayerRegistry) -> String {
    run_outline_kind(source, registry, None)
}

pub fn run_outline_kind(source: &str, registry: &LayerRegistry, kind: Option<FileKind>) -> String {
    if check_as_layer(source, kind) {
        if let Ok(graph) = veil_ir::build_layer_ir(source, "active") {
            let mut lines = vec!["layer outline:".to_string()];
            for n in graph.nodes.iter().filter(|n| {
                matches!(
                    n.kind,
                    veil_ir::NodeKind::TypeDef | veil_ir::NodeKind::Group | veil_ir::NodeKind::Action
                )
            }) {
                let sk = n.metadata.subkind.as_deref().unwrap_or("");
                lines.push(format!("  - {} {}", sk, n.name));
            }
            return lines.join("\n");
        }
    }
    match parse_source(source, registry) {
        Ok(sol) => {
            let graph = build_ir_with_registry(&sol, Some(registry));
            let mut lines = vec!["outline:".to_string()];
            for n in graph.nodes.iter().filter(|n| {
                !matches!(
                    n.kind,
                    veil_ir::NodeKind::Solution
                        | veil_ir::NodeKind::Action
                        | veil_ir::NodeKind::Inputs
                        | veil_ir::NodeKind::Return
                        | veil_ir::NodeKind::Field
                )
            }) {
                let sk = n.metadata.subkind.as_deref().unwrap_or("");
                lines.push(format!(
                    "  - {:?} {} {}",
                    n.kind,
                    if sk.is_empty() { "" } else { sk },
                    n.name
                ));
            }
            lines.join("\n")
        }
        Err(e) => format!("parse error: {e}"),
    }
}

/// Format-preserving rename: patch identifier tokens in the original source.
///
/// Does **not** round-trip through `serialize_solution` (which rewrites layout,
/// drops comments, and reorders members). Validates by re-parsing after patch.
///
/// Replaces every `Ident` token equal to `from` (definition + type references).
/// Compound names like `UserStatus` are untouched.
pub fn apply_rename(
    source: &str,
    registry: &LayerRegistry,
    from: &str,
    to: &str,
) -> Result<(String, String), String> {
    if from.is_empty() || to.is_empty() {
        return Err("rename requires non-empty from/to".into());
    }
    if from == to {
        return Err("from and to are identical".into());
    }
    // Ensure the name exists as a construct (or at least parses as source).
    let sol = parse_source(source, registry)?;
    let graph = build_ir_with_registry(&sol, Some(registry));
    let has_construct = graph.nodes.iter().any(|n| n.name == from);
    if !has_construct {
        // Still allow pure identifier renames if the token exists.
        let tokens = veil_parser::lex(source);
        let any = tokens
            .iter()
            .any(|t| t.kind == TokenKind::Ident && t.text == from);
        if !any {
            return Err(format!("no construct or identifier named '{from}'"));
        }
    }

    let tokens = veil_parser::lex(source);
    // Lexer spans are **char** indices into the source (see veil-parser lexer).
    let mut char_spans: Vec<(usize, usize)> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Ident && t.text == from)
        .map(|t| (t.span.start, t.span.end))
        .collect();
    if char_spans.is_empty() {
        return Err(format!("no identifier token '{from}' in source"));
    }
    char_spans.sort_by_key(|(s, _)| *s);
    char_spans.dedup();

    let mut byte_spans: Vec<(usize, usize)> = Vec::with_capacity(char_spans.len());
    for (cs, ce) in &char_spans {
        let bs = char_index_to_byte(source, *cs);
        let be = char_index_to_byte(source, *ce);
        byte_spans.push((bs, be));
    }

    let mut new_src = source.to_string();
    for (start, end) in byte_spans.iter().rev() {
        if *end > new_src.len() || *start >= *end {
            return Err(format!("invalid rename span {start}..{end}"));
        }
        if &new_src[*start..*end] != from {
            // Defensive fallback for any residual indexing quirks.
            new_src = rename_idents_text(source, from, to)?;
            break;
        }
        new_src.replace_range(*start..*end, to);
    }
    // Must still parse.
    parse_source(&new_src, registry)?;
    let n = char_spans.len();
    Ok((
        new_src,
        format!(
            "Renamed '{from}' → '{to}' (format-preserving, {n} identifier site{}).",
            if n == 1 { "" } else { "s" }
        ),
    ))
}

/// Convert a character index (from the lexer) to a UTF-8 byte offset.
fn char_index_to_byte(source: &str, char_idx: usize) -> usize {
    source
        .char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or_else(|| source.len())
}

/// Word-boundary identifier rename on raw text (byte-safe).
fn rename_idents_text(source: &str, from: &str, to: &str) -> Result<String, String> {
    let bytes = source.as_bytes();
    let from_b = from.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;
    let mut n = 0usize;
    while i < bytes.len() {
        if i + from_b.len() <= bytes.len() && &bytes[i..i + from_b.len()] == from_b {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_i = i + from_b.len();
            let after_ok = after_i >= bytes.len() || !is_ident_byte(bytes[after_i]);
            if before_ok && after_ok {
                out.push_str(to);
                i = after_i;
                n += 1;
                continue;
            }
        }
        // copy one utf-8 char
        let ch = source[i..]
            .chars()
            .next()
            .ok_or_else(|| "invalid utf-8 in source".to_string())?;
        out.push(ch);
        i += ch.len_utf8();
    }
    if n == 0 {
        return Err(format!("no word-boundary match for '{from}'"));
    }
    Ok(out)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use veil_ir::LayerRegistry;

    #[test]
    fn rename_preserves_comments_and_layout() {
        let src = "\
# keep this comment
pkg Demo
  use ddd
  ctx App
    group domain
      agg User
        root
          id: Id
          name: Str
";
        let reg = LayerRegistry::builtin();
        // ddd may be needed — if builtin lacks ddd, use empty parse of simpler source
        let simple = "\
# header stays
pkg Demo
  struct User
    id: Id
  struct Bag
    owner: User
";
        let (out, summary) = apply_rename(simple, &reg, "User", "Account").expect("rename");
        assert!(out.contains("# header stays"), "comment must survive: {out}");
        assert!(out.contains("struct Account"), "{out}");
        assert!(out.contains("owner: Account"), "{out}");
        assert!(!out.contains("struct User"), "{out}");
        assert!(summary.contains("format-preserving"));
        let _ = src; // silence
    }
}

#[cfg(test)]
mod rename_hello {
    use super::*;
    use veil_ir::LayerRegistry;
    #[test]
    fn rename_hello_user() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/hello_world.veil");
        let src = std::fs::read_to_string(&path).expect("read hello_world");
        let reg = LayerRegistry::for_veil_file(&path).unwrap_or_else(|_| LayerRegistry::builtin());
        let (out, sum) = apply_rename(&src, &reg, "User", "AppUser").expect("rename");
        assert!(out.contains("pkg HelloWorld"), "package header lost: {out}");
        assert!(out.contains("agg AppUser"), "{out}");
        assert!(!out.contains("agg User\n"), "{out}");
        assert!(
            out.contains("AppUser.new") || out.contains("AppUser)"),
            "{out}"
        );
        assert!(out.contains("lang"), "lang block lost: {out}");
        eprintln!("{sum}");
    }
}

#[cfg(test)]
mod check_kind_tests {
    use super::*;
    use veil_ir::LayerRegistry;

    #[test]
    fn package_with_ctx_is_not_layer_checked() {
        let src = "\
pkg Demo
  use ddd
  ctx App
    group infrastructure
      adapter AwsSns for Sns
        impl publish
          aws_sns.publish!({ topic: \"t\" })
";
        // Trap: the layer parser is permissive and accepts ordinary packages.
        assert!(
            veil_ir::parse_layer_file(src, "active").is_ok(),
            "document the trap: parse_layer_file must not gate veil_check"
        );
        let out = run_check(src, &LayerRegistry::builtin());
        assert!(
            out.starts_with("package "),
            "veil_check must use the package pipeline, got: {out}"
        );
        let as_layer = run_check_kind(src, &LayerRegistry::builtin(), Some(FileKind::Layer));
        assert!(as_layer.starts_with("layer "), "{as_layer}");
        let as_pkg = run_check_kind(src, &LayerRegistry::builtin(), Some(FileKind::Package));
        assert!(as_pkg.starts_with("package "), "{as_pkg}");
        assert!(
            as_pkg.contains("escape_external_call")
                || as_pkg.contains("unresolved_external")
                || as_pkg.contains("parse_error"),
            "package check must surface the aws_sns call, got: {as_pkg}"
        );
    }
}
