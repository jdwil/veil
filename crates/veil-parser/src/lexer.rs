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
    // Structure
    Indent,
    Dedent,
    Newline,

    // Keywords — top-level
    Sol,
    Ctx,
    Agg,
    Ent,
    Val,
    Evt,
    Cmd,
    Qry,
    Port,
    Adapter,
    Flow,
    Svc,
    Pipe,
    Lang,
    Pkg,
    Use,
    Expose,
    Node,
    Saga,

    // Keywords — flow
    Step,
    Par,
    Alt,
    Loop,
    Err,
    Match,
    Emit,
    Call,
    Ret,
    Input,
    Fallback,
    Impl,
    For,
    Boundary,
    As,
    Desc,
    Output,
    Constraints,
    Fn,
    State,
    Root,
    Compensate,
    Contexts,

    // Operators
    Arrow,      // ->
    FatArrow,   // =>
    Parallel,   // ||
    Colon,      // :
    Dot,        // .
    Comma,      // ,
    Eq,         // =
    NotEq,      // !=
    Bang,       // !
    LParen,
    RParen,
    LAngle,     // <
    RAngle,     // >
    LBrace,     // {
    RBrace,     // }

    // Literals
    StringLit,
    IntLit,
    FloatLit,

    // Annotation (@something or @something(args))
    Annotation,

    // Comment (# ...)
    Comment,

    // Identifiers
    Ident,

    // End of file
    Eof,
}

/// Lex VEIL source code into a token stream.
pub fn lex(source: &str) -> Vec<Token> {
    Lexer::new(source).tokenize()
}

struct Lexer<'a> {
    source: &'a str,
    chars: Vec<char>,
    pos: usize,
    indent_stack: Vec<usize>,
    tokens: Vec<Token>,
    at_line_start: bool,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
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
                '|' if self.peek() == Some('|') => {
                    self.emit(TokenKind::Parallel, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '!' if self.peek() == Some('=') => {
                    self.emit(TokenKind::NotEq, self.pos, self.pos + 2);
                    self.pos += 2;
                }
                '=' => {
                    self.emit(TokenKind::Eq, self.pos, self.pos + 1);
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
        "sol" => TokenKind::Sol,
        "ctx" => TokenKind::Ctx,
        "agg" => TokenKind::Agg,
        "ent" => TokenKind::Ent,
        "val" => TokenKind::Val,
        "evt" => TokenKind::Evt,
        "cmd" => TokenKind::Cmd,
        "qry" => TokenKind::Qry,
        "port" => TokenKind::Port,
        "adapter" => TokenKind::Adapter,
        "flow" => TokenKind::Flow,
        "svc" => TokenKind::Svc,
        "pipe" => TokenKind::Pipe,
        "lang" => TokenKind::Lang,
        "pkg" => TokenKind::Pkg,
        "use" => TokenKind::Use,
        "expose" => TokenKind::Expose,
        "node" => TokenKind::Node,
        "step" => TokenKind::Step,
        "par" => TokenKind::Par,
        "alt" => TokenKind::Alt,
        "loop" => TokenKind::Loop,
        "err" => TokenKind::Err,
        "match" => TokenKind::Match,
        "emit" => TokenKind::Emit,
        "call" => TokenKind::Call,
        "ret" => TokenKind::Ret,
        "input" => TokenKind::Input,
        "fallback" => TokenKind::Fallback,
        "impl" => TokenKind::Impl,
        "for" => TokenKind::For,
        "boundary" => TokenKind::Boundary,
        "as" => TokenKind::As,
        "desc" => TokenKind::Desc,
        "output" => TokenKind::Output,
        "constraints" => TokenKind::Constraints,
        "saga" => TokenKind::Saga,
        "fn" => TokenKind::Fn,
        "state" => TokenKind::State,
        "root" => TokenKind::Root,
        "compensate" => TokenKind::Compensate,
        "contexts" => TokenKind::Contexts,
        _ => TokenKind::Ident,
    }
}

/// Check if a word is a construct keyword (used during annotation arg parsing
/// to avoid consuming keywords as annotation arguments).
fn is_construct_keyword(word: &str) -> bool {
    matches!(
        word,
        "sol" | "ctx" | "agg" | "ent" | "val" | "evt" | "cmd" | "qry"
            | "port" | "adapter" | "flow" | "svc" | "pipe" | "lang"
            | "pkg" | "use" | "expose" | "node" | "saga"
            | "step" | "par" | "alt" | "loop" | "err" | "match"
            | "emit" | "call" | "ret" | "input" | "fallback" | "impl"
            | "for" | "boundary" | "as" | "desc" | "output" | "constraints"
            | "fn" | "state" | "root" | "compensate" | "contexts"
    )
}

#[cfg(test)]
#[path = "lexer_tests.rs"]
mod tests;
