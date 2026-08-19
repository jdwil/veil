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

/// HTTP method tokens (protocol, not product vocabulary).
pub fn is_http_verb(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    )
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

// ─── HarnessIR (lowered, role-driven) ────────────────────────────────────────

use crate::ast::{Construct, Solution, TopLevelItem, TypeExpr};
use crate::layer::{LayerRegistry, Shape};

/// Listen bind for the generated bin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenSpec {
    pub env: String,
    pub host: String,
    pub default_port: u16,
}

impl Default for ListenSpec {
    fn default() -> Self {
        Self {
            env: "PORT".into(),
            host: "0.0.0.0".into(),
            default_port: 3000,
        }
    }
}

/// Lowered harness. No product annotation names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessIR {
    pub profile: String,
    pub bin_name: String,
    pub listen: ListenSpec,
    pub health_path: Option<String>,
    pub cors: CorsMode,
    pub cors_outside_auth: bool,
    pub auth: AuthMode,
    pub path_prefix: Option<String>,
    pub collide: CollideMode,
    pub emit_bin: EmitBin,
    pub compat: CompatMode,
    pub contexts: Vec<HarnessContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessContext {
    pub crate_name: String,
    pub module_name: String,
    pub deps: Option<DepsDecl>,
    pub compose: Option<ComposeDecl>,
    pub endpoints: Vec<EndpointDecl>,
    pub bus_handlers: Vec<BusHandlerDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepsDecl {
    pub type_name: String,
    pub fields: Vec<DepsField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepsField {
    pub name: String,
    pub trait_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposeDecl {
    pub name: String,
    pub bundle: String,
    pub wires: Vec<WireDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDecl {
    pub field: String,
    pub kind: WireKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireKind {
    Adapter { name: String },
    ProvidedRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointDecl {
    pub name: String,
    pub method: String,
    pub path: String,
    pub handler: String,
    pub binds: Vec<BindDecl>,
    /// `endpoint` | `compat_route` | `compat_name`
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindDecl {
    pub input: String,
    pub source: BindSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindSource {
    Path,
    Query,
    Header,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusHandlerDecl {
    pub name: String,
}

/// True when the package authored compose + at least one endpoint.
pub fn has_declared_harness(sol: &Solution, registry: &LayerRegistry) -> bool {
    let mut compose = false;
    let mut endpoint = false;
    for c in iter_constructs(sol) {
        walk_roles(c, registry, &mut compose, &mut endpoint);
    }
    compose && endpoint
}

fn walk_roles(c: &Construct, registry: &LayerRegistry, compose: &mut bool, endpoint: &mut bool) {
    if registry.construct_has_role(c, "compose") {
        *compose = true;
    }
    if registry.construct_has_role(c, "http_endpoint") {
        *endpoint = true;
    }
    for child in &c.children {
        walk_roles(child, registry, compose, endpoint);
    }
}

/// Lower Solution → HarnessIR (declared constructs + optional compat synthesis).
pub fn lower_harness(sol: &Solution, registry: &LayerRegistry) -> HarnessIR {
    let p = &registry.harness_policy;
    let mut ir = HarnessIR {
        profile: p.profile.clone().unwrap_or_else(|| "axum_http".into()),
        bin_name: p.bin.clone().unwrap_or_else(|| "veil_bin".into()),
        listen: listen_from_policy(p),
        health_path: p.health.clone(),
        cors: p.cors.unwrap_or(CorsMode::Localhost),
        cors_outside_auth: p.cors_outside_auth.unwrap_or(true),
        auth: p.auth.unwrap_or(AuthMode::ApiKey),
        path_prefix: p.path_prefix.clone(),
        collide: p.collide.unwrap_or(CollideMode::Error),
        emit_bin: p.emit_bin.unwrap_or(EmitBin::OnEntry),
        compat: p.compat.unwrap_or(CompatMode::Auto),
        contexts: Vec::new(),
    };

    let mods: Vec<&Construct> = sol
        .items
        .iter()
        .filter_map(|i| match i {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(c),
            _ => None,
        })
        .collect();

    if mods.is_empty() {
        let ctx = lower_context("app", "App", sol, &iter_constructs(sol).collect::<Vec<_>>(), registry, ir.compat);
        if ctx.deps.is_some() || ctx.compose.is_some() || !ctx.endpoints.is_empty() {
            ir.contexts.push(ctx);
        }
    } else {
        for m in mods {
            let mut members = Vec::new();
            collect_constructs(m, &mut members);
            ir.contexts.push(lower_context(
                &to_snake(&m.name),
                &m.name,
                sol,
                &members,
                registry,
                ir.compat,
            ));
        }
    }
    ir
}

pub fn list_endpoints_from_ir(ir: &HarnessIR) -> Vec<&EndpointDecl> {
    ir.contexts.iter().flat_map(|c| c.endpoints.iter()).collect()
}

fn listen_from_policy(p: &HarnessPolicy) -> ListenSpec {
    let mut spec = ListenSpec {
        env: p.listen_env.clone().unwrap_or_else(|| "PORT".into()),
        host: "0.0.0.0".into(),
        default_port: p.listen_default.unwrap_or(3000),
    };
    if let Some(listen) = &p.listen {
        if let Some((host, port)) = listen.rsplit_once(':') {
            spec.host = host.to_string();
            if let Ok(n) = port.parse() {
                spec.default_port = n;
            }
        }
    }
    spec
}

fn lower_context(
    crate_name: &str,
    module_name: &str,
    sol: &Solution,
    members: &[&Construct],
    registry: &LayerRegistry,
    compat: CompatMode,
) -> HarnessContext {
    let deps = members
        .iter()
        .copied()
        .find(|c| registry.construct_has_role(c, "deps_bundle"))
        .map(lower_deps);
    let compose = members
        .iter()
        .copied()
        .find(|c| registry.construct_has_role(c, "compose"))
        .map(|c| lower_compose(c, registry));
    let mut endpoints: Vec<EndpointDecl> = members
        .iter()
        .copied()
        .filter(|c| registry.construct_has_role(c, "http_endpoint"))
        .filter_map(|c| lower_endpoint(c, registry))
        .collect();

    if endpoints.is_empty() && compat == CompatMode::Auto {
        endpoints = synthesize_compat_endpoints(members, registry);
    }

    // Only when this context's Deps actually includes a routing trait.
    // (Do not register every fn just because some layer declared Bus.)
    let routing_on_bundle = deps.as_ref().is_some_and(|d| {
        d.fields.iter().any(|f| {
            registry.routing_traits().iter().any(|t| t == &f.trait_name)
        })
    });

    let bus_handlers = if routing_on_bundle {
        members
            .iter()
            .copied()
            .filter(|c| c.shape == Shape::Fn)
            .filter(|c| !registry.construct_has_role(c, crate::deploy_hooks::DEPLOY_HOOK_ROLE))
            .map(|c| BusHandlerDecl {
                name: c.name.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };

    let _ = sol;
    HarnessContext {
        crate_name: crate_name.to_string(),
        module_name: module_name.to_string(),
        deps,
        compose,
        endpoints,
        bus_handlers,
    }
}

fn lower_deps(c: &Construct) -> DepsDecl {
    DepsDecl {
        type_name: c.name.clone(),
        fields: c
            .fields
            .iter()
            .filter_map(|f| {
                Some(DepsField {
                    name: f.name.clone(),
                    trait_name: type_name(&f.type_expr)?.to_string(),
                })
            })
            .collect(),
    }
}

fn lower_compose(c: &Construct, registry: &LayerRegistry) -> ComposeDecl {
    let bundle = c
        .fields
        .iter()
        .find(|f| f.name == "bundle")
        .and_then(|f| type_name(&f.type_expr))
        .unwrap_or("")
        .to_string();
    let wires = c
        .blocks
        .iter()
        .filter(|b| b.keyword == "wire")
        .flat_map(|b| b.fields.iter())
        .filter_map(|f| {
            let target = type_name(&f.type_expr)?;
            let kind = if target == "provided_runtime"
                || registry.constructs.iter().any(|s| {
                    (s.keyword == target || s.name == target)
                        && s.roles.iter().any(|r| r == "runtime_provider")
                })
            {
                WireKind::ProvidedRuntime
            } else {
                WireKind::Adapter {
                    name: target.to_string(),
                }
            };
            Some(WireDecl {
                field: f.name.clone(),
                kind,
            })
        })
        .collect();
    ComposeDecl {
        name: c.name.clone(),
        bundle,
        wires,
    }
}

fn lower_endpoint(c: &Construct, registry: &LayerRegistry) -> Option<EndpointDecl> {
    let method = c
        .fields
        .iter()
        .find(|f| f.name == "method")
        .and_then(|f| type_name(&f.type_expr))?
        .to_ascii_uppercase();
    let path = c
        .fields
        .iter()
        .find(|f| f.name == "path")
        .and_then(|f| type_name(&f.type_expr))?
        .to_string();
    let handler = c
        .fields
        .iter()
        .find(|f| f.name == "handle")
        .and_then(|f| type_name(&f.type_expr))?
        .to_string();
    let binds = c
        .blocks
        .iter()
        .filter(|b| b.keyword == "bind")
        .flat_map(|b| b.fields.iter())
        .filter_map(|f| {
            Some(BindDecl {
                input: f.name.clone(),
                source: parse_bind_source(type_name(&f.type_expr)?)?,
            })
        })
        .collect();
    let mut path = path;
    if let Some(pre) = &registry.harness_policy.path_prefix {
        if !path.starts_with(pre) {
            path = format!("{}{}", pre.trim_end_matches('/'), path);
        }
    }
    Some(EndpointDecl {
        name: c.name.clone(),
        method,
        path,
        handler,
        binds,
        via: "endpoint".into(),
    })
}

fn parse_bind_source(s: &str) -> Option<BindSource> {
    match s {
        "path" => Some(BindSource::Path),
        "query" => Some(BindSource::Query),
        "header" => Some(BindSource::Header),
        "body" => Some(BindSource::Body),
        _ => None,
    }
}

fn synthesize_compat_endpoints(
    members: &[&Construct],
    registry: &LayerRegistry,
) -> Vec<EndpointDecl> {
    let fns: Vec<&Construct> = members.iter().copied().filter(|c| c.shape == Shape::Fn).collect();
    let with_route: Vec<&Construct> = fns
        .iter()
        .copied()
        .filter(|c| registry.construct_has_http_route(c))
        .collect();
    // Bit-compat: if any role:http_route exists, only those; else every fn.
    let routable = if !with_route.is_empty() {
        with_route
    } else {
        fns
    };
    let mut out = Vec::new();
    for svc in routable {
        let (method, path, via) = compat_rest_route(svc, registry);
        out.push(EndpointDecl {
            name: format!("{}Http", svc.name),
            method: method.to_ascii_uppercase(),
            path,
            handler: svc.name.clone(),
            binds: Vec::new(),
            via,
        });
    }
    out
}

/// Same method/path table as today's `rest_route_for_service` (http_route +
/// English prefixes + unconditional POST `/api/{snake}` fallback).
///
/// Returns `(METHOD, path, via)` where `via` is `compat_route` or `compat_name`.
pub fn compat_rest_route(
    svc: &Construct,
    registry: &LayerRegistry,
) -> (String, String, String) {
    let use_id = service_has_id_input(svc, registry);
    if let Some(ann) = registry.http_route_annotation(svc) {
        if let Some(raw) = ann.args.first() {
            let s = raw.trim().trim_matches('"').trim_matches('\'');
            let mut parts = s.splitn(2, char::is_whitespace);
            if let (Some(first), Some(path)) = (parts.next(), parts.next()) {
                let path = path.trim();
                if is_http_verb(first) && path.starts_with('/') {
                    return (
                        first.to_ascii_uppercase(),
                        path.to_string(),
                        "compat_route".into(),
                    );
                }
            }
            if s.starts_with('/') {
                let (m, _) = derive_name_route(&svc.name, registry, use_id);
                return (m, s.to_string(), "compat_route".into());
            }
        }
    }
    let (m, p) = derive_name_route(&svc.name, registry, use_id);
    (m, p, "compat_name".into())
}

/// True when `compose` may wire this trait as `provided_runtime`.
pub fn trait_is_provided_runtime(trait_name: &str, registry: &LayerRegistry) -> bool {
    if trait_name.is_empty() {
        return false;
    }
    if registry
        .harness_policy
        .provided_runtime_traits
        .iter()
        .any(|t| t == trait_name)
    {
        return true;
    }
    if registry.routing_traits().iter().any(|t| t == trait_name) {
        return true;
    }
    if registry.is_auth_service_trait(trait_name) {
        return true;
    }
    registry.constructs.iter().any(|s| {
        (s.name == trait_name || s.keyword == trait_name)
            && s.roles.iter().any(|r| r == "runtime_provider")
    })
}

fn service_has_id_input(svc: &Construct, registry: &LayerRegistry) -> bool {
    // Only a bare `id` input maps to REST `/{id}` (matches rust.rs).
    svc.inputs.iter().any(|i| {
        if registry.field_is_dependency(i) {
            return false;
        }
        to_snake(&i.name) == "id"
    })
}

fn pluralize_resource(snake: &str) -> String {
    if snake.is_empty() {
        return snake.to_string();
    }
    if snake.ends_with('s') {
        return snake.to_string();
    }
    if snake.ends_with("sh")
        || snake.ends_with("ch")
        || snake.ends_with('x')
        || snake.ends_with('z')
    {
        return format!("{snake}es");
    }
    if snake.ends_with('y') {
        let prev = snake.chars().rev().nth(1);
        if prev.map(|c| !"aeiou".contains(c)).unwrap_or(true) {
            return format!("{}ies", &snake[..snake.len() - 1]);
        }
    }
    format!("{snake}s")
}

fn derive_name_route(
    service_name: &str,
    registry: &LayerRegistry,
    use_id_path: bool,
) -> (String, String) {
    let pol = &registry.http_name_policy;
    let path_root = pol.path_prefix.as_deref().unwrap_or("/api/");
    let pairs: [(&Option<String>, &str, bool); 5] = [
        (&pol.list_prefix, "GET", true),
        (&pol.get_prefix, "GET", false),
        (&pol.create_prefix, "POST", true),
        (&pol.update_prefix, "PUT", false),
        (&pol.delete_prefix, "DELETE", false),
    ];
    for (prefix_opt, method, collection) in pairs {
        let Some(prefix) = prefix_opt.as_ref() else {
            continue;
        };
        if prefix.is_empty() {
            continue;
        }
        if let Some(resource) = service_name.strip_prefix(prefix.as_str()) {
            if resource.is_empty() {
                continue;
            }
            let snake = to_snake(resource);
            let plural = pluralize_resource(&snake);
            let path = if collection || method == "POST" || !use_id_path {
                format!("{path_root}{plural}")
            } else {
                format!("{path_root}{plural}/{{id}}")
            };
            return ((*method).to_string(), path);
        }
    }
    let fallback = format!(
        "{}/{}",
        path_root.trim_end_matches('/'),
        to_snake(service_name).replace('_', "-")
    );
    ("POST".into(), fallback)
}

fn type_name(ty: &TypeExpr) -> Option<&str> {
    match ty {
        TypeExpr::Named(n) | TypeExpr::LitStr(n) => Some(n.as_str()),
        _ => None,
    }
}

fn to_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn iter_constructs(sol: &Solution) -> impl Iterator<Item = &Construct> {
    sol.items.iter().filter_map(|i| match i {
        TopLevelItem::Construct(c) => Some(c),
        _ => None,
    })
}

fn collect_constructs<'a>(c: &'a Construct, out: &mut Vec<&'a Construct>) {
    out.push(c);
    for child in &c.children {
        collect_constructs(child, out);
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

    #[test]
    fn lower_declared_endpoint() {
        let mut reg = crate::layer::LayerRegistry::builtin();
        reg.load_content("harness", include_str!("../../../layers/harness.layer"))
            .unwrap();
        let mut ep = crate::ast::Construct::new(
            "endpoint",
            "HttpEndpoint",
            crate::layer::Shape::Struct,
            "CreateItemHttp".into(),
            crate::span::Span::new(0, 0),
        );
        ep.fields.push(crate::ast::Field {
            annotations: Vec::new(),
            name: "method".into(),
            type_expr: crate::ast::TypeExpr::Named("POST".into()),
            default_expr: None,
            span: crate::span::Span::new(0, 0),
        });
        ep.fields.push(crate::ast::Field {
            annotations: Vec::new(),
            name: "path".into(),
            type_expr: crate::ast::TypeExpr::LitStr("/api/items".into()),
            default_expr: None,
            span: crate::span::Span::new(0, 0),
        });
        ep.fields.push(crate::ast::Field {
            annotations: Vec::new(),
            name: "handle".into(),
            type_expr: crate::ast::TypeExpr::Named("CreateItem".into()),
            default_expr: None,
            span: crate::span::Span::new(0, 0),
        });
        let sol = crate::ast::Solution {
            name: "App".into(),
            span: crate::span::Span::new(0, 0),
            uses: Vec::new(),
            links: Vec::new(),
            items: vec![crate::ast::TopLevelItem::Construct(ep)],
            expose: None,
            guidance: Vec::new(),
        };
        let ir = lower_harness(&sol, &reg);
        assert_eq!(ir.endpoints_count_for_test(), 1);
        let ep = &ir.contexts[0].endpoints[0];
        assert_eq!(ep.method, "POST");
        assert_eq!(ep.path, "/api/items");
        assert_eq!(ep.handler, "CreateItem");
        assert_eq!(ep.via, "endpoint");
    }

    fn fn_construct(name: &str, inputs: &[(&str, &str)]) -> crate::ast::Construct {
        let mut c = crate::ast::Construct::new(
            "svc",
            "ApplicationService",
            crate::layer::Shape::Fn,
            name.into(),
            crate::span::Span::new(0, 0),
        );
        for (n, ty) in inputs {
            c.inputs.push(crate::ast::Field {
                annotations: Vec::new(),
                name: (*n).into(),
                type_expr: crate::ast::TypeExpr::Named((*ty).into()),
                default_expr: None,
                span: crate::span::Span::new(0, 0),
            });
        }
        c
    }

    #[test]
    fn compat_synthesis_post_fallback_and_use_id() {
        let mut reg = crate::layer::LayerRegistry::builtin();
        reg.load_content("rest_english", include_str!("../../../layers/rest_english.layer"))
            .unwrap();
        let greet = fn_construct("GreetUser", &[("name", "Str")]);
        let get = fn_construct("GetItem", &[("id", "Id")]);
        let list = fn_construct("ListItem", &[]);
        let (m, p, via) = compat_rest_route(&greet, &reg);
        assert_eq!((m.as_str(), p.as_str(), via.as_str()), ("POST", "/api/greet-user", "compat_name"));
        let (m, p, via) = compat_rest_route(&get, &reg);
        assert_eq!((m.as_str(), p.as_str(), via.as_str()), ("GET", "/api/items/{id}", "compat_name"));
        let (m, p, via) = compat_rest_route(&list, &reg);
        assert_eq!((m.as_str(), p.as_str(), via.as_str()), ("GET", "/api/items", "compat_name"));
    }
}

impl HarnessIR {
    #[cfg(test)]
    fn endpoints_count_for_test(&self) -> usize {
        self.contexts.iter().map(|c| c.endpoints.len()).sum()
    }
}
