use std::sync::Arc;

use acr_core::ir::Algorithm;
use acr_core::value::Value;
use acr_library::metadata::{AlgorithmEntry, LibraryQuery, PromotionStatus};
use acr_library::store::AlgorithmStore;

use crate::error::RuntimeError;

/// A goal describes what the runtime needs to accomplish.
#[derive(Debug, Clone)]
pub struct Goal {
    pub description: String,
    pub domain: String,
    pub tags: Vec<String>,
    pub input: Vec<Value>,
}

/// Selects algorithms from the promoted library based on a goal.
pub struct AlgorithmSelector;

impl AlgorithmSelector {
    /// Select relevant algorithms from the library for a given goal.
    ///
    /// v0 strategy: filter by domain and promoted status, then rank by tag overlap
    /// and historical score. Returns up to 5 best matches.
    pub async fn select(
        store: &Arc<dyn AlgorithmStore>,
        goal: &Goal,
    ) -> Result<Vec<Algorithm>, RuntimeError> {
        // Query for promoted algorithms in the goal's domain.
        // We pass empty tags here so the store doesn't require ALL tags to match;
        // we do our own tag-overlap scoring below.
        let query = LibraryQuery {
            domain: Some(goal.domain.clone()),
            status: Some(PromotionStatus::Promoted),
            tags: Vec::new(),
            name_contains: None,
        };

        let entries = store.list(&query).await?;

        if entries.is_empty() {
            return Err(RuntimeError::NoAlgorithmFound(goal.description.clone()));
        }

        // Score entries by tag overlap
        let mut scored: Vec<(AlgorithmEntry, usize)> = entries
            .into_iter()
            .map(|entry| {
                let tag_overlap = entry
                    .tags
                    .iter()
                    .filter(|t| goal.tags.contains(t))
                    .count();
                (entry, tag_overlap)
            })
            .collect();

        // Sort by tag overlap descending, then by average score descending
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                let avg_a = average_score(&a.0);
                let avg_b = average_score(&b.0);
                avg_b
                    .partial_cmp(&avg_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        // Load top algorithms (max 5 for v0)
        let mut algorithms = Vec::new();
        for (entry, _) in scored.into_iter().take(5) {
            match store.get(entry.id).await {
                Ok(alg) => algorithms.push(alg),
                Err(_) => continue,
            }
        }

        if algorithms.is_empty() {
            return Err(RuntimeError::NoAlgorithmFound(goal.description.clone()));
        }

        Ok(algorithms)
    }
}

fn average_score(entry: &AlgorithmEntry) -> f64 {
    if entry.score_history.is_empty() {
        return 0.0;
    }
    let sum: f64 = entry.score_history.iter().map(|s| s.score).sum();
    sum / entry.score_history.len() as f64
}
