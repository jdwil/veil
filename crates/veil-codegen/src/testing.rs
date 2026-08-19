//! VEIL Testing Framework Codegen
//!
//! Generates test code from VEIL testing AST nodes:
//! - Rust unit tests → `#[tokio::test]` with trait-based mocks
//! - TypeScript unit tests → vitest with `vi.mock()`
//! - Component tests → vitest + @testing-library
//! - E2E scenarios → Playwright

use veil_ir::ast::*;
use veil_ir::layer::LayerRegistry;

/// Generated test file.
pub struct GeneratedTestFile {
    pub path: String,
    pub content: String,
}

/// Generate Rust test code from testing AST nodes (isolated `veil test` fallback).
/// Prefer [`generate_crate_tests`] — that path calls the real handler and compiles
/// inside the product crate.
pub fn generate_rust_tests(items: &[TopLevelItem]) -> Vec<GeneratedTestFile> {
    let mut blocks = Vec::new();
    collect_test_blocks_from_items(items, &mut blocks);
    if blocks.is_empty() {
        return Vec::new();
    }
    // Isolated emission is not a compiling harness. `veil gen` / `veil test`
    // use [`generate_crate_tests`] inside the product crate.
    Vec::new()
}

/// Emit `crates/{crate}/src/tests.rs` that cargo-check --tests can compile.
///
/// SL-022: `tests Target` / `it` / `stub Port.method` / `given` / `then` lower to
/// `#[tokio::test]` that constructs `Deps` from port test-doubles, calls
/// `application::{to_snake(target)}`, and asserts on `result`.
pub fn generate_crate_tests(
    solution: &Solution,
    registry: &LayerRegistry,
    crate_name: &str,
    module: &Construct,
    ports: &[&Construct],
    handlers: &[&Construct],
    enums: &[&Construct],
    structs: &[&Construct],
) -> Option<crate::rust::GeneratedFile> {
    let mut blocks = Vec::new();
    collect_test_blocks_from_construct(module, &mut blocks);
    for item in &solution.items {
        if let TopLevelItem::TestBlock(tb) = item {
            let matches_handler = tb.target.as_ref().is_some_and(|t| {
                handlers.iter().any(|h| {
                    h.name == *t || crate::rust::to_snake(&h.name) == crate::rust::to_snake(t)
                })
            });
            if matches_handler {
                blocks.push(tb);
            }
        }
    }
    if blocks.is_empty() {
        return None;
    }

    let mut name_to_shape = std::collections::HashMap::new();
    for p in ports {
        name_to_shape.insert(p.name.clone(), veil_ir::layer::Shape::Trait);
    }
    let (all_deps, dep_field_names) =
        crate::rust::collect_deps_field_map(handlers, registry, &name_to_shape);

    let mut out = String::new();
    out.push_str("//! Generated from VEIL `tests` / `it` blocks (SL-022).\n\n");
    out.push_str("#![allow(unused_imports, unused_mut)]\n\n");
    out.push_str("use super::*;\n");
    out.push_str("use crate::application::*;\n");
    out.push_str("use crate::domain::types::*;\n");
    out.push_str("use crate::ports::*;\n");
    out.push_str("use async_trait::async_trait;\n");
    out.push_str("use std::sync::Arc;\n\n");

    for port in ports {
        if all_deps.contains(&port.name) {
            out.push_str(&gen_port_double(port));
        }
    }

    let mut used_names = std::collections::HashSet::new();
    for tb in &blocks {
        for case in &tb.cases {
            out.push_str(&gen_handler_test_case(
                tb.target.as_deref(),
                case,
                ports,
                handlers,
                enums,
                structs,
                registry,
                &all_deps,
                &dep_field_names,
                &mut used_names,
            ));
        }
    }

    Some(crate::rust::GeneratedFile {
        path: format!("crates/{crate_name}/src/tests.rs"),
        content: out,
    })
}

fn collect_test_blocks_from_items<'a>(items: &'a [TopLevelItem], out: &mut Vec<&'a TestBlock>) {
    for item in items {
        match item {
            TopLevelItem::TestBlock(tb) => out.push(tb),
            TopLevelItem::Construct(c) => collect_test_blocks_from_construct(c, out),
            _ => {}
        }
    }
}

fn collect_test_blocks_from_construct<'a>(c: &'a Construct, out: &mut Vec<&'a TestBlock>) {
    for tb in &c.test_blocks {
        out.push(tb);
    }
    for child in &c.children {
        collect_test_blocks_from_construct(child, out);
    }
}

fn parse_stub_target(raw: &str) -> (String, String) {
    let clean = raw.trim_end_matches('!');
    if let Some((port, method)) = clean.split_once('.') {
        (
            port.trim().to_string(),
            method.trim().trim_end_matches('!').to_string(),
        )
    } else {
        (clean.to_string(), String::new())
    }
}

fn result_ok_inner(ret: &Option<TypeExpr>) -> String {
    let rust = match ret {
        Some(t) => crate::rust::type_to_rust(t),
        None => "()".to_string(),
    };
    if rust == "Result<(), DomainError>" {
        "()".into()
    } else if let Some(inner) = rust
        .strip_prefix("Result<")
        .and_then(|s| s.strip_suffix(", DomainError>"))
    {
        inner.to_string()
    } else {
        rust
    }
}

fn is_unit_ok(inner: &str) -> bool {
    inner == "()"
}

fn gen_port_double(port: &Construct) -> String {
    let name = format!("TestDouble{}", port.name);
    let mut fields = String::new();
    let mut methods = String::new();
    for m in &port.methods {
        let fname = crate::rust::to_snake(&m.name);
        let inner = result_ok_inner(&m.return_type);
        let field_ty = if is_unit_ok(&inner) {
            "Option<()>".to_string()
        } else {
            format!("Option<{inner}>")
        };
        fields.push_str(&format!("    {fname}: {field_ty},\n"));
        fields.push_str(&format!("    {fname}_err: Option<String>,\n"));

        let params = m
            .params
            .iter()
            .map(|p| {
                format!(
                    "_{}: {}",
                    crate::rust::to_snake(&p.name),
                    crate::rust::type_to_rust(&p.type_expr)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sep = if params.is_empty() { "" } else { ", " };
        let ret = match &m.return_type {
            Some(t) => format!(" -> {}", crate::rust::type_to_rust(t)),
            None => " -> Result<(), DomainError>".into(),
        };
        let ok_arm = if is_unit_ok(&inner) {
            "Some(()) => Ok(())".to_string()
        } else {
            "Some(v) => Ok(v.clone())".to_string()
        };
        methods.push_str(&format!(
            "    async fn {}(&self{sep}{params}){ret} {{\n        if let Some(msg) = &self.{fname}_err {{\n            return Err(DomainError::External(msg.clone()));\n        }}\n        match &self.{fname} {{\n            {ok_arm},\n            None => Err(DomainError::External(\"unstubbed {}.{}\".into())),\n        }}\n    }}\n",
            crate::rust::to_snake(&m.name),
            port.name,
            m.name
        ));
    }
    format!(
        "#[derive(Default)]\n\
         struct {name} {{\n\
         {fields}}}\n\n\
         #[async_trait]\n\
         impl {} for {name} {{\n\
         {methods}}}\n\n",
        port.name
    )
}

fn gen_handler_test_case(
    target: Option<&str>,
    case: &TestCase,
    ports: &[&Construct],
    handlers: &[&Construct],
    enums: &[&Construct],
    structs: &[&Construct],
    registry: &LayerRegistry,
    all_deps: &std::collections::HashSet<String>,
    dep_field_names: &std::collections::HashMap<String, String>,
    used_names: &mut std::collections::HashSet<String>,
) -> String {
    let mut fn_name = case
        .name
        .replace(' ', "_")
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "");
    if fn_name.is_empty() {
        fn_name = "case".into();
    }
    let mut rust_fn = format!("test_{fn_name}");
    let mut n = 2u32;
    while !used_names.insert(rust_fn.clone()) {
        rust_fn = format!("test_{fn_name}_{n}");
        n += 1;
    }

    let handler = target.and_then(|t| {
        handlers.iter().copied().find(|h| {
            h.name == t || crate::rust::to_snake(&h.name) == crate::rust::to_snake(t)
        })
    });

    let mut body = String::new();

    // Port doubles (one per Deps field), then apply stubs.
    let mut double_vars: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut sorted_deps: Vec<&String> = all_deps.iter().collect();
    sorted_deps.sort();
    for trait_name in &sorted_deps {
        let field = dep_field_names
            .get(*trait_name)
            .cloned()
            .unwrap_or_else(|| crate::rust::to_snake(trait_name));
        let var = format!("dbl_{field}");
        body.push_str(&format!(
            "        let mut {var} = TestDouble{trait_name}::default();\n"
        ));
        double_vars.insert((*trait_name).clone(), var);
    }

    for stub in &case.stubs {
        let (port, method) = parse_stub_target(&stub.target);
        let port_c = ports.iter().find(|p| {
            p.name == port || p.name.eq_ignore_ascii_case(&port)
        });
        let Some(port_c) = port_c else {
            body.push_str(&format!("        // stub {} — no such port\n", stub.target));
            continue;
        };
        let Some(var) = double_vars.get(&port_c.name) else {
            body.push_str(&format!(
                "        // stub {} — port not in Deps\n",
                stub.target
            ));
            continue;
        };
        let method_c = port_c.methods.iter().find(|m| {
            m.name == method
                || crate::rust::to_snake(&m.name) == crate::rust::to_snake(&method)
        });
        let fname = method_c
            .map(|m| crate::rust::to_snake(&m.name))
            .unwrap_or_else(|| crate::rust::to_snake(&method));
        match &stub.variant {
            StubVariant::Error(msg) => {
                body.push_str(&format!(
                    "        {var}.{fname}_err = Some({}.to_string());\n",
                    rust_string_lit(msg)
                ));
            }
            StubVariant::Simple(expr) => {
                let inner = method_c
                    .map(|m| result_ok_inner(&m.return_type))
                    .unwrap_or_else(|| "()".into());
                let val = expr_to_test_rust(expr, &inner, enums, structs);
                if is_unit_ok(&inner) {
                    body.push_str(&format!("        {var}.{fname} = Some(());\n"));
                } else if inner.starts_with("Option<") {
                    if val == "None" {
                        body.push_str(&format!("        {var}.{fname} = Some(None);\n"));
                    } else {
                        body.push_str(&format!("        {var}.{fname} = Some(Some({val}));\n"));
                    }
                } else {
                    body.push_str(&format!("        {var}.{fname} = Some({val});\n"));
                }
            }
            StubVariant::Sequence(exprs) => {
                if let Some(first) = exprs.first() {
                    let inner = method_c
                        .map(|m| result_ok_inner(&m.return_type))
                        .unwrap_or_else(|| "()".into());
                    let val = expr_to_test_rust(first, &inner, enums, structs);
                    body.push_str(&format!(
                        "        {var}.{fname} = Some({val}); // sequence: first value only\n"
                    ));
                }
            }
            StubVariant::Conditional { otherwise, .. } => {
                if let Some(expr) = otherwise {
                    let inner = method_c
                        .map(|m| result_ok_inner(&m.return_type))
                        .unwrap_or_else(|| "()".into());
                    let val = expr_to_test_rust(expr, &inner, enums, structs);
                    body.push_str(&format!(
                        "        {var}.{fname} = Some({val}); // conditional otherwise\n"
                    ));
                }
            }
        }
    }

    if !sorted_deps.is_empty() {
        body.push_str("        let deps = Deps {\n");
        for trait_name in &sorted_deps {
            let field = dep_field_names
                .get(*trait_name)
                .cloned()
                .unwrap_or_else(|| crate::rust::to_snake(trait_name));
            let var = &double_vars[*trait_name];
            body.push_str(&format!("            {field}: Arc::new({var}),\n"));
        }
        body.push_str("        };\n");
    }

    for binding in &case.given {
        let ty_hint = handler
            .and_then(|h| {
                h.inputs.iter().find(|f| {
                    f.name == binding.name
                        || crate::rust::to_snake(&f.name) == binding.name
                })
            })
            .map(|f| crate::rust::type_to_rust(&f.type_expr))
            .unwrap_or_else(|| infer_given_type(&binding.value));
        let val = expr_to_test_rust(&binding.value, &ty_hint, enums, structs);
        body.push_str(&format!(
            "        let {} = {};\n",
            crate::rust::to_snake(&binding.name),
            val
        ));
    }

    if let Some(h) = handler {
        let fn_call = crate::rust::to_snake(&h.name);
        let mut args = Vec::new();
        if !sorted_deps.is_empty() {
            args.push("&deps".to_string());
        }
        for input in &h.inputs {
            if registry.field_is_dependency(input) {
                continue;
            }
            let iname = crate::rust::to_snake(&input.name);
            if case.given.iter().any(|g| {
                g.name == input.name || crate::rust::to_snake(&g.name) == iname
            }) {
                args.push(iname);
            } else {
                let ty = crate::rust::type_to_rust(&input.type_expr);
                args.push(default_expr_for_rust_ty(&ty, enums, structs));
            }
        }
        body.push_str(&format!(
            "        let result = {fn_call}({}).await;\n",
            args.join(", ")
        ));
    } else {
        body.push_str(&format!(
            "        let result: Result<(), DomainError> = Err(DomainError::External(\
             \"unresolved VEIL test target {}\".into()));\n",
            target.unwrap_or("<none>")
        ));
    }

    for assertion in &case.then {
        match assertion {
            Assertion::ResultEq(expr) => {
                let expected = expr_to_test_rust(expr, "", enums, structs);
                body.push_str(&format!(
                    "        assert_eq!(result, Ok({expected}));\n"
                ));
            }
            Assertion::FieldEq(field, expr) => {
                let expected = expr_to_test_rust(expr, "", enums, structs);
                body.push_str(&format!(
                    "        assert_eq!(result.as_ref().expect(\"ok\").{field}, {expected});\n"
                ));
            }
            Assertion::Fails(msg) => {
                body.push_str("        assert!(result.is_err());\n");
                if !msg.is_empty() {
                    body.push_str(&format!(
                        "        let __e = result.unwrap_err().to_string();\n        assert!(__e.contains({}), \"{{__e}}\");\n",
                        rust_string_lit(msg)
                    ));
                }
            }
            Assertion::Ok => {
                body.push_str("        assert!(result.is_ok(), \"{result:?}\");\n");
            }
            Assertion::Settles => {
                body.push_str("        tokio::task::yield_now().await;\n");
            }
            Assertion::Expr(expr) => {
                body.push_str(&format!(
                    "        assert!({});\n",
                    expr_to_test_rust(expr, "bool", enums, structs)
                ));
            }
        }
    }

    format!(
        "    #[tokio::test]\n    async fn {rust_fn}() {{\n{body}    }}\n\n"
    )
}

fn rust_string_lit(s: &str) -> String {
    format!("{:?}", s)
}

fn infer_given_type(expr: &Expr) -> String {
    match expr {
        Expr::StringLit(_) => "String".into(),
        Expr::IntLit(_) => "i64".into(),
        Expr::FloatLit(_) => "f64".into(),
        Expr::BoolLit(_) => "bool".into(),
        Expr::StructLit(name, _) => name.clone(),
        _ => "String".into(),
    }
}

fn default_expr_for_rust_ty(
    ty: &str,
    enums: &[&Construct],
    structs: &[&Construct],
) -> String {
    match ty {
        "String" => "String::new()".into(),
        "i64" | "i32" | "u64" | "usize" => "0".into(),
        "f64" | "f32" => "0.0".into(),
        "bool" => "false".into(),
        "()" => "()".into(),
        t if t.starts_with("Option<") => "None".into(),
        t if t.starts_with("Vec<") => "vec![]".into(),
        other => {
            if enums.iter().any(|e| e.name == other) {
                format!("{other}::default()")
            } else if let Some(s) = structs.iter().find(|s| s.name == other) {
                let fields: Vec<String> = s
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = crate::rust::to_snake(&f.name);
                        let fty = crate::rust::type_to_rust(&f.type_expr);
                        format!(
                            "{fname}: {}",
                            default_expr_for_rust_ty(&fty, enums, structs)
                        )
                    })
                    .collect();
                format!("{other} {{ {} }}", fields.join(", "))
            } else {
                format!("{other}::default()")
            }
        }
    }
}

fn expr_to_test_rust(
    expr: &Expr,
    expected_ty: &str,
    enums: &[&Construct],
    structs: &[&Construct],
) -> String {
    match expr {
        Expr::Ident(name) if name == "null" || name == "None" => "None".into(),
        Expr::Ident(name) => {
            if enums.iter().any(|e| e.variants.iter().any(|v| v == name))
                && let Some(e) = enums.iter().find(|e| e.variants.iter().any(|v| v == name)) {
                    return format!("{}::{name}", e.name);
                }
            crate::rust::to_snake(name)
        }
        Expr::StringLit(s) => {
            if expected_ty == "bool" {
                rust_string_lit(s)
            } else {
                format!("{}.to_string()", rust_string_lit(s))
            }
        }
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::StructLit(name, fields) => {
            let st = structs.iter().find(|s| &s.name == name);
            let fs: Vec<String> = fields
                .iter()
                .map(|(k, v)| {
                    let fty = st
                        .and_then(|s| s.fields.iter().find(|f| f.name == *k || crate::rust::to_snake(&f.name) == crate::rust::to_snake(k)))
                        .map(|f| crate::rust::type_to_rust(&f.type_expr))
                        .unwrap_or_default();
                    format!(
                        "{}: {}",
                        crate::rust::to_snake(k),
                        expr_to_test_rust(v, &fty, enums, structs)
                    )
                })
                .collect();
            format!("{name} {{ {} }}", fs.join(", "))
        }
        Expr::ArrayLit(items) => {
            let elems: Vec<String> = items
                .iter()
                .map(|e| expr_to_test_rust(e, "", enums, structs))
                .collect();
            format!("vec![{}]", elems.join(", "))
        }
        Expr::Call(call) => {
            let args: Vec<String> = call
                .args
                .iter()
                .map(|a| expr_to_test_rust(a, "", enums, structs))
                .collect();
            if call.method.is_empty() {
                format!("{}({})", call.target, args.join(", "))
            } else {
                format!("{}.{}({})", call.target, call.method, args.join(", "))
            }
        }
        Expr::FieldAccess(base, field) => {
            format!(
                "{}.{}",
                expr_to_test_rust(base, "", enums, structs),
                field
            )
        }
        Expr::BinaryOp(op) => {
            let op_str = match &op.op {
                BinOp::Eq => "==",
                BinOp::NotEq => "!=",
                BinOp::Lt => "<",
                BinOp::Gt => ">",
                BinOp::LtEq => "<=",
                BinOp::GtEq => ">=",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Mod => "%",
                BinOp::And => "&&",
                BinOp::Or => "||",
            };
            format!(
                "{} {op_str} {}",
                expr_to_test_rust(&op.left, "", enums, structs),
                expr_to_test_rust(&op.right, "", enums, structs)
            )
        }
        _ => "Default::default()".into(),
    }
}

/// Generate TypeScript test code (vitest) from testing AST nodes.
pub fn generate_ts_tests(items: &[TopLevelItem]) -> Vec<GeneratedTestFile> {
    let mut files = Vec::new();
    let mut test_code = String::new();
    let mut scenario_code = String::new();

    for item in items {
        match item {
            TopLevelItem::TestBlock(tb) => {
                test_code.push_str(&gen_ts_test_block(tb));
            }
            TopLevelItem::Fixture(fix) => {
                test_code.push_str(&gen_ts_fixture(fix));
            }
            TopLevelItem::Integration(integ) => {
                test_code.push_str(&gen_ts_integration(integ));
            }
            TopLevelItem::Scenario(scen) => {
                scenario_code.push_str(&gen_playwright_scenario(scen));
            }
            TopLevelItem::Construct(c) => {
                collect_construct_tests_ts(c, &mut test_code);
            }
            _ => {}
        }
    }

    if !test_code.is_empty() {
        files.push(GeneratedTestFile {
            path: "src/__tests__/unit.test.ts".to_string(),
            content: format!(
                "import {{ describe, it, expect, vi }} from 'vitest';\n\n{}\n",
                test_code
            ),
        });
    }

    if !scenario_code.is_empty() {
        files.push(GeneratedTestFile {
            path: "e2e/scenarios.spec.ts".to_string(),
            content: format!(
                "import {{ test, expect }} from '@playwright/test';\n\n{}\n",
                scenario_code
            ),
        });
    }

    files
}

fn gen_ts_test_block(tb: &TestBlock) -> String {
    let desc = tb.target.as_deref().unwrap_or("module");
    let mut out = format!("describe('{}', () => {{\n", desc);

    for case in &tb.cases {
        out.push_str(&gen_ts_test_case(case));
    }

    out.push_str("});\n\n");
    out
}

fn gen_ts_test_case(tc: &TestCase) -> String {
    let mut body = String::new();

    // Stubs via vi.mock
    for stub in &tc.stubs {
        match &stub.variant {
            StubVariant::Simple(expr) => {
                body.push_str(&format!(
                    "    vi.spyOn({}).mockReturnValue({});\n",
                    stub.target.replace('.', ", '") + "'",
                    expr_to_ts(expr)
                ));
            }
            StubVariant::Error(msg) => {
                body.push_str(&format!(
                    "    vi.spyOn({}).mockRejectedValue(new Error('{}'));\n",
                    stub.target.replace('.', ", '") + "'",
                    msg
                ));
            }
            StubVariant::Sequence(exprs) => {
                let values: Vec<String> = exprs.iter().map(expr_to_ts).collect();
                body.push_str(&format!(
                    "    const mock = vi.spyOn({});\n",
                    stub.target.replace('.', ", '") + "'"
                ));
                for (i, val) in values.iter().enumerate() {
                    body.push_str(&format!(
                        "    mock.mockReturnValueOnce({});\n",
                        val
                    ));
                    let _ = i;
                }
            }
            StubVariant::Conditional { .. } => {
                body.push_str(&format!(
                    "    // conditional stub for {}\n",
                    stub.target
                ));
            }
        }
    }

    // Given
    for binding in &tc.given {
        body.push_str(&format!(
            "    const {} = {};\n",
            binding.name,
            expr_to_ts(&binding.value)
        ));
    }

    // Mount (component tests)
    if let Some(mount) = &tc.mount {
        body.push_str(&format!(
            "    const {{ container }} = render({}, {{\n",
            mount.component
        ));
        for prop in &mount.props {
            body.push_str(&format!(
                "      {}: {},\n",
                prop.name,
                expr_to_ts(&prop.value)
            ));
        }
        body.push_str("    });\n");
    }

    // Actions
    for action in &tc.actions {
        match action {
            TestAction::Click(sel) => {
                body.push_str(&format!(
                    "    await userEvent.click(screen.getByRole('{}'));\n",
                    sel
                ));
            }
            TestAction::Fill(sel, val) => {
                body.push_str(&format!(
                    "    await userEvent.type(screen.getByLabelText('{}'), '{}');\n",
                    sel, val
                ));
            }
            TestAction::Fire(evt, sel) => {
                body.push_str(&format!(
                    "    fireEvent.{}(screen.getByRole('{}'));\n",
                    evt, sel
                ));
            }
            TestAction::Wait(ms) => {
                body.push_str(&format!(
                    "    await new Promise(r => setTimeout(r, {}));\n",
                    ms
                ));
            }
        }
    }

    // Then
    for assertion in &tc.then {
        match assertion {
            Assertion::ResultEq(expr) => {
                body.push_str(&format!(
                    "    expect(result).toEqual({});\n",
                    expr_to_ts(expr)
                ));
            }
            Assertion::FieldEq(field, expr) => {
                body.push_str(&format!(
                    "    expect(result.{}).toEqual({});\n",
                    field,
                    expr_to_ts(expr)
                ));
            }
            Assertion::Fails(msg) => {
                body.push_str(&format!(
                    "    await expect(result).rejects.toThrow('{}');\n",
                    msg
                ));
            }
            Assertion::Ok => {
                body.push_str("    expect(result).toBeDefined();\n");
            }
            Assertion::Settles => {
                body.push_str("    await vi.waitFor(() => {{}});\n");
            }
            Assertion::Expr(expr) => {
                body.push_str(&format!("    expect({}).toBeTruthy();\n", expr_to_ts(expr)));
            }
        }
    }

    if body.is_empty() {
        body.push_str("    // TODO: implement test\n");
    }

    format!("  it('{}', async () => {{\n{}\n  }});\n\n", tc.name, body)
}

fn gen_ts_fixture(fix: &Fixture) -> String {
    let mut out = format!("const {} = {{\n", fix.name);
    for binding in &fix.bindings {
        out.push_str(&format!("  {}: {},\n", binding.name, expr_to_ts(&binding.value)));
    }
    out.push_str("};\n\n");
    out
}

fn gen_ts_integration(integ: &IntegrationBlock) -> String {
    let mut out = format!("describe('integration: {}', () => {{\n", integ.name);
    out.push_str("  // TODO: implement integration test\n");
    out.push_str("});\n\n");
    out
}

fn gen_playwright_scenario(scen: &ScenarioBlock) -> String {
    let mut body = String::new();

    for step in &scen.steps {
        match step {
            ScenarioStep::Navigate(path) => {
                body.push_str(&format!("  await page.goto('{}');\n", path));
            }
            ScenarioStep::Fill(sel, val) => {
                body.push_str(&format!(
                    "  await page.locator('{}').fill('{}');\n",
                    sel, val
                ));
            }
            ScenarioStep::Select(sel, val) => {
                body.push_str(&format!(
                    "  await page.locator('{}').selectOption('{}');\n",
                    sel, val
                ));
            }
            ScenarioStep::Click(sel) => {
                body.push_str(&format!("  await page.locator('{}').click();\n", sel));
            }
            ScenarioStep::WaitFor(sel) => {
                body.push_str(&format!(
                    "  await page.locator('{}').waitFor();\n",
                    sel
                ));
            }
            ScenarioStep::Assert(expr) => {
                body.push_str(&format!("  await expect({}).toBeTruthy();\n", expr_to_ts(expr)));
            }
        }
    }

    format!(
        "test('{}', async ({{ page }}) => {{\n{}}});\n\n",
        scen.name, body
    )
}

// ─── Expression helpers ───────────────────────────────────────────────────────

fn expr_to_ts(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::StringLit(s) => format!("'{}'", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::StructLit(_name, fields) => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_ts(v)))
                .collect();
            format!("{{ {} }}", fs.join(", "))
        }
        Expr::Call(call) => {
            let args: Vec<String> = call.args.iter().map(expr_to_ts).collect();
            if call.method.is_empty() {
                format!("{}({})", call.target, args.join(", "))
            } else {
                format!("{}.{}({})", call.target, call.method, args.join(", "))
            }
        }
        Expr::FieldAccess(base, field) => {
            format!("{}.{}", expr_to_ts(base), field)
        }
        Expr::ArrayLit(items) => {
            let elems: Vec<String> = items.iter().map(expr_to_ts).collect();
            format!("[{}]", elems.join(", "))
        }
        Expr::BinaryOp(op) => {
            let op_str = match &op.op {
                BinOp::Eq => "===",
                BinOp::NotEq => "!==",
                BinOp::Lt => "<",
                BinOp::Gt => ">",
                BinOp::LtEq => "<=",
                BinOp::GtEq => ">=",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Mod => "%",
                BinOp::And => "&&",
                BinOp::Or => "||",
            };
            format!("{} {} {}", expr_to_ts(&op.left), op_str, expr_to_ts(&op.right))
        }
        _ => "undefined /* TODO */".to_string(),
    }
}

/// Recursively collect test blocks from a construct and its children (TypeScript).
fn collect_construct_tests_ts(c: &Construct, out: &mut String) {
    for tb in &c.test_blocks {
        out.push_str(&gen_ts_test_block(tb));
    }
    for child in &c.children {
        collect_construct_tests_ts(child, out);
    }
}
