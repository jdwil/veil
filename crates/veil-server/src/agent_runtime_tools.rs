//! Agent runtime observability tools (AGT-020–028).
//!
//! Shared logic for MCP + Rig: dual-loop status/logs, generated tree reads,
//! scoped HTTP probes, and restarts. Pure helpers take `project_root` so both
//! dispatch paths stay thin.

use std::path::{Component, Path, PathBuf};

use crate::devloop::{self, parse_project_config, global_dev_loops, get_or_create_dev_loop};
use crate::project_layout::project_display_name;

// ─── Project resolution ────────────────────────────────────────────────────

fn resolve_project_name(project_root: &Path, explicit: Option<&str>) -> String {
    explicit
        .map(|s| s.to_string())
        .or_else(|| {
            crate::provider::hub::CURRENT_PROJECT
                .try_with(|n| n.clone())
                .ok()
        })
        .unwrap_or_else(|| project_display_name(project_root))
}

fn ensure_dev_loop(project_root: &Path, project_name: &str) -> Result<(), String> {
    let loops = global_dev_loops().ok_or_else(|| {
        "devloop not registered (server must call set_global_dev_loops)".to_string()
    })?;
    get_or_create_dev_loop(loops, project_name, project_root)
}

// ─── AGT-020 dev_status ────────────────────────────────────────────────────

/// Format dual-loop target status for the agent.
pub fn tool_dev_status(
    project_root: &Path,
    name_filter: Option<&str>,
    project_name: Option<&str>,
) -> Result<String, String> {
    let pname = resolve_project_name(project_root, project_name);
    ensure_dev_loop(project_root, &pname)?;
    let loops = global_dev_loops().unwrap();
    let mut map = loops.lock().map_err(|e| format!("lock: {e}"))?;
    let dev = map
        .get_mut(&pname)
        .ok_or_else(|| format!("no devloop for {pname}"))?;
    dev.poll_health();

    let mut lines = vec![format!("project: {pname}")];
    let mut any = false;
    for s in dev.status() {
        if let Some(n) = name_filter {
            if s.name != n {
                continue;
            }
        }
        any = true;
        lines.push(format!(
            "- {name}: status={status:?} package={pkg} target={lang} output={out} port={port} attached={att} last_gen={gen} last_error={err}",
            name = s.name,
            status = s.status,
            pkg = s.config.package,
            lang = s.config.target,
            out = s.config.output,
            port = s.config.dev_port.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
            att = s.attached,
            gen = s.last_gen.as_deref().unwrap_or("—"),
            err = s.last_error.as_deref().unwrap_or("—"),
        ));
    }
    if !any {
        lines.push(if name_filter.is_some() {
            "no matching target".into()
        } else {
            "no [[targets]] in veil.toml — add targets first".into()
        });
    }
    Ok(lines.join("\n"))
}

// ─── AGT-021 dev_logs ──────────────────────────────────────────────────────

/// Format dual-loop log ring buffer.
pub fn tool_dev_logs(
    project_root: &Path,
    name: Option<&str>,
    tail: Option<usize>,
    project_name: Option<&str>,
) -> Result<String, String> {
    let tail = tail.unwrap_or(40).clamp(1, 200);
    let pname = resolve_project_name(project_root, project_name);
    ensure_dev_loop(project_root, &pname)?;
    let loops = global_dev_loops().unwrap();
    let map = loops.lock().map_err(|e| format!("lock: {e}"))?;
    let dev = map
        .get(&pname)
        .ok_or_else(|| format!("no devloop for {pname}"))?;

    let mut out = Vec::new();
    let targets: Vec<_> = if let Some(n) = name {
        match dev.target_status(n) {
            Some(s) => vec![s],
            None => return Ok(format!("unknown target: {n}")),
        }
    } else {
        dev.status()
    };

    for s in targets {
        out.push(format!("── {} ({} lines, showing last {tail}) ──", s.name, s.logs.len()));
        if s.logs.is_empty() {
            out.push("  (no logs yet — start the target or make a write that triggers gen)".into());
            continue;
        }
        let start = s.logs.len().saturating_sub(tail);
        for line in &s.logs[start..] {
            out.push(line.clone());
        }
    }
    Ok(out.join("\n"))
}

// ─── AGT-022 / AGT-027 read_generated + list_routes ─────────────────────────

fn output_roots(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cfg) = parse_project_config(project_root) {
        for t in &cfg.targets {
            let p = project_root.join(&t.output);
            roots.push(p);
        }
    }
    let generated_dir = project_root.join("generated");
    if generated_dir.is_dir() && !roots.iter().any(|r| r == &generated_dir) {
        roots.push(generated_dir);
    }
    roots
}

fn resolve_under_outputs(project_root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim_start_matches("./").replace('\\', "/");
    if rel.is_empty() {
        return Err("path is empty".into());
    }
    if rel.contains("..") || Path::new(&rel).components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err("path must not contain '..'".into());
    }
    let candidate = project_root.join(&rel);
    let canon_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    // Allow if under any output root (even if not yet created for list)
    let roots = output_roots(project_root);
    let ok = roots.iter().any(|root| {
        candidate.starts_with(root)
            || {
                let r = root.canonicalize().unwrap_or_else(|_| root.clone());
                candidate
                    .canonicalize()
                    .map(|c| c.starts_with(&r) || c.starts_with(&canon_root.join(root.strip_prefix(project_root).unwrap_or(root))))
                    .unwrap_or(candidate.starts_with(root))
            }
    });
    // Also allow rel that starts with a configured output prefix
    let ok = ok
        || roots.iter().any(|root| {
            root.strip_prefix(project_root)
                .map(|pref| {
                    let pref = pref.to_string_lossy().replace('\\', "/");
                    rel == pref || rel.starts_with(&format!("{pref}/"))
                })
                .unwrap_or(false)
        });
    if !ok {
        let allowed: Vec<String> = roots
            .iter()
            .filter_map(|r| r.strip_prefix(project_root).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        return Err(format!(
            "path not under allowed codegen outputs. path={rel} allowed={allowed:?}"
        ));
    }
    Ok(candidate)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}…\n\n[truncated {} / {} chars]",
            &s[..max],
            max,
            s.len()
        )
    }
}

fn extract_route_lines(src: &str) -> Vec<String> {
    src.lines()
        .filter(|l| l.contains(".route("))
        .map(|l| l.trim().to_string())
        .collect()
}

/// Read generated files or list under allowlisted outputs.
pub fn tool_read_generated(
    project_root: &Path,
    path: Option<&str>,
    what: Option<&str>,
    max_chars: Option<usize>,
    list: bool,
) -> Result<String, String> {
    let max = max_chars.unwrap_or(12_000).clamp(500, 100_000);

    if let Some(w) = what {
        match w {
            "harness" => {
                let roots = output_roots(project_root);
                let mut sections = Vec::new();
                for root in &roots {
                    let main = root.join("crates/veil_bin/src/main.rs");
                    if main.is_file() {
                        let body = std::fs::read_to_string(&main)
                            .map_err(|e| format!("read {}: {e}", main.display()))?;
                        let rel = main
                            .strip_prefix(project_root)
                            .unwrap_or(&main)
                            .to_string_lossy();
                        sections.push(format!("// ==== {rel} ====\n{}", truncate(&body, max)));
                    }
                }
                if sections.is_empty() {
                    return Ok(
                        "no crates/veil_bin/src/main.rs under target outputs — run dual-loop gen first"
                            .into(),
                    );
                }
                return Ok(sections.join("\n\n"));
            }
            "routes" => {
                return tool_list_routes(project_root);
            }
            other => {
                return Err(format!(
                    "unknown what={other:?}; use harness | routes or path="
                ));
            }
        }
    }

    let path = path.ok_or_else(|| "read_generated requires path= or what=".to_string())?;
    let full = resolve_under_outputs(project_root, path)?;

    if list || full.is_dir() {
        let dir = if full.is_dir() {
            full
        } else {
            full.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(full)
        };
        let mut entries = Vec::new();
        collect_files(&dir, project_root, &mut entries, 200)?;
        entries.sort();
        if entries.is_empty() {
            return Ok(format!("(empty) {}", dir.display()));
        }
        return Ok(format!(
            "files under {}:\n{}",
            dir.strip_prefix(project_root)
                .unwrap_or(&dir)
                .display(),
            entries
                .iter()
                .map(|e| format!("  {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !full.is_file() {
        return Err(format!("not a file: {}", full.display()));
    }
    let body =
        std::fs::read_to_string(&full).map_err(|e| format!("read {}: {e}", full.display()))?;
    Ok(truncate(&body, max))
}

fn collect_files(
    dir: &Path,
    project_root: &Path,
    out: &mut Vec<String>,
    limit: usize,
) -> Result<(), String> {
    if out.len() >= limit {
        return Ok(());
    }
    let rd = std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for e in rd.flatten() {
        if out.len() >= limit {
            out.push("  … (truncated)".into());
            break;
        }
        let p = e.path();
        if p.is_dir() {
            // skip target/ and node_modules
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            collect_files(&p, project_root, out, limit)?;
        } else if p.is_file() {
            let rel = p
                .strip_prefix(project_root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

/// Structured route list from generated harness (AGT-027).
pub fn tool_list_routes(project_root: &Path) -> Result<String, String> {
    let roots = output_roots(project_root);
    let mut routes: Vec<serde_json::Value> = Vec::new();
    for root in &roots {
        let main = root.join("crates/veil_bin/src/main.rs");
        if !main.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&main)
            .map_err(|e| format!("read {}: {e}", main.display()))?;
        let rel = main
            .strip_prefix(project_root)
            .unwrap_or(&main)
            .to_string_lossy()
            .replace('\\', "/");
        for line in extract_route_lines(&body) {
            // .route("/api/foo", get(bar_handler))
            if let Some(path) = extract_quoted_path(&line) {
                let method = if line.contains("get(") {
                    "get"
                } else if line.contains("post(") {
                    "post"
                } else if line.contains("put(") {
                    "put"
                } else if line.contains("delete(") {
                    "delete"
                } else {
                    "?"
                };
                let handler = extract_handler_name(&line);
                routes.push(serde_json::json!({
                    "method": method,
                    "path": path,
                    "handler": handler,
                    "source": rel,
                }));
            }
        }
    }
    if routes.is_empty() {
        return Ok(
            "[]\n(no .route( lines found — generate backend first or check output paths)"
                .into(),
        );
    }
    Ok(serde_json::to_string_pretty(&routes).unwrap_or_else(|_| "[]".into()))
}

fn extract_quoted_path(line: &str) -> Option<String> {
    let start = line.find(".route(\"")?;
    let rest = &line[start + 8..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_handler_name(line: &str) -> Option<String> {
    // get(list_wear_tests_handler) or get(|| async
    for m in ["get(", "post(", "put(", "delete("] {
        if let Some(i) = line.find(m) {
            let rest = &line[i + m.len()..];
            if rest.starts_with('|') {
                return Some("closure".into());
            }
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

// ─── AGT-023 http_request ──────────────────────────────────────────────────

fn allowed_ports(project_root: &Path) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Ok(cfg) = parse_project_config(project_root) {
        for t in &cfg.targets {
            if let Some(p) = t.dev_port {
                ports.push(p);
            }
        }
    }
    if let Ok(extra) = std::env::var("VEIL_AGENT_HTTP_PORTS") {
        for part in extra.split(',') {
            if let Ok(p) = part.trim().parse::<u16>() {
                ports.push(p);
            }
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn resolve_http_url(
    project_root: &Path,
    target: Option<&str>,
    path: &str,
    absolute_url: Option<&str>,
) -> Result<String, String> {
    if let Some(u) = absolute_url {
        validate_local_url(u, project_root)?;
        return Ok(u.to_string());
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let port = if let Some(tname) = target {
        let cfg = parse_project_config(project_root)?;
        cfg.targets
            .iter()
            .find(|t| t.name == tname)
            .and_then(|t| t.dev_port)
            .ok_or_else(|| {
                format!("unknown target '{tname}' or no dev_port — check veil.toml / dev_status")
            })?
    } else {
        // Prefer first rust target port, else first any
        let cfg = parse_project_config(project_root)?;
        cfg.targets
            .iter()
            .find(|t| t.target == "rust")
            .or_else(|| cfg.targets.first())
            .and_then(|t| t.dev_port)
            .ok_or_else(|| "no dev_port in veil.toml — pass target= or absolute url=".to_string())?
    };
    let allowed = allowed_ports(project_root);
    if !allowed.contains(&port) {
        return Err(format!(
            "port {port} not in allowlist {allowed:?} (veil.toml dev_port or VEIL_AGENT_HTTP_PORTS)"
        ));
    }
    Ok(format!("http://127.0.0.1:{port}{path}"))
}

fn validate_local_url(url: &str, project_root: &Path) -> Result<(), String> {
    let u = url.trim();
    if !(u.starts_with("http://127.0.0.1:")
        || u.starts_with("http://localhost:")
        || u.starts_with("https://127.0.0.1:")
        || u.starts_with("https://localhost:"))
    {
        return Err(
            "http_request only allows 127.0.0.1 / localhost (SSRF protection)".into(),
        );
    }
    // Extract port
    let after_scheme = u
        .split("://")
        .nth(1)
        .ok_or_else(|| "bad url".to_string())?;
    let hostport = after_scheme.split('/').next().unwrap_or(after_scheme);
    let port: u16 = hostport
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| "could not parse port from url".to_string())?;
    let allowed = allowed_ports(project_root);
    if !allowed.is_empty() && !allowed.contains(&port) {
        return Err(format!(
            "port {port} not allowed; configured dev_ports: {allowed:?}"
        ));
    }
    Ok(())
}

/// Scoped HTTP request against local dual-loop ports.
pub async fn tool_http_request(
    project_root: &Path,
    method: Option<&str>,
    path: Option<&str>,
    target: Option<&str>,
    url: Option<&str>,
    body: Option<&str>,
    timeout_ms: Option<u64>,
) -> Result<String, String> {
    let method = method.unwrap_or("GET").to_uppercase();
    let path = path.unwrap_or("/health");
    let full_url = resolve_http_url(project_root, target, path, url)?;
    let timeout = std::time::Duration::from_millis(timeout_ms.unwrap_or(3000).clamp(200, 30_000));

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let mut req = match method.as_str() {
        "GET" => client.get(&full_url),
        "POST" => client.post(&full_url),
        "PUT" => client.put(&full_url),
        "DELETE" => client.delete(&full_url),
        "PATCH" => client.patch(&full_url),
        "HEAD" => client.head(&full_url),
        other => return Err(format!("unsupported method {other}")),
    };
    if let Some(b) = body {
        req = req
            .header("content-type", "application/json")
            .body(b.to_string());
    }

    let resp = req.send().await.map_err(|e| {
        format!(
            "request failed: {e}\nurl={full_url}\n(is the server running? try dev_status / start dual-loop backend)"
        )
    })?;

    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("read body: {e}"))?;
    let max = 12_000usize;
    let body_str = if bytes.len() > max {
        let s = String::from_utf8_lossy(&bytes[..max]);
        format!("{s}…\n[truncated {} / {} bytes]", max, bytes.len())
    } else {
        String::from_utf8_lossy(&bytes).to_string()
    };

    Ok(format!(
        "HTTP {status} {method} {full_url}\ncontent-type: {content_type}\n\n{body_str}"
    ))
}

// ─── AGT-025 dev_restart ───────────────────────────────────────────────────

/// Restart an owned dual-loop target (stop + start).
pub fn tool_dev_restart(
    project_root: &Path,
    name: Option<&str>,
    project_name: Option<&str>,
) -> Result<String, String> {
    let pname = resolve_project_name(project_root, project_name);
    ensure_dev_loop(project_root, &pname)?;
    let loops = global_dev_loops().unwrap();
    let mut map = loops.lock().map_err(|e| format!("lock: {e}"))?;
    let dev = map
        .get_mut(&pname)
        .ok_or_else(|| format!("no devloop for {pname}"))?;

    let names: Vec<String> = if let Some(n) = name {
        vec![n.to_string()]
    } else {
        // Restart all that are running or have a dev_command
        dev.status()
            .iter()
            .filter(|s| {
                s.config.dev_command.is_some()
                    && (matches!(
                        s.status,
                        devloop::TargetStatus::Running
                            | devloop::TargetStatus::Error
                            | devloop::TargetStatus::Stopped
                    ) || s.attached)
            })
            .map(|s| s.name.clone())
            .collect()
    };

    if names.is_empty() {
        return Ok("no targets to restart".into());
    }

    let mut lines = Vec::new();
    for n in names {
        match dev.start(&n) {
            Ok(()) => lines.push(format!("✓ restarted {n}")),
            Err(e) => lines.push(format!("✗ {n}: {e}")),
        }
    }
    Ok(lines.join("\n"))
}

// ─── AGT-028 smoke_status ──────────────────────────────────────────────────

/// Last-known smoke / error summary from target states.
pub fn tool_smoke_status(
    project_root: &Path,
    project_name: Option<&str>,
) -> Result<String, String> {
    let pname = resolve_project_name(project_root, project_name);
    ensure_dev_loop(project_root, &pname)?;
    let loops = global_dev_loops().unwrap();
    let map = loops.lock().map_err(|e| format!("lock: {e}"))?;
    let dev = map
        .get(&pname)
        .ok_or_else(|| format!("no devloop for {pname}"))?;

    let mut lines = vec![
        format!("project: {pname}"),
        format!(
            "VEIL_AGENT_SMOKE={}",
            if devloop::smoke_enabled() {
                "on (default)"
            } else {
                "off"
            }
        ),
    ];
    for s in dev.status() {
        let smoke_lines: Vec<_> = s
            .logs
            .iter()
            .rev()
            .filter(|l| l.contains("[check]") || l.contains("[smoke]") || l.contains("SMOKE"))
            .take(5)
            .cloned()
            .collect();
        lines.push(format!(
            "- {}: status={:?} last_error={}",
            s.name,
            s.status,
            s.last_error.as_deref().unwrap_or("—")
        ));
        if smoke_lines.is_empty() {
            lines.push("  (no recent check/smoke log lines)".into());
        } else {
            for l in smoke_lines.into_iter().rev() {
                lines.push(format!("  {l}"));
            }
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extract_route_parses_axum_line() {
        let line = r#"        .route("/api/wear_tests", get(list_wear_tests_handler))"#;
        assert_eq!(
            extract_quoted_path(line).as_deref(),
            Some("/api/wear_tests")
        );
        assert_eq!(
            extract_handler_name(line).as_deref(),
            Some("list_wear_tests_handler")
        );
    }

    #[test]
    fn path_reject_parent_dir() {
        let root = std::env::temp_dir().join("veil-agt-test-root");
        let _ = std::fs::create_dir_all(root.join("generated/backend"));
        let err = resolve_under_outputs(&root, "../etc/passwd").unwrap_err();
        assert!(err.contains(".."));
    }

    #[test]
    fn path_allow_under_generated() {
        let root = std::env::temp_dir().join(format!(
            "veil-agt-allow-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let out = root.join("generated/backend");
        std::fs::create_dir_all(&out).unwrap();
        // veil.toml with output
        let mut f = std::fs::File::create(root.join("veil.toml")).unwrap();
        writeln!(
            f,
            r#"
[[targets]]
name = "backend"
package = "app.veil"
target = "rust"
output = "generated/backend"
dev_port = 3000
"#
        )
        .unwrap();
        let p = resolve_under_outputs(&root, "generated/backend/Cargo.toml").unwrap();
        assert!(p.ends_with("Cargo.toml"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_local_url_denies_external() {
        let root = Path::new("/tmp");
        let err = validate_local_url("http://example.com/x", root).unwrap_err();
        assert!(err.contains("127.0.0.1") || err.contains("localhost"));
    }
}
