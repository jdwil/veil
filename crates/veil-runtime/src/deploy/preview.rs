//! Static preview backend for "vibe-code from the frontend".
//!
//! On "Open UI" (and on each accepted change), the runtime:
//!   1. materializes the project source (ui.veil + layers/ + stubs/) to a
//!      per-slug preview dir,
//!   2. runs `veil gen -t typescript` with DEVELOPER MODE ON (so the emitted
//!      DOM carries provenance attributes — see crates/veil-codegen provenance
//!      stamping + layers/developer.layer),
//!   3. `npm install` + `npm run build` → a browsable static bundle,
//!   4. serves the bundle under `/preview/{slug}/*`, injecting the overlay
//!      client script into served HTML so text-selection / right-click raise a
//!      `veil:edit-intent` the Open-UI window listens for.
//!
//! This is the STATIC (default, universal, phone-friendly, no-zombie) backend
//! from the design (palace: veil-vibe-code-from-frontend-design). The opt-in
//! local-dev-server (true HMR) path is a capable-client enhancement layered on
//! top of the same overlay loop and is not built here.
//!
//! Developer mode is scoped to the `veil gen` subprocess env for preview builds
//! ONLY — a normal deploy build never sets it, so provenance/overlay never ship
//! to production.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tokio::sync::Mutex;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Root dir for preview working trees + built bundles (per slug).
pub fn preview_root(slug: &str) -> PathBuf {
    std::env::temp_dir().join("veil-preview").join(slug)
}

/// Where the project source is materialized for the preview build.
pub fn preview_source_dir(slug: &str) -> PathBuf {
    preview_root(slug).join("source")
}

/// Where `veil gen` writes + the SPA build runs (node_modules cached here).
pub fn preview_gen_dir(slug: &str) -> PathBuf {
    preview_root(slug).join("generated")
}

/// Status of a project's preview build, surfaced to the Open-UI window so it
/// can render a graceful "starting preview…" state during build latency.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PreviewStatus {
    /// No build has been started for this slug.
    Idle,
    /// A build is in progress.
    Building,
    /// Bundle built and being served. `dir` is the static root.
    Ready { dir: String },
    /// The last build failed. `error` is the failure detail.
    Failed { error: String },
}

/// Per-slug preview status registry + a per-slug build lock so concurrent
/// "Open UI" / change events don't run overlapping builds for one project.
struct PreviewRegistry {
    status: Mutex<HashMap<String, PreviewStatus>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

static REGISTRY: OnceLock<PreviewRegistry> = OnceLock::new();

fn registry() -> &'static PreviewRegistry {
    REGISTRY.get_or_init(|| PreviewRegistry {
        status: Mutex::new(HashMap::new()),
        locks: Mutex::new(HashMap::new()),
    })
}

/// Current preview status for a slug (Idle if never built).
pub async fn status_for(slug: &str) -> PreviewStatus {
    registry()
        .status
        .lock()
        .await
        .get(slug)
        .cloned()
        .unwrap_or(PreviewStatus::Idle)
}

async fn set_status(slug: &str, status: PreviewStatus) {
    registry().status.lock().await.insert(slug.to_string(), status);
}

async fn build_lock(slug: &str) -> Arc<Mutex<()>> {
    let mut locks = registry().locks.lock().await;
    locks
        .entry(slug.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// The static root served under `/preview/{slug}/*` after a successful build.
/// Framework-agnostic: any project whose build emits a browsable static site
/// with an `index.html` works. We look in the conventional output dirs
/// (`build/`, `dist/`) — the project owns its own build (`npm run build`).
fn resolve_build_output(gen_dir: &Path) -> Option<PathBuf> {
    for cand in ["build", "dist"] {
        let p = gen_dir.join(cand);
        if p.join("index.html").exists() {
            return Some(p);
        }
    }
    None
}

/// Build (or rebuild) the static preview for a project.
///
/// `materialize` writes the project source into `preview_source_dir(slug)` and
/// returns the entry `.veil` file name (e.g. "ui.veil"). Kept as a callback so
/// this module stays free of the storage-layer dependency (the caller in
/// platform_http owns storage access).
///
/// Returns the served static root on success. Sets PreviewStatus throughout so
/// the window can poll `status_for`.
pub async fn build_preview<F, Fut>(slug: &str, materialize: F) -> Result<PathBuf, String>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    // Serialize builds per slug; a concurrent request awaits then reuses result.
    let lock = build_lock(slug).await;
    let _guard = lock.lock().await;

    set_status(slug, PreviewStatus::Building).await;

    let result = build_preview_inner(slug, materialize).await;
    match &result {
        Ok(dir) => {
            set_status(
                slug,
                PreviewStatus::Ready {
                    dir: dir.to_string_lossy().to_string(),
                },
            )
            .await;
        }
        Err(e) => {
            set_status(slug, PreviewStatus::Failed { error: e.clone() }).await;
        }
    }
    result
}

async fn build_preview_inner<F, Fut>(slug: &str, materialize: F) -> Result<PathBuf, String>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let source_dir = preview_source_dir(slug);
    let gen_dir = preview_gen_dir(slug);
    tokio::fs::create_dir_all(&source_dir)
        .await
        .map_err(|e| format!("mkdir preview source: {e}"))?;
    tokio::fs::create_dir_all(&gen_dir)
        .await
        .map_err(|e| format!("mkdir preview gen: {e}"))?;

    // Caller materializes the project source and tells us the entry .veil.
    let veil_file = materialize(source_dir.clone()).await?;
    let veil_path = source_dir.join(&veil_file);
    if !veil_path.exists() {
        return Err(format!(
            "preview entry '{}' not found in materialized source",
            veil_file
        ));
    }

    // Step 1: veil gen -t typescript WITH DEVELOPER MODE ON (env scoped to this
    // subprocess only). Provenance stamping + developer-layer injection are
    // driven entirely by these env vars — see codegen + layers/developer.layer.
    let gen_out = Command::new("veil")
        .args([
            "gen",
            &veil_path.to_string_lossy(),
            "-t",
            "typescript",
            "-o",
            &gen_dir.to_string_lossy(),
        ])
        .env("VEIL_DEVELOPER_MODE", "1")
        .env("VEIL_PROJECT_SLUG", slug)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("veil gen failed to start: {e}"))?;
    if !gen_out.status.success() {
        return Err(format!(
            "veil gen (developer preview) failed: {}",
            String::from_utf8_lossy(&gen_out.stderr)
        ));
    }

    // Step 2: npm install (node_modules cached in gen_dir across rebuilds).
    let install = Command::new("npm")
        .args(["install", "--prefer-offline", "--no-audit"])
        .current_dir(&gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("npm install failed to start: {e}"))?;
    if !install.status.success() {
        return Err(format!(
            "npm install failed: {}",
            String::from_utf8_lossy(&install.stderr)
        ));
    }

    // Step 3: npm run build → static bundle (build/ or dist/).
    let build = Command::new("npm")
        .args(["run", "build"])
        .current_dir(&gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("npm run build failed to start: {e}"))?;
    if !build.status.success() {
        return Err(format!(
            "npm run build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }

    resolve_build_output(&gen_dir)
        .ok_or_else(|| "no build/ or dist/ with index.html after preview build".to_string())
}

/// The overlay client script injected into served preview HTML.
///
/// Generic — reads the nearest `data-veil-*` provenance (stamped by the
/// developer layer, Part 1) on text-selection or right-click and raises a
/// `veil:edit-intent` CustomEvent AND (when inside an iframe) postMessages the
/// same payload to the parent Open-UI window. No project-specific knowledge.
pub const OVERLAY_SCRIPT: &str = r#"<script data-veil-overlay="1">
(function () {
  if (window.__veilOverlayInstalled) return;
  window.__veilOverlayInstalled = true;

  function nearestProvenance(el) {
    while (el && el.nodeType === 1) {
      if (el.hasAttribute && el.hasAttribute('data-veil-construct')) {
        return {
          project: el.getAttribute('data-veil-project') || null,
          construct: el.getAttribute('data-veil-construct'),
          el: el.getAttribute('data-veil-el'),
          label: (el.textContent || '').trim().slice(0, 80),
        };
      }
      el = el.parentElement;
    }
    return null;
  }

  function raise(ref, selection) {
    if (!ref) return;
    var payload = Object.assign({ selection: selection || null }, ref);
    try {
      window.dispatchEvent(new CustomEvent('veil:edit-intent', { detail: payload }));
    } catch (e) {}
    // Bubble to the Open-UI window when previewed inside an iframe.
    if (window.parent && window.parent !== window) {
      try {
        window.parent.postMessage({ type: 'veil:edit-intent', payload: payload }, '*');
      } catch (e) {}
    }
  }

  // Right-click an element → edit intent for that element's construct.
  document.addEventListener('contextmenu', function (ev) {
    var ref = nearestProvenance(ev.target);
    if (ref) {
      ev.preventDefault();
      raise(ref, null);
    }
  }, true);

  // Select text → edit intent carrying the selection + its construct.
  document.addEventListener('mouseup', function () {
    var sel = window.getSelection ? window.getSelection() : null;
    var text = sel && sel.toString ? sel.toString().trim() : '';
    if (!text) return;
    var node = sel.anchorNode;
    var el = node && node.nodeType === 1 ? node : (node && node.parentElement);
    var ref = nearestProvenance(el);
    if (ref) raise(ref, text);
  }, true);
})();
</script>"#;

/// Inject the overlay script into an HTML document just before `</body>`
/// (fallback: append). Idempotent — skips if already present.
pub fn inject_overlay(html: &str) -> String {
    if html.contains("data-veil-overlay=\"1\"") {
        return html.to_string();
    }
    if let Some(idx) = html.rfind("</body>") {
        let mut out = String::with_capacity(html.len() + OVERLAY_SCRIPT.len());
        out.push_str(&html[..idx]);
        out.push_str(OVERLAY_SCRIPT);
        out.push_str(&html[idx..]);
        out
    } else {
        format!("{html}{OVERLAY_SCRIPT}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_injected_before_body_close() {
        let html = "<html><body><h1>hi</h1></body></html>";
        let out = inject_overlay(html);
        assert!(out.contains("data-veil-overlay=\"1\""), "script not injected");
        let script_at = out.find("data-veil-overlay").unwrap();
        let body_close = out.find("</body>").unwrap();
        assert!(script_at < body_close, "script must precede </body>");
    }

    #[test]
    fn overlay_injection_is_idempotent() {
        let html = "<html><body></body></html>";
        let once = inject_overlay(html);
        let twice = inject_overlay(&once);
        assert_eq!(once, twice, "double injection must be a no-op");
    }

    #[test]
    fn overlay_appends_when_no_body_tag() {
        let html = "<h1>fragment</h1>";
        let out = inject_overlay(html);
        assert!(out.starts_with("<h1>fragment</h1>"));
        assert!(out.contains("data-veil-overlay=\"1\""));
    }

    #[test]
    fn resolve_output_prefers_build_then_dist() {
        let tmp = std::env::temp_dir().join(format!("veil-prev-test-{}", std::process::id()));
        let build = tmp.join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("index.html"), "<html></html>").unwrap();
        assert_eq!(resolve_build_output(&tmp), Some(build));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
