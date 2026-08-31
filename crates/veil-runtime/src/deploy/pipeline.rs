//! Pipeline orchestrator — coordinates deploy steps, tracks jobs, handles concurrency.
//!
//! Constraints:
//! - All build/deploy happens inside the container
//! - Terraform files come from S3 source
//! - Build artifacts are ephemeral (/tmp)
//! - Each deploy is a recorded, auditable event
//! - Concurrent deploys per-project are rejected (one at a time)

use super::{
    build_contribution, build_frontend, build_rust, config, deploy_frontend, deploy_lambda, gates,
    terraform, types::*,
};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Determine the source `.veil` file to generate from for a given deploy
/// target. VEIL convention: a single project holds BOTH backend and UI —
/// backend/domain lives in `main.veil` (→ Rust), the user interface lives in
/// `ui.veil` (→ TypeScript contribution). The source is therefore PER-TARGET,
/// not hardcoded:
///
/// - Contribution / Frontend (Typescript) builds gen from **`ui.veil`**,
///   falling back to `main.veil` when `ui.veil` is absent (back-compat with
///   UI-only projects that still keep their UI in `main.veil`).
/// - Lambda / ECS (Rust backend) builds gen from **`main.veil`**.
///
/// An explicit override always wins:
/// - `[deploy.contribution].veil` (or `.package`) for contribution builds,
/// - `[deploy.build].veil` (or `.package`) for other builds,
/// - legacy top-level `main = "..."` in veil.toml.
///
/// `source_dir` is the on-disk checkout used to test file existence for the
/// `ui.veil` → `main.veil` fallback.
pub fn resolve_veil_source(
    config: &ProjectDeployConfig,
    source_dir: &Path,
    veil_toml: &str,
) -> String {
    let parsed: Option<toml::Value> = veil_toml.parse().ok();

    // Explicit per-target override wins.
    let explicit = parsed.as_ref().and_then(|p| {
        let deploy = p.get("deploy");
        let from_contribution = if config.deploy_type == DeployType::Contribution {
            deploy
                .and_then(|d| d.get("contribution"))
                .and_then(|c| c.get("veil").or_else(|| c.get("package")))
                .and_then(|v| v.as_str())
        } else {
            None
        };
        from_contribution
            .or_else(|| {
                deploy
                    .and_then(|d| d.get("build"))
                    .and_then(|b| b.get("veil").or_else(|| b.get("package")))
                    .and_then(|v| v.as_str())
            })
            .or_else(|| p.get("main").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
    });
    if let Some(f) = explicit {
        if !f.is_empty() {
            return f;
        }
    }

    // Convention-based default, per deploy target.
    let is_ui_build = matches!(
        config.deploy_type,
        DeployType::Contribution | DeployType::Frontend
    ) || config
        .build
        .as_ref()
        .map(|b| b.target == BuildTarget::Typescript)
        .unwrap_or(false);

    if is_ui_build {
        // Prefer ui.veil, fall back to main.veil for back-compat.
        if source_dir.join("ui.veil").exists() {
            return "ui.veil".to_string();
        }
        return "main.veil".to_string();
    }

    "main.veil".to_string()
}

/// The pipeline state shared across the HTTP server.
/// Tracks active jobs and prevents concurrent deploys per project.
#[derive(Clone)]
pub struct PipelineState {
    /// Active jobs indexed by job_id.
    pub jobs: Arc<Mutex<HashMap<String, DeployJob>>>,
    /// Currently-running job per project slug (prevents concurrent deploys).
    pub active_per_project: Arc<Mutex<HashMap<String, String>>>,
    /// Completed job history per project slug.
    pub history: Arc<Mutex<HashMap<String, Vec<DeployJob>>>>,
    /// Drift status per project slug.
    pub drift_cache: Arc<Mutex<HashMap<String, DriftStatus>>>,
    /// Storage deps for reading project files from S3.
    pub storage_deps: Arc<storage::application::Deps>,
    /// AWS clients for deploy operations.
    pub s3_client: aws_sdk_s3::Client,
    pub lambda_client: aws_sdk_lambda::Client,
}

impl PipelineState {
    pub async fn new(storage_deps: Arc<storage::application::Deps>) -> Self {
        let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            active_per_project: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(HashMap::new())),
            drift_cache: Arc::new(Mutex::new(HashMap::new())),
            storage_deps,
            s3_client: aws_sdk_s3::Client::new(&aws_config),
            lambda_client: aws_sdk_lambda::Client::new(&aws_config),
        }
    }

    // ─── Project source access ──────────────────────────────────────────────

    /// Resolve a project slug to a repo ID.
    async fn resolve_repo_id(&self, slug: &str) -> Result<String, String> {
        let repo = storage::application::resolve_repo(&self.storage_deps, slug)
            .await
            .map_err(|e| format!("Failed to resolve project '{slug}': {e:?}"))?;
        Ok(repo.id.value)
    }

    /// Load deploy configuration from the project's veil.toml in S3.
    pub async fn load_deploy_config(&self, slug: &str) -> Result<ProjectDeployConfig, String> {
        let repo_id = self.resolve_repo_id(slug).await?;
        let rid = storage::domain::types::RepoId { value: repo_id };

        // Try reading veil.toml from the project's main branch
        let toml_bytes = storage::application::read_file(
            &self.storage_deps,
            rid,
            "main".to_string(),
            "veil.toml".to_string(),
        )
        .await;

        match toml_bytes {
            Ok(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                parse_toml_deploy_config(&content)
            }
            Err(_) => {
                // No veil.toml — return defaults
                Ok(ProjectDeployConfig::default())
            }
        }
    }

    /// Fetch terraform files from the project's S3 source.
    pub async fn fetch_terraform_files(
        &self,
        slug: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        let repo_id = self.resolve_repo_id(slug).await?;
        let rid = storage::domain::types::RepoId {
            value: repo_id.clone(),
        };

        // List files under terraform/ prefix
        let files = storage::application::list_files(
            &self.storage_deps,
            rid.clone(),
            "main".to_string(),
            "terraform/".to_string(),
        )
        .await
        .map_err(|e| format!("Failed to list terraform files: {e:?}"))?;

        let mut tf_files = Vec::new();
        for file_path in &files {
            if file_path.ends_with(".tf") || file_path.ends_with(".tf.json") {
                let read_rid = storage::domain::types::RepoId {
                    value: repo_id.clone(),
                };
                match storage::application::read_file(
                    &self.storage_deps,
                    read_rid,
                    "main".to_string(),
                    file_path.clone(),
                )
                .await
                {
                    Ok(bytes) => {
                        // Strip the terraform/ prefix for the local filename
                        let filename = file_path
                            .strip_prefix("terraform/")
                            .unwrap_or(file_path)
                            .to_string();
                        tf_files.push((filename, bytes));
                    }
                    Err(e) => {
                        warn!(slug, file_path, "Failed to read terraform file: {e:?}");
                    }
                }
            }
        }

        Ok(tf_files)
    }

    /// Fetch all project source files from S3 and write them to a local directory.
    /// Returns the path to the local source directory.
    pub async fn fetch_source_to_disk(&self, slug: &str) -> Result<PathBuf, String> {
        let repo_id = self.resolve_repo_id(slug).await?;
        let rid = storage::domain::types::RepoId {
            value: repo_id.clone(),
        };
        let source_dir = config::working_dir(slug).join("source");

        tokio::fs::create_dir_all(&source_dir)
            .await
            .map_err(|e| format!("Failed to create source dir: {e}"))?;

        // List all files in the project
        let files = storage::application::list_files(
            &self.storage_deps,
            rid.clone(),
            "main".to_string(),
            "".to_string(),
        )
        .await
        .map_err(|e| format!("Failed to list source files: {e:?}"))?;

        for file_path in &files {
            // Skip terraform/ files (handled separately) and .git metadata
            if file_path.starts_with("terraform/") || file_path.starts_with(".git/") {
                continue;
            }

            let read_rid = storage::domain::types::RepoId {
                value: repo_id.clone(),
            };
            match storage::application::read_file(
                &self.storage_deps,
                read_rid,
                "main".to_string(),
                file_path.clone(),
            )
            .await
            {
                Ok(bytes) => {
                    let dest = source_dir.join(file_path);
                    if let Some(parent) = dest.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    if let Err(e) = tokio::fs::write(&dest, &bytes).await {
                        warn!(slug, file_path, "Failed to write source file: {e}");
                    }
                }
                Err(e) => {
                    warn!(slug, file_path, "Failed to read source file: {e:?}");
                }
            }
        }

        Ok(source_dir)
    }

    /// Determine the source `.veil` file to generate from for a given deploy
    /// target. VEIL convention: a single project holds BOTH backend and UI —
    /// backend/domain lives in `main.veil` (→ Rust), the user interface lives in
    /// `ui.veil` (→ TypeScript contribution). The source is therefore
    /// PER-TARGET, not hardcoded:
    ///
    /// - Contribution / Frontend (Typescript) builds gen from **`ui.veil`**,
    ///   falling back to `main.veil` when `ui.veil` is absent (back-compat with
    ///   UI-only projects that still keep their UI in `main.veil`).
    /// - Lambda / ECS (Rust backend) builds gen from **`main.veil`**.
    ///
    /// An explicit override always wins:
    /// - `[deploy.contribution].veil` for contribution builds,
    /// - `[deploy.build].package` / `[deploy.build].veil` for other builds,
    /// - legacy top-level `main = "..."` in veil.toml.
    ///
    /// `source_dir` is the on-disk checkout used to test file existence for the
    /// `ui.veil` → `main.veil` fallback.
    pub fn resolve_veil_file_for(
        &self,
        config: &ProjectDeployConfig,
        source_dir: &Path,
        veil_toml: &str,
    ) -> String {
        resolve_veil_source(config, source_dir, veil_toml)
    }

    // ─── Deploy trigger ─────────────────────────────────────────────────────

    /// Trigger a new deploy job. Returns immediately with job_id.
    /// The actual execution happens in a spawned task.
    pub async fn trigger_deploy(
        &self,
        project_slug: String,
        request: TriggerDeployRequest,
        triggered_by: String,
    ) -> Result<TriggerDeployResponse, String> {
        // Check if a deploy is already in progress for this project
        {
            let active = self.active_per_project.lock().await;
            if let Some(active_job_id) = active.get(&project_slug) {
                return Err(format!(
                    "Deploy already in progress for project '{project_slug}' (job: {active_job_id})"
                ));
            }
        }

        let steps = DeployStepKind::from_str_vec(&request.steps);
        if steps.is_empty() {
            return Err("No valid steps specified".into());
        }

        // Load project config from S3
        let project_config = self.load_deploy_config(&project_slug).await?;

        let mut job = DeployJob::new(
            project_slug.clone(),
            request.environment.clone(),
            triggered_by,
            request.dry_run,
        );

        // Check approval gates
        if gates::requires_approval(&project_config, &request.environment) && !request.dry_run {
            job.status = JobStatus::AwaitingApproval;
            let response = TriggerDeployResponse {
                job_id: job.id.clone(),
                status: job.status.clone(),
            };
            self.jobs.lock().await.insert(job.id.clone(), job);
            return Ok(response);
        }

        // Mark as running and register
        job.status = JobStatus::Running;
        let job_id = job.id.clone();

        {
            let mut active = self.active_per_project.lock().await;
            active.insert(project_slug.clone(), job_id.clone());
        }
        self.jobs.lock().await.insert(job_id.clone(), job.clone());

        // Spawn the pipeline execution
        let state = self.clone();
        let job_id_clone = job_id.clone();
        let slug_clone = project_slug.clone();
        tokio::spawn(async move {
            state.execute_pipeline(job_id_clone, slug_clone, steps).await;
        });

        Ok(TriggerDeployResponse {
            job_id,
            status: JobStatus::Running,
        })
    }

    /// Approve a pending deploy job and start execution.
    pub async fn approve_job(&self, job_id: &str) -> Result<(), String> {
        let (project_slug, steps) = {
            let mut jobs = self.jobs.lock().await;
            let job = jobs.get_mut(job_id).ok_or("Job not found")?;
            gates::validate_approval(job)?;
            job.status = JobStatus::Running;
            let slug = job.project_slug.clone();
            let steps_raw = job.steps.iter().map(|s| s.step.clone()).collect::<Vec<_>>();
            let steps = if steps_raw.is_empty() {
                DeployStepKind::all()
            } else {
                steps_raw
            };
            (slug, steps)
        };

        {
            let mut active = self.active_per_project.lock().await;
            active.insert(project_slug.clone(), job_id.to_string());
        }

        let state = self.clone();
        let job_id_owned = job_id.to_string();
        let slug_clone = project_slug.clone();
        tokio::spawn(async move {
            state.execute_pipeline(job_id_owned, slug_clone, steps).await;
        });

        Ok(())
    }

    /// Get the current status for a project.
    pub async fn get_status(&self, project_slug: &str) -> DeployStatusResponse {
        let last_deploy = {
            let history = self.history.lock().await;
            history.get(project_slug).and_then(|h| h.last()).map(|j| DeployJobSummary {
                id: j.id.clone(),
                status: j.status.clone(),
                environment: j.environment.clone(),
                at: j.triggered_at,
                steps: j.steps.clone(),
            })
        };

        // Also check active jobs
        let active_job = {
            let jobs = self.jobs.lock().await;
            jobs.values()
                .find(|j| j.project_slug == project_slug && j.status == JobStatus::Running)
                .map(|j| DeployJobSummary {
                    id: j.id.clone(),
                    status: j.status.clone(),
                    environment: j.environment.clone(),
                    at: j.triggered_at,
                    steps: j.steps.clone(),
                })
        };

        let drift = {
            let cache = self.drift_cache.lock().await;
            cache.get(project_slug).cloned()
        };

        let infra_drifted = drift.as_ref().map(|d| d.detected).unwrap_or(false);

        // Check if code has changed since last deploy by comparing commit SHAs
        let (code_changed, never_deployed) = {
            let history = self.history.lock().await;
            let last_sha = history
                .get(project_slug)
                .and_then(|h| h.last())
                .and_then(|j| j.commit_sha.as_deref());
            let is_never_deployed = history.get(project_slug).map(|h| h.is_empty()).unwrap_or(true);
            // If we have a last deploy commit, check if head has moved
            // If no history at all, mark as changed (never deployed)
            let changed = match last_sha {
                Some(_sha) => {
                    // For now: if there's a last deploy, assume in-sync until
                    // a new commit is pushed. The trigger_deploy will fetch fresh source.
                    false
                }
                None => is_never_deployed,
            };
            (changed, is_never_deployed)
        };

        // If project has never been deployed and has infrastructure config,
        // infra is pending (not "synced"). Drift only applies to already-provisioned infra.
        let infra_pending = if never_deployed && !infra_drifted {
            match self.load_deploy_config(project_slug).await {
                Ok(config) => config.infrastructure.is_some(),
                Err(_) => false,
            }
        } else {
            infra_drifted
        };

        DeployStatusResponse {
            last_deploy: active_job.or(last_deploy),
            drift,
            pending_changes: PendingChanges {
                infra: infra_pending,
                code: code_changed,
            },
        }
    }

    /// Get job by ID.
    pub async fn get_job(&self, job_id: &str) -> Option<DeployJob> {
        self.jobs.lock().await.get(job_id).cloned()
    }

    /// Get deploy history for a project.
    pub async fn get_history(&self, project_slug: &str) -> Vec<DeployHistoryItem> {
        let history = self.history.lock().await;
        history
            .get(project_slug)
            .map(|jobs| {
                jobs.iter()
                    .map(|j| DeployHistoryItem {
                        id: j.id.clone(),
                        status: j.status.clone(),
                        environment: j.environment.clone(),
                        at: j.triggered_at,
                        by: j.triggered_by.clone(),
                        steps: j.steps.iter().map(|s| s.step.clone()).collect(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get cached drift status.
    pub async fn get_drift(&self, project_slug: &str) -> Option<DriftStatus> {
        self.drift_cache.lock().await.get(project_slug).cloned()
    }

    /// Store drift status in cache.
    pub async fn set_drift(&self, project_slug: &str, drift: DriftStatus) {
        self.drift_cache
            .lock()
            .await
            .insert(project_slug.to_string(), drift);
    }

    /// Run drift detection for a project. Fetches tf files from S3 and runs plan.
    pub async fn check_drift(&self, slug: &str) -> Result<DriftStatus, String> {
        let config = self.load_deploy_config(slug).await?;
        let infra = config
            .infrastructure
            .as_ref()
            .ok_or("No infrastructure config for this project")?;

        let tf_files = self.fetch_terraform_files(slug).await?;
        if tf_files.is_empty() {
            return Ok(DriftStatus::in_sync());
        }

        let drift = super::drift::check_drift(slug, infra, &tf_files).await?;
        self.set_drift(slug, drift.clone()).await;
        Ok(drift)
    }

    // ─── Internal pipeline execution ────────────────────────────────────────

    async fn execute_pipeline(
        &self,
        job_id: String,
        slug: String,
        steps: Vec<DeployStepKind>,
    ) {
        let dry_run = {
            let jobs = self.jobs.lock().await;
            jobs.get(&job_id).map(|j| j.dry_run).unwrap_or(false)
        };

        info!(job_id = %job_id, slug = %slug, "pipeline execution starting");

        // Fetch project config and source from S3
        let project_config = match self.load_deploy_config(&slug).await {
            Ok(c) => c,
            Err(e) => {
                self.fail_job(&job_id, &format!("Failed to load deploy config: {e}")).await;
                self.release_project_lock(&slug).await;
                return;
            }
        };

        let tf_files = match self.fetch_terraform_files(&slug).await {
            Ok(f) => f,
            Err(e) => {
                warn!(slug = %slug, "Failed to fetch terraform files: {e}");
                Vec::new()
            }
        };

        let source_dir = match self.fetch_source_to_disk(&slug).await {
            Ok(d) => d,
            Err(e) => {
                self.fail_job(&job_id, &format!("Failed to fetch source: {e}")).await;
                self.release_project_lock(&slug).await;
                return;
            }
        };

        let veil_file = {
            let veil_toml = tokio::fs::read_to_string(source_dir.join("veil.toml"))
                .await
                .unwrap_or_default();
            self.resolve_veil_file_for(&project_config, &source_dir, &veil_toml)
        };

        for step_kind in &steps {
            let result = self
                .execute_step(
                    &job_id, &slug, step_kind, &project_config, &tf_files, &source_dir, &veil_file, dry_run,
                )
                .await;

            if let Err(err) = result {
                error!(job_id = %job_id, slug = %slug, step = ?step_kind, error = %err, "step failed");
                self.fail_job(&job_id, &err).await;
                self.release_project_lock(&slug).await;
                self.cleanup_working_dir(&slug).await;
                return;
            }
        }

        // All steps completed
        self.complete_job(&job_id).await;
        self.release_project_lock(&slug).await;
        self.cleanup_working_dir(&slug).await;
        info!(job_id = %job_id, slug = %slug, "pipeline execution complete");
    }

    async fn execute_step(
        &self,
        job_id: &str,
        slug: &str,
        step_kind: &DeployStepKind,
        project_config: &ProjectDeployConfig,
        tf_files: &[(String, Vec<u8>)],
        source_dir: &PathBuf,
        veil_file: &str,
        dry_run: bool,
    ) -> Result<(), String> {
        self.update_step_status(job_id, step_kind, StepStatus::Running)
            .await;

        let result = match step_kind {
            DeployStepKind::Infrastructure => {
                self.run_infrastructure(slug, project_config, tf_files, dry_run)
                    .await
            }
            DeployStepKind::Build => {
                self.run_build(slug, project_config, source_dir, veil_file)
                    .await
            }
            DeployStepKind::Deploy => {
                self.run_deploy(slug, project_config, dry_run).await
            }
        };

        match &result {
            Ok(output) => {
                self.complete_step(job_id, step_kind, output.clone()).await;
            }
            Err(err) => {
                self.fail_step(job_id, step_kind, err.clone()).await;
            }
        }

        result.map(|_| ())
    }

    async fn run_infrastructure(
        &self,
        slug: &str,
        config: &ProjectDeployConfig,
        tf_files: &[(String, Vec<u8>)],
        dry_run: bool,
    ) -> Result<String, String> {
        let infra = match config.infrastructure.as_ref() {
            Some(i) => i,
            None => return Ok("No infrastructure config — skipping terraform step".into()),
        };

        if tf_files.is_empty() {
            return Ok("No terraform files found — skipping infrastructure step".into());
        }

        let result = terraform::run(slug, infra, tf_files, dry_run).await?;

        if result.applied {
            Ok(format!(
                "Terraform applied successfully. Outputs: {:?}",
                result.outputs
            ))
        } else if result.has_changes {
            Ok(format!(
                "Terraform plan has changes (dry_run=true):\n{}",
                result.plan_output
            ))
        } else {
            Ok("Infrastructure in sync — no changes needed".into())
        }
    }

    async fn run_build(
        &self,
        slug: &str,
        config: &ProjectDeployConfig,
        source_dir: &PathBuf,
        veil_file: &str,
    ) -> Result<String, String> {
        // Contribution builds use their own pipeline (vite library mode)
        if config.deploy_type == DeployType::Contribution {
            let contribution = config.contribution.as_ref().ok_or(
                "deploy type is 'contribution' but [deploy.contribution] section is missing"
            )?;
            let result =
                build_contribution::run(slug, veil_file, source_dir, contribution).await?;
            // Store artifact hash in the job
            {
                let mut jobs = self.jobs.lock().await;
                if let Some(job) = jobs.values_mut().find(|j| {
                    j.project_slug == slug && j.status == JobStatus::Running
                }) {
                    job.artifact_hash = Some(result.bundle_hash.clone());
                }
            }
            return Ok(format!(
                "Contribution build complete. Bundle: {} (hash: {}). CSS: {:?}",
                result.bundle_path, result.bundle_hash, result.css_path
            ));
        }

        let build_config = match config.build.as_ref() {
            Some(b) => b,
            None => return Ok("No build config — skipping build step".into()),
        };

        match build_config.target {
            BuildTarget::Rust => {
                let rust_target = build_config
                    .rust_target
                    .as_deref()
                    .unwrap_or("x86_64-unknown-linux-gnu");
                let result =
                    build_rust::run(slug, veil_file, source_dir, rust_target).await?;

                // Store artifact hash in the job
                {
                    let mut jobs = self.jobs.lock().await;
                    // Find job by project slug (active job)
                    if let Some(job) = jobs.values_mut().find(|j| {
                        j.project_slug == slug && j.status == JobStatus::Running
                    }) {
                        job.artifact_hash = Some(result.artifact_hash.clone());
                    }
                }

                Ok(format!(
                    "Rust build complete. Artifact: {} (hash: {})",
                    result.artifact_path, result.artifact_hash
                ))
            }
            BuildTarget::Typescript => {
                let result = build_frontend::run(slug, veil_file, source_dir).await?;
                Ok(format!("Frontend build complete. Output: {}", result.build_dir))
            }
        }
    }

    async fn run_deploy(
        &self,
        slug: &str,
        config: &ProjectDeployConfig,
        dry_run: bool,
    ) -> Result<String, String> {
        if dry_run {
            return Ok("Dry run — skipping deploy step".into());
        }

        let artifact_bucket = config
            .artifacts
            .as_ref()
            .map(|a| a.bucket.as_str())
            .unwrap_or("veil-deploy-artifacts");

        match config.deploy_type {
            DeployType::Lambda => self.deploy_lambda(slug, artifact_bucket).await,
            DeployType::Frontend => self.deploy_frontend(slug, artifact_bucket).await,
            DeployType::Ecs => self.deploy_ecs(slug, artifact_bucket).await,
            DeployType::Contribution => {
                let contribution = config.contribution.as_ref().ok_or(
                    "deploy type is 'contribution' but [deploy.contribution] section is missing"
                )?;
                self.deploy_contribution(slug, contribution).await
            }
        }
    }

    async fn deploy_lambda(&self, slug: &str, artifact_bucket: &str) -> Result<String, String> {
        let artifact_path = config::build_output_dir(slug).join("lambda.zip");
        if !artifact_path.exists() {
            return Err("No lambda.zip found — did the build step run?".into());
        }

        let version = Utc::now().format("%Y%m%d%H%M%S").to_string();

        // Derive function names from project slug (dlx-bus convention)
        let function_names = vec![
            format!("veil-{slug}-api"),
            format!("veil-{slug}-consumer"),
        ];

        let result = deploy_lambda::run(
            slug,
            &artifact_path,
            artifact_bucket,
            &version,
            &function_names,
            &self.s3_client,
            &self.lambda_client,
        )
        .await?;

        Ok(format!(
            "Lambda deploy complete. S3 key: {}. Functions updated: {:?}",
            result.s3_key,
            result.updated_functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        ))
    }

    async fn deploy_frontend(&self, slug: &str, artifact_bucket: &str) -> Result<String, String> {
        let gen_dir = config::generated_dir(slug);
        let build_dir = if gen_dir.join("build").exists() {
            gen_dir.join("build")
        } else if gen_dir.join("dist").exists() {
            gen_dir.join("dist")
        } else {
            return Err("No build/dist directory found — did the build step run?".into());
        };

        // CloudFront distribution ID from env var (set by terraform outputs)
        let dist_id = std::env::var(format!(
            "VEIL_CF_DIST_{}",
            slug.to_uppercase().replace('-', "_")
        ))
        .ok();

        let result = deploy_frontend::run(
            slug,
            &build_dir.to_string_lossy(),
            artifact_bucket,
            dist_id.as_deref(),
        )
        .await?;

        Ok(format!(
            "Frontend deploy complete. Bucket: {}. Invalidation: {:?}",
            result.bucket, result.invalidation_id
        ))
    }

    async fn deploy_ecs(&self, slug: &str, artifact_bucket: &str) -> Result<String, String> {
        // ECS deploy: build Docker image → push to ECR → update ECS service
        let gen_dir = config::generated_dir(slug);

        // Expect a Dockerfile in the generated output
        let dockerfile = gen_dir.join("Dockerfile");
        if !dockerfile.exists() {
            return Err("No Dockerfile found in generated output for ECS deploy".into());
        }

        // Determine ECR repository and ECS cluster/service from env vars
        let ecr_repo = std::env::var(format!(
            "VEIL_ECR_REPO_{}",
            slug.to_uppercase().replace('-', "_")
        ))
        .unwrap_or_else(|_| format!("veil-{slug}"));

        let ecs_cluster = std::env::var("VEIL_ECS_CLUSTER")
            .unwrap_or_else(|_| "veil-cluster".to_string());
        let ecs_service = format!("veil-{slug}");

        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-west-2".to_string());
        let account_id = self.get_aws_account_id().await?;

        let image_tag = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let full_image = format!(
            "{account_id}.dkr.ecr.{region}.amazonaws.com/{ecr_repo}:{image_tag}"
        );

        // Step 1: Docker build
        let build_out = Command::new("docker")
            .args(["build", "-t", &full_image, "."])
            .current_dir(&gen_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("docker build failed to start: {e}"))?;

        if !build_out.status.success() {
            let stderr = String::from_utf8_lossy(&build_out.stderr);
            return Err(format!("docker build failed: {stderr}"));
        }
        info!(slug, image = %full_image, "Docker image built");

        // Step 2: ECR login
        let login_out = Command::new("aws")
            .args([
                "ecr",
                "get-login-password",
                "--region",
                &region,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("ECR login failed: {e}"))?;

        if !login_out.status.success() {
            return Err("Failed to get ECR login password".into());
        }

        let ecr_password = String::from_utf8_lossy(&login_out.stdout).trim().to_string();
        let ecr_endpoint = format!(
            "{account_id}.dkr.ecr.{region}.amazonaws.com"
        );

        let docker_login = Command::new("docker")
            .args(["login", "--username", "AWS", "--password-stdin", &ecr_endpoint])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("docker login spawn failed: {e}"))?;

        if let Some(mut stdin) = docker_login.stdin {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(ecr_password.as_bytes()).await;
            drop(stdin);
        }

        // Step 3: Docker push
        let push_out = Command::new("docker")
            .args(["push", &full_image])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("docker push failed to start: {e}"))?;

        if !push_out.status.success() {
            let stderr = String::from_utf8_lossy(&push_out.stderr);
            return Err(format!("docker push failed: {stderr}"));
        }
        info!(slug, image = %full_image, "Image pushed to ECR");

        // Step 4: Update ECS service to use new image
        let update_out = Command::new("aws")
            .args([
                "ecs",
                "update-service",
                "--cluster",
                &ecs_cluster,
                "--service",
                &ecs_service,
                "--force-new-deployment",
                "--region",
                &region,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("ecs update-service failed: {e}"))?;

        if !update_out.status.success() {
            let stderr = String::from_utf8_lossy(&update_out.stderr);
            return Err(format!("ECS service update failed: {stderr}"));
        }
        info!(slug, cluster = %ecs_cluster, service = %ecs_service, "ECS service updated");

        // Upload artifact hash to S3 for audit trail
        let _ = self
            .s3_client
            .put_object()
            .bucket(artifact_bucket)
            .key(format!("deploys/{slug}/{image_tag}/image.txt"))
            .body(full_image.as_bytes().to_vec().into())
            .send()
            .await;

        Ok(format!(
            "ECS deploy complete. Image: {full_image}. Service: {ecs_service} in {ecs_cluster}"
        ))
    }

    /// Deploy a contribution bundle: upload to S3 and register with the runtime API.
    async fn deploy_contribution(
        &self,
        slug: &str,
        contribution: &ContributionConfig,
    ) -> Result<String, String> {
        let gen_dir = config::generated_dir(slug);
        let dist_dir = gen_dir.join("dist");
        let bundle_path = dist_dir.join("index.js");

        if !bundle_path.exists() {
            return Err("dist/index.js not found — did the contribution build step run?".into());
        }

        let version = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
        let contribution_id = &contribution.contribution_id;
        let bucket = &contribution.bucket;

        // Step 1: Upload index.js to S3
        let js_key = format!("{contribution_id}/{version}/index.js");
        let js_bytes = tokio::fs::read(&bundle_path)
            .await
            .map_err(|e| format!("read bundle: {e}"))?;

        self.s3_client
            .put_object()
            .bucket(bucket)
            .key(&js_key)
            .body(js_bytes.into())
            .content_type("application/javascript")
            .cache_control("public, max-age=31536000, immutable")
            .send()
            .await
            .map_err(|e| format!("S3 put index.js failed: {e}"))?;

        info!(slug, key = %js_key, bucket, "contribution bundle uploaded");

        // Step 2: Upload style.css if it exists
        let css_key = {
            let css_path = dist_dir.join("style.css");
            if css_path.exists() {
                let css_key = format!("{contribution_id}/{version}/style.css");
                let css_bytes = tokio::fs::read(&css_path)
                    .await
                    .map_err(|e| format!("read css: {e}"))?;

                self.s3_client
                    .put_object()
                    .bucket(bucket)
                    .key(&css_key)
                    .body(css_bytes.into())
                    .content_type("text/css")
                    .cache_control("public, max-age=31536000, immutable")
                    .send()
                    .await
                    .map_err(|e| format!("S3 put style.css failed: {e}"))?;

                info!(slug, key = %css_key, bucket, "contribution CSS uploaded");
                Some(css_key)
            } else {
                None
            }
        };

        // Step 3: Compute bundle URL (CDN or direct S3)
        let s3_base = format!("https://{bucket}.s3.amazonaws.com");
        let base_url = contribution
            .cdn_base_url
            .as_deref()
            .unwrap_or(&s3_base);
        let bundle_url = format!("{base_url}/{js_key}");
        let css_url = css_key.as_ref().map(|k| format!("{base_url}/{k}"));

        // Step 4: Register contribution with runtime API (self — POST /api/contributions)
        let registration_body = serde_json::json!({
            "app_id": contribution.app_id,
            "id": contribution.contribution_id,
            "name": contribution.name,
            "version": version,
            "bundle_url": bundle_url,
            "css_url": css_url,
            "enabled": true,
            "order": contribution.order,
            "slots": contribution.slots,
        });

        // Use internal HTTP call to self (runtime API)
        let runtime_url = std::env::var("VEIL_RUNTIME_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{runtime_url}/api/contributions"))
            .json(&registration_body)
            .send()
            .await
            .map_err(|e| format!("contribution registration request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "contribution registration failed (HTTP {status}): {body}"
            ));
        }

        info!(
            slug,
            contribution_id,
            version = %version,
            bundle_url = %bundle_url,
            "contribution registered"
        );

        Ok(format!(
            "Contribution deployed. Bundle: {bundle_url}. Version: {version}. Registered with app '{}'.",
            contribution.app_id
        ))
    }

    async fn get_aws_account_id(&self) -> Result<String, String> {
        let output = Command::new("aws")
            .args(["sts", "get-caller-identity", "--query", "Account", "--output", "text"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("sts get-caller-identity failed: {e}"))?;

        if !output.status.success() {
            return Err("Failed to get AWS account ID".into());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    // ─── Cleanup ────────────────────────────────────────────────────────────

    async fn cleanup_working_dir(&self, slug: &str) {
        let dir = config::working_dir(slug);
        if dir.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                warn!(slug, "Failed to cleanup working dir: {e}");
            }
        }
    }

    // ─── Job state mutations ────────────────────────────────────────────────

    async fn update_step_status(
        &self,
        job_id: &str,
        step_kind: &DeployStepKind,
        status: StepStatus,
    ) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(job_id) {
            if let Some(step) = job.steps.iter_mut().find(|s| s.step == *step_kind) {
                step.status = status;
                if step.started_at.is_none() {
                    step.started_at = Some(Utc::now());
                }
            } else {
                let mut sr = StepResult::pending(step_kind.clone());
                sr.status = status;
                sr.started_at = Some(Utc::now());
                job.steps.push(sr);
            }
        }
    }

    async fn complete_step(&self, job_id: &str, step_kind: &DeployStepKind, output: String) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(job_id) {
            if let Some(step) = job.steps.iter_mut().find(|s| s.step == *step_kind) {
                step.status = StepStatus::Completed;
                step.completed_at = Some(Utc::now());
                step.output = Some(output);
            }
        }
    }

    async fn fail_step(&self, job_id: &str, step_kind: &DeployStepKind, error: String) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(job_id) {
            if let Some(step) = job.steps.iter_mut().find(|s| s.step == *step_kind) {
                step.status = StepStatus::Failed;
                step.completed_at = Some(Utc::now());
                step.error = Some(error);
            }
        }
    }

    async fn complete_job(&self, job_id: &str) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Completed;
            job.completed_at = Some(Utc::now());
            let slug = job.project_slug.clone();
            let completed = job.clone();
            drop(jobs);
            self.history
                .lock()
                .await
                .entry(slug)
                .or_default()
                .push(completed);
        }
    }

    async fn fail_job(&self, job_id: &str, error: &str) {
        let mut jobs = self.jobs.lock().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Failed;
            job.completed_at = Some(Utc::now());
            job.error = Some(error.to_string());
            let slug = job.project_slug.clone();
            let failed = job.clone();
            drop(jobs);
            self.history
                .lock()
                .await
                .entry(slug)
                .or_default()
                .push(failed);
        }
    }

    async fn release_project_lock(&self, slug: &str) {
        self.active_per_project.lock().await.remove(slug);
    }
}

// ─── Configuration parsing ──────────────────────────────────────────────────

/// Parse deploy config from veil.toml content.
/// Supports the [deploy] table with type, infrastructure, build, artifacts, gates.
fn parse_toml_deploy_config(content: &str) -> Result<ProjectDeployConfig, String> {
    // Parse the TOML content manually (avoiding adding toml crate dep).
    // We look for key = value patterns in [deploy.*] sections.
    let mut config = ProjectDeployConfig::default();
    let mut current_section = String::new();
    // For contribution slots, accumulate JSON-like values from [deploy.contribution.slots.*]
    let mut contribution_slots: serde_json::Map<String, serde_json::Value> = Default::default();
    let mut contribution_externals: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Section headers
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].to_string();
            continue;
        }

        // Key = value
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');

            match current_section.as_str() {
                "deploy" => match key {
                    "type" => {
                        config.deploy_type = match value {
                            "frontend" => DeployType::Frontend,
                            "ecs" => DeployType::Ecs,
                            "contribution" => DeployType::Contribution,
                            _ => DeployType::Lambda,
                        };
                    }
                    _ => {}
                },
                "deploy.infrastructure" => match key {
                    "backend_bucket" => {
                        config.infrastructure.get_or_insert_with(|| InfraConfig {
                            backend_bucket: String::new(),
                            backend_key: String::new(),
                            backend_region: "us-west-2".into(),
                        }).backend_bucket = value.to_string();
                    }
                    "backend_key" => {
                        config.infrastructure.get_or_insert_with(|| InfraConfig {
                            backend_bucket: String::new(),
                            backend_key: String::new(),
                            backend_region: "us-west-2".into(),
                        }).backend_key = value.to_string();
                    }
                    "backend_region" => {
                        config.infrastructure.get_or_insert_with(|| InfraConfig {
                            backend_bucket: String::new(),
                            backend_key: String::new(),
                            backend_region: "us-west-2".into(),
                        }).backend_region = value.to_string();
                    }
                    _ => {}
                },
                "deploy.build" => match key {
                    "target" => {
                        let target = match value {
                            "typescript" => BuildTarget::Typescript,
                            _ => BuildTarget::Rust,
                        };
                        config.build.get_or_insert_with(|| BuildConfig {
                            target: BuildTarget::Rust,
                            rust_target: None,
                        }).target = target;
                    }
                    "rust_target" => {
                        config.build.get_or_insert_with(|| BuildConfig {
                            target: BuildTarget::Rust,
                            rust_target: None,
                        }).rust_target = Some(value.to_string());
                    }
                    _ => {}
                },
                "deploy.artifacts" => match key {
                    "bucket" => {
                        config.artifacts.get_or_insert_with(|| ArtifactConfig {
                            bucket: String::new(),
                        }).bucket = value.to_string();
                    }
                    _ => {}
                },
                "deploy.contribution" => {
                    let contrib = config.contribution.get_or_insert_with(|| ContributionConfig {
                        app_id: String::new(),
                        contribution_id: String::new(),
                        name: String::new(),
                        bucket: "dlx-ai-contributions".to_string(),
                        cdn_base_url: None,
                        order: 100,
                        slots: serde_json::Value::Object(Default::default()),
                        entry: "src/index.ts".to_string(),
                        externals: vec!["svelte".to_string(), "svelte/internal".to_string()],
                    });
                    match key {
                        "app_id" => contrib.app_id = value.to_string(),
                        "id" => contrib.contribution_id = value.to_string(),
                        "name" => contrib.name = value.to_string(),
                        "bucket" => contrib.bucket = value.to_string(),
                        "cdn_base_url" => contrib.cdn_base_url = Some(value.to_string()),
                        "order" => contrib.order = value.parse().unwrap_or(100),
                        "entry" => contrib.entry = value.to_string(),
                        "externals" => {
                            // Parse comma-separated or bracket array
                            let raw = value.trim_matches('[').trim_matches(']');
                            contribution_externals = raw
                                .split(',')
                                .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                        "slots" => {
                            // If slots is specified as inline JSON
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) {
                                contrib.slots = parsed;
                            }
                        }
                        _ => {}
                    }
                },
                "deploy.gates" => {
                    config.gates.insert(
                        key.to_string(),
                        GatePolicy::from_str(value),
                    );
                }
                section if section.starts_with("deploy.contribution.slots.") => {
                    // [deploy.contribution.slots.main-menu] etc.
                    // Accumulate as JSON entries; the value is inline JSON array
                    let slot_name = section.strip_prefix("deploy.contribution.slots.").unwrap_or("");
                    if !slot_name.is_empty() {
                        // Each line in this section is a JSON value for this slot
                        let raw_value = trimmed.split_once('=')
                            .map(|(_, v)| v.trim())
                            .unwrap_or(value);
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw_value) {
                            contribution_slots.insert(slot_name.to_string(), parsed);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Merge accumulated contribution slots and externals
    if let Some(ref mut contrib) = config.contribution {
        if !contribution_slots.is_empty() {
            contrib.slots = serde_json::Value::Object(contribution_slots);
        }
        if !contribution_externals.is_empty() {
            contrib.externals = contribution_externals;
        }
    }

    Ok(config)
}

#[cfg(test)]
mod resolve_veil_source_tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(deploy_type: DeployType, target: Option<BuildTarget>) -> ProjectDeployConfig {
        ProjectDeployConfig {
            deploy_type,
            infrastructure: None,
            build: target.map(|t| BuildConfig {
                target: t,
                rust_target: None,
            }),
            artifacts: None,
            contribution: None,
            gates: HashMap::new(),
        }
    }

    #[test]
    fn contribution_prefers_ui_veil_when_present() {
        let dir = std::env::temp_dir().join(format!("veilsrc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ui.veil"), b"pkg X").unwrap();
        std::fs::write(dir.join("main.veil"), b"pkg Y").unwrap();
        let c = cfg(DeployType::Contribution, None);
        assert_eq!(resolve_veil_source(&c, &dir, ""), "ui.veil");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn contribution_falls_back_to_main_when_no_ui() {
        let dir = std::env::temp_dir().join(format!("veilsrc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.veil"), b"pkg Y").unwrap();
        let c = cfg(DeployType::Contribution, None);
        assert_eq!(resolve_veil_source(&c, &dir, ""), "main.veil");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lambda_always_uses_main_even_with_ui_present() {
        let dir = std::env::temp_dir().join(format!("veilsrc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ui.veil"), b"pkg X").unwrap();
        std::fs::write(dir.join("main.veil"), b"pkg Y").unwrap();
        let c = cfg(DeployType::Lambda, Some(BuildTarget::Rust));
        assert_eq!(resolve_veil_source(&c, &dir, ""), "main.veil");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frontend_typescript_prefers_ui_veil() {
        let dir = std::env::temp_dir().join(format!("veilsrc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ui.veil"), b"pkg X").unwrap();
        std::fs::write(dir.join("main.veil"), b"pkg Y").unwrap();
        let c = cfg(DeployType::Frontend, Some(BuildTarget::Typescript));
        assert_eq!(resolve_veil_source(&c, &dir, ""), "ui.veil");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_contribution_override_wins() {
        let dir = std::env::temp_dir().join(format!("veilsrc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ui.veil"), b"pkg X").unwrap();
        let toml = "[deploy]\ntype=\"contribution\"\n[deploy.contribution]\nveil=\"custom.veil\"\n";
        let c = cfg(DeployType::Contribution, None);
        assert_eq!(resolve_veil_source(&c, &dir, toml), "custom.veil");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_top_level_main_override_wins_for_backend() {
        let dir = std::env::temp_dir().join(format!("veilsrc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let toml = "main = \"backend.veil\"\n[deploy]\ntype=\"lambda\"\n";
        let c = cfg(DeployType::Lambda, Some(BuildTarget::Rust));
        assert_eq!(resolve_veil_source(&c, &dir, toml), "backend.veil");
        std::fs::remove_dir_all(&dir).ok();
    }
}
