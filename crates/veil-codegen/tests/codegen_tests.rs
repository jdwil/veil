//! Codegen integration tests — generate Rust from the example VEIL files and
//! assert the semantic properties that were previously broken (guards enforced,
//! adapter impls real, saga compensation emitted). These lock in the fixes so
//! "it compiles" can't silently regress to "it compiles but does nothing".

use veil_ir::LayerRegistry;

/// Parse an example .veil file with the ddd layer and generate the project.
fn generate_example(src: &str) -> String {
    let mut reg = LayerRegistry::builtin();
    reg.load_content("ddd", include_str!("../../../examples/ddd.layer"))
        .expect("ddd layer should load");
    let tokens = veil_parser::lex(src);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse failed");
    let project = veil_codegen::generate(&sol, &reg);
    // Concatenate all generated files so tests can assert on the whole output.
    project
        .files
        .iter()
        .map(|f| format!("// ==== {} ====\n{}", f.path, f.content))
        .collect::<Vec<_>>()
        .join("\n")
}

fn customer_onboarding() -> String {
    generate_example(include_str!("../../../examples/customer_onboarding.veil"))
}

#[test]
fn guard_enforces_validation() {
    let out = customer_onboarding();
    // The `guard call Email.validate(email), "invalid email"` must propagate an
    // error, not silently bind-and-discard.
    assert!(
        out.contains("map_err(|_| DomainError::Validation(\"invalid email\".to_string()))?"),
        "fallible-call guard not enforced:\n{}",
        grep(&out, "validate")
    );
    // The old no-op form must be gone.
    assert!(!out.contains("let __guard"), "guard is still a no-op");
}

#[test]
fn aggregate_fn_bodies_are_real() {
    let out = customer_onboarding();
    assert!(out.contains("impl Customer"), "no Customer impl generated");
    assert!(
        out.contains("pub fn verify(&mut self"),
        "aggregate business method not emitted"
    );
    // Invariant guard + state transition + event emission.
    assert!(out.contains("self.status = CustomerStatus::Verified;"));
    assert!(out.contains("events.push(CustomerEvent::CustomerVerified"));
}

#[test]
fn adapter_impls_are_real_not_todo_comments() {
    let out = customer_onboarding();
    // A real trait impl, not the old commented-out stub.
    assert!(
        out.contains("impl Notifier for SmsTwilio"),
        "adapter impl not generated:\n{}",
        grep(&out, "SmsTwilio")
    );
    assert!(
        !out.contains("// TODO: Implement Notifier"),
        "adapter still emits the commented-out stub"
    );
    // External-effect call routed to a generated runtime hook.
    assert!(out.contains("fn http_post("), "external-effect hook not generated");
    // The impl must cover ALL trait methods (send_email too), else it won't compile.
    assert!(out.contains("async fn send_email"), "unimplemented trait method not stubbed");
}

#[test]
fn saga_emits_reverse_order_compensation() {
    let out = customer_onboarding();
    assert!(out.contains("let __saga: Result<(), DomainError>"), "saga wrapper missing");
    assert!(out.contains("if let Err(__e) = __saga"), "saga error handler missing");
    // Compensations run in reverse: setup_billing before verify_identity before
    // create_customer. Check ordering by byte position.
    let sb = out.find("// compensate: setup_billing");
    let vi = out.find("// compensate: verify_identity");
    let cc = out.find("// compensate: create_customer");
    assert!(sb.is_some() && vi.is_some() && cc.is_some(), "compensations missing");
    assert!(sb < vi && vi < cc, "compensations not in reverse order");
}

#[test]
fn bus_port_generated_from_layer_declaration() {
    let out = customer_onboarding();
    // The injected Bus port becomes a trait with the declared methods.
    assert!(out.contains("trait Bus"), "declared Bus port not generated");
    assert!(out.contains("async fn dispatch"), "Bus.dispatch missing");
}

/// Return only lines containing `needle` (for readable assertion failures).
fn grep(haystack: &str, needle: &str) -> String {
    haystack
        .lines()
        .filter(|l| l.contains(needle))
        .collect::<Vec<_>>()
        .join("\n")
}
