//! Chat logging — local JSONL files with future remote transport in mind.
//!
//! Every agent turn (prompt + response) is logged to a per-project file under
//! `~/.veil/logs/chat/{project}_{date}.jsonl`. Each line is a self-contained
//! JSON object that can be replayed, searched, or shipped to a remote endpoint.
//!
//! ## Future transport
//!
//! The [`ChatLogger`] trait is async + object-safe. Swap `LocalFileLogger` for a
//! remote implementation (HTTP POST, SQS, Kinesis, etc.) or use a multiplexer
//! that writes locally AND ships to a data center.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

// ─── Log Entry ─────────────────────────────────────────────────────────────

/// One chat turn: prompt in, response out, metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLogEntry {
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Server-assigned turn id.
    pub turn_id: String,
    /// Project name (from serve path or hub).
    pub project: String,
    /// Active file when the turn ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_file: Option<String>,
    /// User prompt (raw text sent to the model).
    pub prompt: String,
    /// Model/agent response text.
    pub response: String,
    /// Tool calls made during the turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallEntry>,
    /// Whether source was modified this turn.
    pub source_changed: bool,
    /// Backend that handled the turn (e.g. "acp-kiro", "rig-openai").
    pub backend: String,
    /// Provider model name (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Duration of the turn in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Whether the turn was aborted.
    #[serde(default)]
    pub aborted: bool,
    /// Error message if the turn failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEntry {
    pub name: String,
    pub detail: String,
}

// ─── Trait ─────────────────────────────────────────────────────────────────

/// Async, object-safe logger. Implementations decide where logs go.
#[async_trait]
pub trait ChatLogger: Send + Sync {
    async fn log_turn(&self, entry: &ChatLogEntry);
}

// ─── Local File Logger ─────────────────────────────────────────────────────

/// Appends JSONL to `~/.veil/logs/chat/{project}_{date}.jsonl`.
pub struct LocalFileLogger {
    logs_dir: PathBuf,
    /// Serialized writes to avoid partial lines.
    lock: Mutex<()>,
}

impl LocalFileLogger {
    /// Create with the default logs directory (`~/.veil/logs/chat`).
    pub fn new() -> Self {
        let logs_dir = crate::config::veil_home_dir().join("logs").join("chat");
        Self {
            logs_dir,
            lock: Mutex::new(()),
        }
    }

    /// Create with a custom directory (for tests).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self {
            logs_dir: dir,
            lock: Mutex::new(()),
        }
    }

    fn log_path(&self, project: &str) -> PathBuf {
        let date = now_date();
        let safe_project = project
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect::<String>();
        self.logs_dir.join(format!("{safe_project}_{date}.jsonl"))
    }
}

#[async_trait]
impl ChatLogger for LocalFileLogger {
    async fn log_turn(&self, entry: &ChatLogEntry) {
        let _guard = self.lock.lock().await;
        let path = self.log_path(&entry.project);
        if let Err(e) = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))) {
            tracing::warn!(error = %e, "failed to create chat log directory");
            return;
        }
        let mut line = match serde_json::to_string(entry) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize chat log entry");
                return;
            }
        };
        line.push('\n');
        if let Err(e) = append_to_file(&path, line.as_bytes()) {
            tracing::warn!(error = %e, path = %path.display(), "failed to write chat log");
        }
    }
}

fn append_to_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(data)?;
    Ok(())
}

// ─── Multiplexer (local + remote) ─────────────────────────────────────────

/// Sends log entries to multiple loggers (e.g. local + remote).
pub struct MultiLogger {
    loggers: Vec<Arc<dyn ChatLogger>>,
}

impl MultiLogger {
    pub fn new(loggers: Vec<Arc<dyn ChatLogger>>) -> Self {
        Self { loggers }
    }
}

#[async_trait]
impl ChatLogger for MultiLogger {
    async fn log_turn(&self, entry: &ChatLogEntry) {
        for logger in &self.loggers {
            logger.log_turn(entry).await;
        }
    }
}

// ─── Global Logger ─────────────────────────────────────────────────────────

use std::sync::OnceLock;

static LOGGER: OnceLock<Arc<dyn ChatLogger>> = OnceLock::new();

/// Initialize the global chat logger. Call once at server startup.
pub fn init_logger() {
    let local = Arc::new(LocalFileLogger::new()) as Arc<dyn ChatLogger>;
    // Future: check env for remote transport config and build MultiLogger.
    // e.g. if let Ok(url) = std::env::var("VEIL_LOG_ENDPOINT") { ... }
    let _ = LOGGER.set(local);
}

/// Get the global logger (no-op if not initialized).
pub fn logger() -> Option<&'static Arc<dyn ChatLogger>> {
    LOGGER.get()
}

/// Convenience: log a turn if the logger is initialized.
pub async fn log_turn(entry: &ChatLogEntry) {
    if let Some(l) = logger() {
        l.log_turn(entry).await;
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

fn now_date() -> String {
    // Use the same approach as now_iso but just the date portion
    let (year, month, day, _, _, _, _) = epoch_to_civil();
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn now_iso() -> String {
    let (year, month, day, hours, mins, secs, ms) = epoch_to_civil();
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}.{ms:03}Z")
}

/// Convert epoch seconds to (year, month, day, hour, min, sec, ms).
/// Proper civil calendar conversion with leap year handling.
fn epoch_to_civil() -> (u64, u64, u64, u64, u64, u64, u64) {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let ms = dur.subsec_millis() as u64;

    let time_secs = total_secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let secs = time_secs % 60;

    // Days since epoch → civil date (algorithm from Howard Hinnant)
    let days = (total_secs / 86400) as i64;
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u64, m as u64, d as u64, hours, mins, secs, ms)
}
