//! VEIL Parser — lexer and parser for the VEIL language.

pub mod lexer;
pub mod parser;

pub use lexer::{Token, TokenKind, lex};
pub use parser::{parse, parse_file, parse_with_registry, parse_file_with_registry, ParseError};
