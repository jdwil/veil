//! WebSocket-based deploy streaming.
//!
//! The client connects to `/api/projects/{slug}/deploy/ws` and sends a start message.
//! The server runs terraform (or build+deploy) and streams structured events back:
//!
//! Events (server → client, JSON):
//!   { "type": "started", "job_id": "...", "steps": [...] }
//!   { "type": "step_start", "step": "init" }
//!   { "type": "log", "step": "init", "line": "Initializing provider..." }
//!   { "type": "step_done", "step": "init", "ok": true }
//!   { "type": "step_start", "step": "plan" }
//!   { "type": "resource", "action": "create", "address": "aws_s3_bucket.frontend", "kind": "S3 Bucket", "name": "dlx-ai-frontend" }
//!   { "type": "step_done", "step": "plan", "ok": true, "creates": 9, "updates": 0, "destroys": 0 }
//!   { "type": "step_start", "step": "apply" }
//!   { "type": "progress", "step": "apply", "resource": "aws_s3_bucket.frontend", "status": "creating" }
//!   { "type": "progress", "step": "apply", "resource": "aws_s3_bucket.frontend", "status": "created", "elapsed": "2s" }
//!   { "type": "step_done", "step": "apply", "ok": true }
//!   { "type": "done", "ok": true, "outputs": { "site_url": "https://ai.dev.dashlx.com" } }
//!   { "type": "error", "message": "..." }
//!
//! Client → server:
//!   { "action": "start", "environment": "dev", "steps": ["infrastructure"] }
//!   { "action": "cancel" }

use axum::extract::ws::{Message, WebSocket};
use serde_json::json;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{error, info, warn};

use super::config;
use super::types::InfraConfig;

/// Run the full terraform deploy over a websocket, streaming output in real time.
pub async fn run_terraform_ws(
    ws: &mut WebSocket,
    slug: &str,
    tf_files: &[(String, Vec<u8>)],
    infra_config: &InfraConfig,
) {
    let job_id = uuid::Uuid::new_v4().to_string();
    let tf_dir = config::terraform_dir(slug);

    // Send started
    let _ = send(ws, json!({
        "type": "started",
        "job_id": &job_id,
        "steps": ["init", "plan", "apply"],
    })).await;

    // Write terraform files to working dir
    if let Err(e) = write_tf_files(&tf_dir, tf_files).await {
        let _ = send(ws, json!({"type": "error", "message": e})).await;
        return;
    }

    // ─── INIT ────────────────────────────────────────────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "init"})).await;
    match run_streaming(ws, "init", &tf_dir, "terraform", &[
        "init", "-no-color",
        &format!("-backend-config=bucket={}", infra_config.backend_bucket),
        &format!("-backend-config=key={}", infra_config.backend_key),
        &format!("-backend-config=region={}", infra_config.backend_region),
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "step_done", "step": "init", "ok": true})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "init", "ok": false, "error": e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("terraform init failed: {e}")})).await;
            return;
        }
    }

    // ─── PLAN ────────────────────────────────────────────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "plan"})).await;
    match run_streaming(ws, "plan", &tf_dir, "terraform", &[
        "plan", "-out=plan.tfplan", "-no-color",
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "step_done", "step": "plan", "ok": true})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "plan", "ok": false, "error": e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("terraform plan failed: {e}")})).await;
            return;
        }
    }

    // ─── APPLY ───────────────────────────────────────────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "apply"})).await;
    match run_streaming(ws, "apply", &tf_dir, "terraform", &[
        "apply", "-no-color", "-auto-approve", "plan.tfplan",
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "step_done", "step": "apply", "ok": true})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "apply", "ok": false, "error": e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("terraform apply failed: {e}")})).await;
            return;
        }
    }

    // ─── OUTPUTS ─────────────────────────────────────────────────────────────
    let outputs = capture_outputs(&tf_dir).await.unwrap_or_default();

    let _ = send(ws, json!({
        "type": "done",
        "ok": true,
        "job_id": &job_id,
        "outputs": outputs,
    })).await;

    info!(slug, job_id, "terraform deploy complete via websocket");
}

/// Run a full frontend deploy: generate → build → S3 sync → CloudFront invalidate.
/// Streams progress over the websocket.
pub async fn run_frontend_deploy_ws(
    ws: &mut WebSocket,
    slug: &str,
    build_config: &FrontendBuildConfig,
) {
    let job_id = uuid::Uuid::new_v4().to_string();
    let work_dir = config::generated_dir(slug);

    let steps: Vec<&str> = vec!["generate", "build", "deploy"];
    let _ = send(ws, json!({
        "type": "started",
        "job_id": &job_id,
        "steps": steps,
    })).await;

    // ─── GENERATE (veil gen) ─────────────────────────────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "generate"})).await;

    // Resolve veil binary: check PATH first, then sibling to current exe
    let veil_bin = which_veil();

    match run_streaming(ws, "generate", std::path::Path::new("/tmp"), &veil_bin, &[
        "gen", &build_config.main_veil_path, "-t", "typescript", "-o", work_dir.to_str().unwrap_or("/tmp/gen"),
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "step_done", "step": "generate", "ok": true})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "generate", "ok": false, "error": &e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("veil gen failed: {e}")})).await;
            return;
        }
    }

    // ─── BUILD (npm install + npm run build) ─────────────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "build"})).await;
    for cmd_str in &build_config.commands {
        let parts: Vec<&str> = cmd_str.split_whitespace().collect();
        if parts.is_empty() { continue; }
        let (cmd, args) = (parts[0], &parts[1..]);
        match run_streaming(ws, "build", &work_dir, cmd, args).await {
            Ok(_) => {}
            Err(e) => {
                let _ = send(ws, json!({"type": "step_done", "step": "build", "ok": false, "error": &e})).await;
                let _ = send(ws, json!({"type": "error", "message": format!("{cmd_str} failed: {e}")})).await;
                return;
            }
        }
    }
    let _ = send(ws, json!({"type": "step_done", "step": "build", "ok": true})).await;

    // ─── DEPLOY (S3 sync + CloudFront invalidation) ──────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "deploy"})).await;

    let build_output = work_dir.join(&build_config.output_dir);
    let build_dir_str = build_output.to_str().unwrap_or(".");

    // S3 sync
    match run_streaming(ws, "deploy", &work_dir, "aws", &[
        "s3", "sync", build_dir_str, &format!("s3://{}/", build_config.bucket), "--delete", "--no-progress",
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({
                "type": "progress", "step": "deploy",
                "resource": &build_config.bucket, "status": "synced",
            })).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "deploy", "ok": false, "error": &e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("S3 sync failed: {e}")})).await;
            return;
        }
    }

    // CloudFront invalidation
    if let Some(ref dist_id) = build_config.cloudfront_distribution_id {
        match run_streaming(ws, "deploy", &work_dir, "aws", &[
            "cloudfront", "create-invalidation", "--distribution-id", dist_id, "--paths", "/*",
        ]).await {
            Ok(_) => {
                let _ = send(ws, json!({
                    "type": "progress", "step": "deploy",
                    "resource": "CloudFront", "status": "invalidated",
                })).await;
            }
            Err(e) => {
                // Non-fatal
                warn!(slug, "CloudFront invalidation failed: {e}");
                let _ = send(ws, json!({"type": "log", "step": "deploy", "line": format!("CloudFront invalidation failed (non-fatal): {e}")})).await;
            }
        }
    }

    let _ = send(ws, json!({"type": "step_done", "step": "deploy", "ok": true})).await;
    let _ = send(ws, json!({
        "type": "done",
        "ok": true,
        "job_id": &job_id,
        "outputs": {
            "url": format!("https://{}", build_config.domain.as_deref().unwrap_or("(unknown)")),
            "bucket": &build_config.bucket,
        },
    })).await;

    info!(slug, job_id, bucket = %build_config.bucket, "frontend deploy complete via websocket");
}

/// Build + deploy a contribution (ui.veil → ES-module bundle) over a websocket,
/// streaming progress. Mirrors the frontend path but targets the shared
/// contributions bucket + re-registers the manifest on the runtime.
pub async fn run_contribution_deploy_ws(
    ws: &mut WebSocket,
    slug: &str,
    source_dir: &Path,
    contribution: &super::types::ContributionConfig,
    component_deps: &[super::component_deps::ComponentDep],
) {
    let job_id = uuid::Uuid::new_v4().to_string();
    let steps: Vec<&str> = vec!["build", "upload", "register"];
    let _ = send(ws, json!({"type": "started", "job_id": &job_id, "steps": steps})).await;

    // ─── BUILD (veil gen ui.veil → vite library bundle) ──────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "build"})).await;
    let veil_file = "ui.veil";
    let build_res =
        super::build_contribution::run(slug, veil_file, source_dir, contribution, component_deps)
            .await;
    let build = match build_res {
        Ok(b) => {
            let _ = send(ws, json!({"type": "step_done", "step": "build", "ok": true})).await;
            b
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "build", "ok": false, "error": &e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("contribution build failed: {e}")})).await;
            return;
        }
    };

    // ─── UPLOAD (aws s3 cp bundle + css to contributions bucket) ─────────────
    let _ = send(ws, json!({"type": "step_start", "step": "upload"})).await;
    let version = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let bucket = &contribution.bucket;
    let cid = &contribution.contribution_id;
    let js_key = format!("{cid}/{version}/index.js");
    let work = std::path::Path::new(&build.bundle_path)
        .parent()
        .unwrap_or(Path::new("."));

    match run_streaming(ws, "upload", work, "aws", &[
        "s3", "cp", &build.bundle_path, &format!("s3://{bucket}/{js_key}"),
        "--content-type", "application/javascript",
        "--cache-control", "public, max-age=31536000, immutable", "--no-progress",
    ]).await {
        Ok(_) => { let _ = send(ws, json!({"type":"progress","step":"upload","resource":&js_key,"status":"uploaded"})).await; }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "upload", "ok": false, "error": &e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("S3 upload failed: {e}")})).await;
            return;
        }
    }
    let css_key = if let Some(css_path) = &build.css_path {
        let key = format!("{cid}/{version}/style.css");
        let _ = run_streaming(ws, "upload", work, "aws", &[
            "s3", "cp", css_path, &format!("s3://{bucket}/{key}"),
            "--content-type", "text/css",
            "--cache-control", "public, max-age=31536000, immutable", "--no-progress",
        ]).await;
        Some(key)
    } else { None };
    let _ = send(ws, json!({"type": "step_done", "step": "upload", "ok": true})).await;

    // ─── REGISTER (re-register the manifest on the runtime) ──────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "register"})).await;
    let base = contribution.cdn_base_url.clone()
        .unwrap_or_else(|| format!("https://{bucket}.s3.amazonaws.com"));
    let bundle_url = format!("{base}/{js_key}");
    let css_url = css_key.as_ref().map(|k| format!("{base}/{k}"));
    let mut body = json!({
        "app_id": contribution.app_id,
        "id": contribution.contribution_id,
        "name": contribution.name,
        "version": version,
        "bundle_url": bundle_url,
        "css_url": css_url,
        "enabled": true,
        "order": contribution.order,
        "access": {"public": true},
        "slots": contribution.slots,
    });
    if css_url.is_none() { body.as_object_mut().map(|m| m.remove("css_url")); }
    let port = std::env::var("VEIL_PORT").unwrap_or_else(|_| "8080".into());
    let reg_url = format!("http://127.0.0.1:{port}/api/contributions");
    match reqwest::Client::new().post(&reg_url).json(&body).send().await {
        Ok(r) if r.status().is_success() => {
            let _ = send(ws, json!({"type": "step_done", "step": "register", "ok": true})).await;
        }
        Ok(r) => {
            let code = r.status().as_u16();
            let txt = r.text().await.unwrap_or_default();
            let _ = send(ws, json!({"type": "step_done", "step": "register", "ok": false, "error": format!("HTTP {code}: {txt}")})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("manifest register failed: HTTP {code}")})).await;
            return;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "register", "ok": false, "error": e.to_string()})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("manifest register request failed: {e}")})).await;
            return;
        }
    }

    let _ = send(ws, json!({
        "type": "done", "ok": true, "job_id": &job_id,
        "outputs": {"bundle_url": bundle_url, "version": version, "id": contribution.contribution_id},
    })).await;
    info!(slug, job_id, version, "contribution deploy complete via websocket");
}

/// Configuration for a frontend build+deploy.
pub struct FrontendBuildConfig {
    pub main_veil_path: String,
    pub commands: Vec<String>,
    pub output_dir: String,
    pub bucket: String,
    pub cloudfront_distribution_id: Option<String>,
    pub domain: Option<String>,
}

/// Configuration for a Lambda build+deploy.
pub struct LambdaBuildConfig {
    pub main_veil_path: String,
    pub rust_target: String,
    pub api_function_name: String,
    pub consumer_function_name: String,
    pub artifact_bucket: String,
    pub artifact_prefix: String,
}

/// Run a full Lambda deploy: generate → cargo build → zip → upload → update function code.
/// Streams progress over the websocket.
pub async fn run_lambda_deploy_ws(
    ws: &mut WebSocket,
    slug: &str,
    config: &LambdaBuildConfig,
) {
    let job_id = uuid::Uuid::new_v4().to_string();
    let work_dir = super::config::generated_dir(slug);

    let _ = send(ws, json!({
        "type": "started",
        "job_id": &job_id,
        "steps": ["generate", "build", "package", "deploy"],
    })).await;

    // ─── GENERATE (veil gen → Rust) ──────────────────────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "generate"})).await;
    let veil_bin = which_veil();
    match run_streaming(ws, "generate", std::path::Path::new("/tmp"), &veil_bin, &[
        "gen", &config.main_veil_path, "-t", "rust", "-o", work_dir.to_str().unwrap_or("/tmp/gen"),
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "step_done", "step": "generate", "ok": true})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "generate", "ok": false, "error": &e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("veil gen failed: {e}")})).await;
            return;
        }
    }

    // ─── BUILD (cargo lambda build — handles glibc compat + zip packaging) ───
    // cargo-lambda cross-compiles with the correct toolchain so the binary runs
    // on Lambda's provided.al2023 runtime (avoids GLIBC version mismatches from
    // building directly against the host's newer glibc).
    let _ = send(ws, json!({"type": "step_start", "step": "build"})).await;
    match run_streaming(ws, "build", &work_dir, "cargo", &[
        "lambda", "build", "--release", "--x86-64", "--output-format", "zip",
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "step_done", "step": "build", "ok": true})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "step_done", "step": "build", "ok": false, "error": &e})).await;
            let _ = send(ws, json!({"type": "error", "message": format!("cargo lambda build failed: {e}")})).await;
            return;
        }
    }

    // ─── PACKAGE (locate the cargo-lambda zip output) ────────────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "package"})).await;

    // cargo-lambda writes zips to target/lambda/{binary_name}/bootstrap.zip
    let lambda_out = work_dir.join("target/lambda");
    let zip_path = match find_lambda_zip(&lambda_out).await {
        Some(p) => p,
        None => {
            let _ = send(ws, json!({"type": "step_done", "step": "package", "ok": false, "error": format!("No bootstrap.zip found under {:?}", lambda_out)})).await;
            let _ = send(ws, json!({"type": "error", "message": "cargo lambda build did not produce a bootstrap.zip"})).await;
            return;
        }
    };
    let _ = send(ws, json!({"type": "log", "step": "package", "line": format!("Found Lambda package: {:?}", zip_path)})).await;
    let _ = send(ws, json!({"type": "step_done", "step": "package", "ok": true})).await;

    // ─── DEPLOY (upload to S3 + update Lambda function code) ─────────────────
    let _ = send(ws, json!({"type": "step_start", "step": "deploy"})).await;

    // Upload zip to S3
    let s3_key_api = format!("{}/api/bootstrap.zip", config.artifact_prefix);
    let s3_key_consumer = format!("{}/consumer/bootstrap.zip", config.artifact_prefix);
    let zip_str = zip_path.to_str().unwrap_or("");

    match run_streaming(ws, "deploy", &work_dir, "aws", &[
        "s3", "cp", zip_str, &format!("s3://{}/{}", config.artifact_bucket, s3_key_api),
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "progress", "step": "deploy", "resource": "S3 upload (api)", "status": "uploaded"})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "error", "message": format!("S3 upload failed: {e}")})).await;
            return;
        }
    }

    match run_streaming(ws, "deploy", &work_dir, "aws", &[
        "s3", "cp", zip_str, &format!("s3://{}/{}", config.artifact_bucket, s3_key_consumer),
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "progress", "step": "deploy", "resource": "S3 upload (consumer)", "status": "uploaded"})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "error", "message": format!("S3 upload failed: {e}")})).await;
            return;
        }
    }

    // Update Lambda function code
    match run_streaming(ws, "deploy", &work_dir, "aws", &[
        "lambda", "update-function-code",
        "--function-name", &config.api_function_name,
        "--s3-bucket", &config.artifact_bucket,
        "--s3-key", &s3_key_api,
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "progress", "step": "deploy", "resource": &config.api_function_name, "status": "updated"})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "error", "message": format!("Lambda update failed (api): {e}")})).await;
            return;
        }
    }

    match run_streaming(ws, "deploy", &work_dir, "aws", &[
        "lambda", "update-function-code",
        "--function-name", &config.consumer_function_name,
        "--s3-bucket", &config.artifact_bucket,
        "--s3-key", &s3_key_consumer,
    ]).await {
        Ok(_) => {
            let _ = send(ws, json!({"type": "progress", "step": "deploy", "resource": &config.consumer_function_name, "status": "updated"})).await;
        }
        Err(e) => {
            let _ = send(ws, json!({"type": "error", "message": format!("Lambda update failed (consumer): {e}")})).await;
            return;
        }
    }

    let _ = send(ws, json!({"type": "step_done", "step": "deploy", "ok": true})).await;
    let _ = send(ws, json!({
        "type": "done",
        "ok": true,
        "job_id": &job_id,
        "outputs": {
            "api_function": &config.api_function_name,
            "consumer_function": &config.consumer_function_name,
        },
    })).await;

    info!(slug, job_id, "lambda deploy complete via websocket");
}

/// Write tf files to the working directory.
async fn write_tf_files(tf_dir: &Path, tf_files: &[(String, Vec<u8>)]) -> Result<(), String> {
    tokio::fs::create_dir_all(tf_dir)
        .await
        .map_err(|e| format!("Failed to create terraform dir: {e}"))?;

    let tf_root = tf_dir
        .canonicalize()
        .unwrap_or_else(|_| tf_dir.to_path_buf());
    for (filename, content) in tf_files {
        let name = std::path::Path::new(filename);
        if name.is_absolute()
            || name
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!("invalid terraform path: {filename}"));
        }
        let path = tf_root.join(name);
        if !path.starts_with(&tf_root) {
            return Err(format!("terraform path escapes work dir: {filename}"));
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| format!("Failed to write {filename}: {e}"))?;
    }
    Ok(())
}

/// Run a command, streaming stdout+stderr line by line over the websocket.
/// Parses terraform-specific progress lines into structured events.
async fn run_streaming(
    ws: &mut WebSocket,
    step: &str,
    cwd: &Path,
    cmd: &str,
    args: &[&str],
) -> Result<(), String> {
    let mut child = Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{cmd} failed to start: {e}"))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    // Stream both stdout and stderr concurrently
    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        let event = parse_terraform_line(step, &text);
                        let _ = send(ws, event).await;
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        let clean = strip_ansi(&text);
                        let _ = send(ws, json!({
                            "type": "log",
                            "step": step,
                            "line": clean,
                            "stream": "stderr",
                        })).await;
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            }
        }
    }

    // Drain remaining stderr
    while let Ok(Some(text)) = stderr_reader.next_line().await {
        let clean = strip_ansi(&text);
        let _ = send(ws, json!({
            "type": "log",
            "step": step,
            "line": clean,
            "stream": "stderr",
        })).await;
    }

    let status = child.wait().await.map_err(|e| format!("wait failed: {e}"))?;

    // For plan, exit code 2 means "changes present" (success with diff)
    if step == "plan" && status.code() == Some(2) {
        return Ok(());
    }

    if !status.success() {
        return Err(format!("exit code {}", status.code().unwrap_or(-1)));
    }
    Ok(())
}

/// Parse a terraform output line into a structured websocket event.
fn parse_terraform_line(step: &str, line: &str) -> serde_json::Value {
    let trimmed = strip_ansi(line).trim().to_string();
    let trimmed = trimmed.as_str();

    // Apply phase: "aws_s3_bucket.frontend: Creating..."
    if trimmed.contains(": Creating...") {
        let resource = trimmed.split(": Creating").next().unwrap_or(trimmed);
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "creating",
        });
    }

    // Apply phase: "aws_s3_bucket.frontend: Creation complete after 2s [id=...]"
    if trimmed.contains(": Creation complete after ") {
        let resource = trimmed.split(": Creation").next().unwrap_or(trimmed);
        let elapsed = trimmed.split("after ").nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap_or("");
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "created",
            "elapsed": elapsed,
        });
    }

    // Apply phase: "aws_s3_bucket.frontend: Modifying..."
    if trimmed.contains(": Modifying...") {
        let resource = trimmed.split(": Modifying").next().unwrap_or(trimmed);
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "updating",
        });
    }

    // Apply phase: "aws_s3_bucket.frontend: Modifications complete after 1s"
    if trimmed.contains(": Modifications complete after ") {
        let resource = trimmed.split(": Modifications").next().unwrap_or(trimmed);
        let elapsed = trimmed.split("after ").nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap_or("");
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "updated",
            "elapsed": elapsed,
        });
    }

    // Apply phase: "aws_s3_bucket.frontend: Destroying..."
    if trimmed.contains(": Destroying...") {
        let resource = trimmed.split(": Destroying").next().unwrap_or(trimmed);
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "destroying",
        });
    }

    // Apply phase: "aws_s3_bucket.frontend: Destruction complete after 0s"
    if trimmed.contains(": Destruction complete after ") {
        let resource = trimmed.split(": Destruction").next().unwrap_or(trimmed);
        let elapsed = trimmed.split("after ").nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap_or("");
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "destroyed",
            "elapsed": elapsed,
        });
    }

    // Apply phase: "aws_s3_bucket.frontend: Still creating... [10s elapsed]"
    if trimmed.contains(": Still creating...") || trimmed.contains(": Still modifying...") || trimmed.contains(": Still destroying...") {
        let resource = trimmed.split(": Still").next().unwrap_or(trimmed);
        let elapsed = trimmed.split('[').nth(1)
            .and_then(|s| s.split(' ').next())
            .unwrap_or("");
        return json!({
            "type": "progress",
            "step": step,
            "resource": resource,
            "status": "waiting",
            "elapsed": elapsed,
        });
    }

    // Plan phase: "# aws_s3_bucket.frontend will be created"
    if trimmed.starts_with("# ") && trimmed.contains(" will be ") {
        let resource = trimmed.strip_prefix("# ").unwrap_or(trimmed)
            .split(" will be").next().unwrap_or(trimmed);
        let action = if trimmed.contains("created") { "create" }
            else if trimmed.contains("updated") { "update" }
            else if trimmed.contains("destroyed") { "destroy" }
            else { "change" };
        return json!({
            "type": "resource",
            "step": step,
            "action": action,
            "address": resource,
        });
    }

    // Plan summary: "Plan: 9 to add, 0 to change, 0 to destroy."
    if trimmed.starts_with("Plan: ") {
        return json!({
            "type": "plan_summary",
            "step": step,
            "line": trimmed,
        });
    }

    // Apply complete: "Apply complete! Resources: 9 added, 0 changed, 0 destroyed."
    if trimmed.starts_with("Apply complete!") {
        return json!({
            "type": "apply_summary",
            "step": step,
            "line": trimmed,
        });
    }

    // Default: generic log line
    json!({
        "type": "log",
        "step": step,
        "line": line,
    })
}

/// Capture terraform outputs as key-value pairs.
pub async fn capture_outputs(tf_dir: &Path) -> Result<serde_json::Value, String> {
    let output = Command::new("terraform")
        .args(["output", "-json"])
        .current_dir(tf_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("terraform output failed: {e}"))?;

    if !output.status.success() {
        return Ok(json!({}));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw: serde_json::Value = serde_json::from_str(&stdout).unwrap_or(json!({}));

    // Flatten from { "key": { "value": "...", "type": "..." } } to { "key": "value" }
    let mut flat = serde_json::Map::new();
    if let Some(obj) = raw.as_object() {
        for (k, v) in obj {
            if let Some(val) = v.get("value") {
                flat.insert(k.clone(), val.clone());
            }
        }
    }
    Ok(serde_json::Value::Object(flat))
}

/// Send a JSON message over the websocket.
async fn send(ws: &mut WebSocket, msg: serde_json::Value) -> Result<(), ()> {
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|_| ())
}

/// Strip ANSI escape sequences from a string (colors, bold, dim, etc.)
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC + '[' + params + final byte
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Consume all parameter bytes (0x30-0x3F) and intermediate bytes (0x20-0x2F)
                while let Some(&next) = chars.peek() {
                    if (next >= '0' && next <= '?') || (next >= ' ' && next <= '/') {
                        chars.next();
                    } else {
                        break;
                    }
                }
                // Consume final byte (0x40-0x7E)
                if let Some(&next) = chars.peek() {
                    if next >= '@' && next <= '~' {
                        chars.next();
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Find the bootstrap.zip produced by `cargo lambda build --output-format zip`.
/// cargo-lambda writes to target/lambda/{binary_name}/bootstrap.zip — there may
/// be multiple binaries, so return the first bootstrap.zip found.
async fn find_lambda_zip(lambda_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut entries = tokio::fs::read_dir(lambda_dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if entry.metadata().await.map(|m| m.is_dir()).unwrap_or(false) {
            let candidate = path.join("bootstrap.zip");
            if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolve the veil CLI binary path.
/// Checks: sibling of current exe → PATH → /usr/local/bin/veil
fn which_veil() -> String {
    // Check sibling to current executable (target/release/veil next to target/release/veil-runtime)
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap_or(exe.as_path()).join("veil");
        if sibling.exists() {
            return sibling.to_string_lossy().to_string();
        }
    }
    // Check PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = std::path::Path::new(dir).join("veil");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    // Fallback
    "/usr/local/bin/veil".to_string()
}
