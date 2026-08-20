use super::*;
use super::lower::{lower_call, lower_string_interp};
use super::super::context::GenCtx;
use veil_ir::ast::{Expr};

#[test]
fn emit_string_lit() {
    let expr = RustExpr::StringLit("hello".to_string());
    assert_eq!(emit(&expr), r#""hello""#);
}

#[test]
fn emit_string_lit_escaped() {
    let expr = RustExpr::StringLit(r#"say "hi""#.to_string());
    assert_eq!(emit(&expr), r#""say \"hi\"""#);
}

#[test]
fn emit_string_lit_backslash() {
    let expr = RustExpr::StringLit(r"path\to\file".to_string());
    assert_eq!(emit(&expr), r#""path\\to\\file""#);
}

#[test]
fn emit_int_lit() {
    let expr = RustExpr::IntLit(42);
    assert_eq!(emit(&expr), "42");
}

#[test]
fn emit_int_lit_negative() {
    let expr = RustExpr::IntLit(-7);
    assert_eq!(emit(&expr), "-7");
}

#[test]
fn emit_float_lit() {
    let expr = RustExpr::FloatLit(3.15);
    assert_eq!(emit(&expr), "3.15");
}

#[test]
fn emit_bool_lit() {
    assert_eq!(emit(&RustExpr::BoolLit(true)), "true");
    assert_eq!(emit(&RustExpr::BoolLit(false)), "false");
}

#[test]
fn emit_raw_passthrough() {
    let expr = RustExpr::Statement {
        text: "some_complex_expr.await?".to_string(),
        ty: None,
    };
    assert_eq!(emit(&expr), "some_complex_expr.await?");
}

#[test]
fn emit_ident() {
    let expr = RustExpr::Ident {
        name: "my_var".to_string(),
        ty: None,
    };
    assert_eq!(emit(&expr), "my_var");
}

#[test]
fn emit_clone() {
    let expr = RustExpr::Clone(Box::new(RustExpr::Ident {
        name: "x".to_string(),
        ty: None,
    }));
    assert_eq!(emit(&expr), "x.clone()");
}

#[test]
fn emit_borrow() {
    let expr = RustExpr::Borrow {
        inner: Box::new(RustExpr::Ident {
            name: "x".to_string(),
            ty: None,
        }),
        mutable: false,
    };
    assert_eq!(emit(&expr), "&x");

    let mut_expr = RustExpr::Borrow {
        inner: Box::new(RustExpr::Ident {
            name: "y".to_string(),
            ty: None,
        }),
        mutable: true,
    };
    assert_eq!(emit(&mut_expr), "&mut y");
}

#[test]
fn emit_await() {
    let expr = RustExpr::Await(Box::new(RustExpr::Ident {
        name: "future".to_string(),
        ty: None,
    }));
    assert_eq!(emit(&expr), "future.await");
}

#[test]
fn emit_try() {
    let expr = RustExpr::Try(Box::new(RustExpr::Ident {
        name: "result".to_string(),
        ty: None,
    }));
    assert_eq!(emit(&expr), "result?");
}

#[test]
fn emit_method_call_simple() {
    let expr = RustExpr::MethodCall {
        receiver: Box::new(RustExpr::Ident {
            name: "self.repo".to_string(),
            ty: None,
        }),
        method: "find".to_string(),
        args: vec![RustExpr::Ident {
            name: "id".to_string(),
            ty: None,
        }],
        ty: None,
        is_async: false,
        is_fallible: false,
    };
    assert_eq!(emit(&expr), "self.repo.find(id)");
}

#[test]
fn emit_method_call_async_fallible() {
    let expr = RustExpr::MethodCall {
        receiver: Box::new(RustExpr::Ident {
            name: "deps.repo".to_string(),
            ty: None,
        }),
        method: "save".to_string(),
        args: vec![RustExpr::Ident {
            name: "entity".to_string(),
            ty: None,
        }],
        ty: None,
        is_async: true,
        is_fallible: true,
    };
    assert_eq!(emit(&expr), "deps.repo.save(entity).await?");
}

#[test]
fn emit_fn_call() {
    let expr = RustExpr::FnCall {
        path: "serde_json::from_str".to_string(),
        args: vec![RustExpr::Ident {
            name: "input".to_string(),
            ty: None,
        }],
        ty: None,
    };
    assert_eq!(emit(&expr), "serde_json::from_str(input)");
}

#[test]
fn emit_let_binding() {
    let expr = RustExpr::Let {
        name: "x".to_string(),
        mutable: false,
        ty: None,
        value: Box::new(RustExpr::IntLit(42)),
    };
    assert_eq!(emit(&expr), "let x = 42");
}

#[test]
fn emit_let_mut_typed() {
    let expr = RustExpr::Let {
        name: "count".to_string(),
        mutable: true,
        ty: Some("i64".to_string()),
        value: Box::new(RustExpr::IntLit(0)),
    };
    assert_eq!(emit(&expr), "let mut count: i64 = 0");
}

#[test]
fn rust_type_from_str_basic() {
    assert_eq!(RustType::parse("i64"), RustType::Named("i64".to_string()));
    assert_eq!(RustType::parse("()"), RustType::Unit);
    assert_eq!(RustType::parse("serde_json::Value"), RustType::Json);
}

#[test]
fn rust_type_from_str_option() {
    assert_eq!(
        RustType::parse("Option<String>"),
        RustType::Option(Box::new(RustType::Named("String".to_string())))
    );
}

#[test]
fn rust_type_from_str_vec() {
    assert_eq!(
        RustType::parse("Vec<Customer>"),
        RustType::Vec(Box::new(RustType::Named("Customer".to_string())))
    );
}

#[test]
fn rust_type_parse_nested_generics() {
    // Option<Vec<String>> — nested generic
    assert_eq!(
        RustType::parse("Option<Vec<String>>"),
        RustType::Option(Box::new(RustType::Vec(Box::new(RustType::Named("String".to_string())))))
    );
    // Result<HashMap<String, Vec<Customer>>, Error> — comma inside nested generics
    assert_eq!(
        RustType::parse("Result<HashMap<String, Vec<Customer>>, Error>"),
        RustType::Result(Box::new(RustType::Named("HashMap<String, Vec<Customer>>".to_string())))
    );
    // Vec<Option<String>> — nested
    assert_eq!(
        RustType::parse("Vec<Option<String>>"),
        RustType::Vec(Box::new(RustType::Option(Box::new(RustType::Named("String".to_string())))))
    );
    // HashMap<String, Vec<Customer>> — falls through to Named (not Option/Result/Vec)
    assert_eq!(
        RustType::parse("HashMap<String, Vec<Customer>>"),
        RustType::Named("HashMap<String, Vec<Customer>>".to_string())
    );
}

#[test]
fn rust_type_is_copy() {
    assert!(RustType::Named("i64".to_string()).is_copy());
    assert!(RustType::Named("bool".to_string()).is_copy());
    assert!(RustType::Unit.is_copy());
    assert!(!RustType::Named("String".to_string()).is_copy());
    assert!(!RustType::Named("Customer".to_string()).is_copy());
}

// ─── apply_ownership tests ───────────────────────────────────────

fn make_ctx_with_uses(name: &str, uses: usize) -> GenCtx {
    use std::collections::HashMap;
    let mut ctx = GenCtx::new(HashMap::new());
    ctx.ownership.ident_uses.insert(name.to_string(), uses);
    ctx
}

#[test]
fn ownership_clone_not_needed_for_literals() {
    let ctx = GenCtx::new(std::collections::HashMap::new());
    let expr = RustExpr::StringLit("hello".to_string());
    let result = apply_ownership(expr.clone(), &ctx);
    assert_eq!(emit(&result), emit(&expr)); // unchanged
}

#[test]
fn ownership_clone_not_needed_for_copy_ident() {
    let mut ctx = make_ctx_with_uses("count", 3);
    ctx.types.local_types.insert("count".to_string(), "i64".to_string());
    let expr = RustExpr::Ident {
        name: "count".to_string(),
        ty: Some(RustType::Named("i64".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "count"); // no clone
}

#[test]
fn ownership_clone_multi_use_ident() {
    let ctx = make_ctx_with_uses("name", 2);
    let expr = RustExpr::Ident {
        name: "name".to_string(),
        ty: Some(RustType::Named("String".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "name.clone()");
}

#[test]
fn ownership_no_clone_single_use_ident() {
    let ctx = make_ctx_with_uses("name", 1);
    let expr = RustExpr::Ident {
        name: "name".to_string(),
        ty: Some(RustType::Named("String".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "name"); // last use, move
}

#[test]
fn ownership_no_double_clone() {
    let ctx = make_ctx_with_uses("x", 3);
    let expr = RustExpr::Clone(Box::new(RustExpr::Ident {
        name: "x".to_string(),
        ty: None,
    }));
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "x.clone()"); // not x.clone().clone()
}

#[test]
fn ownership_ref_elem_always_clones() {
    let mut ctx = make_ctx_with_uses("item", 1);
    ctx.ownership.ref_elem_locals.insert("item".to_string());
    let expr = RustExpr::Ident {
        name: "item".to_string(),
        ty: Some(RustType::Named("String".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "item.clone()");
}

#[test]
fn ownership_ref_local_no_clone() {
    let mut ctx = make_ctx_with_uses("data", 3);
    ctx.types.local_types.insert("data".to_string(), "&str".to_string());
    let expr = RustExpr::Ident {
        name: "data".to_string(),
        ty: Some(RustType::Ref(Box::new(RustType::Named("str".to_string())))),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "data"); // refs are copy
}

// ─── lower_string_interp tests ───────────────────────────────────

#[test]
fn lower_string_interp_basic() {
    use veil_ir::ast::StringPart;
    let ctx = GenCtx::new(std::collections::HashMap::new());
    let parts = vec![
        StringPart::Literal("Hello, ".to_string()),
        StringPart::Expr(Expr::Ident("name".to_string())),
        StringPart::Literal("!".to_string()),
    ];
    let result = lower_string_interp(&parts, &ctx);
    assert_eq!(emit(&result), "format!(\"Hello, {}!\", name)");
}

#[test]
fn lower_string_interp_no_exprs() {
    use veil_ir::ast::StringPart;
    let ctx = GenCtx::new(std::collections::HashMap::new());
    let parts = vec![StringPart::Literal("static text".to_string())];
    let result = lower_string_interp(&parts, &ctx);
    assert_eq!(emit(&result), "\"static text\".to_string()");
}

#[test]
fn lower_string_interp_brace_escape() {
    use veil_ir::ast::StringPart;
    let ctx = GenCtx::new(std::collections::HashMap::new());
    let parts = vec![
        StringPart::Literal("/{".to_string()),
        StringPart::Expr(Expr::Ident("id".to_string())),
        StringPart::Literal("}".to_string()),
    ];
    let result = lower_string_interp(&parts, &ctx);
    // `{` → `{{`, `}` → `}}` in literal parts; expr part → `{}`
    // Template: /{{ + {} + }} = /{{{}}}, which renders as /{<value>}
    assert_eq!(emit(&result), "format!(\"/{{{}}}\", id)");
}

// ─── lower_call tests ────────────────────────────────────────────

#[test]
fn lower_call_wraps_translate_call_output() {
    use veil_ir::ast::CallExpr;
    use veil_ir::Span;
    let ctx = GenCtx::new(std::collections::HashMap::new());
    let call = CallExpr {
        target: "Uuid".to_string(),
        method: "new_v4".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::default(),
    };
    let result = lower_call(&call, &ctx);
    assert_eq!(emit(&result), "Uuid::new_v4()");
}

// ─── apply_ownership on call results ─────────────────────────────

#[test]
fn ownership_raw_call_result_no_clone() {
    let ctx = GenCtx::new(std::collections::HashMap::new());
    // A function call result is already owned
    let expr = RustExpr::Statement {
        text: "Uuid::new_v4()".to_string(),
        ty: Some(RustType::Named("Uuid".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "Uuid::new_v4()"); // no clone
}

#[test]
fn ownership_raw_async_fallible_no_clone() {
    let ctx = GenCtx::new(std::collections::HashMap::new());
    // async+fallible call result is owned
    let expr = RustExpr::Statement {
        text: "deps.repo.save(entity).await?".to_string(),
        ty: Some(RustType::Named("String".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "deps.repo.save(entity).await?"); // no clone
}

#[test]
fn ownership_raw_block_expr_no_clone() {
    let ctx = GenCtx::new(std::collections::HashMap::new());
    // Block expression is owned
    let expr = RustExpr::Statement {
        text: "{ let x = 1; x }".to_string(),
        ty: Some(RustType::Named("i64".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "{ let x = 1; x }"); // no clone
}

#[test]
fn ownership_raw_bare_ident_still_clones() {
    let ctx = make_ctx_with_uses("data", 2);
    // A bare ident in Raw should still get cloned when multi-use
    let expr = RustExpr::Statement {
        text: "data".to_string(),
        ty: Some(RustType::Named("String".to_string())),
    };
    let result = apply_ownership(expr, &ctx);
    assert_eq!(emit(&result), "data.clone()");
}

// ─── suppress_try_in_closure tests ───────────────────────────────

#[test]
fn suppress_try_converts_try_to_unwrap() {
    let expr = RustExpr::Try(Box::new(RustExpr::Ident {
        name: "result".to_string(),
        ty: None,
    }));
    let result = suppress_try_in_closure(expr);
    assert_eq!(emit(&result), "result.unwrap()");
}

#[test]
fn suppress_try_converts_map_err_to_unwrap() {
    let expr = RustExpr::MapErr {
        inner: Box::new(RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident {
                name: "serde_json".to_string(),
                ty: None,
            }),
            method: "from_str".to_string(),
            args: vec![RustExpr::Ident {
                name: "s".to_string(),
                ty: None,
            }],
            ty: None,
            is_async: false,
            is_fallible: false,
        }),
        variant: "DomainError::External".to_string(),
    };
    let result = suppress_try_in_closure(expr);
    assert_eq!(emit(&result), "serde_json.from_str(s).unwrap()");
}

#[test]
fn suppress_try_converts_fallible_method_call() {
    let expr = RustExpr::MethodCall {
        receiver: Box::new(RustExpr::Ident {
            name: "repo".to_string(),
            ty: None,
        }),
        method: "save".to_string(),
        args: vec![],
        ty: None,
        is_async: true,
        is_fallible: true,
    };
    let result = suppress_try_in_closure(expr);
    // save().await? → save().await.unwrap()
    assert_eq!(emit(&result), "repo.save().await.unwrap()");
}

#[test]
fn suppress_try_raw_fixup_map_err() {
    let expr = RustExpr::Statement {
        text: "serde_json::from_str(&s).map_err(|e| DomainError::External(format!(\"{e:?}\")))?".to_string(),
        ty: None,
    };
    let result = suppress_try_in_closure(expr);
    assert_eq!(emit(&result), "serde_json::from_str(&s).unwrap()");
}

#[test]
fn suppress_try_raw_fixup_question_mark() {
    // `)?` pattern: parenthesized expr followed by `?`
    let expr = RustExpr::Statement {
        text: "serde_json::from_str(&s)?".to_string(),
        ty: None,
    };
    let result = suppress_try_in_closure(expr);
    assert_eq!(emit(&result), "serde_json::from_str(&s).unwrap()");
}

#[test]
fn suppress_try_leaves_non_fallible_unchanged() {
    let expr = RustExpr::MethodCall {
        receiver: Box::new(RustExpr::Ident {
            name: "items".to_string(),
            ty: None,
        }),
        method: "len".to_string(),
        args: vec![],
        ty: Some(RustType::Named("usize".to_string())),
        is_async: false,
        is_fallible: false,
    };
    let result = suppress_try_in_closure(expr);
    assert_eq!(emit(&result), "items.len()");
}
