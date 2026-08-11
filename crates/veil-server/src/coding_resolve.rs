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

/// Minimum score for a strong auto-match (token + phrase + branch heuristic).
const STRONG: f64 = 0.32;
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
    /// How the score was composed (debug / agent transparency).
    pub score_parts: Vec<String>,
}

pub fn is_open_status(status: &str) -> bool {
    let s = status.trim();
    if s.is_empty() {
        return true; // unknown → treat as open
    }
    !CLOSED.iter().any(|c| c.eq_ignore_ascii_case(s))
}

/// Tokenize request / PR text for cheap scope scoring.
/// Splits on non-alphanumeric (including `-`); keeps `_` for diagnostic codes.
pub fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
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

/// Score request against PR title/description (and optional branch/project).
/// Returns (score 0..=1, human-readable parts for gate transparency).
pub fn score_request_against(request: &str, title: &str, description: &str) -> f64 {
    score_request_detailed(request, title, description, None, None).0
}

pub fn score_request_detailed(
    request: &str,
    title: &str,
    description: &str,
    source_branch: Option<&str>,
    project: Option<&str>,
) -> (f64, Vec<String>) {
    let mut parts = Vec::new();
    let req_raw = request.trim().to_lowercase();
    let title_l = title.to_lowercase();
    let desc_l = description.to_lowercase();
    let hay_text = format!("{title_l}\n{desc_l}");

    let req = tokens(request);
    if req.is_empty() && req_raw.len() < 4 {
        return (0.0, parts);
    }

    // 1) Unigram token overlap
    let hay: std::collections::HashSet<String> = tokens(title)
        .into_iter()
        .chain(tokens(description))
        .collect();
    let mut unigram = 0.0;
    if !req.is_empty() && !hay.is_empty() {
        let hit = req.iter().filter(|t| hay.contains(*t)).count();
        unigram = hit as f64 / req.len() as f64;
        parts.push(format!("tokens={unigram:.2}"));
    }

    // 2) Title token density
    let title_toks: std::collections::HashSet<_> = tokens(title).into_iter().collect();
    let mut title_boost = 0.0;
    if !req.is_empty() && !title_toks.is_empty() {
        let title_hit = req.iter().filter(|t| title_toks.contains(*t)).count();
        title_boost =
            0.18 * (title_hit as f64 / title_toks.len().min(req.len()).max(1) as f64);
        if title_boost > 0.02 {
            parts.push(format!("title={title_boost:.2}"));
        }
    }

    // 3) Bigram phrases (consecutive tokens)
    let mut bigram = 0.0;
    if req.len() >= 2 {
        let mut hits = 0usize;
        let mut total = 0usize;
        for w in req.windows(2) {
            total += 1;
            let phrase = format!("{} {}", w[0], w[1]);
            if hay_text.contains(&phrase) {
                hits += 1;
            }
        }
        if total > 0 {
            bigram = 0.22 * (hits as f64 / total as f64);
            if bigram > 0.02 {
                parts.push(format!("bigrams={bigram:.2}"));
            }
        }
    }

    // 4) Substring / diagnostic-code style tokens (type_mismatch, EntityRepo)
    let mut substr = 0.0;
    let special: Vec<&str> = request
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(str::trim)
        .filter(|s| s.len() >= 4)
        .filter(|s| s.contains('_') || s.contains('-') || s.chars().any(|c| c.is_ascii_uppercase()))
        .take(12)
        .collect();
    if !special.is_empty() {
        let mut hit = 0usize;
        for s in &special {
            let sl = s.to_lowercase();
            if hay_text.contains(&sl) || title_l.contains(&sl) {
                hit += 1;
            }
        }
        substr = 0.2 * (hit as f64 / special.len() as f64);
        if substr > 0.02 {
            parts.push(format!("codes={substr:.2}"));
        }
    }

    // 5) Branch name overlap (fix-reqwest ↔ "fix reqwest")
    let mut branch_boost = 0.0;
    if let Some(br) = source_branch {
        let br = br.trim();
        if !br.is_empty() && br != "main" && br != "master" {
            let br_toks = tokens(br);
            if !br_toks.is_empty() && !req.is_empty() {
                let hit = br_toks
                    .iter()
                    .filter(|t| req.iter().any(|r| r.as_str() == t.as_str()))
                    .count();
                if hit > 0 {
                    branch_boost = 0.2 * (hit as f64 / br_toks.len() as f64);
                    // Floor so a single meaningful branch token still moves the needle
                    branch_boost = branch_boost.max(0.12);
                    parts.push(format!("branch={branch_boost:.2}"));
                }
            }
            // exact branch mentioned in request
            if req_raw.contains(&br.to_lowercase()) {
                branch_boost = (branch_boost + 0.25).min(0.45);
                parts.push("branch_exact".into());
            }
        }
    }

    // 6) Project slug mentioned
    let mut project_boost = 0.0;
    if let Some(p) = project {
        let pl = p.trim().to_lowercase();
        if !pl.is_empty() && (req_raw.contains(&pl) || hay_text.contains(&pl)) {
            project_boost = 0.08;
            parts.push("project".into());
        }
    }

    // 7) Full request contained in description (strong continuation)
    let mut contain = 0.0;
    if req_raw.len() >= 12 && (desc_l.contains(&req_raw) || title_l.contains(&req_raw)) {
        contain = 0.35;
        parts.push("contained".into());
    }

    let score = (unigram * 0.55
        + title_boost
        + bigram
        + substr
        + branch_boost
        + project_boost
        + contain)
        .min(1.0);
    parts.push(format!("total={score:.2}"));
    (score, parts)
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
        let (score, score_parts) = score_request_detailed(
            request,
            &title,
            &description,
            source_branch.as_deref(),
            project.as_deref().or(project_filter),
        );
        out.push(PrCandidate {
            id,
            title,
            description,
            status,
            source_branch,
            project,
            score,
            score_parts,
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
        "score_parts": c.score_parts,
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
        let (score, parts) = score_request_detailed(
            "fix reqwest stubs agent-registry",
            "Fix reqwest stubs",
            "agent-registry warnings",
            Some("fix-stubs"),
            Some("agent-registry"),
        );
        assert!(score >= STRONG, "score={score} parts={parts:?}");
        let c = vec![PrCandidate {
            id: "pr1".into(),
            title: "Fix reqwest stubs".into(),
            description: "agent-registry warnings".into(),
            status: "Draft".into(),
            source_branch: Some("fix-stubs".into()),
            project: Some("agent-registry".into()),
            score,
            score_parts: parts,
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
                score_parts: vec![],
            },
            PrCandidate {
                id: "b".into(),
                title: "Fix auth tokens".into(),
                description: "".into(),
                status: "Draft".into(),
                source_branch: None,
                project: None,
                score: 0.38,
                score_parts: vec![],
            },
        ];
        assert_eq!(
            decide(&c, "fix authentication"),
            ResolveDecision::NeedsChoice
        );
    }

    #[test]
    fn branch_name_boosts_score() {
        let (with_br, parts) = score_request_detailed(
            "keep going on reqwest stubs",
            "Misc work",
            "ongoing notes",
            Some("reqwest-stubs-hardening"),
            None,
        );
        let (no_br, _) = score_request_detailed(
            "keep going on reqwest stubs",
            "Misc work",
            "ongoing notes",
            None,
            None,
        );
        assert!(
            with_br > no_br,
            "with={with_br} no={no_br} parts={parts:?}"
        );
    }

    #[test]
    fn bigram_helps_phrase_match() {
        let s = score_request_against(
            "entity repository type params",
            "Entity repository fixes",
            "type params for constructs",
        );
        assert!(s > 0.25, "score={s}");
    }
}
