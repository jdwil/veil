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
fn orchestrator_bus_calls_use_real_json_not_placeholders() {
    let out = customer_onboarding();
    // Cross-context calls carry a typed JSON envelope.
    assert!(
        out.contains("deps.bus.invoke(serde_json::json!({ \"target\": \"CustomerRepo\""),
        "bus call not a JSON envelope:\n{}",
        grep(&out, "bus.invoke")
    );
    // Events dispatch with a typed JSON message.
    assert!(
        out.contains("\"type\": \"CustomerCreated\""),
        "event not a typed JSON message"
    );
    // The old junk placeholders must be gone.
    assert!(!out.contains("{}:id"), "symbolic-placeholder junk still present");
    assert!(
        !out.contains("format!(\"Customer.new"),
        "debug-string pseudo-call still present"
    );
    // Bus results index as JSON.
    assert!(out.contains("[\"id\"]"), "JSON field indexing missing");
}

#[test]
fn bus_port_generated_from_layer_declaration() {
    let out = customer_onboarding();
    // The injected Bus port becomes a trait with the declared methods.
    assert!(out.contains("trait Bus"), "declared Bus port not generated");
    assert!(out.contains("async fn dispatch"), "Bus.dispatch missing");
}

#[test]
fn bus_and_errors_defined_once_in_shared_crate() {
    let out = customer_onboarding();
    // Exactly one `pub trait Bus` definition, in veil_shared.
    let bus_defs = out.matches("pub trait Bus").count();
    assert_eq!(bus_defs, 1, "Bus trait should be defined exactly once, found {}", bus_defs);
    assert!(
        out.contains("// ==== crates/veil_shared/src/lib.rs ===="),
        "shared crate not generated"
    );
    // Error types defined once (in the shared crate), re-exported elsewhere.
    let err_defs = out.matches("pub enum DomainError").count();
    assert_eq!(err_defs, 1, "DomainError should be defined once, found {}", err_defs);
    assert!(out.contains("pub use veil_shared::{DomainError, ValidationError}"), "context crates should re-export shared errors");
}

#[test]
fn flow_return_type_is_inferred_not_hardcoded() {
    // A service returning `ret c.id` (a UUID field of a Customer) infers Uuid.
    let out = customer_onboarding();
    assert!(
        out.contains("pub async fn create_customer_service(") && out.contains("-> Result<Uuid, DomainError>"),
        "service return type not inferred as Uuid:\n{}",
        grep(&out, "create_customer_service")
    );

    // A flow that returns an Int field must infer i64, proving it's not a
    // blanket Uuid. Build a minimal solution inline.
    let src = "\
sol T
  use ddd
  ctx C
    group g
      agg Order
        root
          id: UUID
          total: Int
      svc TotalService
        input
          order_id: UUID
        step load
          o = call Order.new(order_id)
        ret o.total";
    let out2 = generate_example(src);
    assert!(
        out2.contains("-> Result<i64, DomainError>"),
        "Int return not inferred as i64:\n{}",
        grep(&out2, "total_service")
    );
}

/// Return only lines containing `needle` (for readable assertion failures).
fn grep(haystack: &str, needle: &str) -> String {
    haystack
        .lines()
        .filter(|l| l.contains(needle))
        .collect::<Vec<_>>()
        .join("\n")
}
