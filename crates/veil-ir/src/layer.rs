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
use std::path::Path;

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
}

impl StmtShape {
    pub fn from_name(s: &str) -> Option<StmtShape> {
        match s {
            "call" => Some(StmtShape::Call),
            "if" => Some(StmtShape::If),
            _ => None,
        }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationSpec {
    pub name: String,
    pub desc: String,
    /// Parameter names (rendered as free-text inputs by the viewer).
    pub params: Vec<String>,
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
    pub layer: String,
    pub desc: String,
    pub semantics: String,
    pub visual: Visual,
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
    /// Loaded third-party crate stubs.
    pub stubs: Vec<StubCrate>,
    /// External layer resolver — called when a layer isn't found locally or in system.
    /// Provided by the hosting runtime (e.g. veil-runtime for database-backed resolution).
    pub external_resolver: Option<Box<dyn Fn(&str) -> Option<String> + Send + Sync>>,
}

impl Default for LayerRegistry {
    fn default() -> Self {
        Self {
            constructs: Vec::new(),
            statements: Vec::new(),
            layers: Vec::new(),
            declarations: Vec::new(),
            stubs: Vec::new(),
            external_resolver: None,
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
            stubs: self.stubs.clone(),
            external_resolver: None, // resolver is not cloneable — cleared on clone
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
            ("step", "Step", Shape::Fn, "▶", "#3b82f6", "Step", "Flow"),
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
                annotations: Vec::new(),
                runtime: None,
                tgt: String::new(),
                dg: String::new(),
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

    /// Look up a statement by its source keyword.
    pub fn statement(&self, keyword: &str) -> Option<&StatementSpec> {
        self.statements.iter().find(|s| s.keyword == keyword)
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
    /// These are the traits that statements target via `maps_to Port.method` (e.g. Bus).
    /// Orchestrators keep only these as direct deps; all other calls route through them.
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
    /// Load a layer by name, searching in order:
    /// 1. The provided local directory
    /// 2. The system layers directory (ships with VEIL)
    /// 3. An external resolver (if configured)
    pub fn load_layer(&mut self, name: &str, dir: &Path) -> Result<(), String> {
        if self.layers.iter().any(|l| l == name) {
            return Ok(()); // already loaded
        }

        // Resolution order: local → system → external
        let content = self.resolve_layer_content(name, dir)?;

        // First, load dependency layers (`use xxx` lines at pkg level).
        // Skip silently if not found — it might be a .stub or package reference.
        for line in content.lines() {
            let t = line.trim();
            if let Some(dep) = t.strip_prefix("use ") {
                let _ = self.load_layer(dep.trim(), dir);
            }
        }

        self.layers.push(name.to_string());
        let raw = parse_layer_file(&content, name);
        self.merge_and_resolve(raw)?;
        Ok(())
    }

    /// Resolve layer content by searching multiple locations.
    fn resolve_layer_content(&self, name: &str, local_dir: &Path) -> Result<String, String> {
        // 1. Local directory (same dir as the .veil file)
        let local_path = local_dir.join(format!("{}.layer", name));
        if local_path.exists() {
            return std::fs::read_to_string(&local_path)
                .map_err(|e| format!("cannot read layer '{}' at {}: {}", name, local_path.display(), e));
        }

        // 2. System layers directory
        if let Some(content) = Self::load_system_layer(name) {
            return Ok(content);
        }

        // 3. External resolver (port for veil-runtime or other backends)
        if let Some(resolver) = &self.external_resolver {
            if let Some(content) = resolver(name) {
                return Ok(content);
            }
        }

        Err(format!(
            "layer '{}' not found (searched: {}, system layers)",
            name, local_dir.display()
        ))
    }

    /// Load a layer from the system layers directory.
    /// Searches relative to the VEIL binary or the VEIL_LAYERS_DIR env var.
    fn load_system_layer(name: &str) -> Option<String> {
        // Check VEIL_LAYERS_DIR env var first
        if let Ok(dir) = std::env::var("VEIL_LAYERS_DIR") {
            let path = Path::new(&dir).join(format!("{}.layer", name));
            if path.exists() {
                return std::fs::read_to_string(&path).ok();
            }
        }

        // Check relative to the executable (for installed VEIL)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                // Try ../layers/ (standard install layout)
                let path = exe_dir.join("../layers").join(format!("{}.layer", name));
                if path.exists() {
                    return std::fs::read_to_string(&path).ok();
                }
                // Try ./layers/ (dev layout)
                let path = exe_dir.join("layers").join(format!("{}.layer", name));
                if path.exists() {
                    return std::fs::read_to_string(&path).ok();
                }
            }
        }

        // Try workspace root /layers/ (for dev)
        let path = Path::new("layers").join(format!("{}.layer", name));
        if path.exists() {
            return std::fs::read_to_string(&path).ok();
        }

        None
    }

    /// Load a layer from in-memory content (no `use` dependency resolution).
    pub fn load_content(&mut self, name: &str, content: &str) -> Result<(), String> {
        if self.layers.iter().any(|l| l == name) {
            return Ok(());
        }
        self.layers.push(name.to_string());
        let raw = parse_layer_file(content, name);
        self.merge_and_resolve(raw)
    }

    /// Build a registry for a `.veil` file: built-ins plus every layer the
    /// file references via `use` lines. Layer resolution is transitive.
    pub fn for_veil_file(veil_path: &Path) -> Result<Self, String> {
        let mut reg = LayerRegistry::builtin();
        let dir = veil_path.parent().unwrap_or(Path::new("."));
        let content = std::fs::read_to_string(veil_path)
            .map_err(|e| format!("cannot read {}: {}", veil_path.display(), e))?;
        for line in content.lines() {
            let t = line.trim();
            if let Some(name) = t.strip_prefix("use ") {
                // Strip aliases: "use onboarding_kit as ok"
                let name = name.split_whitespace().next().unwrap_or("");

                // Try to load as a layer (searches local → system → external)
                let _ = reg.load_layer(name, dir);

                // Also check for .stub files (local dir, then stubs/ subdir)
                let stub_path = dir.join(format!("{}.stub", name));
                let stub_subdir_path = dir.join("stubs").join(format!("{}.stub", name));
                let found_stub = if stub_path.exists() {
                    Some(stub_path)
                } else if stub_subdir_path.exists() {
                    Some(stub_subdir_path)
                } else {
                    None
                };
                if let Some(path) = found_stub {
                    if let Ok(stub_content) = std::fs::read_to_string(&path) {
                        if let Some(stub) = parse_stub_file(&stub_content) {
                            reg.stubs.push(stub);
                        }
                    }
                }
            }
        }
        Ok(reg)
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

/// A parsed `.stub` file — declares the public API of an external Rust crate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StubCrate {
    /// The crate name (e.g. "reqwest").
    pub name: String,
    /// The crate version (e.g. "0.12").
    pub version: String,
    /// Struct declarations with their methods.
    pub structs: Vec<StubStruct>,
    /// Impl blocks (methods grouped by target type).
    pub impls: Vec<StubImpl>,
}

/// A struct declared in a stub file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubStruct {
    pub name: String,
    /// Methods declared directly on the struct (instance methods).
    pub methods: Vec<StubMethod>,
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
    pub params: Vec<(String, String)>, // (param_name, type_string)
    pub return_type: Option<String>,   // VEIL type syntax (e.g. "Res!<Str>")
}

/// Parse a `.stub` file into a StubCrate.
pub fn parse_stub_file(content: &str) -> Option<StubCrate> {
    let mut stub = StubCrate::default();
    let mut current_struct: Option<StubStruct> = None;
    let mut current_impl: Option<StubImpl> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        // Header: stub <name> <version>
        if trimmed.starts_with("stub ") {
            let parts: Vec<&str> = trimmed.strip_prefix("stub ").unwrap().split_whitespace().collect();
            stub.name = parts.first().unwrap_or(&"").to_string();
            stub.version = parts.get(1).unwrap_or(&"*").to_string();
            continue;
        }

        // Top-level struct declaration
        if indent <= 2 && trimmed.starts_with("struct ") {
            // Flush previous
            if let Some(s) = current_struct.take() { stub.structs.push(s); }
            if let Some(i) = current_impl.take() { stub.impls.push(i); }
            let name = trimmed.strip_prefix("struct ").unwrap().trim().to_string();
            current_struct = Some(StubStruct { name, methods: Vec::new() });
            continue;
        }

        // Top-level impl declaration
        if indent <= 2 && trimmed.starts_with("impl ") {
            if let Some(s) = current_struct.take() { stub.structs.push(s); }
            if let Some(i) = current_impl.take() { stub.impls.push(i); }
            let target = trimmed.strip_prefix("impl ").unwrap().trim().to_string();
            current_impl = Some(StubImpl { target, methods: Vec::new() });
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

    let params: Vec<(String, String)> = if params_str.is_empty() {
        Vec::new()
    } else {
        params_str.split(',').map(|p| {
            let p = p.trim();
            if let Some((name, ty)) = p.split_once(':') {
                (name.trim().to_string(), ty.trim().to_string())
            } else {
                (p.to_string(), "Str".to_string())
            }
        }).collect()
    };

    StubMethod { name, params, return_type: ret }
}


/// Parse a `.layer` file into raw (shape-unresolved) specs.

struct RawLayer {
    constructs: Vec<ConstructSpec>,
    statements: Vec<StatementSpec>,
    /// Raw VEIL source blocks declared by this layer (e.g. `port Bus ...`).
    /// Each entry is one top-level construct declaration, dedented for parsing.
    declarations: Vec<String>,
}

fn parse_layer_file(content: &str, layer_name: &str) -> RawLayer {
    #[derive(PartialEq)]
    enum Section {
        None,
        Contains,
        Constraints,
        Visual,
        Annotations,
        Runtime,
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

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            // Blank lines inside declare blocks are preserved
            if in_declare && !current_decl_lines.is_empty() {
                current_decl_lines.push(String::new());
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        // Handle `declare` section: accumulate raw VEIL source text
        if trimmed == "declare" && indent <= 2 {
            // Flush any previous construct/statement
            if let Some(item) = current.take() {
                items.push(item);
            }
            in_declare = true;
            declare_base_indent = indent + 2; // items inside declare are at +2
            section = Section::None;
            continue;
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

        if trimmed.starts_with("construct ") {
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
                visual: Visual {
                    label: name,
                    ..Default::default()
                },
                annotations: Vec::new(),
                runtime: None,
                tgt: String::new(),
                dg: String::new(),
            }));
            section = Section::None;
            continue;
        }
        if trimmed.starts_with("statement ") {
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

        let Some(item) = current.as_mut() else { continue };

        // Section headers (indent 4 = direct child of construct/statement).
        if indent <= 4 {
            // `runtime <coordinator> <step_trait>` opens a runtime binding whose
            // nested `sub_block -> method` lines fill the method map.
            if let Some(rest) = trimmed.strip_prefix("runtime ") {
                let mut parts = rest.split_whitespace();
                let coordinator = parts.next().unwrap_or("").to_string();
                let step_trait = parts.next().unwrap_or("").to_string();
                if let Item::Construct(c) = item {
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
                    section = Section::Contains;
                    continue;
                }
                "cst" | "constraints" => {
                    section = Section::Constraints;
                    continue;
                }
                "visual" => {
                    section = Section::Visual;
                    continue;
                }
                "ann" | "annotations" => {
                    section = Section::Annotations;
                    continue;
                }
                _ => section = Section::None,
            }
        }

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
                        let params = params_str
                            .split(',')
                            .map(|p| p.trim().to_string())
                            .filter(|p| !p.is_empty())
                            .collect();
                        c.annotations.push(AnnotationSpec {
                            name: name.trim().to_string(),
                            desc,
                            params,
                        });
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
                    }
                }
            },
        }
    }
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
    RawLayer { constructs, statements, declarations }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
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
}

pub fn palette_from_registry(reg: &LayerRegistry) -> Vec<PaletteEntry> {
    let mut out = Vec::new();
    for c in &reg.constructs {
        if c.layer == "core" && c.keyword != "flow" && c.keyword != "group" && c.keyword != "mod" && c.keyword != "step" {
            // Core type primitives are implicit; keep the palette focused on
            // structural + layer vocabulary. mod/group/flow stay draggable.
            if reg.constructs.iter().any(|o| o.layer != "core") {
                continue;
            }
        }
        out.push(PaletteEntry {
            name: c.name.clone(),
            keyword: c.keyword.clone(),
            kind: shape_to_node_kind(c.shape).to_string(),
            shape: c.shape.name().to_string(),
            icon: c.visual.icon.clone(),
            color: c.visual.color.clone(),
            label: c.visual.label.clone(),
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
        });
    }
    for s in &reg.statements {
        out.push(PaletteEntry {
            name: s.keyword.clone(),
            keyword: s.keyword.clone(),
            kind: "Action".to_string(),
            shape: match s.shape {
                StmtShape::Call => "call".to_string(),
                StmtShape::If => "if".to_string(),
            },
            icon: s.visual.icon.clone(),
            color: s.visual.color.clone(),
            label: s.visual.label.clone(),
            group: String::new(),
            allowed_in: "Step".to_string(),
            layer: s.layer.clone(),
            entry_type: "statement".to_string(),
            au: false,
            annotations: Vec::new(),
            expected_groups: Vec::new(),
            tgt: String::new(),
            dg: String::new(),
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

    #[test]
    fn layer_annotations_parse_and_reach_palette() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../examples/ddd.layer"))
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
}
