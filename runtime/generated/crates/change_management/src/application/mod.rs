//! Application services and flow functions.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::messages::*;
use crate::domain::types::*;
use crate::ports::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Injected dependencies (ports).
pub struct Deps {
    pub approval_repo: std::sync::Arc<dyn ApprovalRepo + Send + Sync>,
    pub cr_repo: std::sync::Arc<dyn ChangeRequestRepo + Send + Sync>,
    pub ci_repo: std::sync::Arc<dyn CiRunRepo + Send + Sync>,
    pub comment_repo: std::sync::Arc<dyn CommentRepo + Send + Sync>,
    pub git: std::sync::Arc<dyn GitService + Send + Sync>,
}

/// DomainService: CreateChangeRequest
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn create_change_request(
    deps: &Deps,
    repo_id: Uuid,
    slug: String,
    title: String,
    description: String,
    jira_ticket: String,
    author: String,
) -> Result<ChangeRequest, DomainError> {
    // step: validate
    if !((jira_ticket.len() as i64) > 0) {
        return Err(DomainError::Validation("Jira ticket required".to_string()));
    };
    if !((title.len() as i64) > 0) {
        return Err(DomainError::Validation("Title required".to_string()));
    };

    // step: create_branch
    let branch_name = format!(
        "cr/{}/{}",
        jira_ticket,
        title.to_lowercase().replace(" ", "-")
    );
    deps.git
        .create_branch(slug.clone(), branch_name.clone(), "main".to_string())
        .await?;

    // step: persist
    let now = Utc::now();
    let id = Uuid::new_v4();
    let cr = ChangeRequest {
        id: id.clone(),
        repo_id: repo_id.clone(),
        title: title.clone(),
        description: description.clone(),
        jira_ticket: jira_ticket.clone(),
        source_branch: branch_name.clone(),
        target_branch: "main".to_string(),
        author: author.clone(),
        status: PrStatus::Draft.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
        merged_at: None,
        merged_by: None,
        merge_commit: None,
    };
    deps.cr_repo.save(cr.clone()).await?;
    return Ok(cr);
}

/// DomainService: CommitToChange
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn commit_to_change(
    deps: &Deps,
    pr_id: Uuid,
    slug: String,
    path: String,
    content: String,
    message: String,
    author: String,
) -> Result<serde_json::Value, DomainError> {
    // step: resolve
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let pr = cr.clone().ok_or(DomainError::NotFound)?;
    if !(pr.status != PrStatus::Merged) {
        return Err(DomainError::Validation(
            "Cannot commit to merged PR".to_string(),
        ));
    };
    if !(pr.status != PrStatus::Rejected) {
        return Err(DomainError::Validation(
            "Cannot commit to rejected PR".to_string(),
        ));
    };

    // step: commit
    let hash = deps
        .git
        .commit_file(
            slug.clone(),
            pr.source_branch.clone(),
            path.clone(),
            content.clone(),
            format!("[{}] {}", pr.jira_ticket, message),
            author.clone(),
        )
        .await?;
    return Ok(
        serde_json::json!({ "hash": hash.clone(), "branch": serde_json::json!(pr.clone())["source_branch"].clone() }),
    );
}

/// DomainService: SubmitForReview
/// @dep
#[tracing::instrument(skip_all)]
pub async fn submit_for_review(deps: &Deps, pr_id: Uuid) -> Result<(), DomainError> {
    // step: transition
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let mut pr = cr.clone().ok_or(DomainError::NotFound)?;
    if !(pr.status == PrStatus::Draft || pr.status == PrStatus::ChangesRequested) {
        return Err(DomainError::Validation(
            "PR not in submittable state".to_string(),
        ));
    };
    pr.status = PrStatus::ReadyForReview;
    pr.updated_at = Utc::now();
    deps.cr_repo.save(pr.clone()).await?;

    Ok(())
}

/// DomainService: ApproveChange
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn approve_change(
    deps: &Deps,
    pr_id: Uuid,
    reviewer: String,
    comment: Option<String>,
) -> Result<(), DomainError> {
    // step: validate
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let mut pr = cr.clone().ok_or(DomainError::NotFound)?;
    if !(pr.status == PrStatus::ReadyForReview) {
        return Err(DomainError::Validation(
            "PR not in reviewable state".to_string(),
        ));
    };
    if !(reviewer != pr.author) {
        return Err(DomainError::Validation(
            "Author cannot approve their own change".to_string(),
        ));
    };

    // step: record
    let approval = Approval {
        pr_id: pr_id.clone(),
        reviewer: reviewer.clone(),
        approved_at: Utc::now(),
        comment: comment.clone(),
    };
    deps.approval_repo.save(approval.clone()).await?;
    pr.status = PrStatus::Approved;
    pr.updated_at = Utc::now();
    deps.cr_repo.save(pr.clone()).await?;

    Ok(())
}

/// DomainService: RequestChanges
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn request_changes(
    deps: &Deps,
    pr_id: Uuid,
    reviewer: String,
    comment: String,
) -> Result<(), DomainError> {
    // step: validate
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let mut pr = cr.clone().ok_or(DomainError::NotFound)?;
    if !(pr.status == PrStatus::ReadyForReview) {
        return Err(DomainError::Validation(
            "PR not in reviewable state".to_string(),
        ));
    };
    if !(reviewer != pr.author) {
        return Err(DomainError::Validation(
            "Author cannot review their own change".to_string(),
        ));
    };

    // step: record
    let review_comment = ReviewComment {
        id: Uuid::new_v4(),
        pr_id: pr_id.clone(),
        author: reviewer.clone(),
        construct_path: None,
        body: comment.clone(),
        created_at: Utc::now(),
        resolved: false,
    };
    deps.comment_repo.save(review_comment.clone()).await?;
    pr.status = PrStatus::ChangesRequested;
    pr.updated_at = Utc::now();
    deps.cr_repo.save(pr.clone()).await?;

    Ok(())
}

/// DomainService: MergeChange
/// @dep
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn merge_change(
    deps: &Deps,
    pr_id: Uuid,
    merger: String,
    slug: String,
) -> Result<serde_json::Value, DomainError> {
    // step: validate_gates
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let mut pr = cr.clone().ok_or(DomainError::NotFound)?;
    if !(pr.status == PrStatus::Approved) {
        return Err(DomainError::Validation(
            "PR must be approved before merge".to_string(),
        ));
    };
    let ci = deps.ci_repo.latest_for_pr(pr_id.clone()).await?;
    if ci.is_none() {
        return Err(DomainError::Validation(
            "No CI run — build must pass before merge".to_string(),
        ));
    };
    if !(ci.clone().ok_or(DomainError::NotFound)?.status == CiStatus::Passed) {
        return Err(DomainError::Validation(
            "CI must pass before merge".to_string(),
        ));
    };
    let approvals = deps.approval_repo.find_for_pr(pr_id.clone()).await?;
    if !(!approvals.is_empty()) {
        return Err(DomainError::Validation(
            "At least one approval required".to_string(),
        ));
    };

    // step: merge
    pr.status = PrStatus::Merging;
    deps.cr_repo.save(pr.clone()).await?;
    let merge_msg = format!(
        "Merge {}: {} [{}]",
        pr.source_branch, pr.title, pr.jira_ticket
    );
    let merge_hash = deps
        .git
        .merge(
            slug.clone(),
            pr.source_branch.clone(),
            pr.target_branch.clone(),
            merge_msg.clone(),
            merger.clone(),
        )
        .await?;

    // step: finalize
    pr.status = PrStatus::Merged;
    pr.merged_at = Some(Utc::now());
    pr.merged_by = Some(merger);
    pr.merge_commit = Some(merge_hash.clone());
    pr.updated_at = Utc::now();
    deps.cr_repo.save(pr.clone()).await?;
    return Ok(
        serde_json::json!({ "merge_commit": merge_hash.clone(), "branch": serde_json::json!(pr.clone())["source_branch"].clone(), "target": serde_json::json!(pr.clone())["target_branch"].clone() }),
    );
}

/// DomainService: GetStructuralDiff
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn get_structural_diff(
    deps: &Deps,
    pr_id: Uuid,
    slug: String,
) -> Result<serde_json::Value, DomainError> {
    // step: resolve
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let pr = cr.clone().ok_or(DomainError::NotFound)?;

    // step: compute_diff
    let file_diffs = deps
        .git
        .diff_files(
            slug.clone(),
            pr.target_branch.clone(),
            pr.source_branch.clone(),
        )
        .await?;
    return Ok(serde_json::Value::Object(serde_json::Map::new()));
}

/// DomainService: ComputeStructuralDiffFromSource
#[tracing::instrument(skip_all)]
pub async fn compute_structural_diff_from_source(
    base_content: String,
    branch_content: String,
    base_label: String,
    head_label: String,
) -> Result<serde_json::Value, DomainError> {
    // step: parse_and_diff
    return Ok(serde_json::Value::Object(serde_json::Map::new()));
}

/// DomainService: AddComment
/// @dep
#[tracing::instrument(skip_all)]
pub async fn add_comment(
    deps: &Deps,
    pr_id: Uuid,
    author: String,
    construct_path: Option<String>,
    body: String,
) -> Result<ReviewComment, DomainError> {
    // step: persist
    let comment = ReviewComment {
        id: Uuid::new_v4(),
        pr_id: pr_id.clone(),
        author: author.clone(),
        construct_path: construct_path.clone(),
        body: body.clone(),
        created_at: Utc::now(),
        resolved: false,
    };
    deps.comment_repo.save(comment.clone()).await?;
    return Ok(comment);
}

/// DomainService: ListChangeRequests
/// @dep
#[tracing::instrument(skip_all)]
pub async fn list_change_requests(
    deps: &Deps,
    repo_id: Option<Uuid>,
    status: Option<PrStatus>,
) -> Result<Vec<ChangeRequest>, DomainError> {
    // step: query
    if repo_id.is_none() {
        return Ok(vec![]);
    };
    let filter = status.clone();
    let items = deps
        .cr_repo
        .list_by_repo(
            repo_id.clone().ok_or(DomainError::NotFound)?,
            filter.clone(),
        )
        .await?;
    return Ok(items);
}

/// DomainService: GetChangeRequest
/// @dep
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn get_change_request(
    deps: &Deps,
    pr_id: Uuid,
) -> Result<serde_json::Value, DomainError> {
    // step: load
    let cr = deps.cr_repo.find(pr_id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let pr = cr.clone().ok_or(DomainError::NotFound)?;
    let approvals = deps.approval_repo.find_for_pr(pr_id.clone()).await?;
    let comments = deps.comment_repo.list_for_pr(pr_id.clone()).await?;
    let ci_runs = deps.ci_repo.list_for_pr(pr_id.clone()).await?;
    return Ok(
        serde_json::json!({ "pr": pr.clone(), "approvals": approvals.clone(), "comments": comments.clone(), "ci_runs": ci_runs.clone() }),
    );
}
