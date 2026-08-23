//! Predicate evaluator for layer pass rules.
//!
//! Evaluates simple predicate expressions against a `NodeContext` that provides
//! data about the current AST node being inspected. Not Turing-complete —
//! supports only comparisons, boolean logic, and path access.

use std::collections::HashMap;

/// Context for evaluating a predicate against a single AST node.
/// Populated by the pass executor as it walks the AST.
#[derive(Debug, Clone, Default)]
pub struct NodeContext {
    /// Properties accessible via dotted paths (e.g. "expr.kind" → "ident").
    /// String-valued properties.
    pub strings: HashMap<String, String>,
    /// Numeric-valued properties (e.g. "expr.use_count" → 3).
    pub numbers: HashMap<String, i64>,
    /// Boolean-valued properties (e.g. "expr.in_loop" → true).
    pub booleans: HashMap<String, bool>,
}

impl NodeContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_str(&mut self, key: &str, value: &str) -> &mut Self {
        self.strings.insert(key.to_string(), value.to_string());
        self
    }

    pub fn set_num(&mut self, key: &str, value: i64) -> &mut Self {
        self.numbers.insert(key.to_string(), value);
        self
    }

    pub fn set_bool(&mut self, key: &str, value: bool) -> &mut Self {
        self.booleans.insert(key.to_string(), value);
        self
    }
}

/// Evaluate a predicate expression against a node context.
/// Returns `true` if the predicate matches, `false` otherwise.
/// On parse errors, returns `false` (fail-closed).
pub fn evaluate_predicate(pred: &str, ctx: &NodeContext) -> bool {
    let pred = pred.trim();
    if pred.is_empty() {
        return false;
    }
    let tokens = tokenize(pred);
    if tokens.is_empty() {
        return false;
    }
    match eval_tokens(&tokens, ctx) {
        Value::Bool(b) => b,
        Value::Num(n) => n != 0,
        Value::Str(s) => !s.is_empty(),
        Value::Undefined => false,
    }
}

// ─── Token / Value types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),      // path like expr.kind or expr.type.is_copy
    Str(String),        // "literal"
    Num(i64),           // 42
    Eq,                 // ==
    Neq,                // !=
    Lt,                 // <
    Gt,                 // >
    Lte,                // <=
    Gte,                // >=
    And,                // &&
    Or,                 // ||
    Not,                // !
    LParen,             // (
    RParen,             // )
    True,               // true
    False,              // false
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Bool(bool),
    Str(String),
    Num(i64),
    Undefined,
}

// ─── Tokenizer ───────────────────────────────────────────────────────────────

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        // Skip whitespace
        if ch.is_whitespace() {
            i += 1;
            continue;
        }

        // Two-char operators
        if i + 1 < chars.len() {
            let two = format!("{}{}", ch, chars[i + 1]);
            match two.as_str() {
                "==" => { tokens.push(Token::Eq); i += 2; continue; }
                "!=" => { tokens.push(Token::Neq); i += 2; continue; }
                "&&" => { tokens.push(Token::And); i += 2; continue; }
                "||" => { tokens.push(Token::Or); i += 2; continue; }
                "<=" => { tokens.push(Token::Lte); i += 2; continue; }
                ">=" => { tokens.push(Token::Gte); i += 2; continue; }
                _ => {}
            }
        }

        // Single-char tokens
        match ch {
            '!' => { tokens.push(Token::Not); i += 1; continue; }
            '(' => { tokens.push(Token::LParen); i += 1; continue; }
            ')' => { tokens.push(Token::RParen); i += 1; continue; }
            '<' => { tokens.push(Token::Lt); i += 1; continue; }
            '>' => { tokens.push(Token::Gt); i += 1; continue; }
            _ => {}
        }

        // String literal
        if ch == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    s.push(chars[i]);
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            i += 1; // skip closing "
            tokens.push(Token::Str(s));
            continue;
        }

        // Number
        if ch.is_ascii_digit() || (ch == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let start = i;
            if ch == '-' { i += 1; }
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            tokens.push(Token::Num(num_str.parse().unwrap_or(0)));
            continue;
        }

        // Identifier / path (dotted, may include parens for function calls)
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                i += 1;
            }
            // Handle function-call syntax like has_annotation("X") or has_role("X")
            // Consume (args) into the ident token for built-in functions
            let ident: String = chars[start..i].iter().collect();
            if i < chars.len() && chars[i] == '(' {
                // Include the argument in the ident for built-in functions
                let mut depth = 0;
                while i < chars.len() {
                    if chars[i] == '(' { depth += 1; }
                    if chars[i] == ')' { depth -= 1; if depth == 0 { i += 1; break; } }
                    i += 1;
                }
                let full: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(full));
            } else {
                match ident.as_str() {
                    "true" => tokens.push(Token::True),
                    "false" => tokens.push(Token::False),
                    _ => tokens.push(Token::Ident(ident)),
                }
            }
            continue;
        }

        // Unknown char — skip
        i += 1;
    }

    tokens
}

// ─── Context-aware evaluator ─────────────────────────────────────────────────

impl Value {
    fn to_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0,
            Value::Str(s) => !s.is_empty(),
            Value::Undefined => false,
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
}

fn compare_numeric(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

// ─── Actual evaluator that has NodeContext ────────────────────────────────────
// The parser above doesn't have the context. We need a different approach:
// resolve identifiers during evaluation, not during parsing.
// Rewrite: single-pass evaluator that resolves idents on-the-fly.

/// Internal: resolve an identifier path against the context.
fn resolve_ident(path: &str, ctx: &NodeContext) -> Value {
    // Handle function calls like construct.has_annotation("X") or construct.has_role("Y")
    if path.contains('(') {
        return resolve_function_call(path, ctx);
    }

    // Try as boolean first (most specific)
    if let Some(&b) = ctx.booleans.get(path) {
        return Value::Bool(b);
    }
    // Try as number
    if let Some(&n) = ctx.numbers.get(path) {
        return Value::Num(n);
    }
    // Try as string
    if let Some(s) = ctx.strings.get(path) {
        return Value::Str(s.clone());
    }
    Value::Undefined
}

/// Resolve a function-call style identifier like `construct.has_role("X")`.
fn resolve_function_call(path: &str, ctx: &NodeContext) -> Value {
    // Parse: base_path(arg)
    let paren_start = path.find('(').unwrap_or(path.len());
    let fn_path = &path[..paren_start];
    let args_str = path[paren_start..].trim_start_matches('(').trim_end_matches(')');
    let arg = args_str.trim().trim_matches('"');

    // Look up "fn_path(arg)" as a boolean key in context
    // The executor pre-populates these for known functions
    let key = format!("{}(\"{}\")", fn_path, arg);
    if let Some(&b) = ctx.booleans.get(&key) {
        return Value::Bool(b);
    }

    Value::Bool(false)
}

// ─── Context-aware evaluator (replaces the parser-based approach) ────────────

/// Evaluate a full predicate expression with context-aware identifier resolution.
/// This is the real entry point — the public `evaluate_predicate` calls this.
fn eval_tokens(tokens: &[Token], ctx: &NodeContext) -> Value {
    let mut evaluator = CtxEvaluator { tokens, pos: 0, ctx };
    evaluator.parse_or()
}

struct CtxEvaluator<'a> {
    tokens: &'a [Token],
    pos: usize,
    ctx: &'a NodeContext,
}

impl<'a> CtxEvaluator<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        self.pos += 1;
        tok
    }

    fn parse_or(&mut self) -> Value {
        let mut left = self.parse_and();
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_and();
            left = Value::Bool(left.to_bool() || right.to_bool());
        }
        left
    }

    fn parse_and(&mut self) -> Value {
        let mut left = self.parse_not();
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_not();
            left = Value::Bool(left.to_bool() && right.to_bool());
        }
        left
    }

    fn parse_not(&mut self) -> Value {
        if self.peek() == Some(&Token::Not) {
            self.advance();
            let val = self.parse_not();
            return Value::Bool(!val.to_bool());
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Value {
        let left = self.parse_primary();
        match self.peek().cloned() {
            Some(Token::Eq) => {
                self.advance();
                let right = self.parse_primary();
                Value::Bool(values_equal(&left, &right))
            }
            Some(Token::Neq) => {
                self.advance();
                let right = self.parse_primary();
                Value::Bool(!values_equal(&left, &right))
            }
            Some(Token::Lt) => {
                self.advance();
                let right = self.parse_primary();
                Value::Bool(compare_numeric(&left, &right) == Some(std::cmp::Ordering::Less))
            }
            Some(Token::Gt) => {
                self.advance();
                let right = self.parse_primary();
                Value::Bool(compare_numeric(&left, &right) == Some(std::cmp::Ordering::Greater))
            }
            Some(Token::Lte) => {
                self.advance();
                let right = self.parse_primary();
                Value::Bool(matches!(
                    compare_numeric(&left, &right),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                ))
            }
            Some(Token::Gte) => {
                self.advance();
                let right = self.parse_primary();
                Value::Bool(matches!(
                    compare_numeric(&left, &right),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                ))
            }
            _ => left,
        }
    }

    fn parse_primary(&mut self) -> Value {
        match self.peek().cloned() {
            Some(Token::True) => { self.advance(); Value::Bool(true) }
            Some(Token::False) => { self.advance(); Value::Bool(false) }
            Some(Token::Num(n)) => { self.advance(); Value::Num(n) }
            Some(Token::Str(ref s)) => { let s = s.clone(); self.advance(); Value::Str(s) }
            Some(Token::Ident(ref path)) => {
                let path = path.clone();
                self.advance();
                resolve_ident(&path, self.ctx)
            }
            Some(Token::LParen) => {
                self.advance();
                let val = self.parse_or();
                if self.peek() == Some(&Token::RParen) {
                    self.advance();
                }
                val
            }
            Some(Token::Not) => {
                self.advance();
                let val = self.parse_primary();
                Value::Bool(!val.to_bool())
            }
            _ => Value::Undefined,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_string_eq() {
        let mut ctx = NodeContext::new();
        ctx.set_str("expr.kind", "ident");
        assert!(evaluate_predicate(r#"expr.kind == "ident""#, &ctx));
        assert!(!evaluate_predicate(r#"expr.kind == "call""#, &ctx));
    }

    #[test]
    fn numeric_comparison() {
        let mut ctx = NodeContext::new();
        ctx.set_num("expr.use_count", 3);
        assert!(evaluate_predicate("expr.use_count > 1", &ctx));
        assert!(evaluate_predicate("expr.use_count == 3", &ctx));
        assert!(!evaluate_predicate("expr.use_count == 1", &ctx));
        assert!(evaluate_predicate("expr.use_count >= 3", &ctx));
        assert!(!evaluate_predicate("expr.use_count > 3", &ctx));
    }

    #[test]
    fn boolean_properties() {
        let mut ctx = NodeContext::new();
        ctx.set_bool("expr.in_loop", true);
        ctx.set_bool("expr.type.is_copy", false);
        assert!(evaluate_predicate("expr.in_loop", &ctx));
        assert!(!evaluate_predicate("expr.type.is_copy", &ctx));
        assert!(evaluate_predicate("!expr.type.is_copy", &ctx));
    }

    #[test]
    fn logical_and_or() {
        let mut ctx = NodeContext::new();
        ctx.set_str("expr.kind", "ident");
        ctx.set_num("expr.use_count", 1);
        assert!(evaluate_predicate(
            r#"expr.kind == "ident" && expr.use_count == 1"#,
            &ctx,
        ));
        assert!(!evaluate_predicate(
            r#"expr.kind == "ident" && expr.use_count > 1"#,
            &ctx,
        ));
        assert!(evaluate_predicate(
            r#"expr.kind == "call" || expr.use_count == 1"#,
            &ctx,
        ));
    }

    #[test]
    fn negation() {
        let mut ctx = NodeContext::new();
        ctx.set_bool("expr.type.is_copy", true);
        ctx.set_num("expr.use_count", 2);
        assert!(evaluate_predicate(
            r#"expr.use_count > 1 && !expr.type.is_copy"#,
            &ctx,
        ).not());
        // Now with is_copy = false
        ctx.set_bool("expr.type.is_copy", false);
        assert!(evaluate_predicate(
            r#"expr.use_count > 1 && !expr.type.is_copy"#,
            &ctx,
        ));
    }

    #[test]
    fn function_call_syntax() {
        let mut ctx = NodeContext::new();
        ctx.set_bool(r#"construct.has_annotation("dep")"#, true);
        ctx.set_bool(r#"construct.has_role("http_endpoint")"#, false);
        assert!(evaluate_predicate(r#"construct.has_annotation("dep")"#, &ctx));
        assert!(!evaluate_predicate(r#"construct.has_role("http_endpoint")"#, &ctx));
    }

    #[test]
    fn parenthesized_expressions() {
        let mut ctx = NodeContext::new();
        ctx.set_str("expr.kind", "ident");
        ctx.set_num("expr.use_count", 2);
        ctx.set_bool("expr.type.is_copy", false);
        assert!(evaluate_predicate(
            r#"(expr.kind == "ident") && (expr.use_count > 1)"#,
            &ctx,
        ));
    }

    #[test]
    fn empty_and_invalid_predicates() {
        let ctx = NodeContext::new();
        assert!(!evaluate_predicate("", &ctx));
        assert!(!evaluate_predicate("unknown_field", &ctx));
    }

    #[test]
    fn neq_operator() {
        let mut ctx = NodeContext::new();
        ctx.set_str("expr.kind", "call");
        assert!(evaluate_predicate(r#"expr.kind != "ident""#, &ctx));
        assert!(!evaluate_predicate(r#"expr.kind != "call""#, &ctx));
    }

    // Suppresses the use of `!` method by tests
    trait BoolNot {
        fn not(self) -> bool;
    }
    impl BoolNot for bool {
        fn not(self) -> bool { !self }
    }
}
