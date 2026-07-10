//! Protocol types — request/response shapes shared with the viewer.

use serde::{Deserialize, Serialize};
use veil_ir::EditOp;

/// Request body for `POST /api/edit`.
#[derive(Debug, Serialize, Deserialize)]
pub struct EditRequest {
    pub edits: Vec<EditOp>,
}

/// Response for a successful edit.
#[derive(Debug, Serialize)]
pub struct EditResponse {
    pub source: String,
    pub ir: serde_json::Value,
    pub generated: serde_json::Value,
    /// Fresh diagnostics after the edit (same pipeline as `/api/check`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<veil_ir::Diagnostic>>,
}

/// Response for `GET|POST /api/check` — full check pipeline result.
#[derive(Debug, Serialize)]
pub struct CheckResponse {
    pub diagnostics: Vec<veil_ir::Diagnostic>,
    pub error_count: usize,
    pub warning_count: usize,
    pub target: String,
    pub escape_hatch: veil_ir::EscapeHatchSummary,
    /// True when any diagnostic has severity Error.
    pub ok: bool,
}

/// Optional body for `POST /api/check`.
#[derive(Debug, Default, Deserialize)]
pub struct CheckRequest {
    /// Codegen target for capability checks (`rust`, `typescript`). Default: rust.
    #[serde(default)]
    pub target: Option<String>,
    /// Promote escape-hatch debt to errors.
    #[serde(default)]
    pub deny_escape_hatches: bool,
    /// Include multi-target debt warnings when target is rust (default false —
    /// primary-target only; use `true` or `?target_debt=1` to chase portability).
    #[serde(default)]
    pub target_debt: bool,
}

/// Request to switch active file.
#[derive(Debug, Deserialize)]
pub struct SelectFileRequest {
    pub index: usize,
}

/// File listing entry.
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub index: usize,
    pub name: String,
    pub path: String,
    pub editable: bool,
    pub active: bool,
}
