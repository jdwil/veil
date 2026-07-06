//! VEIL Parser — consumes token stream and produces AST.
//!
//! The parser is fully shape-driven: it knows how to parse exactly seven
//! construct shapes (mod, struct, enum, trait, impl, fn, group) and two
//! statement shapes (call, if). Which keyword maps to which shape comes
//! entirely from the `LayerRegistry` — the parser has ZERO domain knowledge.
//!
//! When it sees an identifier in construct position, it looks the word up
//! in the registry and dispatches to the shape's parse function. Named
//! sub-blocks (like `root` or `state` inside an aggregate) are likewise
//! declared by the layer via `contains` entries of the form `keyword: shape`.

use veil_ir::ast::*;
use veil_ir::layer::{LayerRegistry, Shape, StmtShape, StatementSpec};
use veil_ir::span::Span;

use crate::lexer::{Token, TokenKind};

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

/// Parse a token stream into a Solution using only built-in core shapes.
pub fn parse(tokens: &[Token]) -> Result<Solution, Vec<ParseError>> {
    parse_with_registry(tokens, LayerRegistry::builtin())
}

/// Parse a token stream into a Solution with a layer registry.
pub fn parse_with_registry(
    tokens: &[Token],
    registry: LayerRegistry,
) -> Result<Solution, Vec<ParseError>> {
    let mut sol = match parse_file_with_registry(tokens, registry.clone())? {
        VeilFile::Solution(sol) => sol,
        VeilFile::Package(pkg) => Solution {
            name: pkg.name,
            span: pkg.span,
            uses: Vec::new(),
            items: pkg.items,
        },
        VeilFile::Composition(comp) => Solution {
            name: "composition".to_string(),
            span: comp.span,
            uses: comp.imports.clone(),
            items: comp.flows.into_iter().map(TopLevelItem::Flow).collect(),
        },
    };

    // Inject layer declarations (e.g. `port Bus` from ddd.layer's declare section)
    inject_declarations(&mut sol, &registry);

    Ok(sol)
}

/// Parse raw declaration blocks from the layer registry and inject them into the solution.
/// Declarations are parsed using the same registry so they can use layer keywords.
/// Duplicate constructs (by name) are not injected.
fn inject_declarations(sol: &mut Solution, registry: &LayerRegistry) {
    use crate::lexer::lex;

    // Collect existing top-level names (constructs + functions) to avoid dupes.
    let existing_names: Vec<String> = sol.items.iter().filter_map(|item| {
        match item {
            TopLevelItem::Construct(c) => Some(c.name.clone()),
            TopLevelItem::Function(f) => Some(f.name.clone()),
            _ => None,
        }
    }).collect();

    for decl_source in &registry.declarations {
        // Wrap in a minimal solution so the parser can handle it
        let wrapped = format!("sol __decl__\n{}", indent_block(decl_source, 2));
        let tokens = lex(&wrapped);
        if let Ok(decl_sol) = parse_file_with_registry(&tokens, registry.clone()) {
            let items = match decl_sol {
                VeilFile::Solution(s) => s.items,
                _ => continue,
            };
            for mut item in items {
                match &mut item {
                    TopLevelItem::Construct(c) => {
                        if existing_names.contains(&c.name) {
                            continue; // already exists
                        }
                        // Mark provenance so the serializer skips it and the
                        // viewer can distinguish layer-provided infrastructure.
                        c.layer_provided = true;
                    }
                    TopLevelItem::Function(f) => {
                        if existing_names.contains(&f.name) {
                            continue;
                        }
                        f.layer_provided = true;
                    }
                    _ => {}
                }
                sol.items.push(item);
            }
        }
    }
}

/// Indent each line of a block by n spaces.
fn indent_block(text: &str, n: usize) -> String {
    let prefix = " ".repeat(n);
    text.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{}{}", prefix, l) })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse a token stream into a full VeilFile using only built-in core shapes.
pub fn parse_file(tokens: &[Token]) -> Result<VeilFile, Vec<ParseError>> {
    parse_file_with_registry(tokens, LayerRegistry::builtin())
}

/// Parse a token stream into a VeilFile with a layer registry.
pub fn parse_file_with_registry(
    tokens: &[Token],
    registry: LayerRegistry,
) -> Result<VeilFile, Vec<ParseError>> {
    let mut parser = Parser::new(tokens, registry);
    parser.skip_newlines();

    let result = match parser.peek_kind().clone() {
        TokenKind::Pkg => parser.parse_package().map(VeilFile::Package),
        TokenKind::Use => parser.parse_composition().map(VeilFile::Composition),
        _ => parser.parse_solution().map(VeilFile::Solution),
    };

    match result {
        Ok(file) if parser.errors.is_empty() => Ok(file),
        Ok(_) => Err(parser.errors),
        Err(e) => {
            parser.errors.push(e);
            Err(parser.errors)
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    errors: Vec<ParseError>,
    registry: LayerRegistry,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token], registry: LayerRegistry) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            registry,
        }
    }

    // ─── Navigation helpers ───────────────────────────────────────────

    fn current(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or(&self.tokens[self.tokens.len() - 1])
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    /// If the current token is a word (ident or core-construct keyword),
    /// return its text.
    fn current_word(&self) -> Option<&str> {
        match self.peek_kind() {
            TokenKind::Ident
            | TokenKind::Struct
            | TokenKind::Enum
            | TokenKind::Fn
            | TokenKind::Trait
            | TokenKind::Mod
            | TokenKind::Impl
            | TokenKind::Group
            | TokenKind::Flow => Some(self.current().text.as_str()),
            _ => None,
        }
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

    fn error(&self, message: String) -> ParseError {
        ParseError {
            message,
            span: self.current().span,
        }
    }

    /// Check if we're at the start of a block (INDENT follows).
    fn at_block_start(&self) -> bool {
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

    /// Skip an entire indented block without interpreting it.
    fn skip_block(&mut self) -> Result<(), ParseError> {
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at_block_start() {
                    self.skip_block()?;
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }
        Ok(())
    }

    // ─── Type parsing ─────────────────────────────────────────────────

    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        let name = self.expect_ident()?;

        // Res! / Res!<T> syntax
        if self.at(&TokenKind::Bang) {
            self.advance();
            if self.at(&TokenKind::LAngle) {
                self.advance();
                let inner = self.parse_type()?;
                self.expect(&TokenKind::RAngle)?;
                return Ok(TypeExpr::Result(Some(Box::new(inner))));
            }
            return Ok(TypeExpr::Result(None));
        }

        // Generic: Name<T> or Name<T, U>
        if self.at(&TokenKind::LAngle) {
            self.advance();
            let mut args = vec![self.parse_type()?];
            while self.at(&TokenKind::Comma) {
                self.advance();
                args.push(self.parse_type()?);
            }
            self.expect(&TokenKind::RAngle)?;

            return match name.as_str() {
                "Opt" => Ok(TypeExpr::Optional(Box::new(args.into_iter().next().unwrap()))),
                "List" => Ok(TypeExpr::List(Box::new(args.into_iter().next().unwrap()))),
                "Set" => Ok(TypeExpr::Set(Box::new(args.into_iter().next().unwrap()))),
                "Map" if args.len() == 2 => {
                    let mut it = args.into_iter();
                    Ok(TypeExpr::Map(
                        Box::new(it.next().unwrap()),
                        Box::new(it.next().unwrap()),
                    ))
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
            let tok = self.advance();
            let (name, args) = parse_annotation_text(&tok.text);
            annotations.push(Annotation {
                name,
                args,
                span: tok.span,
            });
            self.skip_newlines();
        }
        annotations
    }

    /// Parse a field like "name: Type".
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

    /// Parse fields that may use shorthand: `id email created` on one line,
    /// or full `name: Type` per line.
    fn parse_field_or_shorthand_line(&mut self, fields: &mut Vec<Field>) -> Result<(), ParseError> {
        while self.at(&TokenKind::Ident) {
            let start_span = self.current().span;
            let name = self.advance().text;
            if self.at(&TokenKind::Colon) {
                self.advance();
                let type_expr = self.parse_type()?;
                fields.push(Field {
                    name,
                    type_expr,
                    span: start_span.merge(self.current().span),
                });
            } else {
                // Shorthand: type inferred from name later.
                fields.push(Field {
                    name: name.clone(),
                    type_expr: TypeExpr::Named(name),
                    span: start_span,
                });
            }
        }
        Ok(())
    }
}

/// Parse annotation text like "@async", "@retry(3)", "@env(A, B)".
fn parse_annotation_text(text: &str) -> (String, Vec<String>) {
    let text = text.strip_prefix('@').unwrap_or(text);
    if let Some(paren_idx) = text.find('(') {
        let name = text[..paren_idx].to_string();
        let args_str = &text[paren_idx + 1..text.len() - 1];
        let args: Vec<String> = args_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        (name, args)
    } else {
        (text.to_string(), Vec::new())
    }
}

// ─── Top-level parsing ────────────────────────────────────────────────────

impl<'a> Parser<'a> {
    fn parse_solution(&mut self) -> Result<Solution, ParseError> {
        self.skip_newlines();
        let start_span = self.current().span;
        self.expect(&TokenKind::Sol)?;
        let name = self.expect_ident()?;

        let mut items = Vec::new();
        let mut uses = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                let annotations = self.parse_annotations();
                if self.at_block_end() {
                    break;
                }
                match self.peek_kind().clone() {
                    TokenKind::Use => {
                        // Layer reference — vocabulary already loaded into the
                        // registry by the caller; keep for serialization.
                        uses.push(self.parse_use_import()?);
                    }
                    TokenKind::Allow | TokenKind::Deny => {
                        self.advance();
                        self.skip_block()?;
                    }
                    TokenKind::Lang => {
                        items.push(TopLevelItem::Lang(self.parse_lang_block()?));
                    }
                    TokenKind::Flow => {
                        let mut flow = self.parse_flow()?;
                        let mut all = annotations;
                        all.extend(flow.annotations);
                        flow.annotations = all;
                        items.push(TopLevelItem::Flow(flow));
                    }
                    // Free function with an expression body (e.g. a layer's
                    // declared saga coordinator). `fn name(params) -> T` then body.
                    TokenKind::Fn => {
                        let mut func = self.parse_fn_def()?;
                        let mut all = annotations;
                        all.extend(func.annotations);
                        func.annotations = all;
                        items.push(TopLevelItem::Function(func));
                    }
                    TokenKind::TypeKw => {
                        self.advance(); // consume 'type'
                        let alias_name = self.expect_ident()?;
                        self.expect(&TokenKind::Eq)?;
                        let target = self.parse_type()?;
                        items.push(TopLevelItem::TypeAlias { name: alias_name, target });
                    }
                    TokenKind::ConstKw => {
                        self.advance(); // consume 'const'
                        let const_name = self.expect_ident()?;
                        self.expect(&TokenKind::Eq)?;
                        let value = self.parse_expr()?;
                        items.push(TopLevelItem::Const { name: const_name, value });
                    }
                    _ => {
                        if let Some(c) = self.parse_any_construct(annotations)? {
                            items.push(TopLevelItem::Construct(c));
                        }
                    }
                }
            }
            self.exit_block();
        }

        Ok(Solution {
            name,
            span: start_span.merge(self.current().span),
            uses,
            items,
        })
    }

    fn parse_package(&mut self) -> Result<Package, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Pkg)?;
        let name = self.expect_ident()?;

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
                if self.at_block_end() {
                    break;
                }
                let annotations = self.parse_annotations();
                if self.at_block_end() {
                    break;
                }
                match self.peek_kind().clone() {
                    TokenKind::Ident if self.is_metadata_key() => {
                        let key_span = self.current().span;
                        let key = self.advance().text;
                        let value = if self.at(&TokenKind::StringLit) {
                            let text = self.advance().text;
                            text[1..text.len() - 1].to_string()
                        } else if self.at(&TokenKind::Ident) {
                            self.advance().text
                        } else {
                            String::new()
                        };
                        metadata.push(PackageMeta {
                            key,
                            value,
                            span: key_span,
                        });
                    }
                    TokenKind::Desc => {
                        let key_span = self.current().span;
                        self.advance();
                        let value = if self.at(&TokenKind::StringLit) {
                            let text = self.advance().text;
                            text[1..text.len() - 1].to_string()
                        } else {
                            String::new()
                        };
                        metadata.push(PackageMeta {
                            key: "desc".to_string(),
                            value,
                            span: key_span,
                        });
                    }
                    TokenKind::Use => {
                        let _ = self.parse_use_import()?;
                    }
                    TokenKind::Lang => {
                        items.push(TopLevelItem::Lang(self.parse_lang_block()?));
                    }
                    TokenKind::Expose => {
                        expose = Some(self.parse_expose_block()?);
                    }
                    TokenKind::Flow => {
                        items.push(TopLevelItem::Flow(self.parse_flow()?));
                    }
                    _ => {
                        if let Some(c) = self.parse_any_construct(annotations)? {
                            items.push(TopLevelItem::Construct(c));
                        }
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
        matches!(self.current().text.as_str(), "author" | "license" | "repo")
    }

    fn parse_composition(&mut self) -> Result<Composition, ParseError> {
        let start_span = self.current().span;
        let mut imports = Vec::new();
        let mut flows = Vec::new();

        while self.at(&TokenKind::Use) {
            imports.push(self.parse_use_import()?);
            self.skip_newlines();
        }

        while !self.at(&TokenKind::Eof) {
            self.skip_newlines();
            if self.at(&TokenKind::Eof) {
                break;
            }
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
                let entry_span = self.current().span;
                let term = self.expect_ident()?;
                self.expect(&TokenKind::Colon)?;
                let mut def_parts = Vec::new();
                while !self.at(&TokenKind::Newline)
                    && !self.at(&TokenKind::Eof)
                    && !self.at(&TokenKind::Dedent)
                {
                    def_parts.push(self.advance().text);
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

    fn parse_expose_block(&mut self) -> Result<ExposeBlock, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Expose)?;

        let mut nodes = Vec::new();
        let mut constraints = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                match self.peek_kind().clone() {
                    TokenKind::Node => nodes.push(self.parse_exposed_node()?),
                    TokenKind::Constraints => constraints = self.parse_constraint_lines()?,
                    _ => {
                        self.errors.push(self.error(format!(
                            "unexpected token {:?} in expose block",
                            self.peek_kind()
                        )));
                        self.advance();
                    }
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
                if self.at_block_end() {
                    break;
                }
                match self.peek_kind().clone() {
                    TokenKind::Desc => {
                        self.advance();
                        if self.at(&TokenKind::StringLit) {
                            let text = self.advance().text;
                            description = Some(text[1..text.len() - 1].to_string());
                        }
                    }
                    TokenKind::Input => {
                        self.advance();
                        inputs = self.parse_field_block()?;
                    }
                    TokenKind::Output => {
                        self.advance();
                        outputs = self.parse_field_block()?;
                    }
                    _ => {
                        self.errors.push(self.error(format!(
                            "unexpected token {:?} in exposed node",
                            self.peek_kind()
                        )));
                        self.advance();
                    }
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

    fn parse_constraint_lines(&mut self) -> Result<Vec<String>, ParseError> {
        self.expect(&TokenKind::Constraints)?;
        let mut constraints = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                let mut parts = Vec::new();
                while !self.at(&TokenKind::Newline)
                    && !self.at(&TokenKind::Eof)
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

    /// Parse an indented block of `name: Type` fields.
    fn parse_field_block(&mut self) -> Result<Vec<Field>, ParseError> {
        let mut fields = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
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
}

// ─── Generic construct parsing ────────────────────────────────────────────

impl<'a> Parser<'a> {
    /// Parse whatever construct starts at the current token, dispatching on
    /// its registry shape. Returns None (with a recorded error) for unknown words.
    fn parse_any_construct(
        &mut self,
        annotations: Vec<Annotation>,
    ) -> Result<Option<Construct>, ParseError> {
        // `export` prefix
        let exported = if self.at(&TokenKind::Export) {
            self.advance();
            true
        } else {
            false
        };

        // `group` is a core structural token
        if self.at(&TokenKind::Group) {
            let mut g = self.parse_group_construct()?;
            g.annotations = annotations;
            g.exported = exported;
            return Ok(Some(g));
        }

        let Some(word) = self.current_word().map(|s| s.to_string()) else {
            self.errors.push(self.error(format!(
                "expected a construct keyword, got {:?}",
                self.peek_kind()
            )));
            self.advance();
            return Ok(None);
        };

        let Some(spec) = self.registry.construct(&word).cloned() else {
            self.errors.push(self.error(format!(
                "unknown construct keyword '{}' — not defined by any loaded layer",
                word
            )));
            // Skip the line and any block under it to recover.
            while !self.at(&TokenKind::Newline)
                && !self.at(&TokenKind::Eof)
                && !self.at(&TokenKind::Dedent)
            {
                self.advance();
            }
            self.skip_block()?;
            return Ok(None);
        };

        let mut c = match spec.shape {
            Shape::Mod => self.parse_mod_shape(&spec)?,
            Shape::Struct => self.parse_struct_shape(&spec)?,
            Shape::Enum => self.parse_enum_shape(&spec)?,
            Shape::Trait => self.parse_trait_shape(&spec)?,
            Shape::Impl => self.parse_impl_shape(&spec)?,
            Shape::Fn => self.parse_fn_shape(&spec)?,
            Shape::Group => self.parse_group_construct()?,
        };
        let mut all = annotations;
        all.extend(c.annotations);
        c.annotations = all;
        c.exported = exported;
        Ok(Some(c))
    }


    /// Parse optional generic type parameters: `<A, B, C>` after a construct name.
    fn parse_type_params(&mut self) -> Vec<String> {
        if !self.at(&TokenKind::LAngle) {
            return Vec::new();
        }
        self.advance(); // consume <
        let mut params = Vec::new();
        while !self.at(&TokenKind::RAngle) && !self.at(&TokenKind::Eof) && !self.at(&TokenKind::Newline) {
            if !params.is_empty() && self.at(&TokenKind::Comma) {
                self.advance();
            }
            if let Ok(name) = self.expect_ident() {
                params.push(name);
            } else {
                break;
            }
        }
        if self.at(&TokenKind::RAngle) {
            self.advance();
        }
        params
    }
    /// mod shape: `kw Name` + block of child constructs and groups.
    fn parse_mod_shape(
        &mut self,
        spec: &veil_ir::layer::ConstructSpec,
    ) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.advance(); // keyword
        let name = self.expect_ident()?;
        let mut c = Construct::new(&spec.keyword, &spec.name, Shape::Mod, name, start_span);

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                let annotations = self.parse_annotations();
                if self.at_block_end() {
                    break;
                }
                if let Some(child) = self.parse_any_construct(annotations)? {
                    c.children.push(child);
                }
            }
            self.exit_block();
        }
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    /// group: `group name` + block of child constructs.
    fn parse_group_construct(&mut self) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Group)?;
        let name = self.expect_ident()?;
        let mut c = Construct::new("group", "Group", Shape::Group, name, start_span);

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                let annotations = self.parse_annotations();
                if self.at_block_end() {
                    break;
                }
                if let Some(child) = self.parse_any_construct(annotations)? {
                    c.children.push(child);
                }
            }
            self.exit_block();
        }
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    /// struct shape: `kw Name` + block of fields, named sub-blocks (from the
    /// layer's `contains`), nested constructs, fns, and an optional `-> Type`.
    fn parse_struct_shape(
        &mut self,
        spec: &veil_ir::layer::ConstructSpec,
    ) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.advance(); // keyword
        let name = self.expect_ident()?;
        let mut c = Construct::new(&spec.keyword, &spec.name, Shape::Struct, name, start_span);
        c.type_params = self.parse_type_params();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Annotation) {
                    c.annotations.extend(self.parse_annotations());
                    continue;
                }
                // `-> Type` return type line (e.g. commands)
                if self.at(&TokenKind::Arrow) {
                    self.advance();
                    c.return_type = Some(self.parse_type()?);
                    continue;
                }
                // Nested fn (business logic)
                if self.at(&TokenKind::Fn) {
                    c.fns.push(self.parse_fn_def()?);
                    continue;
                }
                // Named sub-block declared by the layer (`root: struct`, `state: enum`)
                if let Some(word) = self.current_word().map(|s| s.to_string()) {
                    if !self.is_field_line() {
                        if let Some((kw, shape)) = spec
                            .blocks
                            .iter()
                            .find(|(kw, _)| kw == &word)
                            .cloned()
                            .map(|(kw, sh)| (kw, sh))
                        {
                            c.blocks.push(self.parse_named_block(&kw, shape)?);
                            continue;
                        }
                        // Nested construct (e.g. events/commands inside an aggregate)
                        if self.registry.construct(&word).is_some() {
                            let child = self.parse_any_construct(Vec::new())?;
                            if let Some(child) = child {
                                c.children.push(child);
                            }
                            continue;
                        }
                    }
                }
                // Field line(s) — typed (`name: Type`) or shorthand (`id email created`)
                if self.at(&TokenKind::Ident) {
                    self.parse_field_or_shorthand_line(&mut c.fields)?;
                } else {
                    self.errors.push(self.error(format!(
                        "unexpected token {:?} in '{}' body",
                        self.peek_kind(),
                        c.keyword
                    )));
                    self.advance();
                }
            }
            self.exit_block();
        }
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    /// Is the current position a `name: Type` field line (rather than a
    /// nested construct whose keyword shadows a field name)?
    fn is_field_line(&self) -> bool {
        self.tokens
            .get(self.pos + 1)
            .map(|t| t.kind == TokenKind::Colon)
            .unwrap_or(false)
    }

    /// A named sub-block within a struct-shaped construct.
    /// struct-shaped block: fields. enum-shaped block: variants/transitions.
    fn parse_named_block(&mut self, keyword: &str, shape: Shape) -> Result<NamedBlock, ParseError> {
        let start_span = self.current().span;
        self.advance(); // block keyword
        let name = if self.at(&TokenKind::Ident) {
            Some(self.advance().text)
        } else {
            None
        };

        let mut block = NamedBlock {
            keyword: keyword.to_string(),
            shape,
            name,
            fields: Vec::new(),
            variants: Vec::new(),
            transitions: Vec::new(),
            span: start_span,
        };

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                match shape {
                    Shape::Enum => {
                        // Transition chains: A -> B -> C, or bare variants.
                        if self.at(&TokenKind::Ident) {
                            let trans_span = self.current().span;
                            let mut states = vec![self.advance().text];
                            while self.at(&TokenKind::Arrow) {
                                self.advance();
                                if self.at(&TokenKind::Ident) {
                                    states.push(self.advance().text);
                                }
                            }
                            for s in &states {
                                if !block.variants.contains(s) {
                                    block.variants.push(s.clone());
                                }
                            }
                            for i in 0..states.len().saturating_sub(1) {
                                block.transitions.push(StateTransition {
                                    from: states[i].clone(),
                                    to: states[i + 1].clone(),
                                    span: trans_span,
                                });
                            }
                        } else {
                            self.advance();
                        }
                    }
                    _ => {
                        // struct-shaped block: fields
                        if self.at(&TokenKind::Ident) {
                            block.fields.push(self.parse_field()?);
                        } else {
                            self.advance();
                        }
                    }
                }
            }
            self.exit_block();
        }
        block.span = start_span.merge(self.current().span);
        Ok(block)
    }

    /// enum shape: `kw Name` + block of variants / transitions.
    fn parse_enum_shape(
        &mut self,
        spec: &veil_ir::layer::ConstructSpec,
    ) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.advance(); // keyword
        let name = self.expect_ident()?;
        let mut c = Construct::new(&spec.keyword, &spec.name, Shape::Enum, name, start_span);

        c.type_params = self.parse_type_params();
        let block = self.parse_named_block_body_as_enum()?;
        c.variants = block.0;
        c.transitions = block.1;
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    fn parse_named_block_body_as_enum(
        &mut self,
    ) -> Result<(Vec<String>, Vec<StateTransition>), ParseError> {
        let mut variants = Vec::new();
        let mut transitions = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Ident) {
                    let trans_span = self.current().span;
                    let mut states = vec![self.advance().text];
                    while self.at(&TokenKind::Arrow) {
                        self.advance();
                        if self.at(&TokenKind::Ident) {
                            states.push(self.advance().text);
                        }
                    }
                    for s in &states {
                        if !variants.contains(s) {
                            variants.push(s.clone());
                        }
                    }
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
        Ok((variants, transitions))
    }

    /// trait shape: `kw Name` + block of method signatures.
    fn parse_trait_shape(
        &mut self,
        spec: &veil_ir::layer::ConstructSpec,
    ) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.advance(); // keyword
        let name = self.expect_ident()?;
        let mut c = Construct::new(&spec.keyword, &spec.name, Shape::Trait, name, start_span);

        c.type_params = self.parse_type_params();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Ident) {
                    c.methods.push(self.parse_method_signature()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    fn parse_method_signature(&mut self) -> Result<Method, ParseError> {
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

        Ok(Method {
            name,
            span: start_span.merge(self.current().span),
            params,
            return_type,
        })
    }

    /// impl shape: `kw Name for Target` + block of method impls.
    fn parse_impl_shape(
        &mut self,
        spec: &veil_ir::layer::ConstructSpec,
    ) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.advance(); // keyword
        let name = self.expect_ident()?;
        self.expect(&TokenKind::For)?;
        let target = self.expect_ident()?;

        let mut c = Construct::new(&spec.keyword, &spec.name, Shape::Impl, name, start_span);
        c.target = Some(target);

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Annotation) {
                    c.annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at(&TokenKind::Impl) {
                    c.impls.push(self.parse_method_impl()?);
                } else {
                    self.advance();
                }
            }
            self.exit_block();
        }
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    fn parse_method_impl(&mut self) -> Result<MethodImpl, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Impl)?;
        let method_name = self.expect_ident()?;

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

        let mut body = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                match self.parse_expr() {
                    Ok(expr) => body.push(expr),
                    Err(_) => {
                        self.advance();
                    }
                }
            }
            self.exit_block();
        }

        Ok(MethodImpl {
            method_name,
            params,
            span: start_span.merge(self.current().span),
            body,
        })
    }

    /// fn shape: `kw Name` + inputs, steps, par blocks, reference lines, ret.
    fn parse_fn_shape(
        &mut self,
        spec: &veil_ir::layer::ConstructSpec,
    ) -> Result<Construct, ParseError> {
        let start_span = self.current().span;
        self.advance(); // keyword
        let name = self.expect_ident()?;
        let mut c = Construct::new(&spec.keyword, &spec.name, Shape::Fn, name, start_span);

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Annotation) {
                    c.annotations.extend(self.parse_annotations());
                    continue;
                }
                // `step`/`par` are layer flow-vocabulary (ident-keywords),
                // recognized contextually so they don't shadow variable names.
                if self.at_step_header() {
                    c.steps.push(FlowStep::Step(self.parse_step_def()?));
                    continue;
                }
                if self.at_par_header() {
                    c.steps.push(FlowStep::Parallel(self.parse_par_block()?));
                    continue;
                }
                match self.peek_kind().clone() {
                    TokenKind::Input => {
                        self.advance();
                        c.inputs = self.parse_field_block()?;
                    }
                    TokenKind::Ret => {
                        self.advance();
                        if !self.at(&TokenKind::Newline)
                            && !self.at(&TokenKind::Eof)
                            && !self.at(&TokenKind::Dedent)
                        {
                            c.return_expr = Some(Box::new(self.parse_expr()?));
                        }
                    }
                    // Reference line: `keyword Name, Name` (e.g. `contexts Identity, Billing`).
                    TokenKind::Ident => {
                        let ref_span = self.current().span;
                        let keyword = self.advance().text;
                        let mut values = Vec::new();
                        while !self.at(&TokenKind::Newline)
                            && !self.at(&TokenKind::Eof)
                            && !self.at(&TokenKind::Dedent)
                        {
                            if self.at(&TokenKind::Ident) {
                                values.push(self.advance().text);
                            } else if self.at(&TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        c.refs.push(RefLine {
                            keyword,
                            values,
                            span: ref_span.merge(self.current().span),
                        });
                    }
                    _ => {
                        self.errors.push(self.error(format!(
                            "unexpected token {:?} in '{}' body",
                            self.peek_kind(),
                            c.keyword
                        )));
                        self.advance();
                    }
                }
            }
            self.exit_block();
        }
        c.span = start_span.merge(self.current().span);
        Ok(c)
    }

    /// A nested `fn name(params) -> Type` definition with an expression body.
    fn parse_fn_def(&mut self) -> Result<FnDef, ParseError> {
        let start_span = self.current().span;
        let annotations = self.parse_annotations();
        self.expect(&TokenKind::Fn)?;
        let name = self.expect_ident()?;

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
                if self.at(&TokenKind::Comma) {
                    self.advance();
                }
            }
            if self.at(&TokenKind::RParen) {
                self.advance();
            }
        }

        let mut return_type = None;
        if self.at(&TokenKind::Arrow) {
            self.advance();
            return_type = Some(self.parse_type()?);
        }

        let mut body = Vec::new();
        let mut fn_annotations = annotations;
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Annotation) {
                    fn_annotations.extend(self.parse_annotations());
                    continue;
                }
                match self.parse_expr() {
                    Ok(expr) => body.push(expr),
                    Err(_) => {
                        self.advance();
                    }
                }
            }
            self.exit_block();
        }

        Ok(FnDef {
            name,
            span: start_span.merge(self.current().span),
            params,
            return_type,
            annotations: fn_annotations,
            body,
            layer_provided: false,
        })
    }
}

// ─── Flow parsing (core language) ─────────────────────────────────────────

impl<'a> Parser<'a> {
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
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
                if self.at_step_header() {
                    steps.push(FlowStep::Step(self.parse_step_def()?));
                    continue;
                }
                if self.at_par_header() {
                    steps.push(FlowStep::Parallel(self.parse_par_block()?));
                    continue;
                }
                match self.peek_kind().clone() {
                    TokenKind::Input => {
                        self.advance();
                        inputs = self.parse_field_block()?;
                    }
                    TokenKind::Err => {
                        error_boundary = Some(self.parse_error_boundary()?);
                    }
                    TokenKind::Ret => {
                        self.advance();
                        if !self.at(&TokenKind::Newline)
                            && !self.at(&TokenKind::Eof)
                            && !self.at(&TokenKind::Dedent)
                        {
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

    fn parse_error_boundary(&mut self) -> Result<ErrorBoundary, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Err)?;
        if self.at(&TokenKind::Boundary) {
            self.advance();
        }

        let mut annotations = Vec::new();
        let mut fallback = None;

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at(&TokenKind::Annotation) {
                    annotations.extend(self.parse_annotations());
                    continue;
                }
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
        self.advance(); // `step` ident-keyword
        let name = self.expect_ident()?;

        let mut body = Vec::new();
        let mut refs = Vec::new();
        let mut sub_blocks = Vec::new();

        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                // Reference line: an ident-keyword followed by a bare Name and
                // then end-of-line (e.g. `ctx Identity`). Distinguished from
                // expressions by the absence of call/assign syntax.
                if self.is_ref_line() {
                    let ref_span = self.current().span;
                    let keyword = self.advance().text;
                    let mut values = Vec::new();
                    while !self.at(&TokenKind::Newline)
                        && !self.at(&TokenKind::Eof)
                        && !self.at(&TokenKind::Dedent)
                    {
                        if self.at(&TokenKind::Ident) {
                            values.push(self.advance().text);
                        } else if self.at(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    refs.push(RefLine {
                        keyword,
                        values,
                        span: ref_span.merge(self.current().span),
                    });
                    continue;
                }
                // Named expression sub-block: an ident-keyword alone on its
                // line with an indented block under it (e.g. `compensate`).
                if self.is_sub_block_header() {
                    let sb_span = self.current().span;
                    let keyword = self.advance().text;
                    let mut sb_body = Vec::new();
                    if self.at_block_start() {
                        self.enter_block()?;
                        while !self.at_block_end() {
                            self.skip_newlines();
                            if self.at_block_end() {
                                break;
                            }
                            match self.parse_expr() {
                                Ok(expr) => sb_body.push(expr),
                                Err(_) => {
                                    self.advance();
                                }
                            }
                        }
                        self.exit_block();
                    }
                    sub_blocks.push(SubBlock {
                        keyword,
                        body: sb_body,
                        span: sb_span.merge(self.current().span),
                    });
                    continue;
                }
                match self.parse_expr() {
                    Ok(expr) => body.push(expr),
                    Err(_) => {
                        self.advance();
                    }
                }
            }
            self.exit_block();
        }

        Ok(StepDef {
            name,
            span: start_span.merge(self.current().span),
            body,
            refs,
            sub_blocks,
        })
    }

    /// `ctx Identity` — an ident that is a registry mod-shaped keyword,
    /// followed by a bare ident and end of line.
    fn is_ref_line(&self) -> bool {
        let Some(word) = self.current_word() else {
            return false;
        };
        // Must be a known mod-shaped construct keyword to avoid eating exprs.
        let Some(spec) = self.registry.construct(word) else {
            return false;
        };
        if spec.shape != Shape::Mod {
            return false;
        }
        // Next token is a bare ident, then newline/dedent.
        let next = self.tokens.get(self.pos + 1);
        let after = self.tokens.get(self.pos + 2);
        matches!(next.map(|t| &t.kind), Some(TokenKind::Ident))
            && matches!(
                after.map(|t| &t.kind),
                Some(TokenKind::Newline)
                    | Some(TokenKind::Dedent)
                    | Some(TokenKind::Eof)
                    | Some(TokenKind::Comma)
            )
    }

    /// An ident alone on its line, with an indented block following, that is
    /// NOT a layer statement keyword — e.g. `compensate`.
    fn is_sub_block_header(&self) -> bool {
        if !self.at(&TokenKind::Ident) {
            return false;
        }
        let word = &self.current().text;
        if self.registry.statement(word).is_some() {
            return false;
        }
        let next = self.tokens.get(self.pos + 1);
        matches!(
            next.map(|t| &t.kind),
            Some(TokenKind::Newline) | Some(TokenKind::Indent)
        )
    }

    /// `step <name>` — the flow-step header. `step`/`par` are layer vocabulary
    /// (lexed as idents), recognized here by word + shape so users can still
    /// name variables `step`. A step header is `step <Ident>`; anything else
    /// (e.g. `step = 1`) is an ordinary expression.
    fn at_step_header(&self) -> bool {
        self.current_word() == Some("step")
            && matches!(self.tokens.get(self.pos + 1).map(|t| &t.kind), Some(TokenKind::Ident))
    }

    /// `par` alone on its line, opening a parallel block.
    fn at_par_header(&self) -> bool {
        self.current_word() == Some("par")
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Newline) | Some(TokenKind::Indent)
            )
    }

    fn parse_par_block(&mut self) -> Result<ParBlock, ParseError> {
        let start_span = self.current().span;
        // `par` is now an ident-keyword, not a reserved token.
        self.advance();

        let mut steps = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            while !self.at_block_end() {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                if self.at_step_header() {
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
}

// ─── Expression parsing ───────────────────────────────────────────────────

impl<'a> Parser<'a> {
    /// Parse an expression with operator precedence. Layer statements
    /// (registry lookup on the leading ident) are parsed by statement shape.
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::Call => return self.parse_call_stmt(),
            TokenKind::Match => return self.parse_match_expr(),
            TokenKind::For => return self.parse_for_loop(),
            TokenKind::While => return self.parse_while_loop(),
            TokenKind::Mut => {
                self.advance(); // consume 'mut'
                let name = self.expect_ident()?;
                self.expect(&TokenKind::Eq)?;
                let rhs = self.parse_expr()?;
                return Ok(Expr::MutAssign(name, Box::new(rhs)));
            }
            TokenKind::Await => {
                self.advance(); // consume 'await'
                let inner = self.parse_expr()?;
                return Ok(Expr::Await(Box::new(inner)));
            }
            TokenKind::Ret => {
                self.advance();
                let inner = self.parse_expr()?;
                return Ok(Expr::Return(Box::new(inner)));
            }
            TokenKind::Ident => {
                // Layer-defined statement?
                let word = self.current().text.clone();
                if let Some(stmt) = self.registry.statement(&word).cloned() {
                    return self.parse_action(&word, &stmt);
                }
            }
            _ => {}
        }

        let lhs = self.parse_primary()?;

        // Assignment: name = expr (only if LHS is a simple ident)
        if self.at(&TokenKind::Eq) {
            if let Expr::Ident(name) = &lhs {
                let name = name.clone();
                self.advance();
                let rhs = self.parse_expr()?;
                return Ok(Expr::Assign(name, Box::new(rhs)));
            }
        }

        self.parse_binary_rhs(lhs, 0)
    }

    /// Parse a layer statement according to its core shape.
    fn parse_action(&mut self, keyword: &str, stmt_spec: &StatementSpec) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        let shape = stmt_spec.shape;
        self.advance(); // statement keyword

        let mut action = ActionExpr {
            keyword: keyword.to_string(),
            shape,
            target: String::new(),
            method: String::new(),
            args: Vec::new(),
            named_args: Vec::new(),
            condition: None,
            message: None,
            span: start_span,
        };

        match shape {
            StmtShape::Call => {
                action.target = self.expect_ident()?;
                if self.at(&TokenKind::Dot) {
                    self.advance();
                    action.method = self.expect_ident()?;
                }
                if self.at(&TokenKind::LParen) {
                    action.args = self.parse_paren_args();
                } else if self.at(&TokenKind::LBrace) {
                    action.named_args = self.parse_brace_args()?;
                }
            }
            StmtShape::If => {
                let condition = self.parse_condition_expr()?;
                action.condition = Some(Box::new(condition));
                if self.at(&TokenKind::Comma) {
                    self.advance();
                    if self.at(&TokenKind::StringLit) {
                        let text = self.advance().text;
                        action.message = Some(text[1..text.len() - 1].to_string());
                    }
                }
            }
        }

        action.span = start_span.merge(self.current().span);

        // Desugar port-targeted statements into Expr::Call
        if let (Some(port_target), Some(port_method)) = (&stmt_spec.port_target, &stmt_spec.port_method) {
            // Build the argument: if named_args present, it's a StructLit; else positional
            let call_arg = if !action.named_args.is_empty() {
                // dispatch Evt{field: val} → Bus.dispatch(Evt{field: val})
                let fields: Vec<(String, Expr)> = action.named_args;
                vec![Expr::StructLit(action.target.clone(), fields)]
            } else if !action.args.is_empty() {
                // dispatch Target.method(args) → Bus.dispatch(args) — rare case
                action.args
            } else {
                // dispatch Evt → Bus.dispatch(Evt)  (bare identifier as arg)
                vec![Expr::Ident(action.target.clone())]
            };
            return Ok(Expr::Call(CallExpr {
                target: port_target.clone(),
                method: port_method.clone(),
                args: call_arg,
                receiver: None,
                sugar: Some(keyword.to_string()),
                span: action.span,
            }));
        }

        Ok(Expr::Action(action))
    }

    /// Parse a condition expression (no assignment, stops before comma).
    fn parse_condition_expr(&mut self) -> Result<Expr, ParseError> {
        // A condition may itself start with a layer call statement:
        //   guard call Email.validate(email), "invalid"
        if self.at(&TokenKind::Call) {
            let call = self.parse_call_stmt()?;
            return self.parse_binary_rhs(call, 0);
        }
        if self.at(&TokenKind::Ident) {
            let word = self.current().text.clone();
            if let Some(stmt) = self.registry.statement(&word).cloned() {
                if stmt.shape == StmtShape::Call {
                    let inner = self.parse_action(&word, &stmt)?;
                    return self.parse_binary_rhs(inner, 0);
                }
            }
        }
        let lhs = self.parse_primary()?;
        self.parse_binary_rhs(lhs, 0)
    }

    /// `{name: expr, name, ...}` — named argument block.
    fn parse_brace_args(&mut self) -> Result<Vec<(String, Expr)>, ParseError> {
        let mut named = Vec::new();
        self.expect(&TokenKind::LBrace)?;
        while !self.at(&TokenKind::RBrace)
            && !self.at(&TokenKind::Eof)
            && !self.at(&TokenKind::Newline)
        {
            let name = self.expect_ident()?;
            let value = if self.at(&TokenKind::Colon) {
                self.advance();
                self.parse_expr()?
            } else if self.at(&TokenKind::Dot) {
                // Shorthand field access: {c.id} means key="id", value=c.id
                let mut expr = Expr::Ident(name.clone());
                let mut leaf_name = name.clone();
                while self.at(&TokenKind::Dot) {
                    self.advance();
                    let f = self.expect_ident()?;
                    leaf_name = f.clone();
                    expr = Expr::FieldAccess(Box::new(expr), f);
                }
                // Use the leaf field name as the key, not the root variable
                named.push((leaf_name, expr));
                if self.at(&TokenKind::Comma) {
                    self.advance();
                }
                continue;
            } else if self.at(&TokenKind::LParen) {
                // Shorthand call value: {id, now()}
                let args = self.parse_paren_args();
                Expr::Call(CallExpr {
                    target: name.clone(),
                    method: String::new(),
                    args,
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 0),
                })
            } else {
                Expr::Ident(name.clone())
            };
            named.push((name, value));
            if self.at(&TokenKind::Comma) {
                self.advance();
            }
        }
        if self.at(&TokenKind::RBrace) {
            self.advance();
        }
        Ok(named)
    }

    /// The core `call` statement.
    fn parse_call_stmt(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.current().span;
        self.expect(&TokenKind::Call)?;

        let target = self.expect_ident()?;
        let mut method = String::new();

        if self.at(&TokenKind::Dot) {
            self.advance();
            method = self.expect_ident()?;
        }

        let mut args = Vec::new();
        if self.at(&TokenKind::LParen) {
            args = self.parse_paren_args();
        } else if self.at(&TokenKind::LBrace) {
            let named = self.parse_brace_args()?;
            args = named.into_iter().map(|(_, v)| v).collect();
        }

        let call = Expr::Call(CallExpr {
            target,
            method,
            args,
            receiver: None,
            sugar: None,
            span: start_span.merge(self.current().span),
        });
        // Continue any method chain: `call a.b(i).c(x)` → the `.c(x)` attaches
        // via receiver form.
        self.parse_postfix(call, start_span)
    }

    /// Parse a primary (atomic) expression.
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::Pipe => {
                // Closure: |params| body
                self.advance(); // consume opening |
                let mut params = Vec::new();
                while !self.at(&TokenKind::Pipe) && !self.at(&TokenKind::Eof) {
                    if !params.is_empty() && self.at(&TokenKind::Comma) {
                        self.advance();
                    }
                    params.push(self.expect_ident()?);
                }
                if self.at(&TokenKind::Pipe) {
                    self.advance(); // consume closing |
                }
                // Parse body: single expression on same line, or indented block
                let mut body = Vec::new();
                if self.at_block_start() {
                    let _ = self.enter_block();
                    loop {
                        self.skip_newlines();
                        if self.at_block_end() { break; }
                        body.push(self.parse_expr()?);
                    }
                    self.exit_block();
                } else if !self.at(&TokenKind::Newline) && !self.at(&TokenKind::Eof)
                    && !self.at(&TokenKind::RParen) && !self.at(&TokenKind::Comma)
                {
                    body.push(self.parse_expr()?);
                }
                return Ok(Expr::Closure { params, body });
            }
            TokenKind::Ident => {
                let start_span = self.current().span;
                let name = self.advance().text;
                let atom = if self.at(&TokenKind::LParen) {
                    // Bare function call: `name(args)`.
                    let args = self.parse_paren_args();
                    Expr::Call(CallExpr {
                        target: name,
                        method: String::new(),
                        args,
                        receiver: None,
                        sugar: None,
                        span: start_span.merge(self.current().span),
                    })
                } else {
                    Expr::Ident(name)
                };
                self.parse_postfix(atom, start_span)
            }
            TokenKind::StringLit => {
                let text = self.advance().text;
                Ok(Expr::StringLit(text[1..text.len() - 1].to_string()))
            }
            TokenKind::IntLit => {
                let text = self.advance().text;
                Ok(Expr::IntLit(text.parse::<i64>().unwrap_or(0)))
            }
            TokenKind::FloatLit => {
                let text = self.advance().text;
                Ok(Expr::FloatLit(text.parse::<f64>().unwrap_or(0.0)))
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
                let start_span = self.current().span;
                self.advance();
                let expr = self.parse_expr()?;
                if self.at(&TokenKind::RParen) {
                    self.advance();
                }
                // A parenthesized expression can also be the head of a chain.
                self.parse_postfix(expr, start_span)
            }
            TokenKind::LBrace => {
                // Anonymous record/map literal: `{ key: value, key, ... }`.
                // Represented as a StructLit with an empty type name.
                let fields = self.parse_brace_args()?;
                Ok(Expr::StructLit(String::new(), fields))
            }
            _ => Err(self.error(format!("expected expression, got {:?}", self.peek_kind()))),
        }
    }

    /// Consume trailing postfix operators after an atom: `.field`, `.method(args)`,
    /// and chained calls like `items.map(f).filter(g).collect()`.
    ///
    /// The first `.name` after a bare identifier atom collapses into the
    /// `CallExpr.target`/`FieldAccess` form (so `Repo.find(id)` keeps its named
    /// target for shape-driven codegen); subsequent links attach via
    /// `CallExpr.receiver`, threading the accumulated expression as the receiver.
    fn parse_postfix(&mut self, mut expr: Expr, start_span: Span) -> Result<Expr, ParseError> {
        while self.at(&TokenKind::Dot) {
            self.advance(); // consume '.'
            let field = self.expect_ident()?;
            if self.at(&TokenKind::LParen) {
                let args = self.parse_paren_args();
                let span = start_span.merge(self.current().span);
                // `Ident.method(args)` (no prior chaining) → named target form,
                // preserving `Repo.find(id)` for the codegen name resolver.
                expr = match expr {
                    Expr::Ident(target) => Expr::Call(CallExpr {
                        target,
                        method: field,
                        args,
                        receiver: None,
                        sugar: None,
                        span,
                    }),
                    Expr::FieldAccess(base, last) => {
                        // `a.b.method(args)` → target "a.b" (dotted path).
                        let target = flatten_dotted_path(&base, &last);
                        match target {
                            Some(t) => Expr::Call(CallExpr {
                                target: t,
                                method: field,
                                args,
                                receiver: None,
                                sugar: None,
                                span,
                            }),
                            None => Expr::Call(CallExpr {
                                target: String::new(),
                                method: field,
                                args,
                                receiver: Some(Box::new(Expr::FieldAccess(base, last))),
                                sugar: None,
                                span,
                            }),
                        }
                    }
                    // Chain link on a call/other expression → receiver form.
                    other => Expr::Call(CallExpr {
                        target: String::new(),
                        method: field,
                        args,
                        receiver: Some(Box::new(other)),
                        sugar: None,
                        span,
                    }),
                };
            } else {
                // Plain field access.
                expr = Expr::FieldAccess(Box::new(expr), field);
            }
        }
        Ok(expr)
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

    fn current_binop_precedence(&self) -> u8 {
        match self.peek_kind() {
            TokenKind::Or => 1,
            TokenKind::And => 2,
            TokenKind::EqEq | TokenKind::NotEq => 3,
            TokenKind::LAngle | TokenKind::RAngle | TokenKind::LtEq | TokenKind::GtEq => 4,
            TokenKind::Plus | TokenKind::Minus => 5,
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 6,
            _ => 0,
        }
    }

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

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        self.advance(); // consume 'match'

        // Parse the scrutinee expression (up to newline)
        let scrutinee = self.parse_expr()?;

        // Parse arms in indented block
        let mut arms = Vec::new();
        if self.at_block_start() {
            self.enter_block()?;
            loop {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                let arm_span = self.current().span;

                // Parse pattern: everything up to `->`
                let mut pattern_parts = Vec::new();
                while !self.at(&TokenKind::Arrow)
                    && !self.at(&TokenKind::Newline)
                    && !self.at(&TokenKind::Eof)
                    && !self.at(&TokenKind::Dedent)
                {
                    pattern_parts.push(self.advance().text);
                }
                let pattern = pattern_parts.join(" ");

                if !self.at(&TokenKind::Arrow) {
                    // Skip malformed arm
                    continue;
                }
                self.advance(); // consume ->

                // Parse body: either a single expression on the same line, or an indented block
                let mut body = Vec::new();
                if self.at(&TokenKind::Newline) || self.at_block_start() {
                    // Multi-line body
                    if self.at_block_start() {
                        self.enter_block()?;
                        loop {
                            self.skip_newlines();
                            if self.at_block_end() {
                                break;
                            }
                            body.push(self.parse_expr()?);
                        }
                        self.exit_block();
                    }
                } else {
                    // Single expression on same line
                    body.push(self.parse_expr()?);
                }

                arms.push(MatchArm {
                    pattern,
                    span: arm_span.merge(self.current().span),
                    body,
                });
            }
            self.exit_block();
        }

        Ok(Expr::Match(Box::new(scrutinee), arms))
    }

    /// Safely parse parenthesized argument list.
    /// Parse a for loop: `for <binding> in <expr>` with indented body.
    fn parse_for_loop(&mut self) -> Result<Expr, ParseError> {
        self.advance(); // consume 'for'

        // Parse binding (and optional index): `for i, item in ...` or `for item in ...`
        let first = self.expect_ident()?;
        let (index, binding) = if self.at(&TokenKind::Comma) {
            self.advance();
            let second = self.expect_ident()?;
            (Some(first), second)
        } else {
            (None, first)
        };

        // Expect 'in' (which is just an ident)
        let word = self.current_word();
        if word == Some("in") {
            self.advance();
        } else {
            return Err(self.error("expected 'in' after for binding".to_string()));
        }

        // Parse iterable expression
        let iterable = self.parse_expr()?;

        // Parse body block
        let mut body = Vec::new();
        if self.at_block_start() {
            let _ = self.enter_block();
            loop {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                body.push(self.parse_expr()?);
            }
            self.exit_block();
        }

        Ok(Expr::ForLoop { binding, index, iterable: Box::new(iterable), body })
    }

    /// Parse a while loop: `while <condition>` with indented body.
    fn parse_while_loop(&mut self) -> Result<Expr, ParseError> {
        self.advance(); // consume 'while'

        // Parse condition expression (everything on the same line)
        let condition = self.parse_expr()?;

        // Parse body block
        let mut body = Vec::new();
        if self.at_block_start() {
            let _ = self.enter_block();
            loop {
                self.skip_newlines();
                if self.at_block_end() {
                    break;
                }
                body.push(self.parse_expr()?);
            }
            self.exit_block();
        }

        Ok(Expr::WhileLoop { condition: Box::new(condition), body })
    }

    fn parse_paren_args(&mut self) -> Vec<Expr> {
        if !self.at(&TokenKind::LParen) {
            return Vec::new();
        }
        self.advance(); // consume (

        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen)
            && !self.at(&TokenKind::Eof)
            && !self.at(&TokenKind::Newline)
            && !self.at(&TokenKind::Dedent)
        {
            let before = self.pos;
            // Each argument is a full expression (handles literals, binary ops,
            // closures, and nested/chained calls). Falling back to advance() on
            // error prevents an infinite loop without silently dropping tokens
            // from a well-formed argument.
            match self.parse_expr() {
                Ok(expr) => args.push(expr),
                Err(_) => {
                    if self.pos == before {
                        self.advance();
                    }
                }
            }
            if self.at(&TokenKind::Comma) {
                self.advance();
            }
            if self.pos == before {
                // No progress made (e.g. an unexpected token parse_expr couldn't
                // consume) — advance to guarantee termination.
                self.advance();
            }
        }
        if self.at(&TokenKind::RParen) {
            self.advance();
        }
        args
    }
}

/// Flatten a `FieldAccess` chain rooted at a plain identifier into a dotted
/// path string (`a.b.c`). Returns `None` if the base is not a simple ident
/// chain (e.g. it contains a call), in which case the receiver form is used.
fn flatten_dotted_path(base: &Expr, last: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut cur = base;
    loop {
        match cur {
            Expr::Ident(n) => {
                parts.push(n.clone());
                break;
            }
            Expr::FieldAccess(inner, field) => {
                parts.push(field.clone());
                cur = inner;
            }
            _ => return None,
        }
    }
    parts.reverse();
    parts.push(last.to_string());
    Some(parts.join("."))
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
