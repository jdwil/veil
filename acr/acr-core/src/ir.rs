//! The Algorithm IR AST - a restricted, safely executable expression language.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A complete algorithm definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Algorithm {
    pub id: Uuid,
    pub name: String,
    pub version: u32,
    pub description: String,
    pub params: Vec<Param>,
    pub body: Vec<Statement>,
    pub created_at: DateTime<Utc>,
    pub metadata: AlgorithmMetadata,
}

impl Algorithm {
    /// Create a new algorithm with a generated UUID and current timestamp.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        domain: impl Into<String>,
        params: Vec<Param>,
        body: Vec<Statement>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            version: 1,
            description: description.into(),
            params,
            body,
            created_at: Utc::now(),
            metadata: AlgorithmMetadata {
                domain: domain.into(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                provenance: Provenance::Manual,
            },
        }
    }
}

/// Metadata associated with an algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlgorithmMetadata {
    pub domain: String,
    pub tags: Vec<String>,
    /// Other algorithms this one calls.
    pub dependencies: Vec<Uuid>,
    pub provenance: Provenance,
}

/// Tracks how an algorithm was created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Provenance {
    Generated { by: String, prompt: Option<String> },
    Mutated { from_id: Uuid, from_version: u32 },
    Composed { sources: Vec<Uuid> },
    Manual,
}

/// A parameter declaration for an algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub type_hint: TypeHint,
}

/// Type hints for parameters and values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TypeHint {
    Any,
    Bool,
    Int,
    Float,
    Str,
    List(Box<TypeHint>),
    Map,
}

/// A statement in the algorithm body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Statement {
    Let { name: String, value: Expr },
    Assign { name: String, value: Expr },
    If {
        condition: Expr,
        then_body: Vec<Statement>,
        else_body: Vec<Statement>,
    },
    While { condition: Expr, body: Vec<Statement> },
    For {
        var: String,
        iter: Expr,
        body: Vec<Statement>,
    },
    Return(Expr),
    Expr(Expr),
    Assert { condition: Expr, message: String },
}

/// An expression in the algorithm IR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    Literal(LiteralValue),
    Var(String),
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Call { function: String, args: Vec<Expr> },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    FieldAccess {
        target: Box<Expr>,
        field: String,
    },
    ListLiteral(Vec<Expr>),
    MapLiteral(Vec<(String, Expr)>),
    Lambda {
        params: Vec<String>,
        body: Vec<Statement>,
    },
}

/// A literal value in the IR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LiteralValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}
