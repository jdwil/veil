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
    /// Required fields declared by the layer via `has field_name: TypeName`.
    /// At check time, the engine validates that instances of this construct
    /// include these fields. The engine does NOT auto-generate them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_fields: Vec<(String, String)>,
    /// Per-target lowering templates (e.g. `"rust"` → template string).
    /// When present for a target, the backend uses this template INSTEAD of
    /// its default shape-based emission. Variables: `{{name}}`, `{{subkind}}`,
    /// `{{for field in fields}}...{{end}}`, `{{for method in methods}}...{{end}}`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub lowers_to: HashMap<String, String>,
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

// ─── Layer Pass System ───────────────────────────────────────────────────────

/// A pass declared by a layer: runs before or after the engine backend to
/// annotate AST nodes based on predicate rules. Extension mechanism — does NOT
/// replace the compiled engine backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassSpec {
    /// Pass name (e.g. "ownership", "async_marking").
    pub name: String,
    /// Execution priority — lower numbers run first.
    pub priority: u32,
    /// Whether this pass runs before or after the engine backend.
    pub phase: PassPhase,
    /// Rules within this pass (evaluated in order).
    pub rules: Vec<RuleSpec>,
    /// Which layer declared this pass.
    pub layer: String,
}

/// When a pass executes relative to the engine backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PassPhase {
    /// Runs before the engine backend — annotates AST for the backend to read.
    Pre,
    /// Runs after the engine backend — augments or transforms output.
    Post,
}

/// A single rule within a pass: when a predicate matches, apply actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSpec {
    /// Rule name (e.g. "last_use_moves", "multi_use_clones").
    pub name: String,
    /// Predicate expression evaluated against each node context.
    /// Example: `expr.kind == "ident" && expr.use_count == 1`
    pub when: String,
    /// Actions to apply when the predicate matches.
    pub actions: Vec<RuleAction>,
}

/// An action applied by a pass rule when its predicate matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleAction {
    /// Set an annotation key=value on the matched node.
    Annotate { key: String, value: String },
    /// Wrap the matched expression in a language construct.
    Wrap(WrapKind),
    /// Mark the node for removal from output.
    Remove,
}

/// Wrap operations for expression nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WrapKind {
    Clone,
    Borrow,
    MutBorrow,
    OptionalChain,
    Try,
    Await,
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
    /// Direct `use` edges recorded when a layer was loaded.
    /// Agent preambles walk this graph so unused layer docs stay out of context.
    pub layer_deps: HashMap<String, Vec<String>>,
    /// Layers the host loaded for this package without a matching `use` line
    /// (R21: veil.toml `[package].layer`). Teaching walks these as extra roots
    /// so product-layer prompts match what compile already loaded.
    pub implicit_uses: Vec<String>,
    /// Codegen templates declared by loaded layers.
    pub codegen_templates: Vec<CodegenTemplate>,
    /// Layer-declared passes (pre/post engine) for AST annotation.
    pub passes: Vec<PassSpec>,
    /// Loaded third-party crate stubs.
    pub stubs: Vec<StubCrate>,
    /// External layer resolver — called when a layer isn't found locally or in system.
    /// Provided by the hosting runtime (e.g. veil-runtime for database-backed resolution).
    pub external_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    /// External package source resolver — resolves `use X` package .veil content
    /// when not found on filesystem (DDB/S3 in deployed environments).
    pub source_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    /// External stub resolver — resolves `.stub` content by crate name when not
    /// found on local disk or system paths (DDB/S3 in deployed environments).
    pub stub_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
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
    /// Layer-declared error model (type name + variants). None = no error model declared.
    pub error_model: Option<ErrorModelPolicy>,
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
    /// Output type for the project: "bin" (default) or "cdylib" (shared library).
    /// Set from `veil.toml` `[codegen] output_type = "cdylib"`.
    pub output_type: Option<String>,
    /// Per-target lowering templates for declared trait/struct methods.
    /// Key: `(TypeName, MethodName)`, Value: `{ target → template }`.
    /// Populated from `declare` blocks with `lowers_to` on methods.
    /// Example: `("ApiClient", "fetch")` → `{ "typescript" → "..." }`.
    pub method_lowers_to: HashMap<(String, String), HashMap<String, String>>,
    /// Raw target-language code to emit into the shared crate.
    /// Each entry is `(target, code_template)`. Templates support `{error_type}` substitution.
    /// Populated from `shared_emit <target>` blocks in layer files.
    pub shared_emit: Vec<(String, String)>,
    /// Harness render templates per target. The engine interpolates HarnessTemplateData
    /// into these templates to produce framework-specific main.rs code.
    /// Populated from `harness_template <target>` blocks in layer files.
    /// Key: target (e.g. "rust_bin"), Value: template string.
    pub harness_render_templates: HashMap<String, String>,
    /// Library constructs from companion .veil files declared via `library` directives.
    /// Each entry is `(layer_name, veil_source)` — the raw source of the companion file.
    /// These are parsed and merged into the consuming solution at codegen time.
    pub library_constructs: Vec<(String, String)>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthPolicy {
    #[serde(default)]
    pub service_trait: Option<String>,
    /// Name of the generated mock impl struct (default: "AllowAllAuth").
    /// The engine generates a struct with this name that implements every method
    /// on the service_trait with Ok(default) returns.
    #[serde(default = "default_mock_impl_name")]
    pub mock_impl_name: String,
}

fn default_mock_impl_name() -> String { "AllowAllAuth".to_string() }

impl Default for AuthPolicy {
    fn default() -> Self {
        AuthPolicy {
            service_trait: None,
            mock_impl_name: default_mock_impl_name(),
        }
    }
}

/// Layer-declared error model: type name + variant names for domain errors.
/// Lets codegen emit `ErrorType::Variant(...)` without hardcoding names.
/// A layer (e.g. ddd.layer) declares: `error_model DomainError { external External, not_found NotFound, validation Validation }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorModelPolicy {
    /// The error type name (e.g. "DomainError", "AppError").
    pub type_name: String,
    /// Named variants: key = semantic role, value = variant name.
    /// Standard roles: "external", "not_found", "validation".
    /// Layers may declare additional variants.
    pub variants: Vec<(String, String)>,
}

impl ErrorModelPolicy {
    /// Get variant name by semantic role (e.g. "external" → "External").
    pub fn variant(&self, role: &str) -> Option<&str> {
        self.variants.iter().find(|(r, _)| r == role).map(|(_, v)| v.as_str())
    }

    /// Get full path for a variant role: `DomainError::External`.
    pub fn variant_path(&self, role: &str) -> Option<String> {
        self.variant(role).map(|v| format!("{}::{}", self.type_name, v))
    }
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
            layer_deps: HashMap::new(),
            implicit_uses: Vec::new(),
            codegen_templates: Vec::new(),
            passes: Vec::new(),
            stubs: Vec::new(),
            external_resolver: None,
            source_resolver: None,
            stub_resolver: None,
            constructor_policy: ConstructorPolicy::default(),
            reactivity_policy: ReactivityPolicy::default(),
            review_policies: HashMap::new(),
            identity_policy: IdentityPolicy::default(),
            bus_policy: BusPolicy::default(),
            auth_policy: AuthPolicy::default(),
            error_model: None,
            http_name_policy: HttpNamePolicy::default(),
            harness_policy: crate::harness::HarnessPolicy::documented_defaults(),
            extra_layer_roots: Vec::new(),
            codegen_http_from_toml: false,
            output_type: None,
            method_lowers_to: HashMap::new(),
            shared_emit: Vec::new(),
            harness_render_templates: HashMap::new(),
            library_constructs: Vec::new(),
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
            layer_deps: self.layer_deps.clone(),
            implicit_uses: self.implicit_uses.clone(),
            codegen_templates: self.codegen_templates.clone(),
            passes: self.passes.clone(),
            stubs: self.stubs.clone(),
            external_resolver: None, // resolver is not cloneable — cleared on clone
            source_resolver: None, // resolver is not cloneable — cleared on clone
            stub_resolver: None, // resolver is not cloneable — cleared on clone
            constructor_policy: self.constructor_policy.clone(),
            reactivity_policy: self.reactivity_policy.clone(),
            review_policies: self.review_policies.clone(),
            identity_policy: self.identity_policy.clone(),
            bus_policy: self.bus_policy.clone(),
            auth_policy: self.auth_policy.clone(),
            error_model: self.error_model.clone(),
            http_name_policy: self.http_name_policy.clone(),
            harness_policy: self.harness_policy.clone(),
            extra_layer_roots: self.extra_layer_roots.clone(),
            codegen_http_from_toml: self.codegen_http_from_toml,
            output_type: self.output_type.clone(),
            method_lowers_to: self.method_lowers_to.clone(),
            shared_emit: self.shared_emit.clone(),
            harness_render_templates: self.harness_render_templates.clone(),
            library_constructs: self.library_constructs.clone(),
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
            .field("stub_resolver", &self.stub_resolver.is_some())
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
                required_fields: Vec::new(),
                lowers_to: HashMap::new(),
            });
        }
        reg.layers.push("core".to_string());
        reg
    }

    /// Package `use` names plus [`Self::implicit_uses`], then the recorded `use` graph.
    /// Agent preambles and vocabulary filters use this so compile and teaching agree.
    pub fn teaching_closure<S: AsRef<str>>(
        &self,
        package_uses: impl IntoIterator<Item = S>,
    ) -> HashSet<String> {
        let mut roots: Vec<String> = package_uses
            .into_iter()
            .map(|s| s.as_ref().to_string())
            .collect();
        roots.extend(self.implicit_uses.iter().cloned());
        self.layer_use_closure(roots)
    }

    /// Layers named in `roots` plus every layer they `use` (recorded at load).
    pub fn layer_use_closure<S: AsRef<str>>(
        &self,
        roots: impl IntoIterator<Item = S>,
    ) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut stack: Vec<String> = roots.into_iter().map(|s| s.as_ref().to_string()).collect();
        while let Some(name) = stack.pop() {
            if !out.insert(name.clone()) {
                continue;
            }
            if let Some(deps) = self.layer_deps.get(&name) {
                stack.extend(deps.iter().cloned());
            }
        }
        out
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

    /// Union of policy roles declared on annotation `name` across loaded layers.
    pub fn annotation_roles(&self, name: &str) -> Vec<String> {
        let mut roles = Vec::new();
        for c in &self.constructs {
            for a in &c.annotations {
                if a.name != name {
                    continue;
                }
                for r in &a.roles {
                    if !roles.iter().any(|x| x == r) {
                        roles.push(r.clone());
                    }
                }
            }
        }
        roles
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

    /// Type names injected by layer `declare` blocks (`trait AuthService`, `struct Principal`, …).
    /// Product constructs must not reuse these names (they already exist).
    pub fn declared_type_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for decl in &self.declarations {
            for line in decl.lines() {
                let t = line.trim();
                for prefix in ["trait ", "struct ", "enum ", "port "] {
                    if let Some(rest) = t.strip_prefix(prefix) {
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            names.insert(name);
                        }
                    }
                }
            }
        }
        names
    }

    /// Free function names injected by layer `declare` blocks (`run_saga`, …).
    pub fn declared_fn_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for decl in &self.declarations {
            for line in decl.lines() {
                let t = line.trim();
                let rest = t.strip_prefix("fn ")
                    .or_else(|| t.strip_prefix("async fn "));
                if let Some(rest) = rest {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() && !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
        }
        names
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
        if let Some(v) = &o.output_type {
            self.output_type = Some(v.clone());
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

    /// Layer-declared lowering template for a construct (by target).
    /// When present, the backend uses this template INSTEAD of default emission.
    pub fn construct_lowers_to(&self, c: &crate::ast::Construct, target: &str) -> Option<&str> {
        self.spec_for_construct(c)
            .and_then(|spec| spec.lowers_to.get(target))
            .map(|s| s.as_str())
    }

    /// Get the lowering template for a declared type's method.
    /// Returns `None` if no `lowers_to` was declared for this (type, method, target).
    pub fn method_lowers_to_template(&self, type_name: &str, method: &str, target: &str) -> Option<&str> {
        self.method_lowers_to
            .get(&(type_name.to_string(), method.to_string()))
            .and_then(|targets| targets.get(target))
            .map(|s| s.as_str())
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

    /// True if a keyword/subkind (or its construct-chain ancestors) belongs to
    /// the given group (e.g. "domain", "application"). Resolves transitively via
    /// `maps_to`, so a product keyword that inherits from DomainService (group
    /// "domain") will return true for group "domain".
    pub fn construct_in_group(&self, keyword: &str, group: &str) -> bool {
        let Some(mut current) = self.construct(keyword) else {
            return false;
        };
        let mut visited: HashSet<&str> = HashSet::new();
        loop {
            if current.group.eq_ignore_ascii_case(group) {
                return true;
            }
            if !visited.insert(&current.keyword) {
                return false;
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
            return Ok(()); // already loaded or claimed this walk
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

        // Claim before walking `use` so A→B→A cannot recurse. Unclaim if parse/merge
        // fails so a later retry is not treated as a successful load.
        self.layers.push(name.to_string());
        let deps = collect_layer_use_names(&content);
        self.layer_deps.insert(name.to_string(), deps.clone());

        // Load dependency layers (`use xxx` at pkg level).
        // Skip silently if not found — it might be a .stub or package reference.
        for dep in &deps {
            let _ = self.load_layer(dep, dir);
        }

        let raw = parse_layer_file(&content, name).map_err(|e| {
            self.layers.retain(|l| l != name);
            self.layer_deps.remove(name);
            format!("layer '{}': {}", name, e)
        })?;
        let library_file = raw.library.clone();
        if let Err(e) = self.merge_and_resolve(raw) {
            self.layers.retain(|l| l != name);
            self.layer_deps.remove(name);
            return Err(e);
        }
        // Library companion: resolve the .veil file and store its source for
        // later injection into consuming solutions.
        if let Some(ref lib_path) = library_file {
            self.load_library_companion(name, lib_path, dir);
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
        if let Some(em) = parse_error_model(&content) {
            self.error_model = Some(em);
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

        // 1e. VEIL_LIBRARY_PATH: colon-separated dirs containing library projects
        if let Ok(lib_path_env) = std::env::var("VEIL_LIBRARY_PATH") {
            let separator = if cfg!(windows) { ';' } else { ':' };
            for root in lib_path_env.split(separator) {
                let root = std::path::Path::new(root.trim());
                if !root.is_dir() {
                    continue;
                }
                if let Some(content) = Self::load_layer_from_product_root(name, root) {
                    return Ok(content);
                }
            }
        }

        // 1f. Registered resolution points (VEIL_SEARCH_PATHS) — Spec 4.
        // AFTER project/[dependencies]/library-path (local always wins), BEFORE
        // the external (DDB/S3) resolver. Each root may be a workspace repo.
        for root in Self::search_path_roots() {
            if let Some(content) = Self::load_layer_from_search_root(name, &root) {
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

    /// Registered resolution-point roots from `VEIL_SEARCH_PATHS`.
    ///
    /// Colon/semicolon/newline separated, each entry `/abs/path` or
    /// `name=/abs/path`. Empty/blank entries are skipped. Only existing
    /// directories are returned. Order preserved (deterministic — no ambient
    /// `$HOME`/`/tmp` scanning). The `name=` prefix is a display id only; it
    /// does NOT constrain which `use <name>` resolves against the root.
    ///
    /// Spec: registry-repo-structure-04-search-path-settings.md.
    /// Populated by veil-server `search_fs::export_env` (in-process) or the
    /// shell env (child `veil` CLI process).
    fn search_path_roots() -> Vec<PathBuf> {
        let Ok(raw) = std::env::var("VEIL_SEARCH_PATHS") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for part in raw.split([':', ';', '\n']) {
            let entry = part.trim().trim_matches('"');
            if entry.is_empty() {
                continue;
            }
            // Strip optional `name=` prefix (id is display-only here).
            let path_str = match entry.split_once('=') {
                Some((name, path))
                    if !name.trim().is_empty()
                        && !name.contains('/')
                        && !name.contains('\\')
                        && !path.trim().is_empty() =>
                {
                    path.trim()
                }
                _ => entry,
            };
            let root = Path::new(path_str);
            if root.is_dir() {
                out.push(root.to_path_buf());
            }
        }
        out
    }

    /// Resolve `use <name>` layer content from a single search-path root.
    ///
    /// Deterministic lookup order (a search-path root may be a multi-project
    /// workspace repo OR a single project):
    ///   1. `<root>/<name>/layers/<name>.layer`  (workspace member subdir)
    ///   2. `<root>/<name>/<name>.layer`          (workspace member subdir, flat)
    ///   3. `<root>/layers/<name>.layer`          (root-level layers dir)
    ///   4. `<root>/<name>.layer`                 (root-level flat)
    /// Steps 3-4 also honor the root's own `veil.toml [package] provides_use`
    /// via `layer_source_in_root`.
    fn load_layer_from_search_root(name: &str, root: &Path) -> Option<String> {
        let member = root.join(name);
        for rel in [
            member.join("layers").join(format!("{name}.layer")),
            member.join(format!("{name}.layer")),
        ] {
            if rel.is_file() {
                if let Ok(s) = std::fs::read_to_string(&rel) {
                    return Some(s);
                }
            }
        }
        // Root-level (also checks veil.toml [package] provides_use).
        if let Some(s) = Self::load_layer_from_product_root(name, root) {
            return Some(s);
        }
        None
    }

    /// Resolve a companion `.veil` (library) relative path from a search root.
    /// Checks `<root>/<lib_path>` and each workspace member `<root>/<m>/<lib_path>`.
    fn load_library_from_search_root(root: &Path, lib_path: &str) -> Option<String> {
        let direct = root.join(lib_path);
        if direct.is_file() {
            if let Ok(s) = std::fs::read_to_string(&direct) {
                return Some(s);
            }
        }
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut members: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            members.sort_by_key(|e| e.file_name());
            for ent in members {
                let p = ent.path();
                if !p.is_dir() {
                    continue;
                }
                let candidate = p.join(lib_path);
                if candidate.is_file() {
                    if let Ok(s) = std::fs::read_to_string(&candidate) {
                        return Some(s);
                    }
                }
            }
        }
        None
    }

    /// Resolve a `.stub` by crate name from a search root.
    /// Checks `<root>/stubs/<name>.stub`, `<root>/<name>.stub`, and each
    /// workspace member's `stubs/` dir. Honors dashed/underscored stem variants.
    fn find_stub_in_search_root(name: &str, root: &Path) -> Option<PathBuf> {
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
                if p.is_file() {
                    return Some(p);
                }
            }
            None
        };
        if let Some(p) = try_dir(&root.join("stubs")) {
            return Some(p);
        }
        if let Some(p) = try_dir(root) {
            return Some(p);
        }
        if let Ok(rd) = std::fs::read_dir(root) {
            let mut members: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            members.sort_by_key(|e| e.file_name());
            for ent in members {
                let p = ent.path();
                if !p.is_dir() {
                    continue;
                }
                if let Some(hit) = try_dir(&p.join("stubs")) {
                    return Some(hit);
                }
            }
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
        // Claim before walking `use` (same as load_layer).
        self.layers.push(name.to_string());
        let deps = collect_layer_use_names(content);
        self.layer_deps.insert(name.to_string(), deps.clone());
        // Resolve `use` deps first (policy packs, foundations).
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for dep in &deps {
            let _ = self.load_layer(dep, &cwd);
        }
        let raw = parse_layer_file(content, name).map_err(|e| {
            self.layers.retain(|l| l != name);
            self.layer_deps.remove(name);
            format!("layer '{}': {}", name, e)
        })?;
        let library_file = raw.library.clone();
        if let Err(e) = self.merge_and_resolve(raw) {
            self.layers.retain(|l| l != name);
            self.layer_deps.remove(name);
            return Err(e);
        }
        // Library companion: resolve the .veil file and store its source.
        if let Some(ref lib_path) = library_file {
            self.load_library_companion(name, lib_path, &cwd);
        }
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
        if let Some(em) = parse_error_model(content) {
            self.error_model = Some(em);
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

    /// Load the companion .veil implementation file declared by a library layer.
    ///
    /// Resolution order:
    /// 1. Relative to the layer file's directory on disk
    /// 2. Via the external source resolver (runtime/S3)
    /// 3. Via VEIL_LIBRARY_PATH directories
    ///
    /// The raw source is stored; parsing + injection happens in the consumer's
    /// pipeline (see `inject_library_constructs`).
    fn load_library_companion(&mut self, layer_name: &str, lib_path: &str, dir: &Path) {
        // Already loaded a companion for this layer — skip.
        if self.library_constructs.iter().any(|(n, _)| n == layer_name) {
            return;
        }

        // 1. Try filesystem relative to layer location
        let candidate = dir.join(lib_path);
        if candidate.is_file() {
            if let Ok(source) = std::fs::read_to_string(&candidate) {
                self.library_constructs.push((layer_name.to_string(), source));
                return;
            }
        }
        // Also check layers/ subdir (layer might be at layers/<name>.layer, companion at layers/../main.veil)
        let parent_candidate = dir.join("..").join(lib_path);
        if parent_candidate.is_file() {
            if let Ok(source) = std::fs::read_to_string(&parent_candidate) {
                self.library_constructs.push((layer_name.to_string(), source));
                return;
            }
        }

        // 2. External source resolver (runtime provides package source from S3/DDB)
        if let Some(resolver) = &self.source_resolver {
            // Convention: resolver key for library companion is "layer_name/lib_path"
            let key = format!("{}/{}", layer_name, lib_path);
            if let Some(source) = resolver(&key) {
                self.library_constructs.push((layer_name.to_string(), source));
                return;
            }
        }

        // 3. VEIL_LIBRARY_PATH: colon-separated dirs containing library projects
        if let Ok(lib_path_env) = std::env::var("VEIL_LIBRARY_PATH") {
            let separator = if cfg!(windows) { ';' } else { ':' };
            for root in lib_path_env.split(separator) {
                let root = Path::new(root.trim());
                if !root.is_dir() {
                    continue;
                }
                // Look for <root>/layers/<layer_name>.layer sibling to <root>/<lib_path>
                let companion = root.join(lib_path);
                if companion.is_file() {
                    if let Ok(source) = std::fs::read_to_string(&companion) {
                        self.library_constructs.push((layer_name.to_string(), source));
                        return;
                    }
                }
            }
        }

        // 4. extra_layer_roots (from veil.toml [dependencies])
        for root in &self.extra_layer_roots.clone() {
            let companion = root.join(lib_path);
            if companion.is_file() {
                if let Ok(source) = std::fs::read_to_string(&companion) {
                    self.library_constructs.push((layer_name.to_string(), source));
                    return;
                }
            }
        }

        // 5. Registered resolution points (VEIL_SEARCH_PATHS) — Spec 4.
        for root in Self::search_path_roots() {
            if let Some(source) = Self::load_library_from_search_root(&root, lib_path) {
                self.library_constructs.push((layer_name.to_string(), source));
                return;
            }
        }

        // Not found — silently skip (library is optional / may only be available in runtime)
    }

    /// Build a registry for a `.veil` file: built-ins plus every layer the
    /// file references via `use` lines. Layer resolution is transitive.
    pub fn for_veil_file(veil_path: &Path) -> Result<Self, String> {
        Self::for_veil_file_with_resolvers(veil_path, None, None, None)
    }

    /// Build a registry with optional external resolvers for deployed environments.
    ///
    /// - `layer_resolver`: called when a layer isn't found on disk (e.g. DDB lookup)
    /// - `pkg_source_resolver`: called to get package .veil source for cross-package deps
    /// - `stub_resolver`: called to get .stub content by crate name when not found on disk
    pub fn for_veil_file_with_resolvers(
        veil_path: &Path,
        layer_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
        pkg_source_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
        stub_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    ) -> Result<Self, String> {
        let mut reg = LayerRegistry::builtin();
        reg.external_resolver = layer_resolver;
        reg.source_resolver = pkg_source_resolver;
        reg.stub_resolver = stub_resolver;
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
                // runtime/src/stubs next to layers) → runtime (DDB/S3).
                let stub_path = dir.join(format!("{}.stub", name));
                let stub_subdir_path = dir.join("stubs").join(format!("{}.stub", name));
                let found_stub = if stub_path.exists() {
                    Some(stub_path)
                } else if stub_subdir_path.exists() {
                    Some(stub_subdir_path)
                } else {
                    Self::find_system_stub(name)
                        // Search paths (VEIL_SEARCH_PATHS) — Spec 4: after
                        // local/system, before the external (DDB/S3) resolver.
                        .or_else(|| {
                            Self::search_path_roots()
                                .iter()
                                .find_map(|root| Self::find_stub_in_search_root(name, root))
                        })
                };
                if let Some(path) = found_stub {
                    if let Ok(stub_content) = std::fs::read_to_string(&path) {
                        if let Some(mut stub) = parse_stub_file(&stub_content) {
                            stub.alias = alias;
                            reg.stubs.push(stub);
                        }
                    }
                } else if let Some(resolver) = &reg.stub_resolver {
                    // Runtime fallback: resolve stub content from DDB/S3
                    if let Some(stub_content) = resolver(name) {
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
        Self::fill_stub_gaps_from_system(&mut reg);
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
                let name = entry.use_name.clone();
                if !reg.layers.iter().any(|l| l == &name) {
                    let _ = reg.load_layer(&name, dir);
                }
                // Teaching must include the primary product layer even when the
                // package omitted `use <name>` — compile already loaded it (R21).
                if reg.layers.iter().any(|l| l == &name)
                    && !reg.implicit_uses.iter().any(|l| l == &name)
                {
                    reg.implicit_uses.push(name);
                }
            }
        }
        // Product veil.toml [codegen] / [harness] win over layer policies (INV-001).
        if let Some(o) = crate::deps::load_codegen_overrides_for(veil_path) {
            reg.apply_codegen_overrides(&o);
        }
        if let Some(h) = crate::deps::load_harness_overrides_for(veil_path) {
            reg.apply_harness_overrides(&h);
        }
        // Developer/edit mode (preview windows): auto-inject the `developer`
        // layer so HTML/CSS-lowering codegen stamps provenance + injects the
        // overlay. Gated by env so a normal build/deploy never ships it.
        // Generic — no product opt-in, no per-project hardcoding.
        if crate::platform_layers::developer_mode_enabled()
            && !reg.layers.iter().any(|l| l == "developer")
        {
            let _ = reg.load_layer("developer", dir);
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

    /// Rustdoc dumps often mention types in signatures (`fn payload(input: Blob)`)
    /// without declaring them. If a curated system stub for the same crate
    /// defines that type, fold the missing struct in so check/codegen see the
    /// real contract. Never overwrites a type the product stub already has.
    ///
    /// Also inherit *unset* codegen policy from the system stub: `types_module`,
    /// `root_types`, and per-struct `path`. Product stubs that omit those
    /// headers still get `crate::types::T` / `crate::primitives::Blob`.
    fn fill_stub_gaps_from_system(reg: &mut LayerRegistry) {
        let mut additions: Vec<(usize, StubStruct)> = Vec::new();
        let mut types_module_fill: Vec<(usize, String)> = Vec::new();
        let mut root_types_fill: Vec<(usize, Vec<String>)> = Vec::new();
        let mut path_fill: Vec<(usize, String, String)> = Vec::new();
        for (i, stub) in reg.stubs.iter().enumerate() {
            let Some(path) = Self::find_system_stub(&stub.name) else {
                continue;
            };
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(sys) = parse_stub_file(&content) else {
                continue;
            };
            if stub.types_module.is_none() {
                if let Some(tm) = sys.types_module.clone() {
                    types_module_fill.push((i, tm));
                }
            }
            if stub.root_types.is_empty() && !sys.root_types.is_empty() {
                root_types_fill.push((i, sys.root_types.clone()));
            }
            for s in &sys.structs {
                if let Some(mp) = &s.module_path {
                    if stub
                        .structs
                        .iter()
                        .any(|p| p.name == s.name && p.module_path.is_none())
                    {
                        path_fill.push((i, s.name.clone(), mp.clone()));
                    }
                }
            }
            let have: HashSet<&str> = stub.structs.iter().map(|s| s.name.as_str()).collect();
            let needed = stub_referenced_type_names(stub);
            for s in sys.structs {
                if !have.contains(s.name.as_str()) && needed.contains(&s.name) {
                    additions.push((i, s));
                }
            }
        }
        for (i, tm) in types_module_fill {
            if let Some(stub) = reg.stubs.get_mut(i) {
                if stub.types_module.is_none() {
                    stub.types_module = Some(tm);
                }
            }
        }
        for (i, rt) in root_types_fill {
            if let Some(stub) = reg.stubs.get_mut(i) {
                if stub.root_types.is_empty() {
                    stub.root_types = rt;
                }
            }
        }
        for (i, name, mp) in path_fill {
            if let Some(stub) = reg.stubs.get_mut(i) {
                if let Some(s) = stub.structs.iter_mut().find(|s| s.name == name) {
                    if s.module_path.is_none() {
                        s.module_path = Some(mp);
                    }
                }
            }
        }
        for (i, s) in additions {
            if let Some(stub) = reg.stubs.get_mut(i) {
                if !stub.structs.iter().any(|x| x.name == s.name) {
                    stub.structs.push(s);
                }
            }
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
        // Next to system layers: VEIL_LAYERS_DIR/../stubs
        if let Ok(layers) = std::env::var("VEIL_LAYERS_DIR") {
            if let Some(p) = try_dir(&Path::new(&layers).join("../stubs")) {
                return Some(p);
            }
            if let Some(p) = try_dir(&Path::new(&layers).join("../examples")) {
                return Some(p);
            }
        }
        // Walk CWD ancestors for stubs/ and examples/
        if let Ok(cwd) = std::env::current_dir() {
            for anc in cwd.ancestors() {
                for rel in ["stubs", "examples"] {
                    if let Some(p) = try_dir(&anc.join(rel)) {
                        return Some(p);
                    }
                }
            }
        }
        // Relative to executable (installed layout)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                for rel in ["stubs", "../stubs"] {
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
            stmt.port_target = target.clone();
            stmt.port_method = method.clone();
            // If statement has port_target + lowers_to, register as method_lowers_to
            // so that `trait.method(...)` call syntax also uses the layer template.
            if let (Some(port), Some(meth)) = (target.as_ref(), method.as_ref()) {
                if !stmt.lowers_to.is_empty() {
                    self.method_lowers_to
                        .entry((port.to_string(), meth.to_string()))
                        .or_default()
                        .extend(stmt.lowers_to.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
            }
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

        // Accumulate layer passes.
        for pass in raw.passes {
            self.passes.push(pass);
        }

        // Accumulate declared method lowering templates.
        for (key, targets) in raw.method_lowers_to {
            self.method_lowers_to.entry(key).or_default().extend(targets);
        }

        // Accumulate shared_emit blocks from layers.
        for entry in raw.shared_emit {
            self.shared_emit.push(entry);
        }

        // Accumulate harness_render_templates from layers (last loaded wins for same target).
        for (target, template) in raw.harness_render_templates {
            self.harness_render_templates.insert(target, template);
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
    /// Method names that are always async on this crate's types (e.g. `send`, `send_with`).
    /// Line form: `async_methods send, send_with`.
    /// When a bang method (e.g. `.send!()`) is in this list, it emits `.await` before `?`.
    /// Methods NOT in this list with bang emit just `.map_err(...)?` (no `.await`).
    #[serde(default)]
    pub async_methods: Vec<String>,
    /// Field names that require borrow (`&self.field`) instead of clone.
    /// Line form: `borrow_fields pool`.
    /// Used when the type requires `&T` for trait impls (e.g. sqlx Executor for &Pool).
    #[serde(default)]
    pub borrow_fields: Vec<String>,
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
                // Rustdoc paths point at the private definition module
                // (`_`-prefixed leaf, e.g. `types::_message`), but SDKs re-export
                // the type publicly one level up (`types::Message`). Drop trailing
                // private segments so we name the public path. General across any
                // stub — no crate-family knowledge.
                let public_mod: String = mp
                    .split("::")
                    .filter(|seg| !seg.starts_with('_'))
                    .collect::<Vec<_>>()
                    .join("::");
                if public_mod.is_empty() {
                    return rust_name;
                }
                return format!("{public_mod}::{rust_name}");
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
    /// Per-target lowering templates: target name (e.g. "rust") → template string.
    /// When present, the codegen engine uses this template directly instead of
    /// heuristic suffix detection (async/fallible/builder patterns).
    #[serde(default)]
    pub lowers_to: HashMap<String, String>,
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

fn stub_referenced_type_names(stub: &StubCrate) -> HashSet<String> {
    let mut names = HashSet::new();
    let scan = |raw: &str, names: &mut HashSet<String>| {
        for tok in raw.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            if tok.is_empty() {
                continue;
            }
            let upper = tok.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            if !upper {
                continue;
            }
            if matches!(
                tok,
                "Str" | "String" | "Int" | "F64" | "Bool" | "Bytes" | "UUID" | "Id"
                    | "DateTime" | "Dt" | "List" | "Map" | "Set" | "Opt" | "Res" | "Json"
                    | "Any" | "Unit" | "Self" | "HashMap" | "Option" | "Vec" | "Result"
            ) {
                continue;
            }
            names.insert(tok.to_string());
        }
    };
    let scan_method = |m: &StubMethod, names: &mut HashSet<String>| {
        for (_, ty, _) in &m.params {
            scan(ty, names);
        }
        if let Some(ret) = &m.return_type {
            scan(ret, names);
        }
    };
    for s in &stub.structs {
        for m in &s.methods {
            scan_method(m, &mut names);
        }
    }
    for imp in &stub.impls {
        for m in &imp.methods {
            scan_method(m, &mut names);
        }
    }
    for m in &stub.free_fns {
        scan_method(m, &mut names);
    }
    names
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
    // lowers_to block parsing: tracks whether we're inside a lowers_to block
    // and which method container (struct/impl/free_fns) + index the template attaches to.
    let mut in_lowers_to = false;
    let mut lowers_to_base_indent: usize = 0;
    // Which container and index the lowers_to block belongs to
    enum LowersToTarget {
        StructMethod(usize),  // index in current_struct.methods
        ImplMethod(usize),    // index in current_impl.methods
        FreeFn(usize),        // index in stub.free_fns
    }
    let mut lowers_to_target: Option<LowersToTarget> = None;

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

        // lowers_to block parsing: if we're inside a lowers_to block, accumulate
        // `target: "template"` lines until indent drops back.
        if in_lowers_to {
            if indent > lowers_to_base_indent {
                // Parse `rust: "template string"` or `typescript: "template"`
                if let Some(colon_pos) = trimmed.find(':') {
                    let target = trimmed[..colon_pos].trim();
                    let value = trimmed[colon_pos + 1..].trim();
                    // Strip surrounding quotes
                    let template = if (value.starts_with('"') && value.ends_with('"'))
                        || (value.starts_with('\'') && value.ends_with('\''))
                    {
                        value[1..value.len() - 1].to_string()
                    } else {
                        value.to_string()
                    };
                    if !target.is_empty() && !template.is_empty() {
                        // Attach to the correct method
                        match &lowers_to_target {
                            Some(LowersToTarget::StructMethod(idx)) => {
                                if let Some(ref mut s) = current_struct {
                                    if let Some(m) = s.methods.get_mut(*idx) {
                                        m.lowers_to.insert(target.to_string(), template);
                                    }
                                }
                            }
                            Some(LowersToTarget::ImplMethod(idx)) => {
                                if let Some(ref mut i) = current_impl {
                                    if let Some(m) = i.methods.get_mut(*idx) {
                                        m.lowers_to.insert(target.to_string(), template);
                                    }
                                }
                            }
                            Some(LowersToTarget::FreeFn(idx)) => {
                                if let Some(m) = stub.free_fns.get_mut(*idx) {
                                    m.lowers_to.insert(target.to_string(), template);
                                }
                            }
                            None => {}
                        }
                    }
                }
                continue;
            } else {
                // Indent dropped — exit lowers_to mode, fall through to normal parsing
                in_lowers_to = false;
                lowers_to_target = None;
            }
        }

        // Detect `lowers_to` keyword (deeper indent than the method it follows)
        if trimmed == "lowers_to" {
            in_lowers_to = true;
            lowers_to_base_indent = indent;
            // Determine which method we're attaching to
            if let Some(ref s) = current_struct {
                if !s.methods.is_empty() {
                    lowers_to_target = Some(LowersToTarget::StructMethod(s.methods.len() - 1));
                }
            } else if let Some(ref i) = current_impl {
                if !i.methods.is_empty() {
                    lowers_to_target = Some(LowersToTarget::ImplMethod(i.methods.len() - 1));
                }
            } else if !stub.free_fns.is_empty() {
                lowers_to_target = Some(LowersToTarget::FreeFn(stub.free_fns.len() - 1));
            }
            continue;
        }

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

        // async_methods send, send_with — method names that are async on this stub's types.
        // When bang is used on these methods, .await is emitted before .map_err.
        if trimmed.starts_with("async_methods ") {
            stub.async_methods = trimmed
                .strip_prefix("async_methods ")
                .unwrap_or("")
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            continue;
        }

        // borrow_fields pool — field names that should use &self.field (not clone)
        if trimmed.starts_with("borrow_fields ") {
            stub.borrow_fields = trimmed
                .strip_prefix("borrow_fields ")
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
            // Enum variants as constructors: `S(Str)` → `fn S(v0: Str) -> Self`.
            if let Some(m) = parse_stub_enum_variant(trimmed) {
                if let Some(ref mut s) = current_struct {
                    s.methods.push(m);
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
                        lowers_to: HashMap::new(),
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
/// `S(Str)` / `Healthy` under an `enum` — constructor method returning Self.
fn parse_stub_enum_variant(line: &str) -> Option<StubMethod> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let first = line.chars().next()?;
    if !first.is_ascii_uppercase() {
        return None;
    }
    let name_end = line
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(line.len());
    if name_end == 0 {
        return None;
    }
    let rest = line[name_end..].trim();
    if !rest.is_empty() && !rest.starts_with('(') {
        return None;
    }
    let mut method = parse_stub_method(line);
    if method.return_type.is_none() {
        method.return_type = Some("Self".into());
    }
    // `S(Str)` has no `name: Type` — parse_stub_method stores the type as the name.
    method.params = method
        .params
        .into_iter()
        .enumerate()
        .map(|(i, (n, t, r))| {
            let type_only = t == "Str"
                && (n.contains('<')
                    || n.starts_with('[')
                    || n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false));
            if type_only {
                (format!("v{i}"), n, r)
            } else {
                (n, t, r)
            }
        })
        .collect();
    Some(method)
}

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
        split_top_level_commas(&params_str)
            .into_iter()
            .map(|p| {
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
            })
            .collect()
    };

    StubMethod { name, params, return_type: ret, lowers_to: HashMap::new() }
}

/// Split on commas that are not inside `<…>` or `(…)`.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
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
    /// Layer-declared passes (pre/post engine) for AST annotation.
    pub passes: Vec<PassSpec>,
    /// Per-target lowering templates for declared trait/struct methods.
    /// Key: `(TypeName, MethodName)`, Value: `{ target → template }`.
    pub method_lowers_to: HashMap<(String, String), HashMap<String, String>>,
    /// Raw target-language code to emit into the shared crate.
    /// Each entry is `(target, code_template)`. Templates support `{error_type}` substitution.
    /// Populated from `shared_emit <target>` blocks in layer files.
    pub shared_emit: Vec<(String, String)>,
    /// Harness render templates per target (e.g. "rust_bin" → template string).
    /// Populated from `harness_template <target>` blocks in layer files.
    pub harness_render_templates: HashMap<String, String>,
    /// Optional companion .veil implementation file path (relative to layer location).
    /// When present, the layer acts as a library: the companion is parsed and its
    /// constructs are merged into consuming projects.
    pub library: Option<String>,
    /// Cross-project UI component provider metadata, if this layer declares one.
    /// Present when the layer declares `implemented_by <project-slug>` and one or
    /// more `provides <Comp> …` directives. Enables the ProductHost UI build to
    /// materialize the implementing project's Svelte components into a consumer's
    /// generated tree. Fully data-driven — no project names in the engine.
    pub component_provider: Option<ComponentProvider>,
}

/// Cross-project UI component provider declared by a vocabulary layer.
///
/// A layer that provides component *vocabulary* (e.g. a design-kit layer) may
/// declare the separate project that *implements* those components plus the
/// exported component names. When a consumer `use`s such a layer, the UI build
/// resolves the implementing project, generates it, and copies the exported
/// `src/lib/components/<Name>.svelte` into the consumer's generated tree so the
/// emitted `$lib/components/<Name>.svelte` imports resolve.
///
/// Declared in the `.layer` file (top-level, inside the `pkg` body) as:
/// ```text
///   implemented_by dlx-designkit
///   provides CollectionView StatusPill DetailShell
///   provides FormField FormSection
/// ```
/// `provides` is repeatable; names accumulate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentProvider {
    /// Slug of the project that implements the exported components.
    pub implemented_by: String,
    /// Exported component names (PascalCase, matching `<Name>.svelte`).
    pub provides: Vec<String>,
}

/// Pkg-level `use` names in a layer body (`use deploy`, `use harness`, …).
fn collect_layer_use_names(content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(dep) = t.strip_prefix("use ") {
            let dep = dep.split_whitespace().next().unwrap_or("").trim();
            if !dep.is_empty() && !deps.iter().any(|d| d == dep) {
                deps.push(dep.to_string());
            }
        }
    }
    deps
}

/// Extract the cross-project UI component provider declaration from raw `.layer`
/// content, if present. Lightweight standalone parse (no LayerRegistry needed) so
/// hosts (e.g. the ProductHost UI build) can resolve component dependencies from
/// materialized layer files.
///
/// Reads top-level `implemented_by <slug>` and `provides <Comp> …` directives
/// inside the `pkg` body. `provides` is repeatable; names accumulate and are
/// de-duplicated. Returns `None` unless BOTH an implementing project and at least
/// one exported component are declared. Fully data-driven — no project names in
/// the engine.
pub fn parse_layer_component_provider(content: &str) -> Option<ComponentProvider> {
    let mut implemented_by: Option<String> = None;
    let mut provides: Vec<String> = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(slug) = t.strip_prefix("implemented_by ") {
            let slug = slug.trim();
            if !slug.is_empty() {
                implemented_by = Some(slug.to_string());
            }
        } else if let Some(rest) = t.strip_prefix("provides ") {
            for name in rest.split_whitespace() {
                let name = name.trim();
                if !name.is_empty() && !provides.iter().any(|n| n == name) {
                    provides.push(name.to_string());
                }
            }
        }
    }
    match (implemented_by, provides.is_empty()) {
        (Some(implemented_by), false) => Some(ComponentProvider {
            implemented_by,
            provides,
        }),
        _ => None,
    }
}

/// Collect top-level `use <name>` package/layer names from `.veil` source.
/// Lightweight standalone parse used by hosts to determine which layers a
/// consumer depends on without building a full IR. De-duplicates in order.
pub fn collect_veil_use_names(content: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("use ") {
            let name = rest.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    names
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
    // Declare lowers_to: track current type+method for attaching templates
    let mut decl_current_type: String = String::new();
    let mut decl_current_method: String = String::new();
    let mut decl_in_lowers_to = false;
    let mut decl_lowers_to_indent: usize = 0;
    let mut method_lowers_to: HashMap<(String, String), HashMap<String, String>> = HashMap::new();
    let mut in_prompt = false;
    let mut prompt_base_indent: usize = 0;
    let mut prompt_lines: Vec<String> = Vec::new();
    // Codegen block parsing state
    let mut codegen_templates: Vec<CodegenTemplate> = Vec::new();
    let mut in_codegen = false;
    let mut codegen_target: String = String::new();
    let mut codegen_base_indent: usize = 0;
    let mut codegen_lines: Vec<String> = Vec::new();
    // shared_emit block parsing state
    let mut shared_emit: Vec<(String, String)> = Vec::new();
    let mut in_shared_emit = false;
    let mut shared_emit_target: String = String::new();
    let mut shared_emit_base_indent: usize = 0;
    let mut shared_emit_lines: Vec<String> = Vec::new();
    // harness_template block parsing state
    let mut harness_render_templates: HashMap<String, String> = HashMap::new();
    let mut in_harness_template = false;
    let mut harness_template_target: String = String::new();
    let mut harness_template_base_indent: usize = 0;
    let mut harness_template_lines: Vec<String> = Vec::new();
    // Pass block parsing state
    let mut passes: Vec<PassSpec> = Vec::new();
    let mut in_pass = false;
    let mut pass_name: String = String::new();
    let mut _pass_base_indent: usize = 0;
    let mut pass_phase = PassPhase::Pre;
    let mut pass_priority: u32 = 100;
    let mut pass_rules: Vec<RuleSpec> = Vec::new();
    let mut pass_current_rule_name: Option<String> = None;
    let mut pass_current_when: String = String::new();
    let mut pass_current_actions: Vec<RuleAction> = Vec::new();
    // Library companion file path (from `library <path>` directive)
    let mut library_path: Option<String> = None;
    // Cross-project component provider metadata (from `implemented_by` + `provides`)
    let mut cp_implemented_by: Option<String> = None;
    let mut cp_provides: Vec<String> = Vec::new();
    // In-progress `view` under `present` (flushed on next view / role / section).
    let mut present_view: Option<crate::presentation::ViewSpec> = None;
    let mut errors: Vec<String> = Vec::new();
    // Multi-line lowers_to template accumulator (triple-quoted strings).
    let mut lowers_to_target: Option<String> = None;
    let mut lowers_to_lines: Vec<String> = Vec::new();

    let flush_present_view = |item: &mut Option<Item>,
                              view: &mut Option<crate::presentation::ViewSpec>| {
        if let (Some(Item::Construct(c)), Some(v)) = (item.as_mut(), view.take()) {
            c.presentation.views.push(v);
        }
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            // Inside multi-line lowers_to: preserve blank lines and #-lines (code).
            if lowers_to_target.is_some() {
                lowers_to_lines.push(line.to_string());
                continue;
            }
            // Blank lines inside declare blocks are preserved
            if in_declare && !current_decl_lines.is_empty() {
                current_decl_lines.push(String::new());
            }
            // Inside codegen blocks, blank lines are preserved AND lines starting
            // with # are real code (e.g. #[derive(...)]), not layer comments.
            if in_codegen {
                if trimmed.is_empty() {
                    codegen_lines.push(String::new());
                } else {
                    // # line inside codegen — treat as code content, not comment
                    let dedented = if line.len() > codegen_base_indent {
                        &line[codegen_base_indent..]
                    } else {
                        trimmed
                    };
                    codegen_lines.push(dedented.to_string());
                }
            }
            // Inside harness_template blocks, blank lines and #-lines are code content.
            if in_harness_template {
                if trimmed.is_empty() {
                    harness_template_lines.push(String::new());
                } else {
                    let dedented = if line.len() > harness_template_base_indent {
                        &line[harness_template_base_indent..]
                    } else {
                        trimmed
                    };
                    harness_template_lines.push(dedented.to_string());
                }
            }
            // Inside shared_emit blocks, blank lines and #-lines are code content
            // (e.g. #[derive(Debug, thiserror::Error)], #[error("...")]).
            // Only include lines that are indented within the block.
            if in_shared_emit {
                let line_indent = line.len() - line.trim_start().len();
                if line_indent >= shared_emit_base_indent || trimmed.is_empty() {
                    if trimmed.is_empty() {
                        shared_emit_lines.push(String::new());
                    } else {
                        let dedented = if line.len() > shared_emit_base_indent {
                            &line[shared_emit_base_indent..]
                        } else {
                            trimmed
                        };
                        shared_emit_lines.push(dedented.to_string());
                    }
                } else {
                    // # at a lesser indent means we've left the shared_emit section.
                    // Flush and fall through to normal parsing.
                    if !shared_emit_lines.is_empty() {
                        shared_emit.push((shared_emit_target.clone(), shared_emit_lines.join("\n")));
                        shared_emit_lines.clear();
                    }
                    in_shared_emit = false;
                    // This # line is a normal comment — skip it.
                }
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        // Accumulating a multi-line lowers_to template (inside triple-quotes).
        if lowers_to_target.is_some() {
            if trimmed == "\"\"\"" {
                // End of multi-line template — flush.
                let template = lowers_to_lines.join("\n");
                let target = lowers_to_target.take().unwrap();
                lowers_to_lines.clear();
                if let Some(item) = current.as_mut() {
                    match item {
                        Item::Construct(c) => { c.lowers_to.insert(target, template); }
                        Item::Statement(s) => { s.lowers_to.insert(target, template); }
                    }
                }
            } else {
                lowers_to_lines.push(line.to_string());
            }
            continue;
        }

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
                decl_in_lowers_to = false;
                // Fall through to normal parsing of this line
            } else {
                // ── Intercept lowers_to blocks on declared methods ────────
                // lowers_to keyword at method+2 indent
                if trimmed == "lowers_to" {
                    decl_in_lowers_to = true;
                    decl_lowers_to_indent = indent;
                    continue;
                }
                // Inside a lowers_to block: `target: "template"`
                // Must be indented deeper than the lowers_to keyword itself,
                // and target must be a bare word (language name like "typescript").
                if decl_in_lowers_to {
                    if indent > decl_lowers_to_indent {
                        if let Some((target, rest)) = trimmed.split_once(':') {
                            let target = target.trim();
                            // Valid target is a bare word (no parens, no spaces)
                            let is_valid_target = !target.is_empty()
                                && !target.contains('(')
                                && !target.contains(' ');
                            if is_valid_target {
                                let template = unquote(rest.trim());
                                if !template.is_empty() {
                                    let key = if decl_current_type.is_empty() {
                                        (decl_current_method.clone(), String::new())
                                    } else {
                                        (decl_current_type.clone(), decl_current_method.clone())
                                    };
                                    if !key.0.is_empty() {
                                        method_lowers_to
                                            .entry(key)
                                            .or_default()
                                            .insert(target.to_string(), template);
                                    }
                                }
                                continue;
                            }
                        }
                    }
                    // Indent decreased or line doesn't match → exit lowers_to
                    decl_in_lowers_to = false;
                }

                // Track current type name (trait/struct at base indent)
                if indent == declare_base_indent {
                    decl_in_lowers_to = false;
                    // Free function at base indent: `fn name(...)` or `async fn name(...)`
                    let fn_rest = trimmed.strip_prefix("fn ")
                        .or_else(|| trimmed.strip_prefix("async fn "));
                    if let Some(rest) = fn_rest {
                        let fn_name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !fn_name.is_empty() {
                            decl_current_type.clear(); // free fn, no enclosing type
                            decl_current_method = fn_name;
                        }
                    } else {
                        for prefix in ["trait ", "struct ", "enum "] {
                            if let Some(rest) = trimmed.strip_prefix(prefix) {
                                decl_current_type = rest
                                    .chars()
                                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                                    .collect();
                                decl_current_method.clear();
                                break;
                            }
                        }
                    }
                }

                // Track current method name (fn or bare method at base+2 indent)
                if indent == declare_base_indent + 2 && !decl_current_type.is_empty() {
                    decl_in_lowers_to = false;
                    let method_line = trimmed
                        .strip_prefix("async fn ")
                        .or_else(|| trimmed.strip_prefix("fn "))
                        .or_else(|| trimmed.strip_prefix("async "))
                        .unwrap_or(trimmed);
                    let method_name: String = method_line
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !method_name.is_empty() && method_line.contains('(') {
                        decl_current_method = method_name;
                    }
                }

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

        // Handle `shared_emit <target>` blocks: raw target-language code for the shared crate
        if trimmed.starts_with("shared_emit ") && indent <= 2 {
            // Flush any in-progress sections
            if in_shared_emit && !shared_emit_lines.is_empty() {
                shared_emit.push((shared_emit_target.clone(), shared_emit_lines.join("\n")));
                shared_emit_lines.clear();
            }
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
            if let Some(item) = current.take() {
                items.push(item);
            }
            in_declare = false;
            in_prompt = false;
            in_codegen = false;
            shared_emit_target = trimmed.strip_prefix("shared_emit ").unwrap().trim().to_string();
            in_shared_emit = true;
            shared_emit_base_indent = indent + 2; // content inside is one level deeper
            section = Section::None;
            continue;
        }

        // Handle `harness_template <target>` blocks: template for harness main.rs generation
        if trimmed.starts_with("harness_template ") && indent <= 2 {
            // Flush any in-progress sections
            if in_harness_template && !harness_template_lines.is_empty() {
                harness_render_templates.insert(harness_template_target.clone(), harness_template_lines.join("\n"));
                harness_template_lines.clear();
            }
            if in_shared_emit && !shared_emit_lines.is_empty() {
                shared_emit.push((shared_emit_target.clone(), shared_emit_lines.join("\n")));
                shared_emit_lines.clear();
            }
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
            if let Some(item) = current.take() {
                items.push(item);
            }
            in_declare = false;
            in_prompt = false;
            in_codegen = false;
            in_shared_emit = false;
            harness_template_target = trimmed.strip_prefix("harness_template ").unwrap().trim().to_string();
            in_harness_template = true;
            harness_template_base_indent = indent + 2;
            section = Section::None;
            continue;
        }

        if in_shared_emit {
            if indent <= 2 && !trimmed.is_empty() {
                // Leaving shared_emit — flush
                if !shared_emit_lines.is_empty() {
                    shared_emit.push((shared_emit_target.clone(), shared_emit_lines.join("\n")));
                    shared_emit_lines.clear();
                }
                in_shared_emit = false;
                // Fall through to normal parsing of this line
            } else {
                // Accumulate raw code lines (dedented)
                let dedented = if line.len() > shared_emit_base_indent {
                    &line[shared_emit_base_indent..]
                } else if trimmed.is_empty() {
                    ""
                } else {
                    trimmed
                };
                shared_emit_lines.push(dedented.to_string());
                continue;
            }
        }

        if in_harness_template {
            if indent <= 2 && !trimmed.is_empty() {
                // Leaving harness_template — flush
                if !harness_template_lines.is_empty() {
                    harness_render_templates.insert(harness_template_target.clone(), harness_template_lines.join("\n"));
                    harness_template_lines.clear();
                }
                in_harness_template = false;
                // Fall through to normal parsing of this line
            } else {
                // Accumulate raw template lines (dedented)
                let dedented = if line.len() > harness_template_base_indent {
                    &line[harness_template_base_indent..]
                } else if trimmed.is_empty() {
                    ""
                } else {
                    trimmed
                };
                harness_template_lines.push(dedented.to_string());
                continue;
            }
        }

        // Handle `pass <name>` blocks: layer-declared pre/post passes
        if trimmed.starts_with("pass ") && indent <= 2 {
            // Flush any in-progress pass
            if in_pass {
                // Flush current rule
                if let Some(rn) = pass_current_rule_name.take() {
                    if !pass_current_when.is_empty() || !pass_current_actions.is_empty() {
                        pass_rules.push(RuleSpec {
                            name: rn,
                            when: std::mem::take(&mut pass_current_when),
                            actions: std::mem::take(&mut pass_current_actions),
                        });
                    }
                }
                passes.push(PassSpec {
                    name: std::mem::take(&mut pass_name),
                    priority: pass_priority,
                    phase: pass_phase,
                    rules: std::mem::take(&mut pass_rules),
                    layer: layer_name.to_string(),
                });
            }
            // Flush other sections
            if in_codegen && !codegen_lines.is_empty() {
                let template = parse_codegen_block(&codegen_target, &codegen_lines, layer_name);
                codegen_templates.push(template);
                codegen_lines.clear();
            }
            if in_declare && !current_decl_lines.is_empty() {
                while current_decl_lines.last().map(|l| l.is_empty()).unwrap_or(false) {
                    current_decl_lines.pop();
                }
                declarations.push(current_decl_lines.join("\n"));
                current_decl_lines.clear();
            }
            if let Some(item) = current.take() {
                items.push(item);
            }
            in_declare = false;
            in_prompt = false;
            in_codegen = false;
            in_shared_emit = false;
            in_harness_template = false;
            pass_name = trimmed.strip_prefix("pass ").unwrap().trim().to_string();
            pass_phase = PassPhase::Pre;
            pass_priority = 100;
            pass_rules.clear();
            pass_current_rule_name = None;
            pass_current_when.clear();
            pass_current_actions.clear();
            in_pass = true;
            _pass_base_indent = indent + 2;
            section = Section::None;
            continue;
        }

        if in_pass {
            if indent <= 2 && !trimmed.is_empty() {
                // Leaving pass block — flush
                if let Some(rn) = pass_current_rule_name.take() {
                    if !pass_current_when.is_empty() || !pass_current_actions.is_empty() {
                        pass_rules.push(RuleSpec {
                            name: rn,
                            when: std::mem::take(&mut pass_current_when),
                            actions: std::mem::take(&mut pass_current_actions),
                        });
                    }
                }
                passes.push(PassSpec {
                    name: std::mem::take(&mut pass_name),
                    priority: pass_priority,
                    phase: pass_phase,
                    rules: std::mem::take(&mut pass_rules),
                    layer: layer_name.to_string(),
                });
                in_pass = false;
                // Fall through to normal parsing
            } else {
                // Parse pass contents
                if let Some(rest) = trimmed.strip_prefix("phase ") {
                    pass_phase = match rest.trim() {
                        "post" => PassPhase::Post,
                        _ => PassPhase::Pre,
                    };
                } else if let Some(rest) = trimmed.strip_prefix("priority ") {
                    pass_priority = rest.trim().parse().unwrap_or(100);
                } else if let Some(rest) = trimmed.strip_prefix("rule ") {
                    // Flush previous rule
                    if let Some(rn) = pass_current_rule_name.take() {
                        if !pass_current_when.is_empty() || !pass_current_actions.is_empty() {
                            pass_rules.push(RuleSpec {
                                name: rn,
                                when: std::mem::take(&mut pass_current_when),
                                actions: std::mem::take(&mut pass_current_actions),
                            });
                        }
                    }
                    pass_current_rule_name = Some(rest.trim().to_string());
                    pass_current_when.clear();
                    pass_current_actions.clear();
                } else if let Some(rest) = trimmed.strip_prefix("when:") {
                    pass_current_when = rest.trim().to_string();
                } else if let Some(rest) = trimmed.strip_prefix("annotate:") {
                    // Parse `key = "value"` or `key = value`
                    let rest = rest.trim();
                    if let Some((key, val)) = rest.split_once('=') {
                        pass_current_actions.push(RuleAction::Annotate {
                            key: key.trim().to_string(),
                            value: unquote(val.trim()),
                        });
                    }
                } else if let Some(rest) = trimmed.strip_prefix("wrap:") {
                    let kind = match rest.trim() {
                        "clone" => Some(WrapKind::Clone),
                        "borrow" => Some(WrapKind::Borrow),
                        "mut_borrow" => Some(WrapKind::MutBorrow),
                        "optional_chain" => Some(WrapKind::OptionalChain),
                        "try" => Some(WrapKind::Try),
                        "await" => Some(WrapKind::Await),
                        _ => None,
                    };
                    if let Some(k) = kind {
                        pass_current_actions.push(RuleAction::Wrap(k));
                    }
                } else if trimmed == "remove" {
                    pass_current_actions.push(RuleAction::Remove);
                }
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

        // Handle `library <path>` directive: companion .veil implementation file.
        if let Some(path) = trimmed.strip_prefix("library ") {
            let path = path.trim();
            if !path.is_empty() {
                library_path = Some(path.to_string());
            }
            continue;
        }

        // Handle `implemented_by <project-slug>` directive: the separate project
        // that implements this layer's exported UI components (cross-project
        // component resolution). Data-driven — no project names in the engine.
        if let Some(slug) = trimmed.strip_prefix("implemented_by ") {
            let slug = slug.trim();
            if !slug.is_empty() {
                cp_implemented_by = Some(slug.to_string());
            }
            continue;
        }

        // Handle `provides <Comp1> <Comp2> …` directive: exported component names
        // this layer's implementing project makes available. Repeatable; names
        // accumulate. Matches the emitted `$lib/components/<Name>.svelte` imports.
        if let Some(rest) = trimmed.strip_prefix("provides ") {
            for name in rest.split_whitespace() {
                let name = name.trim();
                if !name.is_empty() && !cp_provides.iter().any(|n| n == name) {
                    cp_provides.push(name.to_string());
                }
            }
            continue;
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
                required_fields: Vec::new(),
                lowers_to: HashMap::new(),
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
                // Lines: `rust: "template…"` or `rust: """` (multi-line)
                if let Some((target, rest)) = trimmed.split_once(':') {
                    let target = target.trim().to_string();
                    let rest = rest.trim();
                    if rest == "\"\"\"" || rest.starts_with("\"\"\"") {
                        // Start of multi-line triple-quoted template.
                        // If the line is `rust: """content` (rare), we'd need to
                        // handle inline start — but typical is just `rust: """`
                        lowers_to_target = Some(target);
                        lowers_to_lines.clear();
                    } else {
                        let template = unquote(rest);
                        if !target.is_empty() && !template.is_empty() {
                            match item {
                                Item::Construct(c) => { c.lowers_to.insert(target, template); }
                                Item::Statement(s) => { s.lowers_to.insert(target, template); }
                            }
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
                    } else if let Some(v) = trimmed.strip_prefix("has ") {
                        // `has field_name: TypeName` — layer-required field declaration.
                        if let Some((name, ty)) = v.split_once(':') {
                            let name = name.trim().to_string();
                            let ty = ty.trim().to_string();
                            if !name.is_empty() && !ty.is_empty() {
                                c.required_fields.push((name, ty));
                            }
                        }
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

    // Flush any trailing shared_emit block
    if in_shared_emit && !shared_emit_lines.is_empty() {
        shared_emit.push((shared_emit_target.clone(), shared_emit_lines.join("\n")));
    }

    // Flush any trailing harness_template block
    if in_harness_template && !harness_template_lines.is_empty() {
        harness_render_templates.insert(harness_template_target.clone(), harness_template_lines.join("\n"));
    }

    // Flush any trailing pass block
    if in_pass {
        if let Some(rn) = pass_current_rule_name.take() {
            if !pass_current_when.is_empty() || !pass_current_actions.is_empty() {
                pass_rules.push(RuleSpec {
                    name: rn,
                    when: std::mem::take(&mut pass_current_when),
                    actions: std::mem::take(&mut pass_current_actions),
                });
            }
        }
        passes.push(PassSpec {
            name: std::mem::take(&mut pass_name),
            priority: pass_priority,
            phase: pass_phase,
            rules: std::mem::take(&mut pass_rules),
            layer: layer_name.to_string(),
        });
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
        passes,
        method_lowers_to,
        shared_emit,
        harness_render_templates,
        library: library_path,
        component_provider: match (cp_implemented_by, cp_provides.is_empty()) {
            // A provider is only meaningful when it names an implementing project
            // AND exports at least one component. Otherwise leave it None.
            (Some(implemented_by), false) => Some(ComponentProvider {
                implemented_by,
                provides: cp_provides,
            }),
            _ => None,
        },
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

/// Parse optional `error_model` block from layer content:
/// ```text
/// error_model DomainError
///   external External
///   not_found NotFound
///   validation Validation
/// ```
/// The first word after `error_model` is the type name. Indented lines are
/// `role VariantName` pairs. A layer may declare any number of variants.
pub fn parse_error_model(content: &str) -> Option<ErrorModelPolicy> {
    let mut in_block = false;
    let mut type_name = String::new();
    let mut variants: Vec<(String, String)> = Vec::new();
    let mut found = false;
    let mut header_indent: usize = 0;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("error_model ") {
            let name = rest.trim();
            if !name.is_empty() {
                type_name = name.to_string();
                in_block = true;
                found = true;
                // Track the indentation of the header line
                header_indent = line.len() - line.trim_start().len();
            }
            continue;
        }
        if !in_block {
            continue;
        }
        // End of block: line at same or less indentation than header
        let line_indent = line.len() - line.trim_start().len();
        if line_indent <= header_indent {
            break;
        }
        // Parse "role VariantName" lines
        let parts: Vec<&str> = t.splitn(2, ' ').collect();
        if parts.len() == 2 {
            variants.push((parts[0].to_string(), parts[1].trim().to_string()));
        }
    }
    if found {
        Some(ErrorModelPolicy { type_name, variants })
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
    use std::sync::Mutex;

    static STUBS_DIR_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_component_provider_directives() {
        let src = r#"
pkg widgetkit v1
  desc "Widget vocabulary"
  implemented_by acme-widgetkit
  provides CollectionView StatusPill
  provides DetailShell

  construct CollectionView
    kw collection_view
    mt struct
"#;
        let raw = parse_layer_file(src, "widgetkit").expect("parse");
        let cp = raw.component_provider.expect("component_provider present");
        assert_eq!(cp.implemented_by, "acme-widgetkit");
        assert_eq!(cp.provides, vec!["CollectionView", "StatusPill", "DetailShell"]);
    }

    #[test]
    fn component_provider_absent_without_directives() {
        let src = "pkg plain v1\n  construct Foo\n    kw foo\n    mt struct\n";
        let raw = parse_layer_file(src, "plain").expect("parse");
        assert!(raw.component_provider.is_none());
    }

    #[test]
    fn component_provider_requires_both_implementer_and_exports() {
        // implemented_by only → None
        let a = parse_layer_component_provider("pkg x v1\n  implemented_by acme-x\n");
        assert!(a.is_none(), "implementer without provides must be None");
        // provides only → None
        let b = parse_layer_component_provider("pkg x v1\n  provides Foo Bar\n");
        assert!(b.is_none(), "provides without implementer must be None");
        // both → Some
        let c = parse_layer_component_provider(
            "pkg x v1\n  implemented_by acme-x\n  provides Foo Bar\n",
        )
        .expect("both present → Some");
        assert_eq!(c.implemented_by, "acme-x");
        assert_eq!(c.provides, vec!["Foo", "Bar"]);
    }

    #[test]
    fn collect_veil_use_names_dedups_in_order() {
        let src = "pkg app\n  use designkit\n  use ddd\n  use designkit\n\n  agg Foo\n";
        let names = collect_veil_use_names(src);
        assert_eq!(names, vec!["designkit", "ddd"]);
    }

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
    fn ddd_does_not_ship_bus_verbs() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        for kw in ["dispatch", "invoke", "request"] {
            assert!(
                reg.statement(kw).is_none(),
                "DDD must not declare {kw} — messaging is user-land"
            );
        }
        assert!(reg.routing_traits().is_empty(), "DDD must not declare routing traits");
    }

    #[test]
    fn bus_layer_provides_invoke_dispatch_request_and_routing_trait() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("bus", include_str!("../../../layers/bus.layer"))
            .expect("bus layer should load");
        // Bus layer declares all three verbs.
        for kw in ["invoke", "dispatch", "request"] {
            let stmt = reg.statement(kw).unwrap_or_else(|| panic!("bus must declare {kw}"));
            assert_eq!(stmt.port_target.as_deref(), Some("Bus"), "{kw} port_target");
            assert!(!stmt.lowers_to.is_empty(), "{kw} must have lowers_to");
            assert!(stmt.lowers_to.contains_key("rust"), "{kw} must have rust template");
        }
        // "Bus" becomes a routing trait via port_target.
        let routing = reg.routing_traits();
        assert!(routing.contains(&"Bus".to_string()), "Bus must be a routing trait");
        // Declare block provides Bus trait.
        assert!(
            !reg.declarations.is_empty(),
            "bus.layer must provide declare block"
        );
        let decl_joined = reg.declarations.join("\n");
        assert!(
            decl_joined.contains("trait Bus"),
            "declare must contain Bus trait: {decl_joined}"
        );
        // method_lowers_to should be populated from statement port_target+lowers_to.
        assert!(
            reg.method_lowers_to_template("Bus", "invoke", "rust").is_some(),
            "Bus.invoke must have rust method_lowers_to"
        );
        assert!(
            reg.method_lowers_to_template("Bus", "dispatch", "rust").is_some(),
            "Bus.dispatch must have rust method_lowers_to"
        );
        assert!(
            reg.method_lowers_to_template("Bus", "request", "rust").is_some(),
            "Bus.request must have rust method_lowers_to"
        );
    }

    #[test]
    fn ddd_fullstack_with_bus_has_routing_traits_and_verbs() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        reg.load_content("bus", include_str!("../../../layers/bus.layer"))
            .expect("bus");
        reg.load_content("bus_handle", include_str!("../../../layers/bus_handle.layer"))
            .expect("bus_handle");
        // After loading bus.layer, routing traits are present.
        let routing = reg.routing_traits();
        assert!(
            routing.contains(&"Bus".to_string()),
            "Bus must be routing trait when bus.layer loaded"
        );
        // Bus verbs available.
        for kw in ["invoke", "dispatch", "request"] {
            assert!(
                reg.statement(kw).is_some(),
                "{kw} must be available with bus.layer"
            );
        }
        // bus_handle strip policy still works.
        assert_eq!(reg.bus_message_name("HandleCreateUser"), "CreateUser");
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
        reg.load_content("base", include_str!("../../../layers/base.layer")).unwrap();
        reg.load_content("rust", include_str!("../../../layers/rust.layer")).unwrap();
        reg.load_content("tokio", include_str!("../../../layers/tokio.layer")).unwrap();
        reg.load_content("di", include_str!("../../../layers/di.layer")).unwrap();
        reg.load_content("rest_english", include_str!("../../../layers/rest_english.layer")).unwrap();
        reg.load_content("bus_handle", include_str!("../../../layers/bus_handle.layer")).unwrap();
        reg.load_content("auth_local", include_str!("../../../layers/auth_local.layer")).unwrap();
        reg.load_content("harness", include_str!("../../../layers/harness.layer")).unwrap();
        reg.load_content("deploy", include_str!("../../../layers/deploy.layer")).unwrap();
        reg.load_content("bus", include_str!("../../../layers/bus.layer")).unwrap();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer")).unwrap();
        reg.load_content("tokio_ddd", include_str!("../../../layers/tokio_ddd.layer")).unwrap();
        reg.load_content("ddd_fullstack", include_str!("../../../layers/ddd_fullstack.layer")).unwrap();
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
        assert!(reg.annotation_has_role("immutable", "immutable"), "immutable role from base.layer");
        assert!(reg.annotation_has_role("equality_by_value", "equality_by_value"), "equality_by_value role from base.layer");
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
        // Bus trait comes from bus.layer (not ddd) — this is correct.
        assert!(decl.contains("run_saga"), "run_saga missing: {decl}");
    }

    #[test]
    fn rest_english_and_bus_handle_packs_load_via_ddd_use() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("base", include_str!("../../../layers/base.layer")).unwrap();
        reg.load_content("rust", include_str!("../../../layers/rust.layer")).unwrap();
        reg.load_content("tokio", include_str!("../../../layers/tokio.layer")).unwrap();
        reg.load_content("di", include_str!("../../../layers/di.layer")).unwrap();
        reg.load_content("rest_english", include_str!("../../../layers/rest_english.layer")).unwrap();
        reg.load_content("bus_handle", include_str!("../../../layers/bus_handle.layer")).unwrap();
        reg.load_content("auth_local", include_str!("../../../layers/auth_local.layer")).unwrap();
        reg.load_content("harness", include_str!("../../../layers/harness.layer")).unwrap();
        reg.load_content("deploy", include_str!("../../../layers/deploy.layer")).unwrap();
        reg.load_content("bus", include_str!("../../../layers/bus.layer")).unwrap();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer")).unwrap();
        reg.load_content("tokio_ddd", include_str!("../../../layers/tokio_ddd.layer")).unwrap();
        reg.load_content("ddd_fullstack", include_str!("../../../layers/ddd_fullstack.layer")).unwrap();
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
        assert!(
            reg.layers.iter().any(|l| l == "deploy"),
            "ddd must pull deploy: {:?}",
            reg.layers
        );
        let hook = crate::ast::Construct::new(
            "hook",
            "DeployHook",
            crate::layer::Shape::Fn,
            "ConfigureX".into(),
            crate::span::Span::new(0, 0),
        );
        assert!(
            reg.construct_has_role(&hook, "deploy_hook"),
            "hook must carry role:deploy_hook"
        );
        assert!(!reg.construct_has_role(&hook, "http_endpoint"));
        assert_eq!(reg.http_name_policy.list_prefix.as_deref(), Some("List"));
        assert_eq!(reg.bus_policy.strip_name_prefix.as_deref(), Some("Handle"));
    }

    /// Mutual `use` must load each layer once (no stack overflow).
    #[test]
    fn load_layer_cycle_loads_each_once() {
        let dir = std::env::temp_dir().join(format!(
            "veil-layer-cycle-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let layers = dir.join("layers");
        std::fs::create_dir_all(&layers).unwrap();
        std::fs::write(
            layers.join("cycle_a.layer"),
            r#"
pkg cycle_a v1
  use cycle_b
  construct Alpha
    kw cycle_a_kw
    mt struct
"#,
        )
        .unwrap();
        std::fs::write(
            layers.join("cycle_b.layer"),
            r#"
pkg cycle_b v1
  use cycle_a
  construct Beta
    kw cycle_b_kw
    mt struct
"#,
        )
        .unwrap();

        let mut reg = LayerRegistry::builtin();
        reg.load_layer("cycle_a", &dir)
            .expect("cyclic use must not overflow");
        let a_count = reg.layers.iter().filter(|l| *l == "cycle_a").count();
        let b_count = reg.layers.iter().filter(|l| *l == "cycle_b").count();
        assert_eq!(a_count, 1, "cycle_a loaded once: {:?}", reg.layers);
        assert_eq!(b_count, 1, "cycle_b loaded once: {:?}", reg.layers);
        assert!(
            reg.constructs.iter().any(|c| c.keyword == "cycle_a_kw"),
            "cycle_a constructs merged"
        );
        assert!(
            reg.constructs.iter().any(|c| c.keyword == "cycle_b_kw"),
            "cycle_b constructs merged"
        );

        // Second call is a no-op — still once.
        reg.load_layer("cycle_a", &dir).unwrap();
        assert_eq!(
            reg.layers.iter().filter(|l| *l == "cycle_a").count(),
            1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layer_use_closure_follows_recorded_edges() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content(
            "leaf",
            "pkg leaf v1\n  construct Leaf\n    kw leaf_kw\n    mt struct\n",
        )
        .unwrap();
        reg.load_content(
            "root",
            "pkg root v1\n  use leaf\n  construct Root\n    kw root_kw\n    mt struct\n",
        )
        .unwrap();
        let via_root = reg.layer_use_closure(["root"]);
        assert!(
            via_root.contains("root") && via_root.contains("leaf"),
            "{via_root:?}"
        );
        let via_leaf = reg.layer_use_closure(["leaf"]);
        assert!(via_leaf.contains("leaf"), "{via_leaf:?}");
        assert!(!via_leaf.contains("root"), "{via_leaf:?}");
    }

    #[test]
    fn teaching_closure_includes_implicit_uses() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content(
            "acme",
            "pkg acme v1\n  construct Acme\n    kw acme_kw\n    mt struct\n",
        )
        .unwrap();
        assert!(
            !reg.teaching_closure(std::iter::empty::<&str>())
                .contains("acme"),
            "implicit-empty teaching must not pull an unused layer"
        );
        reg.implicit_uses.push("acme".into());
        let taught = reg.teaching_closure(std::iter::empty::<&str>());
        assert!(taught.contains("acme"), "{taught:?}");
    }

    #[test]
    fn for_veil_file_records_primary_layer_as_implicit_use() {
        let dir = std::env::temp_dir().join(format!(
            "veil-implicit-use-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("layers")).unwrap();
        std::fs::write(
            dir.join("veil.toml"),
            "name = \"acme\"\n[package]\nname = \"acme\"\nveil = \"main.veil\"\nlayer = \"layers/main.layer\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("layers/main.layer"),
            "pkg acme v1\n  prompt\n    ACME_PRIMARY_MARK\n  construct AcmeThing\n    kw acme_thing\n    mt struct\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.veil"),
            "pkg App\n  struct Point\n    x: Int\n",
        )
        .unwrap();
        let reg = LayerRegistry::for_veil_file(&dir.join("main.veil")).expect("registry");
        assert!(
            reg.implicit_uses.iter().any(|l| l == "acme"),
            "primary layer must be an implicit use: {:?}",
            reg.implicit_uses
        );
        assert!(
            reg.prompts.iter().any(|(_, t)| t.contains("ACME_PRIMARY_MARK")),
            "primary layer prompt must load: {:?}",
            reg.prompts
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        std::fs::write(dir.join("main.veil"), "pkg app\n  use ddd_fullstack\n").unwrap();

        // Load ddd policies then apply project overrides (mirrors for_veil_file tail).
        let mut reg = LayerRegistry::builtin();
        reg.load_content("base", include_str!("../../../layers/base.layer")).unwrap();
        reg.load_content("rust", include_str!("../../../layers/rust.layer")).unwrap();
        reg.load_content("tokio", include_str!("../../../layers/tokio.layer")).unwrap();
        reg.load_content("di", include_str!("../../../layers/di.layer")).unwrap();
        reg.load_content("rest_english", include_str!("../../../layers/rest_english.layer")).unwrap();
        reg.load_content("bus_handle", include_str!("../../../layers/bus_handle.layer")).unwrap();
        reg.load_content("auth_local", include_str!("../../../layers/auth_local.layer")).unwrap();
        reg.load_content("harness", include_str!("../../../layers/harness.layer")).unwrap();
        reg.load_content("deploy", include_str!("../../../layers/deploy.layer")).unwrap();
        reg.load_content("bus", include_str!("../../../layers/bus.layer")).unwrap();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer")).unwrap();
        reg.load_content("tokio_ddd", include_str!("../../../layers/tokio_ddd.layer")).unwrap();
        reg.load_content("ddd_fullstack", include_str!("../../../layers/ddd_fullstack.layer")).unwrap();
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
    fn harness_render_template_parsed_from_layer() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("harness", include_str!("../../../layers/harness.layer")).unwrap();
        assert!(reg.harness_render_templates.contains_key("rust_bin"),
            "harness.layer should declare harness_template rust_bin");
        let tpl = &reg.harness_render_templates["rust_bin"];
        assert!(tpl.contains("{{package_name}}"), "template should contain package_name variable");
        assert!(tpl.contains("{{for endpoint in endpoints}}"), "template should contain endpoint loop");
        assert!(tpl.contains("axum"), "template should contain axum framework code");
        // Also check rust_bin_cargo
        assert!(reg.harness_render_templates.contains_key("rust_bin_cargo"),
            "harness.layer should declare harness_template rust_bin_cargo");
        let cargo = &reg.harness_render_templates["rust_bin_cargo"];
        assert!(cargo.contains("axum"), "cargo deps should reference axum");
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
    fn stub_method_hashmap_param_is_one_arg() {
        let src = r#"
stub example-sdk 1.0.0
  struct Builder
    fn set_item(input: Opt<HashMap<Str, AttributeValue>>) -> Self
    fn item(k: Str, v: AttributeValue) -> Self
"#;
        let stub = parse_stub_file(src).expect("stub");
        let b = stub.structs.iter().find(|s| s.name == "Builder").unwrap();
        let set = b.methods.iter().find(|m| m.name == "set_item").unwrap();
        assert_eq!(set.params.len(), 1, "{:?}", set.params);
        assert_eq!(set.params[0].1, "Opt<HashMap<Str, AttributeValue>>");
        let item = b.methods.iter().find(|m| m.name == "item").unwrap();
        assert_eq!(item.params.len(), 2, "{:?}", item.params);
    }

    #[test]
    fn stub_enum_variant_becomes_constructor() {
        let src = r#"
stub example-sdk 1.0.0
types_module types
  enum AttributeValue
    S(Str)
    N(Str)
"#;
        let stub = parse_stub_file(src).expect("stub");
        let av = stub
            .structs
            .iter()
            .find(|s| s.name == "AttributeValue")
            .unwrap();
        let s = av.methods.iter().find(|m| m.name == "S").expect("S");
        assert_eq!(s.params.len(), 1, "{:?}", s.params);
        assert_eq!(s.params[0].1, "Str");
        assert_eq!(s.return_type.as_deref(), Some("Self"));
    }

    #[test]
    fn ddd_declare_exposes_saga_not_bus() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .unwrap();
        assert!(
            !reg.declared_type_names().contains("Bus"),
            "DDD must not declare Bus"
        );
        assert!(reg.declared_type_names().contains("SagaStep"));
        assert!(reg.declared_fn_names().contains(&"run_saga".to_string()));
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

    #[test]
    fn system_stub_fills_undeclared_referenced_types() {
        let _guard = STUBS_DIR_LOCK.lock().expect("stubs dir lock");
        let dir = std::env::temp_dir().join(format!(
            "veil-stub-gap-{}",
            std::process::id()
        ));
        let sys = std::env::temp_dir().join(format!(
            "veil-sys-stubs-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sys);
        std::fs::create_dir_all(dir.join("stubs")).expect("mkdir project stubs");
        std::fs::create_dir_all(&sys).expect("mkdir system stubs");
        std::fs::write(
            dir.join("main.veil"),
            "pkg gap_test v1\n  use example_sdk\n",
        )
        .expect("write main.veil");
        // Product rustdoc dump: uses Blob in a signature, never declares it.
        std::fs::write(
            dir.join("stubs").join("example_sdk.stub"),
            "stub example-sdk 1.0.0\n  struct Client\n    fn payload(input: Blob) -> Self\n",
        )
        .expect("write product stub");
        // Curated system stub declares Blob with a module path.
        std::fs::write(
            sys.join("example_sdk.stub"),
            "stub example-sdk 1.0.0\n  struct Blob\n    path primitives\n    fn new(data: Bytes) -> Self\n",
        )
        .expect("write system stub");
        let old = std::env::var("VEIL_STUBS_DIR").ok();
        unsafe { std::env::set_var("VEIL_STUBS_DIR", &sys) };
        let reg = LayerRegistry::for_veil_file(&dir.join("main.veil")).expect("registry");
        match old {
            Some(v) => unsafe { std::env::set_var("VEIL_STUBS_DIR", v) },
            None => unsafe { std::env::remove_var("VEIL_STUBS_DIR") },
        }
        let stub = reg
            .stubs
            .iter()
            .find(|s| s.name == "example-sdk" || s.name == "example_sdk")
            .expect("example-sdk loaded");
        assert!(
            stub.structs.iter().any(|s| s.name == "Blob"
                && s.module_path.as_deref() == Some("primitives")),
            "system Blob must be folded in: {:?}",
            stub.structs.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn system_stub_inherits_unset_codegen_policy() {
        let _guard = STUBS_DIR_LOCK.lock().expect("stubs dir lock");
        let nonce = format!("{}-{}", std::process::id(), {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        });
        let dir = std::env::temp_dir().join(format!("veil-stub-policy-{nonce}"));
        let sys = std::env::temp_dir().join(format!("veil-sys-policy-{nonce}"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sys);
        std::fs::create_dir_all(dir.join("stubs")).expect("mkdir project stubs");
        std::fs::create_dir_all(&sys).expect("mkdir system stubs");
        std::fs::write(
            dir.join("main.veil"),
            "pkg policy_test v1\n  use policy_sdk\n",
        )
        .expect("write main.veil");
        // Product stub has Blob but no path / types_module.
        std::fs::write(
            dir.join("stubs").join("policy_sdk.stub"),
            "stub policy-sdk 1.0.0\n  struct Blob\n    fn new(data: Str) -> Blob\n",
        )
        .expect("write product stub");
        std::fs::write(
            sys.join("policy_sdk.stub"),
            "stub policy-sdk 1.0.0\ntypes_module types\nroot_types Client\n  struct Blob\n    path primitives\n    fn new(data: Bytes) -> Self\n  struct Client\n",
        )
        .expect("write system stub");
        let old = std::env::var("VEIL_STUBS_DIR").ok();
        unsafe { std::env::set_var("VEIL_STUBS_DIR", &sys) };
        let reg = LayerRegistry::for_veil_file(&dir.join("main.veil")).expect("registry");
        match old {
            Some(v) => unsafe { std::env::set_var("VEIL_STUBS_DIR", v) },
            None => unsafe { std::env::remove_var("VEIL_STUBS_DIR") },
        }
        let stub = reg
            .stubs
            .iter()
            .find(|s| s.name == "policy-sdk" || s.name == "policy_sdk")
            .expect("policy-sdk loaded");
        assert_eq!(
            stub.types_module.as_deref(),
            Some("types"),
            "types_module inherit: {:?}",
            stub.types_module
        );
        assert_eq!(stub.root_types, vec!["Client".to_string()]);
        let blob = stub.structs.iter().find(|s| s.name == "Blob").unwrap();
        assert_eq!(blob.module_path.as_deref(), Some("primitives"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn construct_lowers_to_parses_multiline_and_single_line() {
        let src = r#"
pkg test v1
  construct ValueObject
    kw val
    mt struct
    lowers_to
      rust: """
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct {{name}} {
            {{for field in fields}}pub {{field.name}}: {{field.type}},
            {{end}}
        }
      """
      typescript: "export interface {{name}} {}"
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");
        let spec = reg.construct("val").expect("val construct");
        // Multi-line template
        let rust_tpl = spec.lowers_to.get("rust").expect("rust template");
        assert!(
            rust_tpl.contains("#[derive(Debug, Clone, PartialEq, Eq, Hash)]"),
            "multi-line template should preserve derives: {}",
            rust_tpl
        );
        assert!(
            rust_tpl.contains("pub struct {{name}}"),
            "multi-line template should preserve placeholders: {}",
            rust_tpl
        );
        assert!(
            rust_tpl.contains("{{for field in fields}}"),
            "multi-line template should preserve loop: {}",
            rust_tpl
        );
        // Single-line template
        let ts_tpl = spec.lowers_to.get("typescript").expect("typescript template");
        assert_eq!(ts_tpl, "export interface {{name}} {}");
    }

    #[test]
    fn parse_inline_has_field_requirement() {
        let src = r#"pkg test v1
  construct Entity
    kw ent
    mt struct
    has id: Id
    has tenant_id: Id
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");
        let spec = reg.construct("ent").expect("Entity spec");
        assert_eq!(spec.required_fields.len(), 2);
        assert_eq!(spec.required_fields[0], ("id".to_string(), "Id".to_string()));
        assert_eq!(spec.required_fields[1], ("tenant_id".to_string(), "Id".to_string()));
    }

    #[test]
    fn has_block_does_not_become_required_field() {
        // `has` alone (block syntax) should NOT create required_fields.
        // Indent construct inside `pkg` so that `has` block contents are > indent 4.
        let src = r#"pkg test v1
  construct Aggregate
    kw agg
    mt struct
    has
      root: struct
      fn[]
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");
        let spec = reg.construct("agg").expect("Aggregate spec");
        assert!(spec.required_fields.is_empty(), "block `has` should not produce required_fields");
        assert!(spec.contains.iter().any(|c| c.starts_with("root")), "root should be in contains: {:?}", spec.contains);
    }

    #[test]
    fn declare_method_lowers_to_parses_templates() {
        let src = r#"pkg test v1
  declare
    trait ApiClient
      fetch(endpoint: Str, params: Json) -> Res!<Json>
        lowers_to
          typescript: "(async () => { const __r = await fetch({arg0}); return __r.json(); })()"
      mutate(endpoint: Str, body: Json) -> Res!<Json>
        lowers_to
          typescript: "(async () => { const __r = await fetch({arg0}, { method: 'POST', body: JSON.stringify({arg1}) }); return __r.json(); })()"

    struct LocalStorage
      fn get_or(key: Str, default: Str) -> Str
        lowers_to
          typescript: "(localStorage.getItem({arg0}) ?? {arg1})"

    fn goto(url: Str) -> Res!
      lowers_to
        typescript: "window.location.href = {arg0}"
      ret Ok
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");

        // Trait method: (ApiClient, fetch)
        let tmpl = reg.method_lowers_to_template("ApiClient", "fetch", "typescript");
        assert!(tmpl.is_some(), "ApiClient.fetch should have lowers_to template");
        let tmpl_str = tmpl.unwrap();
        assert!(tmpl_str.contains("await fetch({arg0})"), "template should contain fetch call, got: {}", tmpl_str);

        // Trait method: (ApiClient, mutate)
        let tmpl = reg.method_lowers_to_template("ApiClient", "mutate", "typescript");
        assert!(tmpl.is_some(), "ApiClient.mutate should have lowers_to template");
        assert!(tmpl.unwrap().contains("method: 'POST'"), "template should contain POST");

        // Struct method: (LocalStorage, get_or)
        let tmpl = reg.method_lowers_to_template("LocalStorage", "get_or", "typescript");
        assert!(tmpl.is_some(), "LocalStorage.get_or should have lowers_to template");
        assert!(tmpl.unwrap().contains("localStorage.getItem"), "template should use localStorage");

        // Free function: (goto, "") — called as CallExpr { target: "goto", method: "" }
        let tmpl = reg.method_lowers_to_template("goto", "", "typescript");
        assert!(tmpl.is_some(), "goto should have lowers_to template, got {:?}", reg.method_lowers_to);
        assert!(tmpl.unwrap().contains("window.location.href"), "template should use window.location");
    }

    #[test]
    fn parse_error_model_basic() {
        let content = r#"
error_model DomainError
  external External
  not_found NotFound
  validation Validation
"#;
        let em = super::parse_error_model(content).unwrap();
        assert_eq!(em.type_name, "DomainError");
        assert_eq!(em.variants.len(), 3);
        assert_eq!(em.variant("external"), Some("External"));
        assert_eq!(em.variant("not_found"), Some("NotFound"));
        assert_eq!(em.variant("validation"), Some("Validation"));
    }

    #[test]
    fn parse_error_model_custom() {
        let content = r#"
error_model AppError
  external Upstream
  not_found Missing
  validation BadInput
  timeout Timeout
"#;
        let em = super::parse_error_model(content).unwrap();
        assert_eq!(em.type_name, "AppError");
        assert_eq!(em.variants.len(), 4);
        assert_eq!(em.variant("external"), Some("Upstream"));
        assert_eq!(em.variant("not_found"), Some("Missing"));
        assert_eq!(em.variant("validation"), Some("BadInput"));
        assert_eq!(em.variant("timeout"), Some("Timeout"));
        // Full path generation
        assert_eq!(em.variant_path("external"), Some("AppError::Upstream".to_string()));
    }

    #[test]
    fn parse_error_model_nested_in_layer() {
        // Simulates ddd.layer where error_model is indented inside pkg block
        let content = r#"pkg ddd v1
  desc "test"
  error_model DomainError
    external External
    not_found NotFound
    validation Validation
  construct Context
    kw ctx
"#;
        let em = super::parse_error_model(content).unwrap();
        assert_eq!(em.type_name, "DomainError");
        assert_eq!(em.variants.len(), 3);
        assert_eq!(em.variant("external"), Some("External"));
    }

    #[test]
    fn parse_error_model_absent() {
        let content = "construct Foo\n  kw foo\n";
        assert!(super::parse_error_model(content).is_none());
    }

    #[test]
    fn pass_declaration_parses_from_layer() {
        let src = r#"
pkg test v1
  construct Aggregate
    kw agg
    mt struct
  pass ownership
    phase pre
    priority 20
    rule last_use_moves
      when: construct.kind == "struct"
      annotate: ownership = "move"
    rule multi_use_clones
      when: construct.kind == "struct" && construct.exported
      annotate: ownership = "clone"
      wrap: clone
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");
        assert_eq!(reg.passes.len(), 1);
        let pass = &reg.passes[0];
        assert_eq!(pass.name, "ownership");
        assert_eq!(pass.phase, PassPhase::Pre);
        assert_eq!(pass.priority, 20);
        assert_eq!(pass.layer, "test");
        assert_eq!(pass.rules.len(), 2);
        // First rule
        let r0 = &pass.rules[0];
        assert_eq!(r0.name, "last_use_moves");
        assert_eq!(r0.when, r#"construct.kind == "struct""#);
        assert_eq!(r0.actions.len(), 1);
        match &r0.actions[0] {
            RuleAction::Annotate { key, value } => {
                assert_eq!(key, "ownership");
                assert_eq!(value, "move");
            }
            _ => panic!("expected Annotate"),
        }
        // Second rule has annotate + wrap
        let r1 = &pass.rules[1];
        assert_eq!(r1.name, "multi_use_clones");
        assert_eq!(r1.actions.len(), 2);
        match &r1.actions[0] {
            RuleAction::Annotate { key, value } => {
                assert_eq!(key, "ownership");
                assert_eq!(value, "clone");
            }
            _ => panic!("expected Annotate"),
        }
        match &r1.actions[1] {
            RuleAction::Wrap(WrapKind::Clone) => {}
            _ => panic!("expected Wrap(Clone)"),
        }
    }

    #[test]
    fn pass_post_phase_parses() {
        let src = r#"
pkg test v1
  pass style_enforce
    phase post
    priority 200
    rule remove_unused
      when: construct.kind == "struct" && !construct.exported
      remove
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");
        assert_eq!(reg.passes.len(), 1);
        let pass = &reg.passes[0];
        assert_eq!(pass.phase, PassPhase::Post);
        assert_eq!(pass.priority, 200);
        let r = &pass.rules[0];
        assert_eq!(r.actions.len(), 1);
        matches!(&r.actions[0], RuleAction::Remove);
    }

    #[test]
    fn multiple_passes_accumulate() {
        let src = r#"
pkg test v1
  pass first_pass
    phase pre
    priority 10
    rule r1
      when: construct.kind == "struct"
      annotate: x = "1"
  pass second_pass
    phase pre
    priority 20
    rule r2
      when: construct.kind == "trait"
      annotate: y = "2"
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("test", src).expect("layer should load");
        assert_eq!(reg.passes.len(), 2);
        assert_eq!(reg.passes[0].name, "first_pass");
        assert_eq!(reg.passes[1].name, "second_pass");
    }

    // ── Library Projects ────────────────────────────────────────────────

    #[test]
    fn parse_layer_file_library_directive() {
        let src = r#"
layer test_lib
  use base
  library main.veil

  construct Widget
    kw widget
    mt struct
"#;
        let raw = super::parse_layer_file(src, "test_lib").unwrap();
        assert_eq!(raw.library, Some("main.veil".to_string()));
        assert_eq!(raw.constructs.len(), 1);
        assert_eq!(raw.constructs[0].keyword, "widget");
    }

    #[test]
    fn parse_layer_file_no_library_directive() {
        let src = r#"
layer plain
  construct Foo
    kw foo
    mt struct
"#;
        let raw = super::parse_layer_file(src, "plain").unwrap();
        assert_eq!(raw.library, None);
    }

    #[test]
    fn load_content_with_library_filesystem() {
        use std::io::Write;
        // Create a temp dir with a layer and companion .veil file
        let tmp = std::env::temp_dir().join("veil_test_library");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let layer_content = r#"layer test_lib
  library companion.veil

  construct Widget
    kw widget
    mt struct
"#;
        let companion_content = r#"sol TestLib
  ctx Widgets
    struct DefaultWidget
      name: Str
"#;
        let layer_file = tmp.join("test_lib.layer");
        let companion_file = tmp.join("companion.veil");
        std::fs::File::create(&layer_file).unwrap().write_all(layer_content.as_bytes()).unwrap();
        std::fs::File::create(&companion_file).unwrap().write_all(companion_content.as_bytes()).unwrap();

        let mut reg = LayerRegistry::builtin();
        reg.load_layer("test_lib", &tmp).unwrap();

        // Library constructs should be loaded
        assert_eq!(reg.library_constructs.len(), 1);
        assert_eq!(reg.library_constructs[0].0, "test_lib");
        assert!(reg.library_constructs[0].1.contains("DefaultWidget"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_content_library_via_env_path() {
        use std::io::Write;
        // Create a library project in a temp dir
        let tmp = std::env::temp_dir().join("veil_test_lib_path");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("layers")).unwrap();

        let layer_content = "layer env_lib\n  library main.veil\n\n  construct Gadget\n    kw gadget\n    mt struct\n";
        let companion_content = "sol EnvLib\n  ctx Gadgets\n    struct SomeGadget\n      id: Int\n";

        std::fs::File::create(tmp.join("layers").join("env_lib.layer"))
            .unwrap().write_all(layer_content.as_bytes()).unwrap();
        std::fs::File::create(tmp.join("main.veil"))
            .unwrap().write_all(companion_content.as_bytes()).unwrap();

        // Set VEIL_LIBRARY_PATH and load via load_content (which uses cwd)
        let old_env = std::env::var("VEIL_LIBRARY_PATH").ok();
        unsafe { std::env::set_var("VEIL_LIBRARY_PATH", tmp.display().to_string()); }

        let mut reg = LayerRegistry::builtin();
        // load_content doesn't find the companion from layers/ subdir relative path
        // directly, but it does look at VEIL_LIBRARY_PATH
        reg.load_content("env_lib", layer_content).unwrap();

        // Should find via VEIL_LIBRARY_PATH
        assert_eq!(reg.library_constructs.len(), 1);
        assert_eq!(reg.library_constructs[0].0, "env_lib");

        // Restore env
        match old_env {
            Some(val) => unsafe { std::env::set_var("VEIL_LIBRARY_PATH", val); },
            None => unsafe { std::env::remove_var("VEIL_LIBRARY_PATH"); },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn library_constructs_not_duplicated_on_reload() {
        use std::io::Write;
        let tmp = std::env::temp_dir().join("veil_test_lib_nodup");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let layer_content = "layer nodup\n  library lib.veil\n\n  construct Thing\n    kw thing\n    mt struct\n";
        let companion_content = "sol NoDup\n  ctx Things\n    struct AThing\n      x: Int\n";
        std::fs::File::create(tmp.join("nodup.layer")).unwrap().write_all(layer_content.as_bytes()).unwrap();
        std::fs::File::create(tmp.join("lib.veil")).unwrap().write_all(companion_content.as_bytes()).unwrap();

        let mut reg = LayerRegistry::builtin();
        reg.load_layer("nodup", &tmp).unwrap();
        // Attempting to load again should not duplicate
        reg.load_layer("nodup", &tmp).unwrap();
        assert_eq!(reg.library_constructs.len(), 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Spec 4: resolution search path (VEIL_SEARCH_PATHS) ──────────────────

    /// Serialize VEIL_SEARCH_PATHS mutation across tests (env is process-global).
    static SEARCH_PATHS_LOCK: Mutex<()> = Mutex::new(());

    fn mk_search_repo(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "veil-sp-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_layer(path: &Path, kw: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Minimal parseable layer declaring a construct so codegen/check has vocab.
        let src = format!(
            "layer {kw}\n\n  construct {}\n    kw {kw}\n    mt struct\n",
            {
                let mut c = kw.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        );
        std::fs::write(path, src).unwrap();
    }

    #[test]
    fn search_root_lookup_order_is_deterministic() {
        // Root-level layers/<name>.layer resolves.
        let root = mk_search_repo("order-root");
        write_layer(&root.join("layers").join("shopkit.layer"), "shopkit");
        assert!(
            LayerRegistry::load_layer_from_search_root("shopkit", &root).is_some(),
            "root-level layers/<name>.layer should resolve"
        );
        let _ = std::fs::remove_dir_all(&root);

        // Workspace member subdir <root>/<name>/layers/<name>.layer resolves.
        let ws = mk_search_repo("order-ws");
        write_layer(&ws.join("orders").join("layers").join("orders.layer"), "orders");
        assert!(
            LayerRegistry::load_layer_from_search_root("orders", &ws).is_some(),
            "workspace member layers/<name>.layer should resolve"
        );
        // Unknown name must not resolve.
        assert!(
            LayerRegistry::load_layer_from_search_root("missing", &ws).is_none(),
            "unknown name must not resolve"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn layer_resolves_from_search_path_and_project_local_overrides() {
        let _guard = SEARCH_PATHS_LOCK.lock().unwrap();

        // A registered search-path repo supplies `inventory.layer` (workspace member).
        // Uses a NON-platform name so it flows through the userland chain where
        // search paths are consulted (platform names like `di`/`ddd` take a
        // different branch).
        let repo = mk_search_repo("resolve");
        write_layer(
            &repo.join("inventory").join("layers").join("inventory.layer"),
            "inventory",
        );

        // A consumer project directory that does NOT contain inventory.layer.
        let proj = mk_search_repo("consumer");
        std::fs::write(proj.join("main.veil"), "sol App\n").unwrap();

        // Without the search path: resolution fails with the normal diagnostic.
        unsafe {
            std::env::remove_var("VEIL_SEARCH_PATHS");
        }
        let reg = LayerRegistry::default();
        let err = reg.resolve_layer_content("inventory", &proj).unwrap_err();
        assert!(err.contains("not found"), "expected not-found diagnostic, got: {err}");

        // With the search path: `inventory` resolves from the registered repo.
        unsafe {
            std::env::set_var("VEIL_SEARCH_PATHS", format!("libs={}", repo.display()));
        }
        let reg = LayerRegistry::default();
        let content = reg
            .resolve_layer_content("inventory", &proj)
            .expect("inventory must resolve from the registered search path");
        assert!(
            content.contains("layer inventory"),
            "resolved wrong content: {content}"
        );

        // Project-local layer of the same name OVERRIDES the search-path one.
        std::fs::write(
            proj.join("inventory.layer"),
            "layer inventory\n  # LOCAL-OVERRIDE\n\n  construct Inventory\n    kw inventory\n    mt struct\n",
        )
        .unwrap();
        let reg = LayerRegistry::default();
        let local = reg.resolve_layer_content("inventory", &proj).unwrap();
        assert!(
            local.contains("LOCAL-OVERRIDE"),
            "project-local inventory.layer must win over search path: {local}"
        );

        // Removing the search path → a search-only name fails again.
        unsafe {
            std::env::remove_var("VEIL_SEARCH_PATHS");
        }
        let reg = LayerRegistry::default();
        let err2 = reg.resolve_layer_content("orders", &proj).unwrap_err();
        assert!(err2.contains("not found"), "expected not-found, got: {err2}");

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn search_path_roots_parses_named_and_skips_missing() {
        let _guard = SEARCH_PATHS_LOCK.lock().unwrap();
        let repo = mk_search_repo("roots-parse");
        unsafe {
            std::env::set_var(
                "VEIL_SEARCH_PATHS",
                format!("libs={}:/no/such/veil/repo", repo.display()),
            );
        }
        let roots = LayerRegistry::search_path_roots();
        // Only the existing dir survives; the `name=` prefix is stripped.
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(roots[0], repo);
        unsafe {
            std::env::remove_var("VEIL_SEARCH_PATHS");
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}
