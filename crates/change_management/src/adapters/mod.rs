//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

// External-effect runtime hooks (stubs). Replace with real
// integrations; generated so adapter bodies compile.
fn attribute_value_s(_arg0: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn client_put_item() { /* stub — replace with real integration */
}
fn client_query() { /* stub — replace with real integration */
}
fn client_scan() { /* stub — replace with real integration */
}

/// Adapter: BusAuthAdapter (implements AuthService)
pub struct BusAuthAdapter {}

#[async_trait]
impl AuthService for BusAuthAdapter {
    async fn check_permission(
        &self,
        principal: Principal,
        permission: String,
    ) -> Result<bool, DomainError> {
        return Ok(true);
    }

    async fn validate_token(&self, token: String) -> Result<Principal, DomainError> {
        return Ok(Principal {
            id: token.clone(),
            roles: vec![],
            claims: HashMap::new(),
        });
    }
}

/// Adapter: DdbPullRequestRepo (implements PullRequestRepo)
pub struct DdbPullRequestRepo {
    pub client: aws_sdk_dynamodb::Client,
    pub table: String,
}

#[async_trait]
impl PullRequestRepo for DdbPullRequestRepo {
    async fn find(&self, id: Uuid) -> Result<Option<PullRequest>, DomainError> {
        let pk = format!("PR#{}", id);
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Ok(None);
        };
        return Ok(Some(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?));
    }

    async fn list_all(&self, status: Option<PrStatus>) -> Result<Vec<PullRequest>, DomainError> {
        // FilterExpression is applied after each 1MB scan page. Follow
        // LastEvaluatedKey so PRs on later pages are not dropped.
        let mut items = Vec::new();
        let mut exclusive_start_key = None;
        loop {
            let mut req = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(PK, :prefix) AND SK = :sk".to_string())
                .expression_attribute_values(
                    ":prefix".to_string(),
                    aws_sdk_dynamodb::types::AttributeValue::S("PR#".to_string()),
                )
                .expression_attribute_values(
                    ":sk".to_string(),
                    aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
                );
            if let Some(key) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(key));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| DomainError::External(format!("{e:?}")))?;
            items.extend(resp.items().iter().cloned());
            match resp.last_evaluated_key() {
                Some(key) if !key.is_empty() => exclusive_start_key = Some(key.clone()),
                _ => break,
            }
        }
        let mut out = vec![];
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            let cr: PullRequest = serde_json::from_str::<_>(&data)?;
            if status.is_none() || cr.status == status.clone().ok_or(DomainError::NotFound)? {
                out.push(cr);
            };
        }
        return Ok(out);
    }

    async fn list_by_repo(
        &self,
        repo_id: Uuid,
        status: Option<PrStatus>,
    ) -> Result<Vec<PullRequest>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("GSI1".to_string())
            .key_condition_expression("GSI1PK = :pk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO_PRS#{}", repo_id)),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        let mut out = vec![];
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            let cr: PullRequest = serde_json::from_str::<_>(&data)?;
            if status.is_none() || cr.status == status.clone().ok_or(DomainError::NotFound)? {
                out.push(cr);
            };
        }
        return Ok(out);
    }

    async fn list_open(&self, repo_id: Uuid) -> Result<Vec<PullRequest>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("GSI1".to_string())
            .key_condition_expression("GSI1PK = :pk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO_PRS#{}", repo_id)),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        let mut out = vec![];
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            let cr: PullRequest = serde_json::from_str::<_>(&data)?;
            if cr.status != PrStatus::Merged
                && cr.status != PrStatus::Rejected
                && cr.status != PrStatus::Closed
            {
                out.push(cr);
            };
        }
        return Ok(out);
    }

    async fn save(&self, cr: PullRequest) -> Result<(), DomainError> {
        let pk = format!("PR#{}", cr.id);
        let gsi1pk = format!("REPO_PRS#{}", cr.repo_id);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .item(
                "GSI1PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(gsi1pk),
            )
            .item(
                "GSI1SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("{}", cr.updated_at)),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&cr)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }
}

/// Adapter: DdbApprovalRepo (implements ApprovalRepo)
pub struct DdbApprovalRepo {
    pub client: aws_sdk_dynamodb::Client,
    pub table: String,
}

#[async_trait]
impl ApprovalRepo for DdbApprovalRepo {
    async fn find_for_pr(&self, pr_id: Uuid) -> Result<Vec<Approval>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("PR#{}", pr_id)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("APPROVAL#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        let mut out = vec![];
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            out.push(serde_json::from_str::<_>(&data)?);
        }
        return Ok(out);
    }

    async fn save(&self, approval: Approval) -> Result<(), DomainError> {
        let pk = format!("PR#{}", approval.pr_id);
        let sk = format!("APPROVAL#{}", approval.reviewer);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(sk),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&approval)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }
}

/// Adapter: DdbCiRunRepo (implements CiRunRepo)
pub struct DdbCiRunRepo {
    pub client: aws_sdk_dynamodb::Client,
    pub table: String,
}

#[async_trait]
impl CiRunRepo for DdbCiRunRepo {
    async fn latest_for_pr(&self, pr_id: Uuid) -> Result<Option<CiRun>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("PR#{}", pr_id)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("CI#".to_string()),
            )
            .scan_index_forward(false)
            .limit((1) as i32)
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Ok(None);
        };
        return Ok(Some(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?));
    }

    async fn list_for_pr(&self, pr_id: Uuid) -> Result<Vec<CiRun>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("PR#{}", pr_id)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("CI#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        let mut out = vec![];
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            out.push(serde_json::from_str::<_>(&data)?);
        }
        return Ok(out);
    }

    async fn save(&self, run: CiRun) -> Result<(), DomainError> {
        let pk = format!("PR#{}", run.pr_id);
        let sk = format!("CI#{}#{}", run.started_at, run.id);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(sk),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&run)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }
}

/// Adapter: DdbCommentRepo (implements CommentRepo)
pub struct DdbCommentRepo {
    pub client: aws_sdk_dynamodb::Client,
    pub table: String,
}

#[async_trait]
impl CommentRepo for DdbCommentRepo {
    async fn list_for_pr(&self, pr_id: Uuid) -> Result<Vec<ReviewComment>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("PR#{}", pr_id)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("COMMENT#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        let mut out = vec![];
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?;
            out.push(serde_json::from_str::<_>(&data)?);
        }
        return Ok(out);
    }

    async fn resolve(&self, id: Uuid) -> Result<(), DomainError> {
        return Ok(());
    }

    async fn save(&self, comment: ReviewComment) -> Result<(), DomainError> {
        let pk = format!("PR#{}", comment.pr_id);
        let sk = format!("COMMENT#{}#{}", comment.created_at, comment.id);
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(pk),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(sk),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&comment)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }
}

/// Adapter: S3GitServiceAdapter (implements GitService)
///
/// When git origin is on (`VEIL_GIT_ORIGIN`, default auto with sessions),
/// mutating methods are no-ops. Real history lives in `veil_server::git_origin`.
/// This adapter must not write SHA stubs to `git/{slug}/refs`.
pub struct S3GitServiceAdapter {
    pub bucket: String,
    pub s3: aws_sdk_s3::Client,
}

fn git_origin_owns() -> bool {
    let flag = |name: &str, default: &str| {
        std::env::var(name).unwrap_or_else(|_| default.into())
            .to_ascii_lowercase()
    };
    match flag("VEIL_GIT_ORIGIN", "auto").as_str() {
        "0" | "false" | "off" | "no" => false,
        "1" | "true" | "on" | "yes" => true,
        _ => match flag("VEIL_SESSIONS", "auto").as_str() {
            "0" | "false" | "off" | "no" => false,
            "1" | "true" | "on" | "yes" => true,
            _ => {
                let mode = flag("VEIL_SOURCE_MODE", "prefer_s3");
                !matches!(mode.as_str(), "disk" | "fs" | "filesystem" | "local")
            }
        },
    }
}

#[async_trait]
impl GitService for S3GitServiceAdapter {
    async fn can_merge(
        &self,
        slug: String,
        source: String,
        target: String,
    ) -> Result<serde_json::Value, DomainError> {
        return Ok(
            serde_json::json!({ "can_merge": true, "conflicts": serde_json::Value::Array(vec![]), "behind_by": 0 }),
        );
    }

    async fn commit_file(
        &self,
        slug: String,
        branch: String,
        path: String,
        content: String,
        message: String,
        author: String,
    ) -> Result<String, DomainError> {
        if git_origin_owns() {
            return Ok("git-origin".into());
        }
        let cache_path = format!("/tmp/veil-git-cache/{}", slug);
        veil_local_fs::LocalFs::create_dir_all(cache_path.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let hash = {
            let repo = gix::ThreadSafeRepository::open(cache_path.clone())
                .map_err(|e| DomainError::External(e.to_string()))?
                .to_thread_local();
            let blob_id = repo
                .write_blob(content.as_bytes())
                .map_err(|e| DomainError::External(e.to_string()))?;
            let blob_oid = blob_id.detach();
            let tree = repo.empty_tree();
            let mut editor = tree
                .edit()
                .map_err(|e| DomainError::External(e.to_string()))?;
            editor
                .upsert(
                    path.clone(),
                    gix::objs::tree::EntryKind::Blob,
                    blob_oid.clone(),
                )
                .map_err(|e| DomainError::External(e.to_string()))?;
            let new_tree_id = editor
                .write()
                .map_err(|e| DomainError::External(e.to_string()))?;
            let new_tree_oid = new_tree_id.detach();
            let parent_ref = format!("refs/heads/{}", branch);
            let commit_id = repo
                .commit(
                    &*parent_ref,
                    &*message,
                    new_tree_oid.clone(),
                    vec![new_tree_oid],
                )
                .map_err(|e| DomainError::External(e.to_string()))?;
            commit_id.detach().to_string()
        };
        let head_key = format!("git/{}/refs/heads/{}", slug, branch);
        self.s3
            .put_object()
            .bucket(self.bucket.clone())
            .key(head_key.clone())
            .body(hash.clone().into_bytes().into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(hash);
    }

    async fn create_branch(
        &self,
        slug: String,
        branch_name: String,
        from_ref: String,
    ) -> Result<String, DomainError> {
        if git_origin_owns() {
            return Ok(branch_name);
        }
        let src_key = format!("git/{}/refs/heads/{}", slug, from_ref);
        let resp = self
            .s3
            .get_object()
            .bucket(self.bucket.clone())
            .key(src_key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let agg = resp
            .body
            .collect()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let body = agg.into_bytes();
        let text = body.to_vec();
        let src_sha = str::from_utf8(text.as_slice()).unwrap_or("").to_string();
        let dest_key = format!("git/{}/refs/heads/{}", slug, branch_name);
        self.s3
            .put_object()
            .bucket(self.bucket.clone())
            .key(dest_key.clone())
            .body(src_sha.clone().into_bytes().into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(branch_name);
    }

    async fn delete_branch(&self, slug: String, branch_name: String) -> Result<(), DomainError> {
        if git_origin_owns() {
            return Ok(());
        }
        let key = format!("git/{}/refs/heads/{}", slug, branch_name);
        self.s3
            .delete_object()
            .bucket(self.bucket.clone())
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn diff_files(
        &self,
        slug: String,
        base_ref: String,
        head_ref: String,
    ) -> Result<serde_json::Value, DomainError> {
        return Ok(serde_json::from_str::<_>(&"[]".to_string())?);
    }

    async fn get_head(&self, slug: String, branch: String) -> Result<String, DomainError> {
        let key = format!("git/{}/refs/heads/{}", slug, branch);
        let resp = self
            .s3
            .get_object()
            .bucket(self.bucket.clone())
            .key(key.clone())
            .send()
            .await;
        match resp {
            Ok(output) => {
                let agg = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                let body = agg.into_bytes();
                let text = body.to_vec();
                return Ok(str::from_utf8(text.as_slice())
                    .unwrap_or("initial")
                    .to_string()
                    .trim()
                    .to_string()
                    .to_string());
            }
            Err(_) => return Ok("initial".to_string()),
        }
    }

    async fn init_repo(&self, slug: String) -> Result<(), DomainError> {
        if git_origin_owns() {
            return Ok(());
        }
        let cache_path = format!("/tmp/veil-git-cache/{}", slug);
        veil_local_fs::LocalFs::create_dir_all(cache_path.clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let commit_hash = {
            let repo =
                gix::init_bare(&cache_path).map_err(|e| DomainError::External(e.to_string()))?;
            let empty_tree = repo.empty_tree();
            let tree_oid = empty_tree.id().detach();
            let commit_id = repo
                .commit(
                    "refs/heads/main",
                    "Initial commit",
                    tree_oid.clone(),
                    vec![tree_oid],
                )
                .map_err(|e| DomainError::External(e.to_string()))?;
            commit_id.detach().to_string()
        };
        let head_key = format!("git/{}/refs/heads/main", slug);
        self.s3
            .put_object()
            .bucket(self.bucket.clone())
            .key(head_key.clone())
            .body(commit_hash.clone().into_bytes().into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let marker_key = format!("git/{}/.initialized", slug);
        let marker_body = "true".to_string();
        self.s3
            .put_object()
            .bucket(self.bucket.clone())
            .key(marker_key.clone())
            .body(marker_body.into_bytes().into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn list_branches(&self, slug: String) -> Result<Vec<String>, DomainError> {
        let prefix = format!("git/{}/refs/heads/", slug);
        let resp = self
            .s3
            .list_objects_v2()
            .bucket(self.bucket.clone())
            .prefix(prefix.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let mut out = vec![];
        for obj in resp.contents() {
            let raw = obj.key().unwrap_or_default().replace(&prefix, "");
            if raw != "".to_string() && !raw.starts_with(".") {
                out.push(raw);
            };
        }
        return Ok(out);
    }

    async fn list_files(&self, slug: String, branch: String) -> Result<Vec<String>, DomainError> {
        let prefix = format!("repos/{}/{}/", slug, branch);
        let resp = self
            .s3
            .list_objects_v2()
            .bucket(self.bucket.clone())
            .prefix(prefix.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let mut out = vec![];
        if !resp.contents().is_empty() {
            for obj in resp.contents() {
                let k = obj.key().unwrap().replace(&prefix, "");
                if !k.starts_with(".") {
                    out.push(k);
                };
            }
        };
        return Ok(out);
    }

    async fn log(
        &self,
        slug: String,
        branch: String,
        limit: i64,
    ) -> Result<serde_json::Value, DomainError> {
        return Ok(serde_json::from_str::<_>(&"[]".to_string())?);
    }

    async fn merge(
        &self,
        slug: String,
        source: String,
        target: String,
        message: String,
        author: String,
    ) -> Result<String, DomainError> {
        if git_origin_owns() {
            // Real merge is GitOrigin::merge_and_push in platform_http.
            return Ok("git-origin".into());
        }
        let src_key = format!("git/{}/refs/heads/{}", slug, source);
        let resp = self
            .s3
            .get_object()
            .bucket(self.bucket.clone())
            .key(src_key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let agg = resp
            .body
            .collect()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let body = agg.into_bytes();
        let text = body.to_vec();
        let src_sha = str::from_utf8(text.as_slice()).unwrap_or("").to_string();
        let dest_key = format!("git/{}/refs/heads/{}", slug, target);
        self.s3
            .put_object()
            .bucket(self.bucket.clone())
            .key(dest_key.clone())
            .body(src_sha.clone().into_bytes().into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(src_sha);
    }

    async fn read_file(
        &self,
        slug: String,
        branch: String,
        path: String,
    ) -> Result<Option<String>, DomainError> {
        let key = format!("repos/{}/{}/{}", slug, branch, path);
        let resp = self
            .s3
            .get_object()
            .bucket(self.bucket.clone())
            .key(key.clone())
            .send()
            .await;
        match resp {
            Ok(output) => {
                let agg = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| DomainError::External(format!("{e:?}")))?;
                let body = agg.into_bytes();
                let text = body.to_vec();
                return Ok(Some(
                    str::from_utf8(text.as_slice()).unwrap_or("").to_string(),
                ));
            }
            Err(_) => return Ok(None),
        }
    }

    async fn repo_exists(&self, slug: String) -> Result<bool, DomainError> {
        let key = format!("git/{}/.initialized", slug);
        let resp = self
            .s3
            .head_object()
            .bucket(self.bucket.clone())
            .key(key.clone())
            .send()
            .await;
        return Ok(resp.is_ok());
    }
}
