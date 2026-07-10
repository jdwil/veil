//! Protocol types — request/response shapes shared with the viewer.

use serde::{Deserialize, Serialize};
use veil_ir::EditOp;

/// Request body for `POST /api/edit`.
#[derive(Debug, Deserialize)]
pub struct EditRequest {
    pub edits: Vec<EditOp>,
}

/// Response for a successful edit.
#[derive(Debug, Serialize)]
pub struct EditResponse {
    pub source: String,
    pub ir: serde_json::Value,
    pub generated: serde_json::Value,
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
