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

/// Configuration for a frontend build+deploy.
pub struct FrontendBuildConfig {
    pub main_veil_path: String,
    pub commands: Vec<String>,
    pub output_dir: String,
    pub bucket: String,
    pub cloudfront_distribution_id: Option<String>,
    pub domain: Option<String>,
}

/// Write tf files to the working directory.
async fn write_tf_files(tf_dir: &Path, tf_files: &[(String, Vec<u8>)]) -> Result<(), String> {
    tokio::fs::create_dir_all(tf_dir)
        .await
        .map_err(|e| format!("Failed to create terraform dir: {e}"))?;

    for (filename, content) in tf_files {
        let path = tf_dir.join(filename);
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
                        let _ = send(ws, json!({
                            "type": "log",
                            "step": step,
                            "line": text,
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
        let _ = send(ws, json!({
            "type": "log",
            "step": step,
            "line": text,
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
    let trimmed = line.trim();

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
