//! Session Focus — continuous "what the operator is looking at."
//!
//! Client publishes focus on each agent turn (ChatRequest.focus). Stored
//! in-process by session id so `get_current_context` and preambles can read it.
//!
//! See `docs/ADR_FOCUS_INTENT_PRESENT.md`.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};

fn latest() -> &'static RwLock<Option<Value>> {
    static LATEST: OnceLock<RwLock<Option<Value>>> = OnceLock::new();
    LATEST.get_or_init(|| RwLock::new(None))
}

fn by_session() -> &'static RwLock<HashMap<String, Value>> {
    static BY_SESSION: OnceLock<RwLock<HashMap<String, Value>>> = OnceLock::new();
    BY_SESSION.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Store focus from the UI. `session_id` optional; always updates latest.
pub fn set_focus(session_id: Option<&str>, focus: Value) {
    if let Ok(mut g) = latest().write() {
        *g = Some(focus.clone());
    }
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        if let Ok(mut map) = by_session().write() {
            map.insert(sid.to_string(), focus.clone());
            // Cap map size (simple LRU-ish: drop arbitrary when large)
            if map.len() > 256 {
                let drop_n = map.len() - 200;
                let keys: Vec<String> = map.keys().take(drop_n).cloned().collect();
                for k in keys {
                    map.remove(&k);
                }
            }
        }
        // Best-effort durable META (DDB) — don't block on failure
        let _ = crate::session::merge_session_focus_intents(sid, Some(focus), None);
    }
}

/// Resolve focus for a session, falling back to process-latest.
pub fn get_focus(session_id: Option<&str>) -> Option<Value> {
    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        if let Ok(map) = by_session().read() {
            if let Some(v) = map.get(sid) {
                return Some(v.clone());
            }
        }
    }
    latest().read().ok().and_then(|g| g.clone())
}

/// Markdown block for agent instructions / tool results.
pub fn format_focus_block(focus: &Value) -> String {
    let route = focus
        .get("route")
        .or_else(|| focus.get("page"))
        .and_then(|v| v.as_str())
        .unwrap_or("/");
    let project = focus
        .get("project")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let mut lines = vec![
        "## Session focus (authoritative — use for \"this\" / \"here\")".to_string(),
        format!("- Route: {route}"),
        format!("- Project: {}", project.unwrap_or("(none)")),
    ];
    if let Some(f) = focus.get("file").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        lines.push(format!("- Active file: {f}"));
    }
    if let Some(c) = focus
        .get("construct")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        let kind = focus
            .get("constructKind")
            .or_else(|| focus.get("construct_kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let k = if kind.is_empty() {
            String::new()
        } else {
            format!(" ({kind})")
        };
        lines.push(format!("- Construct / component in view: `{c}`{k}"));
        lines.push(
            "  When the user says \"this component\", \"this construct\", or \"this node\", they mean the above."
                .into(),
        );
    }
    if let Some(sel) = focus.get("selection") {
        let kind = sel.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let id = sel.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let label = sel
            .get("label")
            .and_then(|v| v.as_str())
            .map(|l| format!(" label={l}"))
            .unwrap_or_default();
        lines.push(format!("- Selection: {kind} id={id}{label}"));
    }
    if let Some(ch) = focus
        .get("changeId")
        .or_else(|| focus.get("pr_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        lines.push(format!("- Pull request: {ch}"));
    }
    if let Some(panel) = focus.get("panel").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
    {
        lines.push(format!("- Panel: {panel}"));
    }
    if let Some(form) = focus.get("form") {
        if let Some(id) = form.get("id").and_then(|v| v.as_str()) {
            lines.push(format!("- Form: {id}"));
        }
    }
    if let Some(d) = focus.get("diagnostics") {
        let count = d.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        if count > 0 {
            lines.push(format!("- Open diagnostics: {count}"));
        }
    }
    lines.join("\n")
}

/// JSON for `get_current_context` tool.
pub fn context_tool_json(session_id: Option<&str>) -> Value {
    let intents = recent_intents(12);
    let acks = recent_acks(8);
    // Prefer live focus; fall back to durable session META
    let mut focus = get_focus(session_id);
    if focus.is_none() {
        if let Some(sid) = session_id {
            if let Ok(meta) = crate::session::get_session_meta(sid) {
                focus = meta.last_focus;
                // Merge durable intent log if process log empty
            }
        }
    }
    match focus {
        Some(focus) => json!({
            "ok": true,
            "summary": "Session focus from the runtime UI",
            "focus": focus,
            "focus_markdown": format_focus_block(&focus),
            "recent_intents": intents,
            "recent_acks": acks,
            "hint": "Prefer focus.construct / focus.project for deictic references. After pending_ux create, check recent_acks before write_source. Prefer platform tools over curl."
        }),
        None => json!({
            "ok": true,
            "summary": "No focus published yet (UI has not sent ChatRequest.focus)",
            "focus": null,
            "recent_intents": intents,
            "recent_acks": acks,
            "hint": "Context is injected each browser turn. create_project / list_projects / navigate_to / open_ide control the UX."
        }),
    }
}

/// Browser attached this turn (ChatRequest.focus present). Used for via=ux defaults
/// on pure host short-circuit product tools — not for mid-ACP multi-step.
tokio::task_local! {
    pub static CLIENT_PRESENT: bool;
}

pub fn client_present() -> bool {
    CLIENT_PRESENT.try_with(|c| *c).unwrap_or(false)
}

/// Domain mode for product create tools.
/// - `ux`: Present then UX POSTs `/api/ux/*` (true Agent→UX→Server)
/// - `server`: domain already applied; Present is illustrative
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainMode {
    Ux,
    Server,
}

/// Build present choreography for create_project.
///
/// `domain_mode=Server` → domain already done; Present illustrates form then navigates.
/// `domain_mode=Ux` → Present fills form, pulses, **commit** via `/api/ux/create_project`, then navigates.
pub fn create_project_intent(
    name: &str,
    description: Option<&str>,
    final_path: &str,
    open_ide: bool,
    domain_mode: DomainMode,
) -> Value {
    let mut fields = json!({ "name": name });
    if let Some(d) = description.filter(|s| !s.is_empty()) {
        fields["description"] = json!(d);
    }
    let action = if open_ide { "open-ide" } else { "goto" };
    let mut steps = vec![
        json!({ "kind": "goto", "path": "/projects/new", "ms": 320 }),
        json!({
            "kind": "fill",
            "formId": "create-project",
            "fields": fields,
            "mode": "type"
        }),
        json!({ "kind": "wait", "ms": 180 }),
        json!({ "kind": "pulse", "target": "submit", "ms": 600, "activate": true }),
        json!({ "kind": "wait", "ms": 280 }),
    ];
    match domain_mode {
        DomainMode::Ux => {
            // Click the real Create button — ProjectCreateView POSTs /api/ux/create_project.
            steps.push(json!({ "kind": "wait", "ms": 200 }));
        }
        DomainMode::Server => {
            steps.push(json!({
                "kind": "goto",
                "path": final_path,
                "ms": 320,
                "project": name
            }));
        }
    }
    let (domain, present_kind) = match domain_mode {
        DomainMode::Ux => (json!({ "mode": "ux", "done": false }), "ux_click"),
        DomainMode::Server => (json!({ "mode": "server", "done": true }), "illustrate"),
    };
    json!({
        "type": "CreateProject",
        "id": format!("intent_create_project_{}", short_id()),
        "actor": "agent",
        "payload": {
            "name": name,
            "description": description,
            "open_ide": open_ide,
        },
        "domain": domain,
        "navigation": {
            "action": action,
            "path": final_path,
            "project": name,
        },
        "present": {
            "announce": format!("Creating project {name}"),
            "steps": steps
        },
        "present_kind": present_kind
    })
}

/// Present for create_pr (form + optional UX commit).
pub fn create_pr_intent(
    title: Option<&str>,
    description: Option<&str>,
    project: Option<&str>,
    final_path: &str,
    domain_mode: DomainMode,
) -> Value {
    let mut fields = json!({});
    if let Some(t) = title.filter(|s| !s.is_empty()) {
        fields["title"] = json!(t);
    }
    if let Some(d) = description.filter(|s| !s.is_empty()) {
        fields["description"] = json!(d);
    }
    if let Some(p) = project.filter(|s| !s.is_empty()) {
        fields["project"] = json!(p);
        fields["slug"] = json!(p);
    }
    let mut steps = vec![
        json!({ "kind": "goto", "path": "/pulls/new", "ms": 320 }),
    ];
    if !fields.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        steps.push(json!({
            "kind": "fill",
            "formId": "create-change",
            "fields": fields,
            "mode": "type"
        }));
        steps.push(json!({ "kind": "wait", "ms": 160 }));
        steps.push(json!({ "kind": "pulse", "target": "submit", "ms": 550, "activate": true }));
        steps.push(json!({ "kind": "wait", "ms": 180 }));
    }
    match domain_mode {
        DomainMode::Ux if title.is_some() => {
            // Click Create — the form POSTs /api/pull_requests (same as a human).
        }
        DomainMode::Server => {
            steps.push(json!({
                "kind": "goto",
                "path": final_path,
                "ms": 300
            }));
        }
        DomainMode::Ux => {
            // Open form only
        }
    }
    let domain = match domain_mode {
        DomainMode::Ux => json!({ "mode": "ux", "done": false }),
        DomainMode::Server => json!({ "mode": "server", "done": true }),
    };
    json!({
        "type": "CreateChange",
        "id": format!("intent_create_pr_{}", short_id()),
        "actor": "agent",
        "payload": {
            "title": title,
            "description": description,
            "project": project,
        },
        "domain": domain,
        "navigation": { "action": "goto", "path": final_path },
        "present": {
            "announce": title.map(|t| format!("Creating change: {t}"))
                .unwrap_or_else(|| "Open create change form".into()),
            "steps": steps
        }
    })
}

/// Simple navigate intent with present goto.
pub fn navigate_intent(path: &str, project: Option<&str>) -> Value {
    json!({
        "type": "Navigate",
        "id": format!("intent_nav_{}", short_id()),
        "actor": "agent",
        "payload": { "path": path, "project": project },
        "domain": { "mode": "none" },
        "navigation": {
            "action": "goto",
            "path": path,
            "project": project,
        },
        "present": {
            "steps": [{
                "kind": "goto",
                "path": path,
                "ms": 280,
                "project": project
            }]
        }
    })
}

/// Ring buffer of recent intents (agent + human) for preamble awareness.
fn intent_log() -> &'static RwLock<Vec<Value>> {
    static LOG: OnceLock<RwLock<Vec<Value>>> = OnceLock::new();
    LOG.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn push_intent_log(entry: Value) {
    if let Ok(mut g) = intent_log().write() {
        g.push(entry.clone());
        if g.len() > 40 {
            let drain = g.len() - 30;
            g.drain(0..drain);
        }
    }
    // Persist on coding session when in scope
    if let Ok(sid) = crate::session::CURRENT_SESSION.try_with(|s| s.clone()) {
        let _ = crate::session::merge_session_focus_intents(&sid, None, Some(entry));
    }
}

// ─── UX Intent ACK (Present finished / domain committed) ─────────────────────
//
// Supports non-blocking ACK + optional blocking wait (`wait_intent_ack` tool).
// Wait uses oneshot channels so MCP/ACP can pause until Present completes
// without deadlocking: tool1 returns intent immediately (streamed to FE),
// tool2 wait_intent_ack blocks until FE POSTs /api/ux/intent_ack.

use std::sync::Mutex;
use tokio::sync::oneshot;

struct AckHub {
    pending_meta: HashMap<String, Value>,
    completed: HashMap<String, Value>,
    waiters: HashMap<String, Vec<oneshot::Sender<Value>>>,
}

fn ack_hub() -> &'static Mutex<AckHub> {
    static H: OnceLock<Mutex<AckHub>> = OnceLock::new();
    H.get_or_init(|| {
        Mutex::new(AckHub {
            pending_meta: HashMap::new(),
            completed: HashMap::new(),
            waiters: HashMap::new(),
        })
    })
}

/// Mark intent as awaiting browser Present (optional wait).
pub fn register_pending_intent(intent_id: &str, meta: Value) {
    if intent_id.is_empty() {
        return;
    }
    if let Ok(mut g) = ack_hub().lock() {
        g.pending_meta.insert(intent_id.to_string(), meta);
        // Cap
        if g.pending_meta.len() > 128 {
            let keys: Vec<String> = g.pending_meta.keys().take(g.pending_meta.len() - 96).cloned().collect();
            for k in keys {
                g.pending_meta.remove(&k);
            }
        }
    }
}

/// Browser reports Present complete (and UX commit if any).
pub fn ack_intent(intent_id: &str, result: Value) {
    if intent_id.is_empty() {
        return;
    }
    let mut entry = result;
    if entry.get("intent_id").is_none() {
        entry["intent_id"] = json!(intent_id);
    }
    entry["acked_at"] = json!(short_id());

    let waiters = if let Ok(mut g) = ack_hub().lock() {
        g.pending_meta.remove(intent_id);
        g.completed.insert(intent_id.to_string(), entry.clone());
        if g.completed.len() > 128 {
            let keys: Vec<String> = g.completed.keys().take(g.completed.len() - 96).cloned().collect();
            for k in keys {
                g.completed.remove(&k);
            }
        }
        g.waiters.remove(intent_id).unwrap_or_default()
    } else {
        Vec::new()
    };

    for tx in waiters {
        let _ = tx.send(entry.clone());
    }

    push_intent_log(json!({
        "type": "IntentAck",
        "actor": "ux",
        "summary": intent_id,
        "payload": entry,
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    }));
}

pub fn get_intent_ack(intent_id: &str) -> Option<Value> {
    ack_hub()
        .lock()
        .ok()
        .and_then(|g| g.completed.get(intent_id).cloned())
}

pub fn recent_acks(limit: usize) -> Vec<Value> {
    ack_hub()
        .lock()
        .ok()
        .map(|g| {
            let mut v: Vec<Value> = g.completed.values().cloned().collect();
            v.truncate(limit);
            v
        })
        .unwrap_or_default()
}

/// Block until FE ACKs this intent (or timeout). Safe after intent was streamed to UI.
pub async fn wait_intent_ack(intent_id: &str, timeout_ms: u64) -> Result<Value, String> {
    if intent_id.is_empty() {
        return Err("wait_intent_ack requires intent_id".into());
    }
    // Already done?
    if let Some(v) = get_intent_ack(intent_id) {
        return Ok(v);
    }
    let (tx, rx) = oneshot::channel();
    {
        let mut g = ack_hub()
            .lock()
            .map_err(|e| format!("ack hub lock: {e}"))?;
        // Re-check under lock
        if let Some(v) = g.completed.get(intent_id) {
            return Ok(v.clone());
        }
        g.waiters
            .entry(intent_id.to_string())
            .or_default()
            .push(tx);
    }
    let timeout = std::time::Duration::from_millis(timeout_ms.max(500));
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err(format!(
            "wait_intent_ack: channel closed for `{intent_id}` (no ACK)"
        )),
        Err(_) => Err(format!(
            "wait_intent_ack: timed out after {timeout_ms}ms waiting for Present ACK `{intent_id}`. \
             Ensure the browser is open and running Present; or use via=server for headless multi-step."
        )),
    }
}

/// Present for change lifecycle actions (submit / approve / merge / …).
pub fn change_action_intent(action: &str, change_id: &str, summary: &str) -> Value {
    let path = format!("/pulls/{change_id}");
    // Prefer stable data-veil-action; fall back to button text labels.
    let (action_attr, btn_text) = match action {
        "submit" => ("submit-pr", "Submit for Review"),
        "approve" => ("approve-pr", "Approve"),
        "merge" => ("merge-pr", "Merge"),
        "request_pr_changes" => ("request-pr-changes", "Request Changes"),
        "comment" => ("add-comment", "Post Comment"),
        _ => ("primary", "btn-primary"),
    };
    json!({
        "type": "ChangeAction",
        "id": format!("intent_change_{action}_{}", short_id()),
        "actor": "agent",
        "payload": { "action": action, "id": change_id },
        "domain": { "mode": "server", "done": true },
        "navigation": { "action": "goto", "path": path },
        "present": {
            "announce": summary,
            "steps": [
                { "kind": "goto", "path": path, "ms": 320 },
                { "kind": "wait", "ms": 220 },
                {
                    "kind": "pulse",
                    "selector": format!("[data-veil-action='{action_attr}']"),
                    "target": format!("text:{btn_text}"),
                    "ms": 550
                },
                { "kind": "announce", "message": summary }
            ]
        }
    })
}

/// Present for deploy / provision ops.
pub fn deploy_action_intent(action: &str, summary: &str, project: Option<&str>) -> Value {
    json!({
        "type": "DeployAction",
        "id": format!("intent_deploy_{action}_{}", short_id()),
        "actor": "agent",
        "payload": { "action": action, "project": project },
        "domain": { "mode": "server", "done": true },
        "navigation": { "action": "goto", "path": "/deploy" },
        "present": {
            "announce": summary,
            "steps": [
                { "kind": "goto", "path": "/deploy", "ms": 300, "project": project },
                { "kind": "wait", "ms": 220 },
                {
                    "kind": "pulse",
                    "selector": "[data-veil-action='provision-project'], [data-veil-action='provision-confirm']",
                    "target": "text:Provision",
                    "ms": 500
                },
                { "kind": "announce", "message": summary }
            ]
        }
    })
}

/// Present for config / dashboard product pages.
pub fn page_action_intent(action: &str, path: &str, summary: &str) -> Value {
    json!({
        "type": "PageAction",
        "id": format!("intent_page_{action}_{}", short_id()),
        "actor": "agent",
        "payload": { "action": action, "path": path },
        "domain": { "mode": "none" },
        "navigation": { "action": "goto", "path": path },
        "present": {
            "announce": summary,
            "steps": [
                { "kind": "goto", "path": path, "ms": 280 },
                { "kind": "wait", "ms": 160 },
                { "kind": "pulse", "selector": "main, .dk-page-shell, [data-veil-role]", "ms": 400 },
                { "kind": "announce", "message": summary }
            ]
        }
    })
}

pub fn recent_intents(limit: usize) -> Vec<Value> {
    intent_log()
        .read()
        .ok()
        .map(|g| g.iter().rev().take(limit).cloned().collect())
        .unwrap_or_default()
}

pub fn format_intent_log_block(limit: usize) -> String {
    let items = recent_intents(limit);
    if items.is_empty() {
        return String::new();
    }
    let mut lines = vec!["## Recent intents (agent + human)".to_string()];
    for it in items {
        let actor = it.get("actor").and_then(|v| v.as_str()).unwrap_or("?");
        let ty = it.get("type").and_then(|v| v.as_str()).unwrap_or("?");
        let summary = it
            .get("summary")
            .and_then(|v| v.as_str())
            .or_else(|| it.pointer("/payload/name").and_then(|v| v.as_str()))
            .or_else(|| it.pointer("/payload/title").and_then(|v| v.as_str()))
            .unwrap_or("");
        lines.push(format!("- [{actor}] {ty} {summary}"));
    }
    lines.join("\n")
}

fn short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{t:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_includes_construct() {
        let f = json!({
            "route": "/projects/relay/ide",
            "project": "relay",
            "construct": "RelayAuth",
            "constructKind": "aggregate"
        });
        let s = format_focus_block(&f);
        assert!(s.contains("RelayAuth"));
        assert!(s.contains("this component"));
    }

    #[test]
    fn create_intent_server_illustrates() {
        let i = create_project_intent(
            "demo",
            Some("hi"),
            "/projects/demo/ide",
            true,
            DomainMode::Server,
        );
        assert_eq!(i["type"], "CreateProject");
        assert_eq!(i["domain"]["mode"], "server");
        let steps = i["present"]["steps"].as_array().unwrap();
        assert!(steps.len() >= 4);
        assert_eq!(steps[0]["kind"], "goto");
        assert_eq!(steps[1]["kind"], "fill");
        assert!(steps.iter().any(|s| s["kind"] == "goto" && s.get("path").is_some()));
        assert!(!steps.iter().any(|s| s["kind"] == "commit"));
    }

    #[test]
    fn create_intent_ux_clicks_submit() {
        let i = create_project_intent(
            "demo",
            None,
            "/projects/demo/ide",
            true,
            DomainMode::Ux,
        );
        assert_eq!(i["domain"]["mode"], "ux");
        let steps = i["present"]["steps"].as_array().unwrap();
        assert!(!steps.iter().any(|s| s["kind"] == "commit"));
        let pulse = steps.iter().find(|s| s["kind"] == "pulse").unwrap();
        assert_eq!(pulse["activate"], true);
    }

    #[test]
    fn change_and_deploy_intents() {
        let c = change_action_intent("approve", "cr-1", "Approved change cr-1");
        assert_eq!(c["type"], "ChangeAction");
        assert!(c["present"]["steps"].as_array().unwrap().len() >= 2);
        let d = deploy_action_intent("provision", "Provision foo", Some("foo"));
        assert_eq!(d["type"], "DeployAction");
        assert_eq!(d["navigation"]["path"], "/deploy");
    }

    #[test]
    fn ack_roundtrip() {
        register_pending_intent("i1", json!({ "tool": "t" }));
        ack_intent("i1", json!({ "ok": true, "slug": "x" }));
        let a = get_intent_ack("i1").unwrap();
        assert_eq!(a["ok"], true);
        assert_eq!(a["intent_id"], "i1");
    }

    #[tokio::test]
    async fn wait_ack_receives_from_other_task() {
        let id = format!("wait_test_{}", short_id());
        register_pending_intent(&id, json!({ "tool": "t" }));
        let id2 = id.clone();
        let joiner = tokio::spawn(async move { wait_intent_ack(&id2, 5_000).await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        ack_intent(&id, json!({ "ok": true, "slug": "demo" }));
        let got = joiner.await.unwrap().unwrap();
        assert_eq!(got["ok"], true);
        assert_eq!(got["slug"], "demo");
    }

    #[tokio::test]
    async fn wait_ack_timeout() {
        let id = format!("wait_timeout_{}", short_id());
        let err = wait_intent_ack(&id, 80).await.unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }
}
