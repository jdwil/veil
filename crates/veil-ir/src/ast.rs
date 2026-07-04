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
    pub items: Vec<TopLevelItem>,
    pub expose: Option<ExposeBlock>,
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
    pub items: Vec<TopLevelItem>,
}

/// Top-level items within a solution or package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TopLevelItem {
    Lang(LangBlock),
    Construct(Construct),
    Flow(Flow),
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
    pub span: Span,
    pub annotations: Vec<Annotation>,
    /// Was this construct prefixed with `export`?
    pub exported: bool,

    // ─── struct shape ─────────────────────────────────────────────────
    pub fields: Vec<Field>,
    /// Optional trailing `-> Type` line (e.g. command return types).
    pub return_type: Option<TypeExpr>,
    /// Named sub-blocks declared by the layer (`root: struct`, `state: enum`).
    pub blocks: Vec<NamedBlock>,
    /// Nested function definitions (business logic methods).
    pub fns: Vec<FnDef>,

    // ─── enum shape ───────────────────────────────────────────────────
    pub variants: Vec<String>,
    pub transitions: Vec<StateTransition>,

    // ─── trait shape ──────────────────────────────────────────────────
    pub methods: Vec<Method>,

    // ─── impl shape ───────────────────────────────────────────────────
    /// Target trait-shaped construct (`kw Name for Target`).
    pub target: Option<String>,
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
            span,
            annotations: Vec::new(),
            exported: false,
            fields: Vec::new(),
            return_type: None,
            blocks: Vec::new(),
            fns: Vec::new(),
            variants: Vec::new(),
            transitions: Vec::new(),
            methods: Vec::new(),
            target: None,
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
    pub name: String,
    pub type_expr: TypeExpr,
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
    /// Variable assignment: name = expr
    Assign(String, Box<Expr>),
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
    pub target: String,
    pub method: String,
    pub args: Vec<Expr>,
    pub span: Span,
}
