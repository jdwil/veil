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
    pub constraints: Vec<String>,
    pub allowed_in: String,
    pub group: String,
    pub visual: Visual,
}

/// A statement definition loaded from a `.layer` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatementSpec {
    /// Source keyword, e.g. "dispatch".
    pub keyword: String,
    /// Raw maps_to value.
    pub maps_to: String,
    /// Resolved core statement shape.
    pub shape: StmtShape,
    /// If maps_to is `Port.method`, this is the port target name.
    pub port_target: Option<String>,
    /// If maps_to is `Port.method`, this is the method name.
    pub port_method: Option<String>,
    pub layer: String,
    pub desc: String,
    pub semantics: String,
    pub visual: Visual,
}

/// The resolved vocabulary for a compilation: built-in core constructs plus
/// everything from the loaded (possibly stacked) layers.
#[derive(Debug, Clone, Default)]
pub struct LayerRegistry {
    pub constructs: Vec<ConstructSpec>,
    pub statements: Vec<StatementSpec>,
    /// Names of layers loaded (in load order).
    pub layers: Vec<String>,
    /// Raw VEIL source blocks to inject into solutions using this registry.
    /// Each entry is one dedented construct block (e.g. a `port Bus` definition).
    pub declarations: Vec<String>,
}

impl LayerRegistry {
    /// Registry with only the core language built-ins.
    pub fn builtin() -> Self {
        let mut reg = LayerRegistry::default();
        let core = [
            ("mod", "Module", Shape::Mod, "📦", "#8b5cf6", "Module"),
            ("struct", "Struct", Shape::Struct, "📋", "#14b8a6", "Struct"),
            ("enum", "Enum", Shape::Enum, "🔀", "#8b5cf6", "Enum"),
            ("trait", "Trait", Shape::Trait, "🔌", "#10b981", "Trait"),
            ("impl", "Impl", Shape::Impl, "🔗", "#a855f7", "Implementation"),
            ("fn", "Fn", Shape::Fn, "⚡", "#f97316", "Function"),
            ("flow", "Flow", Shape::Fn, "🌊", "#f97316", "Flow"),
            ("group", "Group", Shape::Group, "📂", "#475569", "Group"),
        ];
        for (kw, name, shape, icon, color, label) in core {
            reg.constructs.push(ConstructSpec {
                name: name.to_string(),
                keyword: kw.to_string(),
                maps_to: shape.name().to_string(),
                shape,
                layer: "core".to_string(),
                desc: String::new(),
                contains: Vec::new(),
                blocks: Vec::new(),
                constraints: Vec::new(),
                allowed_in: "any".to_string(),
                group: String::new(),
                visual: Visual {
                    icon: icon.to_string(),
                    color: color.to_string(),
                    label: label.to_string(),
                },
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
    pub fn load_layer(&mut self, name: &str, dir: &Path) -> Result<(), String> {
        if self.layers.iter().any(|l| l == name) {
            return Ok(()); // already loaded
        }
        let path = dir.join(format!("{}.layer", name));
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read layer '{}' at {}: {}", name, path.display(), e))?;

        // First, load dependency layers (`use xxx` lines at pkg level).
        for line in content.lines() {
            let t = line.trim();
            if let Some(dep) = t.strip_prefix("use ") {
                self.load_layer(dep.trim(), dir)?;
            }
        }

        self.layers.push(name.to_string());
        let raw = parse_layer_file(&content, name);
        self.merge_and_resolve(raw)?;
        Ok(())
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
                let layer_path = dir.join(format!("{}.layer", name));
                if layer_path.exists() {
                    reg.load_layer(name, dir)?;
                }
                // Non-layer uses (package imports) are handled by the resolver.
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

struct RawLayer {
    constructs: Vec<ConstructSpec>,
    statements: Vec<StatementSpec>,
    /// Raw VEIL source blocks declared by this layer (e.g. `port Bus ...`).
    /// Each entry is one top-level construct declaration, dedented for parsing.
    declarations: Vec<String>,
}

/// Parse a `.layer` file into raw (shape-unresolved) specs.
fn parse_layer_file(content: &str, layer_name: &str) -> RawLayer {
    #[derive(PartialEq)]
    enum Section {
        None,
        Contains,
        Constraints,
        Visual,
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
                // Check if this is a new top-level item within declare
                if indent == declare_base_indent && !current_decl_lines.is_empty() {
                    // Flush previous declaration
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
                constraints: Vec::new(),
                allowed_in: String::new(),
                group: String::new(),
                visual: Visual {
                    label: name,
                    ..Default::default()
                },
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
            match trimmed {
                "contains" => {
                    section = Section::Contains;
                    continue;
                }
                "constraints" => {
                    section = Section::Constraints;
                    continue;
                }
                "visual" => {
                    section = Section::Visual;
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
                        if let Some(shape) = Shape::from_name(shape_name.trim()) {
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
                    if let Some(v) = trimmed.strip_prefix("keyword ") {
                        c.keyword = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("maps_to ") {
                        c.maps_to = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("desc ") {
                        c.desc = unquote(v);
                    } else if let Some(v) = trimmed.strip_prefix("allowed_in ") {
                        c.allowed_in = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("group ") {
                        c.group = v.trim().to_string();
                    }
                }
                Item::Statement(s) => {
                    if let Some(v) = trimmed.strip_prefix("maps_to ") {
                        s.maps_to = v.trim().to_string();
                    } else if let Some(v) = trimmed.strip_prefix("desc ") {
                        s.desc = unquote(v);
                    } else if let Some(v) = trimmed.strip_prefix("semantics ") {
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
}

pub fn palette_from_registry(reg: &LayerRegistry) -> Vec<PaletteEntry> {
    let mut out = Vec::new();
    for c in &reg.constructs {
        if c.layer == "core" && c.keyword != "flow" && c.keyword != "group" && c.keyword != "mod" {
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
