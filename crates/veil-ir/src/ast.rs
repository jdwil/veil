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
    Saga(Saga),
}

/// A saga — cross-context orchestration with compensation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saga {
    pub name: String,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    pub context_refs: Vec<String>,
    pub inputs: Vec<Field>,
    pub steps: Vec<SagaStep>,
}

/// A step within a saga, associated with a specific context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SagaStep {
    pub name: String,
    pub context: Option<String>,
    pub span: Span,
    pub body: Vec<Expr>,
    pub compensate: Vec<Expr>,
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
    Construct(Construct),
    Group(Group),
}

/// A generic construct — unified representation for all layer-defined constructs.
/// The `keyword` field identifies what layer construct this is (e.g., "val", "agg", "port").
/// Fields are populated based on the construct's category (struct/trait/impl/fn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Construct {
    /// The keyword used in source (e.g., "val", "agg", "port", "svc")
    pub keyword: String,
    pub name: String,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    /// Fields — for struct-like constructs (val, ent, agg, evt, cmd)
    pub fields: Vec<Field>,
    /// Methods — for trait-like constructs (port, repo)
    pub methods: Vec<PortMethod>,
    /// Inputs — for fn-like constructs (svc, saga)
    pub inputs: Vec<Field>,
    /// Steps — for fn-like constructs (svc, saga)
    pub steps: Vec<FlowStep>,
    /// Return expression — for fn-like constructs
    pub return_expr: Option<Box<Expr>>,
    /// Target port — for impl-like constructs (adapter)
    pub target: Option<String>,
    /// Method implementations — for impl-like constructs (adapter)
    pub impls: Vec<AdapterImpl>,
    /// Sub-constructs — for composites (e.g., aggregate's events/commands)
    pub sub_constructs: Vec<Construct>,
    /// State machines — for aggregates
    pub state_machines: Vec<StateMachine>,
    /// Business logic methods — for aggregates
    pub aggregate_fns: Vec<AggregateFn>,
    /// Context refs — for sagas
    pub context_refs: Vec<String>,
    /// Return type — for commands
    pub return_type: Option<TypeExpr>,
}

/// A visual group — purely organizational, no codegen impact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub span: Span,
    pub items: Vec<ContextItem>,
}

impl Construct {
    /// Create an empty construct with just keyword, name, and span.
    pub fn new(keyword: &str, name: String, span: Span) -> Self {
        Construct {
            keyword: keyword.to_string(),
            name,
            span,
            annotations: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            inputs: Vec::new(),
            steps: Vec::new(),
            return_expr: None,
            target: None,
            impls: Vec::new(),
            sub_constructs: Vec::new(),
            state_machines: Vec::new(),
            aggregate_fns: Vec::new(),
            context_refs: Vec::new(),
            return_type: None,
        }
    }

    pub fn from_value_object(vo: ValueObject) -> Self {
        let mut c = Self::new("val", vo.name, vo.span);
        c.annotations = vo.annotations;
        c.fields = vo.fields;
        c
    }

    pub fn from_entity(ent: Entity) -> Self {
        let mut c = Self::new("ent", ent.name, ent.span);
        c.annotations = ent.annotations;
        c.fields = ent.fields;
        c
    }

    pub fn from_aggregate(agg: Aggregate) -> Self {
        let mut c = Self::new("agg", agg.name, agg.span);
        c.annotations = agg.annotations;
        c.fields = agg.fields;
        c.state_machines = agg.state_machines;
        c.aggregate_fns = agg.methods;
        // Convert events and commands to sub-constructs
        for evt in agg.events {
            let mut ec = Self::new("evt", evt.name, evt.span);
            ec.fields = evt.fields;
            c.sub_constructs.push(ec);
        }
        for cmd in agg.commands {
            let mut cc = Self::new("cmd", cmd.name, cmd.span);
            cc.fields = cmd.fields;
            cc.return_type = cmd.return_type;
            c.sub_constructs.push(cc);
        }
        c
    }

    pub fn from_port(port: Port) -> Self {
        let mut c = Self::new("port", port.name, port.span);
        c.methods = port.methods;
        c
    }

    pub fn from_service(svc: Service, keyword: &str) -> Self {
        let mut c = Self::new(keyword, svc.name, svc.span);
        c.annotations = svc.annotations;
        c.inputs = svc.inputs;
        c.steps = svc.steps;
        c.return_expr = svc.return_expr.map(Box::new);
        c
    }

    pub fn from_adapter(adp: Adapter) -> Self {
        let mut c = Self::new("adapter", adp.name, adp.span);
        c.annotations = adp.annotations;
        c.target = Some(adp.target_port);
        c.impls = adp.impls;
        c
    }
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
    pub state_machines: Vec<StateMachine>,
    pub methods: Vec<AggregateFn>,
}

/// A state machine definition within an aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMachine {
    pub name: String,
    pub span: Span,
    pub transitions: Vec<StateTransition>,
}

/// A state transition: From -> To
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransition {
    pub from: String,
    pub to: String,
    pub span: Span,
}

/// A method/function on an aggregate (business logic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateFn {
    pub name: String,
    pub span: Span,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub annotations: Vec<Annotation>,
    pub body: Vec<Expr>,
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

/// A service (domain service — orchestrates within a context, flow-like).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub span: Span,
    pub annotations: Vec<Annotation>,
    pub inputs: Vec<Field>,
    pub steps: Vec<FlowStep>,
    pub return_expr: Option<Expr>,
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

/// An expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    // ─── Core expressions ─────────────────────────────────────────────
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

    // ─── Kit-level expressions (backward compat) ──────────────────────
    /// Event dispatch (DDD layer)
    Emit(EmitExpr),
    /// Event dispatch via bus (DDD layer)
    Dispatch(DispatchExpr),
    /// Command invocation via bus (DDD layer)
    Invoke(InvokeExpr),
    /// Port request (DDD layer)
    Request(RequestExpr),
    /// Precondition guard (DDD layer)
    Guard(GuardExpr),
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

/// Dispatch — fire an event through the event bus (fire-and-forget).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchExpr {
    pub event_name: String,
    pub fields: Vec<(String, Expr)>,
    pub span: Span,
}

/// Invoke — execute a command through the command bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeExpr {
    pub target: String,
    pub command: String,
    pub params: Vec<(String, Expr)>,
    pub span: Span,
}

/// Request — query through a port (adapter resolves).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestExpr {
    pub port: String,
    pub method: String,
    pub args: Vec<Expr>,
    pub span: Span,
}

/// Guard — precondition check (fails the step if false).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardExpr {
    pub condition: Box<Expr>,
    pub message: Option<String>,
    pub span: Span,
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
