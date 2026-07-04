//! VEIL Parser — consumes token stream and produces AST.
//!
//! This is a recursive descent parser operating on the token stream from the lexer.
//! It uses INDENT/DEDENT tokens to determine block structure.

use veil_ir::ast::*;
use veil_ir::span::Span;

use crate::lexer::{Token, TokenKind};

// ─── Generic construct dispatch ───────────────────────────────────────────────
//
// Instead of hardcoding which TokenKind maps to which parse function,
// we use a data-driven approach: map token kinds to keyword strings,
// then dispatch based on the construct's `maps_to` category.

/// Maps a DDD/layer token kind to its keyword string (as defined in .layer files).
/// Returns None for non-construct tokens.
fn token_kind_to_keyword(kind: &TokenKind) -> Option<&'static str> {
    match kind {
        TokenKind::Ctx => Some("ctx"),
        TokenKind::Agg => Some("agg"),
        TokenKind::Ent => Some("ent"),
        TokenKind::Val => Some("val"),
        TokenKind::Evt => Some("evt"),
        TokenKind::Cmd => Some("cmd"),
        TokenKind::Port => Some("port"),
        TokenKind::Adapter => Some("adapter"),
        TokenKind::Svc => Some("svc"),
        TokenKind::Saga => Some("saga"),
        TokenKind::Orchestrator => Some("orchestrator"),
        TokenKind::Flow => Some("flow"),
        _ => None,
    }
}

/// Construct category — what core primitive a layer keyword maps to.
/// Derived from the `maps_to` field in .layer schema definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructCategory {
    /// maps_to: mod — parsed as module with children (ctx, orchestrator)
    Module,
    /// maps_to: struct — parsed as type with fields (agg, ent, val, evt, cmd)
    Struct,
    /// maps_to: trait — parsed as interface with methods (port, repo)
    Trait,
    /// maps_to: impl — parsed as implementation (adapter)
    Impl,
    /// maps_to: fn — parsed as flow with steps (svc, saga)
    Fn,
}

/// Default keyword→category map (fallback when no layer schema is loaded).
/// When a .layer file is loaded, this is replaced by the schema's keyword/maps_to declarations.
fn default_keyword_categories() -> std::collections::HashMap<String, ConstructCategory> {
    use std::collections::HashMap;
    let mut m = HashMap::new();
    // These are only used as a fallback when no layer schema is provided.
    // The canonical source of truth is the .layer file.
    m.insert("ctx".into(), ConstructCategory::Module);
    m.insert("orchestrator".into(), ConstructCategory::Module);
    m.insert("mod".into(), ConstructCategory::Module);
    m.insert("agg".into(), ConstructCategory::Struct);
    m.insert("ent".into(), ConstructCategory::Struct);
    m.insert("val".into(), ConstructCategory::Struct);
    m.insert("evt".into(), ConstructCategory::Struct);
    m.insert("cmd".into(), ConstructCategory::Struct);
    m.insert("struct".into(), ConstructCategory::Struct);
    m.insert("port".into(), ConstructCategory::Trait);
    m.insert("repo".into(), ConstructCategory::Trait);
    m.insert("trait".into(), ConstructCategory::Trait);
    m.insert("adapter".into(), ConstructCategory::Impl);
    m.insert("impl".into(), ConstructCategory::Impl);
    m.insert("svc".into(), ConstructCategory::Fn);
    m.insert("saga".into(), ConstructCategory::Fn);
    m.insert("flow".into(), ConstructCategory::Fn);
    m.insert("fn".into(), ConstructCategory::Fn);
    m
}

/// Build a keyword→category map from a LayerSchema.
pub fn categories_from_layer(constructs: &[(String, String)]) -> std::collections::HashMap<String, ConstructCategory> {
    use std::collections::HashMap;
    let mut m = HashMap::new();
    for (keyword, maps_to) in constructs {
        let cat = match maps_to.as_str() {
            "mod" => ConstructCategory::Module,
            "struct" => ConstructCategory::Struct,
            "trait" => ConstructCategory::Trait,
            "impl" => ConstructCategory::Impl,
            "fn" => ConstructCategory::Fn,
            _ => continue,
        };
        m.insert(keyword.clone(), cat);
    }
    // Always include core primitives
    m.insert("mod".into(), ConstructCategory::Module);
    m.insert("struct".into(), ConstructCategory::Struct);
    m.insert("trait".into(), ConstructCategory::Trait);
    m.insert("impl".into(), ConstructCategory::Impl);
    m.insert("fn".into(), ConstructCategory::Fn);
    m
}

/// Check if a token kind represents a construct keyword that can appear
/// inside a context/module body.
#[allow(dead_code)]

/// Parse error with span information.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "parse error at {}-{}: {}",
            self.span.start, self.span.end, self.message
        )
    }
}

/// Parse a token stream into a VeilFile (Solution, Package, or Composition).
pub fn parse(tokens: &[Token]) -> Result<Solution, Vec<ParseError>> {
    parse_with_keywords(tokens, None)
}

/// Parse with a keyword→category map loaded from layer files.
/// When `keywords` is None, uses the built-in defaults.
pub fn parse_with_keywords(tokens: &[Token], keywords: Option<std::collections::HashMap<String, ConstructCategory>>) -> Result<Solution, Vec<ParseError>> {
    let mut parser = match keywords {
        Some(kw) => Parser::with_keywords(tokens, kw),
        None => Parser::new(tokens),
    };
    parser.skip_newlines();

    // Detect file type by first keyword
    match parser.peek_kind().clone() {
        TokenKind::Pkg => {
            // Parse as package, but return the inner Solution for now
            match parser.parse_package() {
                Ok(pkg) => {
                    let sol = Solution {
                        name: pkg.name,
                        span: pkg.span,
                        items: pkg.items,
                    };
                    if parser.errors.is_empty() {
                        Ok(sol)
                    } else {
                        Err(parser.errors)
                    }
                }
                Err(e) => {
                    parser.errors.push(e);
                    Err(parser.errors)
                }
            }
        }
        TokenKind::Use => {
            // Parse as composition, convert flows to Solution
            match parser.parse_composition() {
                Ok(comp) => {
                    let sol = Solution {
                        name: "composition".to_string(),
                        span: comp.span,
                        items: comp.flows.into_iter().map(TopLevelItem::Flow).collect(),
                    };
                    if parser.errors.is_empty() {
                        Ok(sol)
                    } else {
                        Err(parser.errors)
                    }
                }
                Err(e) => {
                    parser.errors.push(e);
                    Err(parser.errors)
                }
            }
        }
        _ => {
            // Parse as solution (existing behavior)
            match parser.parse_solution() {
                Ok(sol) => {
                    if parser.errors.is_empty() {
                        Ok(sol)
                    } else {
                        Err(parser.errors)
                    }
                }
                Err(e) => {
                    parser.errors.push(e);
                    Err(parser.errors)
                }
            }
        }
    }
}

/// Parse a token stream into a full VeilFile with package support.
pub fn parse_file(tokens: &[Token]) -> Result<VeilFile, Vec<ParseError>> {
    parse_file_with_keywords(tokens, None)
}

/// Parse a token stream into a VeilFile with layer-driven keywords.
pub fn parse_file_with_keywords(tokens: &[Token], keywords: Option<std::collections::HashMap<String, ConstructCategory>>) -> Result<VeilFile, Vec<ParseError>> {
    let mut parser = match keywords {
        Some(kw) => Parser::with_keywords(tokens, kw),
        None => Parser::new(tokens),
    };
    parser.skip_newlines();

    match parser.peek_kind().clone() {
        TokenKind::Pkg => {
            match parser.parse_package() {
                Ok(pkg) => {
                    if parser.errors.is_empty() {
                        Ok(VeilFile::Package(pkg))
                    } else {
                        Err(parser.errors)
                    }
                }
                Err(e) => {
                    parser.errors.push(e);
                    Err(parser.errors)
                }
            }
        }
        TokenKind::Use => {
            match parser.parse_composition() {
                Ok(comp) => {
                    if parser.errors.is_empty() {
                        Ok(VeilFile::Composition(comp))
                    } else {
                        Err(parser.errors)
                    }
                }
                Err(e) => {
                    parser.errors.push(e);
                    Err(parser.errors)
                }
            }
        }
        _ => {
            match parser.parse_solution() {
                Ok(sol) => {
                    if parser.errors.is_empty() {
                        Ok(VeilFile::Solution(sol))
                    } else {
                        Err(parser.errors)
                    }
                }
                Err(e) => {
                    parser.errors.push(e);
                    Err(parser.errors)
                }
            }
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    errors: Vec<ParseError>,
    /// Keyword→Category mapping loaded from .layer files.
    keyword_categories: std::collections::HashMap<String, ConstructCategory>,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            keyword_categories: default_keyword_categories(),
        }
    }

    fn with_keywords(tokens: &'a [Token], keywords: std::collections::HashMap<String, ConstructCategory>) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            keyword_categories: keywords,
        }
    }

    fn lookup_category(&self, keyword: &str) -> Option<ConstructCategory> {
        self.keyword_categories.get(keyword).copied()
    }

    // ─── Navigation helpers ───────────────────────────────────────────

    fn current(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&self.tokens[self.tokens.len() - 1])
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, ParseError> {
        if self.at(kind) {
            Ok(self.advance())
        } else {
            Err(self.error(format!("expected {:?}, got {:?}", kind, self.peek_kind())))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        if self.at(&TokenKind::Ident) {
            Ok(self.advance().text)
        } else {
            Err(self.error(format!("expected identifier, got {:?}", self.peek_kind())))
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&TokenKind::Newline) || self.at(&TokenKind::Comment) {
            self.advance();
        }
    }

    /// Collect consecutive comment tokens as doc comments (strips # prefix).
    fn collect_doc_comments(&mut self) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        while self.at(&TokenKind::Comment) {
            let text = self.advance().text;
            // Strip "# " prefix
            let line = text.strip_prefix("# ")
                .or_else(|| text.strip_prefix("#"))
                .unwrap_or(&text)
                .to_string();
            lines.push(line);
            // Skip newline after comment
            if self.at(&TokenKind::Newline) {
                self.advance();
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    fn error(&self, message: String) -> ParseError {
        ParseError {
            message,
            span: self.current().span,
        }
    }

    /// Check if we're at the start of a block (INDENT follows).
    fn at_block_start(&self) -> bool {
        // Look ahead past newlines for an INDENT
        let mut i = self.pos;
        while i < self.tokens.len() {
            match self.tokens[i].kind {
                TokenKind::Newline | TokenKind::Comment => i += 1,
                TokenKind::Indent => return true,
                _ => return false,
            }
        }
        false
    }

    /// Enter a block: skip newlines and consume INDENT.
    fn enter_block(&mut self) -> Result<(), ParseError> {
        self.skip_newlines();
        self.expect(&TokenKind::Indent)?;
        Ok(())
    }

    /// Check if current position is at block end (DEDENT or EOF).
    fn at_block_end(&self) -> bool {
        let mut i = self.pos;
        while i < self.tokens.len() {
            match self.tokens[i].kind {
                TokenKind::Newline | TokenKind::Comment => i += 1,
                TokenKind::Dedent | TokenKind::Eof => return true,
                _ => return false,
            }
        }
        true
    }

    /// Exit a block: skip newlines and consume DEDENT.
    fn exit_block(&mut self) {
        self.skip_newlines();
        if self.at(&TokenKind::Dedent) {
            self.advance();
        }
    }

    // ─── Type parsing ─────────────────────────────────────────────────

    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        let name = self.expect_ident()?;

        // Check for Res! syntax
        if self.at(&TokenKind::Bang) {
            self.advance(); // consume !
            if self.at(&TokenKind::LAngle) {
                self.advance(); // consume <
                let inner = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                return Ok(TypeExpr::Result(Some(Box::new(inner))));
            }
            return Ok(TypeExpr::Result(None));
        }

        // Check for generic: Name<T> or Name<T, U>
        if self.at(&TokenKind::LAngle) {
            self.advance(); // consume <
            let mut args = vec![self.parse_type()?];
            while self.at(&TokenKind::Comma) {
                self.advance();
                args.push(self.parse_type()?);
            }
            self.expect(&TokenKind::RAngle)?;

            // Map known generic names to specific variants
            return match name.as_str() {
                "Opt" => Ok(TypeExpr::Optional(Box::new(args.into_iter().next().unwrap()))),
                "List" => Ok(TypeExpr::List(Box::new(args.into_iter().next().unwrap()))),
                "Set" => Ok(TypeExpr::Set(Box::new(args.into_iter().next().unwrap()))),
                "Map" if args.len() == 2 => {
                    let mut it = args.into_iter();
                    Ok(TypeExpr::Map(Box::new(it.next().unwrap()), Box::new(it.next().unwrap())))
                }
                _ => Ok(TypeExpr::Generic(name, args)),
            };
        }

        Ok(TypeExpr::Named(name))
    }

    // ─── Annotation parsing ───────────────────────────────────────────

    fn parse_annotations(&mut self) -> Vec<Annotation> {
        let mut annotations = Vec::new();
        while self.at(&TokenKind::Annotation) {
            let tok = self.advance().clone();
            let text = tok.text.clone();
            // Parse @name or @name(args) or @name arg1 arg2
            let (name, args) = parse_annotation_text(&text);
            annotations.push(Annotation {
                name,
                args,
                span: tok.span,
            });
            self.skip_newlines();
        }
        annotations
    }
}

/// Parse annotation text like "@async", "@retry(3)", "@trace(method=\"xray\")", "@env(TWILIO_SID, TWILIO_TOKEN)"
fn parse_annotation_text(text: &str) -> (String, Vec<String>) {
    let text = text.strip_prefix('@').unwrap_or(text);

    // Check for parenthesized args
    if let Some(paren_idx) = text.find('(') {
        let name = text[..paren_idx].to_string();
        let args_str = &text[paren_idx + 1..text.len() - 1]; // strip parens
        let args: Vec<String> = args_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (name, args)
    } else {
        // No args
        (text.to_string(), Vec::new())
    }
}

// ─── Top-level parsing ────────────────────────────────────────────────────

impl<'a> Parser<'a> {
    fn parse_package(&mut self) -> Result<Package, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Pkg)?;
        let name = self.expect_ident()?;

        // Optional version (e.g., "v1.0" as an identifier)
        let version = if self.at(&TokenKind::Ident) {
            Some(self.advance().text)
        } else {
            None
        };

        let mut metadata = Vec::new();
        let mut items = Vec::new();
        let mut expose = None;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                match self.peek_kind().clone() {
                    // Metadata: author "...", desc "..."
                    TokenKind::Ident if self.is_metadata_key() => {
                        let key_span = self.current().span;
                        let key = self.advance().text;
                        let value = if self.at(&TokenKind::StringLit) {
                            let text = self.advance().text;
                            text[1..text.len()-1].to_string()
                        } else if self.at(&TokenKind::Ident) {
                            self.advance().text
                        } else {
                            String::new()
                        };
                        metadata.push(PackageMeta { key, value, span: key_span });
                    }
                    TokenKind::Desc => {
                        let key_span = self.current().span;
                        self.advance();
                        let value = if self.at(&TokenKind::StringLit) {
                            let text = self.advance().text;
                            text[1..text.len()-1].to_string()
                        } else {
                            String::new()
                        };
                        metadata.push(PackageMeta { key: "desc".to_string(), value, span: key_span });
                    }
                    TokenKind::Lang => {
                        items.push(TopLevelItem::Lang(self.parse_lang_block()?));
                    }
                    TokenKind::Expose => {
                        expose = Some(self.parse_expose_block()?);
                    }
                    TokenKind::Comment => { self.advance(); }
                    // Generic dispatch for construct keywords in package body
                    ref kind if token_kind_to_keyword(kind).is_some() => {
                        let keyword = token_kind_to_keyword(&self.peek_kind().clone()).unwrap();
                        let category = self.lookup_category(keyword);
                        match category {
                            Some(ConstructCategory::Module) => {
                                items.push(TopLevelItem::Context(self.parse_construct_module(keyword)?));
                            }
                            Some(ConstructCategory::Fn) if keyword == "flow" => {
                                items.push(TopLevelItem::Flow(self.parse_flow()?));
                            }
                            Some(ConstructCategory::Impl) => {
                                items.push(TopLevelItem::Adapter(self.parse_adapter()?));
                            }
                            _ => {
                                self.errors.push(self.error(format!(
                                    "unexpected construct '{}' in package body", keyword
                                )));
                                self.advance();
                            }
                        }
                    }
                    _ => {
                        let err = self.error(format!(
                            "unexpected token {:?} in package body", self.peek_kind()
                        ));
                        self.errors.push(err);
                        self.advance();
                    }
                }
            }
            self.exit_block();
        }

        Ok(Package {
            name,
            version,
            span: start_span.merge(self.current().span),
            metadata,
            items,
            expose,
        })
    }

    fn is_metadata_key(&self) -> bool {
        let text = &self.current().text;
        matches!(text.as_str(), "author" | "license" | "repo")
    }

    fn parse_expose_block(&mut self) -> Result<ExposeBlock, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Expose)?;

        let mut nodes = Vec::new();
        let mut constraints = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.peek_kind().clone() {
                    TokenKind::Node => {
                        nodes.push(self.parse_exposed_node()?);
                    }
                    TokenKind::Constraints => {
                        constraints = self.parse_constraints()?;
                    }
                    _ => { self.errors.push(self.error(format!("unexpected token {:?} in expose block", self.peek_kind()))); self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(ExposeBlock {
            span: start_span.merge(self.current().span),
            nodes,
            constraints,
        })
    }

    fn parse_exposed_node(&mut self) -> Result<ExposedNode, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Node)?;
        let name = self.expect_ident()?;

        let mut description = None;
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.peek_kind().clone() {
                    TokenKind::Desc => {
                        self.advance();
                        if self.at(&TokenKind::StringLit) {
                            let text = self.advance().text;
                            description = Some(text[1..text.len()-1].to_string());
                        }
                    }
                    TokenKind::Input => {
                        self.advance();
                        if self.at_block_start() {
                            self.enter_block()?;
                            while !self.at_block_end() {
                                self.skip_newlines();
                                if self.at_block_end() { break; }
                                if self.at(&TokenKind::Ident) {
                                    inputs.push(self.parse_field()?);
                                } else {
                                    self.advance();
                                }
                            }
                            self.exit_block();
                        }
                    }
                    TokenKind::Output => {
                        self.advance();
                        if self.at_block_start() {
                            self.enter_block()?;
                            while !self.at_block_end() {
                                self.skip_newlines();
                                if self.at_block_end() { break; }
                                if self.at(&TokenKind::Ident) {
                                    outputs.push(self.parse_field()?);
                                } else {
                                    self.advance();
                                }
                            }
                            self.exit_block();
                        }
                    }
                    _ => { self.errors.push(self.error(format!("unexpected token {:?} in exposed node", self.peek_kind()))); self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(ExposedNode {
            name,
            description,
            inputs,
            outputs,
            span: start_span.merge(self.current().span),
        })
    }

    fn parse_constraints(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(&TokenKind::Constraints)?;
        let mut constraints = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                // Collect each line as a constraint string
                let mut parts = Vec::new();
                while !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Eof)
                    && !self.at(&TokenKind::Dedent)
                {
                    parts.push(self.advance().text);
                }
                if !parts.is_empty() {
                    constraints.push(parts.join(" "));
                }
            }
            self.exit_block();
        }
        Ok(constraints)
    }

    fn parse_composition(&mut self) -> Result<Composition, ParseError> {
        let start_span = self.current().span;
        let mut imports = Vec::new();
        let mut flows = Vec::new();

        // Parse use statements
        while self.at(&TokenKind::Use) {
            imports.push(self.parse_use_import()?);
            self.skip_newlines();
        }

        // Parse flows
        while !self.at(&TokenKind::Eof) {
            self.skip_newlines();
            if self.at(&TokenKind::Eof) { break; }
            if self.at(&TokenKind::Comment) { self.advance(); continue; }
            if self.at(&TokenKind::Flow) {
                flows.push(self.parse_flow()?);
            } else {
                self.advance();
            }
        }

        Ok(Composition {
            imports,
            flows,
            span: start_span.merge(self.current().span),
        })
    }

    fn parse_use_import(&mut self) -> Result<UseImport, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Use)?;
        let package_name = self.expect_ident()?;

        let alias = if self.at(&TokenKind::As) {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };

        Ok(UseImport {
            package_name,
            alias,
            span: start_span.merge(self.current().span),
        })
    }

    fn parse_solution(&mut self) -> Result<Solution, ParseError> {
        self.skip_newlines();
        let start_span = self.current().span;
        self.expect(&TokenKind::Sol)?;
        let name = self.expect_ident()?;

        let mut items = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                // Collect doc comments that precede the next construct
                let doc = self.collect_doc_comments();
                // Collect annotations that decorate the next construct
                let prefix_annotations = self.parse_annotations();

                // Generic construct dispatch: look up token's keyword and category
                let current_kind = self.peek_kind().clone();
                match current_kind {
                    TokenKind::Use => {
                        // Kit declaration — consume and store for later resolution
                        let _import = self.parse_use_import()?;
                        let _ = doc;
                        // TODO: resolve kit and apply constraints/metadata
                    }
                    TokenKind::Allow | TokenKind::Deny => {
                        // Allow/deny construct lists — consume the block
                        let _ = doc;
                        self.advance(); // consume allow/deny keyword
                        if self.at_block_start() {
                            self.enter_block()?;
                            while !self.at_block_end() {
                                self.skip_newlines();
                                if self.at_block_end() { break; }
                                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                                if self.at(&TokenKind::Ident) { self.advance(); }
                                else { self.advance(); }
                            }
                            self.exit_block();
                        }
                    }
                    TokenKind::Lang => {
                        let _ = doc;
                        items.push(TopLevelItem::Lang(self.parse_lang_block()?));
                    }
                    TokenKind::Comment => {
                        self.advance();
                    }
                    // Generic dispatch for all construct keywords
                    ref kind if token_kind_to_keyword(kind).is_some() => {
                        let keyword = token_kind_to_keyword(&current_kind).unwrap();
                        let category = self.lookup_category(keyword);
                        match category {
                            Some(ConstructCategory::Module) => {
                                let _ = doc;
                                let ctx = self.parse_construct_module(keyword)?;
                                let _ = prefix_annotations;
                                items.push(TopLevelItem::Context(ctx));
                            }
                            Some(ConstructCategory::Fn) if keyword == "flow" => {
                                let mut flow = self.parse_flow()?;
                                let mut all = prefix_annotations;
                                all.extend(flow.annotations);
                                flow.annotations = all;
                                items.push(TopLevelItem::Flow(flow));
                            }
                            Some(ConstructCategory::Fn) if keyword == "saga" => {
                                let mut saga = self.parse_saga()?;
                                let mut all = prefix_annotations;
                                all.extend(saga.annotations);
                                saga.annotations = all;
                                items.push(TopLevelItem::Saga(saga));
                            }
                            Some(ConstructCategory::Impl) => {
                                let mut adapter = self.parse_adapter()?;
                                let mut all = prefix_annotations;
                                all.extend(adapter.annotations);
                                adapter.annotations = all;
                                items.push(TopLevelItem::Adapter(adapter));
                            }
                            _ => {
                                let err = self.error(format!(
                                    "construct '{}' not allowed at solution level",
                                    keyword
                                ));
                                self.errors.push(err);
                                self.advance();
                            }
                        }
                    }
                    _ => {
                        if !prefix_annotations.is_empty() || doc.is_some() {
                            // Annotations/comments with no following construct — skip
                        } else {
                            let err = self.error(format!(
                                "unexpected token {:?} in solution body",
                                self.peek_kind()
                            ));
                            self.errors.push(err);
                            self.advance();
                        }
                    }
                }
            }
            self.exit_block();
        }

        Ok(Solution {
            name,
            span: start_span.merge(self.current().span),
            items,
        })
    }

    fn parse_lang_block(&mut self) -> Result<LangBlock, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Lang)?;

        let mut entries = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Comment) {
                    self.advance();
                    continue;
                }
                // Parse "Term: definition text..."
                let entry_span = self.current().span;
                let term = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                // Collect all remaining tokens on this line as the definition
                let mut def_parts = Vec::new();
                while !self.at(&TokenKind::Newline)
                    && !self.at(&TokenKind::Eof)
                    && !self.at(&TokenKind::Dedent)
                {
                    def_parts.push(self.advance().text.clone());
                }
                entries.push(LangEntry {
                    term,
                    definition: def_parts.join(" "),
                    span: entry_span.merge(self.current().span),
                });
            }
            self.exit_block();
        }

        Ok(LangBlock {
            span: start_span.merge(self.current().span),
            entries,
        })
    }

    /// Generic module construct parser — dispatches based on keyword.
    /// Handles both "ctx" (bounded context) and "orchestrator" keywords,
    /// as well as any future module-mapped constructs from layer schemas.
    fn parse_construct_module(&mut self, keyword: &str) -> Result<Context, ParseError> {
        match keyword {
            "orchestrator" => self.parse_orchestrator(),
            // All other module-mapped keywords (ctx, or any future ones) use context parsing
            _ => self.parse_context(),
        }
    }

    fn parse_orchestrator(&mut self) -> Result<Context, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Orchestrator)?;
        let name = self.expect_ident()?;

        let mut items = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                let _annotations = self.parse_annotations();
                match self.peek_kind().clone() {
                    TokenKind::Group => {
                        items.push(ContextItem::Group(self.parse_group()?));
                    }
                    TokenKind::Export | TokenKind::Saga => {
                        if self.at(&TokenKind::Export) {
                            self.advance();
                        }
                        if self.at(&TokenKind::Saga) {
                            let saga = self.parse_saga()?;
                            let mut annotations = saga.annotations;
                            annotations.push(Annotation {
                                name: "__saga".to_string(),
                                args: saga.context_refs.clone(),
                                span: saga.span,
                            });
                            items.push(ContextItem::Construct(Construct::from_service(Service {
                                name: saga.name,
                                span: saga.span,
                                annotations,
                                inputs: saga.inputs,
                                steps: saga.steps.iter().map(|s| FlowStep::Step(StepDef {
                                    name: s.name.clone(),
                                    span: s.span,
                                    body: s.body.clone(),
                                })).collect(),
                                return_expr: None,
                            }, "saga")));
                        }
                    }
                    _ => { self.errors.push(self.error(format!("unexpected token {:?} in orchestrator body", self.peek_kind()))); self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(Context {
            name,
            span: start_span.merge(self.current().span),
            items,
        })
    }

    fn parse_context(&mut self) -> Result<Context, ParseError> {
        let start_span = self.current().span;
        // Consume any module-mapped keyword (ctx, or future keywords)
        self.advance();
        let name = self.expect_ident()?;

        let mut items = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                let annotations = self.parse_annotations();
                // Generic dispatch for context children
                if let Some(item) = self.parse_context_child(annotations)? {
                    items.push(item);
                }
            }
            self.exit_block();
        }

        Ok(Context {
            name,
            span: start_span.merge(self.current().span),
            items,
        })
    }

    /// Generic dispatch for children inside a context/module body.
    /// Uses keyword→category lookup instead of hardcoded token matching.
    fn parse_context_child(&mut self, annotations: Vec<Annotation>) -> Result<Option<ContextItem>, ParseError> {
        let kind = self.peek_kind().clone();

        // Handle non-construct tokens
        match kind {
            TokenKind::Comment => { self.advance(); return Ok(None); }
            TokenKind::Group => return Ok(Some(ContextItem::Group(self.parse_group()?))),
            _ => {}
        }

        // Look up keyword from token kind for generic dispatch
        if let Some(keyword) = token_kind_to_keyword(&kind) {
            let category = self.lookup_category(keyword);
            match (category, keyword) {
                // struct-mapped: val, ent, agg
                (Some(ConstructCategory::Struct), "val") => {
                    Ok(Some(ContextItem::Construct(Construct::from_value_object(self.parse_value_object(annotations)?))))
                }
                (Some(ConstructCategory::Struct), "ent") => {
                    Ok(Some(ContextItem::Construct(Construct::from_entity(self.parse_entity(annotations)?))))
                }
                (Some(ConstructCategory::Struct), "agg") => {
                    Ok(Some(ContextItem::Construct(Construct::from_aggregate(self.parse_aggregate(annotations)?))))
                }
                // trait-mapped: port, repo
                (Some(ConstructCategory::Trait), _) => {
                    Ok(Some(ContextItem::Construct(Construct::from_port(self.parse_port()?))))
                }
                // fn-mapped: svc
                (Some(ConstructCategory::Fn), "svc") => {
                    let svc_flow = self.parse_domain_service()?;
                    Ok(Some(ContextItem::Construct(Construct::from_service(svc_flow, "svc"))))
                }
                // impl-mapped: adapter
                (Some(ConstructCategory::Impl), _) => {
                    Ok(Some(ContextItem::Construct(Construct::from_adapter(self.parse_adapter()?))))
                }
                _ => {
                    let err = self.error(format!(
                        "construct '{}' not expected in context body",
                        keyword
                    ));
                    self.errors.push(err);
                    self.advance();
                    Ok(None)
                }
            }
        } else {
            let err = self.error(format!(
                "unexpected token {:?} in context body",
                self.peek_kind()
            ));
            self.errors.push(err);
            self.advance();
            Ok(None)
        }
    }
}

// ─── Domain construct parsing ─────────────────────────────────────────────

impl<'a> Parser<'a> {
    fn parse_group(&mut self) -> Result<Group, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Group)?;
        let name = self.expect_ident()?;

        let mut items = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                let annotations = self.parse_annotations();
                // Generic dispatch for group children
                if let Some(item) = self.parse_group_child(annotations)? {
                    items.push(item);
                }
            }
            self.exit_block();
        }

        Ok(Group {
            name,
            span: start_span.merge(self.current().span),
            items,
        })
    }

    /// Generic dispatch for children inside a group body.
    /// Groups can contain the same constructs as contexts, plus exported sagas.
    fn parse_group_child(&mut self, annotations: Vec<Annotation>) -> Result<Option<ContextItem>, ParseError> {
        let kind = self.peek_kind().clone();

        // Handle special cases first
        match kind {
            TokenKind::Comment => { self.advance(); return Ok(None); }
            TokenKind::Export => {
                // Export is a modifier — skip it, parse the next construct
                self.advance(); // consume 'export'
                if self.at(&TokenKind::Saga) {
                    let saga = self.parse_saga()?;
                    let mut ann = saga.annotations;
                    ann.push(Annotation {
                        name: "__saga".to_string(),
                        args: saga.context_refs.clone(),
                        span: saga.span,
                    });
                    return Ok(Some(ContextItem::Construct(Construct::from_service(Service {
                        name: saga.name,
                        span: saga.span,
                        annotations: ann,
                        inputs: saga.inputs,
                        steps: saga.steps.iter().map(|s| FlowStep::Step(StepDef {
                            name: s.name.clone(),
                            span: s.span,
                            body: s.body.clone(),
                        })).collect(),
                        return_expr: None,
                    }, "saga"))));
                } else if self.at(&TokenKind::Svc) || self.at(&TokenKind::Flow) {
                    let svc_flow = self.parse_domain_service()?;
                    return Ok(Some(ContextItem::Construct(Construct::from_service(svc_flow, "svc"))));
                }
                return Ok(None);
            }
            _ => {}
        }

        // Look up keyword for generic dispatch
        if let Some(keyword) = token_kind_to_keyword(&kind) {
            let category = self.lookup_category(keyword);
            match (category, keyword) {
                (Some(ConstructCategory::Struct), "val") => {
                    Ok(Some(ContextItem::Construct(Construct::from_value_object(self.parse_value_object(annotations)?))))
                }
                (Some(ConstructCategory::Struct), "ent") => {
                    Ok(Some(ContextItem::Construct(Construct::from_entity(self.parse_entity(annotations)?))))
                }
                (Some(ConstructCategory::Struct), "agg") => {
                    Ok(Some(ContextItem::Construct(Construct::from_aggregate(self.parse_aggregate(annotations)?))))
                }
                (Some(ConstructCategory::Trait), _) => {
                    Ok(Some(ContextItem::Construct(Construct::from_port(self.parse_port()?))))
                }
                (Some(ConstructCategory::Fn), "svc") => {
                    let svc_flow = self.parse_domain_service()?;
                    Ok(Some(ContextItem::Construct(Construct::from_service(svc_flow, "svc"))))
                }
                (Some(ConstructCategory::Fn), "saga") => {
                    let saga = self.parse_saga()?;
                    let mut ann = saga.annotations;
                    ann.push(Annotation {
                        name: "__saga".to_string(),
                        args: saga.context_refs.clone(),
                        span: saga.span,
                    });
                    Ok(Some(ContextItem::Construct(Construct::from_service(Service {
                        name: saga.name,
                        span: saga.span,
                        annotations: ann,
                        inputs: saga.inputs,
                        steps: saga.steps.iter().map(|s| FlowStep::Step(StepDef {
                            name: s.name.clone(),
                            span: s.span,
                            body: s.body.clone(),
                        })).collect(),
                        return_expr: None,
                    }, "saga"))))
                }
                (Some(ConstructCategory::Impl), _) => {
                    Ok(Some(ContextItem::Construct(Construct::from_adapter(self.parse_adapter()?))))
                }
                _ => {
                    self.errors.push(self.error(format!("unexpected construct '{}' in group body", keyword)));
                    self.advance();
                    Ok(None)
                }
            }
        } else {
            self.errors.push(self.error(format!("unexpected token {:?} in group body", self.peek_kind())));
            self.advance();
            Ok(None)
        }
    }

    fn parse_value_object(&mut self, annotations: Vec<Annotation>) -> Result<ValueObject, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Val)?;
        let name = self.expect_ident()?;

        let mut fields = Vec::new();
        let mut field_annotations = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    field_annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Ident) {
                    fields.push(self.parse_field()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        let mut all_annotations = annotations;
        all_annotations.extend(field_annotations);

        Ok(ValueObject {
            name,
            span: start_span.merge(self.current().span),
            fields,
            annotations: all_annotations,
        })
    }

    fn parse_entity(&mut self, annotations: Vec<Annotation>) -> Result<Entity, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Ent)?;
        let name = self.expect_ident()?;

        let mut fields = Vec::new();
        let mut field_annotations = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    field_annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Ident) {
                    fields.push(self.parse_field()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        let mut all_annotations = annotations;
        all_annotations.extend(field_annotations);

        Ok(Entity {
            name,
            span: start_span.merge(self.current().span),
            fields,
            annotations: all_annotations,
        })
    }

    fn parse_aggregate(&mut self, annotations: Vec<Annotation>) -> Result<Aggregate, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Agg)?;
        let name = self.expect_ident()?;

        let mut fields = Vec::new();
        let mut events = Vec::new();
        let mut commands = Vec::new();
        let mut state_machines = Vec::new();
        let mut methods = Vec::new();
        let mut agg_annotations = annotations;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    agg_annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.peek_kind().clone() {
                    TokenKind::Evt => events.push(self.parse_event()?),
                    TokenKind::Cmd => commands.push(self.parse_command()?),
                    TokenKind::Root => {
                        // root block is just fields
                        self.advance(); // consume 'root'
                        if self.at_block_start() {
                            self.enter_block()?;
                            while !self.at_block_end() {
                                self.skip_newlines();
                                if self.at_block_end() { break; }
                                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                                if self.at(&TokenKind::Ident) {
                                    fields.push(self.parse_field()?);
                                } else { self.advance(); }
                            }
                            self.exit_block();
                        }
                    }
                    TokenKind::State => {
                        state_machines.push(self.parse_state_machine()?);
                    }
                    TokenKind::Fn => {
                        methods.push(self.parse_aggregate_fn()?);
                    }
                    TokenKind::Ident => fields.push(self.parse_field()?),
                    _ => { self.errors.push(self.error(format!("unexpected token {:?} in aggregate body", self.peek_kind()))); self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(Aggregate {
            name,
            span: start_span.merge(self.current().span),
            fields,
            annotations: agg_annotations,
            events,
            commands,
            state_machines,
            methods,
        })
    }

    fn parse_event(&mut self) -> Result<Event, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Evt)?;
        let name = self.expect_ident()?;

        let mut fields = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Ident) {
                    fields.push(self.parse_field_or_shorthand()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(Event {
            name,
            span: start_span.merge(self.current().span),
            fields,
        })
    }

    fn parse_command(&mut self) -> Result<Command, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Cmd)?;
        let name = self.expect_ident()?;

        let mut fields = Vec::new();
        let mut return_type = None;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Arrow) {
                    self.advance(); // consume ->
                    return_type = Some(self.parse_type()?);
                } else if self.at(&TokenKind::Ident) {
                    fields.push(self.parse_field()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(Command {
            name,
            span: start_span.merge(self.current().span),
            fields,
            return_type,
        })
    }

    fn parse_state_machine(&mut self) -> Result<StateMachine, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::State)?;
        let name = self.expect_ident()?;

        let mut transitions = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                // Parse transition chains: Pending -> Verified -> Active
                if self.at(&TokenKind::Ident) {
                    let trans_span = self.current().span;
                    let mut states = vec![self.advance().text.clone()];
                    while self.at(&TokenKind::Arrow) {
                        self.advance();
                        if self.at(&TokenKind::Ident) {
                            states.push(self.advance().text.clone());
                        }
                    }
                    // Create pairwise transitions
                    for i in 0..states.len().saturating_sub(1) {
                        transitions.push(StateTransition {
                            from: states[i].clone(),
                            to: states[i + 1].clone(),
                            span: trans_span,
                        });
                    }
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(StateMachine {
            name,
            span: start_span.merge(self.current().span),
            transitions,
        })
    }

    fn parse_aggregate_fn(&mut self) -> Result<AggregateFn, ParseError> {
        let start_span = self.current().span;
        let annotations = self.parse_annotations();
        self.expect(&TokenKind::Fn)?;
        let name = self.expect_ident()?;

        // Parse params
        let mut params = Vec::new();
        if self.at(&TokenKind::LParen) {
            self.advance();
            while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof) {
                let param_span = self.current().span;
                let param_name = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                let param_type = self.parse_type()?;
                params.push(Param {
                    name: param_name,
                    type_expr: param_type,
                    span: param_span.merge(self.current().span),
                });
                if self.at(&TokenKind::Comma) { self.advance(); }
            }
            if self.at(&TokenKind::RParen) { self.advance(); }
        }

        // Optional return type
        let mut return_type = None;
        if self.at(&TokenKind::Arrow) {
            self.advance();
            return_type = Some(self.parse_type()?);
        }

        // Body
        let mut body = Vec::new();
        let mut fn_annotations = annotations;
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    fn_annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.parse_expr() {
                    Ok(expr) => body.push(expr),
                    Err(_) => { self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(AggregateFn {
            name,
            span: start_span.merge(self.current().span),
            params,
            return_type,
            annotations: fn_annotations,
            body,
        })
    }

    /// Parse a field like "name: Type"
    fn parse_field(&mut self) -> Result<Field, ParseError> {
        let start_span = self.current().span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let type_expr = self.parse_type()?;
        Ok(Field {
            name,
            type_expr,
            span: start_span.merge(self.current().span),
        })
    }

    /// Parse a field that might be shorthand (just name, no type) or full "name: Type".
    /// Used in event fields like "id email created" (shorthand) or "verified_at: DateTime".
    fn parse_field_or_shorthand(&mut self) -> Result<Field, ParseError> {
        let start_span = self.current().span;
        let name = self.expect_ident()?;
        if self.at(&TokenKind::Colon) {
            self.advance();
            let type_expr = self.parse_type()?;
            Ok(Field {
                name,
                type_expr,
                span: start_span.merge(self.current().span),
            })
        } else {
            // Shorthand: infer type from name (will be resolved later)
            Ok(Field {
                name: name.clone(),
                type_expr: TypeExpr::Named(name),
                span: start_span,
            })
        }
    }
}

// ─── Port, service, adapter, and flow parsing ────────────────────────────

impl<'a> Parser<'a> {
    fn parse_port(&mut self) -> Result<Port, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Port)?;
        let name = self.expect_ident()?;

        let mut methods = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Ident) {
                    methods.push(self.parse_port_method()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(Port {
            name,
            span: start_span.merge(self.current().span),
            methods,
        })
    }

    fn parse_port_method(&mut self) -> Result<PortMethod, ParseError> {
        let start_span = self.current().span;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;

        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof) {
            let param_span = self.current().span;
            let param_name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let param_type = self.parse_type()?;
            params.push(Param {
                name: param_name,
                type_expr: param_type,
                span: param_span.merge(self.current().span),
            });
            if self.at(&TokenKind::Comma) {
                self.advance();
            }
        }
        self.expect(&TokenKind::RParen)?;

        let mut return_type = None;
        if self.at(&TokenKind::Arrow) {
            self.advance();
            return_type = Some(self.parse_type()?);
        }

        Ok(PortMethod {
            name,
            span: start_span.merge(self.current().span),
            params,
            return_type,
        })
    }

    fn parse_domain_service(&mut self) -> Result<Service, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Svc)?;
        let name = self.expect_ident()?;

        let mut annotations = Vec::new();
        let mut inputs = Vec::new();
        let mut steps = Vec::new();
        let mut return_expr = None;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.peek_kind().clone() {
                    TokenKind::Input => {
                        inputs = self.parse_flow_inputs()?;
                    }
                    TokenKind::Step => {
                        steps.push(FlowStep::Step(self.parse_step_def()?));
                    }
                    TokenKind::Par => {
                        steps.push(FlowStep::Parallel(self.parse_par_block()?));
                    }
                    TokenKind::Ret => {
                        self.advance();
                        if !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Eof) && !self.at(&TokenKind::Dedent) {
                            return_expr = Some(self.parse_expr()?);
                        }
                    }
                    _ => { self.errors.push(self.error(format!("unexpected token {:?} in service/flow body", self.peek_kind()))); self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(Service {
            name,
            span: start_span.merge(self.current().span),
            annotations,
            inputs,
            steps,
            return_expr,
        })
    }

    fn parse_adapter(&mut self) -> Result<Adapter, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Adapter)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::For)?;
        let target_port = self.expect_ident()?;

        let mut annotations = Vec::new();
        let mut impls = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Impl) {
                    impls.push(self.parse_adapter_impl()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(Adapter {
            name,
            target_port,
            span: start_span.merge(self.current().span),
            annotations,
            impls,
        })
    }

    fn parse_adapter_impl(&mut self) -> Result<AdapterImpl, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Impl)?;
        let method_name = self.expect_ident()?;

        // Parse parameter names (no types, just names)
        let mut params = Vec::new();
        if self.at(&TokenKind::LParen) {
            self.advance();
            while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof) {
                params.push(self.expect_ident()?);
                if self.at(&TokenKind::Comma) {
                    self.advance();
                }
            }
            self.expect(&TokenKind::RParen)?;
        }

        // Parse body (skip for now — just consume the block)
        let mut body = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                // For now, consume body as expressions (simplified)
                if let Ok(expr) = self.parse_expr() {
                    body.push(expr);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(AdapterImpl {
            method_name,
            params,
            span: start_span.merge(self.current().span),
            body,
        })
    }

    /// Parse a flow (stub — full implementation in Task 4)
    fn parse_saga(&mut self) -> Result<Saga, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Saga)?;
        let name = self.expect_ident()?;

        let mut annotations = Vec::new();
        let mut context_refs = Vec::new();
        let mut inputs = Vec::new();
        let mut steps = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.peek_kind().clone() {
                    TokenKind::Contexts => {
                        self.advance();
                        // Parse comma-separated context names
                        while !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Eof)
                            && !self.at(&TokenKind::Dedent)
                        {
                            if self.at(&TokenKind::Ident) {
                                context_refs.push(self.advance().text.clone());
                            } else if self.at(&TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    TokenKind::Input => {
                        inputs = self.parse_flow_inputs()?;
                    }
                    TokenKind::Step => {
                        steps.push(self.parse_saga_step()?);
                    }
                    _ => { self.errors.push(self.error(format!("unexpected token {:?} in saga body", self.peek_kind()))); self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(Saga {
            name,
            span: start_span.merge(self.current().span),
            annotations,
            context_refs,
            inputs,
            steps,
        })
    }

    fn parse_saga_step(&mut self) -> Result<SagaStep, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Step)?;
        let name = self.expect_ident()?;

        let mut context = None;
        let mut body = Vec::new();
        let mut compensate = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Ctx) {
                    self.advance();
                    if self.at(&TokenKind::Ident) {
                        context = Some(self.advance().text.clone());
                    }
                } else if self.at(&TokenKind::Compensate) {
                    self.advance();
                    if self.at_block_start() {
                        self.enter_block()?;
                        while !self.at_block_end() {
                            self.skip_newlines();
                            if self.at_block_end() { break; }
                            if self.at(&TokenKind::Comment) { self.advance(); continue; }
                            match self.parse_expr() {
                                Ok(expr) => compensate.push(expr),
                                Err(_) => { self.advance(); }
                            }
                        }
                        self.exit_block();
                    }
                } else {
                    match self.parse_expr() {
                        Ok(expr) => body.push(expr),
                        Err(_) => { self.advance(); }
                    }
                }
            }
            self.exit_block();
        }

        Ok(SagaStep {
            name,
            context,
            span: start_span.merge(self.current().span),
            body,
            compensate,
        })
    }

    fn parse_flow(&mut self) -> Result<Flow, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Flow)?;
        let name = self.expect_ident()?;

        let mut annotations = Vec::new();
        let mut inputs = Vec::new();
        let mut steps = Vec::new();
        let mut error_boundary = None;
        let mut return_expr = None;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.peek_kind().clone() {
                    TokenKind::Input => {
                        inputs = self.parse_flow_inputs()?;
                    }
                    TokenKind::Err => {
                        error_boundary = Some(self.parse_error_boundary()?);
                    }
                    TokenKind::Step => {
                        steps.push(FlowStep::Step(self.parse_step_def()?));
                    }
                    TokenKind::Par => {
                        steps.push(FlowStep::Parallel(self.parse_par_block()?));
                    }
                    TokenKind::Ret => {
                        self.advance(); // consume ret
                        if !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Eof) && !self.at(&TokenKind::Dedent) {
                            return_expr = Some(self.parse_expr()?);
                        }
                    }
                    _ => {
                        self.advance();
                    }
                }
            }
            self.exit_block();
        }

        Ok(Flow {
            name,
            span: start_span.merge(self.current().span),
            annotations,
            inputs,
            steps,
            error_boundary,
            return_expr,
        })
    }

    fn parse_flow_inputs(&mut self) -> Result<Vec<Field>, ParseError> {
        self.expect(&TokenKind::Input)?;
        let mut fields = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Ident) {
                    fields.push(self.parse_field()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }
        Ok(fields)
    }

    fn parse_error_boundary(&mut self) -> Result<ErrorBoundary, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Err)?;
        // expect "boundary"
        if self.at(&TokenKind::Boundary) {
            self.advance();
        }

        let mut annotations = Vec::new();
        let mut fallback = None;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Fallback) {
                    self.advance();
                    if self.at(&TokenKind::Arrow) {
                        self.advance();
                    }
                    fallback = self.parse_expr().ok();
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(ErrorBoundary {
            span: start_span.merge(self.current().span),
            annotations,
            fallback,
        })
    }

    fn parse_step_def(&mut self) -> Result<StepDef, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Step)?;
        let name = self.expect_ident()?;

        let mut body = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                match self.parse_expr() {
                    Ok(expr) => body.push(expr),
                    Err(_) => { self.advance(); }
                }
            }
            self.exit_block();
        }

        Ok(StepDef {
            name,
            span: start_span.merge(self.current().span),
            body,
        })
    }

    fn parse_par_block(&mut self) -> Result<ParBlock, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Par)?;

        let mut steps = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                if self.at(&TokenKind::Comment) { self.advance(); continue; }
                if self.at(&TokenKind::Step) {
                    steps.push(self.parse_step_def()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }

        Ok(ParBlock {
            span: start_span.merge(self.current().span),
            steps,
        })
    }

    /// Parse an expression with operator precedence.
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        // Handle special statement-level keywords first
        match self.peek_kind().clone() {
            TokenKind::Call => return self.parse_call_expr(),
            TokenKind::Emit => return self.parse_emit_expr(),
            TokenKind::Dispatch => return self.parse_dispatch_expr(),
            TokenKind::Invoke => return self.parse_invoke_expr(),
            TokenKind::Request => return self.parse_request_expr(),
            TokenKind::Guard => return self.parse_guard_expr(),
            TokenKind::Match => return self.parse_match_expr(),
            TokenKind::Ret => {
                self.advance();
                let inner = self.parse_expr()?;
                return Ok(Expr::Return(Box::new(inner)));
            }
            _ => {}
        }

        // Parse with precedence climbing
        let lhs = self.parse_primary()?;

        // Check for assignment: name = expr (only if LHS is a simple ident)
        if self.at(&TokenKind::Eq) {
            if let Expr::Ident(name) = &lhs {
                let name = name.clone();
                self.advance();
                let rhs = self.parse_expr()?;
                return Ok(Expr::Assign(name, Box::new(rhs)));
            }
        }

        // Parse binary operators with precedence
        self.parse_binary_rhs(lhs, 0)
    }

    /// Parse a primary (atomic) expression.
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::Ident => {
                let start_span = self.current().span;
                let name = self.advance().text.clone();
                // Field access / method call
                if self.at(&TokenKind::Dot) {
                    let mut parts = vec![name.clone()];
                    while self.at(&TokenKind::Dot) {
                        self.advance();
                        parts.push(self.expect_ident()?);
                    }
                    if self.at(&TokenKind::LParen) {
                        let method = parts.pop().unwrap_or_default();
                        let target = parts.join(".");
                        let args = self.parse_paren_args();
                        Ok(Expr::Call(CallExpr {
                            target,
                            method,
                            args,
                            span: start_span.merge(self.current().span),
                        }))
                    } else {
                        let mut expr = Expr::Ident(parts[0].clone());
                        for part in &parts[1..] {
                            expr = Expr::FieldAccess(Box::new(expr), part.clone());
                        }
                        Ok(expr)
                    }
                }
                // Direct function call
                else if self.at(&TokenKind::LParen) {
                    let args = self.parse_paren_args();
                    Ok(Expr::Call(CallExpr {
                        target: name,
                        method: String::new(),
                        args,
                        span: start_span.merge(self.current().span),
                    }))
                } else {
                    Ok(Expr::Ident(name))
                }
            }
            TokenKind::StringLit => {
                let text = self.advance().text.clone();
                let inner = text[1..text.len() - 1].to_string();
                Ok(Expr::StringLit(inner))
            }
            TokenKind::IntLit => {
                let text = self.advance().text.clone();
                let val = text.parse::<i64>().unwrap_or(0);
                Ok(Expr::IntLit(val))
            }
            TokenKind::FloatLit => {
                let text = self.advance().text.clone();
                let val = text.parse::<f64>().unwrap_or(0.0);
                Ok(Expr::FloatLit(val))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::BoolLit(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::BoolLit(false))
            }
            TokenKind::Bang => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(Expr::UnaryOp(UnaryOpExpr {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                }))
            }
            TokenKind::Minus => {
                self.advance();
                let expr = self.parse_primary()?;
                Ok(Expr::UnaryOp(UnaryOpExpr {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                }))
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                if self.at(&TokenKind::RParen) { self.advance(); }
                Ok(expr)
            }
            _ => Err(self.error(format!("expected expression, got {:?}", self.peek_kind()))),
        }
    }

    /// Parse binary operators with precedence climbing.
    fn parse_binary_rhs(&mut self, mut lhs: Expr, min_prec: u8) -> Result<Expr, ParseError> {
        loop {
            let prec = self.current_binop_precedence();
            if prec == 0 || prec < min_prec {
                break;
            }

            let op = self.consume_binop().unwrap();
            let mut rhs = self.parse_primary()?;

            // Look ahead for higher-precedence operator
            let next_prec = self.current_binop_precedence();
            if next_prec > prec {
                rhs = self.parse_binary_rhs(rhs, prec + 1)?;
            }

            lhs = Expr::BinaryOp(BinaryOpExpr {
                left: Box::new(lhs),
                op,
                right: Box::new(rhs),
            });
        }
        Ok(lhs)
    }

    /// Get the precedence of the current token if it's a binary operator.
    fn current_binop_precedence(&self) -> u8 {
        match self.peek_kind() {
            TokenKind::Or => 1,
            TokenKind::And => 2,
            TokenKind::EqEq | TokenKind::NotEq => 3,
            TokenKind::LAngle | TokenKind::RAngle | TokenKind::LtEq | TokenKind::GtEq => 4,
            TokenKind::Plus | TokenKind::Minus => 5,
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 6,
            _ => 0, // Not a binary operator
        }
    }

    /// Consume the current token as a binary operator, returning the BinOp.
    fn consume_binop(&mut self) -> Option<BinOp> {
        let op = match self.peek_kind() {
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Mod,
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::NotEq => BinOp::NotEq,
            TokenKind::LAngle => BinOp::Lt,
            TokenKind::RAngle => BinOp::Gt,
            TokenKind::LtEq => BinOp::LtEq,
            TokenKind::GtEq => BinOp::GtEq,
            TokenKind::And => BinOp::And,
            TokenKind::Or => BinOp::Or,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn parse_call_expr(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Call)?;

        let target = self.expect_ident()?;
        let mut method = String::new();

        if self.at(&TokenKind::Dot) {
            self.advance();
            method = self.expect_ident()?;
        }

        // Parse args if there are parens or braces
        let mut args = Vec::new();
        if self.at(&TokenKind::LParen) {
            self.advance();
            while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof)
                && !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Dedent)
            {
                if let Ok(arg) = self.parse_expr() {
                    args.push(arg);
                }
                if self.at(&TokenKind::Comma) {
                    self.advance();
                }
                // Skip nested parens/tokens we can't parse
                if self.at(&TokenKind::LParen) {
                    self.skip_balanced_parens();
                }
            }
            if self.at(&TokenKind::RParen) {
                self.advance();
            }
        } else if self.at(&TokenKind::LBrace) {
            // Struct-like call: Billing.CreateTrial{field: val, ...}
            self.advance();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof)
                && !self.at(&TokenKind::Newline)
            {
                if let Ok(arg) = self.parse_expr() {
                    args.push(arg);
                }
                if self.at(&TokenKind::Comma) { self.advance(); }
                if self.at(&TokenKind::Colon) {
                    self.advance();
                    if let Ok(val) = self.parse_expr() {
                        args.push(val);
                    }
                }
            }
            if self.at(&TokenKind::RBrace) { self.advance(); }
        }

        Ok(Expr::Call(CallExpr {
            target,
            method,
            args,
            span: start_span.merge(self.current().span),
        }))
    }

    fn parse_emit_expr(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Emit)?;
        let event_name = self.expect_ident()?;

        let mut fields = Vec::new();
        if self.at(&TokenKind::LBrace) {
            self.advance();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof)
                && !self.at(&TokenKind::Newline)
            {
                let field_name = self.expect_ident()?;
                let value = if self.at(&TokenKind::Dot) {
                    let mut expr = Expr::Ident(field_name.clone());
                    while self.at(&TokenKind::Dot) {
                        self.advance();
                        let f = self.expect_ident()?;
                        expr = Expr::FieldAccess(Box::new(expr), f);
                    }
                    expr
                } else {
                    Expr::Ident(field_name.clone())
                };
                fields.push((field_name, value));
                if self.at(&TokenKind::Comma) { self.advance(); }
            }
            if self.at(&TokenKind::RBrace) { self.advance(); }
        }

        Ok(Expr::Emit(EmitExpr {
            event_name,
            fields,
            span: start_span.merge(self.current().span),
        }))
    }

    fn parse_dispatch_expr(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Dispatch)?;
        let event_name = self.expect_ident()?;

        let mut fields = Vec::new();
        if self.at(&TokenKind::LBrace) {
            self.advance();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof)
                && !self.at(&TokenKind::Newline)
            {
                let field_name = self.expect_ident()?;
                let value = if self.at(&TokenKind::Dot) {
                    let mut expr = Expr::Ident(field_name.clone());
                    while self.at(&TokenKind::Dot) {
                        self.advance();
                        let f = self.expect_ident()?;
                        expr = Expr::FieldAccess(Box::new(expr), f);
                    }
                    expr
                } else {
                    Expr::Ident(field_name.clone())
                };
                fields.push((field_name, value));
                if self.at(&TokenKind::Comma) { self.advance(); }
            }
            if self.at(&TokenKind::RBrace) { self.advance(); }
        }

        Ok(Expr::Dispatch(DispatchExpr {
            event_name,
            fields,
            span: start_span.merge(self.current().span),
        }))
    }

    fn parse_invoke_expr(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Invoke)?;
        let target = self.expect_ident()?;

        let mut command = String::new();
        if self.at(&TokenKind::Dot) {
            self.advance();
            command = self.expect_ident()?;
        }

        let mut params = Vec::new();
        if self.at(&TokenKind::LBrace) {
            self.advance();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof)
                && !self.at(&TokenKind::Newline)
            {
                let param_name = self.expect_ident()?;
                let value = if self.at(&TokenKind::Colon) {
                    self.advance();
                    self.parse_expr()?
                } else {
                    Expr::Ident(param_name.clone())
                };
                params.push((param_name, value));
                if self.at(&TokenKind::Comma) { self.advance(); }
            }
            if self.at(&TokenKind::RBrace) { self.advance(); }
        } else if self.at(&TokenKind::LParen) {
            let args = self.parse_paren_args();
            for (i, arg) in args.into_iter().enumerate() {
                params.push((format!("arg{}", i), arg));
            }
        }

        Ok(Expr::Invoke(InvokeExpr {
            target,
            command,
            params,
            span: start_span.merge(self.current().span),
        }))
    }

    fn parse_request_expr(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Request)?;
        let port = self.expect_ident()?;

        let mut method = String::new();
        if self.at(&TokenKind::Dot) {
            self.advance();
            method = self.expect_ident()?;
        }

        let args = if self.at(&TokenKind::LParen) {
            self.parse_paren_args()
        } else {
            Vec::new()
        };

        Ok(Expr::Request(RequestExpr {
            port,
            method,
            args,
            span: start_span.merge(self.current().span),
        }))
    }

    fn parse_guard_expr(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Guard)?;

        let condition = self.parse_expr()?;

        let message = if self.at(&TokenKind::Comma) {
            self.advance();
            if self.at(&TokenKind::StringLit) {
                let text = self.advance().text;
                Some(text[1..text.len()-1].to_string())
            } else {
                None
            }
        } else {
            None
        };

        Ok(Expr::Guard(GuardExpr {
            condition: Box::new(condition),
            message,
            span: start_span.merge(self.current().span),
        }))
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        // For now, just skip match blocks (consume them as tokens)
        // Full implementation in Task 4
        self.advance(); // consume 'match'
        // Skip remaining tokens on this line
        while !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Eof)
            && !self.at(&TokenKind::Dedent)
        {
            self.advance();
        }
        // Skip the match body block if present
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() { break; }
                self.advance();
            }
            self.exit_block();
        }
        Ok(Expr::Ident("__match_placeholder".to_string()))
    }

    fn skip_balanced_parens(&mut self) {
        let mut depth = 0;
        if self.at(&TokenKind::LParen) {
            depth = 1;
            self.advance();
        }
        while depth > 0 && !self.at(&TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                self.advance();
            }
        }
    }

    /// Safely parse parenthesized argument list.
    /// Handles nested parens without recursion into parse_expr to avoid infinite loops.
    fn parse_paren_args(&mut self) -> Vec<Expr> {
        if !self.at(&TokenKind::LParen) {
            return Vec::new();
        }
        self.advance(); // consume (

        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Eof)
            && !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Dedent)
        {
            let before = self.pos;
            // Try to parse a simple expression (ident, field access, string, int)
            match self.peek_kind().clone() {
                TokenKind::Ident => {
                    let name = self.advance().text.clone();
                    if self.at(&TokenKind::Dot) {
                        let mut parts = vec![name];
                        while self.at(&TokenKind::Dot) {
                            self.advance();
                            if self.at(&TokenKind::Ident) {
                                parts.push(self.advance().text.clone());
                            } else {
                                break;
                            }
                        }
                        // Check for nested call: obj.method(...)
                        if self.at(&TokenKind::LParen) {
                            let method = parts.pop().unwrap_or_default();
                            let target = parts.join(".");
                            let nested_args = self.parse_paren_args();
                            args.push(Expr::Call(CallExpr {
                                target,
                                method,
                                args: nested_args,
                                span: Span::new(0, 0),
                            }));
                        } else {
                            let mut expr = Expr::Ident(parts[0].clone());
                            for p in &parts[1..] {
                                expr = Expr::FieldAccess(Box::new(expr), p.clone());
                            }
                            args.push(expr);
                        }
                    } else if self.at(&TokenKind::LParen) {
                        // Direct call: func(...)
                        let nested_args = self.parse_paren_args();
                        args.push(Expr::Call(CallExpr {
                            target: name,
                            method: String::new(),
                            args: nested_args,
                            span: Span::new(0, 0),
                        }));
                    } else {
                        args.push(Expr::Ident(name));
                    }
                }
                TokenKind::StringLit => {
                    let text = self.advance().text.clone();
                    let inner = if text.len() >= 2 { text[1..text.len()-1].to_string() } else { text };
                    args.push(Expr::StringLit(inner));
                }
                TokenKind::IntLit => {
                    let text = self.advance().text.clone();
                    args.push(Expr::IntLit(text.parse().unwrap_or(0)));
                }
                _ => {
                    // Skip unknown token to prevent infinite loop
                    self.advance();
                }
            }
            if self.at(&TokenKind::Comma) { self.advance(); }
            // Safety: if we didn't advance, break to prevent infinite loop
            if self.pos == before {
                self.advance();
            }
        }
        if self.at(&TokenKind::RParen) { self.advance(); }
        args
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
