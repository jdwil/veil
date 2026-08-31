//! Parse the `veil.toml [[triggers]]` block into [`TriggerDeclaration`]s.
//!
//! Mirrors how `[deploy.contribution]` declares UI slots: an execution artifact
//! declares its triggers in-tree, and they flow into registration. Example:
//!
//! ```toml
//! [[triggers]]
//! kind = "schedule"
//! schedule_expr = "rate(5 minutes)"
//! timezone = "UTC"
//! payload_template = { job = "nightly-sync" }
//!
//! [[triggers]]
//! kind = "event"
//! event_type = "order.created"
//! filter = { region = "us" }
//!
//! [[triggers]]
//! id = "manual-run"
//! kind = "on_demand"
//! ```
//!
//! The parser is tolerant: unknown keys are ignored and a `[[triggers]]` entry
//! with an unrecognized `kind` is skipped (logged by the caller).

use super::{TriggerDeclaration, TriggerKind};

/// Parse a `serde_json::Value` (the deserialized veil.toml) for a top-level
/// `triggers` array, returning the declarations. Missing/empty ⇒ empty vec.
///
/// veil.toml is loaded elsewhere as `toml` → this accepts the already-parsed
/// value so it composes with existing `[deploy]` parsing (which also takes a
/// `serde_json::Value`).
pub fn parse_triggers(veil_toml: &serde_json::Value) -> Vec<TriggerDeclaration> {
    let arr = match veil_toml.get("triggers").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter().filter_map(parse_one).collect()
}

fn parse_one(v: &serde_json::Value) -> Option<TriggerDeclaration> {
    let kind = match v.get("kind").and_then(|k| k.as_str()) {
        Some("on_demand") | Some("ondemand") => TriggerKind::OnDemand,
        Some("schedule") => TriggerKind::Schedule,
        Some("event") => TriggerKind::Event,
        _ => return None, // unrecognized/missing kind → skip
    };
    Some(TriggerDeclaration {
        id: v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string()),
        kind,
        schedule_expr: v
            .get("schedule_expr")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        timezone: v
            .get("timezone")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        event_type: v
            .get("event_type")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        filter: v.get("filter").cloned(),
        payload_template: v.get("payload_template").cloned(),
        enabled: v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(true),
    })
}

/// Load + parse triggers directly from a `veil.toml` file on disk. Returns an
/// empty vec if the file is missing or has no `[[triggers]]`. A malformed TOML
/// file is an error the caller should surface.
pub fn parse_triggers_from_file(
    path: &std::path::Path,
) -> Result<Vec<TriggerDeclaration>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let toml_value: toml::Value =
        toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    // Convert to serde_json::Value so parse_triggers can share the JSON accessor
    // path used by the rest of veil.toml parsing.
    let json_value = serde_json::to_value(&toml_value)
        .map_err(|e| format!("convert toml→json {}: {e}", path.display()))?;
    Ok(parse_triggers(&json_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_triggers() {
        let toml_src = r#"
[[triggers]]
kind = "schedule"
schedule_expr = "rate(5 minutes)"
timezone = "UTC"
payload_template = { job = "nightly-sync" }

[[triggers]]
kind = "event"
event_type = "order.created"
filter = { region = "us" }

[[triggers]]
id = "manual-run"
kind = "on_demand"
"#;
        let toml_value: toml::Value = toml::from_str(toml_src).unwrap();
        let json_value = serde_json::to_value(&toml_value).unwrap();
        let decls = parse_triggers(&json_value);
        assert_eq!(decls.len(), 3);

        assert_eq!(decls[0].kind, TriggerKind::Schedule);
        assert_eq!(decls[0].schedule_expr.as_deref(), Some("rate(5 minutes)"));
        assert_eq!(
            decls[0].payload_template,
            Some(serde_json::json!({ "job": "nightly-sync" }))
        );

        assert_eq!(decls[1].kind, TriggerKind::Event);
        assert_eq!(decls[1].event_type.as_deref(), Some("order.created"));
        assert_eq!(decls[1].filter, Some(serde_json::json!({ "region": "us" })));

        assert_eq!(decls[2].kind, TriggerKind::OnDemand);
        assert_eq!(decls[2].id.as_deref(), Some("manual-run"));
    }

    #[test]
    fn no_triggers_block_is_empty() {
        let json_value = serde_json::json!({ "deploy": { "type": "ecs" } });
        assert!(parse_triggers(&json_value).is_empty());
    }

    #[test]
    fn unknown_kind_is_skipped() {
        let toml_src = r#"
[[triggers]]
kind = "webhook"

[[triggers]]
kind = "on_demand"
"#;
        let toml_value: toml::Value = toml::from_str(toml_src).unwrap();
        let json_value = serde_json::to_value(&toml_value).unwrap();
        let decls = parse_triggers(&json_value);
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].kind, TriggerKind::OnDemand);
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let p = std::path::Path::new("/nonexistent/veil.toml");
        assert!(parse_triggers_from_file(p).unwrap().is_empty());
    }
}
