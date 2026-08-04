use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A library entry tracking an algorithm's metadata, versions, and scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgorithmEntry {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub tags: Vec<String>,
    pub status: PromotionStatus,
    pub current_version: u32,
    pub versions: Vec<VersionInfo>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub score_history: Vec<ScoreRecord>,
}

/// Promotion lifecycle status for an algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PromotionStatus {
    Candidate,
    Promoted,
    Retired,
}

/// Information about a specific version of an algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub change_summary: String,
}

/// A recorded evaluation score for an algorithm version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreRecord {
    pub version: u32,
    pub score: f64,
    pub task_id: String,
    pub evaluated_at: DateTime<Utc>,
}

/// Query parameters for filtering library entries.
#[derive(Debug, Clone, Default)]
pub struct LibraryQuery {
    pub domain: Option<String>,
    pub tags: Vec<String>,
    pub status: Option<PromotionStatus>,
    pub name_contains: Option<String>,
}
