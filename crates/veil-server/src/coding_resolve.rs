//! Resolve coding work onto an open unmerged pull request (or create new).
//!
//! Product language: **PR**, not Change Request. API ids remain `change_request`.
//!
//! Auto-bind when a single strong scope match exists; Present modal only when
//! multiple open candidates are plausible (or match scores are close).

use serde_json::{json, Value};

use crate::session::SessionHandle;

/// Statuses treated as closed — never reuse.
const CLOSED: &[&str] = &["Merged", "Rejected", "Closed", "merged", "rejected", "closed"];

/// Minimum score for a strong auto-match (token overlap heuristic).
const STRONG: f64 = 0.28;
/// Scores within this delta of the best are "close" → prefer modal if >1.
const CLOSE_DELTA: f64 = 0.08;

#[derive(Debug, Clone)]
pub struct PrCandidate {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub source_branch: Option<String>,
    pub project: Option<String>,
    pub score: f64,
}

pub fn is_open_status(status: &str) -> bool {
    let s = status.trim();
    if s.is_empty() {
        return true; // unknown → treat as open
    }
    !CLOSED.iter().any(|c| c.eq_ignore_ascii_case(s))
}

/// Tokenize request / PR text for cheap scope scoring.
pub fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|t| t.len() >= 3)
        .filter(|t| {
            !matches!(
                *t,
                "the"
                    | "and"
                    | "for"
                    | "with"
                    | "from"
                    | "this"
                    | "that"
                    | "into"
                    | "project"
                    | "agent"
                    | "fix"
                    | "pull"
                    | "request"
                    | "branch"
            )
        })
        .map(|t| t.to_string())
        .collect()
}

pub fn score_request_against(request: &str, title: &str, description: &str) -> f64 {
    let req = tokens(request);
    if req.is_empty() {
        return 0.0;
    }
    let hay: std::collections::HashSet<String> = tokens(title)
        .into_iter()
        .chain(tokens(description))
        .collect();
    if hay.is_empty() {
        return 0.0;
    }
    let mut hit = 0usize;
    for t in &req {
        if hay.contains(t) {
            hit += 1;
        }
    }
    // Title-only bonus
    let title_toks: std::collections::HashSet<_> = tokens(title).into_iter().collect();
    let mut title_hit = 0usize;
    for t in &req {
        if title_toks.contains(t) {
            title_hit += 1;
        }
    }
    let base = hit as f64 / req.len() as f64;
    let title_boost = if !title_toks.is_empty() {
        0.15 * (title_hit as f64 / title_toks.len().min(req.len()).max(1) as f64)
    } else {
        0.0
    };
    (base + title_boost).min(1.0)
}

/// Parse list payload from GET /api/change_requests into open candidates.
pub fn candidates_from_list(data: &Value, project_filter: Option<&str>, request: &str) -> Vec<PrCandidate> {
    let arr = if let Some(a) = data.as_array() {
        a.clone()
    } else if let Some(a) = data
        .get("change_requests")
        .or_else(|| data.get("pull_requests"))
        .or_else(|| data.get("items"))
        .and_then(|v| v.as_array())
    {
        a.clone()
    } else {
        vec![]
    };

    let mut out = Vec::new();
    for item in arr {
        let status = item
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !is_open_status(&status) {
            continue;
        }
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = item
            .get("description")
            .or_else(|| item.get("body"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let source_branch = item
            .get("source_branch")
            .or_else(|| item.get("branch"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let project = item
            .get("slug")
            .or_else(|| item.get("project"))
            .or_else(|| item.get("repo_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(pf) = project_filter {
            if let Some(ref p) = project {
                if !p.eq_ignore_ascii_case(pf) && !pf.is_empty() {
                    // Also allow title/desc mentioning project
                    let blob = format!("{title} {description}").to_lowercase();
                    if !blob.contains(&pf.to_lowercase()) {
                        continue;
                    }
                }
            }
        }
        let score = score_request_against(request, &title, &description);
        out.push(PrCandidate {
            id,
            title,
            description,
            status,
            source_branch,
            project,
            score,
        });
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveDecision {
    /// No open PR — start fresh (new branch when multi-step; PR at finish).
    New,
    /// Bind this open PR.
    Bind { pr_id: String },
    /// Operator must choose.
    NeedsChoice,
}

pub fn decide(candidates: &[PrCandidate], request: &str) -> ResolveDecision {
    if candidates.is_empty() {
        return ResolveDecision::New;
    }
    // Explicit "new PR" / "new pull request" language
    let lower = request.to_lowercase();
    if lower.contains("new pr")
        || lower.contains("new pull request")
        || lower.contains("separate pr")
        || lower.contains("fresh branch")
    {
        return ResolveDecision::New;
    }

    let best = &candidates[0];
    let strong = best.score >= STRONG;
    let close_seconds = candidates
        .iter()
        .skip(1)
        .filter(|c| best.score - c.score <= CLOSE_DELTA && c.score >= STRONG * 0.7)
        .count();

    if candidates.len() == 1 {
        if strong || request_is_continue_style(request) {
            return ResolveDecision::Bind {
                pr_id: best.id.clone(),
            };
        }
        // Single open PR but weak/empty scope match — prefer new unless continue-ish
        if best.title.trim().is_empty() && best.description.trim().is_empty() {
            return ResolveDecision::NeedsChoice;
        }
        if best.score < 0.12 {
            return ResolveDecision::New;
        }
        return ResolveDecision::NeedsChoice;
    }

    // Multiple open PRs
    if strong && close_seconds == 0 {
        return ResolveDecision::Bind {
            pr_id: best.id.clone(),
        };
    }
    ResolveDecision::NeedsChoice
}

fn request_is_continue_style(request: &str) -> bool {
    let l = request.to_lowercase();
    l.contains("continue")
        || l.contains("same pr")
        || l.contains("this pr")
        || l.contains("same branch")
        || l.contains("keep going")
        || l.contains("more on")
}

/// Bind session to PR id (+ optional branch name memory).
pub fn bind_session_to_pr(h: &SessionHandle, pr_id: &str) -> Result<(), String> {
    h.set_active_change_id(Some(pr_id))
}

pub fn candidate_json(c: &PrCandidate) -> Value {
    json!({
        "id": c.id,
        "title": c.title,
        "status": c.status,
        "source_branch": c.source_branch,
        "project": c.project,
        "score": (c.score * 1000.0).round() / 1000.0,
        "description_preview": c.description.chars().take(160).collect::<String>(),
    })
}

fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{t:x}")
}

/// Present intent for disambiguation modal.
pub fn choose_pr_intent(request: &str, candidates: &[PrCandidate], project: Option<&str>) -> Value {
    let options: Vec<Value> = candidates
        .iter()
        .take(8)
        .map(|c| {
            json!({
                "id": c.id,
                "label": if c.title.is_empty() { c.id.clone() } else { c.title.clone() },
                "detail": format!(
                    "{}{}score={:.2}",
                    c.source_branch.as_deref().map(|b| format!("branch {b} · ")).unwrap_or_default(),
                    c.status.as_str(),
                    c.score
                ),
                "source_branch": c.source_branch,
            })
        })
        .chain(std::iter::once(json!({
            "id": "__new__",
            "label": "Create new pull request",
            "detail": "Start a fresh work line (new branch / PR at task end)",
            "source_branch": null,
        })))
        .collect();

    let intent_id = format!("intent_choose_pr_{}", short_id());
    json!({
        "type": "ChooseCodingTarget",
        "id": intent_id,
        "actor": "agent",
        "payload": {
            "request": request,
            "project": project,
            "options": options,
        },
        "domain": { "mode": "ux", "done": false },
        "present": {
            "announce": "Which pull request should this apply to?",
            "steps": [
                {
                    "kind": "choose",
                    "title": "Which pull request?",
                    "message": "Apply this work to an open unmerged PR, or create a new one.",
                    "options": options,
                    "ms": 120_000
                }
            ]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_statuses_filtered() {
        assert!(!is_open_status("Merged"));
        assert!(!is_open_status("closed"));
        assert!(is_open_status("ReadyForReview"));
        assert!(is_open_status("Draft"));
    }

    #[test]
    fn score_prefers_title_overlap() {
        let s = score_request_against(
            "fix reqwest stub warnings in agent-registry",
            "Fix reqwest stubs agent-registry",
            "install stubs",
        );
        assert!(s > 0.3, "score was {s}");
    }

    #[test]
    fn decide_empty_is_new() {
        assert_eq!(decide(&[], "fix stuff"), ResolveDecision::New);
    }

    #[test]
    fn decide_strong_single_binds() {
        let c = vec![PrCandidate {
            id: "pr1".into(),
            title: "Fix reqwest stubs".into(),
            description: "agent-registry warnings".into(),
            status: "Draft".into(),
            source_branch: Some("fix-stubs".into()),
            project: Some("agent-registry".into()),
            score: score_request_against(
                "fix reqwest stubs agent-registry",
                "Fix reqwest stubs",
                "agent-registry warnings",
            ),
        }];
        match decide(&c, "fix reqwest stubs agent-registry") {
            ResolveDecision::Bind { pr_id } => assert_eq!(pr_id, "pr1"),
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    #[test]
    fn decide_two_close_needs_choice() {
        let c = vec![
            PrCandidate {
                id: "a".into(),
                title: "Fix auth middleware".into(),
                description: "".into(),
                status: "Draft".into(),
                source_branch: None,
                project: None,
                score: 0.4,
            },
            PrCandidate {
                id: "b".into(),
                title: "Fix auth tokens".into(),
                description: "".into(),
                status: "Draft".into(),
                source_branch: None,
                project: None,
                score: 0.38,
            },
        ];
        assert_eq!(
            decide(&c, "fix authentication"),
            ResolveDecision::NeedsChoice
        );
    }
}
