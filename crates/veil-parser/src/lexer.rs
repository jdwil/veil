//! VEIL Lexer — tokenizes source into a stream with INDENT/DEDENT tokens.
//!
//! Handles indentation-sensitive structure (Python-style), producing synthetic
//! INDENT and DEDENT tokens. Supports all VEIL keywords, operators, annotations,
//! string literals, numbers, and comments.

use veil_ir::span::Span;

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
}

/// Token kinds for the VEIL language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // ─── Structure ────────────────────────────────────────────────────
    Indent,
    Dedent,
    Newline,

    // ─── CORE: Type system ────────────────────────────────────────────
    Struct,
    Enum,
    Fn,
    Trait,
    Let,
    Mod,

    // ─── CORE: Control flow ───────────────────────────────────────────
    If,
    Else,
    Match,
    Ret,

    // ─── CORE: Literals ───────────────────────────────────────────────
    StringLit,
    IntLit,
    FloatLit,
    True,
    False,

    // ─── CORE: Operators ──────────────────────────────────────────────
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Percent,    // %
    EqEq,      // ==
    NotEq,      // !=
    LAngle,     // < (also used for generics)
    RAngle,     // > (also used for generics)
    LtEq,      // <=
    GtEq,      // >=
    And,        // &&
    Or,         // ||
    Pipe,       // |
    Bang,       // !
    Eq,         // =
    Arrow,      // ->
    FatArrow,   // =>
    Dot,        // .
    Colon,      // :
    Comma,      // ,
    LParen,
    RParen,
    LBrace,     // {
    RBrace,     // }

    // ─── CORE: Other ──────────────────────────────────────────────────
    Annotation, // @something(args)
    Comment,    // # ...
    Ident,      // identifiers
    Eof,

    // ─── CORE: File & flow structure ──────────────────────────────────
    // These are language-level, not domain-level. All domain vocabulary
    // (ctx, agg, port, saga, dispatch, ...) lexes as Ident and is given
    // meaning by the layer registry at parse time.
    Sol,
    Pkg,
    Use,
    Lang,
    Expose,
    Node,
    Flow,
    Step,
    Par,
    Alt,
    Loop,
    Err,
    Call,
    Input,
    Fallback,
    For,
    While,
    Mut,
    TypeKw,
    ConstKw,
    Await,
    Boundary,
    As,
    Desc,
    Output,
    Constraints,
    Group,
    Allow,
    Deny,
    Export,
    Impl,
}

/// Lex VEIL source code into a token stream.
pub fn lex(source: &str) -> Vec<Token> {
    Lexer::new(source).tokenize()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    indent_stack: Vec<usize>,
    tokens: Vec<Token>,
    at_line_start: bool,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            indent_stack: vec![0],
            tokens: Vec::new(),
            at_line_start: true,
        }
    }

    fn tokenize(mut self) -> Vec<Token> {
        while self.pos < self.chars.len() {
            if self.at_line_start {
                self.handle_indentation();
                self.at_line_start = false;
            }
            self.skip_inline_spaces();
            if self.pos >= self.chars.len() {
                break;
            }
            match self.current() {
                '\n' => {
                    self.emit(TokenKind::Newline, self.pos, self.pos + 1);
                    self.pos += 1;
                    self.at_line_start = true;
                }
                '#' => self.lex_comment(),
                '@' => self.lex_annotation(),
                '"' => self.lex_string(),
                '-' if self.peek() == Some('>') => {
                    self.emit(TokenKind::Arrow, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '=' if self.peek() == Some('>') => {
                    self.emit(TokenKind::FatArrow, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '=' if self.peek() == Some('=') => {
                    self.emit(TokenKind::EqEq, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '|' if self.peek() == Some('|') => {
                    self.emit(TokenKind::Or, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '|' => {
                    self.emit(TokenKind::Pipe, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '&' if self.peek() == Some('&') => {
                    self.emit(TokenKind::And, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '!' if self.peek() == Some('=') => {
                    self.emit(TokenKind::NotEq, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '<' if self.peek() == Some('=') => {
                    self.emit(TokenKind::LtEq, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '>' if self.peek() == Some('=') => {
                    self.emit(TokenKind::GtEq, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '=' => {
                    self.emit(TokenKind::Eq, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '+' => {
                    self.emit(TokenKind::Plus, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '-' => {
                    self.emit(TokenKind::Minus, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '*' => {
                    self.emit(TokenKind::Star, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '/' => {
                    self.emit(TokenKind::Slash, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '%' => {
                    self.emit(TokenKind::Percent, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                ':' => {
                    self.emit(TokenKind::Colon, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '.' => {
                    self.emit(TokenKind::Dot, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                ',' => {
                    self.emit(TokenKind::Comma, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '!' => {
                    self.emit(TokenKind::Bang, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '(' => {
                    self.emit(TokenKind::LParen, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                ')' => {
                    self.emit(TokenKind::RParen, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '<' => {
                    self.emit(TokenKind::LAngle, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '>' => {
                    self.emit(TokenKind::RAngle, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '{' => {
                    self.emit(TokenKind::LBrace, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                '}' => {
                    self.emit(TokenKind::RBrace, self.pos, self.pos + 1);
                    self.pos += 1;
                }
                c if c.is_ascii_digit() => self.lex_number(),
                c if is_ident_start(c) => self.lex_ident_or_keyword(),
                _ => {
                    // Skip unknown characters
                    self.pos += 1;
                }
            }
        }
        // Emit remaining dedents at EOF
        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            self.emit(TokenKind::Dedent, self.pos, self.pos);
        }
        self.emit(TokenKind::Eof, self.pos, self.pos);
        self.tokens
    }

    fn current(&self) -> char {
        self.chars[self.pos]
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn skip_inline_spaces(&mut self) {
        while self.pos < self.chars.len()
            && self.chars[self.pos] == ' '
        {
            self.pos += 1;
        }
    }

    fn handle_indentation(&mut self) {
        let start = self.pos;
        let mut indent = 0;
        while self.pos < self.chars.len() && self.chars[self.pos] == ' ' {
            indent += 1;
            self.pos += 1;
        }
        // Skip blank lines (don't change indentation state)
        if self.pos >= self.chars.len() || self.chars[self.pos] == '\n' {
            return;
        }
        // Skip comment-only lines (don't change indentation state)
        if self.chars[self.pos] == '#' {
            return;
        }

        let current_indent = *self.indent_stack.last().unwrap();
        if indent > current_indent {
            self.indent_stack.push(indent);
            self.emit(TokenKind::Indent, start, self.pos);
        } else {
            while indent < *self.indent_stack.last().unwrap() {
                self.indent_stack.pop();
                self.emit(TokenKind::Dedent, start, self.pos);
            }
        }
    }

    fn lex_comment(&mut self) {
        let start = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos] != '\n' {
            self.pos += 1;
        }
        self.emit(TokenKind::Comment, start, self.pos);
    }

    fn lex_annotation(&mut self) {
        let start = self.pos;
        self.pos += 1; // skip @
        // Read annotation name
        while self.pos < self.chars.len()
            && (self.chars[self.pos].is_alphanumeric() || self.chars[self.pos] == '_')
        {
            self.pos += 1;
        }
        // Include parenthesized args like @retry(3) or @trace(method="xray")
        if self.pos < self.chars.len() && self.chars[self.pos] == '(' {
            self.pos += 1;
            let mut depth = 1;
            while self.pos < self.chars.len() && depth > 0 {
                match self.chars[self.pos] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                self.pos += 1;
            }
        }
        self.emit(TokenKind::Annotation, start, self.pos);
    }

    fn lex_string(&mut self) {
        let start = self.pos;
        self.pos += 1; // skip opening "
        while self.pos < self.chars.len() && self.chars[self.pos] != '"' {
            if self.chars[self.pos] == '\\' {
                self.pos += 1; // skip escape char
            }
            self.pos += 1;
        }
        if self.pos < self.chars.len() {
            self.pos += 1; // skip closing "
        }
        self.emit(TokenKind::StringLit, start, self.pos);
    }

    fn lex_number(&mut self) {
        let start = self.pos;
        let mut is_float = false;
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos < self.chars.len() && self.chars[self.pos] == '.'
            && self.peek().map_or(false, |c| c.is_ascii_digit())
        {
            is_float = true;
            self.pos += 1;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let kind = if is_float {
            TokenKind::FloatLit
        } else {
            TokenKind::IntLit
        };
        self.emit(kind, start, self.pos);
    }

    fn lex_ident_or_keyword(&mut self) {
        let start = self.pos;
        while self.pos < self.chars.len() && is_ident_continue(self.chars[self.pos]) {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        let kind = keyword_lookup(&text);
        self.emit(kind, start, self.pos);
    }

    fn emit(&mut self, kind: TokenKind, start: usize, end: usize) {
        let text: String = if start < self.chars.len() && end <= self.chars.len() {
            self.chars[start..end].iter().collect()
        } else {
            String::new()
        };
        self.tokens.push(Token {
            kind,
            span: Span::new(start, end),
            text,
        });
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn keyword_lookup(text: &str) -> TokenKind {
    match text {
        // Core language primitives
        "struct" => TokenKind::Struct,
        "enum" => TokenKind::Enum,
        "fn" => TokenKind::Fn,
        "trait" => TokenKind::Trait,
        "let" => TokenKind::Let,
        "mod" => TokenKind::Mod,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "match" => TokenKind::Match,
        "ret" => TokenKind::Ret,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "impl" => TokenKind::Impl,
        // Core file & flow structure
        "sol" => TokenKind::Sol,
        "pkg" => TokenKind::Pkg,
        "use" => TokenKind::Use,
        "lang" => TokenKind::Lang,
        "expose" => TokenKind::Expose,
        "node" => TokenKind::Node,
        "flow" => TokenKind::Flow,
        "step" => TokenKind::Step,
        "par" => TokenKind::Par,
        "alt" => TokenKind::Alt,
        "loop" => TokenKind::Loop,
        "err" => TokenKind::Err,
        "call" => TokenKind::Call,
        "input" => TokenKind::Input,
        "fallback" => TokenKind::Fallback,
        "for" => TokenKind::For,
        "while" => TokenKind::While,
        "mut" => TokenKind::Mut,
        "type" => TokenKind::TypeKw,
        "const" => TokenKind::ConstKw,
        "await" => TokenKind::Await,
        "boundary" => TokenKind::Boundary,
        "as" => TokenKind::As,
        "desc" => TokenKind::Desc,
        "output" => TokenKind::Output,
        "constraints" => TokenKind::Constraints,
        "group" => TokenKind::Group,
        "allow" => TokenKind::Allow,
        "deny" => TokenKind::Deny,
        "export" => TokenKind::Export,
        // Everything else — including all layer vocabulary — is an Ident.
        _ => TokenKind::Ident,
    }
}

#[cfg(test)]
#[path = "lexer_tests.rs"]
mod tests;
