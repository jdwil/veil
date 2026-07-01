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
    fn test_all_toplevel_keywords() {
        let src = "sol ctx agg ent val evt cmd qry port adapter flow svc pipe lang";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Sol, TokenKind::Ctx, TokenKind::Agg, TokenKind::Ent,
                TokenKind::Val, TokenKind::Evt, TokenKind::Cmd, TokenKind::Qry,
                TokenKind::Port, TokenKind::Adapter, TokenKind::Flow, TokenKind::Svc,
                TokenKind::Pipe, TokenKind::Lang,
            ]
        );
    }

    #[test]
    fn test_flow_keywords() {
        let src = "step par alt loop err match emit call ret input fallback impl for boundary";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Step, TokenKind::Par, TokenKind::Alt, TokenKind::Loop,
                TokenKind::Err, TokenKind::Match, TokenKind::Emit, TokenKind::Call,
                TokenKind::Ret, TokenKind::Input, TokenKind::Fallback, TokenKind::Impl,
                TokenKind::For, TokenKind::Boundary,
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
                TokenKind::Arrow, TokenKind::FatArrow, TokenKind::Parallel,
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
                TokenKind::Indent, TokenKind::Ctx, TokenKind::Ident, TokenKind::Newline,
                TokenKind::Indent, TokenKind::Agg, TokenKind::Ident,
                TokenKind::Dedent, TokenKind::Dedent,
            ]
        );
    }

    #[test]
    fn test_indent_dedent_multiple() {
        let src = "sol App\n  ctx A\n    agg B\n  ctx C";
        let k = kinds(src);
        // After "agg B" we should dedent back to ctx level for "ctx C"
        assert!(k.contains(&TokenKind::Indent));
        assert!(k.contains(&TokenKind::Dedent));
        // Count: 2 indents (sol->ctx, ctx->agg), 1 dedent (agg->ctx for "ctx C"), 1 dedent at EOF
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
        // @retry 3 followed by keyword 'step' should not consume 'step'
        let src = "@retry 3\nstep foo";
        let k = kinds(src);
        assert_eq!(k[0], TokenKind::Annotation);
        assert!(k.contains(&TokenKind::Step));
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
        // "c.id" should be Ident Dot Ident, not a float
        let k = kinds("c.id");
        assert_eq!(k, vec![TokenKind::Ident, TokenKind::Dot, TokenKind::Ident]);
    }

    #[test]
    fn test_result_type_syntax() {
        // Res!<Customer> should tokenize as: Ident("Res") Bang LAngle Ident("Customer") RAngle
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
        // Comments shouldn't affect indentation state
        let src = "sol App\n  # comment\n  ctx Users";
        let k = kinds(src);
        let indent_count = k.iter().filter(|t| **t == TokenKind::Indent).count();
        assert_eq!(indent_count, 1); // Only one indent: sol -> ctx level
    }

    #[test]
    fn test_blank_lines_skipped() {
        let src = "sol App\n\n  ctx Users";
        let k = kinds(src);
        // Blank line shouldn't produce spurious dedents
        assert!(k.contains(&TokenKind::Sol));
        assert!(k.contains(&TokenKind::Ctx));
        let indent_count = k.iter().filter(|t| **t == TokenKind::Indent).count();
        assert_eq!(indent_count, 1);
    }

    #[test]
    fn test_full_example_lexes() {
        let src = include_str!("../../../examples/customer_onboarding.veil");
        let tokens = lex(src);
        // Should have tokens and end with Eof
        assert!(tokens.len() > 50);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
        // Should have balanced indents/dedents
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
        let src = "emit CustomerCreated{c.id, email, c.created}";
        let t = tokens_text(src);
        assert_eq!(t[0], (TokenKind::Emit, "emit".to_string()));
        assert_eq!(t[1], (TokenKind::Ident, "CustomerCreated".to_string()));
        assert_eq!(t[2], (TokenKind::LBrace, "{".to_string()));
    }

    #[test]
    fn test_adapter_for_syntax() {
        let src = "adapter SmsTwilio for Notifier";
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Adapter, TokenKind::Ident,
                TokenKind::For, TokenKind::Ident,
            ]
        );
    }
}
