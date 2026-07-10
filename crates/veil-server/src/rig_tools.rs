//! VEIL agent tools for the Rig SDK (AGT-005 / AGT-006).
//!
//! Tools operate on an in-memory workspace snapshot; the host loop persists
//! writes through [`SourceProvider`] after the turn.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};
use veil_ir::{apply_edits_with, check_solution, build_ir_with_registry, EditOp, LayerRegistry};

use crate::agent::AgentToolCall;

/// Shared mutable workspace for a single agent turn.
#[derive(Clone)]
pub struct Workspace {
    pub source: Arc<Mutex<String>>,
    pub registry: LayerRegistry,
    pub source_changed: Arc<AtomicBool>,
    pub tool_log: Arc<Mutex<Vec<AgentToolCall>>>,
    pub confirm_writes: bool,
}

impl Workspace {
    pub fn new(source: String, registry: LayerRegistry, confirm_writes: bool) -> Self {
        Self {
            source: Arc::new(Mutex::new(source)),
            registry,
            source_changed: Arc::new(AtomicBool::new(false)),
            tool_log: Arc::new(Mutex::new(Vec::new())),
            confirm_writes,
        }
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
pub struct ToolErr(String);

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
            description: "Run the VEIL dual-loop check pipeline (parse, validate, types, escape hatches) on the active package. Prefer this after any edit.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.ws.log("veil_check", "target=rust");
        let src = self.ws.source_snapshot();
        Ok(run_check(&src, &self.ws.registry))
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
        Ok(run_outline(&src, &self.ws.registry))
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
        match apply_rename(&src, &self.ws.registry, &args.from, &args.to) {
            Ok((new_src, summary)) => {
                if let Ok(mut g) = self.ws.source.lock() {
                    *g = new_src.clone();
                }
                self.ws.source_changed.store(true, Ordering::SeqCst);
                let check = run_check(&new_src, &self.ws.registry);
                self.ws.log("veil_check", "post-rename");
                Ok(format!("{summary}\n\n{check}"))
            }
            Err(e) => Err(ToolErr(e)),
        }
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

pub fn run_check(source: &str, registry: &LayerRegistry) -> String {
    match parse_source(source, registry) {
        Ok(sol) => {
            let result = check_solution(&sol, registry);
            let errs = result.error_count();
            let warns = result.warning_count();
            let mut lines = vec![format!(
                "check: {} error(s), {} warning(s) — {}",
                errs,
                warns,
                if result.has_errors() { "FAIL" } else { "OK" }
            )];
            for d in result.diagnostics.iter().take(12) {
                lines.push(format!(
                    "  [{:?}] {}{}",
                    d.severity,
                    d.message,
                    d.node_name
                        .as_ref()
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default()
                ));
            }
            if result.diagnostics.len() > 12 {
                lines.push(format!("  … +{} more", result.diagnostics.len() - 12));
            }
            lines.join("\n")
        }
        Err(e) => format!("parse error: {e}"),
    }
}

pub fn run_outline(source: &str, registry: &LayerRegistry) -> String {
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

pub fn apply_rename(
    source: &str,
    registry: &LayerRegistry,
    from: &str,
    to: &str,
) -> Result<(String, String), String> {
    let sol = parse_source(source, registry)?;
    let graph = build_ir_with_registry(&sol, Some(registry));
    let node = graph
        .nodes
        .iter()
        .find(|n| n.name == from)
        .ok_or_else(|| format!("no construct named '{from}'"))?;
    let ops = vec![EditOp::Rename {
        span_start: node.span.start,
        name: to.to_string(),
    }];
    let mut sol2 = sol;
    apply_edits_with(&mut sol2, &ops, |s| {
        veil_parser::parse_expr_str(s, registry).map_err(|e| e.to_string())
    })
    .map_err(|e| e.to_string())?;
    let new_src = veil_ir::serialize_solution(&sol2);
    Ok((
        new_src,
        format!("Renamed '{from}' → '{to}' via EditOp::Rename."),
    ))
}
