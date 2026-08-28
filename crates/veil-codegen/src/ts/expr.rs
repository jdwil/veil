//! TypeScript expression IR — typed intermediate representation.
//!
//! `TsExpr` sits between VEIL AST lowering and final TypeScript text emission.
//! It carries type information for emit decisions (Promise vs sync, null handling)
//! without any ownership semantics (unlike the Rust IR).
//!
//! ## Design Principles
//!
//! 1. Mirror RustExpr variant categories (literals, binops, calls, control flow, bindings)
//! 2. Carry type information for emit decisions
//! 3. No ownership nodes — add TS-specific nodes (optional chaining, nullish coalesce)
//! 4. Composable with transforms (dead code elimination, import tracking, async detection)

// ─── TsType ──────────────────────────────────────────────────────────────────

/// TypeScript type annotation — used for optional type annotations on nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsType {
    String,
    Number,
    Boolean,
    Null,
    Void,
    Array(Box<TsType>),
    Promise(Box<TsType>),
    Union(Vec<TsType>),
    Named(std::string::String),
    Record(Box<TsType>, Box<TsType>),
    Fn {
        params: Vec<TsType>,
        ret: Box<TsType>,
    },
}

impl TsType {
    /// Render to TypeScript type syntax.
    pub fn to_ts(&self) -> String {
        match self {
            TsType::String => "string".to_string(),
            TsType::Number => "number".to_string(),
            TsType::Boolean => "boolean".to_string(),
            TsType::Null => "null".to_string(),
            TsType::Void => "void".to_string(),
            TsType::Array(inner) => format!("{}[]", inner.to_ts()),
            TsType::Promise(inner) => format!("Promise<{}>", inner.to_ts()),
            TsType::Union(types) => {
                let parts: Vec<String> = types.iter().map(|t| t.to_ts()).collect();
                parts.join(" | ")
            }
            TsType::Named(name) => name.clone(),
            TsType::Record(key, val) => {
                format!("Record<{}, {}>", key.to_ts(), val.to_ts())
            }
            TsType::Fn { params, ret } => {
                let param_strs: Vec<String> = params
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("arg{}: {}", i, t.to_ts()))
                    .collect();
                format!("({}) => {}", param_strs.join(", "), ret.to_ts())
            }
        }
    }
}

// ─── TsBinOp ─────────────────────────────────────────────────────────────────

/// Binary operators for TypeScript expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,        // ===
    NotEq,     // !==
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,       // &&
    Or,        // ||
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Instanceof,
    In,
}

impl TsBinOp {
    /// Render to TypeScript operator string.
    pub fn as_str(&self) -> &'static str {
        match self {
            TsBinOp::Add => "+",
            TsBinOp::Sub => "-",
            TsBinOp::Mul => "*",
            TsBinOp::Div => "/",
            TsBinOp::Mod => "%",
            TsBinOp::Eq => "===",
            TsBinOp::NotEq => "!==",
            TsBinOp::Lt => "<",
            TsBinOp::Gt => ">",
            TsBinOp::LtEq => "<=",
            TsBinOp::GtEq => ">=",
            TsBinOp::And => "&&",
            TsBinOp::Or => "||",
            TsBinOp::BitAnd => "&",
            TsBinOp::BitOr => "|",
            TsBinOp::BitXor => "^",
            TsBinOp::Shl => "<<",
            TsBinOp::Shr => ">>",
            TsBinOp::Instanceof => "instanceof",
            TsBinOp::In => "in",
        }
    }
}

// ─── TsUnaryOp ──────────────────────────────────────────────────────────────

/// Unary operators for TypeScript expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsUnaryOp {
    Not,       // !
    Neg,       // -
    Typeof,    // typeof
    Void,      // void
    Delete,    // delete
}

impl TsUnaryOp {
    /// Render to TypeScript operator string.
    pub fn as_str(&self) -> &'static str {
        match self {
            TsUnaryOp::Not => "!",
            TsUnaryOp::Neg => "-",
            TsUnaryOp::Typeof => "typeof ",
            TsUnaryOp::Void => "void ",
            TsUnaryOp::Delete => "delete ",
        }
    }
}

// ─── TsPattern ───────────────────────────────────────────────────────────────

/// Destructuring patterns for `const { a, b } = expr` or `const [x, y] = expr`.
#[derive(Debug, Clone, PartialEq)]
pub enum TsPattern {
    /// `{ field1, field2, ... }`
    Object { fields: Vec<String> },
    /// `[item1, item2, ...]`
    Array { items: Vec<String> },
}

// ─── TsTemplatePart ──────────────────────────────────────────────────────────

/// Part of a template literal string.
#[derive(Debug, Clone, PartialEq)]
pub enum TsTemplatePart {
    /// Raw string content between `${...}` interpolations.
    Literal(String),
    /// Expression interpolation: `${expr}`.
    Expr(TsExpr),
}

// ─── TsExpr ─────────────────────────────────────────────────────────────────

/// Typed intermediate representation of a TypeScript expression / statement.
///
/// Carries enough information for transforms (import tracking, async detection,
/// null-safety insertion) without re-parsing rendered strings.
#[derive(Debug, Clone, PartialEq)]
pub enum TsExpr {
    // ── Literals ──────────────────────────────────────────────────────────

    /// Identifier reference: `myVar`, `console`
    Ident { name: String, ty: Option<TsType> },

    /// String literal: `"hello"`
    StringLit(String),

    /// Template literal: `` `Hello ${name}` ``
    TemplateLit { parts: Vec<TsTemplatePart> },

    /// Integer literal: `42`
    IntLit(i64),

    /// Float literal: `3.14`
    FloatLit(f64),

    /// Boolean literal: `true` / `false`
    BoolLit(bool),

    /// `null`
    NullLit,

    /// `undefined`
    UndefinedLit,

    /// Array literal: `[a, b, c]`
    ArrayLit { items: Vec<TsExpr>, ty: Option<TsType> },

    /// Object literal: `{ key: value, ... }`
    ObjectLit { fields: Vec<(String, TsExpr)>, ty: Option<TsType> },

    // ── Operators ─────────────────────────────────────────────────────────

    /// Binary operation: `left op right`
    BinOp {
        left: Box<TsExpr>,
        op: TsBinOp,
        right: Box<TsExpr>,
        ty: Option<TsType>,
    },

    /// Unary operation: `op expr`
    UnaryOp { op: TsUnaryOp, expr: Box<TsExpr> },

    /// Optional chaining: `base?.field`
    OptionalChain { base: Box<TsExpr>, field: String },

    /// Nullish coalescing: `left ?? right`
    NullishCoalesce { left: Box<TsExpr>, right: Box<TsExpr> },

    // ── Access ────────────────────────────────────────────────────────────

    /// Field access: `base.field`
    FieldAccess {
        base: Box<TsExpr>,
        field: String,
        ty: Option<TsType>,
    },

    /// Index access: `base[index]`
    Index { base: Box<TsExpr>, index: Box<TsExpr> },

    // ── Calls ─────────────────────────────────────────────────────────────

    /// Method call: `receiver.method(args)` or `await receiver.method(args)`
    MethodCall {
        receiver: Box<TsExpr>,
        method: String,
        args: Vec<TsExpr>,
        ty: Option<TsType>,
        is_async: bool,
    },

    /// Free function call: `name(args)`
    FnCall {
        name: String,
        args: Vec<TsExpr>,
        ty: Option<TsType>,
    },

    /// Constructor call: `new Class(args)`
    NewCall {
        class: String,
        args: Vec<TsExpr>,
        ty: Option<TsType>,
    },

    // ── Bindings ──────────────────────────────────────────────────────────

    /// `const name[: Type] = value;`
    Const {
        name: String,
        ty: Option<String>,
        value: Box<TsExpr>,
    },

    /// `let name[: Type] = value;`
    Let {
        name: String,
        ty: Option<String>,
        value: Box<TsExpr>,
    },

    /// `const { fields } = value;` or `const [items] = value;`
    Destructure {
        pattern: TsPattern,
        value: Box<TsExpr>,
    },

    /// Assignment: `target = value;`
    Assign {
        target: Box<TsExpr>,
        value: Box<TsExpr>,
    },

    // ── Control Flow ──────────────────────────────────────────────────────

    /// `if (cond) { ... } else { ... }`
    If {
        condition: Box<TsExpr>,
        then_body: Vec<TsExpr>,
        else_body: Option<Vec<TsExpr>>,
    },

    /// Conditional (ternary) expression: `cond ? then : else`.
    /// Used for value-context `if/else` (e.g. a `derived` field RHS).
    Ternary {
        condition: Box<TsExpr>,
        then_expr: Box<TsExpr>,
        else_expr: Box<TsExpr>,
    },

    /// `switch (scrutinee) { case "X": ...; break; default: ... }`
    Switch {
        scrutinee: Box<TsExpr>,
        cases: Vec<(String, Vec<TsExpr>)>,
        default: Option<Vec<TsExpr>>,
    },

    /// `for (const binding of iterable) { body }`
    For {
        binding: String,
        iterable: Box<TsExpr>,
        body: Vec<TsExpr>,
    },

    /// `for (let index = 0; index < iterable.length; index++) { const binding = iterable[index]; body }`
    /// or `iterable.forEach((binding, index) => { body })`
    ForIndex {
        index: String,
        binding: String,
        iterable: Box<TsExpr>,
        body: Vec<TsExpr>,
    },

    /// `while (condition) { body }`
    While {
        condition: Box<TsExpr>,
        body: Vec<TsExpr>,
    },

    /// `while (true) { body }` — infinite loop
    Loop { body: Vec<TsExpr> },

    // ── Functions ─────────────────────────────────────────────────────────

    /// Arrow function: `(params) => { body }` or `async (params) => { body }`
    ArrowFn {
        params: Vec<String>,
        body: Vec<TsExpr>,
        is_async: bool,
    },

    /// `return expr;`
    Return(Box<TsExpr>),

    /// `await expr`
    Await(Box<TsExpr>),

    /// `throw new Error(message)` or `throw expr`
    Throw { message: Box<TsExpr> },

    // ── TS-Specific ───────────────────────────────────────────────────────

    /// Type assertion: `expr as Type`
    TypeAssertion { expr: Box<TsExpr>, ty: String },

    /// Non-null assertion: `expr!`
    NonNullAssertion(Box<TsExpr>),

    /// Spread: `...expr`
    Spread(Box<TsExpr>),

    // ── Statements ────────────────────────────────────────────────────────

    /// `break;`
    Break,

    /// `continue;`
    Continue,

    // ── Layer-provided / terminal ─────────────────────────────────────────

    /// No-op: emits nothing (e.g., `noop` in VEIL).
    Noop,

    /// Layer template interpolation result — opaque content from lowers_to templates.
    LayerEmit(String),
}
