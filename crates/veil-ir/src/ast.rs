//! VEIL Abstract Syntax Tree definitions.
//!
//! The AST is fully generic: there are NO domain-specific node types.
//! Every layer-defined construct parses into `Construct`, stamped with the
//! resolved core `Shape` and the layer's construct name (`subkind`).
//! Layer-defined statements parse into `Expr::Action`.

use serde::{Deserialize, Serialize};

use crate::layer::{Shape, StmtShape};
use crate::span::Span;

/// Root of a VEIL file — solution, package, or composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VeilFile {
    Solution(Solution),
    Package(Package),
    Composition(Composition),
}

/// A VEIL package — reusable library of building blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
    pub span: Span,
    pub metadata: Vec<PackageMeta>,
    /// Layer/package references (`use ddd`).
    #[serde(default)]
    pub uses: Vec<UseImport>,
    /// External Cargo crate links (`link veil_server path "..."`). CAP-001.
    #[serde(default)]
    pub links: Vec<LinkDecl>,
    /// Stock packages this package specializes (`adapt wear_test`). ADP.
    #[serde(default)]
    pub adapts: Vec<AdaptDecl>,
    /// Path patches applied after base merge (`ins`/`rfn`/`rpl`/`omit`/`ren`).
    #[serde(default)]
    pub patches: Vec<AdaptPatch>,
    pub items: Vec<TopLevelItem>,
    pub expose: Option<ExposeBlock>,
}

/// `adapt wear_test` — pull base package source into this compile unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptDecl {
    pub package_name: String,
    pub span: Span,
}

/// Path into merged IR: `CreateInitiative`, `CreateInitiative.step persist`, `Initiative.fn mark_vip`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdaptPath {
    pub segments: Vec<AdaptPathSeg>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AdaptPathSeg {
    /// Bare name: service, construct, fn, …
    Name(String),
    /// `.step <name>`
    Step(String),
    /// `.fn <name>`
    Fn(String),
}

impl AdaptPath {
    pub fn from_name(name: &str) -> Self {
        Self {
            segments: vec![AdaptPathSeg::Name(name.into())],
        }
    }

    pub fn display(&self) -> String {
        let mut out = String::new();
        for (i, s) in self.segments.iter().enumerate() {
            match s {
                AdaptPathSeg::Name(n) => {
                    if i > 0 {
                        out.push('.');
                    }
                    out.push_str(n);
                }
                AdaptPathSeg::Step(n) => {
                    out.push_str(".step ");
                    out.push_str(n);
                }
                AdaptPathSeg::Fn(n) => {
                    out.push_str(".fn ");
                    out.push_str(n);
                }
            }
        }
        out
    }
}

/// Where to insert a new step relative to existing ones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum StepPosition {
    #[default]
    AtEnd,
    AtStart,
    Before(String),
    After(String),
}

/// One adapt patch in source order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdaptPatch {
    /// Insert members/steps into an existing construct or service.
    Ins {
        path: AdaptPath,
        /// For step inserts: optional position.
        #[serde(default)]
        position: StepPosition,
        /// Members to insert: steps (as FlowStep-like via Construct steps) or
        /// nested constructs / methods carried as free Constructs / FnDefs.
        items: Vec<AdaptInsItem>,
        span: Span,
    },
    /// Refine body; may contain `stock` Expr.
    Rfn {
        path: AdaptPath,
        /// New steps for svc/fn (FlowStep list) or raw exprs for free fn body.
        steps: Vec<FlowStep>,
        /// Free-function body when target is a free fn (optional alternative to steps).
        #[serde(default)]
        body: Vec<Expr>,
        span: Span,
    },
    /// Replace body; stock illegal.
    Rpl {
        path: AdaptPath,
        steps: Vec<FlowStep>,
        #[serde(default)]
        body: Vec<Expr>,
        span: Span,
    },
    /// Remove symbol or step.
    Omit { path: AdaptPath, span: Span },
    /// Rename symbol; rewrite references in merged IR.
    Ren {
        path: AdaptPath,
        new_name: String,
        span: Span,
    },
}

/// Content of an `ins` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdaptInsItem {
    /// A flow step with optional insert position relative to existing steps.
    Step {
        step: FlowStep,
        #[serde(default)]
        position: StepPosition,
    },
    /// Nested construct (e.g. method-shaped construct under aggregate).
    Construct(Construct),
    /// Free method as FnDef (fn name on aggregate).
    Function(FnDef),
}

/// Package metadata (author, desc, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMeta {
    pub key: String,
    pub value: String,
    pub span: Span,
}

/// A composition file — imports packages and wires flows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Composition {
    pub imports: Vec<UseImport>,
    pub flows: Vec<Flow>,
    pub span: Span,
}

/// A use/import statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseImport {
    pub package_name: String,
    pub alias: Option<String>,
    pub span: Span,
}

/// External Cargo dependency declared with `link` (CAP-001).
///
/// ```veil
/// link veil_server
/// link veil_local path "../../crates/veil-local" features "local"
/// ```
///
/// Codegen emits path deps into generated `Cargo.toml`. Unallowlisted crates
/// require an explicit relative `path`. Absolute paths are rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkDecl {
    /// Crate name as written (`veil_server` or `veil-server`).
    pub name: String,
    /// Optional path relative to the generated workspace root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional Cargo features.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    pub span: Span,
}

/// The expose block — defines what consumers can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExposeBlock {
    pub span: Span,
    pub nodes: Vec<ExposedNode>,
    pub constraints: Vec<String>,
}

/// An exposed node — a pre-built action consumers can use in flows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExposedNode {
    pub name: String,
    pub description: Option<String>,
    pub inputs: Vec<Field>,
    pub outputs: Vec<Field>,
    pub span: Span,
}

/// Root of a VEIL solution file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Solution {
    pub name: String,
    pub span: Span,
    /// Layer/package references (`use ddd`).
    #[serde(default)]
    pub uses: Vec<UseImport>,
    /// External Cargo crate links (`link veil_server`). CAP-001.
    #[serde(default)]
    pub links: Vec<LinkDecl>,
    pub items: Vec<TopLevelItem>,
    /// Public API contract from `pkg` files' `expose` block (preserved when
    /// packages are lowered to Solution for check/codegen).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose: Option<ExposeBlock>,
}

/// Top-level items within a solution or package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TopLevelItem {
    Lang(LangBlock),
    Construct(Construct),
    Flow(Flow),
    /// Type alias: `type X = Y`
    TypeAlias { name: String, target: TypeExpr },
    /// Constant: `const NAME = value`
    Const { name: String, value: Expr },
    /// Static variable: `static [mut] NAME: Type = value`
    Static { name: String, mutable: bool, value: Expr },
    /// A free function with an expression body: `fn name(params) -> T { body }`.
    /// Used for reusable code declared in a layer's `declare` block (e.g. the
    /// saga coordinator). Distinct from fn-shaped Constructs (svc/saga), which
    /// carry steps rather than a raw body.
    Function(FnDef),
}

/// Ubiquitous language definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangBlock {
    pub span: Span,
    pub entries: Vec<LangEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangEntry {
    pub term: String,
    pub definition: String,
    pub span: Span,
}

/// The one construct type. Which fields are populated depends on `shape`:
///
/// - `Mod`/`Group` — `children`
/// - `Struct` — `fields`, `blocks`, `fns`, nested `children`, `return_type`
/// - `Enum` — `variants`, `transitions`
/// - `Trait` — `methods`
/// - `Impl` — `target`, `impls`
/// - `Fn` — `inputs`, `steps`, `return_expr`, `refs`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Construct {
    /// The keyword used in source (e.g. "agg", "port", or a stacked-layer keyword).
    pub keyword: String,
    /// The layer construct name (e.g. "Aggregate"). Used as the IR subkind.
    pub subkind: String,
    /// Resolved core parse shape.
    pub shape: Shape,
    pub name: String,
    /// Generic type parameters (e.g. ["T", "U"]).
    pub type_params: Vec<String>,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    /// Was this construct prefixed with `export`?
    pub exported: bool,
    /// Visibility modifier: "pub", "pub(crate)", "pub(super)", or "" (private).
    pub visibility: String,
    /// Where clause on generics: ["T: Send + Sync", "U: Clone"]
    pub where_clause: Vec<String>,
    /// Whether this construct is a deployment unit boundary (`au`).
    /// All exported items beneath it group into one service.
    pub deployment_unit: bool,
    /// True when this construct was injected from a layer's `declare` section
    /// (e.g. the `Bus` port from ddd.layer) rather than authored by the user.
    /// Layer-provided constructs are not re-emitted by the serializer and are
    /// visually distinguished in the viewer.
    #[serde(default)]
    pub layer_provided: bool,

    // ─── struct shape ─────────────────────────────────────────────────
    pub fields: Vec<Field>,
    /// Optional trailing `-> Type` line (e.g. command return types).
    pub return_type: Option<TypeExpr>,
    /// Named sub-blocks declared by the layer (`root: struct`, `state: enum`).
    pub blocks: Vec<NamedBlock>,
    /// Raw string content blocks (e.g. `template`, `style`). Stored as-is,
    /// emitted verbatim by target-specific codegen.
    #[serde(default)]
    pub raw_blocks: Vec<(String, String)>,  // (name, content)
    /// Effect blocks — named reactive side-effects (e.g. Svelte 5 $effect).
    #[serde(default)]
    pub effects: Vec<EffectBlock>,
    /// Nested function definitions (business logic methods).
    pub fns: Vec<FnDef>,

    // ─── enum shape ───────────────────────────────────────────────────
    pub variants: Vec<String>,
    /// Rich enum variants with optional data (tuple/struct variants).
    /// When populated, these take precedence over the flat `variants` list.
    #[serde(default)]
    pub rich_variants: Vec<EnumVariant>,
    pub transitions: Vec<StateTransition>,

    // ─── trait shape ──────────────────────────────────────────────────
    pub methods: Vec<Method>,
    /// Associated types: `type Item = T` or `type Item` (unbound).
    pub associated_types: Vec<(String, Option<TypeExpr>)>,

    // ─── impl shape ───────────────────────────────────────────────────
    /// Target trait-shaped construct (`kw Name for Target`).
    pub target: Option<String>,
    /// Type arguments on the target: `for EntityRepo<WearTest>` → `[WearTest]`.
    #[serde(default)]
    pub target_type_args: Vec<TypeExpr>,
    pub impls: Vec<MethodImpl>,

    // ─── fn shape ─────────────────────────────────────────────────────
    pub inputs: Vec<Field>,
    pub steps: Vec<FlowStep>,
    pub return_expr: Option<Box<Expr>>,
    /// Reference lines (`contexts Identity, Billing`).
    pub refs: Vec<RefLine>,

    // ─── mod/group shape + nesting ────────────────────────────────────
    pub children: Vec<Construct>,
}

impl Construct {
    pub fn new(keyword: &str, subkind: &str, shape: Shape, name: String, span: Span) -> Self {
        Construct {
            keyword: keyword.to_string(),
            subkind: subkind.to_string(),
            shape,
            name,
            type_params: Vec::new(),
            span,
            annotations: Vec::new(),
            exported: false,
            deployment_unit: false,
            visibility: String::new(),
            where_clause: Vec::new(),
            layer_provided: false,
            fields: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            raw_blocks: Vec::new(),
            effects: Vec::new(),
            fns: Vec::new(),
            variants: Vec::new(),
            rich_variants: Vec::new(),
            transitions: Vec::new(),
            methods: Vec::new(),
            associated_types: Vec::new(),
            target: None,
            target_type_args: Vec::new(),
            impls: Vec::new(),
            inputs: Vec::new(),
            steps: Vec::new(),
            return_expr: None,
            refs: Vec::new(),
            children: Vec::new(),
        }
    }
}

/// A named sub-block within a struct-shaped construct, declared by the layer
/// via `contains` entries like `root: struct` or `state: enum`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedBlock {
    pub keyword: String,
    pub shape: Shape,
    /// Optional block name (`state CustomerStatus`).
    pub name: Option<String>,
    pub fields: Vec<Field>,
    pub variants: Vec<String>,
    pub transitions: Vec<StateTransition>,
    pub span: Span,
}

/// An effect block — a named reactive side-effect with an expression body.
/// Svelte 5: `$effect(() => { ... })`. Can optionally have a cleanup body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectBlock {
    pub name: String,
    pub body: Vec<Expr>,
    pub cleanup: Vec<Expr>,
    pub span: Span,
}

/// A reference line — `keyword Name, Name, ...` metadata inside a construct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefLine {
    pub keyword: String,
    pub values: Vec<String>,
    pub span: Span,
}

/// A state transition: From -> To
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransition {
    pub from: String,
    pub to: String,
    pub span: Span,
}

/// A nested function definition (business logic within a construct).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnDef {
    pub name: String,
    pub span: Span,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub annotations: Vec<Annotation>,
    pub body: Vec<Expr>,
    /// True when this function was injected from a layer's `declare` block
    /// (e.g. the saga coordinator). Not re-emitted by the serializer.
    #[serde(default)]
    pub layer_provided: bool,
}

/// A method signature on a trait-shaped construct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    pub span: Span,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
}

/// A method implementation within an impl-shaped construct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodImpl {
    pub method_name: String,
    pub params: Vec<String>,
    pub span: Span,
    pub body: Vec<Expr>,
}

/// A behavioral flow (use case orchestration). Core language construct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    pub name: String,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    pub inputs: Vec<Field>,
    pub steps: Vec<FlowStep>,
    pub error_boundary: Option<ErrorBoundary>,
    pub return_expr: Option<Expr>,
}

/// A step within a flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlowStep {
    Step(StepDef),
    Parallel(ParBlock),
    Match(MatchBlock),
}

/// A named sequential step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    pub name: String,
    pub span: Span,
    pub body: Vec<Expr>,
    /// Reference lines within the step (`ctx Identity`).
    pub refs: Vec<RefLine>,
    /// Named expression sub-blocks within the step (`compensate`).
    pub sub_blocks: Vec<SubBlock>,
}

/// A named expression block within a step (e.g. compensation logic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubBlock {
    pub keyword: String,
    pub body: Vec<Expr>,
    pub span: Span,
}

/// A parallel execution block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParBlock {
    pub span: Span,
    pub steps: Vec<StepDef>,
}

/// A match/branch block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchBlock {
    pub span: Span,
    pub expr: Expr,
    pub arms: Vec<MatchArm>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: String,
    /// Structured pattern (when available, takes precedence over string pattern).
    #[serde(default)]
    pub rich_pattern: Option<Pattern>,
    /// Optional guard expression: `pattern if guard -> body`
    #[serde(default)]
    pub guard: Option<Expr>,
    pub span: Span,
    pub body: Vec<Expr>,
}

/// Error boundary with retry/timeout/fallback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBoundary {
    pub span: Span,
    pub annotations: Vec<Annotation>,
    pub fallback: Option<Expr>,
}

/// A field definition (name: Type).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
    pub name: String,
    pub type_expr: TypeExpr,
    /// Optional default/computed expression (e.g. `count: Int = 0` or
    /// `filtered: List<T> = items.filter(predicate)`). The meaning is
    /// determined by the layer and codegen target, not the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_expr: Option<Expr>,
    pub span: Span,
}

/// A parameter in a method signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub type_expr: TypeExpr,
    pub span: Span,
}

/// A type expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeExpr {
    Named(String),
    Generic(String, Vec<TypeExpr>),
    Result(Option<Box<TypeExpr>>),
    Optional(Box<TypeExpr>),
    List(Box<TypeExpr>),
    Map(Box<TypeExpr>, Box<TypeExpr>),
    Set(Box<TypeExpr>),
    /// Tuple type: (A, B, C)
    Tuple(Vec<TypeExpr>),
    /// Fixed-size array: [T; N]
    Array(Box<TypeExpr>, usize),
    /// Reference type: &T or &mut T
    Ref(Box<TypeExpr>, bool), // (inner, is_mut)
    /// dyn Trait
    Dyn(Box<TypeExpr>),
    /// impl Trait (return position)
    ImplTrait(Box<TypeExpr>),
    /// Function pointer: fn(A, B) -> C
    FnPtr(Vec<TypeExpr>, Option<Box<TypeExpr>>),
}

/// An annotation (@keyword or @keyword(args)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub name: String,
    pub args: Vec<String>,
    pub span: Span,
}

/// An expression. Only core language constructs — layer statements become `Action`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// Transpile-time prior body splice inside `rfn` (ADP-007). Never remains after merge.
    Stock,
    /// Identifier reference
    Ident(String),
    /// Field access: expr.field
    FieldAccess(Box<Expr>, String),
    /// Function/method call (the only core invocation primitive)
    Call(CallExpr),
    /// Binary operation: left op right
    BinaryOp(BinaryOpExpr),
    /// Unary operation: op expr
    UnaryOp(UnaryOpExpr),
    /// If/else expression
    IfExpr(IfExprData),
    /// Variable assignment: `name = expr` or typed `name: Type = expr`
    Assign(String, Box<Expr>, Option<TypeExpr>),
    /// Mutable variable assignment: mut name = expr (with optional type annotation)
    MutAssign(String, Box<Expr>, Option<TypeExpr>),
    /// String literal
    StringLit(String),
    /// Integer literal
    IntLit(i64),
    /// Float literal
    FloatLit(f64),
    /// Boolean literal
    BoolLit(bool),
    /// Return expression
    Return(Box<Expr>),
    /// Layer-defined statement (dispatch, invoke, guard, emit, ...).
    Action(ActionExpr),
    /// Struct literal: `Name { field: expr, ... }`.
    StructLit(String, Vec<(String, Expr)>),
    /// Match expression: `match <expr> { pattern -> body, ... }`.
    Match(Box<Expr>, Vec<MatchArm>),
    /// For loop: `for <binding> in <iterable> { body }`.
    ForLoop { binding: String, index: Option<String>, iterable: Box<Expr>, body: Vec<Expr> },
    /// While loop: `while <condition> { body }`.
    WhileLoop { condition: Box<Expr>, body: Vec<Expr> },
    /// Closure: `|params| body`.
    Closure { params: Vec<String>, body: Vec<Expr> },
    /// Tuple expression: (a, b, c)
    Tuple(Vec<Expr>),
    /// String interpolation: f"Hello {name}"
    StringInterp(Vec<StringPart>),
    /// Await expression: `await <expr>` → `<expr>.await`
    Await(Box<Expr>),
    /// Break out of a loop.
    Break,
    /// Continue to next loop iteration.
    Continue,
    /// Index access: `expr[index]`
    Index(Box<Expr>, Box<Expr>),
    /// Array literal: `[1, 2, 3]`
    ArrayLit(Vec<Expr>),
    /// Range expression: `start..end` or `start..=end`
    Range { start: Option<Box<Expr>>, end: Option<Box<Expr>>, inclusive: bool },
    /// Infinite loop: `loop { body }`
    Loop(Vec<Expr>),
    /// Cast: `expr as Type`
    Cast(Box<Expr>, String),
    /// Try/question-mark: `expr?`
    Try(Box<Expr>),
    /// Struct update: `Name { field: val, ..base }`
    StructUpdate { name: String, fields: Vec<(String, Expr)>, base: Box<Expr> },
    /// If let: `if let pattern = expr { body } else { body }`
    IfLet { pattern: String, expr: Box<Expr>, then_body: Vec<Expr>, else_body: Option<Vec<Expr>> },
    /// While let: `while let pattern = expr { body }`
    WhileLet { pattern: String, expr: Box<Expr>, body: Vec<Expr> },
    /// Let binding with destructuring pattern: `let (a, b) = expr` (with optional type annotation)
    LetPattern(Pattern, Box<Expr>, Option<TypeExpr>),
}

/// Part of an interpolated string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StringPart {
    Literal(String),
    Expr(Expr),
}

/// A destructuring pattern used in let bindings, match arms, and if-let.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pattern {
    /// Simple binding: `x` or `_`
    Ident(String),
    /// Tuple destructuring: `(a, b, c)`
    Tuple(Vec<Pattern>),
    /// Struct destructuring: `Name { field1, field2, .. }`
    Struct(String, Vec<(String, Option<Pattern>)>, bool), // (name, fields, has_rest)
    /// Enum variant: `Variant(a, b)` or `Variant { x, y }`
    Variant(String, Vec<Pattern>),
    /// Literal pattern: `42`, `"hello"`, `true`
    Literal(String),
    /// Or-pattern: `A | B | C`
    Or(Vec<Pattern>),
    /// Wildcard: `_`
    Wildcard,
    /// Rest/spread: `..`
    Rest,
}

impl Pattern {
    /// Convert pattern to a string representation (for display and backward compat).
    pub fn to_string_repr(&self) -> String {
        match self {
            Pattern::Ident(s) => s.clone(),
            Pattern::Tuple(parts) => {
                let inner = parts.iter().map(|p| p.to_string_repr()).collect::<Vec<_>>().join(", ");
                format!("({})", inner)
            }
            Pattern::Struct(name, fields, has_rest) => {
                let mut fs: Vec<String> = fields.iter().map(|(k, v)| {
                    match v {
                        Some(pat) => format!("{}: {}", k, pat.to_string_repr()),
                        None => k.clone(),
                    }
                }).collect();
                if *has_rest { fs.push("..".to_string()); }
                format!("{} {{ {} }}", name, fs.join(", "))
            }
            Pattern::Variant(name, args) => {
                if args.is_empty() { name.clone() }
                else {
                    let inner = args.iter().map(|p| p.to_string_repr()).collect::<Vec<_>>().join(", ");
                    format!("{}({})", name, inner)
                }
            }
            Pattern::Literal(s) => s.clone(),
            Pattern::Or(alts) => alts.iter().map(|p| p.to_string_repr()).collect::<Vec<_>>().join(" | "),
            Pattern::Wildcard => "_".to_string(),
            Pattern::Rest => "..".to_string(),
        }
    }
}

/// An enum variant with optional associated data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnumVariant {
    /// Unit variant: `Pending`, `Active`
    Unit(String),
    /// Tuple variant: `Message(String, u32)`
    Tuple(String, Vec<TypeExpr>),
    /// Struct variant: `Error { code: Int, message: Str }`
    Struct(String, Vec<Field>),
}

impl EnumVariant {
    pub fn name(&self) -> &str {
        match self {
            EnumVariant::Unit(n) | EnumVariant::Tuple(n, _) | EnumVariant::Struct(n, _) => n,
        }
    }
}

/// A layer-defined statement, parsed according to its core statement shape.
///
/// - `Call` shape: `kw Target(.method)? (args)` or `kw Target{name: expr, ...}`
/// - `If` shape: `kw <condition> (, "message")?`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionExpr {
    /// The statement keyword from the layer (e.g. "dispatch").
    pub keyword: String,
    pub shape: StmtShape,
    // Call shape
    pub target: String,
    pub method: String,
    pub args: Vec<Expr>,
    pub named_args: Vec<(String, Expr)>,
    // If shape
    pub condition: Option<Box<Expr>>,
    pub message: Option<String>,
    pub span: Span,
}

/// Binary operator kinds
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BinOp {
    Add,    // +
    Sub,    // -
    Mul,    // *
    Div,    // /
    Mod,    // %
    Eq,     // ==
    NotEq,  // !=
    Lt,     // <
    Gt,     // >
    LtEq,   // <=
    GtEq,   // >=
    And,    // &&
    Or,     // ||
}

/// Unary operator kinds
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UnaryOp {
    Not,    // !
    Neg,    // - (negation)
}

/// Binary operation expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryOpExpr {
    pub left: Box<Expr>,
    pub op: BinOp,
    pub right: Box<Expr>,
}

/// Unary operation expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnaryOpExpr {
    pub op: UnaryOp,
    pub expr: Box<Expr>,
}

/// If/else expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IfExprData {
    pub condition: Box<Expr>,
    pub then_body: Vec<Expr>,
    pub else_body: Option<Vec<Expr>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallExpr {
    /// Named target for the base of a call (e.g. `Repo` in `Repo.find(id)`).
    /// Empty when the call is a method invocation on `receiver` (a chain link).
    pub target: String,
    pub method: String,
    pub args: Vec<Expr>,
    /// Expression receiver for method chaining: `<receiver>.method(args)`.
    /// Set when the call is a postfix `.method()` on another expression
    /// (e.g. the `.collect()` in `items.map(f).collect()`). When present,
    /// `target` is empty and the receiver carries the left side of the chain.
    #[serde(default)]
    pub receiver: Option<Box<Expr>>,
    /// Original statement keyword for round-trip fidelity (e.g. "dispatch").
    /// When present, the serializer emits `dispatch Evt{...}` instead of `Bus.dispatch(...)`.
    #[serde(default)]
    pub sugar: Option<String>,
    pub span: Span,
}
