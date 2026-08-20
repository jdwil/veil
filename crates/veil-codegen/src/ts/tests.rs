//! Unit tests for `emit_ts()` — at least one test per TsExpr variant.

use super::emit::emit_ts;
use super::expr::*;

// ─── Literals ────────────────────────────────────────────────────────────────

#[test]
fn emit_ident() {
    let expr = TsExpr::Ident { name: "myVar".into(), ty: None };
    assert_eq!(emit_ts(&expr), "myVar");
}

#[test]
fn emit_ident_with_type() {
    let expr = TsExpr::Ident { name: "count".into(), ty: Some(TsType::Number) };
    // Type info is carried for transforms, not rendered by emit
    assert_eq!(emit_ts(&expr), "count");
}

#[test]
fn emit_string_lit() {
    let expr = TsExpr::StringLit("hello world".into());
    assert_eq!(emit_ts(&expr), "\"hello world\"");
}

#[test]
fn emit_string_lit_escapes() {
    let expr = TsExpr::StringLit("say \"hi\"\nnewline".into());
    assert_eq!(emit_ts(&expr), "\"say \\\"hi\\\"\\nnewline\"");
}

#[test]
fn emit_template_lit_simple() {
    let expr = TsExpr::TemplateLit {
        parts: vec![
            TsTemplatePart::Literal("Hello, ".into()),
            TsTemplatePart::Expr(TsExpr::Ident { name: "name".into(), ty: None }),
            TsTemplatePart::Literal("!".into()),
        ],
    };
    assert_eq!(emit_ts(&expr), "`Hello, ${name}!`");
}

#[test]
fn emit_template_lit_escapes_backtick() {
    let expr = TsExpr::TemplateLit {
        parts: vec![TsTemplatePart::Literal("a `backtick` here".into())],
    };
    assert_eq!(emit_ts(&expr), "`a \\`backtick\\` here`");
}

#[test]
fn emit_int_lit() {
    assert_eq!(emit_ts(&TsExpr::IntLit(42)), "42");
    assert_eq!(emit_ts(&TsExpr::IntLit(-1)), "-1");
    assert_eq!(emit_ts(&TsExpr::IntLit(0)), "0");
}

#[test]
fn emit_float_lit() {
    assert_eq!(emit_ts(&TsExpr::FloatLit(3.14)), "3.14");
    assert_eq!(emit_ts(&TsExpr::FloatLit(1.0)), "1.0");
}

#[test]
fn emit_bool_lit() {
    assert_eq!(emit_ts(&TsExpr::BoolLit(true)), "true");
    assert_eq!(emit_ts(&TsExpr::BoolLit(false)), "false");
}

#[test]
fn emit_null_lit() {
    assert_eq!(emit_ts(&TsExpr::NullLit), "null");
}

#[test]
fn emit_undefined_lit() {
    assert_eq!(emit_ts(&TsExpr::UndefinedLit), "undefined");
}

#[test]
fn emit_array_lit_empty() {
    let expr = TsExpr::ArrayLit { items: vec![], ty: None };
    assert_eq!(emit_ts(&expr), "[]");
}

#[test]
fn emit_array_lit() {
    let expr = TsExpr::ArrayLit {
        items: vec![TsExpr::IntLit(1), TsExpr::IntLit(2), TsExpr::IntLit(3)],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "[1, 2, 3]");
}

#[test]
fn emit_object_lit_empty() {
    let expr = TsExpr::ObjectLit { fields: vec![], ty: None };
    assert_eq!(emit_ts(&expr), "{}");
}

#[test]
fn emit_object_lit() {
    let expr = TsExpr::ObjectLit {
        fields: vec![
            ("name".into(), TsExpr::StringLit("Alice".into())),
            ("age".into(), TsExpr::IntLit(30)),
        ],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "{ name: \"Alice\", age: 30 }");
}

#[test]
fn emit_object_lit_shorthand() {
    let expr = TsExpr::ObjectLit {
        fields: vec![
            ("name".into(), TsExpr::Ident { name: "name".into(), ty: None }),
            ("age".into(), TsExpr::IntLit(30)),
        ],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "{ name, age: 30 }");
}

// ─── Operators ───────────────────────────────────────────────────────────────

#[test]
fn emit_binop_add() {
    let expr = TsExpr::BinOp {
        left: Box::new(TsExpr::Ident { name: "a".into(), ty: None }),
        op: TsBinOp::Add,
        right: Box::new(TsExpr::IntLit(1)),
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "a + 1");
}

#[test]
fn emit_binop_strict_eq() {
    let expr = TsExpr::BinOp {
        left: Box::new(TsExpr::Ident { name: "x".into(), ty: None }),
        op: TsBinOp::Eq,
        right: Box::new(TsExpr::NullLit),
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "x === null");
}

#[test]
fn emit_binop_instanceof() {
    let expr = TsExpr::BinOp {
        left: Box::new(TsExpr::Ident { name: "e".into(), ty: None }),
        op: TsBinOp::Instanceof,
        right: Box::new(TsExpr::Ident { name: "Error".into(), ty: None }),
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "e instanceof Error");
}

#[test]
fn emit_unary_not() {
    let expr = TsExpr::UnaryOp {
        op: TsUnaryOp::Not,
        expr: Box::new(TsExpr::Ident { name: "done".into(), ty: None }),
    };
    assert_eq!(emit_ts(&expr), "!done");
}

#[test]
fn emit_unary_typeof() {
    let expr = TsExpr::UnaryOp {
        op: TsUnaryOp::Typeof,
        expr: Box::new(TsExpr::Ident { name: "x".into(), ty: None }),
    };
    assert_eq!(emit_ts(&expr), "typeof x");
}

#[test]
fn emit_unary_neg() {
    let expr = TsExpr::UnaryOp {
        op: TsUnaryOp::Neg,
        expr: Box::new(TsExpr::IntLit(5)),
    };
    assert_eq!(emit_ts(&expr), "-5");
}

#[test]
fn emit_optional_chain() {
    let expr = TsExpr::OptionalChain {
        base: Box::new(TsExpr::Ident { name: "user".into(), ty: None }),
        field: "name".into(),
    };
    assert_eq!(emit_ts(&expr), "user?.name");
}

#[test]
fn emit_nullish_coalesce() {
    let expr = TsExpr::NullishCoalesce {
        left: Box::new(TsExpr::Ident { name: "value".into(), ty: None }),
        right: Box::new(TsExpr::StringLit("default".into())),
    };
    assert_eq!(emit_ts(&expr), "value ?? \"default\"");
}

// ─── Access ──────────────────────────────────────────────────────────────────

#[test]
fn emit_field_access() {
    let expr = TsExpr::FieldAccess {
        base: Box::new(TsExpr::Ident { name: "user".into(), ty: None }),
        field: "email".into(),
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "user.email");
}

#[test]
fn emit_field_access_chained() {
    let expr = TsExpr::FieldAccess {
        base: Box::new(TsExpr::FieldAccess {
            base: Box::new(TsExpr::Ident { name: "a".into(), ty: None }),
            field: "b".into(),
            ty: None,
        }),
        field: "c".into(),
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "a.b.c");
}

#[test]
fn emit_index() {
    let expr = TsExpr::Index {
        base: Box::new(TsExpr::Ident { name: "arr".into(), ty: None }),
        index: Box::new(TsExpr::IntLit(0)),
    };
    assert_eq!(emit_ts(&expr), "arr[0]");
}

// ─── Calls ───────────────────────────────────────────────────────────────────

#[test]
fn emit_method_call() {
    let expr = TsExpr::MethodCall {
        receiver: Box::new(TsExpr::Ident { name: "arr".into(), ty: None }),
        method: "push".into(),
        args: vec![TsExpr::IntLit(42)],
        ty: None,
        is_async: false,
    };
    assert_eq!(emit_ts(&expr), "arr.push(42)");
}

#[test]
fn emit_method_call_async() {
    let expr = TsExpr::MethodCall {
        receiver: Box::new(TsExpr::Ident { name: "api".into(), ty: None }),
        method: "fetch".into(),
        args: vec![TsExpr::StringLit("/users".into())],
        ty: None,
        is_async: true,
    };
    assert_eq!(emit_ts(&expr), "await api.fetch(\"/users\")");
}

#[test]
fn emit_fn_call() {
    let expr = TsExpr::FnCall {
        name: "parseInt".into(),
        args: vec![TsExpr::StringLit("42".into()), TsExpr::IntLit(10)],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "parseInt(\"42\", 10)");
}

#[test]
fn emit_fn_call_no_args() {
    let expr = TsExpr::FnCall {
        name: "Date.now".into(),
        args: vec![],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "Date.now()");
}

#[test]
fn emit_new_call() {
    let expr = TsExpr::NewCall {
        class: "Map".into(),
        args: vec![],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "new Map()");
}

#[test]
fn emit_new_call_with_args() {
    let expr = TsExpr::NewCall {
        class: "Error".into(),
        args: vec![TsExpr::StringLit("something went wrong".into())],
        ty: None,
    };
    assert_eq!(emit_ts(&expr), "new Error(\"something went wrong\")");
}

// ─── Bindings ────────────────────────────────────────────────────────────────

#[test]
fn emit_const_simple() {
    let expr = TsExpr::Const {
        name: "x".into(),
        ty: None,
        value: Box::new(TsExpr::IntLit(42)),
    };
    assert_eq!(emit_ts(&expr), "const x = 42");
}

#[test]
fn emit_const_with_type() {
    let expr = TsExpr::Const {
        name: "name".into(),
        ty: Some("string".into()),
        value: Box::new(TsExpr::StringLit("Alice".into())),
    };
    assert_eq!(emit_ts(&expr), "const name: string = \"Alice\"");
}

#[test]
fn emit_let_binding() {
    let expr = TsExpr::Let {
        name: "count".into(),
        ty: None,
        value: Box::new(TsExpr::IntLit(0)),
    };
    assert_eq!(emit_ts(&expr), "let count = 0");
}

#[test]
fn emit_let_with_type() {
    let expr = TsExpr::Let {
        name: "items".into(),
        ty: Some("string[]".into()),
        value: Box::new(TsExpr::ArrayLit { items: vec![], ty: None }),
    };
    assert_eq!(emit_ts(&expr), "let items: string[] = []");
}

#[test]
fn emit_destructure_object() {
    let expr = TsExpr::Destructure {
        pattern: TsPattern::Object { fields: vec!["name".into(), "age".into()] },
        value: Box::new(TsExpr::Ident { name: "user".into(), ty: None }),
    };
    assert_eq!(emit_ts(&expr), "const { name, age } = user");
}

#[test]
fn emit_destructure_array() {
    let expr = TsExpr::Destructure {
        pattern: TsPattern::Array { items: vec!["first".into(), "second".into()] },
        value: Box::new(TsExpr::Ident { name: "pair".into(), ty: None }),
    };
    assert_eq!(emit_ts(&expr), "const [first, second] = pair");
}

#[test]
fn emit_assign() {
    let expr = TsExpr::Assign {
        target: Box::new(TsExpr::Ident { name: "x".into(), ty: None }),
        value: Box::new(TsExpr::IntLit(10)),
    };
    assert_eq!(emit_ts(&expr), "x = 10");
}

#[test]
fn emit_assign_field() {
    let expr = TsExpr::Assign {
        target: Box::new(TsExpr::FieldAccess {
            base: Box::new(TsExpr::Ident { name: "obj".into(), ty: None }),
            field: "x".into(),
            ty: None,
        }),
        value: Box::new(TsExpr::IntLit(5)),
    };
    assert_eq!(emit_ts(&expr), "obj.x = 5");
}

// ─── Control Flow ────────────────────────────────────────────────────────────

#[test]
fn emit_if_no_else() {
    let expr = TsExpr::If {
        condition: Box::new(TsExpr::Ident { name: "ready".into(), ty: None }),
        then_body: vec![TsExpr::FnCall {
            name: "start".into(),
            args: vec![],
            ty: None,
        }],
        else_body: None,
    };
    assert_eq!(
        emit_ts(&expr),
        "if (ready) {\n  start();\n}"
    );
}

#[test]
fn emit_if_else() {
    let expr = TsExpr::If {
        condition: Box::new(TsExpr::BinOp {
            left: Box::new(TsExpr::Ident { name: "x".into(), ty: None }),
            op: TsBinOp::Gt,
            right: Box::new(TsExpr::IntLit(0)),
            ty: None,
        }),
        then_body: vec![TsExpr::Return(Box::new(TsExpr::StringLit("positive".into())))],
        else_body: Some(vec![TsExpr::Return(Box::new(TsExpr::StringLit("non-positive".into())))]),
    };
    assert_eq!(
        emit_ts(&expr),
        "if (x > 0) {\n  return \"positive\";\n} else {\n  return \"non-positive\";\n}"
    );
}

#[test]
fn emit_switch() {
    let expr = TsExpr::Switch {
        scrutinee: Box::new(TsExpr::Ident { name: "action".into(), ty: None }),
        cases: vec![
            ("add".into(), vec![TsExpr::FnCall { name: "handleAdd".into(), args: vec![], ty: None }]),
            ("remove".into(), vec![TsExpr::FnCall { name: "handleRemove".into(), args: vec![], ty: None }]),
        ],
        default: Some(vec![TsExpr::Throw {
            message: Box::new(TsExpr::NewCall {
                class: "Error".into(),
                args: vec![TsExpr::StringLit("unknown".into())],
                ty: None,
            }),
        }]),
    };
    let expected = "\
switch (action) {
  case \"add\":
    handleAdd();
    break;
  case \"remove\":
    handleRemove();
    break;
  default:
    throw new Error(\"unknown\");
}";
    assert_eq!(emit_ts(&expr), expected);
}

#[test]
fn emit_for_of() {
    let expr = TsExpr::For {
        binding: "item".into(),
        iterable: Box::new(TsExpr::Ident { name: "items".into(), ty: None }),
        body: vec![TsExpr::FnCall {
            name: "process".into(),
            args: vec![TsExpr::Ident { name: "item".into(), ty: None }],
            ty: None,
        }],
    };
    assert_eq!(
        emit_ts(&expr),
        "for (const item of items) {\n  process(item);\n}"
    );
}

#[test]
fn emit_for_index() {
    let expr = TsExpr::ForIndex {
        index: "i".into(),
        binding: "item".into(),
        iterable: Box::new(TsExpr::Ident { name: "list".into(), ty: None }),
        body: vec![TsExpr::FnCall {
            name: "log".into(),
            args: vec![
                TsExpr::Ident { name: "i".into(), ty: None },
                TsExpr::Ident { name: "item".into(), ty: None },
            ],
            ty: None,
        }],
    };
    let expected = "\
for (let i = 0; i < list.length; i++) {
  const item = list[i];
  log(i, item);
}";
    assert_eq!(emit_ts(&expr), expected);
}

#[test]
fn emit_while() {
    let expr = TsExpr::While {
        condition: Box::new(TsExpr::Ident { name: "running".into(), ty: None }),
        body: vec![TsExpr::FnCall {
            name: "tick".into(),
            args: vec![],
            ty: None,
        }],
    };
    assert_eq!(
        emit_ts(&expr),
        "while (running) {\n  tick();\n}"
    );
}

#[test]
fn emit_loop() {
    let expr = TsExpr::Loop {
        body: vec![
            TsExpr::If {
                condition: Box::new(TsExpr::Ident { name: "done".into(), ty: None }),
                then_body: vec![TsExpr::Break],
                else_body: None,
            },
            TsExpr::FnCall { name: "work".into(), args: vec![], ty: None },
        ],
    };
    let expected = "\
while (true) {
  if (done) {
    break;
  }
  work();
}";
    assert_eq!(emit_ts(&expr), expected);
}

// ─── Functions ───────────────────────────────────────────────────────────────

#[test]
fn emit_arrow_fn_expression() {
    let expr = TsExpr::ArrowFn {
        params: vec!["x".into()],
        body: vec![TsExpr::BinOp {
            left: Box::new(TsExpr::Ident { name: "x".into(), ty: None }),
            op: TsBinOp::Mul,
            right: Box::new(TsExpr::IntLit(2)),
            ty: None,
        }],
        is_async: false,
    };
    assert_eq!(emit_ts(&expr), "(x) => x * 2");
}

#[test]
fn emit_arrow_fn_block() {
    let expr = TsExpr::ArrowFn {
        params: vec!["a".into(), "b".into()],
        body: vec![
            TsExpr::Const {
                name: "sum".into(),
                ty: None,
                value: Box::new(TsExpr::BinOp {
                    left: Box::new(TsExpr::Ident { name: "a".into(), ty: None }),
                    op: TsBinOp::Add,
                    right: Box::new(TsExpr::Ident { name: "b".into(), ty: None }),
                    ty: None,
                }),
            },
            TsExpr::Return(Box::new(TsExpr::Ident { name: "sum".into(), ty: None })),
        ],
        is_async: false,
    };
    assert_eq!(
        emit_ts(&expr),
        "(a, b) => {\n  const sum = a + b;\n  return sum;\n}"
    );
}

#[test]
fn emit_arrow_fn_async() {
    let expr = TsExpr::ArrowFn {
        params: vec!["url".into()],
        body: vec![TsExpr::MethodCall {
            receiver: Box::new(TsExpr::Ident { name: "api".into(), ty: None }),
            method: "get".into(),
            args: vec![TsExpr::Ident { name: "url".into(), ty: None }],
            ty: None,
            is_async: true,
        }],
        is_async: true,
    };
    assert_eq!(emit_ts(&expr), "async (url) => await api.get(url)");
}

#[test]
fn emit_return() {
    let expr = TsExpr::Return(Box::new(TsExpr::IntLit(42)));
    assert_eq!(emit_ts(&expr), "return 42");
}

#[test]
fn emit_await() {
    let expr = TsExpr::Await(Box::new(TsExpr::FnCall {
        name: "fetchData".into(),
        args: vec![],
        ty: None,
    }));
    assert_eq!(emit_ts(&expr), "await fetchData()");
}

#[test]
fn emit_throw() {
    let expr = TsExpr::Throw {
        message: Box::new(TsExpr::NewCall {
            class: "Error".into(),
            args: vec![TsExpr::StringLit("failed".into())],
            ty: None,
        }),
    };
    assert_eq!(emit_ts(&expr), "throw new Error(\"failed\")");
}

// ─── TS-Specific ─────────────────────────────────────────────────────────────

#[test]
fn emit_type_assertion() {
    let expr = TsExpr::TypeAssertion {
        expr: Box::new(TsExpr::Ident { name: "value".into(), ty: None }),
        ty: "string".into(),
    };
    assert_eq!(emit_ts(&expr), "value as string");
}

#[test]
fn emit_non_null_assertion() {
    let expr = TsExpr::NonNullAssertion(Box::new(TsExpr::Ident {
        name: "el".into(),
        ty: None,
    }));
    assert_eq!(emit_ts(&expr), "el!");
}

#[test]
fn emit_spread() {
    let expr = TsExpr::Spread(Box::new(TsExpr::Ident {
        name: "args".into(),
        ty: None,
    }));
    assert_eq!(emit_ts(&expr), "...args");
}

// ─── Statements ──────────────────────────────────────────────────────────────

#[test]
fn emit_break() {
    assert_eq!(emit_ts(&TsExpr::Break), "break");
}

#[test]
fn emit_continue() {
    assert_eq!(emit_ts(&TsExpr::Continue), "continue");
}

// ─── Escape Hatches ──────────────────────────────────────────────────────────

#[test]
fn emit_raw() {
    let expr = TsExpr::Raw("console.log('hello');".into());
    assert_eq!(emit_ts(&expr), "console.log('hello');");
}

#[test]
fn emit_layer_emit() {
    let expr = TsExpr::LayerEmit("// generated by layer".into());
    assert_eq!(emit_ts(&expr), "// generated by layer");
}

// ─── TsType rendering ───────────────────────────────────────────────────────

#[test]
fn ts_type_primitives() {
    assert_eq!(TsType::String.to_ts(), "string");
    assert_eq!(TsType::Number.to_ts(), "number");
    assert_eq!(TsType::Boolean.to_ts(), "boolean");
    assert_eq!(TsType::Null.to_ts(), "null");
    assert_eq!(TsType::Void.to_ts(), "void");
}

#[test]
fn ts_type_array() {
    let ty = TsType::Array(Box::new(TsType::String));
    assert_eq!(ty.to_ts(), "string[]");
}

#[test]
fn ts_type_promise() {
    let ty = TsType::Promise(Box::new(TsType::Number));
    assert_eq!(ty.to_ts(), "Promise<number>");
}

#[test]
fn ts_type_union() {
    let ty = TsType::Union(vec![TsType::String, TsType::Null]);
    assert_eq!(ty.to_ts(), "string | null");
}

#[test]
fn ts_type_named() {
    let ty = TsType::Named("Customer".into());
    assert_eq!(ty.to_ts(), "Customer");
}

#[test]
fn ts_type_record() {
    let ty = TsType::Record(Box::new(TsType::String), Box::new(TsType::Number));
    assert_eq!(ty.to_ts(), "Record<string, number>");
}

#[test]
fn ts_type_fn() {
    let ty = TsType::Fn {
        params: vec![TsType::String, TsType::Number],
        ret: Box::new(TsType::Boolean),
    };
    assert_eq!(ty.to_ts(), "(arg0: string, arg1: number) => boolean");
}

// ─── TsBinOp coverage ────────────────────────────────────────────────────────

#[test]
fn binop_operators_all() {
    assert_eq!(TsBinOp::Add.as_str(), "+");
    assert_eq!(TsBinOp::Sub.as_str(), "-");
    assert_eq!(TsBinOp::Mul.as_str(), "*");
    assert_eq!(TsBinOp::Div.as_str(), "/");
    assert_eq!(TsBinOp::Mod.as_str(), "%");
    assert_eq!(TsBinOp::Eq.as_str(), "===");
    assert_eq!(TsBinOp::NotEq.as_str(), "!==");
    assert_eq!(TsBinOp::Lt.as_str(), "<");
    assert_eq!(TsBinOp::Gt.as_str(), ">");
    assert_eq!(TsBinOp::LtEq.as_str(), "<=");
    assert_eq!(TsBinOp::GtEq.as_str(), ">=");
    assert_eq!(TsBinOp::And.as_str(), "&&");
    assert_eq!(TsBinOp::Or.as_str(), "||");
    assert_eq!(TsBinOp::BitAnd.as_str(), "&");
    assert_eq!(TsBinOp::BitOr.as_str(), "|");
    assert_eq!(TsBinOp::BitXor.as_str(), "^");
    assert_eq!(TsBinOp::Shl.as_str(), "<<");
    assert_eq!(TsBinOp::Shr.as_str(), ">>");
    assert_eq!(TsBinOp::Instanceof.as_str(), "instanceof");
    assert_eq!(TsBinOp::In.as_str(), "in");
}

// ─── Composition tests ───────────────────────────────────────────────────────

#[test]
fn emit_nested_optional_chain_with_nullish_coalesce() {
    // user?.profile?.name ?? "Anonymous"
    let expr = TsExpr::NullishCoalesce {
        left: Box::new(TsExpr::OptionalChain {
            base: Box::new(TsExpr::OptionalChain {
                base: Box::new(TsExpr::Ident { name: "user".into(), ty: None }),
                field: "profile".into(),
            }),
            field: "name".into(),
        }),
        right: Box::new(TsExpr::StringLit("Anonymous".into())),
    };
    assert_eq!(emit_ts(&expr), "user?.profile?.name ?? \"Anonymous\"");
}

#[test]
fn emit_const_with_await_method_call() {
    // const data = await api.fetchUsers("/users");
    let expr = TsExpr::Const {
        name: "data".into(),
        ty: None,
        value: Box::new(TsExpr::MethodCall {
            receiver: Box::new(TsExpr::Ident { name: "api".into(), ty: None }),
            method: "fetchUsers".into(),
            args: vec![TsExpr::StringLit("/users".into())],
            ty: None,
            is_async: true,
        }),
    };
    assert_eq!(emit_ts(&expr), "const data = await api.fetchUsers(\"/users\")");
}

#[test]
fn emit_spread_in_object() {
    // { ...defaults, name: "custom" }
    let expr = TsExpr::ObjectLit {
        fields: vec![
            ("...defaults".into(), TsExpr::Raw(String::new())),
        ],
        ty: None,
    };
    // For a more idiomatic spread-in-object, we'd typically use Spread node in an array context.
    // But let's test the actual Spread variant:
    let spread_expr = TsExpr::Spread(Box::new(TsExpr::Ident {
        name: "defaults".into(),
        ty: None,
    }));
    assert_eq!(emit_ts(&spread_expr), "...defaults");
}

#[test]
fn emit_complex_arrow_with_destructure() {
    // async (req) => {
    //   const { name, age } = req.body;
    //   return name;
    // }
    let expr = TsExpr::ArrowFn {
        params: vec!["req".into()],
        body: vec![
            TsExpr::Destructure {
                pattern: TsPattern::Object { fields: vec!["name".into(), "age".into()] },
                value: Box::new(TsExpr::FieldAccess {
                    base: Box::new(TsExpr::Ident { name: "req".into(), ty: None }),
                    field: "body".into(),
                    ty: None,
                }),
            },
            TsExpr::Return(Box::new(TsExpr::Ident { name: "name".into(), ty: None })),
        ],
        is_async: true,
    };
    assert_eq!(
        emit_ts(&expr),
        "async (req) => {\n  const { name, age } = req.body;\n  return name;\n}"
    );
}

#[test]
fn emit_nested_if_in_for() {
    // for (const item of items) {
    //   if (item.active) {
    //     process(item);
    //   }
    // }
    let expr = TsExpr::For {
        binding: "item".into(),
        iterable: Box::new(TsExpr::Ident { name: "items".into(), ty: None }),
        body: vec![TsExpr::If {
            condition: Box::new(TsExpr::FieldAccess {
                base: Box::new(TsExpr::Ident { name: "item".into(), ty: None }),
                field: "active".into(),
                ty: None,
            }),
            then_body: vec![TsExpr::FnCall {
                name: "process".into(),
                args: vec![TsExpr::Ident { name: "item".into(), ty: None }],
                ty: None,
            }],
            else_body: None,
        }],
    };
    let expected = "\
for (const item of items) {
  if (item.active) {
    process(item);
  }
}";
    assert_eq!(emit_ts(&expr), expected);
}

// ═══════════════════════════════════════════════════════════════════════════════
// lower_to_ts tests — VEIL Expr → TsExpr → emit_ts round-trip
// ═══════════════════════════════════════════════════════════════════════════════

use super::lower::{lower_to_ts, lower_block, to_camel_case, veil_type_to_ts};
use super::expr::TsExpr;
use crate::expr::GenCtx;
use std::collections::HashMap;
use veil_ir::ast::{BinOp, BinaryOpExpr, Expr, IfExprData, MatchArm, Pattern, StringPart, TypeExpr, UnaryOp, UnaryOpExpr};

/// Helper: create a minimal GenCtx for tests.
fn test_ctx() -> GenCtx {
    GenCtx::new(HashMap::new())
}

/// Helper: create a GenCtx with a local type entry.
fn ctx_with_local(name: &str, ty: &str) -> GenCtx {
    let mut ctx = test_ctx();
    ctx.types.local_types.insert(name.to_string(), ty.to_string());
    ctx
}

// ─── Batch 1: Literals ───────────────────────────────────────────────────────

#[test]
fn lower_string_lit() {
    let expr = Expr::StringLit("hello".to_string());
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "\"hello\"");
}

#[test]
fn lower_int_lit() {
    let expr = Expr::IntLit(42);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "42");
}

#[test]
fn lower_float_lit() {
    let expr = Expr::FloatLit(9.81);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "9.81");
}

#[test]
fn lower_bool_lit_true() {
    let expr = Expr::BoolLit(true);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "true");
}

#[test]
fn lower_bool_lit_false() {
    let expr = Expr::BoolLit(false);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "false");
}

#[test]
fn lower_string_interp() {
    let expr = Expr::StringInterp(vec![
        StringPart::Literal("Hello, ".to_string()),
        StringPart::Expr(Expr::Ident("user_name".to_string())),
        StringPart::Literal("!".to_string()),
    ]);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "`Hello, ${userName}!`");
}

// ─── Batch 2: Identifiers + Field Access ─────────────────────────────────────

#[test]
fn lower_ident_snake_case() {
    let expr = Expr::Ident("user_name".to_string());
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "userName");
}

#[test]
fn lower_ident_null() {
    let expr = Expr::Ident("null".to_string());
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "null");
}

#[test]
fn lower_ident_none() {
    let expr = Expr::Ident("None".to_string());
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "null");
}

#[test]
fn lower_ident_with_type_context() {
    let ctx = ctx_with_local("user_name", "Str");
    let expr = Expr::Ident("user_name".to_string());
    let ts = lower_to_ts(&expr, &ctx);
    // Type is carried internally but not rendered by emit
    assert_eq!(emit_ts(&ts), "userName");
}

#[test]
fn lower_field_access() {
    let expr = Expr::FieldAccess(
        Box::new(Expr::Ident("my_user".to_string())),
        "first_name".to_string(),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "myUser.firstName");
}

#[test]
fn lower_field_access_nested() {
    let expr = Expr::FieldAccess(
        Box::new(Expr::FieldAccess(
            Box::new(Expr::Ident("config".to_string())),
            "database".to_string(),
        )),
        "host_name".to_string(),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "config.database.hostName");
}

// ─── Batch 3: Operators ──────────────────────────────────────────────────────

#[test]
fn lower_binop_add() {
    let expr = Expr::BinaryOp(BinaryOpExpr {
        left: Box::new(Expr::Ident("a".to_string())),
        op: BinOp::Add,
        right: Box::new(Expr::IntLit(1)),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "a + 1");
}

#[test]
fn lower_binop_strict_equality() {
    let expr = Expr::BinaryOp(BinaryOpExpr {
        left: Box::new(Expr::Ident("status".to_string())),
        op: BinOp::Eq,
        right: Box::new(Expr::StringLit("active".to_string())),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "status === \"active\"");
}

#[test]
fn lower_binop_strict_inequality() {
    let expr = Expr::BinaryOp(BinaryOpExpr {
        left: Box::new(Expr::Ident("count".to_string())),
        op: BinOp::NotEq,
        right: Box::new(Expr::IntLit(0)),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "count !== 0");
}

#[test]
fn lower_binop_and() {
    let expr = Expr::BinaryOp(BinaryOpExpr {
        left: Box::new(Expr::Ident("a".to_string())),
        op: BinOp::And,
        right: Box::new(Expr::Ident("b".to_string())),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "a && b");
}

#[test]
fn lower_binop_or() {
    let expr = Expr::BinaryOp(BinaryOpExpr {
        left: Box::new(Expr::Ident("x".to_string())),
        op: BinOp::Or,
        right: Box::new(Expr::Ident("y".to_string())),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "x || y");
}

#[test]
fn lower_binop_comparison() {
    let expr = Expr::BinaryOp(BinaryOpExpr {
        left: Box::new(Expr::Ident("age".to_string())),
        op: BinOp::GtEq,
        right: Box::new(Expr::IntLit(18)),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "age >= 18");
}

#[test]
fn lower_unary_not() {
    let expr = Expr::UnaryOp(UnaryOpExpr {
        op: UnaryOp::Not,
        expr: Box::new(Expr::Ident("is_valid".to_string())),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "!isValid");
}

#[test]
fn lower_unary_neg() {
    let expr = Expr::UnaryOp(UnaryOpExpr {
        op: UnaryOp::Neg,
        expr: Box::new(Expr::IntLit(5)),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "-5");
}

// ─── Batch 4: Bindings ───────────────────────────────────────────────────────

#[test]
fn lower_assign_const() {
    let expr = Expr::Assign(
        "user_name".to_string(),
        Box::new(Expr::StringLit("alice".to_string())),
        None,
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "const userName = \"alice\"");
}

#[test]
fn lower_assign_const_with_type() {
    let expr = Expr::Assign(
        "count".to_string(),
        Box::new(Expr::IntLit(0)),
        Some(TypeExpr::Named("Int".to_string())),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "const count: number = 0");
}

#[test]
fn lower_assign_field_write() {
    // `loan.returned = true` → assignment, not const
    let expr = Expr::Assign(
        "loan.returned".to_string(),
        Box::new(Expr::BoolLit(true)),
        None,
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "loan.returned = true");
}

#[test]
fn lower_mut_assign_let() {
    let expr = Expr::MutAssign(
        "counter".to_string(),
        Box::new(Expr::IntLit(0)),
        None,
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "let counter = 0");
}

#[test]
fn lower_mut_assign_let_with_type() {
    let expr = Expr::MutAssign(
        "items".to_string(),
        Box::new(Expr::ArrayLit(vec![])),
        Some(TypeExpr::List(Box::new(TypeExpr::Named("Str".to_string())))),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "let items: string[] = []");
}

#[test]
fn lower_let_pattern_tuple() {
    let expr = Expr::LetPattern(
        Pattern::Tuple(vec![
            Pattern::Ident("first".to_string()),
            Pattern::Ident("second".to_string()),
        ]),
        Box::new(Expr::Ident("pair".to_string())),
        None,
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "const [first, second] = pair");
}

#[test]
fn lower_let_pattern_struct() {
    let expr = Expr::LetPattern(
        Pattern::Struct(
            "User".to_string(),
            vec![
                ("user_name".to_string(), None),
                ("email".to_string(), None),
            ],
            false,
        ),
        Box::new(Expr::Ident("data".to_string())),
        None,
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "const { userName, email } = data");
}

// ─── Batch 5: Simple Wrappers ────────────────────────────────────────────────

#[test]
fn lower_return_value() {
    let expr = Expr::Return(Box::new(Expr::Ident("result".to_string())));
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "return result");
}

#[test]
fn lower_return_null() {
    let expr = Expr::Return(Box::new(Expr::Ident("null".to_string())));
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "return null");
}

#[test]
fn lower_return_ok_bare() {
    // `return Ok` in VEIL → just `return` in TS (void success)
    let expr = Expr::Return(Box::new(Expr::Ident("Ok".to_string())));
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "return");
}

#[test]
fn lower_await() {
    let expr = Expr::Await(Box::new(Expr::Ident("fetch_user".to_string())));
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "await fetchUser");
}

#[test]
fn lower_try_as_await() {
    // VEIL `?` operator → TS `await` (errors throw)
    let expr = Expr::Try(Box::new(Expr::Ident("api_call".to_string())));
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "await apiCall");
}

#[test]
fn lower_require_null_check() {
    let expr = Expr::Require(Box::new(Expr::Ident("user".to_string())));
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // Should contain null check throw pattern
    assert!(output.contains("throw"), "require should produce throw: {}", output);
    assert!(output.contains("NotFound"), "require should mention NotFound: {}", output);
}

#[test]
fn lower_break() {
    let expr = Expr::Break;
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "break");
}

#[test]
fn lower_continue() {
    let expr = Expr::Continue;
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "continue");
}

// ─── Type Mapping Tests ──────────────────────────────────────────────────────

#[test]
fn type_mapping_str() {
    let ty = veil_type_to_ts(&TypeExpr::Named("Str".to_string()));
    assert_eq!(ty.to_ts(), "string");
}

#[test]
fn type_mapping_int() {
    let ty = veil_type_to_ts(&TypeExpr::Named("Int".to_string()));
    assert_eq!(ty.to_ts(), "number");
}

#[test]
fn type_mapping_bool() {
    let ty = veil_type_to_ts(&TypeExpr::Named("Bool".to_string()));
    assert_eq!(ty.to_ts(), "boolean");
}

#[test]
fn type_mapping_optional() {
    let ty = veil_type_to_ts(&TypeExpr::Optional(
        Box::new(TypeExpr::Named("Str".to_string())),
    ));
    assert_eq!(ty.to_ts(), "string | null");
}

#[test]
fn type_mapping_result_to_promise() {
    let ty = veil_type_to_ts(&TypeExpr::Result(Some(
        Box::new(TypeExpr::Named("Int".to_string())),
    )));
    assert_eq!(ty.to_ts(), "Promise<number>");
}

#[test]
fn type_mapping_list_to_array() {
    let ty = veil_type_to_ts(&TypeExpr::List(
        Box::new(TypeExpr::Named("Str".to_string())),
    ));
    assert_eq!(ty.to_ts(), "string[]");
}

#[test]
fn type_mapping_map_to_record() {
    let ty = veil_type_to_ts(&TypeExpr::Map(
        Box::new(TypeExpr::Named("Str".to_string())),
        Box::new(TypeExpr::Named("Int".to_string())),
    ));
    assert_eq!(ty.to_ts(), "Record<string, number>");
}

#[test]
fn type_mapping_named_type() {
    let ty = veil_type_to_ts(&TypeExpr::Named("UserProfile".to_string()));
    assert_eq!(ty.to_ts(), "UserProfile");
}

#[test]
fn type_mapping_fn_ptr() {
    let ty = veil_type_to_ts(&TypeExpr::FnPtr(
        vec![TypeExpr::Named("Str".to_string()), TypeExpr::Named("Int".to_string())],
        Some(Box::new(TypeExpr::Named("Bool".to_string()))),
    ));
    assert_eq!(ty.to_ts(), "(arg0: string, arg1: number) => boolean");
}

// ─── camelCase Helper Tests ──────────────────────────────────────────────────

#[test]
fn camel_case_basic() {
    assert_eq!(to_camel_case("user_name"), "userName");
    assert_eq!(to_camel_case("get_by_id"), "getById");
}

#[test]
fn camel_case_passthrough() {
    assert_eq!(to_camel_case("name"), "name");
    assert_eq!(to_camel_case("userName"), "userName");
}

// ─── Fallback Tests ──────────────────────────────────────────────────────────

#[test]
fn lower_unhandled_falls_to_raw() {
    // Range has no native TS equivalent — uses Raw fallback
    let expr = Expr::Range {
        start: Some(Box::new(Expr::IntLit(0))),
        end: Some(Box::new(Expr::IntLit(10))),
        inclusive: false,
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    // The Raw path uses expr_to_ts which renders range expressions
    match &ts {
        TsExpr::Raw(_) => {} // correct
        other => panic!("Expected Raw, got {:?}", other),
    }
}

// ─── Batch 6: Control Flow Lowering Tests ────────────────────────────────────

use veil_ir::Span;

#[test]
fn lower_if_expr_no_else() {
    let expr = Expr::IfExpr(IfExprData {
        condition: Box::new(Expr::Ident("is_active".to_string())),
        then_body: vec![Expr::Return(Box::new(Expr::IntLit(1)))],
        else_body: None,
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "if (isActive) {\n  return 1;\n}");
}

#[test]
fn lower_if_expr_with_else() {
    let expr = Expr::IfExpr(IfExprData {
        condition: Box::new(Expr::BinaryOp(BinaryOpExpr {
            left: Box::new(Expr::Ident("count".to_string())),
            op: BinOp::Gt,
            right: Box::new(Expr::IntLit(0)),
        })),
        then_body: vec![Expr::Return(Box::new(Expr::StringLit("yes".to_string())))],
        else_body: Some(vec![Expr::Return(Box::new(Expr::StringLit("no".to_string())))]),
    });
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("if (count > 0)"), "output: {}", output);
    assert!(output.contains("return \"yes\""), "output: {}", output);
    assert!(output.contains("} else {"), "output: {}", output);
    assert!(output.contains("return \"no\""), "output: {}", output);
}

#[test]
fn lower_match_to_switch() {
    let expr = Expr::Match(
        Box::new(Expr::Ident("status".to_string())),
        vec![
            MatchArm {
                pattern: "active".to_string(),
                rich_pattern: None,
                guard: None,
                span: Span::new(0, 0),
                body: vec![Expr::Return(Box::new(Expr::IntLit(1)))],
            },
            MatchArm {
                pattern: "inactive".to_string(),
                rich_pattern: None,
                guard: None,
                span: Span::new(0, 0),
                body: vec![Expr::Return(Box::new(Expr::IntLit(0)))],
            },
            MatchArm {
                pattern: "_".to_string(),
                rich_pattern: None,
                guard: None,
                span: Span::new(0, 0),
                body: vec![Expr::Return(Box::new(Expr::IntLit(-1)))],
            },
        ],
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("switch (status)"), "output: {}", output);
    assert!(output.contains("case \"active\":"), "output: {}", output);
    assert!(output.contains("case \"inactive\":"), "output: {}", output);
    assert!(output.contains("default:"), "output: {}", output);
    assert!(output.contains("return -1"), "output: {}", output);
}

#[test]
fn lower_match_wildcard_rich_pattern() {
    let expr = Expr::Match(
        Box::new(Expr::Ident("val".to_string())),
        vec![
            MatchArm {
                pattern: "some_val".to_string(),
                rich_pattern: None,
                guard: None,
                span: Span::new(0, 0),
                body: vec![Expr::IntLit(1)],
            },
            MatchArm {
                pattern: "wild".to_string(),
                rich_pattern: Some(Pattern::Wildcard),
                guard: None,
                span: Span::new(0, 0),
                body: vec![Expr::IntLit(0)],
            },
        ],
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // Rich pattern Wildcard should be the default arm
    assert!(output.contains("default:"), "output: {}", output);
    assert!(output.contains("case \"some_val\":"), "output: {}", output);
}

#[test]
fn lower_for_loop_simple() {
    let expr = Expr::ForLoop {
        binding: "item".to_string(),
        index: None,
        iterable: Box::new(Expr::Ident("items".to_string())),
        body: vec![Expr::Return(Box::new(Expr::Ident("item".to_string())))],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("for (const item of items)"), "output: {}", output);
    assert!(output.contains("return item"), "output: {}", output);
}

#[test]
fn lower_for_loop_with_index() {
    let expr = Expr::ForLoop {
        binding: "user_item".to_string(),
        index: Some("idx".to_string()),
        iterable: Box::new(Expr::Ident("user_list".to_string())),
        body: vec![Expr::Break],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("for (let idx = 0; idx < userList.length; idx++)"), "output: {}", output);
    assert!(output.contains("const userItem = userList[idx]"), "output: {}", output);
    assert!(output.contains("break"), "output: {}", output);
}

#[test]
fn lower_while_loop() {
    let expr = Expr::WhileLoop {
        condition: Box::new(Expr::BoolLit(true)),
        body: vec![Expr::Continue],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "while (true) {\n  continue;\n}");
}

#[test]
fn lower_infinite_loop() {
    let expr = Expr::Loop(vec![
        Expr::Break,
    ]);
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "while (true) {\n  break;\n}");
}

#[test]
fn lower_do_block_iife() {
    let expr = Expr::DoBlock(vec![
        Expr::Assign("x".to_string(), Box::new(Expr::IntLit(1)), None),
        Expr::Return(Box::new(Expr::Ident("x".to_string()))),
    ]);
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // DoBlock → IIFE wrapping an arrow fn
    assert!(output.contains("() =>"), "output: {}", output);
    assert!(output.contains("const x = 1"), "output: {}", output);
    assert!(output.contains("return x"), "output: {}", output);
}

// ─── Batch 7: Collections Lowering Tests ─────────────────────────────────────

#[test]
fn lower_array_lit() {
    let expr = Expr::ArrayLit(vec![Expr::IntLit(1), Expr::IntLit(2), Expr::IntLit(3)]);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "[1, 2, 3]");
}

#[test]
fn lower_array_lit_empty() {
    let expr = Expr::ArrayLit(vec![]);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "[]");
}

#[test]
fn lower_tuple_as_array() {
    let expr = Expr::Tuple(vec![
        Expr::StringLit("hello".to_string()),
        Expr::IntLit(42),
    ]);
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "[\"hello\", 42]");
}

#[test]
fn lower_struct_lit_to_object() {
    let expr = Expr::StructLit(
        "User".to_string(),
        vec![
            ("first_name".to_string(), Expr::StringLit("Alice".to_string())),
            ("is_active".to_string(), Expr::BoolLit(true)),
        ],
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // Fields should be camelCase
    assert!(output.contains("firstName: \"Alice\""), "output: {}", output);
    assert!(output.contains("isActive: true"), "output: {}", output);
}

#[test]
fn lower_struct_lit_shorthand() {
    // When field value is an Ident matching the camelCase field name, emit shorthand
    let expr = Expr::StructLit(
        "Config".to_string(),
        vec![
            ("name".to_string(), Expr::Ident("name".to_string())),
        ],
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // ObjectLit emit detects key == ident name → shorthand
    assert_eq!(output, "{ name }");
}

#[test]
fn lower_struct_update_spread() {
    let expr = Expr::StructUpdate {
        name: "Config".to_string(),
        fields: vec![
            ("port".to_string(), Expr::IntLit(8080)),
        ],
        base: Box::new(Expr::Ident("default_config".to_string())),
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("...defaultConfig"), "output: {}", output);
    assert!(output.contains("port: 8080"), "output: {}", output);
}

#[test]
fn lower_index_access() {
    let expr = Expr::Index(
        Box::new(Expr::Ident("items".to_string())),
        Box::new(Expr::IntLit(0)),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "items[0]");
}

#[test]
fn lower_index_access_expr() {
    let expr = Expr::Index(
        Box::new(Expr::Ident("matrix".to_string())),
        Box::new(Expr::Ident("row_idx".to_string())),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "matrix[rowIdx]");
}

// ─── Batch 8: Additional Wrappers Lowering Tests ─────────────────────────────

#[test]
fn lower_cast_type_assertion() {
    let expr = Expr::Cast(
        Box::new(Expr::Ident("value".to_string())),
        "Str".to_string(),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "value as string");
}

#[test]
fn lower_cast_custom_type() {
    let expr = Expr::Cast(
        Box::new(Expr::Ident("obj".to_string())),
        "UserProfile".to_string(),
    );
    let ts = lower_to_ts(&expr, &test_ctx());
    assert_eq!(emit_ts(&ts), "obj as UserProfile");
}

#[test]
fn lower_closure_sync() {
    let expr = Expr::Closure {
        params: vec!["user_item".to_string(), "idx".to_string()],
        body: vec![Expr::Return(Box::new(Expr::Ident("user_item".to_string())))],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("(userItem, idx) =>"), "output: {}", output);
    assert!(output.contains("return userItem"), "output: {}", output);
    // Should NOT be async
    assert!(!output.contains("async"), "output: {}", output);
}

#[test]
fn lower_closure_async_detected() {
    // Closure body contains Await → async arrow fn
    let expr = Expr::Closure {
        params: vec!["id".to_string()],
        body: vec![Expr::Await(Box::new(Expr::Ident("fetch_user".to_string())))],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("async"), "Should be async: {}", output);
    assert!(output.contains("(id) =>"), "output: {}", output);
    assert!(output.contains("await fetchUser"), "output: {}", output);
}

#[test]
fn lower_closure_async_try_detected() {
    // Closure body contains Try (which lowers to Await) → async
    let expr = Expr::Closure {
        params: vec!["x".to_string()],
        body: vec![Expr::Try(Box::new(Expr::Ident("api_call".to_string())))],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("async"), "Try should trigger async: {}", output);
}

#[test]
fn lower_closure_single_expr_shorthand() {
    // Single expression body → shorthand arrow syntax
    let expr = Expr::Closure {
        params: vec!["x".to_string()],
        body: vec![Expr::BinaryOp(BinaryOpExpr {
            left: Box::new(Expr::Ident("x".to_string())),
            op: BinOp::Mul,
            right: Box::new(Expr::IntLit(2)),
        })],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "(x) => x * 2");
}

#[test]
fn lower_range_raw_fallback() {
    // Range has no native TS equivalent → Raw escape hatch
    let expr = Expr::Range {
        start: Some(Box::new(Expr::IntLit(0))),
        end: Some(Box::new(Expr::IntLit(10))),
        inclusive: false,
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    match &ts {
        TsExpr::Raw(_) => {} // correct
        other => panic!("Expected Raw for Range, got {:?}", other),
    }
}

#[test]
fn lower_if_let_null_check() {
    let expr = Expr::IfLet {
        pattern: "user".to_string(),
        expr: Box::new(Expr::Ident("maybe_user".to_string())),
        then_body: vec![Expr::Return(Box::new(Expr::Ident("maybe_user".to_string())))],
        else_body: None,
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // Should emit null check: if (maybeUser !== null)
    assert!(output.contains("maybeUser !== null"), "output: {}", output);
    assert!(output.contains("return maybeUser"), "output: {}", output);
}

#[test]
fn lower_if_let_with_else() {
    let expr = Expr::IfLet {
        pattern: "val".to_string(),
        expr: Box::new(Expr::Ident("opt_val".to_string())),
        then_body: vec![Expr::Return(Box::new(Expr::Ident("opt_val".to_string())))],
        else_body: Some(vec![Expr::Return(Box::new(Expr::Ident("null".to_string())))]),
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("optVal !== null"), "output: {}", output);
    assert!(output.contains("} else {"), "output: {}", output);
    assert!(output.contains("return null"), "output: {}", output);
}

#[test]
fn lower_while_let_null_check() {
    let expr = Expr::WhileLet {
        pattern: "item".to_string(),
        expr: Box::new(Expr::Ident("next_item".to_string())),
        body: vec![Expr::Continue],
    };
    let ts = lower_to_ts(&expr, &test_ctx());
    let output = emit_ts(&ts);
    // Should emit: while (nextItem !== null) { continue; }
    assert!(output.contains("nextItem !== null"), "output: {}", output);
    assert!(output.contains("continue"), "output: {}", output);
}

// ─── lower_block Helper Test ─────────────────────────────────────────────────

#[test]
fn lower_block_multiple_stmts() {
    let body = vec![
        Expr::Assign("x".to_string(), Box::new(Expr::IntLit(1)), None),
        Expr::Assign("y".to_string(), Box::new(Expr::IntLit(2)), None),
        Expr::Return(Box::new(Expr::BinaryOp(BinaryOpExpr {
            left: Box::new(Expr::Ident("x".to_string())),
            op: BinOp::Add,
            right: Box::new(Expr::Ident("y".to_string())),
        }))),
    ];
    let result = lower_block(&body, &test_ctx());
    assert_eq!(result.len(), 3);
    assert_eq!(emit_ts(&result[0]), "const x = 1");
    assert_eq!(emit_ts(&result[1]), "const y = 2");
    assert_eq!(emit_ts(&result[2]), "return x + y");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Batch 9 + 10: Call & Action Lowering Tests
// ═══════════════════════════════════════════════════════════════════════════════

use veil_ir::ast::{ActionExpr, CallExpr};
use veil_ir::layer::{Shape, StmtShape};
use std::collections::HashSet;

/// Helper: create a GenCtx with trait shapes registered.
fn ctx_with_trait(name: &str) -> GenCtx {
    let mut shapes = HashMap::new();
    shapes.insert(name.to_string(), Shape::Trait);
    GenCtx::new(shapes)
}

/// Helper: create a GenCtx with a struct shape registered.
fn ctx_with_struct(name: &str) -> GenCtx {
    let mut shapes = HashMap::new();
    shapes.insert(name.to_string(), Shape::Struct);
    GenCtx::new(shapes)
}

/// Helper: create a GenCtx with routing traits configured.
fn ctx_with_routing(trait_name: &str) -> GenCtx {
    let mut shapes = HashMap::new();
    shapes.insert(trait_name.to_string(), Shape::Trait);
    let mut ctx = GenCtx::new(shapes);
    ctx.routing.routing_traits.insert(trait_name.to_string());
    ctx.routing.envelope_routing = true;
    ctx
}

// ─── Trait Dependency Calls ──────────────────────────────────────────────────

#[test]
fn lower_trait_dep_call_basic() {
    let call = CallExpr {
        target: "CustomerRepo".to_string(),
        method: "find".to_string(),
        args: vec![Expr::Ident("id".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ctx = ctx_with_trait("CustomerRepo");
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    assert_eq!(output, "await deps.customerRepo.find(id)");
}

#[test]
fn lower_trait_dep_call_bang_stripped() {
    let call = CallExpr {
        target: "OrderRepo".to_string(),
        method: "save!".to_string(),
        args: vec![Expr::Ident("order".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ctx = ctx_with_trait("OrderRepo");
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    // Bang stripped — TS doesn't have it
    assert_eq!(output, "await deps.orderRepo.save(order)");
}

#[test]
fn lower_trait_dep_call_with_dep_field() {
    let call = CallExpr {
        target: "NotificationService".to_string(),
        method: "send".to_string(),
        args: vec![Expr::Ident("msg".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let mut ctx = ctx_with_trait("NotificationService");
    ctx.dep_fields.insert("NotificationService".to_string(), "notifier".to_string());
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    assert_eq!(output, "await deps.notifier.send(msg)");
}

// ─── Struct Constructor Calls ────────────────────────────────────────────────

#[test]
fn lower_struct_ctor_with_fields() {
    let call = CallExpr {
        target: "Order".to_string(),
        method: "new".to_string(),
        args: vec![
            Expr::Ident("id".to_string()),
            Expr::Ident("customer_id".to_string()),
            Expr::IntLit(100),
        ],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let mut ctx = ctx_with_struct("Order");
    ctx.types.struct_fields.insert(
        "Order".to_string(),
        vec![
            ("id".to_string(), "Str".to_string()),
            ("customer_id".to_string(), "Str".to_string()),
            ("total".to_string(), "Int".to_string()),
        ],
    );
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    assert_eq!(output, "{ id, customerId, total: 100 }");
}

#[test]
fn lower_struct_ctor_no_field_metadata() {
    let call = CallExpr {
        target: "Point".to_string(),
        method: "new".to_string(),
        args: vec![Expr::IntLit(1), Expr::IntLit(2)],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ctx = ctx_with_struct("Point");
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    // Without field metadata, falls back to field0, field1
    assert_eq!(output, "{ field0: 1, field1: 2 }");
}

// ─── Receiver Method Calls ───────────────────────────────────────────────────

#[test]
fn lower_clone_stripped() {
    let call = CallExpr {
        target: String::new(),
        method: "clone".to_string(),
        args: vec![],
        receiver: Some(Box::new(Expr::Ident("user".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "user");
}

#[test]
fn lower_to_owned_stripped() {
    let call = CallExpr {
        target: String::new(),
        method: "to_owned".to_string(),
        args: vec![],
        receiver: Some(Box::new(Expr::Ident("name".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "name");
}

#[test]
fn lower_is_some_to_not_null() {
    let call = CallExpr {
        target: String::new(),
        method: "is_some".to_string(),
        args: vec![],
        receiver: Some(Box::new(Expr::Ident("maybe_user".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "maybeUser !== null");
}

#[test]
fn lower_is_none_to_eq_null() {
    let call = CallExpr {
        target: String::new(),
        method: "is_none".to_string(),
        args: vec![],
        receiver: Some(Box::new(Expr::Ident("result".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "result === null");
}

#[test]
fn lower_unwrap_to_non_null_assertion() {
    let call = CallExpr {
        target: String::new(),
        method: "unwrap".to_string(),
        args: vec![],
        receiver: Some(Box::new(Expr::Ident("value".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "value!");
}

#[test]
fn lower_len_to_length() {
    let call = CallExpr {
        target: String::new(),
        method: "len".to_string(),
        args: vec![],
        receiver: Some(Box::new(Expr::Ident("items".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "items.length");
}

#[test]
fn lower_contains_to_includes() {
    let call = CallExpr {
        target: String::new(),
        method: "contains".to_string(),
        args: vec![Expr::StringLit("foo".to_string())],
        receiver: Some(Box::new(Expr::Ident("tags".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "tags.includes(\"foo\")");
}

#[test]
fn lower_unwrap_or_to_nullish_coalesce() {
    let call = CallExpr {
        target: String::new(),
        method: "unwrap_or".to_string(),
        args: vec![Expr::StringLit("default".to_string())],
        receiver: Some(Box::new(Expr::Ident("maybe_name".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "maybeName ?? \"default\"");
}

#[test]
fn lower_push_method() {
    let call = CallExpr {
        target: String::new(),
        method: "push".to_string(),
        args: vec![Expr::Ident("item".to_string())],
        receiver: Some(Box::new(Expr::Ident("list".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "list.push(item)");
}

#[test]
fn lower_filter_with_closure() {
    let call = CallExpr {
        target: String::new(),
        method: "filter".to_string(),
        args: vec![Expr::Closure {
            params: vec!["x".to_string()],
            body: vec![Expr::FieldAccess(
                Box::new(Expr::Ident("x".to_string())),
                "active".to_string(),
            )],
        }],
        receiver: Some(Box::new(Expr::Ident("items".to_string()))),
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "items.filter((x) => x.active)");
}

// ─── Bus / Routing Calls ─────────────────────────────────────────────────────

#[test]
fn lower_routing_invoke_with_struct_lit() {
    let call = CallExpr {
        target: "Bus".to_string(),
        method: "invoke".to_string(),
        args: vec![Expr::StructLit(
            "ProcessOrder".to_string(),
            vec![
                ("order_id".to_string(), Expr::Ident("order_id".to_string())),
                ("amount".to_string(), Expr::IntLit(500)),
            ],
        )],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ctx = ctx_with_routing("Bus");
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    assert_eq!(
        output,
        "await deps.bus.invoke(\"ProcessOrder\", { orderId, amount: 500 })"
    );
}

#[test]
fn lower_routing_dispatch_plain() {
    let call = CallExpr {
        target: "EventBus".to_string(),
        method: "dispatch".to_string(),
        args: vec![Expr::Ident("event".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ctx = ctx_with_routing("EventBus");
    let ts = lower_to_ts(&Expr::Call(call), &ctx);
    let output = emit_ts(&ts);
    assert_eq!(output, "await deps.bus.dispatch(\"EventBus\", event)");
}

// ─── Builtin Calls ───────────────────────────────────────────────────────────

#[test]
fn lower_id_new() {
    let call = CallExpr {
        target: "Id".to_string(),
        method: "new".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "crypto.randomUUID()");
}

#[test]
fn lower_uuid_new() {
    let call = CallExpr {
        target: "UUID".to_string(),
        method: "new".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "crypto.randomUUID()");
}

#[test]
fn lower_str_now_iso8601() {
    let call = CallExpr {
        target: "Str".to_string(),
        method: "now_iso8601".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "new Date().toISOString()");
}

#[test]
fn lower_int_now_unix() {
    let call = CallExpr {
        target: "Int".to_string(),
        method: "now_unix".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "Math.floor(Date.now() / 1000)");
}

#[test]
fn lower_json_parse() {
    let call = CallExpr {
        target: "Json".to_string(),
        method: "parse".to_string(),
        args: vec![Expr::Ident("raw_str".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "JSON.parse(rawStr)");
}

#[test]
fn lower_json_stringify() {
    let call = CallExpr {
        target: "Json".to_string(),
        method: "stringify".to_string(),
        args: vec![Expr::Ident("data".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "JSON.stringify(data)");
}

#[test]
fn lower_dt_now() {
    let call = CallExpr {
        target: "Dt".to_string(),
        method: "now".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "new Date()");
}

#[test]
fn lower_map_new() {
    let call = CallExpr {
        target: "Map".to_string(),
        method: "new".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "new Map()");
}

#[test]
fn lower_list_new() {
    let call = CallExpr {
        target: "List".to_string(),
        method: "new".to_string(),
        args: vec![],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "[]");
}

// ─── Action Lowering Tests ───────────────────────────────────────────────────

#[test]
fn lower_guard_action() {
    let action = ActionExpr {
        keyword: "guard".to_string(),
        shape: StmtShape::If,
        target: String::new(),
        method: String::new(),
        args: vec![],
        named_args: vec![],
        condition: Some(Box::new(Expr::BinaryOp(BinaryOpExpr {
            left: Box::new(Expr::Ident("amount".to_string())),
            op: BinOp::Gt,
            right: Box::new(Expr::IntLit(0)),
        }))),
        message: Some("amount must be positive".to_string()),
        result_binding: None,
        body: vec![],
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Action(action), &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("!(amount > 0)"), "output: {}", output);
    assert!(output.contains("throw new Error(\"amount must be positive\")"), "output: {}", output);
}

#[test]
fn lower_guard_action_with_args() {
    let action = ActionExpr {
        keyword: "guard".to_string(),
        shape: StmtShape::If,
        target: String::new(),
        method: String::new(),
        args: vec![
            Expr::Ident("is_valid".to_string()),
            Expr::StringLit("validation failed".to_string()),
        ],
        named_args: vec![],
        condition: None,
        message: None,
        result_binding: None,
        body: vec![],
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Action(action), &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("!(isValid)"), "output: {}", output);
    assert!(output.contains("throw new Error(\"validation failed\")"), "output: {}", output);
}

#[test]
fn lower_dispatch_action() {
    let action = ActionExpr {
        keyword: "dispatch".to_string(),
        shape: StmtShape::Call,
        target: "OrderCreated".to_string(),
        method: String::new(),
        args: vec![],
        named_args: vec![
            ("order_id".to_string(), Expr::Ident("id".to_string())),
        ],
        condition: None,
        message: None,
        result_binding: None,
        body: vec![],
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Action(action), &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("await"), "output: {}", output);
    assert!(output.contains("deps.bus"), "output: {}", output);
    assert!(output.contains("dispatch"), "output: {}", output);
}

#[test]
fn lower_action_with_result_binding() {
    let action = ActionExpr {
        keyword: "invoke".to_string(),
        shape: StmtShape::Call,
        target: "ProcessPayment".to_string(),
        method: String::new(),
        args: vec![Expr::Ident("payment".to_string())],
        named_args: vec![],
        condition: None,
        message: None,
        result_binding: Some("result".to_string()),
        body: vec![],
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Action(action), &test_ctx());
    let output = emit_ts(&ts);
    assert!(output.contains("const result"), "output: {}", output);
    assert!(output.contains("await"), "output: {}", output);
}

// ─── Target method calls (local var methods) ─────────────────────────────────

#[test]
fn lower_target_method_call() {
    let call = CallExpr {
        target: "items".to_string(),
        method: "map".to_string(),
        args: vec![Expr::Closure {
            params: vec!["x".to_string()],
            body: vec![Expr::FieldAccess(
                Box::new(Expr::Ident("x".to_string())),
                "name".to_string(),
            )],
        }],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "items.map((x) => x.name)");
}

// ─── Free function calls ─────────────────────────────────────────────────────

#[test]
fn lower_free_function_call() {
    let call = CallExpr {
        target: "process_data".to_string(),
        method: String::new(),
        args: vec![Expr::Ident("input".to_string())],
        receiver: None,
        sugar: None,
        span: Span::new(0, 0),
    };
    let ts = lower_to_ts(&Expr::Call(call), &test_ctx());
    let output = emit_ts(&ts);
    assert_eq!(output, "processData(input)");
}

// ─── Svelte Integration Tests (Session 8) ────────────────────────────────────

#[test]
fn svelte_component_default_path() {
    use super::components::{gen_svelte_component, SvelteFile};
    use veil_ir::ast::{Construct, Field, NamedBlock};
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, ReactivityPolicy, Shape};

    let mut registry = LayerRegistry::default();
    registry.reactivity_policy = ReactivityPolicy {
        props_call: "$props()".to_string(),
        state_line: "let {name} = $state<{type}>({default})".to_string(),
        ..Default::default()
    };

    let mut comp = Construct::new("component", "Component", Shape::Struct, "UserCard".to_string(), Span::default());
    comp.blocks.push(NamedBlock {
        keyword: "props".to_string(),
        shape: Shape::Struct,
        name: None,
        fields: vec![Field {
            name: "name".to_string(),
            type_expr: veil_ir::TypeExpr::Named("Str".to_string()),
            default_expr: None,
            annotations: vec![],
            span: Span::default(),
        }],
        variants: vec![],
        transitions: vec![],
        span: Span::default(),
    });
    comp.raw_blocks.push(("template".to_string(), "<h1>{name}</h1>".to_string()));

    let result = gen_svelte_component(&comp, &registry);
    assert_eq!(result.path, "src/lib/components/UserCard.svelte");
    assert!(result.content.contains("<script lang=\"ts\">"));
    assert!(result.content.contains("name: string;"));
    assert!(result.content.contains("$props()"));
    assert!(result.content.contains("<h1>{name}</h1>"));
}

#[test]
fn sveltekit_page_route_path() {
    use super::components::sveltekit_output_path;
    use veil_ir::ast::{Annotation, Construct};
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, Shape};

    let registry = LayerRegistry::default();

    // Page with no explicit route → derives from name
    let mut page = Construct::new("page", "Page", Shape::Struct, "Dashboard".to_string(), Span::default());
    page.subkind = "Page".to_string();
    let path = sveltekit_output_path(&page, &registry);
    assert_eq!(path, "src/routes/dashboard/+page.svelte");
}

#[test]
fn sveltekit_page_with_route_annotation() {
    use super::components::sveltekit_output_path;
    use veil_ir::ast::{Annotation, Construct};
    use veil_ir::Span;
    use veil_ir::layer::{AnnotationSpec, ConstructSpec, LayerRegistry, Shape, Visual};

    let mut registry = LayerRegistry::default();
    // Register @route as ui_route role via a ConstructSpec (annotation_has_role walks constructs)
    let mut spec = ConstructSpec {
        keyword: "page".to_string(),
        name: "Page".to_string(),
        maps_to: "struct".to_string(),
        shape: Shape::Struct,
        layer: "svelte5".to_string(),
        desc: String::new(),
        contains: Vec::new(),
        blocks: Vec::new(),
        raw_block_keywords: Vec::new(),
        constraints: Vec::new(),
        allowed_in: "any".to_string(),
        group: String::new(),
        visual: Visual { icon: String::new(), color: String::new(), label: String::new() },
        au: false,
        is_step: false,
        step_fields: Vec::new(),
        annotations: vec![AnnotationSpec {
            name: "route".to_string(),
            roles: vec!["ui_route".to_string()],
            desc: String::new(),
            params: vec![],
        }],
        runtime: None,
        tgt: String::new(),
        dg: String::new(),
        presentation: Default::default(),
        roles: Vec::new(),
        config_keys: Vec::new(),
        required_fields: Vec::new(),
        lowers_to: std::collections::HashMap::new(),
    };
    registry.constructs.push(spec);

    let mut page = Construct::new("page", "Page", Shape::Struct, "PullRequests".to_string(), Span::default());
    page.subkind = "Page".to_string();
    page.annotations.push(Annotation {
        name: "route".to_string(),
        args: vec!["\"/pulls\"".to_string()],
        span: Span::default(),
    });

    let path = sveltekit_output_path(&page, &registry);
    assert_eq!(path, "src/routes/pulls/+page.svelte");
}

#[test]
fn sveltekit_layout_path() {
    use super::components::sveltekit_output_path;
    use veil_ir::ast::Construct;
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, Shape};

    let registry = LayerRegistry::default();

    let mut layout = Construct::new("layout", "Layout", Shape::Struct, "RootLayout".to_string(), Span::default());
    layout.subkind = "Layout".to_string();
    let path = sveltekit_output_path(&layout, &registry);
    assert_eq!(path, "src/routes/+layout.svelte");
}

#[test]
fn sveltekit_store_path() {
    use super::components::sveltekit_output_path;
    use veil_ir::ast::Construct;
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, Shape};

    let registry = LayerRegistry::default();

    let mut store = Construct::new("store", "Store", Shape::Struct, "AuthStore".to_string(), Span::default());
    store.subkind = "Store".to_string();
    let path = sveltekit_output_path(&store, &registry);
    assert_eq!(path, "src/lib/stores/auth_store.svelte.ts");
}

#[test]
fn svelte_store_generates_state_runes() {
    use super::components::gen_svelte_store;
    use veil_ir::ast::{Construct, Field, NamedBlock};
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, Shape};

    let registry = LayerRegistry::default();

    let mut store = Construct::new("store", "Store", Shape::Struct, "AuthStore".to_string(), Span::default());
    store.subkind = "Store".to_string();
    store.blocks.push(NamedBlock {
        keyword: "state".to_string(),
        shape: Shape::Struct,
        name: None,
        fields: vec![
            Field {
                name: "token".to_string(),
                type_expr: veil_ir::TypeExpr::Optional(Box::new(veil_ir::TypeExpr::Named("Str".to_string()))),
                default_expr: None,
                annotations: vec![],
                span: Span::default(),
            },
            Field {
                name: "is_authenticated".to_string(),
                type_expr: veil_ir::TypeExpr::Named("Bool".to_string()),
                default_expr: None,
                annotations: vec![],
                span: Span::default(),
            },
        ],
        variants: vec![],
        transitions: vec![],
        span: Span::default(),
    });

    let result = gen_svelte_store(&store, &registry);
    assert_eq!(result.path, "src/lib/stores/auth_store.svelte.ts");
    assert!(result.content.contains("export let token = $state<string | null>(null);"));
    assert!(result.content.contains("export let is_authenticated = $state<boolean>(false);"));
}

#[test]
fn svelte_component_at_custom_path() {
    use super::components::gen_svelte_component_at;
    use veil_ir::ast::Construct;
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, ReactivityPolicy, Shape};

    let mut registry = LayerRegistry::default();
    registry.reactivity_policy = ReactivityPolicy {
        props_call: "$props()".to_string(),
        state_line: "let {name} = $state<{type}>({default})".to_string(),
        ..Default::default()
    };

    let mut comp = Construct::new("page", "Page", Shape::Struct, "Dashboard".to_string(), Span::default());
    comp.raw_blocks.push(("template".to_string(), "<h1>Dashboard</h1>".to_string()));

    let result = gen_svelte_component_at(&comp, &registry, "src/routes/+page.svelte");
    assert_eq!(result.path, "src/routes/+page.svelte");
    assert!(result.content.contains("<h1>Dashboard</h1>"));
}

#[test]
fn svelte_page_with_props_state_template_style() {
    use super::components::gen_svelte_component_at;
    use veil_ir::ast::{Construct, Expr, Field, NamedBlock};
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, ReactivityPolicy, Shape};

    let mut registry = LayerRegistry::default();
    registry.reactivity_policy = ReactivityPolicy {
        props_call: "$props()".to_string(),
        state_line: "let {name} = $state<{type}>({default})".to_string(),
        derived_line: "let {name} = $derived({expr})".to_string(),
        effect_sync: "$effect(() => {{ // {name}\n{body}\n  }})".to_string(),
        effect_async: String::new(),
        bindable: String::new(),
        bindable_default: String::new(),
    };

    let mut page = Construct::new("page", "Page", Shape::Struct, "UserProfile".to_string(), Span::default());
    page.subkind = "Page".to_string();

    // Props
    page.blocks.push(NamedBlock {
        keyword: "props".to_string(),
        shape: Shape::Struct,
        name: None,
        fields: vec![Field {
            name: "user_id".to_string(),
            type_expr: veil_ir::TypeExpr::Named("Str".to_string()),
            default_expr: None,
            annotations: vec![],
            span: Span::default(),
        }],
        variants: vec![],
        transitions: vec![],
        span: Span::default(),
    });

    // State
    page.blocks.push(NamedBlock {
        keyword: "state".to_string(),
        shape: Shape::Struct,
        name: None,
        fields: vec![Field {
            name: "loading".to_string(),
            type_expr: veil_ir::TypeExpr::Named("Bool".to_string()),
            default_expr: Some(Expr::BoolLit(true)),
            annotations: vec![],
            span: Span::default(),
        }],
        variants: vec![],
        transitions: vec![],
        span: Span::default(),
    });

    page.raw_blocks.push(("template".to_string(), "<div class=\"profile\">\n  <p>{user_id}</p>\n</div>".to_string()));
    page.raw_blocks.push(("style".to_string(), ".profile { padding: 1rem; }".to_string()));

    let result = gen_svelte_component_at(&page, &registry, "src/routes/profile/+page.svelte");
    assert_eq!(result.path, "src/routes/profile/+page.svelte");
    assert!(result.content.contains("<script lang=\"ts\">"));
    assert!(result.content.contains("user_id: string;"));
    assert!(result.content.contains("$props()"));
    assert!(result.content.contains("let loading = $state<boolean>(true);"));
    assert!(result.content.contains("<div class=\"profile\">"));
    assert!(result.content.contains("<style>"));
    assert!(result.content.contains(".profile"));
    assert!(result.content.contains("padding: 1rem"));
}

#[test]
fn generate_ts_ir_includes_svelte_page() {
    use super::generate::generate_ts_ir;
    use veil_ir::ast::{Annotation, Construct, Solution, TopLevelItem, NamedBlock, Field};
    use veil_ir::Span;
    use veil_ir::layer::{AnnotationSpec, ConstructSpec, LayerRegistry, ReactivityPolicy, Shape, Visual};

    let mut registry = LayerRegistry::default();
    registry.reactivity_policy = ReactivityPolicy {
        props_call: "$props()".to_string(),
        state_line: "let {name} = $state<{type}>({default})".to_string(),
        ..Default::default()
    };
    // Register @route annotation with ui_route role
    registry.constructs.push(ConstructSpec {
        keyword: "page".to_string(),
        name: "Page".to_string(),
        maps_to: "struct".to_string(),
        shape: Shape::Struct,
        layer: "svelte5".to_string(),
        desc: String::new(),
        contains: Vec::new(),
        blocks: Vec::new(),
        raw_block_keywords: Vec::new(),
        constraints: Vec::new(),
        allowed_in: "any".to_string(),
        group: String::new(),
        visual: Visual { icon: String::new(), color: String::new(), label: String::new() },
        au: false,
        is_step: false,
        step_fields: Vec::new(),
        annotations: vec![AnnotationSpec {
            name: "route".to_string(),
            roles: vec!["ui_route".to_string()],
            desc: String::new(),
            params: vec![],
        }],
        runtime: None,
        tgt: String::new(),
        dg: String::new(),
        presentation: Default::default(),
        roles: Vec::new(),
        config_keys: Vec::new(),
        required_fields: Vec::new(),
        lowers_to: std::collections::HashMap::new(),
    });

    // Create a solution with a module containing a page construct
    let mut page = Construct::new("page", "Page", Shape::Struct, "Home".to_string(), Span::default());
    page.subkind = "Page".to_string();
    page.annotations.push(Annotation {
        name: "route".to_string(),
        args: vec!["\"/\"".to_string()],
        span: Span::default(),
    });
    page.raw_blocks.push(("template".to_string(), "<h1>Home</h1>".to_string()));

    let mut module = Construct::new("mod", "", Shape::Mod, "ui".to_string(), Span::default());
    module.children.push(page);

    let solution = Solution {
        name: "test_app".to_string(),
        span: Span::default(),
        items: vec![TopLevelItem::Construct(module)],
        uses: vec![],
        links: vec![],
        expose: None,
        guidance: vec![],
    };

    let project = generate_ts_ir(&solution, &registry);

    // Should have a page file at the SvelteKit route path
    let page_file = project.files.iter().find(|f| f.path.contains("+page.svelte"));
    assert!(page_file.is_some(), "Expected a +page.svelte file, got paths: {:?}",
        project.files.iter().map(|f| &f.path).collect::<Vec<_>>());
    let pf = page_file.unwrap();
    assert_eq!(pf.path, "src/routes/+page.svelte");
    assert!(pf.content.contains("<h1>Home</h1>"));
}

#[test]
fn generate_ts_ir_includes_svelte_store() {
    use super::generate::generate_ts_ir;
    use veil_ir::ast::{Construct, Solution, TopLevelItem, NamedBlock, Field};
    use veil_ir::Span;
    use veil_ir::layer::{LayerRegistry, Shape};

    let registry = LayerRegistry::default();

    let mut store = Construct::new("store", "Store", Shape::Struct, "CartStore".to_string(), Span::default());
    store.subkind = "Store".to_string();
    store.blocks.push(NamedBlock {
        keyword: "state".to_string(),
        shape: Shape::Struct,
        name: None,
        fields: vec![Field {
            name: "items".to_string(),
            type_expr: veil_ir::TypeExpr::List(Box::new(veil_ir::TypeExpr::Named("Str".to_string()))),
            default_expr: None,
            annotations: vec![],
            span: Span::default(),
        }],
        variants: vec![],
        transitions: vec![],
        span: Span::default(),
    });

    let mut module = Construct::new("mod", "", Shape::Mod, "stores".to_string(), Span::default());
    module.children.push(store);

    let solution = Solution {
        name: "test_app".to_string(),
        span: Span::default(),
        items: vec![TopLevelItem::Construct(module)],
        uses: vec![],
        links: vec![],
        expose: None,
        guidance: vec![],
    };

    let project = generate_ts_ir(&solution, &registry);

    let store_file = project.files.iter().find(|f| f.path.contains(".svelte.ts"));
    assert!(store_file.is_some(), "Expected a .svelte.ts store file, got paths: {:?}",
        project.files.iter().map(|f| &f.path).collect::<Vec<_>>());
    let sf = store_file.unwrap();
    assert_eq!(sf.path, "src/lib/stores/cart_store.svelte.ts");
    assert!(sf.content.contains("$state<string[]>([])")); 
}

#[test]
fn sveltekit_page_root_route() {
    use super::components::sveltekit_output_path;
    use veil_ir::ast::{Annotation, Construct};
    use veil_ir::Span;
    use veil_ir::layer::{AnnotationSpec, ConstructSpec, LayerRegistry, Shape, Visual};

    let mut registry = LayerRegistry::default();
    registry.constructs.push(ConstructSpec {
        keyword: "page".to_string(),
        name: "Page".to_string(),
        maps_to: "struct".to_string(),
        shape: Shape::Struct,
        layer: "svelte5".to_string(),
        desc: String::new(),
        contains: Vec::new(),
        blocks: Vec::new(),
        raw_block_keywords: Vec::new(),
        constraints: Vec::new(),
        allowed_in: "any".to_string(),
        group: String::new(),
        visual: Visual { icon: String::new(), color: String::new(), label: String::new() },
        au: false,
        is_step: false,
        step_fields: Vec::new(),
        annotations: vec![AnnotationSpec {
            name: "route".to_string(),
            roles: vec!["ui_route".to_string()],
            desc: String::new(),
            params: vec![],
        }],
        runtime: None,
        tgt: String::new(),
        dg: String::new(),
        presentation: Default::default(),
        roles: Vec::new(),
        config_keys: Vec::new(),
        required_fields: Vec::new(),
        lowers_to: std::collections::HashMap::new(),
    });

    let mut page = Construct::new("page", "Page", Shape::Struct, "Home".to_string(), Span::default());
    page.subkind = "Page".to_string();
    page.annotations.push(Annotation {
        name: "route".to_string(),
        args: vec!["\"/\"".to_string()],
        span: Span::default(),
    });

    let path = sveltekit_output_path(&page, &registry);
    assert_eq!(path, "src/routes/+page.svelte");
}

#[test]
fn sveltekit_page_nested_route() {
    use super::components::sveltekit_output_path;
    use veil_ir::ast::{Annotation, Construct};
    use veil_ir::Span;
    use veil_ir::layer::{AnnotationSpec, ConstructSpec, LayerRegistry, Shape, Visual};

    let mut registry = LayerRegistry::default();
    registry.constructs.push(ConstructSpec {
        keyword: "page".to_string(),
        name: "Page".to_string(),
        maps_to: "struct".to_string(),
        shape: Shape::Struct,
        layer: "svelte5".to_string(),
        desc: String::new(),
        contains: Vec::new(),
        blocks: Vec::new(),
        raw_block_keywords: Vec::new(),
        constraints: Vec::new(),
        allowed_in: "any".to_string(),
        group: String::new(),
        visual: Visual { icon: String::new(), color: String::new(), label: String::new() },
        au: false,
        is_step: false,
        step_fields: Vec::new(),
        annotations: vec![AnnotationSpec {
            name: "route".to_string(),
            roles: vec!["ui_route".to_string()],
            desc: String::new(),
            params: vec![],
        }],
        runtime: None,
        tgt: String::new(),
        dg: String::new(),
        presentation: Default::default(),
        roles: Vec::new(),
        config_keys: Vec::new(),
        required_fields: Vec::new(),
        lowers_to: std::collections::HashMap::new(),
    });

    let mut page = Construct::new("page", "Page", Shape::Struct, "PullDetail".to_string(), Span::default());
    page.subkind = "Page".to_string();
    page.annotations.push(Annotation {
        name: "route".to_string(),
        args: vec!["\"/pulls/[id]\"".to_string()],
        span: Span::default(),
    });

    let path = sveltekit_output_path(&page, &registry);
    assert_eq!(path, "src/routes/pulls/[id]/+page.svelte");
}
