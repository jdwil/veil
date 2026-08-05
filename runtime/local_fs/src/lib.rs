//! Filesystem helpers for VEIL adapters. Called from generated code via the
//! `veil_local_fs` stub — **not** inlined into the VEIL engine (MISSION).

use std::path::{Path, PathBuf};

/// Error type that converts via `?` into generated `DomainError::External`
/// (Display → External string) when adapters use `Res!` methods.
#[derive(Debug)]
pub struct FsError(pub String);

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FsError {}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> Self {
        FsError(e.to_string())
    }
}

/// Static helpers (associated fns) matching `runtime/src/stubs/veil_local_fs.stub`.
pub struct LocalFs;

impl LocalFs {
    pub fn create_dir_all(path: impl AsRef<str>) -> Result<(), FsError> {
        std::fs::create_dir_all(path.as_ref())?;
        Ok(())
    }

    pub fn write(path: impl AsRef<str>, data: impl AsRef<str>) -> Result<(), FsError> {
        let p = Path::new(path.as_ref());
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, data.as_ref().as_bytes())?;
        Ok(())
    }

    pub fn read(path: impl AsRef<str>) -> Result<String, FsError> {
        Ok(std::fs::read_to_string(path.as_ref())?)
    }

    pub fn path_exists(path: impl AsRef<str>) -> bool {
        Path::new(path.as_ref()).exists()
    }

    pub fn path_is_file(path: impl AsRef<str>) -> bool {
        Path::new(path.as_ref()).is_file()
    }

    pub fn list_dir(path: impl AsRef<str>) -> Result<Vec<String>, FsError> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(path.as_ref())? {
            let e = e?;
            out.push(e.file_name().to_string_lossy().to_string());
        }
        out.sort();
        Ok(out)
    }

    /// List only regular files ending in `.json` (extension record files).
    pub fn list_json_files(path: impl AsRef<str>) -> Result<Vec<String>, FsError> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(path.as_ref())? {
            let e = e?;
            let p = e.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".json") {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn join(a: impl AsRef<str>, b: impl AsRef<str>) -> String {
        let mut p = PathBuf::from(a.as_ref());
        p.push(b.as_ref());
        p.to_string_lossy().to_string()
    }

    /// Clone-friendly wrappers used when generated code moves Strings into calls.
    pub fn join_owned(a: String, b: String) -> String {
        Self::join(a, b)
    }

    /// Get the projects directory from env or config.
    pub fn projects_dir() -> String {
        if let Ok(dir) = std::env::var("VEIL_PROJECTS_DIR") {
            return dir;
        }
        // Try ~/.veil/config.json
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let cfg_path = format!("{home}/.veil/config.json");
        if let Ok(contents) = std::fs::read_to_string(&cfg_path) {
            // Minimal JSON parse for projects_dir field
            if let Some(start) = contents.find("\"projects_dir\"") {
                let rest = &contents[start..];
                if let Some(colon) = rest.find(':') {
                    let after = rest[colon + 1..].trim();
                    if after.starts_with('"') {
                        if let Some(end) = after[1..].find('"') {
                            return after[1..1 + end].to_string();
                        }
                    }
                }
            }
        }
        format!("{home}/veil-projects")
    }

    /// Read a project's deploy config as a **JSON snap** for GetProjectInfra /
    /// provision plan.
    ///
    /// Resolution order (source of truth is the projects hub on disk):
    /// 1. Cached JSON: `.veil/deploy-state.json` or `deploy-state.json`
    /// 2. **`veil.toml`** on the hub (`{projects_dir}/{slug}/veil.toml`) — parse
    ///    `[deploy]` / `[[deploy.units]]` into the UI snap shape
    /// 3. Legacy `deploy.toml` / `config/deploy.toml` (only if already JSON)
    ///
    /// Returns a snap with at least `has_toml` / `toml_path` when veil.toml
    /// exists, even if there is no `[deploy]` section.
    pub fn read_project_deploy(slug: impl AsRef<str>) -> Result<String, FsError> {
        let dir = Self::projects_dir();
        let slug = slug.as_ref();
        let root = format!("{dir}/{slug}");

        // 1) Explicit JSON deploy-state cache (written by provisioner / tools)
        for path in [
            format!("{root}/.veil/deploy-state.json"),
            format!("{root}/deploy-state.json"),
        ] {
            if Path::new(&path).is_file() {
                let content = std::fs::read_to_string(&path)?;
                if content.trim_start().starts_with('{') {
                    return Ok(content);
                }
            }
        }

        // 2) Hub veil.toml — primary project config (including [deploy])
        let toml_path = format!("{root}/veil.toml");
        if Path::new(&toml_path).is_file() {
            let content = std::fs::read_to_string(&toml_path)?;
            let snap = parse_veil_toml_deploy_snap(&content, slug, &dir, &toml_path)?;
            return Ok(snap.to_string());
        }

        // 3) Legacy paths — only accept pre-baked JSON
        for path in [
            format!("{root}/deploy.toml"),
            format!("{root}/config/deploy.toml"),
        ] {
            if Path::new(&path).is_file() {
                let content = std::fs::read_to_string(&path)?;
                if content.trim_start().starts_with('{') {
                    return Ok(content);
                }
            }
        }

        // No hub project directory / no veil.toml
        Ok(
            serde_json::json!({
                "has_toml": false,
                "has_deploy": false,
                "slug": slug,
                "projects_dir": dir,
                "toml_path": null,
                "units": [],
            })
            .to_string(),
        )
    }

    /// Read a TOML file and return JSON when possible (tables → object).
    pub fn read_toml_json(path: impl AsRef<str>) -> Result<String, FsError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        if content.trim_start().starts_with('{') {
            return Ok(content);
        }
        let v: toml::Value = content
            .parse()
            .map_err(|e| FsError(format!("toml parse {}: {e}", path.as_ref())))?;
        let json = toml_to_json(&v);
        Ok(json.to_string())
    }

    /// Parse a TOML string and return it as JSON (for S3-sourced veil.toml content).
    pub fn parse_toml_str(content: impl AsRef<str>) -> Result<String, FsError> {
        let text = content.as_ref();
        if text.trim_start().starts_with('{') {
            return Ok(text.to_string());
        }
        let v: toml::Value = text
            .parse()
            .map_err(|e| FsError(format!("toml parse: {e}")))?;
        let json = toml_to_json(&v);
        Ok(json.to_string())
    }

    /// List deploy unit names for a project (directories under deploy/ or names from deploy.toml).
    pub fn project_unit_names(slug: impl AsRef<str>) -> Result<Vec<String>, FsError> {
        let dir = Self::projects_dir();
        let units_dir = format!("{}/{}/deploy", dir, slug.as_ref());
        if Path::new(&units_dir).is_dir() {
            return Self::list_dir(&units_dir);
        }
        // Fallback: return empty list
        Ok(Vec::new())
    }

    /// Get the deploy unit type for a named unit in a project.
    pub fn project_unit_type(slug: impl AsRef<str>, name: impl AsRef<str>) -> Result<String, FsError> {
        let dir = Self::projects_dir();
        let type_file = format!("{}/{}/deploy/{}/type", dir, slug.as_ref(), name.as_ref());
        if Path::new(&type_file).is_file() {
            return Ok(std::fs::read_to_string(&type_file)?.trim().to_string());
        }
        // Default type
        Ok("lambda-api".to_string())
    }

    /// Query [module.<namespace>] sections from a set of repo TOML entries.
    /// Each entry in entries_json is {repo_id, repo_name, slug, branch, raw: <bytes as array>}.
    /// Returns JSON: {results: [{repo_id, repo_name, slug, branch, module: {...}}], count: N}
    pub fn query_modules_from_tomls(
        entries_json: impl AsRef<str>,
        module: impl AsRef<str>,
        filters_json: impl AsRef<str>,
    ) -> Result<String, FsError> {
        let entries: Vec<serde_json::Value> = serde_json::from_str(entries_json.as_ref())
            .map_err(|e| FsError(format!("entries parse: {e}")))?;
        let filters: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(filters_json.as_ref()).unwrap_or_default();
        let ns = module.as_ref();

        let mut results = Vec::new();

        for entry in &entries {
            let repo_id = entry["repo_id"].as_str().unwrap_or_default();
            let repo_name = entry["repo_name"].as_str().unwrap_or_default();
            let slug = entry["slug"].as_str().unwrap_or_default();
            let branch = entry["branch"].as_str().unwrap_or_default();

            // raw is the S3 bytes — may arrive as JSON array of ints or a string
            let toml_content = if let Some(raw_arr) = entry["raw"].as_array() {
                let bytes: Vec<u8> = raw_arr
                    .iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                    .collect();
                String::from_utf8(bytes).unwrap_or_default()
            } else if let Some(s) = entry["raw"].as_str() {
                s.to_string()
            } else {
                continue;
            };

            // Parse TOML
            let config: toml::Value = match toml_content.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Look for [module.<namespace>]
            let mod_section = match config.get("module").and_then(|m| m.get(ns)) {
                Some(v) => v,
                None => continue,
            };

            let mod_json = toml_to_json(mod_section);

            // Apply filters
            let mut matches = true;
            if let serde_json::Value::Object(ref mod_obj) = mod_json {
                for (key, filter_val) in &filters {
                    match mod_obj.get(key) {
                        Some(actual) if actual == filter_val => {}
                        _ => {
                            matches = false;
                            break;
                        }
                    }
                }
            }

            if matches {
                results.push(serde_json::json!({
                    "repo_id": repo_id,
                    "repo_name": repo_name,
                    "slug": slug,
                    "branch": branch,
                    "module": mod_json,
                }));
            }
        }

        let count = results.len();
        Ok(serde_json::json!({ "results": results, "count": count }).to_string())
    }
}

// ─── veil.toml [deploy] → JSON snap (GetProjectInfra / plan_provision) ───────

fn parse_veil_toml_deploy_snap(
    content: &str,
    slug: &str,
    projects_dir: &str,
    toml_path: &str,
) -> Result<serde_json::Value, FsError> {
    let root: toml::Value = content
        .parse()
        .map_err(|e| FsError(format!("parse veil.toml for {slug}: {e}")))?;

    let deploy = root.get("deploy");
    let has_deploy = deploy.is_some();

    let mut region = String::new();
    let mut project_prefix = slug.to_string();
    let mut resource_prefix = "veil".to_string();
    let mut service = slug.to_string();
    let mut network = serde_json::json!({});
    let mut stack = serde_json::json!({});
    let mut units: Vec<serde_json::Value> = Vec::new();

    if let Some(d) = deploy {
        region = toml_str(d, "region").unwrap_or_default();
        if let Some(s) = toml_str(d, "project_prefix") {
            project_prefix = s;
        }
        if let Some(s) = toml_str(d, "resource_prefix") {
            resource_prefix = s;
        }
        if let Some(s) = toml_str(d, "service") {
            service = s;
        }
        if let Some(n) = d.get("network") {
            network = toml_to_json(n);
        }
        if let Some(st) = d.get("stack") {
            stack = toml_to_json(st);
        }
        // [[deploy.units]] — array of tables; nested [deploy.units.lambda] attaches
        // to the last unit via TOML semantics (already resolved by the parser).
        if let Some(arr) = d.get("units").and_then(|u| u.as_array()) {
            for u in arr {
                units.push(normalize_unit(u));
            }
        }
    }

    let base = stack
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{resource_prefix}-{service}"));

    let stack_names = expand_stack_names(&stack, &base);
    // Ensure stack.names exists for provision plan code paths
    if let Some(obj) = stack.as_object_mut() {
        obj.insert("base".into(), serde_json::json!(base.clone()));
        obj.insert("names".into(), stack_names.clone());
    } else if has_deploy {
        stack = serde_json::json!({
            "base": base,
            "names": stack_names,
        });
    }

    Ok(serde_json::json!({
        "has_toml": true,
        "has_deploy": has_deploy,
        "region": region,
        "project_prefix": project_prefix,
        "resource_prefix": resource_prefix,
        "service": service,
        "stack": stack,
        "units": units,
        "network": network,
        "projects_dir": projects_dir,
        "toml_path": toml_path,
        "slug": slug,
    }))
}

fn expand_stack_names(stack: &serde_json::Value, base: &str) -> serde_json::Value {
    let name_of = |section: &str, default_suffix: Option<&str>| -> String {
        stack
            .get(section)
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| match default_suffix {
                Some(suf) => format!("{base}-{suf}"),
                None => base.to_string(),
            })
    };
    let sqs = name_of("sqs", None);
    let dlq = stack
        .get("sqs")
        .and_then(|s| s.get("dlq_name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{sqs}-dlq"));
    serde_json::json!({
        "base": base,
        "dynamodb": name_of("dynamodb", None),
        "sns": name_of("sns", None),
        "sqs": sqs,
        "sqs_dlq": dlq,
        "lambda_api": name_of("lambda_api", Some("api")),
        "lambda_consumer": name_of("lambda_consumer", Some("consumer")),
    })
}

fn normalize_unit(u: &toml::Value) -> serde_json::Value {
    let mut j = toml_to_json(u);
    // UI expects `type` key; TOML field is `type` (quoted in strict TOML as type is reserved
    // in some parsers — toml crate exposes it as "type").
    if let Some(obj) = j.as_object_mut() {
        if !obj.contains_key("type") {
            if let Some(t) = obj.remove("unit_type") {
                obj.insert("type".into(), t);
            }
        }
    }
    j
}

fn toml_str(v: &toml::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(*i),
        toml::Value::Float(f) => serde_json::json!(*f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, val) in t {
                map.insert(k.clone(), toml_to_json(val));
            }
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_relay_style_veil_toml() {
        let toml = r#"
name = "relay"
[deploy]
region = "us-west-2"
project_prefix = "relay"
resource_prefix = "veil"
service = "relay"
[deploy.stack.dynamodb]
billing = "pay_per_request"
[deploy.stack.lambda_api]
[deploy.stack.lambda_consumer]
[deploy.network]
vpc = "dashlx"
[[deploy.units]]
name = "relay-api"
type = "lambda-api"
context = "relay"
description = "API"
stack_role = "lambda_api"
[deploy.units.lambda]
memory_mb = 1024
timeout_seconds = 30
[deploy.units.api_gateway]
gateway = "dashlx-services"
path_prefix = "/relay"
[[deploy.units]]
name = "relay-consumer"
type = "lambda-consumer"
context = "relay"
stack_role = "lambda_consumer"
"#;
        let snap = parse_veil_toml_deploy_snap(toml, "relay", "/hub", "/hub/relay/veil.toml").unwrap();
        assert_eq!(snap["has_toml"], true);
        assert_eq!(snap["has_deploy"], true);
        assert_eq!(snap["region"], "us-west-2");
        assert_eq!(snap["toml_path"], "/hub/relay/veil.toml");
        assert_eq!(snap["stack"]["names"]["base"], "veil-relay");
        assert_eq!(snap["stack"]["names"]["lambda_api"], "veil-relay-api");
        let units = snap["units"].as_array().unwrap();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0]["name"], "relay-api");
        assert_eq!(units[0]["type"], "lambda-api");
        assert_eq!(units[0]["lambda"]["memory_mb"], 1024);
        assert_eq!(units[0]["api_gateway"]["path_prefix"], "/relay");
        assert_eq!(units[1]["name"], "relay-consumer");
    }

    #[test]
    fn no_deploy_section_still_has_toml() {
        let toml = r#"
name = "hello"
[package]
name = "hello"
"#;
        let snap = parse_veil_toml_deploy_snap(toml, "hello", "/hub", "/hub/hello/veil.toml").unwrap();
        assert_eq!(snap["has_toml"], true);
        assert_eq!(snap["has_deploy"], false);
        assert!(snap["units"].as_array().unwrap().is_empty());
    }
}
