#[cfg(test)]
mod tests {
    use crate::lexer::{lex, TokenKind};

    /// Helper to extract just the kinds from a token stream (excluding Eof).
    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| t.kind)
            .collect()
    }

    /// Helper to extract kinds + text pairs (excluding structural tokens).
    fn tokens_text(src: &str) -> Vec<(TokenKind, String)> {
        lex(src)
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Eof | TokenKind::Newline))
            .map(|t| (t.kind, t.text.clone()))
            .collect()
    }

    #[test]
    fn test_basic_keywords() {
        let tokens = lex("sol MyApp");
        assert_eq!(tokens[0].kind, TokenKind::Sol);
        assert_eq!(tokens[0].text, "sol");
        assert_eq!(tokens[1].kind, TokenKind::Ident);
        assert_eq!(tokens[1].text, "MyApp");
    }

    #[test]
    fn test_layer_vocabulary_lexes_as_ident() {
        // The lexer knows NOTHING about domain vocabulary — every layer
        // keyword is a plain identifier until the parser consults the registry.
        let src = "ctx agg ent val evt cmd port adapter svc saga orchestrator dispatch invoke request guard emit compensate contexts root state pipeline lead";
        let k = kinds(src);
        assert!(
            k.iter().all(|t| *t == TokenKind::Ident),
            "expected all Ident, got {:?}",
            k
        );
    }

    #[test]
    fn test_ellipsis_spread_token() {
        // `...` must lex as a single Ellipsis token (longest match).
        let k = kinds("...");
        assert_eq!(k, vec![TokenKind::Ellipsis]);
    }

    #[test]
    fn test_ellipsis_before_range_operators_no_regression() {
        // Longest-match: `...` wins over `..`/`..=` but ranges still lex correctly.
        assert_eq!(kinds(".."), vec![TokenKind::DotDot]);
        assert_eq!(kinds("..="), vec![TokenKind::DotDotEq]);
        // `0..10` is a range; `...items` is a spread.
        assert_eq!(
            kinds("0..10"),
            vec![TokenKind::IntLit, TokenKind::DotDot, TokenKind::IntLit]
        );
        assert_eq!(
            kinds("0..=10"),
            vec![TokenKind::IntLit, TokenKind::DotDotEq, TokenKind::IntLit]
        );
    }

    #[test]
    fn test_spread_in_array_context() {
        // `[...items, x]` → LBracket Ellipsis Ident Comma Ident RBracket
        let k = kinds("[...items, x]");
        assert_eq!(
            k,
            vec![
                TokenKind::LBracket,
                TokenKind::Ellipsis,
                TokenKind::Ident,
                TokenKind::Comma,
                TokenKind::Ident,
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn test_four_dots_lexes_ellipsis_then_dot() {
        // `....` → Ellipsis (`...`) followed by a single Dot.
        assert_eq!(kinds("...."), vec![TokenKind::Ellipsis, TokenKind::Dot]);
    }

    #[test]
    fn test_core_structure_keywords() {
        // `step`/`par` are NOT core tokens — they are layer flow vocabulary and
        // lex as identifiers so they can be used as variable names.
        let src = "sol pkg use link adapt ins rfn rpl omit ren stock lang expose node flow step par err call input group export";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Sol,
                TokenKind::Pkg,
                TokenKind::Use,
                TokenKind::Link,
                TokenKind::Adapt,
                TokenKind::Ins,
                TokenKind::Rfn,
                TokenKind::Rpl,
                TokenKind::Omit,
                TokenKind::Ren,
                TokenKind::Stock,
                TokenKind::Lang,
                TokenKind::Expose,
                TokenKind::Node,
                TokenKind::Flow,
                TokenKind::Ident, // step
                TokenKind::Ident, // par
                TokenKind::Err,
                TokenKind::Call,
                TokenKind::Input,
                TokenKind::Group,
                TokenKind::Export,
            ]
        );
    }

    #[test]
    fn test_core_language_keywords() {
        let src = "struct enum fn trait let mod if else match ret impl";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Struct,
                TokenKind::Enum,
                TokenKind::Fn,
                TokenKind::Trait,
                TokenKind::Let,
                TokenKind::Mod,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::Match,
                TokenKind::Ret,
                TokenKind::Impl,
            ]
        );
    }

    #[test]
    fn test_operators() {
        let src = "-> => || : . , = != ! ( ) < > { }";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Arrow, TokenKind::FatArrow, TokenKind::Or,
                TokenKind::Colon, TokenKind::Dot, TokenKind::Comma,
                TokenKind::Eq, TokenKind::NotEq, TokenKind::Bang,
                TokenKind::LParen, TokenKind::RParen,
                TokenKind::LAngle, TokenKind::RAngle,
                TokenKind::LBrace, TokenKind::RBrace,
            ]
        );
    }

    #[test]
    fn test_indentation_simple() {
        let src = "sol App\n  ctx Users\n    agg User";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Sol, TokenKind::Ident, TokenKind::Newline,
                TokenKind::Indent, TokenKind::Ident, TokenKind::Ident, TokenKind::Newline,
                TokenKind::Indent, TokenKind::Ident, TokenKind::Ident,
                TokenKind::Dedent, TokenKind::Dedent,
            ]
        );
    }

    #[test]
    fn test_indent_dedent_multiple() {
        let src = "sol App\n  ctx A\n    agg B\n  ctx C";
        let k = kinds(src);
        assert!(k.contains(&TokenKind::Indent));
        assert!(k.contains(&TokenKind::Dedent));
        let indent_count = k.iter().filter(|t| **t == TokenKind::Indent).count();
        let dedent_count = k.iter().filter(|t| **t == TokenKind::Dedent).count();
        assert_eq!(indent_count, 2);
        assert_eq!(dedent_count, 2);
    }

    #[test]
    fn test_annotation_simple() {
        let tokens = lex("@async");
        assert_eq!(tokens[0].kind, TokenKind::Annotation);
        assert_eq!(tokens[0].text, "@async");
    }

    #[test]
    fn test_annotation_with_parens() {
        let tokens = lex("@retry(3)");
        assert_eq!(tokens[0].kind, TokenKind::Annotation);
        assert_eq!(tokens[0].text, "@retry(3)");
    }

    #[test]
    fn test_annotation_with_parens_args() {
        let tokens = lex("@env(TWILIO_SID, TWILIO_TOKEN)");
        assert_eq!(tokens[0].kind, TokenKind::Annotation);
        assert_eq!(tokens[0].text, "@env(TWILIO_SID, TWILIO_TOKEN)");
    }

    #[test]
    fn test_annotation_stops_at_keyword() {
        let src = "@retry 3\nstep foo";
        let k = kinds(src);
        assert_eq!(k[0], TokenKind::Annotation);
        // `step` now lexes as an identifier (layer vocabulary, not a core token).
        assert!(k.contains(&TokenKind::Ident));
    }

    #[test]
    fn test_string_literal() {
        let tokens = lex("\"hello world\"");
        assert_eq!(tokens[0].kind, TokenKind::StringLit);
        assert_eq!(tokens[0].text, "\"hello world\"");
    }

    #[test]
    fn test_string_with_escape() {
        let tokens = lex("\"hello \\\"world\\\"\"");
        assert_eq!(tokens[0].kind, TokenKind::StringLit);
    }

    #[test]
    fn test_integer_literal() {
        let tokens = lex("42");
        assert_eq!(tokens[0].kind, TokenKind::IntLit);
        assert_eq!(tokens[0].text, "42");
    }

    #[test]
    fn test_float_literal() {
        let tokens = lex("3.14");
        assert_eq!(tokens[0].kind, TokenKind::FloatLit);
        assert_eq!(tokens[0].text, "3.14");
    }

    #[test]
    fn test_dot_not_float() {
        let k = kinds("c.id");
        assert_eq!(k, vec![TokenKind::Ident, TokenKind::Dot, TokenKind::Ident]);
    }

    #[test]
    fn test_result_type_syntax() {
        let k = kinds("Res!<Customer>");
        assert_eq!(
            k,
            vec![
                TokenKind::Ident, TokenKind::Bang,
                TokenKind::LAngle, TokenKind::Ident, TokenKind::RAngle,
            ]
        );
    }

    #[test]
    fn test_not_equal_operator() {
        let k = kinds("email != nil");
        assert_eq!(
            k,
            vec![TokenKind::Ident, TokenKind::NotEq, TokenKind::Ident]
        );
    }

    #[test]
    fn test_arrow_return_type() {
        let k = kinds("-> Res!<Customer>");
        assert_eq!(
            k,
            vec![
                TokenKind::Arrow, TokenKind::Ident, TokenKind::Bang,
                TokenKind::LAngle, TokenKind::Ident, TokenKind::RAngle,
            ]
        );
    }

    #[test]
    fn test_port_method_signature() {
        let src = "send_sms(phone: Phone, msg: Str) -> Res!";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Ident, TokenKind::LParen,
                TokenKind::Ident, TokenKind::Colon, TokenKind::Ident, TokenKind::Comma,
                TokenKind::Ident, TokenKind::Colon, TokenKind::Ident,
                TokenKind::RParen, TokenKind::Arrow,
                TokenKind::Ident, TokenKind::Bang,
            ]
        );
    }

    #[test]
    fn test_comment_skipped_in_indentation() {
        let src = "sol App\n  # comment\n  ctx Users";
        let k = kinds(src);
        let indent_count = k.iter().filter(|t| **t == TokenKind::Indent).count();
        assert_eq!(indent_count, 1);
    }

    #[test]
    fn test_blank_lines_skipped() {
        let src = "sol App\n\n  ctx Users";
        let k = kinds(src);
        assert!(k.contains(&TokenKind::Sol));
        let indent_count = k.iter().filter(|t| **t == TokenKind::Indent).count();
        assert_eq!(indent_count, 1);
    }

    #[test]
    fn test_full_example_lexes() {
        let src = include_str!("../../../examples/customer_onboarding.veil");
        let tokens = lex(src);
        assert!(tokens.len() > 50);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
        let indents = tokens.iter().filter(|t| t.kind == TokenKind::Indent).count();
        let dedents = tokens.iter().filter(|t| t.kind == TokenKind::Dedent).count();
        assert_eq!(indents, dedents);
    }

    #[test]
    fn test_call_expression_tokens() {
        let src = "call CustomerRepo.save(Customer.new(email, phone))";
        let t = tokens_text(src);
        assert_eq!(t[0], (TokenKind::Call, "call".to_string()));
        assert_eq!(t[1], (TokenKind::Ident, "CustomerRepo".to_string()));
        assert_eq!(t[2], (TokenKind::Dot, ".".to_string()));
        assert_eq!(t[3], (TokenKind::Ident, "save".to_string()));
        assert_eq!(t[4], (TokenKind::LParen, "(".to_string()));
    }

    #[test]
    fn test_emit_expression_tokens() {
        // emit is layer vocabulary now — lexes as Ident.
        let src = "emit CustomerCreated{c.id, email, c.created}";
        let t = tokens_text(src);
        assert_eq!(t[0], (TokenKind::Ident, "emit".to_string()));
        assert_eq!(t[1], (TokenKind::Ident, "CustomerCreated".to_string()));
        assert_eq!(t[2], (TokenKind::LBrace, "{".to_string()));
    }

    #[test]
    fn test_adapter_for_syntax() {
        // adapter is layer vocabulary (Ident); `for` is core.
        let src = "adapter SmsTwilio for Notifier";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Ident, TokenKind::Ident,
                TokenKind::For, TokenKind::Ident,
            ]
        );
    }

    #[test]
    fn test_at_token_before_digit() {
        // `@` followed by a digit emits At (version pin syntax).
        // The lexer produces FloatLit("1.140") because of greedy number parsing.
        let toks = tokens_text("use aws_sdk_dynamodb@1.140.0");
        assert_eq!(toks[0], (TokenKind::Use, "use".into()));
        assert_eq!(toks[1], (TokenKind::Ident, "aws_sdk_dynamodb".into()));
        assert_eq!(toks[2], (TokenKind::At, "@".into()));
        assert_eq!(toks[3], (TokenKind::FloatLit, "1.140".into()));
        assert_eq!(toks[4], (TokenKind::Dot, ".".into()));
        assert_eq!(toks[5], (TokenKind::IntLit, "0".into()));
    }

    #[test]
    fn test_annotation_still_works() {
        // `@retry` (letter after @) still lexes as Annotation.
        let toks = tokens_text("@retry(3)");
        assert_eq!(toks[0].0, TokenKind::Annotation);
        assert_eq!(toks[0].1, "@retry(3)");
    }
}
