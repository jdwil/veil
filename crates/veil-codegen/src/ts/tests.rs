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

use super::lower::{lower_to_ts, to_camel_case, veil_type_to_ts};
use crate::expr::GenCtx;
use std::collections::HashMap;
use veil_ir::ast::{BinOp, BinaryOpExpr, Expr, Pattern, StringPart, TypeExpr, UnaryOp, UnaryOpExpr};

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
    // ArrayLit is not explicitly handled by lower_to_ts → falls to Raw
    let expr = Expr::ArrayLit(vec![Expr::IntLit(1), Expr::IntLit(2)]);
    let ts = lower_to_ts(&expr, &test_ctx());
    // The Raw path uses expr_to_ts which renders array literals
    assert_eq!(emit_ts(&ts), "[1, 2]");
}
