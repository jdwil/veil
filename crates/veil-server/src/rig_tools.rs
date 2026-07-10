//! VEIL agent tools for the Rig SDK (AGT-005 / AGT-006).
//!
//! Tools operate on an in-memory workspace snapshot. When a
//! [`LiveWriter`] is attached, each successful edit is flushed to the
//! host immediately (and SSE revision events fire) so the IDE badge
//! updates mid-turn — not only after the model finishes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};
use veil_ir::{check_solution, build_ir_with_registry, LayerRegistry};
use veil_parser::TokenKind;

use crate::agent::AgentToolCall;

/// Callback to persist source mid-turn (async, host-owned).
pub type LiveWriter = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Shared mutable workspace for a single agent turn.
#[derive(Clone)]
pub struct Workspace {
    pub source: Arc<Mutex<String>>,
    pub registry: LayerRegistry,
    pub source_changed: Arc<AtomicBool>,
    pub tool_log: Arc<Mutex<Vec<AgentToolCall>>>,
    pub confirm_writes: bool,
    /// When set, edits flush to the SourceProvider immediately.
    pub live_writer: Option<LiveWriter>,
}

impl Workspace {
    pub fn new(source: String, registry: LayerRegistry, confirm_writes: bool) -> Self {
        Self {
            source: Arc::new(Mutex::new(source)),
            registry,
            source_changed: Arc::new(AtomicBool::new(false)),
            tool_log: Arc::new(Mutex::new(Vec::new())),
            confirm_writes,
            live_writer: None,
        }
    }

    pub fn with_live_writer(mut self, writer: LiveWriter) -> Self {
        self.live_writer = Some(writer);
        self
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
                if let Err(e) = self.ws.apply_source(new_src.clone()).await {
                    return Err(ToolErr(format!("live write failed: {e}")));
                }
                let check = run_check(&new_src, &self.ws.registry);
                self.ws.log("veil_check", "post-rename");
                Ok(format!("{summary}\n\n{check}"))
            }
            Err(e) => Err(ToolErr(e)),
        }
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn looks_like_layer(source: &str) -> bool {
    source.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("construct ") || (t.starts_with("pkg ") && source.contains("\n  construct "))
    }) && !source.contains("\n  ctx ") && !source.contains("\n    group ")
}

pub fn run_check(source: &str, registry: &LayerRegistry) -> String {
    // Layer files (DSL-003 / DSL-011)
    if looks_like_layer(source) || veil_ir::parse_layer_file(source, "active").is_ok() {
        let name = source
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("pkg ")
                    .map(|r| r.split_whitespace().next().unwrap_or("layer").to_string())
            })
            .unwrap_or_else(|| "layer".into());
        let diags = veil_ir::check_layer(source, &name);
        let errs = diags
            .iter()
            .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
            .count();
        let warns = diags
            .iter()
            .filter(|d| matches!(d.severity, veil_ir::Severity::Warning))
            .count();
        let mut lines = vec![format!(
            "layer check: {} error(s), {} warning(s) — {}",
            errs,
            warns,
            if errs > 0 { "FAIL" } else { "OK" }
        )];
        for d in diags.iter().take(12) {
            lines.push(format!("  [{:?}] {}", d.severity, d.message));
        }
        return lines.join("\n");
    }
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
    if let Ok(graph) = veil_ir::build_layer_ir(source, "active") {
        if graph.nodes.iter().any(|n| n.metadata.subkind.as_deref() == Some("Layer"))
            || veil_ir::parse_layer_file(source, "active").is_ok()
        {
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
        assert!(out.contains("# Hello World"), "comment lost: {out}");
        assert!(out.contains("agg AppUser"), "{out}");
        assert!(!out.contains("agg User\n"), "{out}");
        assert!(out.contains("AppUser.new") || out.contains("AppUser)"), "{out}");
        eprintln!("{sum}");
    }
}
