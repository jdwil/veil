//! VEIL Abstract Syntax Tree definitions.

use serde::{Deserialize, Serialize};

use crate::span::Span;

/// Root of a VEIL file — either a solution or a package.
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

/// Root of a VEIL file — a solution (legacy, still supported).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Solution {
    pub name: String,
    pub span: Span,
    pub items: Vec<TopLevelItem>,
}

/// Top-level items within a solution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TopLevelItem {
    Lang(LangBlock),
    Context(Context),
    Flow(Flow),
    Adapter(Adapter),
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

/// A bounded context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub name: String,
    pub span: Span,
    pub items: Vec<ContextItem>,
}

/// Items within a bounded context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextItem {
    ValueObject(ValueObject),
    Entity(Entity),
    Aggregate(Aggregate),
    Port(Port),
    Service(Service),
}

/// A value object (no identity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueObject {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub annotations: Vec<Annotation>,
}

/// An entity (has identity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub annotations: Vec<Annotation>,
}

/// An aggregate root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Aggregate {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub annotations: Vec<Annotation>,
    pub events: Vec<Event>,
    pub commands: Vec<Command>,
}

/// A domain event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
}

/// A command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub return_type: Option<TypeExpr>,
}

/// A port (interface/trait).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub span: Span,
    pub methods: Vec<PortMethod>,
}

/// A method on a port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMethod {
    pub name: String,
    pub span: Span,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
}

/// A service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub span: Span,
    pub methods: Vec<PortMethod>,
}

/// An adapter (implementation of a port).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adapter {
    pub name: String,
    pub target_port: String,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    pub impls: Vec<AdapterImpl>,
}

/// An adapter method implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterImpl {
    pub method_name: String,
    pub params: Vec<String>,
    pub span: Span,
    pub body: Vec<Expr>,
}

/// A behavioral flow (use case orchestration).
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

/// An expression (simplified for MVP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    Ident(String),
    FieldAccess(Box<Expr>, String),
    Call(CallExpr),
    Emit(EmitExpr),
    Assign(String, Box<Expr>),
    StringLit(String),
    IntLit(i64),
    Return(Box<Expr>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallExpr {
    pub target: String,
    pub method: String,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitExpr {
    pub event_name: String,
    pub fields: Vec<(String, Expr)>,
    pub span: Span,
}
