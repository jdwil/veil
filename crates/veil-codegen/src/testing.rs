//! VEIL Testing Framework Codegen
//!
//! Generates test code from VEIL testing AST nodes:
//! - Rust unit tests → `#[tokio::test]` with trait-based mocks
//! - TypeScript unit tests → vitest with `vi.mock()`
//! - Component tests → vitest + @testing-library
//! - E2E scenarios → Playwright

use veil_ir::ast::*;

/// Generated test file.
pub struct GeneratedTestFile {
    pub path: String,
    pub content: String,
}

/// Generate Rust test code from testing AST nodes.
pub fn generate_rust_tests(items: &[TopLevelItem]) -> Vec<GeneratedTestFile> {
    let mut files = Vec::new();
    let mut test_code = String::new();

    for item in items {
        match item {
            TopLevelItem::TestBlock(tb) => {
                test_code.push_str(&gen_rust_test_block(tb));
            }
            TopLevelItem::Fixture(fix) => {
                test_code.push_str(&gen_rust_fixture(fix));
            }
            TopLevelItem::Integration(integ) => {
                test_code.push_str(&gen_rust_integration(integ));
            }
            TopLevelItem::Construct(c) => {
                collect_construct_tests(c, &mut test_code);
            }
            _ => {}
        }
    }

    if !test_code.is_empty() {
        files.push(GeneratedTestFile {
            path: "src/tests.rs".to_string(),
            content: format!(
                "#[cfg(test)]\nmod tests {{\n    use super::*;\n\n{}}}\n",
                test_code
            ),
        });
    }

    files
}

fn gen_rust_test_block(tb: &TestBlock) -> String {
    let mut out = String::new();
    let target_comment = tb
        .target
        .as_ref()
        .map(|t| format!("    // Tests for {}\n", t))
        .unwrap_or_default();
    out.push_str(&target_comment);

    for case in &tb.cases {
        out.push_str(&gen_rust_test_case(case));
    }
    out
}

fn gen_rust_test_case(tc: &TestCase) -> String {
    let fn_name = tc.name.replace(' ', "_").replace(|c: char| !c.is_alphanumeric() && c != '_', "");
    let mut body = String::new();

    // Stubs
    for stub in &tc.stubs {
        body.push_str(&format!("        // stub: {}\n", stub.target));
        match &stub.variant {
            StubVariant::Simple(expr) => {
                body.push_str(&format!(
                    "        let mock_{} = {};\n",
                    stub.target.replace('.', "_").replace('!', "").to_lowercase(),
                    expr_to_rust(expr)
                ));
            }
            StubVariant::Error(msg) => {
                body.push_str(&format!(
                    "        // returns error: \"{}\"\n",
                    msg
                ));
            }
            StubVariant::Conditional { .. } => {
                body.push_str("        // conditional stub\n");
            }
            StubVariant::Sequence(exprs) => {
                body.push_str(&format!(
                    "        // sequence stub: {} values\n",
                    exprs.len()
                ));
            }
        }
    }

    // Given bindings
    for binding in &tc.given {
        body.push_str(&format!(
            "        let {} = {};\n",
            binding.name,
            expr_to_rust(&binding.value)
        ));
    }

    // Then assertions
    for assertion in &tc.then {
        match assertion {
            Assertion::ResultEq(expr) => {
                body.push_str(&format!(
                    "        assert_eq!(result, {});\n",
                    expr_to_rust(expr)
                ));
            }
            Assertion::FieldEq(field, expr) => {
                body.push_str(&format!(
                    "        assert_eq!(result.{}, {});\n",
                    field,
                    expr_to_rust(expr)
                ));
            }
            Assertion::Fails(msg) => {
                body.push_str(&format!(
                    "        assert!(result.is_err());\n        assert_eq!(result.unwrap_err().to_string(), \"{}\");\n",
                    msg
                ));
            }
            Assertion::Ok => {
                body.push_str("        assert!(result.is_ok());\n");
            }
            Assertion::Settles => {
                body.push_str("        tokio::task::yield_now().await;\n");
            }
            Assertion::Expr(expr) => {
                body.push_str(&format!("        assert!({});\n", expr_to_rust(expr)));
            }
        }
    }

    if body.is_empty() {
        body.push_str("        todo!(\"implement test\");\n");
    }

    format!(
        "    #[tokio::test]\n    async fn test_{}() {{\n{}\n    }}\n\n",
        fn_name, body
    )
}

fn gen_rust_fixture(fix: &Fixture) -> String {
    let mut out = format!("    fn fixture_{}() -> impl std::fmt::Debug {{\n", fix.name);
    if fix.bindings.is_empty() {
        out.push_str("        ()\n");
    } else {
        out.push_str("        (\n");
        for binding in &fix.bindings {
            out.push_str(&format!(
                "            /* {} */ {},\n",
                binding.name,
                expr_to_rust(&binding.value)
            ));
        }
        out.push_str("        )\n");
    }
    out.push_str("    }\n\n");
    out
}

fn gen_rust_integration(integ: &IntegrationBlock) -> String {
    let fn_name = integ.name.replace(' ', "_").replace(|c: char| !c.is_alphanumeric() && c != '_', "");
    let mut body = String::new();

    for dep in &integ.real_deps {
        body.push_str(&format!("        // real: {}\n", dep));
    }
    for stub in &integ.stub_deps {
        body.push_str(&format!("        // stub: {}\n", stub.target));
    }

    if !integ.setup.is_empty() {
        body.push_str("        // setup\n");
    }
    if !integ.verify.is_empty() {
        body.push_str("        // verify\n");
    }
    if !integ.teardown.is_empty() {
        body.push_str("        // teardown\n");
    }

    body.push_str("        todo!(\"implement integration test\");\n");

    format!(
        "    #[tokio::test]\n    async fn integration_{}() {{\n{}\n    }}\n\n",
        fn_name, body
    )
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

fn expr_to_rust(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::StructLit(name, fields) => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_rust(v)))
                .collect();
            format!("{} {{ {} }}", name, fs.join(", "))
        }
        Expr::Call(call) => {
            let args: Vec<String> = call.args.iter().map(expr_to_rust).collect();
            if call.method.is_empty() {
                format!("{}({})", call.target, args.join(", "))
            } else {
                format!("{}.{}({})", call.target, call.method, args.join(", "))
            }
        }
        Expr::FieldAccess(base, field) => {
            format!("{}.{}", expr_to_rust(base), field)
        }
        Expr::ArrayLit(items) => {
            let elems: Vec<String> = items.iter().map(expr_to_rust).collect();
            format!("vec![{}]", elems.join(", "))
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
            format!("{} {} {}", expr_to_rust(&op.left), op_str, expr_to_rust(&op.right))
        }
        _ => format!("todo!(/* {:?} */)", std::mem::discriminant(expr)),
    }
}

fn expr_to_ts(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::StringLit(s) => format!("'{}'", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::StructLit(name, fields) => {
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

/// Recursively collect test blocks from a construct and its children (Rust).
fn collect_construct_tests(c: &Construct, out: &mut String) {
    for tb in &c.test_blocks {
        out.push_str(&gen_rust_test_block(tb));
    }
    for child in &c.children {
        collect_construct_tests(child, out);
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
