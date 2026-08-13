//! Layer registry — the single source of truth for construct vocabulary.
//!
//! The VEIL engine contains zero domain knowledge. All vocabulary (keywords,
//! shapes, visuals, constraints) is loaded from `.layer` files at runtime and
//! resolved into a `LayerRegistry`.
//!
//! Layers are stackable: a construct's `maps_to` may name a core shape
//! (`mod`, `struct`, `enum`, `trait`, `impl`, `fn`, `group`) or another
//! construct from any loaded layer (by keyword or name). Shapes are resolved
//! transitively, so a `crm.layer` can define constructs on top of `ddd.layer`
//! which is itself defined on top of the core shapes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The core parse shapes. Every construct resolves to exactly one of these.
/// This is the ONLY vocabulary the parser understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    /// Block of child constructs and groups.
    Mod,
    /// Named type with fields.
    Struct,
    /// Named set of variants, optionally with transitions (A -> B).
    Enum,
    /// Interface with method signatures.
    Trait,
    /// Implementation binding to a trait (`kw Name for Target`).
    Impl,
    /// Flow with inputs and steps.
    Fn,
    /// Visual grouping — organizational container.
    Group,
}

impl Shape {
    pub fn from_name(s: &str) -> Option<Shape> {
        match s {
            "mod" => Some(Shape::Mod),
            "struct" => Some(Shape::Struct),
            "enum" => Some(Shape::Enum),
            "trait" => Some(Shape::Trait),
            "impl" => Some(Shape::Impl),
            "fn" => Some(Shape::Fn),
            "group" => Some(Shape::Group),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Shape::Mod => "mod",
            Shape::Struct => "struct",
            Shape::Enum => "enum",
            Shape::Trait => "trait",
            Shape::Impl => "impl",
            Shape::Fn => "fn",
            Shape::Group => "group",
        }
    }
}

/// The core statement shapes for layer-defined statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StmtShape {
    /// `kw Target(.method)? (args...)` or `kw Target{named: args}` — an invocation.
    Call,
    /// `kw <condition expr> (, "message")?` — a conditional check.
    If,
    /// `result = kw args` — invocation whose return value is bound (usage-level).
    Assign,
    /// `kw args do ... end` / indented body — statement with a body block.
    Block,
    /// Infix operator form (`expr |> expr`); also flagged via `is_infix`.
    Infix,
}

impl StmtShape {
    pub fn from_name(s: &str) -> Option<StmtShape> {
        match s {
            "call" => Some(StmtShape::Call),
            "if" => Some(StmtShape::If),
            "assign" => Some(StmtShape::Assign),
            "block" => Some(StmtShape::Block),
            "infix" => Some(StmtShape::Infix),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            StmtShape::Call => "call",
            StmtShape::If => "if",
            StmtShape::Assign => "assign",
            StmtShape::Block => "block",
            StmtShape::Infix => "infix",
        }
    }
}

/// Meta-type for step-type construct `has` fields.
/// Drives context-aware property editors in the viewer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldMeta {
    /// Plain value field (text input, checkbox, etc.)
    Plain { type_hint: String },
    /// Pick any callable target (trait method, free fn) in scope.
    Callable,
    /// Pick a construct by shape (optionally filtered by subkind).
    Construct { shape: String },
    /// Pick a method from the construct selected in another field.
    MethodOf { source_field: String },
    /// Auto-generate param inputs from the method selected in another field.
    ParamsOf { source_field: String },
    /// Pick a type defined in scope.
    TypeRef,
    /// Call an exposed operation on a dependency service to populate a select.
    /// `operation` is a dotted path like `relay.ListIntegrations`.
    /// `depends_on` optionally names another field whose value is passed as
    /// input to the query (cascading selects).
    ServiceQuery {
        operation: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depends_on: Option<String>,
    },
    /// Render a dynamic form from the schema of the item selected in another field.
    /// The source field should reference a ServiceQuery-populated value whose
    /// response includes parameter/schema metadata.
    SchemaOf { source_field: String },
}

/// A field declared in a step-type construct's `has` block with meta-type info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFieldSpec {
    pub name: String,
    pub meta: FieldMeta,
    /// Optional label override (from field_hints). If empty, use field name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Optional filter hint (from field_hints). E.g. "subkind:Repository".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filter: String,
    /// Optional editor hint (from field_hints). E.g. "rule_builder".
    /// Tells the IDE to render this field with a specialized editor widget.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub editor: String,
}

impl FieldMeta {
    /// Parse a type string from a `has` block into a FieldMeta.
    /// Recognizes: Callable, MethodOf<field>, ParamsOf<field>, Construct<shape>,
    /// TypeRef, ServiceQuery<pkg.Op>, ServiceQuery<pkg.Op, field>, SchemaOf<field>
    /// Everything else is Plain.
    pub fn parse(type_str: &str) -> FieldMeta {
        let s = type_str.trim();
        if s == "Callable" {
            return FieldMeta::Callable;
        }
        if s == "TypeRef" {
            return FieldMeta::TypeRef;
        }
        if let Some(inner) = s.strip_prefix("MethodOf<").and_then(|r| r.strip_suffix('>')) {
            return FieldMeta::MethodOf { source_field: inner.trim().to_string() };
        }
        if let Some(inner) = s.strip_prefix("ParamsOf<").and_then(|r| r.strip_suffix('>')) {
            return FieldMeta::ParamsOf { source_field: inner.trim().to_string() };
        }
        if let Some(inner) = s.strip_prefix("Construct<").and_then(|r| r.strip_suffix('>')) {
            return FieldMeta::Construct { shape: inner.trim().to_string() };
        }
        // ServiceQuery<pkg.Operation> or ServiceQuery<pkg.Operation, depends_on_field>
        if let Some(inner) = s.strip_prefix("ServiceQuery<").and_then(|r| r.strip_suffix('>')) {
            if let Some((op, dep)) = inner.split_once(',') {
                return FieldMeta::ServiceQuery {
                    operation: op.trim().to_string(),
                    depends_on: Some(dep.trim().to_string()),
                };
            }
            return FieldMeta::ServiceQuery {
                operation: inner.trim().to_string(),
                depends_on: None,
            };
        }
        if let Some(inner) = s.strip_prefix("SchemaOf<").and_then(|r| r.strip_suffix('>')) {
            return FieldMeta::SchemaOf { source_field: inner.trim().to_string() };
        }
        FieldMeta::Plain { type_hint: s.to_string() }
    }
}

/// Visual metadata for a construct or statement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Visual {
    pub icon: String,
    pub color: String,
    pub label: String,
}

/// A construct definition loaded from a `.layer` file (or the built-ins).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstructSpec {
    /// Construct name, e.g. "Aggregate". Used as the IR subkind.
    pub name: String,
    /// Source keyword, e.g. "agg". Falls back to `name` when omitted.
    pub keyword: String,
    /// Raw maps_to value as written in the layer file.
    pub maps_to: String,
    /// Resolved core shape (transitively through stacked layers).
    pub shape: Shape,
    /// Which layer defined this construct.
    pub layer: String,
    pub desc: String,
    /// Raw `contains` entries (construct names, `fn[]`, `step[]`, `group x`, `root: struct`).
    pub contains: Vec<String>,
    /// Named sub-blocks this construct may contain, from `contains` entries
    /// of the form `keyword: shape` (e.g. `root: struct`, `state: enum`).
    pub blocks: Vec<(String, Shape)>,
    /// Keywords that expect a raw string literal (e.g. `template`, `style`).
    /// Declared in the layer as `keyword: raw` in the `has` block.
    pub raw_block_keywords: Vec<String>,
    pub constraints: Vec<String>,
    pub allowed_in: String,
    pub group: String,
    pub visual: Visual,
    /// Optional runtime binding: an fn-shaped construct whose steps are NOT
    /// inlined but packaged and delegated to a layer-declared coordinator
    /// function. `runtime.0` is the coordinator fn name; `runtime.1` maps each
    /// step sub-block keyword to the trait method it fills (e.g.
    /// `compensate -> compensate`). When set, codegen lowers each step into a
    /// generated `impl <StepTrait>` and calls the coordinator with the list.
    #[serde(default)]
    pub runtime: Option<RuntimeBinding>,
    /// Annotations this construct supports, declared in the layer's
    /// `annotations` sub-block. The viewer offers these in the property editor;
    /// no annotation vocabulary is hardcoded in the viewer.
    #[serde(default)]
    /// Whether constructs of this kind are deployment unit boundaries.
    pub au: bool,
    /// Whether this construct appears as a typed step inside fn bodies.
    /// Set when `maps_to` is `"step"`. The parser recognizes the keyword
    /// contextually within fn-shaped constructs instead of at top level.
    #[serde(default)]
    pub is_step: bool,
    /// Structured field specs for step-type constructs (from `has` block).
    /// Carries meta-type info for context-aware property editors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub step_fields: Vec<StepFieldSpec>,
    pub annotations: Vec<AnnotationSpec>,
    /// Target construct name (for impl-shaped constructs): the trait-shaped
    /// construct this implements. Declared as `tgt Port` in the layer file.
    /// The viewer shows a "Create <label>" button on the target construct.
    #[serde(default)]
    pub tgt: String,
    /// Default group placement (for impl-shaped constructs): the group name
    /// where implementations should be created. Declared as `dg infrastructure`.
    #[serde(default)]
    pub dg: String,
    /// Layer-driven IDE presentation (`present` block). See `docs/PRESENTATION.md`.
    #[serde(default)]
    pub presentation: crate::presentation::ConstructPresentation,
    /// INV-001 construct roles (e.g. `http_endpoint`, `deps_bundle`, `compose`).
    /// Engine matches these — never the keyword spelling (`endpoint`, `ctx`, …).
    #[serde(default)]
    pub roles: Vec<String>,
    /// `has` field names that are config/protocol keys (not domain types).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_keys: Vec<String>,
}

/// Runtime binding for a delegated fn-shaped construct (e.g. `saga`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeBinding {
    /// The coordinator function to call (e.g. "run_saga").
    pub coordinator: String,
    /// The trait each step is lowered into an impl of (e.g. "SagaStep").
    pub step_trait: String,
    /// Maps a step's main body + its sub-blocks to trait methods. The main body
    /// fills `action` by convention; entries here map sub-block keywords to
    /// method names, e.g. `("compensate", "compensate")`.
    pub method_map: Vec<(String, String)>,
}

/// A layer-declared annotation available on a construct, with optional params.
/// Grammar in a `.layer` construct's `annotations` block:
///   annotations
///     invariant: "Domain constraint" expr
///     retry: "Retry on failure" attempts, backoff
///     dep: "Injected dependency" field role:dependency
///
/// `role:X` tokens are **roles** (INV-001), not viewer params — engine policy
/// keys off roles (e.g. `dependency`), never hard-coded annotation names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationSpec {
    pub name: String,
    pub desc: String,
    /// Parameter names (rendered as free-text inputs by the viewer).
    pub params: Vec<String>,
    /// Policy roles (e.g. `dependency`, `provider`, `main`). INV-001.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// A statement definition loaded from a `.layer` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatementSpec {
    /// Source keyword, e.g. "dispatch" or "|>" for operator keywords.
    pub keyword: String,
    /// Raw maps_to value.
    pub maps_to: String,
    /// Resolved core statement shape.
    pub shape: StmtShape,
    /// If maps_to is `Port.method`, this is the port target name.
    pub port_target: Option<String>,
    /// If maps_to is `Port.method`, this is the method name.
    pub port_method: Option<String>,
    /// Whether this is an infix operator keyword (like |>).
    /// Infix operators appear BETWEEN expressions: `expr |> expr`
    pub is_infix: bool,
    /// Port/trait type the enclosing construct must provide via `@dep`
    /// (or that is auto-available as a routing trait). Empty = no check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_dep: Option<String>,
    /// Per-target lowering templates (e.g. `"rust"` / `"typescript"` → template).
    /// Variables: `{args}`, `{arg0}`…, `{dep}`, `{self}`, `{named.key}`, `{body}`.
    /// When empty for a target, codegen falls back to Port.method / shape defaults.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub lowers_to: HashMap<String, String>,
    pub layer: String,
    pub desc: String,
    pub semantics: String,
    pub visual: Visual,
}

// ─── Codegen Template Types ──────────────────────────────────────────────────

/// A codegen template block declared in a `.layer` file.
/// Each template targets a language and matches IR patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodegenTemplate {
    /// Target language (e.g., "rust", "typescript", "swift", "kotlin")
    pub target: String,
    /// Layer that defined this template
    pub layer: String,
    /// Match rules and their emit bodies
    pub rules: Vec<CodegenRule>,
    /// Static scaffold files emitted unconditionally for this target.
    #[serde(default)]
    pub scaffold: Vec<ScaffoldFile>,
}

/// A static file emitted as project scaffolding (package.json, config, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldFile {
    /// Output path (relative to gen root).
    pub path: String,
    /// File content.
    pub content: String,
}

/// A single match/emit rule within a codegen block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodegenRule {
    /// The shape to match (e.g., "struct", "fn", "impl", "trait")
    pub match_shape: String,
    /// The where condition (e.g., "has_annotation(\"dep\")")
    pub condition: String,
    /// The template body to emit
    pub emit_body: String,
    /// Optional named section to emit into (for composition)
    pub emit_to: Option<String>,
    /// Optional file path pattern to emit to (e.g., "src/routes/{{route}}/+page.svelte")
    pub emit_file: Option<String>,
    /// Priority for section ordering (lower = earlier, default 100)
    pub priority: u32,
}

// ─── LayerRegistry ───────────────────────────────────────────────────────────

/// Identity / FK edge policy (INV-006). Default: no `*_id` inference.
#[derive(Debug, Clone, Default)]
pub struct IdentityPolicy {
    /// When set (e.g. `"_id"`), fields ending with this suffix become References edges.
    pub ref_suffix: Option<String>,
    /// Canonical identity field name (e.g. `"id"`) for equality_by_value checks.
    pub identity_field: Option<String>,
}

/// Target constructor / field-default policy (INV-002).
/// Declared in target layers (e.g. `rust.layer`); not hardcoded in backends.
#[derive(Debug, Clone, Default)]
pub struct ConstructorPolicy {
    /// Field names auto-filled (timestamps, etc.) rather than constructor params.
    pub auto_fields: Vec<String>,
    /// Named type → default Rust expression (e.g. Int → "0").
    pub type_defaults: Vec<(String, String)>,
}

/// UI-framework reactivity emission forms (MISSION: framework APIs in layers).
/// Declared by `svelte5.layer` (or similar) — engine never hardcodes `$state` / `$derived`.
///
/// Placeholders in templates:
/// - `state_line`: `{name}` `{type}` `{default}`
/// - `derived_line`: `{name}` `{expr}` (prefer value form, not arrow — objects stay valid)
/// - `effect_sync` / `effect_async`: `{name}` `{body}`
/// - `props_call`: no placeholders (e.g. `$props()`)
/// - `bindable` / `bindable_default`: `{default}` for the latter
#[derive(Debug, Clone, Default)]
pub struct ReactivityPolicy {
    /// e.g. `$props()`
    pub props_call: String,
    /// e.g. `let {name} = $state<{type}>({default});`
    pub state_line: String,
    /// e.g. `let {name} = $derived({expr});` — value form, not `$derived(() => …)`
    pub derived_line: String,
    /// e.g. `$effect(() => { // {name}\n{body}\n  });`
    pub effect_sync: String,
    /// e.g. `$effect(() => { // {name}\n    void (async () => {\n{body}\n    })();\n  });`
    pub effect_async: String,
    /// e.g. `$bindable()`
    pub bindable: String,
    /// e.g. `$bindable({default})`
    pub bindable_default: String,
}

impl ReactivityPolicy {
    pub fn is_empty(&self) -> bool {
        self.state_line.is_empty() && self.props_call.is_empty()
    }

    pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
        let mut out = template.to_string();
        for (k, v) in vars {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out
    }
}

/// Layer-declared PR Wizard review presentation (Track B).
/// Parsed from a top-level `review` block in a `.layer` file:
/// ```text
/// review
///   strategy component_sandbox
///   target svelte5
///   fallback structural
///   secondary file_diff
///   impact dependents
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewPolicy {
    /// Primary walk strategy: structural | component_sandbox | file_diff
    #[serde(default)]
    pub strategy: String,
    /// Optional sandbox / renderer target (e.g. svelte5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// When primary cannot render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// Secondary panels (e.g. file_diff).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary: Vec<String>,
    /// Impact dimensions (e.g. dependents).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub impact: Vec<String>,
}

impl ReviewPolicy {
    pub fn is_empty(&self) -> bool {
        self.strategy.is_empty()
    }

    /// Default structural presentation when a layer has no `review` block.
    pub fn structural_default() -> Self {
        Self {
            strategy: "structural".into(),
            target: None,
            fallback: Some("structural".into()),
            secondary: vec!["file_diff".into()],
            impact: vec!["dependents".into()],
        }
    }
}

impl ConstructorPolicy {
    /// Built-in Rust defaults — used only until a target layer overrides.
    /// Living here (layer policy) rather than in rust.rs (INV-002).
    pub fn rust_defaults() -> Self {
        Self {
            auto_fields: vec![
                "created".into(),
                "updated".into(),
                "created_at".into(),
                "updated_at".into(),
                "created_on".into(),
                "updated_on".into(),
                "deleted_on".into(),
                "date_joined".into(),
            ],
            type_defaults: vec![
                ("Int".into(), "0".into()),
                ("Bool".into(), "false".into()),
                ("F64".into(), "0.0".into()),
                ("Json".into(), "serde_json::json!({})".into()),
            ],
        }
    }

    pub fn is_auto_field(&self, name: &str) -> bool {
        self.auto_fields.iter().any(|f| f == name)
    }

    pub fn type_default(&self, type_name: &str) -> Option<&str> {
        self.type_defaults
            .iter()
            .find(|(t, _)| t == type_name)
            .map(|(_, e)| e.as_str())
    }
}

/// The resolved vocabulary for a compilation: built-in core constructs plus
/// everything from the loaded (possibly stacked) layers.
pub struct LayerRegistry {
    pub constructs: Vec<ConstructSpec>,
    pub statements: Vec<StatementSpec>,
    /// Names of layers loaded (in load order).
    pub layers: Vec<String>,
    /// Raw VEIL source blocks to inject into solutions using this registry.
    pub declarations: Vec<String>,
    /// LLM prompt sections from layers — concatenated for RAG context.
    /// Each entry is (layer_name, prompt_text).
    pub prompts: Vec<(String, String)>,
    /// Codegen templates declared by loaded layers.
    pub codegen_templates: Vec<CodegenTemplate>,
    /// Loaded third-party crate stubs.
    pub stubs: Vec<StubCrate>,
    /// External layer resolver — called when a layer isn't found locally or in system.
    /// Provided by the hosting runtime (e.g. veil-runtime for database-backed resolution).
    pub external_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    /// External package source resolver — resolves `use X` package .veil content
    /// when not found on filesystem (DDB/S3 in deployed environments).
    pub source_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    /// Smart-constructor / field-default policy (INV-002). Filled from target layers.
    pub constructor_policy: ConstructorPolicy,
    /// UI framework reactivity forms (Svelte runes, etc.). From layers only.
    pub reactivity_policy: ReactivityPolicy,
    /// Per-layer PR Wizard review presentation (`review` blocks). Key = layer name.
    pub review_policies: HashMap<String, ReviewPolicy>,
    /// Identity / FK inference policy (INV-006). Default off.
    pub identity_policy: IdentityPolicy,
    /// Bus / handler name policy from layers (optional strip prefix). Default: no strip.
    pub bus_policy: BusPolicy,
    /// Auth service trait name for local AllowAllAuth (RT-008). Default: none.
    pub auth_policy: AuthPolicy,
    /// Name-derived REST verb/path prefixes. Default empty = no name-derived REST.
    pub http_name_policy: HttpNamePolicy,
    /// Declared local-harness knobs (layers + `veil.toml` `[harness]`).
    /// Codegen does not emit from this yet.
    pub harness_policy: crate::harness::HarnessPolicy,
    /// Extra product roots from `veil.toml` `[dependencies]` (R20).
    /// Each root may contain `layers/<name>.layer` or `<name>.layer`.
    pub extra_layer_roots: Vec<std::path::PathBuf>,
    /// True when `veil.toml` `[codegen] http_*` was present (warn after flip).
    pub codegen_http_from_toml: bool,
}

/// Layer-declared bus message naming (no hard-coded `Handle` in the engine).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BusPolicy {
    /// If set, strip this prefix from construct names when exporting bus message types.
    /// Example: `strip_name_prefix Handle` → `HandleGetUser` publishes as `GetUser`.
    #[serde(default)]
    pub strip_name_prefix: Option<String>,
}

/// Which trait name triggers local AllowAllAuth emission (layer-configured).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthPolicy {
    #[serde(default)]
    pub service_trait: Option<String>,
}

/// Name-derived REST routes when no role:http_route annotation is present.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HttpNamePolicy {
    #[serde(default)]
    pub list_prefix: Option<String>,
    #[serde(default)]
    pub get_prefix: Option<String>,
    #[serde(default)]
    pub create_prefix: Option<String>,
    #[serde(default)]
    pub update_prefix: Option<String>,
    #[serde(default)]
    pub delete_prefix: Option<String>,
    /// Path root for derived routes (e.g. `/api/`).
    #[serde(default)]
    pub path_prefix: Option<String>,
}

impl Default for LayerRegistry {
    fn default() -> Self {
        Self {
            constructs: Vec::new(),
            statements: Vec::new(),
            layers: Vec::new(),
            declarations: Vec::new(),
            prompts: Vec::new(),
            codegen_templates: Vec::new(),
            stubs: Vec::new(),
            external_resolver: None,
            source_resolver: None,
            constructor_policy: ConstructorPolicy::default(),
            reactivity_policy: ReactivityPolicy::default(),
            review_policies: HashMap::new(),
            identity_policy: IdentityPolicy::default(),
            bus_policy: BusPolicy::default(),
            auth_policy: AuthPolicy::default(),
            http_name_policy: HttpNamePolicy::default(),
            harness_policy: crate::harness::HarnessPolicy::documented_defaults(),
            extra_layer_roots: Vec::new(),
            codegen_http_from_toml: false,
        }
    }
}

impl Clone for LayerRegistry {
    fn clone(&self) -> Self {
        Self {
            constructs: self.constructs.clone(),
            statements: self.statements.clone(),
            layers: self.layers.clone(),
            declarations: self.declarations.clone(),
            prompts: self.prompts.clone(),
            codegen_templates: self.codegen_templates.clone(),
            stubs: self.stubs.clone(),
            external_resolver: None, // resolver is not cloneable — cleared on clone
            source_resolver: None, // resolver is not cloneable — cleared on clone
            constructor_policy: self.constructor_policy.clone(),
            reactivity_policy: self.reactivity_policy.clone(),
            review_policies: self.review_policies.clone(),
            identity_policy: self.identity_policy.clone(),
            bus_policy: self.bus_policy.clone(),
            auth_policy: self.auth_policy.clone(),
            http_name_policy: self.http_name_policy.clone(),
            harness_policy: self.harness_policy.clone(),
            extra_layer_roots: self.extra_layer_roots.clone(),
            codegen_http_from_toml: self.codegen_http_from_toml,
        }
    }
}

impl std::fmt::Debug for LayerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerRegistry")
            .field("constructs", &self.constructs.len())
            .field("statements", &self.statements.len())
            .field("layers", &self.layers)
            .field("declarations", &self.declarations.len())
            .field("stubs", &self.stubs.len())
            .field("external_resolver", &self.external_resolver.is_some())
            .field("source_resolver", &self.source_resolver.is_some())
            .finish()
    }
}

impl LayerRegistry {
    /// Registry with only the core language built-ins.
    pub fn builtin() -> Self {
        let mut reg = LayerRegistry::default();
        let core = [
            ("mod", "Module", Shape::Mod, "📦", "#8b5cf6", "Module", "none"),
            ("struct", "Struct", Shape::Struct, "📋", "#14b8a6", "Struct", "any"),
            ("enum", "Enum", Shape::Enum, "🔀", "#8b5cf6", "Enum", "any"),
            ("trait", "Trait", Shape::Trait, "🔌", "#10b981", "Trait", "any"),
            ("impl", "Impl", Shape::Impl, "🔗", "#a855f7", "Implementation", "any"),
            ("fn", "Fn", Shape::Fn, "⚡", "#f97316", "Function", "any"),
            ("flow", "Flow", Shape::Fn, "🌊", "#f97316", "Flow", "none"),
            ("group", "Group", Shape::Group, "📂", "#475569", "Group", "mod"),
            ("step", "Step", Shape::Fn, "▶", "#3b82f6", "Step", "Flow, InterfaceMethod"),
        ];
        for (kw, name, shape, icon, color, label, allowed) in core {
            reg.constructs.push(ConstructSpec {
                name: name.to_string(),
                keyword: kw.to_string(),
                maps_to: shape.name().to_string(),
                shape,
                layer: "core".to_string(),
                desc: String::new(),
                contains: Vec::new(),
                blocks: Vec::new(),
                raw_block_keywords: Vec::new(),
                constraints: Vec::new(),
                allowed_in: allowed.to_string(),
                group: String::new(),
                visual: Visual {
                    icon: icon.to_string(),
                    color: color.to_string(),
                    label: label.to_string(),
                },
                au: false,
                is_step: false,
                step_fields: Vec::new(),
                annotations: Vec::new(),
                runtime: None,
                tgt: String::new(),
                dg: String::new(),
                presentation: Default::default(),
                roles: Vec::new(),
                config_keys: Vec::new(),
            });
        }
        reg.layers.push("core".to_string());
        reg
    }

    /// Look up a construct by its source keyword.
    pub fn construct(&self, keyword: &str) -> Option<&ConstructSpec> {
        self.constructs.iter().find(|c| c.keyword == keyword)
    }

    /// Look up a construct by its name (e.g. "Aggregate").
    pub fn construct_by_name(&self, name: &str) -> Option<&ConstructSpec> {
        self.constructs.iter().find(|c| c.name == name)
    }

    /// Whether any layer-declared annotation named `name` carries policy role `role`.
    /// INV-001: engine keys off roles, not hard-coded annotation strings.
    pub fn annotation_has_role(&self, name: &str, role: &str) -> bool {
        self.constructs.iter().any(|c| {
            c.annotations
                .iter()
                .any(|a| a.name == name && a.roles.iter().any(|r| r == role))
        })
    }

    /// True if `name` is a dependency-injection annotation (`role:dependency`).
    pub fn is_dependency_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "dependency")
    }

    /// Field/input is an injected dependency per layer policy (INV-001).
    pub fn field_is_dependency(&self, field: &crate::ast::Field) -> bool {
        field
            .annotations
            .iter()
            .any(|a| self.is_dependency_annotation(&a.name))
    }

    /// Annotation name carries `role:secret` (INV-001 — never hardcode `"secret"`).
    pub fn is_secret_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "secret")
    }

    /// Field is a secret (omit from serialization) per layer policy.
    pub fn field_is_secret(&self, field: &crate::ast::Field) -> bool {
        field
            .annotations
            .iter()
            .any(|a| self.is_secret_annotation(&a.name))
    }

    /// Annotation carries `role:http_route` (REST surface for dual-loop harness).
    pub fn is_http_route_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "http_route")
    }

    /// Construct has an HTTP route annotation (any name with role:http_route).
    pub fn construct_has_http_route(&self, c: &crate::ast::Construct) -> bool {
        c.annotations
            .iter()
            .any(|a| self.is_http_route_annotation(&a.name))
    }

    /// First HTTP-route annotation on a construct (for method/path args).
    pub fn http_route_annotation<'a>(
        &self,
        c: &'a crate::ast::Construct,
    ) -> Option<&'a crate::ast::Annotation> {
        c.annotations
            .iter()
            .find(|a| self.is_http_route_annotation(&a.name))
    }

    /// Annotation carries `role:ui_route` (Svelte page/layout `@route` path).
    /// Distinct from `role:http_route` (removed API harness surface).
    pub fn is_ui_route_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "ui_route")
    }

    /// First UI-route annotation on a page/layout (svelte5 `ann route`, role:ui_route).
    pub fn ui_route_annotation<'a>(
        &self,
        c: &'a crate::ast::Construct,
    ) -> Option<&'a crate::ast::Annotation> {
        c.annotations
            .iter()
            .find(|a| self.is_ui_route_annotation(&a.name))
    }

    /// Quoted-stripped UI path (`/pulls/[id]`). Prefer this over `http_route_annotation`
    /// for page/layout constructs.
    pub fn ui_route_path(&self, c: &crate::ast::Construct) -> Option<String> {
        self.ui_route_annotation(c)
            .and_then(|a| a.args.first())
            .map(|s| strip_annotation_arg_quotes(s))
    }

    /// Apply layer bus_policy strip to a construct/fn name for message keys.
    pub fn bus_message_name(&self, construct_name: &str) -> String {
        if let Some(prefix) = &self.bus_policy.strip_name_prefix {
            if !prefix.is_empty() {
                if let Some(rest) = construct_name.strip_prefix(prefix.as_str()) {
                    if !rest.is_empty() {
                        return rest.to_string();
                    }
                }
            }
        }
        construct_name.to_string()
    }

    // ── INV-001 role helpers (never hardcode annotation names in backends) ──

    pub fn is_main_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "main")
    }

    pub fn construct_has_main(&self, c: &crate::ast::Construct) -> bool {
        c.annotations
            .iter()
            .any(|a| self.is_main_annotation(&a.name))
    }

    pub fn is_adapter_field_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "adapter_field")
    }

    pub fn is_adapter_env_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "adapter_env")
    }

    pub fn is_invariant_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "invariant")
    }

    pub fn construct_has_invariant(&self, c: &crate::ast::Construct) -> bool {
        c.annotations
            .iter()
            .any(|a| self.is_invariant_annotation(&a.name))
    }

    pub fn is_shared_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "shared")
    }

    pub fn field_is_shared(&self, field: &crate::ast::Field) -> bool {
        field
            .annotations
            .iter()
            .any(|a| self.is_shared_annotation(&a.name))
    }

    pub fn is_runtime_strategy_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "runtime_strategy")
    }

    pub fn is_provider_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "provider")
    }

    /// Annotation carries `role:permission` (required claim for a service).
    pub fn is_permission_annotation(&self, name: &str) -> bool {
        self.annotation_has_role(name, "permission")
    }

    /// First permission annotation on a construct (e.g. `@auth("relay.admin")`).
    pub fn permission_annotation<'a>(
        &self,
        c: &'a crate::ast::Construct,
    ) -> Option<&'a crate::ast::Annotation> {
        c.annotations
            .iter()
            .find(|a| self.is_permission_annotation(&a.name))
    }

    /// Whether this trait name is the configured local auth service trait.
    pub fn is_auth_service_trait(&self, trait_name: &str) -> bool {
        self.auth_policy
            .service_trait
            .as_ref()
            .is_some_and(|t| t == trait_name)
    }

    /// Apply product `[codegen]` overrides from `veil.toml` (INV-001).
    ///
    /// Merge order: **builtin → layers → veil.toml**. Present keys override;
    /// empty / `"none"` clears optional fields so products can disable layer
    /// defaults without forking layers.
    pub fn apply_codegen_overrides(&mut self, o: &crate::deps::CodegenToml) {
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.bus_strip_prefix) {
            self.bus_policy.strip_name_prefix = v;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.auth_service_trait) {
            self.auth_policy.service_trait = v;
        }
        if o.http_path_prefix.is_some()
            || o.http_list_prefix.is_some()
            || o.http_get_prefix.is_some()
            || o.http_create_prefix.is_some()
            || o.http_update_prefix.is_some()
            || o.http_delete_prefix.is_some()
        {
            self.codegen_http_from_toml = true;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.http_path_prefix) {
            self.http_name_policy.path_prefix = v;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.http_list_prefix) {
            self.http_name_policy.list_prefix = v;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.http_get_prefix) {
            self.http_name_policy.get_prefix = v;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.http_create_prefix) {
            self.http_name_policy.create_prefix = v;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.http_update_prefix) {
            self.http_name_policy.update_prefix = v;
        }
        if let Some(v) = crate::deps::CodegenToml::normalize_opt(&o.http_delete_prefix) {
            self.http_name_policy.delete_prefix = v;
        }
    }

    /// Apply product `[harness]` overrides from `veil.toml`.
    ///
    /// Merge order: **documented defaults → layers → veil.toml**.
    pub fn apply_harness_overrides(&mut self, o: &crate::deps::HarnessToml) {
        let overlay = o.to_policy();
        self.harness_policy = crate::harness::merge_harness_policy(&self.harness_policy, &overlay);
    }

    /// Whether a construct's **layer spec** carries `role`.
    /// Matches keyword or construct name (subkind). Never matches DDD spellings.
    pub fn construct_has_role(&self, c: &crate::ast::Construct, role: &str) -> bool {
        self.spec_for_construct(c)
            .map(|spec| spec.roles.iter().any(|r| r == role))
            .unwrap_or(false)
    }

    /// Layer spec for an authored construct (keyword, then name/subkind).
    pub fn spec_for_construct(&self, c: &crate::ast::Construct) -> Option<&ConstructSpec> {
        self.construct(&c.keyword)
            .or_else(|| self.construct_by_name(&c.subkind))
            .or_else(|| self.construct_by_name(&c.keyword))
    }

    /// Config/protocol field names on this construct's spec (`has` keys).
    pub fn construct_config_keys(&self, c: &crate::ast::Construct) -> &[String] {
        self.spec_for_construct(c)
            .map(|s| s.config_keys.as_slice())
            .unwrap_or(&[])
    }

    /// All constructs in `sol` whose spec has `role`.
    pub fn constructs_with_role<'a>(
        &'a self,
        sol: &'a crate::ast::Solution,
        role: &str,
    ) -> Vec<&'a crate::ast::Construct> {
        let mut out = Vec::new();
        for item in &sol.items {
            if let crate::ast::TopLevelItem::Construct(c) = item {
                collect_constructs_with_role(self, c, role, &mut out);
            }
        }
        out
    }

    /// Look up a statement by its source keyword.
    pub fn statement(&self, keyword: &str) -> Option<&StatementSpec> {
        self.statements.iter().find(|s| s.keyword == keyword)
    }

    /// Look up a step-type construct by its source keyword.
    /// These are constructs with `maps_to: "step"` that appear as typed steps
    /// inside fn bodies.
    pub fn step_construct(&self, keyword: &str) -> Option<&ConstructSpec> {
        self.constructs.iter().find(|c| c.keyword == keyword && c.is_step)
    }

    /// Find an infix operator statement that matches a token text sequence.
    /// E.g., for tokens `|` `>`, checks if any statement has keyword `|>`.
    pub fn infix_operator(&self, token_text: &str) -> Option<&StatementSpec> {
        self.statements.iter().find(|s| s.is_infix && s.keyword == token_text)
    }

    /// Get all infix operator statements.
    pub fn infix_operators(&self) -> Vec<&StatementSpec> {
        self.statements.iter().filter(|s| s.is_infix).collect()
    }

    /// Get the names of traits used as message-routing ports by layer statements.
    /// These are the traits that statements target via `maps_to Port.method`.
    /// Envelope-routing modules keep only these as direct deps; other cross-boundary
    /// calls route through them.
    pub fn routing_traits(&self) -> Vec<String> {
        let mut names: Vec<String> = self.statements.iter()
            .filter_map(|s| s.port_target.as_ref())
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Is-a check through the maps_to chain: a construct "is" another when
    /// its maps_to chain passes through it (by name or keyword). Stacked
    /// constructs inherit the identity of what they build on — e.g. a
    /// crm `Playbook` (playbook -> saga) IS-A ddd `Saga`.
    pub fn is_a(&self, keyword: &str, ancestor: &str) -> bool {
        let mut current = match self.construct(keyword) {
            Some(spec) => spec,
            None => return false,
        };
        let mut visited: HashSet<&str> = HashSet::new();
        loop {
            if current.name == ancestor || current.keyword == ancestor {
                return true;
            }
            if !visited.insert(&current.keyword) {
                return false; // cycle guard
            }
            let next = self
                .constructs
                .iter()
                .find(|c| c.keyword == current.maps_to || c.name == current.maps_to);
            match next {
                Some(spec) if spec.keyword != current.keyword => current = spec,
                _ => return false,
            }
        }
    }

    /// Load a layer file (and, recursively, layers it `use`s) into this registry.
    ///
    /// Resolution:
    /// - **Platform names** (`ddd`, `di`, …): platform catalog only (read-only to products)
    /// - **Product names**: package `layers/`, product root, `[dependencies]`, optional disk-hub siblings
    pub fn load_layer(&mut self, name: &str, dir: &Path) -> Result<(), String> {
        if self.layers.iter().any(|l| l == name) {
            return Ok(()); // already loaded
        }

        let content = self.resolve_layer_content(name, dir)?;

        if crate::platform_layers::is_platform_layer_name(name)
            && crate::platform_layers::is_ghost_layer_content(&content)
        {
            return Err(format!(
                "platform layer '{name}' resolved to empty/ghost content — \
                 check VEIL_LAYERS_DIR or seed platform layers (scripts/seed-layers-platform.sh)"
            ));
        }

        // First, load dependency layers (`use xxx` lines at pkg level).
        // Skip silently if not found — it might be a .stub or package reference.
        for line in content.lines() {
            let t = line.trim();
            if let Some(dep) = t.strip_prefix("use ") {
                let dep = dep.split_whitespace().next().unwrap_or("").trim();
                if !dep.is_empty() {
                    let _ = self.load_layer(dep, dir);
                }
            }
        }

        let raw = parse_layer_file(&content, name)
            .map_err(|e| format!("layer '{}': {}", name, e))?;
        // Only mark loaded after successful parse — avoids ghost "ddd" with no constructs.
        self.layers.push(name.to_string());
        if let Err(e) = self.merge_and_resolve(raw) {
            self.layers.retain(|l| l != name);
            return Err(e);
        }
        // INV-002 / INV-006: same policy install as load_content (load_layer is the
        // normal path for package `use` lines; without this, identity_policy never
        // reaches the IR builder).
        if let Some(pol) = parse_constructor_policy(&content) {
            self.constructor_policy = pol;
        } else if name == "rust" && self.constructor_policy.auto_fields.is_empty() {
            self.constructor_policy = ConstructorPolicy::rust_defaults();
        }
        if let Some(rp) = parse_reactivity_policy(&content) {
            self.reactivity_policy = rp;
        }
        if let Some(rev) = parse_review_policy(&content) {
            self.review_policies.insert(name.to_string(), rev);
        }
        if let Some(id_pol) = parse_identity_policy(&content) {
            self.identity_policy = id_pol;
        }
        if let Some(bus) = parse_bus_policy(&content) {
            // Later layers can override strip prefix when stacked.
            if bus.strip_name_prefix.is_some() {
                self.bus_policy.strip_name_prefix = bus.strip_name_prefix;
            }
        }
        if let Some(auth) = parse_auth_policy(&content) {
            if auth.service_trait.is_some() {
                self.auth_policy = auth;
            }
        }
        if let Some(http) = parse_http_name_policy(&content) {
            self.http_name_policy = merge_http_name_policy(&self.http_name_policy, &http);
        }
        if let Some(harness) = crate::harness::parse_harness_policy(&content) {
            self.harness_policy =
                crate::harness::merge_harness_policy(&self.harness_policy, &harness);
        }
        Ok(())
    }

    /// Resolve layer content by searching multiple locations.
    fn resolve_layer_content(&self, name: &str, local_dir: &Path) -> Result<String, String> {
        // ── Platform language (VEIL-owned, read-only for products) ──────────
        if crate::platform_layers::is_platform_layer_name(name) {
            if let Some(content) = crate::platform_layers::resolve_platform_layer_content(name) {
                return Ok(content);
            }
            if let Some(resolver) = &self.external_resolver {
                if let Some(content) = resolver(name) {
                    if !crate::platform_layers::is_ghost_layer_content(&content) {
                        return Ok(content);
                    }
                }
            }
            return Err(format!(
                "platform layer '{name}' not found (searched VEIL_LAYERS_DIR, \
                 $TMP/veil-platform-layers, install/monorepo layers). \
                 Seed with scripts/seed-layers-platform.sh or set VEIL_LAYERS_DIR."
            ));
        }

        // ── Product / userland layers ───────────────────────────────────────
        // 1. Adjacent to the .veil file
        let local_path = local_dir.join(format!("{name}.layer"));
        if local_path.is_file() {
            return std::fs::read_to_string(&local_path).map_err(|e| {
                format!("cannot read layer '{name}' at {}: {e}", local_path.display())
            });
        }
        let in_layers = local_dir.join("layers").join(format!("{name}.layer"));
        if in_layers.is_file() {
            return std::fs::read_to_string(&in_layers).map_err(|e| {
                format!("cannot read layer '{name}' at {}: {e}", in_layers.display())
            });
        }

        // 1b. Product root (veil.toml [package] provides_use / layers/<name>.layer)
        if let Some(root) = crate::deps::find_project_root(local_dir) {
            if let Some(content) = Self::load_layer_from_product_root(name, &root) {
                return Ok(content);
            }
        }

        // 1c. Declared product deps (veil.toml [dependencies]) — R20
        for root in &self.extra_layer_roots {
            if let Some(content) = Self::load_layer_from_product_root(name, root) {
                return Ok(content);
            }
        }

        // 1d. Disk-hub sibling products (opt-in: VEIL_SOURCE_MODE=disk or VEIL_LAYER_SIBLING_SCAN=1)
        if crate::platform_layers::sibling_product_layer_scan_enabled() {
            if let Some(content) = Self::load_layer_from_sibling_products(name, local_dir) {
                return Ok(content);
            }
        }

        // 2. Non-listed names may still live in the platform install (extensions)
        if let Some(content) = crate::platform_layers::resolve_platform_layer_content(name) {
            return Ok(content);
        }

        // 3. External resolver (port for veil-runtime or other backends)
        if let Some(resolver) = &self.external_resolver {
            if let Some(content) = resolver(name) {
                return Ok(content);
            }
        }

        let dep_hint = if self.extra_layer_roots.is_empty() {
            format!(
                " (no [dependencies] roots — declare e.g. {name} = {{ project = \"…\" }} in veil.toml)"
            )
        } else {
            format!(
                " (also searched {} dependency root(s))",
                self.extra_layer_roots.len()
            )
        };
        Err(format!(
            "layer '{name}' not found (searched: {}, product layers, platform catalog){}",
            local_dir.display(),
            dep_hint
        ))
    }

    /// Load primary layer for `use name` from a product root (R21 package entry).
    fn load_layer_from_product_root(name: &str, root: &Path) -> Option<String> {
        if let Some(p) = crate::deps::layer_source_in_root(root, name) {
            return std::fs::read_to_string(p).ok();
        }
        None
    }

    /// Disk hub only: sibling product dirs under each ancestor (e.g. veil-projects/*).
    fn load_layer_from_sibling_products(name: &str, dir: &Path) -> Option<String> {
        let mut cur = Some(dir);
        while let Some(d) = cur {
            if let Ok(entries) = std::fs::read_dir(d) {
                let mut kids: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                kids.sort_by_key(|e| e.file_name());
                for ent in kids {
                    let p = ent.path();
                    if !p.is_dir() {
                        continue;
                    }
                    let name_s = ent.file_name().to_string_lossy().to_string();
                    // Skip obvious non-product dirs (no language-specific package managers)
                    if name_s.starts_with('.')
                        || name_s == "target"
                        || name_s == "generated"
                        || name_s == "output"
                    {
                        continue;
                    }
                    // Only consider dirs that look like VEIL products
                    if !p.join("veil.toml").is_file() && !p.join("main.veil").is_file() {
                        continue;
                    }
                    if let Some(s) = Self::load_layer_from_product_root(name, &p) {
                        return Some(s);
                    }
                }
            }
            cur = d.parent();
        }
        None
    }

    /// Load a layer from in-memory content. Resolves `use` deps via system /
    /// ancestor layers (same as `load_layer`) so policy packs like
    /// `rest_english` apply when tests `include_str!` a dependent layer.
    pub fn load_content(&mut self, name: &str, content: &str) -> Result<(), String> {
        if self.layers.iter().any(|l| l == name) {
            return Ok(());
        }
        // Resolve `use` deps first (policy packs, foundations).
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for line in content.lines() {
            let t = line.trim();
            if let Some(dep) = t.strip_prefix("use ") {
                let dep = dep.split_whitespace().next().unwrap_or("").trim();
                if !dep.is_empty() {
                    let _ = self.load_layer(dep, &cwd);
                }
            }
        }
        self.layers.push(name.to_string());
        let raw = parse_layer_file(content, name)
            .map_err(|e| format!("layer '{}': {}", name, e))?;
        self.merge_and_resolve(raw)?;
        // INV-002: target layers may install constructor policy tables.
        if let Some(pol) = parse_constructor_policy(content) {
            self.constructor_policy = pol;
        } else if name == "rust" && self.constructor_policy.auto_fields.is_empty() {
            // rust.layer documents policy; apply canonical Rust defaults.
            self.constructor_policy = ConstructorPolicy::rust_defaults();
        }
        // Framework reactivity (Svelte runes, etc.) — never hardcoded in backends.
        if let Some(rp) = parse_reactivity_policy(content) {
            self.reactivity_policy = rp;
        }
        // PR Wizard review presentation (structural / component_sandbox / …).
        if let Some(rev) = parse_review_policy(content) {
            self.review_policies.insert(name.to_string(), rev);
        }
        // INV-006: identity / FK policy (ddd opts in; default is off).
        if let Some(id_pol) = parse_identity_policy(content) {
            self.identity_policy = id_pol;
        }
        if let Some(bus) = parse_bus_policy(content) {
            if bus.strip_name_prefix.is_some() {
                self.bus_policy.strip_name_prefix = bus.strip_name_prefix;
            }
        }
        if let Some(auth) = parse_auth_policy(content) {
            if auth.service_trait.is_some() {
                self.auth_policy = auth;
            }
        }
        if let Some(http) = parse_http_name_policy(content) {
            self.http_name_policy = merge_http_name_policy(&self.http_name_policy, &http);
        }
        if let Some(harness) = crate::harness::parse_harness_policy(content) {
            self.harness_policy =
                crate::harness::merge_harness_policy(&self.harness_policy, &harness);
        }
        Ok(())
    }

    /// Build a registry for a `.veil` file: built-ins plus every layer the
    /// file references via `use` lines. Layer resolution is transitive.
    pub fn for_veil_file(veil_path: &Path) -> Result<Self, String> {
        Self::for_veil_file_with_resolvers(veil_path, None, None)
    }

    /// Build a registry with optional external resolvers for deployed environments.
    ///
    /// - `layer_resolver`: called when a layer isn't found on disk (e.g. DDB lookup)
    /// - `pkg_source_resolver`: called to get package .veil source for cross-package deps
    pub fn for_veil_file_with_resolvers(
        veil_path: &Path,
        layer_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
        pkg_source_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    ) -> Result<Self, String> {
        let mut reg = LayerRegistry::builtin();
        reg.external_resolver = layer_resolver;
        reg.source_resolver = pkg_source_resolver;
        // R20: product deps from veil.toml feed layer search roots
        reg.extra_layer_roots = crate::deps::resolve_dependency_roots_for(veil_path);
        let dir = veil_path.parent().unwrap_or(Path::new("."));
        let content = std::fs::read_to_string(veil_path)
            .map_err(|e| format!("cannot read {}: {}", veil_path.display(), e))?;
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("use ") {
                // Parse: "use <name>" or "use <name> as <alias>"
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let name = parts.first().unwrap_or(&"");
                let alias = if parts.len() >= 3 && parts[1] == "as" {
                    Some(parts[2].to_string())
                } else {
                    None
                };

                // Try to load as a layer (searches local → system → external)
                let _ = reg.load_layer(name, dir);

                // Also check for .stub files: local → stubs/ → system (VEIL_STUBS_DIR /
                // runtime/src/stubs next to layers). Same resolution idea as layers.
                let stub_path = dir.join(format!("{}.stub", name));
                let stub_subdir_path = dir.join("stubs").join(format!("{}.stub", name));
                let found_stub = if stub_path.exists() {
                    Some(stub_path)
                } else if stub_subdir_path.exists() {
                    Some(stub_subdir_path)
                } else {
                    Self::find_system_stub(name)
                };
                if let Some(path) = found_stub {
                    if let Ok(stub_content) = std::fs::read_to_string(&path) {
                        if let Some(mut stub) = parse_stub_file(&stub_content) {
                            stub.alias = alias;
                            reg.stubs.push(stub);
                        }
                    }
                }
            }
        }
        // Auto-load every `stubs/*.stub` under the package dir so `stub_install` /
        // `stub_gen` take effect for check/codegen without requiring a matching
        // `use sqlx` / `use reqwest` line. Dedupes by crate name against use-loaded stubs.
        Self::load_project_stubs_dir(&mut reg, dir);
        // R21: product's primary layer from veil.toml `[package].layer` (or
        // layers/main.layer default). Without this, packages only get layers
        // named in `use` lines — product vocabulary/present never loads and
        // IDE tree views fall back to bare ddd.Context.
        if let Some(entry) = crate::deps::load_package_entry(dir) {
            let is_primary_veil = veil_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| {
                    entry
                        .veil
                        .file_name()
                        .and_then(|e| e.to_str())
                        .map(|e| e == n)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            if is_primary_veil {
                let _ = reg.load_layer(&entry.use_name, dir);
            }
        }
        // Product veil.toml [codegen] / [harness] win over layer policies (INV-001).
        if let Some(o) = crate::deps::load_codegen_overrides_for(veil_path) {
            reg.apply_codegen_overrides(&o);
        }
        if let Some(h) = crate::deps::load_harness_overrides_for(veil_path) {
            reg.apply_harness_overrides(&h);
        }
        Ok(reg)
    }

    /// Load all `*.stub` files from `{package_dir}/stubs/` into the registry.
    /// Skips names already present (e.g. loaded via `use sqlx`).
    fn load_project_stubs_dir(reg: &mut LayerRegistry, package_dir: &Path) {
        let stubs_dir = package_dir.join("stubs");
        if !stubs_dir.is_dir() {
            return;
        }
        let existing: std::collections::HashSet<String> = reg
            .stubs
            .iter()
            .map(|s| s.name.replace('-', "_").to_ascii_lowercase())
            .collect();
        let Ok(rd) = std::fs::read_dir(&stubs_dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("stub") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(stub) = parse_stub_file(&content) else {
                continue;
            };
            let key = stub.name.replace('-', "_").to_ascii_lowercase();
            if existing.contains(&key) {
                continue;
            }
            reg.stubs.push(stub);
        }
    }

    /// Locate a system `.stub` by package use-name (`aws_sdk_dynamodb`, `sqlx`, …).
    fn find_system_stub(name: &str) -> Option<std::path::PathBuf> {
        // Also try dashed/underscored stems (aws-sdk-s3 vs aws_sdk_s3)
        let stems: Vec<String> = {
            let mut v = vec![name.to_string()];
            let u = name.replace('-', "_");
            let d = name.replace('_', "-");
            if u != name {
                v.push(u);
            }
            if d != name && !v.iter().any(|s| s == &d) {
                v.push(d);
            }
            v
        };
        let try_dir = |dir: &Path| -> Option<PathBuf> {
            for stem in &stems {
                let p = dir.join(format!("{stem}.stub"));
                if p.exists() {
                    return Some(p);
                }
            }
            None
        };
        // VEIL_STUBS_DIR
        if let Ok(dir) = std::env::var("VEIL_STUBS_DIR") {
            if let Some(p) = try_dir(Path::new(&dir)) {
                return Some(p);
            }
        }
        // Host cache from DDB seed (veil-server stub_ops default_cache_dir)
        if let Some(p) = try_dir(&std::env::temp_dir().join("veil-platform-stubs")) {
            return Some(p);
        }
        // Next to system layers: VEIL_LAYERS_DIR/../runtime/src/stubs
        if let Ok(layers) = std::env::var("VEIL_LAYERS_DIR") {
            if let Some(p) = try_dir(&Path::new(&layers).join("../runtime/src/stubs")) {
                return Some(p);
            }
            if let Some(p) = try_dir(&Path::new(&layers).join("../examples")) {
                return Some(p);
            }
        }
        // Walk CWD ancestors for runtime/src/stubs and examples/
        if let Ok(cwd) = std::env::current_dir() {
            for anc in cwd.ancestors() {
                for rel in ["runtime/src/stubs", "examples"] {
                    if let Some(p) = try_dir(&anc.join(rel)) {
                        return Some(p);
                    }
                }
            }
        }
        // Relative to executable (installed layout)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                for rel in ["../runtime/src/stubs", "stubs", "../stubs"] {
                    if let Some(p) = try_dir(&exe_dir.join(rel)) {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    /// Merge raw (unresolved) specs into the registry, resolving `maps_to`
    /// transitively against everything already loaded.
    fn merge_and_resolve(&mut self, raw: RawLayer) -> Result<(), String> {
        // Constructs may reference each other within the same file, so resolve
        // against the union of existing + incoming.
        let mut pending: Vec<ConstructSpec> = raw.constructs;
        let existing = self.constructs.clone();
        let snapshot = pending.clone();
        for spec in &mut pending {
            spec.shape = resolve_construct_shape(&spec.maps_to, &existing, &snapshot)
                .ok_or_else(|| {
                    format!(
                        "construct '{}' in layer '{}': cannot resolve maps_to '{}' (not a core shape or known construct)",
                        spec.name, spec.layer, spec.maps_to
                    )
                })?;
            // Flag step-type constructs: maps_to chain passes through "step".
            if spec.maps_to == "step" {
                spec.is_step = true;
            }
        }
        // Later definitions shadow earlier ones with the same keyword.
        for spec in pending {
            self.constructs.retain(|c| c.keyword != spec.keyword);
            self.constructs.push(spec);
        }

        let existing_stmts = self.statements.clone();
        let snapshot_stmts = raw.statements.clone();
        for mut stmt in raw.statements {
            stmt.shape = resolve_statement_shape(&stmt.maps_to, &existing_stmts, &snapshot_stmts)
                .ok_or_else(|| {
                    format!(
                        "statement '{}' in layer '{}': cannot resolve maps_to '{}'",
                        stmt.keyword, stmt.layer, stmt.maps_to
                    )
                })?;
            // Resolve port_target/port_method: follow transitive chain to find Port.method
            let (target, method) = resolve_port_binding(&stmt.maps_to, &existing_stmts, &snapshot_stmts);
            stmt.port_target = target;
            stmt.port_method = method;
            self.statements.retain(|s| s.keyword != stmt.keyword);
            self.statements.push(stmt);
        }

        // Accumulate raw declaration blocks (deduplicated by first line).
        for decl in raw.declarations {
            if !self.declarations.iter().any(|d| d == &decl) {
                self.declarations.push(decl);
            }
        }

        // Store prompt text for LLM context.
        if let Some(prompt_text) = raw.prompt {
            self.prompts.push((raw.name.clone(), prompt_text));
        }

        // Accumulate codegen templates.
        for tpl in raw.codegen_templates {
            self.codegen_templates.push(tpl);
        }

        // Validate presentation construct-name refs + enums (LAY-002).
        let known: std::collections::HashSet<String> =
            self.constructs.iter().map(|c| c.name.clone()).collect();
        let to_check: Vec<(String, &crate::presentation::ConstructPresentation)> = self
            .constructs
            .iter()
            .filter(|c| !c.presentation.is_empty())
            .map(|c| (c.name.clone(), &c.presentation))
            .collect();
        crate::presentation::validate_presentations(&to_check, &known)?;

        Ok(())
    }
}

/// Resolve a `maps_to` value to a core shape, following construct references
/// transitively. Detects cycles.
fn resolve_construct_shape(
    maps_to: &str,
    existing: &[ConstructSpec],
    incoming: &[ConstructSpec],
) -> Option<Shape> {
    let mut current = maps_to.to_string();
    let mut visited: HashSet<String> = HashSet::new();
    loop {
        // "primitive" is used by base.layer to mean "I am the core shape myself".
        if current == "primitive" {
            return None; // handled by caller for base constructs; see below
        }
        // "step" means this construct is a typed step inside fn bodies.
        // It gets Struct shape (for its config fields) but is recognized
        // contextually by the parser — not as a top-level construct.
        if current == "step" {
            return Some(Shape::Struct);
        }
        if let Some(shape) = Shape::from_name(&current) {
            return Some(shape);
        }
        if !visited.insert(current.clone()) {
            return None; // cycle
        }
        // Follow a reference to another construct, by keyword or by name.
        // Incoming (same-file) constructs take precedence, then existing layers.
        let next = incoming
            .iter()
            .chain(existing.iter())
            .find(|c| c.keyword == current || c.name == current)
            .map(|c| c.maps_to.clone())?;
        current = next;
    }
}

fn resolve_statement_shape(
    maps_to: &str,
    existing: &[StatementSpec],
    incoming: &[StatementSpec],
) -> Option<StmtShape> {
    let mut current = maps_to.to_string();
    let mut visited: HashSet<String> = HashSet::new();
    loop {
        // Check for Port.method notation — shape is Call
        if current.contains('.') {
            return Some(StmtShape::Call);
        }
        if let Some(shape) = StmtShape::from_name(&current) {
            return Some(shape);
        }
        if !visited.insert(current.clone()) {
            return None;
        }
        let next = incoming
            .iter()
            .chain(existing.iter())
            .find(|s| s.keyword == current)
            .map(|s| s.maps_to.clone())?;
        current = next;
    }
}


/// Follow the maps_to chain transitively to find a `Target.method` binding.
/// Returns (Some(target), Some(method)) if found, (None, None) otherwise.
fn resolve_port_binding(
    maps_to: &str,
    existing: &[StatementSpec],
    incoming: &[StatementSpec],
) -> (Option<String>, Option<String>) {
    let mut current = maps_to.to_string();
    let mut visited: HashSet<String> = HashSet::new();
    loop {
        if let Some((target, method)) = current.split_once('.') {
            return (Some(target.to_string()), Some(method.to_string()));
        }
        if !visited.insert(current.clone()) {
            return (None, None);
        }
        // Follow reference to another statement
        let next = incoming
            .iter()
            .chain(existing.iter())
            .find(|s| s.keyword == current)
            .map(|s| s.maps_to.clone());
        match next {
            Some(n) => current = n,
            None => return (None, None),
        }
    }
}

// ─── Stub system (.stub files for third-party crate declarations) ─────────

/// Provenance / freshness metadata for a `.stub` (from header comments or directives).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StubProvenance {
    /// True when `# @generated …` or equivalent is present.
    #[serde(default)]
    pub generated: bool,
    /// e.g. `veil-stub-gen 1`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<String>,
    /// Where the API was taken from: `crates.io`, `path`, `git`, `hand`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `full` | `curated` | `sparse` — full = rustdoc dump; curated = hand/minimal pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    /// Hash of rustdoc (or content) input used at generation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustdoc_fingerprint: Option<String>,
    /// ISO-8601 generation timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// crates.io / Cargo package name when it differs from the VEIL use-name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_name: Option<String>,
}

/// A parsed `.stub` file — declares the public API of an external Rust crate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StubCrate {
    /// The crate name (e.g. "reqwest").
    pub name: String,
    /// The crate version (e.g. "0.12").
    pub version: String,
    /// Optional alias from `use <crate> as <alias>`. When set, stub types are
    /// registered with the alias prefix (e.g. `use aws-sdk-s3 as s3` makes
    /// types accessible as `S3Client` → `aws_sdk_s3::Client`).
    #[serde(default)]
    pub alias: Option<String>,
    /// Generation / versioning metadata (not used by codegen).
    #[serde(default)]
    pub provenance: StubProvenance,
    /// Cargo features for workspace.dependencies (GEN-006) — from stub line
    /// `cargo_features a, b, c`. Empty = plain version dep.
    #[serde(default)]
    pub cargo_features: Vec<String>,
    /// Extra Cargo deps needed to use this stub (e.g. `aws-config=1`).
    /// Line: `cargo_deps name=ver, other=ver`.
    #[serde(default)]
    pub cargo_deps: Vec<(String, String)>,
    /// Optional module prefix for model types (e.g. `types` → `crate::types::T`).
    /// Root types listed in `root_types` stay at crate root.
    #[serde(default)]
    pub types_module: Option<String>,
    /// Type names that live at the crate root (not under `types_module`).
    #[serde(default)]
    pub root_types: Vec<String>,
    /// VEIL type name → Rust type name when they differ (e.g. `Pool` → `PgPool`).
    /// Line: `rust_name Pool PgPool`
    #[serde(default)]
    pub rust_names: std::collections::HashMap<String, String>,
    /// Harness field constructors: type name → Rust expr that yields a value.
    /// Line form (multi-line raw): `harness_field Client """ ... """`.
    /// Used by the local `@main` harness for `@field(name: Type)` wiring —
    /// engine never invents SDK-specific construction.
    #[serde(default)]
    pub harness_fields: std::collections::HashMap<String, String>,
    /// Derives for multi-field domain types used as typed row/result types
    /// (e.g. `sqlx::FromRow`). Applied generically by rust codegen — no crate names in the engine.
    #[serde(default)]
    pub row_type_derives: Vec<String>,
    /// Derives for single-field wrapper domain types (e.g. `sqlx::Type`).
    #[serde(default)]
    pub wrapper_type_derives: Vec<String>,
    /// Extra attributes for single-field wrappers (inner of `#[…]`, e.g. `sqlx(transparent)`).
    #[serde(default)]
    pub wrapper_type_attrs: Vec<String>,
    /// Extra `use` lines when this stub is active (e.g. `sqlx::PgPool`).
    #[serde(default)]
    pub codegen_imports: Vec<String>,
    /// Struct declarations with their methods.
    pub structs: Vec<StubStruct>,
    /// Impl blocks (methods grouped by target type).
    pub impls: Vec<StubImpl>,
    /// Package-level free functions (`fn name(...)` at stub root, not under struct/impl).
    /// Called as `use_alias.fn(...)` or `crate_name.fn(...)` → `rust_crate::fn(...)`.
    #[serde(default)]
    pub free_fns: Vec<StubMethod>,
}

impl StubCrate {
    /// Rust path segment after `crate_name::` for a type (e.g. `types::AttributeValue`).
    pub fn rust_type_path(&self, type_name: &str) -> String {
        let rust_name = self
            .rust_names
            .get(type_name)
            .cloned()
            .unwrap_or_else(|| type_name.to_string());
        if self.root_types.iter().any(|t| t == type_name || t == &rust_name) {
            return rust_name;
        }
        // Per-type module path (e.g. `path objs::tree` on EntryKind → objs::tree::EntryKind).
        if let Some(s) = self.structs.iter().find(|s| s.name == type_name) {
            if let Some(ref mp) = s.module_path {
                return format!("{mp}::{rust_name}");
            }
        }
        if let Some(module) = &self.types_module {
            if !module.is_empty() {
                return format!("{module}::{rust_name}");
            }
        }
        rust_name
    }

    /// Count of `struct` / `trait` declarations (for sparse detection).
    pub fn api_type_count(&self) -> usize {
        self.structs.len()
    }

    /// True when the surface looks too thin for a real SDK (re-export facade or hand sketch).
    pub fn is_sparse(&self) -> bool {
        if self
            .provenance
            .surface
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("sparse"))
        {
            return true;
        }
        if self
            .provenance
            .surface
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("curated"))
        {
            // Curated surfaces are intentionally small.
            return false;
        }
        let method_count: usize = self.structs.iter().map(|s| s.methods.len()).sum::<usize>()
            + self.impls.iter().map(|i| i.methods.len()).sum::<usize>()
            + self.free_fns.len();
        self.api_type_count() < 5 && method_count < 8
    }

    /// True when version is missing / wildcard (cannot pin Cargo deps reliably).
    pub fn version_unpinned(&self) -> bool {
        let v = self.version.trim();
        v.is_empty() || v == "*" || v == "0" || !v.chars().next().is_some_and(|c| c.is_ascii_digit())
    }

    /// Human issues for catalog / diagnostics (not hard errors in the typechecker).
    pub fn freshness_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.version_unpinned() {
            notes.push(format!(
                "stub `{}` has no pin version (got {:?}) — re-run `veil stub-gen` or set `stub {} <semver>`",
                self.name, self.version, self.name
            ));
        }
        if self.is_sparse()
            && !self
                .provenance
                .surface
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("curated"))
        {
            notes.push(format!(
                "stub `{}` looks sparse ({} types) — expand with `veil stub-gen {}` or mark `surface curated`",
                self.name,
                self.api_type_count(),
                self.name
            ));
        }
        if !self.provenance.generated
            && self
                .provenance
                .surface
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("full"))
                .unwrap_or(false)
        {
            notes.push(format!(
                "stub `{}` claims full surface but is not @generated — prefer stub-gen",
                self.name
            ));
        }
        notes
    }
}

/// A struct declared in a stub file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubStruct {
    pub name: String,
    /// Optional module path within the crate (e.g. "objs::tree" for gix::objs::tree::EntryKind).
    /// When set, codegen qualifies as `crate::module_path::Name` instead of `crate::Name`.
    #[serde(default)]
    pub module_path: Option<String>,
    /// Methods declared directly on the struct (instance methods).
    pub methods: Vec<StubMethod>,
    /// When `new` lowers to a free function, optional typed free-fn name used when
    /// the enclosing method has a domain return type (e.g. `query_as` for `Query`).
    #[serde(default)]
    pub typed_variant: Option<String>,
    /// Turbofish type-param template for the typed free fn. Tokens:
    /// `_` = inferred, `return_type` = domain type from enclosing return.
    /// Default: `_, return_type` → `::<_, CohortDTO>`.
    #[serde(default)]
    pub typed_type_params: Option<String>,
}

/// An impl block in a stub file (associated functions/constructors).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubImpl {
    pub target: String,
    pub methods: Vec<StubMethod>,
}

/// A method/function signature in a stub file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubMethod {
    pub name: String,
    pub params: Vec<(String, String, bool)>, // (param_name, type_string, is_ref)
    pub return_type: Option<String>,         // VEIL type syntax (e.g. "Res!<Str>")
}

/// Apply a `# key value` or bare directive provenance line onto a stub.
fn apply_stub_meta_line(stub: &mut StubCrate, raw: &str) {
    let t = raw.trim();
    let body = t.strip_prefix('#').unwrap_or(t).trim();
    if body.is_empty() {
        return;
    }
    // `# @generated veil-stub-gen 1` or `# @generated`
    if let Some(rest) = body.strip_prefix("@generated") {
        stub.provenance.generated = true;
        let g = rest.trim();
        if !g.is_empty() {
            stub.provenance.generator = Some(g.to_string());
        } else if stub.provenance.generator.is_none() {
            stub.provenance.generator = Some("veil-stub-gen".into());
        }
        return;
    }
    // Auto-inferred comment from older generators
    if body.contains("Auto-inferred codegen policy") || body.contains("re-run veil stub-gen") {
        stub.provenance.generated = true;
        if stub.provenance.generator.is_none() {
            stub.provenance.generator = Some("veil-stub-gen".into());
        }
        if stub.provenance.surface.is_none() {
            stub.provenance.surface = Some("full".into());
        }
        return;
    }
    // `key value` pairs (comment or directive)
    let mut parts = body.splitn(2, char::is_whitespace);
    let key = parts.next().unwrap_or("").to_ascii_lowercase();
    let val = parts.next().unwrap_or("").trim();
    match key.as_str() {
        "source" if !val.is_empty() => stub.provenance.source = Some(val.to_string()),
        "surface" if !val.is_empty() => stub.provenance.surface = Some(val.to_string()),
        "rustdoc_fingerprint" | "fingerprint" if !val.is_empty() => {
            stub.provenance.rustdoc_fingerprint = Some(val.to_string());
        }
        "generated_at" if !val.is_empty() => stub.provenance.generated_at = Some(val.to_string()),
        "cargo_name" if !val.is_empty() => stub.provenance.cargo_name = Some(val.to_string()),
        "generator" if !val.is_empty() => {
            stub.provenance.generated = true;
            stub.provenance.generator = Some(val.to_string());
        }
        _ => {}
    }
}

/// Parse a `.stub` file into a StubCrate.
pub fn parse_stub_file(content: &str) -> Option<StubCrate> {
    let mut stub = StubCrate::default();
    let mut current_struct: Option<StubStruct> = None;
    let mut current_impl: Option<StubImpl> = None;
    // Multi-line `harness_field Type """ ... """` capture
    let mut harness_field_name: Option<String> = None;
    let mut harness_field_buf: Option<String> = None;
    let mut saw_header = false;

    for line in content.lines() {
        // Finish multi-line harness_field raw string
        if let Some(ref mut buf) = harness_field_buf {
            let trimmed_end = line.trim();
            if trimmed_end.ends_with("\"\"\"") {
                let before = trimmed_end.trim_end_matches("\"\"\"").trim_end();
                if !before.is_empty() {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(before);
                }
                if let Some(name) = harness_field_name.take() {
                    stub.harness_fields.insert(name, buf.clone());
                }
                harness_field_buf = None;
                continue;
            }
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Meta comments (anywhere near header / before structs)
        if trimmed.starts_with('#') {
            apply_stub_meta_line(&mut stub, trimmed);
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        // Header: stub <name> <version>
        if trimmed.starts_with("stub ") {
            let parts: Vec<&str> = trimmed.strip_prefix("stub ").unwrap().split_whitespace().collect();
            stub.name = parts.first().unwrap_or(&"").to_string();
            stub.version = parts.get(1).unwrap_or(&"*").to_string();
            saw_header = true;
            continue;
        }

        // Bare provenance directives (not comments): surface curated
        if saw_header
            && indent <= 2
            && matches!(
                trimmed.split_whitespace().next(),
                Some("surface" | "source" | "cargo_name" | "generator" | "generated_at" | "rustdoc_fingerprint" | "fingerprint")
            )
        {
            apply_stub_meta_line(&mut stub, trimmed);
            continue;
        }

        // GEN-006: cargo_features runtime-tokio, postgres, ...
        if trimmed.starts_with("cargo_features ") {
            stub.cargo_features = trimmed
                .strip_prefix("cargo_features ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // cargo_deps aws-config=1, other=0.2
        if trimmed.starts_with("cargo_deps ") {
            for part in trimmed
                .strip_prefix("cargo_deps ")
                .unwrap_or("")
                .split(',')
            {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                if let Some((n, v)) = part.split_once('=') {
                    stub.cargo_deps
                        .push((n.trim().to_string(), v.trim().to_string()));
                }
            }
            continue;
        }

        // types_module types
        if trimmed.starts_with("types_module ") {
            stub.types_module = Some(
                trimmed
                    .strip_prefix("types_module ")
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            );
            continue;
        }

        // root_types Client, Config, Error
        if trimmed.starts_with("root_types ") {
            stub.root_types = trimmed
                .strip_prefix("root_types ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // rust_name Pool PgPool  (VEIL name → Rust name)
        if trimmed.starts_with("rust_name ") {
            let rest = trimmed.strip_prefix("rust_name ").unwrap_or("").trim();
            let mut parts = rest.split_whitespace();
            if let (Some(veil), Some(rust)) = (parts.next(), parts.next()) {
                stub.rust_names
                    .insert(veil.to_string(), rust.to_string());
            }
            continue;
        }

        // row_type_derives sqlx::FromRow
        if trimmed.starts_with("row_type_derives ") {
            stub.row_type_derives = trimmed
                .strip_prefix("row_type_derives ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // wrapper_type_derives sqlx::Type
        if trimmed.starts_with("wrapper_type_derives ") {
            stub.wrapper_type_derives = trimmed
                .strip_prefix("wrapper_type_derives ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // wrapper_type_attrs sqlx(transparent)
        if trimmed.starts_with("wrapper_type_attrs ") {
            stub.wrapper_type_attrs = trimmed
                .strip_prefix("wrapper_type_attrs ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().trim_start_matches("#[").trim_end_matches(']').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // codegen_imports sqlx::PgPool, other::Thing
        if trimmed.starts_with("codegen_imports ") {
            stub.codegen_imports = trimmed
                .strip_prefix("codegen_imports ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // harness_field Client """ ... """  (same line or multi-line)
        if trimmed.starts_with("harness_field ") {
            let rest = trimmed.strip_prefix("harness_field ").unwrap_or("").trim();
            let (name, after_name) = if let Some((n, r)) = rest.split_once(char::is_whitespace) {
                (n.to_string(), r.trim())
            } else {
                (rest.to_string(), "")
            };
            if after_name.starts_with("\"\"\"") {
                let inner = after_name.trim_start_matches("\"\"\"");
                if inner.ends_with("\"\"\"") && inner.len() >= 3 {
                    let body = inner.trim_end_matches("\"\"\"").trim().to_string();
                    stub.harness_fields.insert(name, body);
                } else {
                    // start multi-line
                    harness_field_name = Some(name);
                    harness_field_buf = Some(inner.to_string());
                }
            }
            continue;
        }

        // Top-level struct declaration
        if indent <= 2 && trimmed.starts_with("struct ") {
            // Flush previous
            if let Some(s) = current_struct.take() { stub.structs.push(s); }
            if let Some(i) = current_impl.take() { stub.impls.push(i); }
            let name = trimmed.strip_prefix("struct ").unwrap().trim().to_string();
            current_struct = Some(StubStruct {
                name,
                module_path: None,
                methods: Vec::new(),
                typed_variant: None,
                typed_type_params: None,
            });
            continue;
        }

        // Top-level enum declaration — treated as a struct for name resolution,
        // with each variant exposed as a constructor method.
        if indent <= 2 && trimmed.starts_with("enum ") {
            if let Some(s) = current_struct.take() { stub.structs.push(s); }
            if let Some(i) = current_impl.take() { stub.impls.push(i); }
            let name = trimmed.strip_prefix("enum ").unwrap().trim().to_string();
            current_struct = Some(StubStruct {
                name,
                module_path: None,
                methods: Vec::new(),
                typed_variant: None,
                typed_type_params: None,
            });
            continue;
        }

        // Struct-level metadata (indented under struct, not a method)
        if indent >= 4 && current_struct.is_some() && !trimmed.starts_with("fn ") {
            if trimmed.starts_with("path ") {
                let v = trimmed
                    .strip_prefix("path ")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if let Some(ref mut s) = current_struct {
                    s.module_path = Some(v);
                }
                continue;
            }
            if trimmed.starts_with("typed_variant ") {
                let v = trimmed
                    .strip_prefix("typed_variant ")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if let Some(ref mut s) = current_struct {
                    s.typed_variant = Some(v);
                }
                continue;
            }
            if trimmed.starts_with("typed_type_params ") {
                let v = trimmed
                    .strip_prefix("typed_type_params ")
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if let Some(ref mut s) = current_struct {
                    s.typed_type_params = Some(v);
                }
                continue;
            }
        }

        // Top-level impl declaration
        if indent <= 2 && trimmed.starts_with("impl ") {
            if let Some(s) = current_struct.take() { stub.structs.push(s); }
            if let Some(i) = current_impl.take() { stub.impls.push(i); }
            let target = trimmed.strip_prefix("impl ").unwrap().trim().to_string();
            current_impl = Some(StubImpl { target, methods: Vec::new() });
            continue;
        }

        // Package-level free functions (indent under stub header, not under struct/impl)
        if indent <= 2 && trimmed.starts_with("fn ") && current_struct.is_none() && current_impl.is_none() {
            stub.free_fns.push(parse_stub_method(trimmed));
            continue;
        }

        // Method declaration (indented under struct or impl)
        if indent >= 4 && trimmed.starts_with("fn ") {
            let method = parse_stub_method(trimmed);
            if let Some(ref mut s) = current_struct {
                s.methods.push(method);
            } else if let Some(ref mut i) = current_impl {
                i.methods.push(method);
            }
        }

        // Enum variant line (indented, not starting with fn) — treat as constructor
        if indent >= 4 && !trimmed.starts_with("fn ") && current_struct.is_some() {
            // Parse variant: Name or Name(Type1, Type2)
            let (vname, params) = if let Some(paren_start) = trimmed.find('(') {
                let name = trimmed[..paren_start].trim().to_string();
                let params_str = &trimmed[paren_start + 1..trimmed.len() - 1];
                let params: Vec<(String, String, bool)> = params_str
                    .split(',')
                    .enumerate()
                    .map(|(i, t)| (format!("v{}", i), t.trim().to_string(), false))
                    .collect();
                (name, params)
            } else {
                (trimmed.to_string(), Vec::new())
            };
            // Only add if it looks like a variant (starts with uppercase)
            if vname.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                let ret_type = current_struct.as_ref().map(|s| s.name.clone());
                if let Some(ref mut s) = current_struct {
                    s.methods.push(StubMethod {
                        name: vname,
                        params,
                        return_type: ret_type,
                    });
                }
            }
        }
    }

    // Flush remaining
    if let Some(s) = current_struct { stub.structs.push(s); }
    if let Some(i) = current_impl { stub.impls.push(i); }

    if stub.name.is_empty() { return None; }
    Some(stub)
}

/// Parse a method signature line like `fn get(url: Str) -> RequestBuilder`
fn parse_stub_method(line: &str) -> StubMethod {
    let line = line.strip_prefix("fn ").unwrap_or(line).trim();

    // Split on -> for return type
    let (sig, ret) = if let Some((l, r)) = line.split_once("->") {
        (l.trim(), Some(r.trim().to_string()))
    } else {
        (line, None)
    };

    // Parse name and params
    let (name, params_str) = if let Some((n, p)) = sig.split_once('(') {
        (n.trim().to_string(), p.trim_end_matches(')').to_string())
    } else {
        (sig.to_string(), String::new())
    };

    let params: Vec<(String, String, bool)> = if params_str.is_empty() {
        Vec::new()
    } else {
        params_str.split(',').map(|p| {
            let p = p.trim();
            // Check for `ref` keyword prefix
            let (is_ref, p) = if p.starts_with("ref ") {
                (true, p.strip_prefix("ref ").unwrap().trim())
            } else {
                (false, p)
            };
            if let Some((name, ty)) = p.split_once(':') {
                (name.trim().to_string(), ty.trim().to_string(), is_ref)
            } else {
                (p.to_string(), "Str".to_string(), is_ref)
            }
        }).collect()
    };

    StubMethod { name, params, return_type: ret }
}


/// Parse a `.layer` file into raw (shape-unresolved) specs.

/// Raw layer parse result (IDE / check / graph).
#[derive(Debug, Clone)]
pub struct RawLayer {
    pub name: String,
    pub constructs: Vec<ConstructSpec>,
    pub statements: Vec<StatementSpec>,
    /// Raw VEIL source blocks declared by this layer (e.g. `port Bus ...`).
    /// Each entry is one top-level construct declaration, dedented for parsing.
    pub declarations: Vec<String>,
    /// LLM prompt text for this layer (RAG context for code-generating agents).
    pub prompt: Option<String>,
    /// Codegen template blocks declared by this layer.
    pub codegen_templates: Vec<CodegenTemplate>,
}

/// Parse a `.layer` file (public for IDE / check; DSL-003).
pub fn parse_layer_file(content: &str, layer_name: &str) -> Result<RawLayer, String> {
    #[derive(PartialEq)]
    enum Section {
        None,
        Contains,
        Constraints,
        Visual,
        Annotations,
        Runtime,
        Present,
        FieldHints,
        /// Per-target lowering templates under a statement (`lowers_to`).
        LowersTo,
    }

    enum Item {
        Construct(ConstructSpec),
        Statement(StatementSpec),
    }

    let mut items: Vec<Item> = Vec::new();
    let mut current: Option<Item> = None;
    let mut section = Section::None;
    let mut declarations: Vec<String> = Vec::new();
    let mut in_declare = false;
    let mut declare_base_indent: usize = 0;
    let mut current_decl_lines: Vec<String> = Vec::new();
    let mut in_prompt = false;
    let mut prompt_base_indent: usize = 0;
    let mut prompt_lines: Vec<String> = Vec::new();
    // Codegen block parsing state
    let mut codegen_templates: Vec<CodegenTemplate> = Vec::new();
    let mut in_codegen = false;
    let mut codegen_target: String = String::new();
    let mut codegen_base_indent: usize = 0;
    let mut codegen_lines: Vec<String> = Vec::new();
    // In-progress `view` under `present` (flushed on next view / role / section).
    let mut present_view: Option<crate::presentation::ViewSpec> = None;
    let mut errors: Vec<String> = Vec::new();

    let flush_present_view = |item: &mut Option<Item>,
                              view: &mut Option<crate::presentation::ViewSpec>| {
        if let (Some(Item::Construct(c)), Some(v)) = (item.as_mut(), view.take()) {
            c.presentation.views.push(v);
        }
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            // Blank lines inside declare blocks are preserved
            if in_declare && !current_decl_lines.is_empty() {
                current_decl_lines.push(String::new());
            }
            // Blank lines inside codegen blocks are preserved
            if in_codegen {
                codegen_lines.push(String::new());
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        // Handle `declare` section: accumulate raw VEIL source text.
        // Must clear other top-level sections first: `prompt`/`codegen` leave
        // conditions only fire on fall-through, but `declare` is matched above
        // those checks — without clearing, declare body is swallowed as prompt
        // (ddd.layer: prompt then comments then declare).
        if trimmed == "declare" && indent <= 2 {
            if let Some(item) = current.take() {
                items.push(item);
            }
            // Flush in-progress codegen before leaving it
            if in_codegen && !codegen_lines.is_empty() {
                let template = parse_codegen_block(&codegen_target, &codegen_lines, layer_name);
                codegen_templates.push(template);
                codegen_lines.clear();
            }
            in_codegen = false;
            in_prompt = false;
            in_declare = true;
            declare_base_indent = indent + 2; // items inside declare are at +2
            section = Section::None;
            continue;
        }

        // Handle `prompt` section: accumulate text for LLM context (ignored by codegen)
        if trimmed == "prompt" && indent <= 2 {
            if let Some(item) = current.take() {
                items.push(item);
            }
            // Leaving declare for prompt — flush declaration blocks
            if in_declare && !current_decl_lines.is_empty() {
                while current_decl_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                    current_decl_lines.pop();
                }
                declarations.push(current_decl_lines.join("\n"));
                current_decl_lines.clear();
            }
            if in_codegen && !codegen_lines.is_empty() {
                let template = parse_codegen_block(&codegen_target, &codegen_lines, layer_name);
                codegen_templates.push(template);
                codegen_lines.clear();
            }
            in_declare = false;
            in_codegen = false;
            in_prompt = true;
            prompt_base_indent = indent + 2;
            section = Section::None;
            continue;
        }

        if in_prompt {
            if indent <= prompt_base_indent.saturating_sub(2) && !trimmed.is_empty() {
                // Leaving prompt section — flush accumulated lines
                in_prompt = false;
                // Fall through to normal parsing of this line
            } else {
                // Accumulate prompt line (strip the base indent)
                let stripped = if line.len() > prompt_base_indent {
                    &line[prompt_base_indent..]
                } else {
                    trimmed
                };
                prompt_lines.push(stripped.to_string());
                continue;
            }
        }

        if in_declare {
            // If we hit something at the same or lesser indent as declare, we're leaving it
            if indent <= declare_base_indent.saturating_sub(2) && !trimmed.is_empty() {
                // Flush current declaration block
                if !current_decl_lines.is_empty() {
                    // Trim trailing blank lines
                    while current_decl_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                        current_decl_lines.pop();
                    }
                    declarations.push(current_decl_lines.join("\n"));
                    current_decl_lines.clear();
                }
                in_declare = false;
                // Fall through to normal parsing of this line
            } else {
                // Determine whether to flush the accumulated declaration block.
                // A declaration is "one construct with optional leading annotations."
                // We flush when a new top-level item begins. An annotation at base
                // indent signals a new item IF the block already has a construct
                // keyword. A non-annotation at base indent always signals a new item
                // if there's anything accumulated.
                let block_has_construct = current_decl_lines.iter().any(|l| {
                    let lt = l.trim();
                    !lt.is_empty() && !lt.starts_with('@')
                });
                let should_flush = indent == declare_base_indent && block_has_construct;
                if should_flush {
                    while current_decl_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                        current_decl_lines.pop();
                    }
                    declarations.push(current_decl_lines.join("\n"));
                    current_decl_lines.clear();
                }
                // Dedent to be parseable as top-level VEIL
                let dedented = if line.len() > declare_base_indent {
                    &line[declare_base_indent..]
                } else {
                    trimmed
                };
                current_decl_lines.push(dedented.to_string());
                continue;
            }
        }

        // Handle `codegen <target>` blocks: accumulate raw template text
        if trimmed.starts_with("codegen ") && indent <= 2 {
            // Flush any previous codegen block
            if in_codegen && !codegen_lines.is_empty() {
                let template = parse_codegen_block(&codegen_target, &codegen_lines, layer_name);
                codegen_templates.push(template);
                codegen_lines.clear();
            }
            // Flush declare if we jumped from declare → codegen
            if in_declare && !current_decl_lines.is_empty() {
                while current_decl_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                    current_decl_lines.pop();
                }
                declarations.push(current_decl_lines.join("\n"));
                current_decl_lines.clear();
            }
            // Flush any previous construct/statement
            if let Some(item) = current.take() {
                items.push(item);
            }
            in_declare = false;
            in_prompt = false;
            codegen_target = trimmed.strip_prefix("codegen ").unwrap().trim().to_string();
            in_codegen = true;
            codegen_base_indent = indent + 2; // rules inside codegen are at +2
            section = Section::None;
            continue;
        }

        if in_codegen {
            // If we hit something at the same or lesser indent as codegen, we're leaving it
            if indent <= 2 && !trimmed.is_empty() {
                // Flush current codegen block
                if !codegen_lines.is_empty() {
                    let template = parse_codegen_block(&codegen_target, &codegen_lines, layer_name);
                    codegen_templates.push(template);
                    codegen_lines.clear();
                }
                in_codegen = false;
                // Fall through to normal parsing of this line
            } else {
                // Accumulate codegen lines (dedented)
                let dedented = if line.len() > codegen_base_indent {
                    &line[codegen_base_indent..]
                } else {
                    trimmed
                };
                codegen_lines.push(dedented.to_string());
                continue;
            }
        }

        if trimmed.starts_with("construct ") {
            flush_present_view(&mut current, &mut present_view);
            if let Some(item) = current.take() {
                items.push(item);
            }
            let name = trimmed.strip_prefix("construct ").unwrap().trim().to_string();
            current = Some(Item::Construct(ConstructSpec {
                keyword: name.clone(),
                name: name.clone(),
                maps_to: String::new(),
                shape: Shape::Struct, // placeholder, resolved later
                layer: layer_name.to_string(),
                desc: String::new(),
                contains: Vec::new(),
                blocks: Vec::new(),
                raw_block_keywords: Vec::new(),
                constraints: Vec::new(),
                allowed_in: String::new(),
                group: String::new(),
                au: false,
                is_step: false,
                step_fields: Vec::new(),
                visual: Visual {
                    label: name,
                    ..Default::default()
                },
                annotations: Vec::new(),
                runtime: None,
                tgt: String::new(),
                dg: String::new(),
                presentation: Default::default(),
                roles: Vec::new(),
                config_keys: Vec::new(),
            }));
            section = Section::None;
            present_view = None;
            continue;
        }
        if trimmed.starts_with("statement ") {
            flush_present_view(&mut current, &mut present_view);
            if let Some(item) = current.take() {
                items.push(item);
            }
            let keyword = trimmed.strip_prefix("statement ").unwrap().trim().to_string();
            current = Some(Item::Statement(StatementSpec {
                keyword: keyword.clone(),
                maps_to: String::new(),
                shape: StmtShape::Call, // placeholder
                port_target: None,
                port_method: None,
                // Auto-detect infix operators: keywords containing non-alphanumeric chars
                is_infix: keyword.chars().any(|c| !c.is_alphanumeric() && c != '_'),
                requires_dep: None,
                lowers_to: HashMap::new(),
                layer: layer_name.to_string(),
                desc: String::new(),
                semantics: String::new(),
                visual: Visual {
                    label: keyword,
                    ..Default::default()
                },
            }));
            section = Section::None;
            continue;
        }

        if current.is_none() {
            continue;
        }

        // Section headers (indent 4 = direct child of construct/statement).
        if indent <= 4 {
            // `runtime <coordinator> <step_trait>` opens a runtime binding whose
            // nested `sub_block -> method` lines fill the method map.
            if let Some(rest) = trimmed.strip_prefix("runtime ") {
                flush_present_view(&mut current, &mut present_view);
                let mut parts = rest.split_whitespace();
                let coordinator = parts.next().unwrap_or("").to_string();
                let step_trait = parts.next().unwrap_or("").to_string();
                if let Some(Item::Construct(c)) = current.as_mut() {
                    c.runtime = Some(RuntimeBinding {
                        coordinator,
                        step_trait,
                        method_map: Vec::new(),
                    });
                }
                section = Section::Runtime;
                continue;
            }
            match trimmed {
                "has" | "contains" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::Contains;
                    continue;
                }
                "cst" | "constraints" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::Constraints;
                    continue;
                }
                "visual" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::Visual;
                    continue;
                }
                "ann" | "annotations" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::Annotations;
                    continue;
                }
                "present" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::Present;
                    continue;
                }
                "field_hints" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::FieldHints;
                    continue;
                }
                "lowers_to" => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::LowersTo;
                    continue;
                }
                _ => {
                    flush_present_view(&mut current, &mut present_view);
                    section = Section::None;
                }
            }
        }

        let Some(item) = current.as_mut() else { continue };

        match section {
            Section::Contains => {
                if let Item::Construct(c) = item {
                    let entry = trimmed.to_string();
                    // `keyword: shape` entries declare named sub-blocks.
                    if let Some((kw, shape_name)) = entry.split_once(':') {
                        let shape_str = shape_name.trim();
                        if shape_str == "raw" {
                            // Raw string block (e.g. template: raw, style: raw)
                            c.raw_block_keywords.push(kw.trim().to_string());
                        } else if let Some(shape) = Shape::from_name(shape_str) {
                            c.blocks.push((kw.trim().to_string(), shape));
                        }
                        let key = kw.trim().to_string();
                        if !key.is_empty() && !c.config_keys.iter().any(|k| k == &key) {
                            c.config_keys.push(key);
                        }
                        // Build StepFieldSpec with meta-type for step-type constructs.
                        c.step_fields.push(StepFieldSpec {
                            name: kw.trim().to_string(),
                            meta: FieldMeta::parse(shape_str),
                            label: String::new(),
                            filter: String::new(),
                            editor: String::new(),
                        });
                    }
                    c.contains.push(entry.trim_end_matches("[]").to_string());
                }
            }
            Section::Constraints => {
                if let Item::Construct(c) = item {
                    c.constraints.push(trimmed.to_string());
                }
            }
            Section::Runtime => {
                // `sub_block -> method` maps a step sub-block to a trait method.
                if let Item::Construct(c) = item {
                    if let Some(rt) = c.runtime.as_mut() {
                        if let Some((kw, method)) = trimmed.split_once("->") {
                            rt.method_map.push((kw.trim().to_string(), method.trim().to_string()));
                        }
                    }
                }
            }
            Section::Annotations => {
                // Grammar: `name: "description" param1, param2`
                if let Item::Construct(c) = item {
                    if let Some((name, rest)) = trimmed.split_once(':') {
                        let rest = rest.trim();
                        // Optional quoted description, then comma-separated params.
                        let (desc, params_str) = if rest.starts_with('"') {
                            if let Some(end) = rest[1..].find('"') {
                                (rest[1..=end].to_string(), rest[end + 2..].trim().to_string())
                            } else {
                                (String::new(), rest.to_string())
                            }
                        } else {
                            (String::new(), rest.to_string())
                        };
                        let mut roles = Vec::new();
                        let mut params = Vec::new();
                        for slot in params_str.split(',') {
                            for tok in slot.split_whitespace() {
                                if let Some(r) = tok.strip_prefix("role:") {
                                    roles.push(r.to_string());
                                } else if !tok.is_empty() {
                                    params.push(tok.to_string());
                                }
                            }
                        }
                        c.annotations.push(AnnotationSpec {
                            name: name.trim().to_string(),
                            desc,
                            params,
                            roles,
                        });
                    }
                }
            }
            Section::FieldHints => {
                // Grammar: `field_name: hint_key hint_value`
                // E.g. `target: filter_by subkind:Repository`
                //       `target: label "Repository"`
                if let Item::Construct(c) = item {
                    if let Some((field_name, rest)) = trimmed.split_once(':') {
                        let field_name = field_name.trim();
                        let rest = rest.trim();
                        if let Some(spec) = c.step_fields.iter_mut().find(|f| f.name == field_name) {
                            if let Some(v) = rest.strip_prefix("filter_by ") {
                                spec.filter = v.trim().to_string();
                            } else if let Some(v) = rest.strip_prefix("label ") {
                                spec.label = unquote(v);
                            } else if let Some(v) = rest.strip_prefix("editor ") {
                                spec.editor = v.trim().to_string();
                            }
                        }
                    }
                }
            }
            Section::LowersTo => {
                // Lines: `rust: "template…"` or `typescript: "…"`
                if let Item::Statement(s) = item {
                    if let Some((target, rest)) = trimmed.split_once(':') {
                        let target = target.trim().to_string();
                        let template = unquote(rest.trim());
                        if !target.is_empty() && !template.is_empty() {
                            s.lowers_to.insert(target, template);
                        }
                    }
                }
            }
            Section::Visual => {
                let visual = match item {
                    Item::Construct(c) => &mut c.visual,
                    Item::Statement(s) => &mut s.visual,
                };
                if let Some(v) = trimmed.strip_prefix("icon ") {
                    visual.icon = unquote(v);
                } else if let Some(v) = trimmed.strip_prefix("color ") {
                    visual.color = unquote(v);
                } else if let Some(v) = trimmed.strip_prefix("label ") {
                    visual.label = unquote(v);
                }
            }
            Section::Present => {
                if let Item::Construct(c) = item {
                    if trimmed == "ide" {
                        // Enter IDE constraints sub-block; flush any in-progress view.
                        if let Some(v) = present_view.take() {
                            c.presentation.views.push(v);
                        }
                        if c.presentation.ide_constraints.is_none() {
                            c.presentation.ide_constraints = Some(crate::presentation::IdeConstraints::default());
                        }
                    } else if c.presentation.ide_constraints.is_some()
                        && crate::presentation::is_ide_constraint_line(trimmed)
                    {
                        if let Err(e) = crate::presentation::apply_ide_constraint_line(
                            c.presentation.ide_constraints.as_mut().unwrap(),
                            trimmed,
                        ) {
                            errors.push(format!("construct '{}': {}", c.name, e));
                        }
                    } else if let Some(rest) = trimmed.strip_prefix("view ") {
                        if let Some(v) = present_view.take() {
                            c.presentation.views.push(v);
                        }
                        present_view = Some(crate::presentation::ViewSpec::new(rest.trim()));
                    } else if present_view.is_some()
                        && crate::presentation::is_view_property_line(trimmed)
                    {
                        if let Some(v) = present_view.as_mut() {
                            if let Err(e) = crate::presentation::apply_view_line(v, trimmed) {
                                errors.push(format!("construct '{}': {}", c.name, e));
                            }
                        }
                    } else {
                        if let Some(v) = present_view.take() {
                            c.presentation.views.push(v);
                        }
                        if let Err(e) =
                            crate::presentation::apply_construct_present_line(&mut c.presentation, trimmed)
                        {
                            errors.push(format!("construct '{}': {}", c.name, e));
                        }
                    }
                }
            }
            Section::None => match item {
                Item::Construct(c) => {
                    if let Some(v) = trimmed.strip_prefix("kw ").or_else(|| trimmed.strip_prefix("keyword ")) {
                        c.keyword = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("mt ").or_else(|| trimmed.strip_prefix("maps_to ")) {
                        c.maps_to = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("desc ") {
                        c.desc = unquote(v);
                    } else if let Some(v) = trimmed.strip_prefix("in ").or_else(|| trimmed.strip_prefix("allowed_in ")) {
                        c.allowed_in = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("group ") {
                        c.group = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("tgt ") {
                        c.tgt = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("dg ") {
                        c.dg = v.trim().to_string();
                    } else if trimmed == "au" {
                        c.au = true;
                    } else if let Some(v) = trimmed.strip_prefix("role ") {
                        for role in parse_construct_roles(v) {
                            if !c.roles.iter().any(|r| r == &role) {
                                c.roles.push(role);
                            }
                        }
                    }
                }
                Item::Statement(s) => {
                    if let Some(v) = trimmed.strip_prefix("mt ").or_else(|| trimmed.strip_prefix("maps_to ")) {
                        s.maps_to = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("kw ").or_else(|| trimmed.strip_prefix("keyword ")) {
                        s.keyword = v.trim().to_string();
                        // Re-detect infix based on the new keyword
                        s.is_infix = s.keyword.chars().any(|c| !c.is_alphanumeric() && c != '_');
                    } else if let Some(v) = trimmed.strip_prefix("desc ") {
                        s.desc = unquote(v);
                    } else if let Some(v) = trimmed.strip_prefix("sem ").or_else(|| trimmed.strip_prefix("semantics ")) {
                        s.semantics = v.trim().to_string();
                    } else if let Some(v) = trimmed
                        .strip_prefix("requires_dep ")
                        .or_else(|| trimmed.strip_prefix("requires "))
                    {
                        let dep = v.trim().to_string();
                        if !dep.is_empty() {
                            s.requires_dep = Some(dep);
                        }
                    }
                }
            },
        }
    }
    flush_present_view(&mut current, &mut present_view);
    if let Some(item) = current.take() {
        items.push(item);
    }

    // Flush any remaining declaration block
    if !current_decl_lines.is_empty() {
        while current_decl_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            current_decl_lines.pop();
        }
        declarations.push(current_decl_lines.join("\n"));
    }

    let mut constructs = Vec::new();
    let mut statements = Vec::new();
    for item in items {
        match item {
            Item::Construct(mut c) => {
                // base.layer marks core constructs with `maps_to primitive`,
                // meaning the construct IS the core shape named by its keyword.
                if c.maps_to == "primitive" {
                    c.maps_to = c.keyword.clone();
                }
                constructs.push(c);
            }
            Item::Statement(s) => statements.push(s),
        }
    }

    // Flush any trailing codegen block
    if in_codegen && !codegen_lines.is_empty() {
        let template = parse_codegen_block(&codegen_target, &codegen_lines, layer_name);
        codegen_templates.push(template);
    }

    // Flush any trailing prompt content
    let prompt = if !prompt_lines.is_empty() {
        while prompt_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            prompt_lines.pop();
        }
        Some(prompt_lines.join("\n"))
    } else {
        None
    };

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

    Ok(RawLayer {
        name: layer_name.to_string(),
        constructs,
        statements,
        declarations,
        prompt,
        codegen_templates,
    })
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse accumulated codegen block lines into a CodegenTemplate.
/// Lines are already dedented relative to the `codegen <target>` block.
///
/// Expected format (each line at this level):
///   match <shape> where <condition>
///     emit_to "<section>" priority <n>   (optional)
///     emit """
///       <template body>
///     """
fn parse_codegen_block(target: &str, lines: &[String], layer_name: &str) -> CodegenTemplate {
    let mut rules: Vec<CodegenRule> = Vec::new();
    let mut scaffold: Vec<ScaffoldFile> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Scaffold file: `scaffold "path/to/file"`  followed by emit """..."""
        if line.starts_with("scaffold ") {
            let path = unquote(&line[9..]);
            i += 1;
            // Expect emit """ block
            let mut content = String::new();
            while i < lines.len() {
                let sl = lines[i].trim();
                if sl.starts_with("emit \"\"\"") || sl == "emit" {
                    i += 1;
                    let mut body_lines: Vec<&str> = Vec::new();
                    while i < lines.len() {
                        let el = lines[i].trim_end();
                        if el.trim() == "\"\"\"" {
                            break;
                        }
                        body_lines.push(el);
                        i += 1;
                    }
                    content = body_lines.join("\n");
                    i += 1; // skip closing """
                    break;
                } else {
                    i += 1;
                }
            }
            scaffold.push(ScaffoldFile { path, content });
            continue;
        }

        // Look for `match <shape> where <condition>` or `match <shape>`
        if line.starts_with("match ") {
            let rest = &line[6..]; // after "match "
            let (match_shape, condition) = if let Some(idx) = rest.find(" where ") {
                (rest[..idx].trim().to_string(), rest[idx + 7..].trim().to_string())
            } else {
                (rest.trim().to_string(), String::new())
            };

            // Parse the rule body (emit_to, emit_file, emit)
            let mut emit_to: Option<String> = None;
            let mut emit_file: Option<String> = None;
            let mut priority: u32 = 100;
            let mut emit_body = String::new();

            i += 1;
            while i < lines.len() {
                let rule_line = lines[i].trim();

                if rule_line.starts_with("match ") || rule_line.starts_with("scaffold ")
                    || (rule_line.is_empty()
                        && i + 1 < lines.len()
                        && (lines[i + 1].trim().starts_with("match ")
                            || lines[i + 1].trim().starts_with("scaffold ")))
                {
                    break; // next rule or scaffold
                }

                if rule_line.starts_with("emit_file ") {
                    // If we already have a pending emit_file+emit_body pair, push it
                    // as its own rule before starting the next one.
                    if emit_file.is_some() && !emit_body.is_empty() {
                        rules.push(CodegenRule {
                            match_shape: match_shape.clone(),
                            condition: condition.clone(),
                            emit_body: std::mem::take(&mut emit_body),
                            emit_to: emit_to.take(),
                            emit_file: emit_file.take(),
                            priority,
                        });
                    }
                    emit_file = Some(unquote(&rule_line[10..]));
                    i += 1;
                } else if rule_line.starts_with("emit_to ") {
                    // Parse: emit_to "section" priority N
                    let et_rest = &rule_line[8..];
                    if let Some(section_end) = et_rest.find('"') {
                        let after_first_quote = &et_rest[section_end + 1..];
                        if let Some(second_quote) = after_first_quote.find('"') {
                            let section_name = after_first_quote[..second_quote].to_string();
                            emit_to = Some(section_name);
                            // Check for priority
                            let after_section = &after_first_quote[second_quote + 1..];
                            if let Some(prio_idx) = after_section.find("priority") {
                                let prio_str = after_section[prio_idx + 8..].trim();
                                priority = prio_str.parse().unwrap_or(100);
                            }
                        }
                    } else if let Some(start) = et_rest.find('"') {
                        // Simple: emit_to "section"
                        let after = &et_rest[start + 1..];
                        if let Some(end) = after.find('"') {
                            emit_to = Some(after[..end].to_string());
                        }
                    }
                    i += 1;
                } else if rule_line.starts_with("emit \"\"\"") || rule_line == "emit" {
                    // Multi-line emit block — collect until closing """
                    i += 1;
                    let mut body_lines: Vec<&str> = Vec::new();
                    while i < lines.len() {
                        let el = lines[i].trim_end();
                        if el.trim() == "\"\"\"" {
                            break;
                        }
                        body_lines.push(el);
                        i += 1;
                    }
                    emit_body = body_lines.join("\n");
                    i += 1; // skip closing """
                } else {
                    i += 1;
                }
            }

            rules.push(CodegenRule {
                match_shape,
                condition,
                emit_body,
                emit_to,
                emit_file,
                priority,
            });
        } else {
            i += 1;
        }
    }

    CodegenTemplate {
        target: target.to_string(),
        layer: layer_name.to_string(),
        rules,
        scaffold,
    }
}

/// Build a serializable palette (constructs + statements with visuals) for the viewer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaletteEntry {
    pub name: String,
    pub keyword: String,
    pub kind: String,
    pub shape: String,
    pub icon: String,
    pub color: String,
    pub label: String,
    /// Domain description from the layer `desc` line — shown in palette/property UI.
    #[serde(default)]
    pub description: String,
    pub group: String,
    pub allowed_in: String,
    pub layer: String,
    /// "construct" or "statement"
    pub entry_type: String,
    /// Layer-declared annotations available on this construct (empty for
    /// statements). The viewer offers these in the property editor.
    #[serde(default)]
    /// Whether constructs of this kind are deployment unit boundaries.
    pub au: bool,
    pub annotations: Vec<AnnotationSpec>,
    /// Expected group names (from `requires_groups` constraint). The viewer
    /// shows these as tabs even if they don't have children yet.
    #[serde(default)]
    pub expected_groups: Vec<String>,
    /// Target construct name — for impl-shaped constructs, the trait-shaped
    /// construct they implement. The viewer shows a button on the target.
    #[serde(default)]
    pub tgt: String,
    /// Default group — where this construct should be created by default.
    #[serde(default)]
    pub dg: String,
    /// For step-type constructs: the config field schema from layer `has`.
    /// Each entry is (field_name, type_hint) e.g. ("port", "Str").
    /// The viewer uses this to render per-type property editors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub has_fields: Vec<(String, String)>,
    /// Structured field specs with meta-type info for context-aware editors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub step_fields: Vec<StepFieldSpec>,
    /// Whether this is a step-type construct (mt step).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_step: bool,
}

pub fn palette_from_registry(reg: &LayerRegistry) -> Vec<PaletteEntry> {
    let mut out = Vec::new();
    for c in &reg.constructs {
        // Always include core type primitives (enum, struct, trait, …).
        // Domain layers (e.g. ddd) deliberately leave `enum` as base; the IDE
        // still needs their visual entries so icons/labels resolve. Create menus
        // may de-emphasize core types, but styles must never be dropped.
        out.push(PaletteEntry {
            name: c.name.clone(),
            keyword: c.keyword.clone(),
            kind: shape_to_node_kind(c.shape).to_string(),
            shape: c.shape.name().to_string(),
            icon: c.visual.icon.clone(),
            color: c.visual.color.clone(),
            label: c.visual.label.clone(),
            description: c.desc.clone(),
            group: c.group.clone(),
            allowed_in: c.allowed_in.clone(),
            layer: c.layer.clone(),
            entry_type: "construct".to_string(),
            au: false,
            annotations: c.annotations.clone(),
            expected_groups: c.constraints.iter()
                .find(|cst| cst.starts_with("requires_groups"))
                .map(|cst| {
                    cst.strip_prefix("requires_groups")
                        .unwrap_or("")
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            tgt: c.tgt.clone(),
            dg: c.dg.clone(),
            has_fields: c.contains.iter()
                .filter_map(|entry| {
                    entry.split_once(':').map(|(name, type_hint)| {
                        (name.trim().to_string(), type_hint.trim().to_string())
                    })
                })
                .collect(),
            step_fields: c.step_fields.clone(),
            is_step: c.is_step,
        });
    }
    for s in &reg.statements {
        out.push(PaletteEntry {
            name: s.keyword.clone(),
            keyword: s.keyword.clone(),
            kind: "Action".to_string(),
            shape: s.shape.name().to_string(),
            icon: s.visual.icon.clone(),
            color: s.visual.color.clone(),
            label: s.visual.label.clone(),
            description: s.desc.clone(),
            group: String::new(),
            allowed_in: "Step".to_string(),
            layer: s.layer.clone(),
            entry_type: "statement".to_string(),
            au: false,
            annotations: Vec::new(),
            expected_groups: Vec::new(),
            tgt: String::new(),
            dg: String::new(),
            has_fields: Vec::new(),
            step_fields: Vec::new(),
            is_step: false,
        });
    }
    out
}

/// Map a core shape to the IR NodeKind name used by the viewer.
pub fn shape_to_node_kind(shape: Shape) -> &'static str {
    match shape {
        Shape::Mod => "Module",
        Shape::Struct => "TypeDef",
        Shape::Enum => "TypeDef",
        Shape::Trait => "Interface",
        Shape::Impl => "Implementation",
        Shape::Fn => "Flow",
        Shape::Group => "Group",
    }
}

/// Parse UI-framework reactivity emission forms from a layer (e.g. svelte5):
/// ```text
/// reactivity_policy
///   props_call $props()
///   state_line let {name}: {type} = $state({default});
///   derived_line let {name} = $derived({expr});
///   effect_sync $effect(() => { // {name}
/// {body}
///   });
///   bindable $bindable()
///   bindable_default $bindable({default})
/// ```
pub fn parse_reactivity_policy(content: &str) -> Option<ReactivityPolicy> {
    let mut in_block = false;
    let mut pol = ReactivityPolicy::default();
    let mut found = false;
    let mut multiline_key: Option<String> = None;
    let mut multiline_buf = String::new();

    let keys = [
        "props_call",
        "state_line",
        "derived_line",
        "effect_sync",
        "effect_async",
        "bindable",
        "bindable_default",
    ];

    let flush = |pol: &mut ReactivityPolicy, key: &str, val: String| {
        let v = val.trim_end().to_string();
        match key {
            "props_call" => pol.props_call = v,
            "state_line" => pol.state_line = v,
            "derived_line" => pol.derived_line = v,
            "effect_sync" => pol.effect_sync = v,
            "effect_async" => pol.effect_async = v,
            "bindable" => pol.bindable = v,
            "bindable_default" => pol.bindable_default = v,
            _ => {}
        }
    };

    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            if multiline_key.is_some() && in_block {
                multiline_buf.push('\n');
            }
            continue;
        }
        if t == "reactivity_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        // Leave block on de-dent to a non-policy top-level line
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !keys.iter().any(|k| t.starts_with(k))
            && multiline_key.is_none()
        {
            break;
        }

        // Continuation of a multi-line value (effect bodies)
        if let Some(ref key) = multiline_key {
            let is_new_key = keys.iter().any(|k| t.starts_with(&format!("{k} ")));
            if is_new_key {
                flush(&mut pol, key, std::mem::take(&mut multiline_buf));
                multiline_key = None;
            } else {
                if !multiline_buf.is_empty() {
                    multiline_buf.push('\n');
                }
                // Preserve relative indent inside the policy block (strip one level)
                let stripped = line
                    .strip_prefix("    ")
                    .or_else(|| line.strip_prefix("\t"))
                    .unwrap_or(line);
                multiline_buf.push_str(stripped);
                continue;
            }
        }

        for k in keys {
            let prefix = format!("{k} ");
            if let Some(rest) = t.strip_prefix(&prefix) {
                if k.starts_with("effect_") {
                    multiline_key = Some(k.to_string());
                    multiline_buf = rest.to_string();
                } else {
                    flush(&mut pol, k, rest.to_string());
                }
                break;
            }
        }
    }
    if let Some(key) = multiline_key {
        flush(&mut pol, &key, multiline_buf);
    }

    if found {
        Some(pol)
    } else {
        None
    }
}

/// Parse PR Wizard `review` presentation block from a layer:
/// ```text
/// review
///   strategy component_sandbox
///   target svelte5
///   fallback structural
///   secondary file_diff
///   impact dependents
/// ```
/// `secondary` and `impact` accept comma- or space-separated values; multiple
/// lines of the same key append.
pub fn parse_review_policy(content: &str) -> Option<ReviewPolicy> {
    let mut in_block = false;
    let mut pol = ReviewPolicy::default();
    let mut found = false;
    let keys = [
        "strategy",
        "target",
        "fallback",
        "secondary",
        "impact",
    ];
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "review" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        // Leave block on column-0 non-key line.
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !keys.iter().any(|k| t.starts_with(k))
        {
            break;
        }
        if let Some(rest) = t.strip_prefix("strategy ") {
            pol.strategy = rest.trim().to_string();
        } else if let Some(rest) = t.strip_prefix("target ") {
            let v = rest.trim().to_string();
            if !v.is_empty() {
                pol.target = Some(v);
            }
        } else if let Some(rest) = t.strip_prefix("fallback ") {
            let v = rest.trim().to_string();
            if !v.is_empty() {
                pol.fallback = Some(v);
            }
        } else if let Some(rest) = t.strip_prefix("secondary ") {
            for part in rest.split(|c: char| c == ',' || c.is_whitespace()) {
                let p = part.trim();
                if !p.is_empty() && !pol.secondary.iter().any(|s| s == p) {
                    pol.secondary.push(p.to_string());
                }
            }
        } else if let Some(rest) = t.strip_prefix("impact ") {
            for part in rest.split(|c: char| c == ',' || c.is_whitespace()) {
                let p = part.trim();
                if !p.is_empty() && !pol.impact.iter().any(|s| s == p) {
                    pol.impact.push(p.to_string());
                }
            }
        }
    }
    if found && !pol.strategy.is_empty() {
        if pol.fallback.is_none() {
            pol.fallback = Some("structural".into());
        }
        if pol.secondary.is_empty() {
            pol.secondary.push("file_diff".into());
        }
        if pol.impact.is_empty() {
            pol.impact.push("dependents".into());
        }
        Some(pol)
    } else if found {
        // `review` present but empty — still return structural default so layers
        // that only open the block are recognized.
        Some(ReviewPolicy::structural_default())
    } else {
        None
    }
}

/// Parse optional INV-002 constructor policy from layer source:
/// ```text
/// constructor_policy
///   auto_fields created, updated, created_at
///   type_default Int 0
///   type_default Bool false
/// ```
pub fn parse_constructor_policy(content: &str) -> Option<ConstructorPolicy> {
    let mut in_block = false;
    let mut pol = ConstructorPolicy::default();
    let mut found = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "constructor_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        // Leave block on de-dent to a top-level keyword-ish line without indent
        // (raw lines starting at column 0 that aren't policy keys).
        if !line.starts_with(' ') && !line.starts_with('\t') && !t.starts_with("auto_fields")
            && !t.starts_with("type_default")
        {
            break;
        }
        if let Some(rest) = t.strip_prefix("auto_fields ") {
            pol.auto_fields = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if let Some(rest) = t.strip_prefix("type_default ") {
            let mut parts = rest.splitn(2, char::is_whitespace);
            if let (Some(ty), Some(expr)) = (parts.next(), parts.next()) {
                pol.type_defaults
                    .push((ty.trim().to_string(), expr.trim().to_string()));
            }
        }
    }
    if found {
        Some(pol)
    } else {
        None
    }
}

/// Parse optional INV-006 identity policy:
/// ```text
/// identity_policy
///   ref_suffix _id
///   identity_field id
/// ```
pub fn parse_identity_policy(content: &str) -> Option<IdentityPolicy> {
    let mut in_block = false;
    let mut pol = IdentityPolicy::default();
    let mut found = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "identity_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !t.starts_with("ref_suffix")
            && !t.starts_with("identity_field")
        {
            break;
        }
        if let Some(rest) = t.strip_prefix("ref_suffix ") {
            pol.ref_suffix = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("identity_field ") {
            pol.identity_field = Some(rest.trim().to_string());
        }
    }
    if found {
        Some(pol)
    } else {
        None
    }
}

/// Parse optional `bus_policy` block from a layer file:
/// ```text
/// bus_policy
///   strip_name_prefix Handle
/// ```
pub fn parse_bus_policy(content: &str) -> Option<BusPolicy> {
    let mut in_block = false;
    let mut pol = BusPolicy::default();
    let mut found = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "bus_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !t.starts_with("strip_name_prefix")
        {
            break;
        }
        if let Some(rest) = t.strip_prefix("strip_name_prefix ") {
            let p = rest.trim();
            pol.strip_name_prefix = if p.is_empty() || p == "-" || p == "none" {
                None
            } else {
                Some(p.to_string())
            };
        }
    }
    if found {
        Some(pol)
    } else {
        None
    }
}

/// Parse optional `auth_policy` block:
/// ```text
/// auth_policy
///   service_trait AuthService
/// ```
pub fn parse_auth_policy(content: &str) -> Option<AuthPolicy> {
    let mut in_block = false;
    let mut pol = AuthPolicy::default();
    let mut found = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "auth_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !t.starts_with("service_trait")
        {
            break;
        }
        if let Some(rest) = t.strip_prefix("service_trait ") {
            let p = rest.trim();
            pol.service_trait = if p.is_empty() || p == "-" || p == "none" {
                None
            } else {
                Some(p.to_string())
            };
        }
    }
    if found {
        Some(pol)
    } else {
        None
    }
}

/// Parse optional `http_name_policy` block for name-derived REST.
pub fn parse_http_name_policy(content: &str) -> Option<HttpNamePolicy> {
    let mut in_block = false;
    let mut pol = HttpNamePolicy::default();
    let mut found = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "http_name_policy" {
            in_block = true;
            found = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if !line.starts_with(' ')
            && !line.starts_with('\t')
            && !t.starts_with("list_prefix")
            && !t.starts_with("get_prefix")
            && !t.starts_with("create_prefix")
            && !t.starts_with("update_prefix")
            && !t.starts_with("delete_prefix")
            && !t.starts_with("path_prefix")
        {
            break;
        }
        if let Some(rest) = t.strip_prefix("list_prefix ") {
            pol.list_prefix = Some(normalize_policy_string(rest.trim()));
        } else if let Some(rest) = t.strip_prefix("get_prefix ") {
            pol.get_prefix = Some(normalize_policy_string(rest.trim()));
        } else if let Some(rest) = t.strip_prefix("create_prefix ") {
            pol.create_prefix = Some(normalize_policy_string(rest.trim()));
        } else if let Some(rest) = t.strip_prefix("update_prefix ") {
            pol.update_prefix = Some(normalize_policy_string(rest.trim()));
        } else if let Some(rest) = t.strip_prefix("delete_prefix ") {
            pol.delete_prefix = Some(normalize_policy_string(rest.trim()));
        } else if let Some(rest) = t.strip_prefix("path_prefix ") {
            pol.path_prefix = Some(normalize_policy_string(rest.trim()));
        }
    }
    if found {
        Some(pol)
    } else {
        None
    }
}

/// Sentinel stored when a layer says `list_prefix none` (explicit clear).
const POLICY_CLEAR: &str = "\0clear";

fn strip_annotation_arg_quotes(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn normalize_policy_string(p: &str) -> String {
    let t = p.trim();
    if t.is_empty() || t == "-" || t.eq_ignore_ascii_case("none") {
        POLICY_CLEAR.to_string()
    } else {
        t.to_string()
    }
}

fn resolve_policy_opt(over: &Option<String>, base: &Option<String>) -> Option<String> {
    match over {
        None => base.clone(),
        Some(s) if s == POLICY_CLEAR => None,
        Some(s) => Some(s.clone()),
    }
}

fn parse_construct_roles(v: &str) -> Vec<String> {
    v.split(',')
        .flat_map(|s| s.split_whitespace())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn collect_constructs_with_role<'a>(
    reg: &'a LayerRegistry,
    c: &'a crate::ast::Construct,
    role: &str,
    out: &mut Vec<&'a crate::ast::Construct>,
) {
    if reg.construct_has_role(c, role) {
        out.push(c);
    }
    for child in &c.children {
        collect_constructs_with_role(reg, child, role, out);
    }
}

fn merge_http_name_policy(base: &HttpNamePolicy, over: &HttpNamePolicy) -> HttpNamePolicy {
    HttpNamePolicy {
        list_prefix: resolve_policy_opt(&over.list_prefix, &base.list_prefix),
        get_prefix: resolve_policy_opt(&over.get_prefix, &base.get_prefix),
        create_prefix: resolve_policy_opt(&over.create_prefix, &base.create_prefix),
        update_prefix: resolve_policy_opt(&over.update_prefix, &base.update_prefix),
        delete_prefix: resolve_policy_opt(&over.delete_prefix, &base.delete_prefix),
        path_prefix: resolve_policy_opt(&over.path_prefix, &base.path_prefix),
    }
}

/// Convenience: keyword→shape map for quick parser lookups.
pub fn keyword_shapes(reg: &LayerRegistry) -> HashMap<String, Shape> {
    reg.constructs
        .iter()
        .map(|c| (c.keyword.clone(), c.shape))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::presentation_from_registry;

    #[test]
    fn parse_review_policy_svelte_block() {
        let src = r#"
pkg svelte5 v1
  review
    strategy component_sandbox
    target svelte5
    fallback structural
    secondary file_diff
    impact dependents
  construct Page
    mt mod
"#;
        let pol = parse_review_policy(src).expect("review policy");
        assert_eq!(pol.strategy, "component_sandbox");
        assert_eq!(pol.target.as_deref(), Some("svelte5"));
        assert_eq!(pol.fallback.as_deref(), Some("structural"));
        assert!(pol.secondary.iter().any(|s| s == "file_diff"));
        assert!(pol.impact.iter().any(|s| s == "dependents"));
    }

    #[test]
    fn load_layer_installs_review_policy() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content(
            "svelte5",
            r#"
pkg svelte5 v1
  review
    strategy component_sandbox
    target svelte5
    fallback structural
    secondary file_diff
  construct App
    mt mod
"#,
        )
        .expect("load");
        let pol = reg.review_policies.get("svelte5").expect("policy");
        assert_eq!(pol.strategy, "component_sandbox");
    }

    #[test]
    fn dependency_role_from_di_layer() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("di", include_str!("../../../layers/di.layer"))
            .expect("di layer should load");
        assert!(
            reg.is_dependency_annotation("dep"),
            "di.layer must tag @dep with role:dependency"
        );
        assert!(!reg.is_dependency_annotation("invariant"));
        assert!(!reg.is_dependency_annotation("nope"));
    }

    #[test]
    fn statement_lowers_to_and_requires_dep_parse() {
        let src = r#"
pkg wf v1
  statement call_agent
    mt call
    desc "Invoke LLM"
    requires_dep LlmPort
    lowers_to
      rust: "self.{dep}.invoke({args}).await?"
      typescript: "await this.{dep}.invoke({args})"
    visual
      icon "🤖"
      label "Call Agent"
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("wf", src).expect("layer should load");
        let stmt = reg.statement("call_agent").expect("call_agent");
        assert_eq!(stmt.requires_dep.as_deref(), Some("LlmPort"));
        assert_eq!(
            stmt.lowers_to.get("rust").map(|s| s.as_str()),
            Some("self.{dep}.invoke({args}).await?")
        );
        assert_eq!(
            stmt.lowers_to.get("typescript").map(|s| s.as_str()),
            Some("await this.{dep}.invoke({args})")
        );
        assert_eq!(stmt.visual.icon, "🤖");
        assert_eq!(stmt.shape, StmtShape::Call);
    }

    #[test]
    fn ddd_statements_require_bus_dep() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        for kw in ["dispatch", "invoke", "request"] {
            let s = reg.statement(kw).unwrap_or_else(|| panic!("{kw}"));
            assert_eq!(
                s.requires_dep.as_deref(),
                Some("Bus"),
                "{kw} should require Bus"
            );
            // Empty lowers_to keeps envelope routing fallback
            assert!(s.lowers_to.is_empty(), "{kw} should not force lowers_to");
        }
    }

    #[test]
    fn layer_annotations_parse_and_reach_palette() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd layer should load");
        // The Aggregate construct declares an `invariant` annotation with an
        // `expr` param — parsed from the layer, not hardcoded anywhere.
        let agg = reg.constructs.iter().find(|c| c.name == "Aggregate").expect("Aggregate");
        let inv = agg.annotations.iter().find(|a| a.name == "invariant").expect("invariant annotation");
        assert_eq!(inv.params, vec!["expr".to_string()]);
        assert!(!inv.desc.is_empty(), "annotation description should be preserved");

        // Palette carries the annotations for the viewer.
        let palette = palette_from_registry(&reg);
        let agg_entry = palette.iter().find(|e| e.name == "Aggregate").expect("Aggregate palette entry");
        assert!(agg_entry.annotations.iter().any(|a| a.name == "invariant"));
        // Statements carry no annotations.
        let dispatch = palette.iter().find(|e| e.name == "dispatch");
        if let Some(d) = dispatch {
            assert!(d.annotations.is_empty());
        }
    }

    /// Core type visuals stay on the palette when domain layers load (enum is base).
    #[test]
    fn palette_keeps_core_enum_icon_with_domain_layers() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd layer should load");
        let palette = palette_from_registry(&reg);
        let en = palette
            .iter()
            .find(|e| e.name == "Enum" || e.keyword == "enum")
            .expect("Enum must remain on palette for icon/label resolution");
        assert_eq!(en.icon, "🔀");
        assert_eq!(en.label, "Enum");
        // ddd → harness → base may tag Enum as "base"; core visual must remain.
        assert!(
            en.layer == "core" || en.layer == "base",
            "Enum layer should stay platform foundation, got {}",
            en.layer
        );
        // Struct etc. also stay (styles), even if create UI de-emphasizes them.
        assert!(
            palette.iter().any(|e| e.keyword == "struct" && e.icon == "📋"),
            "core Struct visual must remain"
        );
    }

    /// LAY-002: `present` blocks parse into ConstructSpec and API model.
    #[test]
    fn present_block_parses_views_and_roles() {
        let src = r#"
pkg demo v1
  construct Host
    kw host
    mt mod
    present
      view groups
        label "Layers"
        layout tabs
        members by_source_group
        tabs domain, application
        default
      view model
        label "Domain model"
        layout tree
        members by_host_children
        roots Aggregate
        nest Event under Aggregate when declared_in_parent
        orphan_policy list
  construct Aggregate
    kw agg
    mt struct
    present
      role container
      nestable_in model as root
  construct Event
    kw evt
    mt struct
    present
      role leaf
      nestable_in model under Aggregate
      lens critical
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("demo", src).expect("load demo layer");

        let host = reg.construct_by_name("Host").expect("Host");
        assert_eq!(host.presentation.views.len(), 2);
        let groups = &host.presentation.views[0];
        assert_eq!(groups.id, "groups");
        assert_eq!(groups.layout, "tabs");
        assert!(groups.is_default);
        assert_eq!(groups.tabs, vec!["domain", "application"]);
        let model = &host.presentation.views[1];
        assert_eq!(model.id, "model");
        assert_eq!(model.layout, "tree");
        assert_eq!(model.roots, vec!["Aggregate"]);
        assert_eq!(model.nest_rules.len(), 1);
        assert_eq!(model.nest_rules[0].child, "Event");
        assert_eq!(model.nest_rules[0].parent, "Aggregate");

        let agg = reg.construct_by_name("Aggregate").expect("Aggregate");
        assert_eq!(agg.presentation.role.as_deref(), Some("container"));

        let evt = reg.construct_by_name("Event").expect("Event");
        assert_eq!(evt.presentation.lenses, vec!["critical".to_string()]);

        let api = presentation_from_registry(&reg);
        assert_eq!(api.version, 1);
        let host_dto = api.hosts.get("Host").expect("Host in presentation API");
        assert_eq!(host_dto.default_view.as_deref(), Some("groups"));
        assert_eq!(host_dto.views.len(), 2);
        assert!(api.constructs.get("Event").unwrap().lenses.contains(&"critical".into()));
    }

    #[test]
    fn present_unknown_layout_fails_load() {
        let src = r#"
pkg bad v1
  construct Host
    kw host
    mt mod
    present
      view x
        layout not_a_layout
"#;
        let mut reg = LayerRegistry::builtin();
        let err = reg.load_content("bad", src).unwrap_err();
        assert!(
            err.contains("unknown layout") || err.contains("not_a_layout"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn present_unknown_construct_ref_fails_load() {
        let src = r#"
pkg bad v1
  construct Host
    kw host
    mt mod
    present
      view model
        layout tree
        roots NotARealConstruct
"#;
        let mut reg = LayerRegistry::builtin();
        let err = reg.load_content("bad", src).unwrap_err();
        assert!(
            err.contains("NotARealConstruct") || err.contains("unknown root"),
            "unexpected error: {err}"
        );
    }

    /// LAY-005: svelte5.layer has a different two-view shape than DDD.
    #[test]
    fn svelte5_layer_app_has_folders_and_routes_views() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("svelte5", include_str!("../../../layers/svelte5.layer"))
            .expect("svelte5.layer must load with present blocks");
        let app = reg.construct_by_name("App").expect("App");
        let ids: Vec<_> = app.presentation.views.iter().map(|v| v.id.as_str()).collect();
        assert!(ids.contains(&"groups"), "{ids:?}");
        assert!(ids.contains(&"routes"), "{ids:?}");
        // Distinct from DDD (no "model" / Aggregate roots)
        assert!(!ids.contains(&"model"));
        let groups = app.presentation.views.iter().find(|v| v.id == "groups").unwrap();
        assert_eq!(groups.layout, "tabs");
        assert_eq!(groups.tabs, vec!["pages", "components", "stores"]);
        let routes = app.presentation.views.iter().find(|v| v.id == "routes").unwrap();
        assert_eq!(routes.layout, "tree");
        assert!(routes.roots.contains(&"Layout".to_string()));
        assert!(routes.roots.contains(&"Page".to_string()));
        assert!(
            routes
                .nest_rules
                .iter()
                .any(|r| r.child == "Page" && r.parent == "Layout"),
            "{:?}",
            routes.nest_rules
        );
        let api = presentation_from_registry(&reg);
        assert!(api.hosts.contains_key("App"));
        assert!(!api.hosts.contains_key("Context")); // different paradigm host
        assert!(
            !reg.is_http_route_annotation("route"),
            "Svelte page @route must never gain role:http_route"
        );
        assert!(
            reg.is_ui_route_annotation("route"),
            "Svelte page @route must carry role:ui_route"
        );
    }

    /// LAY-004: real ddd.layer ships Context groups + model presentation.
    #[test]
    fn annotation_roles_from_ddd_and_di() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer")).unwrap();
        reg.load_content("di", include_str!("../../../layers/di.layer")).unwrap();
        assert!(reg.is_runtime_strategy_annotation("strategy"), "strategy role");
        assert!(
            !reg.is_http_route_annotation("route"),
            "API @route / role:http_route must be gone from ddd"
        );
        assert!(
            reg.constructs.iter().any(|c| c.roles.iter().any(|r| r == "http_endpoint")),
            "ddd use harness must install endpoint role: {:?}",
            reg.constructs.iter().map(|c| &c.keyword).collect::<Vec<_>>()
        );
        assert!(reg.is_main_annotation("main"), "main role");
        assert!(reg.is_adapter_field_annotation("field"), "field role");
        assert!(reg.is_adapter_env_annotation("env"), "env role");
        assert!(reg.is_invariant_annotation("invariant"), "invariant role");
        assert!(reg.is_secret_annotation("secret"), "secret role");
        assert!(reg.is_permission_annotation("auth"), "permission role");
        assert_eq!(reg.auth_policy.service_trait.as_deref(), Some("AuthService"));
        assert!(reg.bus_policy.strip_name_prefix.as_deref() == Some("Handle"));
        assert!(reg.http_name_policy.list_prefix.as_deref() == Some("List"));
        assert!(
            reg.layers.iter().any(|l| l == "auth_local"),
            "ddd must pull auth_local: {:?}",
            reg.layers
        );
        assert!(
            !reg.declarations.is_empty(),
            "declare block empty — policy lines may have broken layer parse"
        );
        let decl = reg.declarations.join("\n");
        assert!(decl.contains("trait Bus"), "Bus missing from declare: {decl}");
        assert!(decl.contains("run_saga"), "run_saga missing: {decl}");
    }

    #[test]
    fn rest_english_and_bus_handle_packs_load_via_ddd_use() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        assert!(
            reg.layers.iter().any(|l| l == "rest_english"),
            "ddd must pull rest_english: {:?}",
            reg.layers
        );
        assert!(
            reg.layers.iter().any(|l| l == "bus_handle"),
            "ddd must pull bus_handle: {:?}",
            reg.layers
        );
        assert_eq!(reg.http_name_policy.list_prefix.as_deref(), Some("List"));
        assert_eq!(reg.bus_policy.strip_name_prefix.as_deref(), Some("Handle"));
    }

    #[test]
    fn rest_rpc_clears_name_derived_prefixes() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("rest_english", include_str!("../../../layers/rest_english.layer"))
            .unwrap();
        reg.load_content("rest_rpc", include_str!("../../../layers/rest_rpc.layer"))
            .unwrap();
        assert!(
            reg.http_name_policy.list_prefix.is_none(),
            "rest_rpc must clear List: {:?}",
            reg.http_name_policy
        );
        assert!(reg.http_name_policy.path_prefix.is_none());
    }

    #[test]
    fn codegen_toml_overrides_layer_policy() {
        let dir = std::env::temp_dir().join(format!(
            "veil-codegen-ov-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "app"
[codegen]
bus_strip_prefix = "Cmd"
http_path_prefix = "/api/v2/"
http_list_prefix = "Fetch"
"#,
        )
        .unwrap();
        std::fs::write(dir.join("main.veil"), "pkg app\n  use ddd\n").unwrap();

        // Load ddd policies then apply project overrides (mirrors for_veil_file tail).
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .unwrap();
        let o = crate::deps::load_codegen_overrides(&dir)
            .unwrap()
            .expect("codegen");
        reg.apply_codegen_overrides(&o);
        assert_eq!(reg.bus_policy.strip_name_prefix.as_deref(), Some("Cmd"));
        assert_eq!(
            reg.http_name_policy.path_prefix.as_deref(),
            Some("/api/v2/")
        );
        assert_eq!(reg.http_name_policy.list_prefix.as_deref(), Some("Fetch"));
        // Unset keys leave layer values
        assert_eq!(reg.http_name_policy.get_prefix.as_deref(), Some("Get"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn harness_policy_loads_and_toml_overrides() {
        let src = r#"
pkg demo v1
  harness_policy
    profile axum_http
    cors localhost
    emit_bin on_entry
    provided_runtime_trait Clock
  construct HttpEndpoint
    kw endpoint
    mt struct
    role http_endpoint
    has
      method: ident
      path: path
      handle: ident
      bind: struct
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("demo", src).expect("load");
        assert_eq!(reg.harness_policy.profile.as_deref(), Some("axum_http"));
        assert_eq!(
            reg.harness_policy.cors,
            Some(crate::harness::CorsMode::Localhost)
        );
        assert!(reg
            .harness_policy
            .provided_runtime_traits
            .iter()
            .any(|t| t == "Clock"));
        let spec = reg.construct("endpoint").expect("endpoint");
        assert!(spec.roles.iter().any(|r| r == "http_endpoint"));
        assert!(spec.config_keys.iter().any(|k| k == "method"));
        assert!(spec.config_keys.iter().any(|k| k == "path"));
        assert!(spec.config_keys.iter().any(|k| k == "bind"));

        let c = crate::ast::Construct::new(
            "endpoint",
            "HttpEndpoint",
            Shape::Struct,
            "CreateItemHttp".into(),
            crate::span::Span::new(0, 0),
        );
        assert!(reg.construct_has_role(&c, "http_endpoint"));
        assert!(!reg.construct_has_role(&c, "deps_bundle"));
        assert!(reg.construct_config_keys(&c).contains(&"handle".to_string()));

        let dir = std::env::temp_dir().join(format!(
            "veil-harness-ov-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
[harness]
emit_bin = "never"
cors = "env"
health = "none"

[harness.wire]
item_repo = "PgItemRepo"
"#,
        )
        .unwrap();
        let o = crate::deps::load_harness_overrides(&dir)
            .unwrap()
            .expect("harness toml");
        reg.apply_harness_overrides(&o);
        assert_eq!(
            reg.harness_policy.emit_bin,
            Some(crate::harness::EmitBin::Never)
        );
        assert_eq!(
            reg.harness_policy.cors,
            Some(crate::harness::CorsMode::Env)
        );
        assert_eq!(reg.harness_policy.health, None);
        assert_eq!(
            reg.harness_policy.wire.get("item_repo").map(String::as_str),
            Some("PgItemRepo")
        );
        // Unset keys keep layer/defaults
        assert_eq!(reg.harness_policy.profile.as_deref(), Some("axum_http"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn construct_roles_comma_list() {
        let src = r#"
pkg demo v1
  construct Foo
    kw foo
    mt struct
    role deps_bundle, compose
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("demo", src).unwrap();
        let spec = reg.construct("foo").unwrap();
        assert_eq!(spec.roles, vec!["deps_bundle", "compose"]);
    }

    /// Regression: `prompt` then comments then `declare` must not swallow declarations
    /// (section leave only ran on fall-through; declare is matched first).
    #[test]
    fn prompt_then_declare_preserves_declarations() {
        let src = r#"
pkg demo v1
  construct X
    kw x
    mt struct
  prompt
    Some LLM prose.
    More prose with the word declare in it.
  # comment between sections
  declare
    trait DemoPort
      go() -> Res!
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("demo", src).expect("load");
        assert!(
            !reg.declarations.is_empty(),
            "declare after prompt must be collected"
        );
        let decl = reg.declarations.join("\n");
        assert!(
            decl.contains("trait DemoPort"),
            "DemoPort missing from declare: {decl}"
        );
    }

    #[test]
    fn ddd_layer_context_has_groups_and_model_views() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd.layer must load with present blocks");
        let ctx = reg.construct_by_name("Context").expect("Context");
        let ids: Vec<_> = ctx.presentation.views.iter().map(|v| v.id.as_str()).collect();
        assert!(ids.contains(&"groups"), "missing groups view: {ids:?}");
        assert!(ids.contains(&"model"), "missing model view: {ids:?}");
        let groups = ctx.presentation.views.iter().find(|v| v.id == "groups").unwrap();
        assert_eq!(groups.layout, "tabs");
        assert_eq!(
            groups.tabs,
            vec!["domain", "application", "infrastructure", "presentation"]
        );
        let model = ctx.presentation.views.iter().find(|v| v.id == "model").unwrap();
        assert_eq!(model.layout, "tree");
        // Domain model tree is the default Context outline (SvelteFlow reserved for control-flow).
        assert!(model.is_default);
        assert_eq!(model.roots, vec!["Aggregate"]);
        assert!(
            model
                .nest_rules
                .iter()
                .any(|r| r.child == "Event" && r.parent == "Aggregate"),
            "Event under Aggregate nest missing: {:?}",
            model.nest_rules
        );
        let api = presentation_from_registry(&reg);
        let host = api.hosts.get("Context").expect("Context in presentation API");
        assert_eq!(host.default_view.as_deref(), Some("model"));
        assert_eq!(host.views.len(), 2);
        let agg = reg.construct_by_name("Aggregate").unwrap();
        assert_eq!(agg.presentation.role.as_deref(), Some("container"));
    }

    /// Project `stubs/*.stub` load even without a matching `use reqwest` line.
    #[test]
    fn for_veil_file_auto_loads_project_stubs_dir() {
        let dir = std::env::temp_dir().join(format!(
            "veil-stub-autoload-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("stubs")).expect("mkdir stubs");
        std::fs::write(
            dir.join("main.veil"),
            "pkg auto_stub_test v1\n  use ddd\n  ctx C\n    group g\n",
        )
        .expect("write main.veil");
        std::fs::write(
            dir.join("stubs").join("reqwest.stub"),
            "stub reqwest 0.1.0\n  struct Client\n    fn post(url: Str) -> Response\n",
        )
        .expect("write reqwest.stub");
        // No `use reqwest` — only stubs/ on disk.
        let reg = LayerRegistry::for_veil_file(&dir.join("main.veil")).expect("registry");
        assert!(
            reg.stubs.iter().any(|s| s.name == "reqwest"),
            "expected reqwest auto-loaded from stubs/; got {:?}",
            reg.stubs.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// temporary - in tests module already closed, add at end of tests:
