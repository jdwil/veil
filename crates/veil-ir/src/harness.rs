//! Declared harness policy (layers + `veil.toml` `[harness]`).
//!
//! Codegen does **not** emit from this yet. This module only parses and merges
//! knobs so layers can declare reusable profiles and products can override them.
//! See `docs/DESIGN_CONFIGURABLE_HARNESS.md` and `docs/POLICY_ROLES.md`.
//!
//! INV-001: engine matches these **tokens**, not product vocabulary (`ctx`,
//! `@route`, `"Bus"`). Trait names in `provided_runtime_traits` are layer-owned.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// CORS origin mode. Same token in layer, toml, and IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CorsMode {
    #[default]
    Localhost,
    Env,
    Permissive,
    None,
}

impl CorsMode {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "localhost" => Some(Self::Localhost),
            "env" => Some(Self::Env),
            "permissive" => Some(Self::Permissive),
            "none" | "-" | "" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Localhost => "localhost",
            Self::Env => "env",
            Self::Permissive => "permissive",
            Self::None => "none",
        }
    }
}

/// Auth mode for the generated local bin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    #[default]
    ApiKey,
    None,
}

impl AuthMode {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "api_key" | "apikey" => Some(Self::ApiKey),
            "none" | "-" | "" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::None => "none",
        }
    }
}

/// When to emit `veil_bin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmitBin {
    /// Emit when compose / `@main` / template main / `link veil_server` / endpoints.
    #[default]
    OnEntry,
    Never,
}

impl EmitBin {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "on_entry" | "on-entry" => Some(Self::OnEntry),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnEntry => "on_entry",
            Self::Never => "never",
        }
    }
}

/// Multi-package path collision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollideMode {
    #[default]
    Error,
    PrefixCrate,
}

impl CollideMode {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "error" => Some(Self::Error),
            "prefix_crate" | "prefix-crate" => Some(Self::PrefixCrate),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::PrefixCrate => "prefix_crate",
        }
    }
}

/// Compat synthesis for undeclared deps/compose/endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompatMode {
    #[default]
    Auto,
    Off,
}

impl CompatMode {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "auto" => Some(Self::Auto),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
}

/// How Bus/auth (and other `role:runtime_provider` traits) get a local impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BusWire {
    #[default]
    Explicit,
    SynthesizeRuntime,
}

impl BusWire {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "explicit" => Some(Self::Explicit),
            "synthesize_runtime" | "synthesize-runtime" | "synthesize" => {
                Some(Self::SynthesizeRuntime)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::SynthesizeRuntime => "synthesize_runtime",
        }
    }
}

/// Fill missing endpoint binds from HTTP method (profile) or require them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BindDefaults {
    #[default]
    Method,
    None,
}

impl BindDefaults {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "method" => Some(Self::Method),
            "none" | "-" | "" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Method => "method",
            Self::None => "none",
        }
    }
}

/// Extra DELETE inputs (non-path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeleteExtras {
    #[default]
    Query,
    Body,
    Error,
}

impl DeleteExtras {
    pub fn parse(s: &str) -> Option<Self> {
        match normalize_token(s) {
            "query" => Some(Self::Query),
            "body" => Some(Self::Body),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Body => "body",
            Self::Error => "error",
        }
    }
}

/// Merged harness knobs. Absent `Option` = not set at this layer (keep previous).
///
/// Documented `axum_http` defaults live in [`HarnessPolicy::documented_defaults`].
/// `LayerRegistry` starts with those so products inherit a complete table even
/// before a layer ships `harness_policy`. Codegen does not read this yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessPolicy {
    pub profile: Option<String>,
    pub bin: Option<String>,
    pub listen_env: Option<String>,
    pub listen_default: Option<u16>,
    /// Full `host:port` from toml `listen = "0.0.0.0:3000"`.
    pub listen: Option<String>,
    /// `None` after explicit `health none`.
    pub health: Option<String>,
    pub cors: Option<CorsMode>,
    pub cors_outside_auth: Option<bool>,
    pub auth: Option<AuthMode>,
    pub emit_bin: Option<EmitBin>,
    pub bus_wire: Option<BusWire>,
    pub collide: Option<CollideMode>,
    pub bind_defaults: Option<BindDefaults>,
    pub delete_extras: Option<DeleteExtras>,
    /// Toml-only until a layer needs it.
    pub compat: Option<CompatMode>,
    pub path_prefix: Option<String>,
    /// Layer-declared trait names that `compose` may `wire …: provided_runtime`.
    pub provided_runtime_traits: Vec<String>,
    /// Product `[harness.wire]` field → adapter name overrides.
    #[serde(default)]
    pub wire: BTreeMap<String, String>,
}

impl Default for HarnessPolicy {
    fn default() -> Self {
        Self {
            profile: None,
            bin: None,
            listen_env: None,
            listen_default: None,
            listen: None,
            health: None,
            cors: None,
            cors_outside_auth: None,
            auth: None,
            emit_bin: None,
            bus_wire: None,
            collide: None,
            bind_defaults: None,
            delete_extras: None,
            compat: None,
            path_prefix: None,
            provided_runtime_traits: Vec::new(),
            wire: BTreeMap::new(),
        }
    }
}

impl HarnessPolicy {
    /// Documented `axum_http` defaults (design §5.3). Not applied by codegen yet.
    pub fn documented_defaults() -> Self {
        Self {
            profile: Some("axum_http".into()),
            bin: Some("veil_bin".into()),
            listen_env: Some("PORT".into()),
            listen_default: Some(3000),
            listen: None,
            health: Some("/health".into()),
            cors: Some(CorsMode::Localhost),
            cors_outside_auth: Some(true),
            auth: Some(AuthMode::ApiKey),
            emit_bin: Some(EmitBin::OnEntry),
            bus_wire: Some(BusWire::Explicit),
            collide: Some(CollideMode::Error),
            bind_defaults: Some(BindDefaults::Method),
            delete_extras: Some(DeleteExtras::Query),
            compat: Some(CompatMode::Auto),
            path_prefix: None,
            provided_runtime_traits: Vec::new(),
            wire: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.profile.is_none()
            && self.bin.is_none()
            && self.listen_env.is_none()
            && self.listen_default.is_none()
            && self.listen.is_none()
            && self.health.is_none()
            && self.cors.is_none()
            && self.cors_outside_auth.is_none()
            && self.auth.is_none()
            && self.emit_bin.is_none()
            && self.bus_wire.is_none()
            && self.collide.is_none()
            && self.bind_defaults.is_none()
            && self.delete_extras.is_none()
            && self.compat.is_none()
            && self.path_prefix.is_none()
            && self.provided_runtime_traits.is_empty()
            && self.wire.is_empty()
    }
}

fn normalize_token(s: &str) -> &str {
    s.trim()
}

fn parse_bool(s: &str) -> Option<bool> {
    match normalize_token(s).to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Sentinel: overlay said `none` / `""` and the merged value must be cleared.
pub const HARNESS_CLEAR: &str = "\0clear";

fn optional_string(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t == "-" || t.eq_ignore_ascii_case("none") {
        Some(HARNESS_CLEAR.to_string())
    } else {
        Some(t.to_string())
    }
}

fn resolve_str(over: &Option<String>, base: &Option<String>) -> Option<String> {
    match over {
        None => base.clone(),
        Some(s) if s == HARNESS_CLEAR => None,
        Some(s) => Some(s.clone()),
    }
}

/// Parse a top-level `harness_policy` block. `None` if the file has no such block.
pub fn parse_harness_policy(content: &str) -> Option<HarnessPolicy> {
    let mut in_block = false;
    let mut pol = HarnessPolicy::default();
    let mut found = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "harness_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !t.starts_with("profile")
            && !t.starts_with("bin")
            && !t.starts_with("listen")
            && !t.starts_with("health")
            && !t.starts_with("cors")
            && !t.starts_with("auth")
            && !t.starts_with("emit_bin")
            && !t.starts_with("bus_wire")
            && !t.starts_with("collide")
            && !t.starts_with("bind_defaults")
            && !t.starts_with("delete_extras")
            && !t.starts_with("provided_runtime_trait")
            && !t.starts_with("path_prefix")
            && !t.starts_with("compat")
        {
            break;
        }
        apply_policy_line(&mut pol, t);
    }
    if found {
        Some(pol)
    } else {
        None
    }
}

fn apply_policy_line(pol: &mut HarnessPolicy, t: &str) {
    if let Some(rest) = t.strip_prefix("provided_runtime_trait ") {
        let name = rest.trim();
        if !name.is_empty() && !pol.provided_runtime_traits.iter().any(|n| n == name) {
            pol.provided_runtime_traits.push(name.to_string());
        }
        return;
    }
    if let Some(rest) = t.strip_prefix("profile ") {
        pol.profile = optional_string(rest);
    } else if let Some(rest) = t.strip_prefix("bin ") {
        pol.bin = optional_string(rest);
    } else if let Some(rest) = t.strip_prefix("listen_env ") {
        pol.listen_env = optional_string(rest);
    } else if let Some(rest) = t.strip_prefix("listen_default ") {
        pol.listen_default = rest.trim().parse().ok();
    } else if let Some(rest) = t.strip_prefix("listen ") {
        pol.listen = optional_string(rest);
    } else if let Some(rest) = t.strip_prefix("health ") {
        pol.health = optional_string(rest);
    } else if let Some(rest) = t.strip_prefix("cors_outside_auth ") {
        pol.cors_outside_auth = parse_bool(rest);
    } else if let Some(rest) = t.strip_prefix("cors ") {
        pol.cors = CorsMode::parse(rest);
    } else if let Some(rest) = t.strip_prefix("auth ") {
        pol.auth = AuthMode::parse(rest);
    } else if let Some(rest) = t.strip_prefix("emit_bin ") {
        pol.emit_bin = EmitBin::parse(rest);
    } else if let Some(rest) = t.strip_prefix("bus_wire ") {
        pol.bus_wire = BusWire::parse(rest);
    } else if let Some(rest) = t.strip_prefix("collide ") {
        pol.collide = CollideMode::parse(rest);
    } else if let Some(rest) = t.strip_prefix("bind_defaults ") {
        pol.bind_defaults = BindDefaults::parse(rest);
    } else if let Some(rest) = t.strip_prefix("delete_extras ") {
        pol.delete_extras = DeleteExtras::parse(rest);
    } else if let Some(rest) = t.strip_prefix("compat ") {
        pol.compat = CompatMode::parse(rest);
    } else if let Some(rest) = t.strip_prefix("path_prefix ") {
        pol.path_prefix = optional_string(rest);
    }
}

fn merge_opt<T: Clone>(over: &Option<T>, base: &Option<T>) -> Option<T> {
    match over {
        Some(v) => Some(v.clone()),
        None => base.clone(),
    }
}

/// Later `use` / toml overlay wins for set keys. Trait lists are a union
/// (overlay first, then base names not already present).
pub fn merge_harness_policy(base: &HarnessPolicy, over: &HarnessPolicy) -> HarnessPolicy {
    let mut traits = over.provided_runtime_traits.clone();
    for t in &base.provided_runtime_traits {
        if !traits.iter().any(|x| x == t) {
            traits.push(t.clone());
        }
    }
    HarnessPolicy {
        profile: resolve_str(&over.profile, &base.profile),
        bin: resolve_str(&over.bin, &base.bin),
        listen_env: resolve_str(&over.listen_env, &base.listen_env),
        listen_default: merge_opt(&over.listen_default, &base.listen_default),
        listen: resolve_str(&over.listen, &base.listen),
        health: resolve_str(&over.health, &base.health),
        cors: merge_opt(&over.cors, &base.cors),
        cors_outside_auth: merge_opt(&over.cors_outside_auth, &base.cors_outside_auth),
        auth: merge_opt(&over.auth, &base.auth),
        emit_bin: merge_opt(&over.emit_bin, &base.emit_bin),
        bus_wire: merge_opt(&over.bus_wire, &base.bus_wire),
        collide: merge_opt(&over.collide, &base.collide),
        bind_defaults: merge_opt(&over.bind_defaults, &base.bind_defaults),
        delete_extras: merge_opt(&over.delete_extras, &base.delete_extras),
        compat: merge_opt(&over.compat, &base.compat),
        path_prefix: resolve_str(&over.path_prefix, &base.path_prefix),
        provided_runtime_traits: traits,
        wire: {
            let mut w = base.wire.clone();
            w.extend(over.wire.clone());
            w
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_harness_policy_block() {
        let src = r#"
pkg demo v1
  harness_policy
    profile axum_http
    bin veil_bin
    listen_env PORT
    listen_default 3000
    health /health
    cors localhost
    cors_outside_auth true
    auth api_key
    emit_bin on_entry
    bus_wire explicit
    collide error
    bind_defaults method
    delete_extras query
    provided_runtime_trait Bus
    provided_runtime_trait AuthService
  construct X
    kw x
    mt struct
"#;
        let pol = parse_harness_policy(src).expect("block");
        assert_eq!(pol.profile.as_deref(), Some("axum_http"));
        assert_eq!(pol.bin.as_deref(), Some("veil_bin"));
        assert_eq!(pol.listen_env.as_deref(), Some("PORT"));
        assert_eq!(pol.listen_default, Some(3000));
        assert_eq!(pol.health.as_deref(), Some("/health"));
        assert_eq!(pol.cors, Some(CorsMode::Localhost));
        assert_eq!(pol.cors_outside_auth, Some(true));
        assert_eq!(pol.auth, Some(AuthMode::ApiKey));
        assert_eq!(pol.emit_bin, Some(EmitBin::OnEntry));
        assert_eq!(pol.bus_wire, Some(BusWire::Explicit));
        assert_eq!(pol.collide, Some(CollideMode::Error));
        assert_eq!(pol.provided_runtime_traits, vec!["Bus", "AuthService"]);
    }

    #[test]
    fn parse_none_clears_health_and_cors() {
        let src = "harness_policy\n  health none\n  cors none\n  auth none\n";
        let pol = parse_harness_policy(src).unwrap();
        assert_eq!(pol.health.as_deref(), Some(HARNESS_CLEAR));
        assert_eq!(pol.cors, Some(CorsMode::None));
        assert_eq!(pol.auth, Some(AuthMode::None));
        let merged = merge_harness_policy(&HarnessPolicy::documented_defaults(), &pol);
        assert_eq!(merged.health, None);
        assert_eq!(merged.cors, Some(CorsMode::None));
    }

    #[test]
    fn merge_later_wins_traits_union() {
        let base = HarnessPolicy {
            profile: Some("axum_http".into()),
            cors: Some(CorsMode::Localhost),
            provided_runtime_traits: vec!["Bus".into()],
            ..HarnessPolicy::default()
        };
        let over = HarnessPolicy {
            cors: Some(CorsMode::Env),
            provided_runtime_traits: vec!["Clock".into()],
            ..HarnessPolicy::default()
        };
        let m = merge_harness_policy(&base, &over);
        assert_eq!(m.profile.as_deref(), Some("axum_http"));
        assert_eq!(m.cors, Some(CorsMode::Env));
        assert_eq!(m.provided_runtime_traits, vec!["Clock", "Bus"]);
    }

    #[test]
    fn absent_file_has_no_block() {
        assert!(parse_harness_policy("pkg x v1\n  construct Y\n    mt struct\n").is_none());
    }
}
