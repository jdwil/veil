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
    pub pr_repo: std::sync::Arc<dyn PullRequestRepo + Send + Sync>,
    pub ci_repo: std::sync::Arc<dyn CiRunRepo + Send + Sync>,
    pub comment_repo: std::sync::Arc<dyn CommentRepo + Send + Sync>,
    pub git: std::sync::Arc<dyn GitService + Send + Sync>,
}

/// DomainService: CreatePullRequest
/// @route
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn create_pull_request(
    deps: &Deps,
    id: Uuid,
    slug: String,
    title: String,
    description: String,
    jira_ticket: String,
    author: String,
) -> Result<PullRequest, DomainError> {
    // step: validate
    if !((title.len() as i64) > 0) {
        return Err(DomainError::Validation("Title required".to_string()));
    };
    let ticket = jira_ticket.trim();
    let ticket_seg = if ticket.is_empty() { "pr" } else { ticket };

    // step: create_branch
    let branch_name = format!(
        "pr/{}/{}",
        ticket_seg,
        title.to_lowercase().replace(" ", "-")
    );
    deps.git
        .create_branch(slug.clone(), branch_name.clone(), "main".to_string())
        .await?;

    // step: persist
    let now = Utc::now();
    let cr_id = Uuid::new_v4();
    let cr = PullRequest {
        id: cr_id.clone(),
        repo_id: id.clone(),
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
    deps.pr_repo.save(cr.clone()).await?;
    return Ok(cr);
}

/// DomainService: CreatePullRequestFlat
/// @route
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn create_pull_request_flat(
    deps: &Deps,
    repo_id: Uuid,
    slug: String,
    title: String,
    description: String,
    jira_ticket: String,
    author: String,
) -> Result<PullRequest, DomainError> {
    // step: validate
    if !((title.len() as i64) > 0) {
        return Err(DomainError::Validation("Title required".to_string()));
    };
    let ticket = jira_ticket.trim();
    let ticket_seg = if ticket.is_empty() { "pr" } else { ticket };

    // step: create_branch
    let branch_name = format!(
        "pr/{}/{}",
        ticket_seg,
        title.to_lowercase().replace(" ", "-")
    );
    deps.git
        .create_branch(slug.clone(), branch_name.clone(), "main".to_string())
        .await?;

    // step: persist
    let now = Utc::now();
    let cr_id = Uuid::new_v4();
    let cr = PullRequest {
        id: cr_id.clone(),
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
    deps.pr_repo.save(cr.clone()).await?;
    return Ok(cr);
}

/// DomainService: CommitToChange
/// @route
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn commit_to_pr(
    deps: &Deps,
    id: Uuid,
    slug: String,
    path: String,
    content: String,
    message: String,
    author: String,
) -> Result<serde_json::Value, DomainError> {
    // step: resolve
    let cr = deps.pr_repo.find(id.clone()).await?;
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
/// @route
/// @dep
#[tracing::instrument(skip_all)]
pub async fn submit_for_review(deps: &Deps, id: Uuid) -> Result<(), DomainError> {
    // step: transition
    let cr = deps.pr_repo.find(id.clone()).await?;
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
    deps.pr_repo.save(pr.clone()).await?;

    Ok(())
}

/// DomainService: ApproveChange
/// @route
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn approve_pr(
    deps: &Deps,
    id: Uuid,
    reviewer: String,
    comment: Option<String>,
) -> Result<(), DomainError> {
    // step: validate
    let cr = deps.pr_repo.find(id.clone()).await?;
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
        pr_id: id.clone(),
        reviewer: reviewer.clone(),
        approved_at: Utc::now(),
        comment: comment.clone(),
    };
    deps.approval_repo.save(approval.clone()).await?;
    pr.status = PrStatus::Approved;
    pr.updated_at = Utc::now();
    deps.pr_repo.save(pr.clone()).await?;

    Ok(())
}

/// DomainService: RequestChanges
/// @route
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn request_pr_changes(
    deps: &Deps,
    id: Uuid,
    reviewer: String,
    comment: String,
) -> Result<(), DomainError> {
    // step: validate
    let cr = deps.pr_repo.find(id.clone()).await?;
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
        pr_id: id.clone(),
        author: reviewer.clone(),
        construct_path: None,
        body: comment.clone(),
        created_at: Utc::now(),
        resolved: false,
    };
    deps.comment_repo.save(review_comment.clone()).await?;
    pr.status = PrStatus::ChangesRequested;
    pr.updated_at = Utc::now();
    deps.pr_repo.save(pr.clone()).await?;

    Ok(())
}

/// DomainService: MergeChange
/// @route
/// @dep
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn merge_pr(
    deps: &Deps,
    id: Uuid,
    merger: String,
    slug: String,
) -> Result<serde_json::Value, DomainError> {
    // step: validate_gates (TRANSPORT ONLY)
    //
    // Approval authority lives in `veil_server::review` (Model B): the recorded
    // human `SignOffRecord` and `review::may_ship` are the sole ship gate, and
    // the HTTP/tool caller enforces `may_ship` *before* calling this function.
    // change_management is git/PR **transport** — it MUST NOT re-decide approval
    // here (no `PrStatus::Approved`, no `Approval` row count). Gating on those
    // would reintroduce the two-source fork this consolidation removed. We only
    // guard the transport lifecycle: the PR must exist and not already be a
    // terminal (merged/rejected/closed) record.
    // See Mind Palace: decision-single-review-source-of-truth.
    let cr = deps.pr_repo.find(id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let mut pr = cr.clone().ok_or(DomainError::NotFound)?;
    if matches!(
        pr.status,
        PrStatus::Merged | PrStatus::Rejected | PrStatus::Closed
    ) {
        return Err(DomainError::Validation(format!(
            "PR is {:?}; cannot merge a terminal transport record",
            pr.status
        )));
    };
    // CI status is transport metadata: if a run exists it must not be Failed,
    // but its presence is NOT a gate (the review sign-off + host check are the
    // authority). A missing run does not block a signed-off merge.
    if let Some(run) = deps.ci_repo.latest_for_pr(id.clone()).await? {
        if run.status == CiStatus::Failed {
            return Err(DomainError::Validation(
                "latest CI run failed — fix the build before merge".to_string(),
            ));
        }
    };

    // step: merge
    pr.status = PrStatus::Merging;
    deps.pr_repo.save(pr.clone()).await?;
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
    deps.pr_repo.save(pr.clone()).await?;
    return Ok(
        serde_json::json!({ "merge_commit": merge_hash.clone(), "branch": serde_json::json!(pr.clone())["source_branch"].clone(), "target": serde_json::json!(pr.clone())["target_branch"].clone() }),
    );
}

/// DomainService: GetStructuralDiff
/// @route
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn get_structural_diff(
    deps: &Deps,
    id: Uuid,
    slug: String,
) -> Result<serde_json::Value, DomainError> {
    // step: resolve
    let cr = deps.pr_repo.find(id.clone()).await?;
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

/// DomainService: AddReviewComment
/// @route
/// @dep
#[tracing::instrument(skip_all)]
pub async fn add_review_comment(
    deps: &Deps,
    id: Uuid,
    author: String,
    construct_path: Option<String>,
    body: String,
) -> Result<ReviewComment, DomainError> {
    // step: persist
    if !((body.len() as i64) > 0) {
        return Err(DomainError::Validation("Comment body required".to_string()));
    };
    let comment = ReviewComment {
        id: Uuid::new_v4(),
        pr_id: id.clone(),
        author: author.clone(),
        construct_path: construct_path.clone(),
        body: body.clone(),
        created_at: Utc::now(),
        resolved: false,
    };
    deps.comment_repo.save(comment.clone()).await?;
    return Ok(comment);
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
    if !((body.len() as i64) > 0) {
        return Err(DomainError::Validation("Comment body required".to_string()));
    };
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

/// DomainService: ListPullRequests
/// @route
/// @dep
#[tracing::instrument(skip_all)]
pub async fn list_pull_requests(
    deps: &Deps,
    id: Uuid,
    status: Option<PrStatus>,
) -> Result<Vec<PullRequest>, DomainError> {
    // step: query
    let filter = status;
    let items = deps
        .pr_repo
        .list_by_repo(id.clone(), filter.clone())
        .await?;
    return Ok(items);
}

/// DomainService: ListAllPullRequests
/// @route
/// @dep
#[tracing::instrument(skip_all)]
pub async fn list_all_pull_requests(
    deps: &Deps,
    status: Option<PrStatus>,
) -> Result<Vec<PullRequest>, DomainError> {
    // step: query
    let filter = status;
    let items = deps.pr_repo.list_all(filter.clone()).await?;
    return Ok(items);
}

/// DomainService: GetPullRequest
/// @route
/// @dep
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn get_pull_request(deps: &Deps, id: Uuid) -> Result<serde_json::Value, DomainError> {
    // step: load
    let cr = deps.pr_repo.find(id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let pr = cr.clone().ok_or(DomainError::NotFound)?;
    let approvals = deps.approval_repo.find_for_pr(id.clone()).await?;
    let comments = deps.comment_repo.list_for_pr(id.clone()).await?;
    let ci_runs = deps.ci_repo.list_for_pr(id.clone()).await?;
    return Ok(
        serde_json::json!({ "pr": pr.clone(), "approvals": approvals.clone(), "comments": comments.clone(), "ci_runs": ci_runs.clone() }),
    );
}

/// DomainService: UpdatePullRequestStatus
/// @route
/// @dep
#[tracing::instrument(skip_all)]
pub async fn update_pull_request_status(
    deps: &Deps,
    id: Uuid,
    status: PrStatus,
) -> Result<PullRequest, DomainError> {
    // step: update
    let cr = deps.pr_repo.find(id.clone()).await?;
    if cr.is_none() {
        return Err(DomainError::NotFound);
    };
    let mut pr = cr.clone().ok_or(DomainError::NotFound)?;
    pr.status = status;
    pr.updated_at = Utc::now();
    deps.pr_repo.save(pr.clone()).await?;
    return Ok(pr);
}

#[cfg(test)]
mod tests {
    //! Transport-only merge gate.
    //!
    //! Approval authority lives in `veil_server::review` (Model B): the
    //! recorded human `SignOffRecord` + `review::may_ship` are the sole ship
    //! gate, enforced by the HTTP/tool caller *before* `merge_pr` runs. These
    //! tests pin `change_management` as git/PR **transport**: it merges a PR
    //! that is NOT `PrStatus::Approved` and has NO `Approval` rows, and it only
    //! refuses on transport-lifecycle grounds (terminal record) or a failed CI
    //! run. See Mind Palace: decision-single-review-source-of-truth.

    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemPrRepo {
        prs: Mutex<HashMap<Uuid, PullRequest>>,
    }
    #[async_trait]
    impl PullRequestRepo for MemPrRepo {
        async fn find(&self, id: Uuid) -> Result<Option<PullRequest>, DomainError> {
            Ok(self.prs.lock().unwrap().get(&id).cloned())
        }
        async fn list_by_repo(
            &self,
            repo_id: Uuid,
            _status: Option<PrStatus>,
        ) -> Result<Vec<PullRequest>, DomainError> {
            Ok(self
                .prs
                .lock()
                .unwrap()
                .values()
                .filter(|p| p.repo_id == repo_id)
                .cloned()
                .collect())
        }
        async fn list_open(&self, _repo_id: Uuid) -> Result<Vec<PullRequest>, DomainError> {
            Ok(vec![])
        }
        async fn list_all(
            &self,
            _status: Option<PrStatus>,
        ) -> Result<Vec<PullRequest>, DomainError> {
            Ok(self.prs.lock().unwrap().values().cloned().collect())
        }
        async fn save(&self, cr: PullRequest) -> Result<(), DomainError> {
            self.prs.lock().unwrap().insert(cr.id, cr);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemApprovalRepo {
        rows: Mutex<Vec<Approval>>,
    }
    #[async_trait]
    impl ApprovalRepo for MemApprovalRepo {
        async fn find_for_pr(&self, pr_id: Uuid) -> Result<Vec<Approval>, DomainError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|a| a.pr_id == pr_id)
                .cloned()
                .collect())
        }
        async fn save(&self, approval: Approval) -> Result<(), DomainError> {
            self.rows.lock().unwrap().push(approval);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemCiRepo {
        runs: Mutex<Vec<CiRun>>,
    }
    #[async_trait]
    impl CiRunRepo for MemCiRepo {
        async fn latest_for_pr(&self, pr_id: Uuid) -> Result<Option<CiRun>, DomainError> {
            Ok(self
                .runs
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.pr_id == pr_id)
                .last()
                .cloned())
        }
        async fn list_for_pr(&self, pr_id: Uuid) -> Result<Vec<CiRun>, DomainError> {
            Ok(self
                .runs
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.pr_id == pr_id)
                .cloned()
                .collect())
        }
        async fn save(&self, run: CiRun) -> Result<(), DomainError> {
            self.runs.lock().unwrap().push(run);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemCommentRepo;
    #[async_trait]
    impl CommentRepo for MemCommentRepo {
        async fn list_for_pr(&self, _pr_id: Uuid) -> Result<Vec<ReviewComment>, DomainError> {
            Ok(vec![])
        }
        async fn save(&self, _comment: ReviewComment) -> Result<(), DomainError> {
            Ok(())
        }
        async fn resolve(&self, _id: Uuid) -> Result<(), DomainError> {
            Ok(())
        }
    }

    /// Merge is a ref copy; report the source SHA so the test can assert it ran.
    #[derive(Default)]
    struct MemGit;
    #[async_trait]
    impl GitService for MemGit {
        async fn init_repo(&self, _slug: String) -> Result<(), DomainError> {
            Ok(())
        }
        async fn repo_exists(&self, _slug: String) -> Result<bool, DomainError> {
            Ok(true)
        }
        async fn create_branch(
            &self,
            _slug: String,
            branch_name: String,
            _from_ref: String,
        ) -> Result<String, DomainError> {
            Ok(branch_name)
        }
        async fn delete_branch(&self, _slug: String, _branch: String) -> Result<(), DomainError> {
            Ok(())
        }
        async fn list_branches(&self, _slug: String) -> Result<Vec<String>, DomainError> {
            Ok(vec![])
        }
        async fn get_head(&self, _slug: String, _branch: String) -> Result<String, DomainError> {
            Ok("head".into())
        }
        async fn commit_file(
            &self,
            _slug: String,
            _branch: String,
            _path: String,
            _content: String,
            _message: String,
            _author: String,
        ) -> Result<String, DomainError> {
            Ok("sha".into())
        }
        async fn read_file(
            &self,
            _slug: String,
            _branch: String,
            _path: String,
        ) -> Result<Option<String>, DomainError> {
            Ok(None)
        }
        async fn list_files(
            &self,
            _slug: String,
            _branch: String,
        ) -> Result<Vec<String>, DomainError> {
            Ok(vec![])
        }
        async fn log(
            &self,
            _slug: String,
            _branch: String,
            _limit: i64,
        ) -> Result<serde_json::Value, DomainError> {
            Ok(serde_json::json!([]))
        }
        async fn merge(
            &self,
            _slug: String,
            _source: String,
            _target: String,
            _message: String,
            _author: String,
        ) -> Result<String, DomainError> {
            Ok("merge-sha".into())
        }
        async fn diff_files(
            &self,
            _slug: String,
            _base: String,
            _head: String,
        ) -> Result<serde_json::Value, DomainError> {
            Ok(serde_json::json!([]))
        }
        async fn can_merge(
            &self,
            _slug: String,
            _source: String,
            _target: String,
        ) -> Result<serde_json::Value, DomainError> {
            Ok(serde_json::json!({ "can_merge": true }))
        }
    }

    fn deps() -> (Deps, Arc<MemPrRepo>, Arc<MemCiRepo>) {
        let pr_repo = Arc::new(MemPrRepo::default());
        let ci_repo = Arc::new(MemCiRepo::default());
        let deps = Deps {
            approval_repo: Arc::new(MemApprovalRepo::default()),
            pr_repo: pr_repo.clone(),
            ci_repo: ci_repo.clone(),
            comment_repo: Arc::new(MemCommentRepo),
            git: Arc::new(MemGit),
        };
        (deps, pr_repo, ci_repo)
    }

    fn draft_pr(status: PrStatus) -> PullRequest {
        let now = Utc::now();
        PullRequest {
            id: Uuid::new_v4(),
            repo_id: Uuid::new_v4(),
            title: "feat".into(),
            description: "slug: acme\n".into(),
            jira_ticket: "".into(),
            source_branch: "cr/feat".into(),
            target_branch: "main".into(),
            author: "agent".into(),
            status,
            created_at: now,
            updated_at: now,
            merged_at: None,
            merged_by: None,
            merge_commit: None,
        }
    }

    /// Transport merges without a CM approval authority: no `PrStatus::Approved`,
    /// no `Approval` rows, no CI run. The ship gate (may_ship) is enforced by the
    /// caller, not here.
    #[tokio::test]
    async fn merge_does_not_require_cm_approval_authority() {
        let (deps, pr_repo, _ci) = deps();
        let pr = draft_pr(PrStatus::ReadyForReview);
        let id = pr.id;
        pr_repo.save(pr).await.unwrap();

        let out = merge_pr(&deps, id, "operator".into(), "acme".into())
            .await
            .expect("transport merge should not depend on CM approval");
        assert_eq!(out["merge_commit"], "merge-sha");
        let merged = pr_repo.find(id).await.unwrap().unwrap();
        assert_eq!(merged.status, PrStatus::Merged);
    }

    /// Even a `Draft` (never submitted / never CM-approved) transport record can
    /// be merged once the caller's may_ship gate has passed.
    #[tokio::test]
    async fn merge_allows_unsubmitted_transport_record() {
        let (deps, pr_repo, _ci) = deps();
        let pr = draft_pr(PrStatus::Draft);
        let id = pr.id;
        pr_repo.save(pr).await.unwrap();

        merge_pr(&deps, id, "operator".into(), "acme".into())
            .await
            .expect("transport merge should not require ReadyForReview/Approved");
        assert_eq!(
            pr_repo.find(id).await.unwrap().unwrap().status,
            PrStatus::Merged
        );
    }

    /// Transport still refuses a terminal record (already merged/closed/rejected).
    #[tokio::test]
    async fn merge_refuses_terminal_transport_record() {
        let (deps, pr_repo, _ci) = deps();
        let pr = draft_pr(PrStatus::Merged);
        let id = pr.id;
        pr_repo.save(pr).await.unwrap();

        let err = merge_pr(&deps, id, "operator".into(), "acme".into())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DomainError::Validation(ref m) if m.contains("terminal")),
            "expected terminal-record refusal, got {err:?}"
        );
    }

    /// A failed CI run is transport metadata that still blocks (build safety),
    /// but its *absence* does not (see merge_does_not_require_cm_approval_authority).
    #[tokio::test]
    async fn merge_refuses_when_ci_failed() {
        let (deps, pr_repo, ci_repo) = deps();
        let pr = draft_pr(PrStatus::ReadyForReview);
        let id = pr.id;
        pr_repo.save(pr).await.unwrap();
        ci_repo
            .save(CiRun::new(
                Uuid::new_v4(),
                id,
                "abc".into(),
                CiStatus::Failed,
                Utc::now(),
            ))
            .await
            .unwrap();

        let err = merge_pr(&deps, id, "operator".into(), "acme".into())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DomainError::Validation(ref m) if m.contains("CI")),
            "expected CI-failed refusal, got {err:?}"
        );
    }
}
