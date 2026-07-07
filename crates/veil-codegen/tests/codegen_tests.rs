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

/// Generate from a custom layer + app source (for language-feature tests).
fn generate_with_layer(layer_name: &str, layer_src: &str, app_src: &str) -> String {
    let mut reg = LayerRegistry::builtin();
    reg.load_content(layer_name, layer_src).expect("layer should load");
    let tokens = veil_parser::lex(app_src);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse failed");
    let project = veil_codegen::generate(&sol, &reg);
    project
        .files
        .iter()
        .map(|f| format!("// ==== {} ====\n{}", f.path, f.content))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn list_of_trait_lowers_to_boxed_trait_objects() {
    // The foundation for saga steps: a declared coordinator taking a
    // List<Trait> and calling methods on loop elements must lower to
    // Vec<Box<dyn Trait + Send + Sync>> with `.await?` method calls.
    let layer = "\
pkg jobs v1
  construct Thing
    keyword thing
    maps_to struct
    allowed_in top
  declare
    trait Job
      run() -> Res!
    fn run_all(jobs: List<Job>) -> Res!
      for j in jobs
        call j.run()
      ret Ok";
    let app = "sol JobsApp\n  use jobs\n  thing Gadget\n    size: Int";
    let out = generate_with_layer("jobs", layer, app);
    // A List<Trait> coordinator param is a borrowed slice of boxed trait
    // objects (boxed trait objects aren't Clone, so they're borrowed not moved).
    assert!(
        out.contains("jobs: &[Box<dyn Job + Send + Sync>]"),
        "List<Trait> param not a boxed-trait slice:\n{}",
        out
    );
    assert!(out.contains("j.run().await?"), "trait method call not async/fallible:\n{}", out);
    assert!(out.contains("return Ok(())"), "`ret Ok` mistranslated:\n{}", out);
    assert!(!out.contains("Ok(Ok)"), "`ret Ok` double-wrapped");
}

#[test]
fn declared_fn_with_body_generates_free_function() {
    // A `fn` with a real body declared in a layer's `declare` block must
    // generate a compiling free function in veil_shared — the foundation for
    // moving the saga coordinator into the layer.
    let layer = "\
pkg mini v1
  construct Widget
    keyword widget
    maps_to struct
    allowed_in top
  declare
    fn sum_all(items: List<Int>) -> Res!<Int>
      mut total = 0
      for x in items
        total = total + x
      ret total";
    let app = "sol MiniApp\n  use mini\n  widget Gadget\n    size: Int";
    let out = generate_with_layer("mini", layer, app);
    assert!(
        out.contains("pub async fn sum_all("),
        "declared fn not generated:\n{}",
        out
    );
    // Reassignment to a `mut` var must not shadow (no second `let`).
    assert!(out.contains("total = total + x;"), "mut reassignment shadowed:\n{}", out);
    assert!(!out.contains("let total = total + x"), "reassignment emitted as let-shadow");
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
fn saga_lowers_to_step_impls_and_delegates_to_coordinator() {
    let out = customer_onboarding();
    // Each step becomes a generated struct + `impl SagaStep` (action/compensate).
    assert!(out.contains("impl SagaStep for OnboardStep0"), "step 0 impl missing:\n{}", grep(&out, "impl SagaStep"));
    assert!(out.contains("async fn action(&self, bus:"), "action method missing");
    assert!(out.contains("async fn compensate(&self, bus:"), "compensate method missing");
    // The saga fn just builds the step list and calls the layer coordinator.
    assert!(out.contains("run_saga(deps.bus.as_ref(), &steps).await"), "coordinator call missing:\n{}", grep(&out, "run_saga"));
    assert!(out.contains("Vec<Box<dyn SagaStep + Send + Sync>>"), "boxed step list missing");
    // Cross-step results thread through shared JSON state (step 0 writes it,
    // later steps read it) — no engine-side unwind machinery.
    assert!(out.contains("state[\"c\"]"), "cross-step state threading missing:\n{}", grep(&out, "state["));
    assert!(!out.contains("let __saga"), "hardcoded saga wrapper still present");
    assert!(!out.contains("if let Err(__e) = __saga"), "hardcoded unwind still present");
}

#[test]
fn saga_knowledge_is_not_in_the_engine() {
    // The saga coordinator + SagaStep trait come from the layer, not the engine.
    let out = customer_onboarding();
    assert!(out.contains("pub async fn run_saga("), "coordinator not generated from layer");
    assert!(out.contains("pub trait SagaStep"), "SagaStep trait not generated from layer");
}

#[test]
fn orchestrator_bus_calls_use_real_json_not_placeholders() {
    let out = customer_onboarding();
    // Cross-context calls carry a typed JSON envelope (now inside step impls,
    // routed through the injected `bus` param).
    assert!(
        out.contains("bus.invoke(serde_json::json!({ \"target\": \"CustomerRepo\""),
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

#[test]
fn manifest_includes_layer_provided_deps_with_strategy() {
    let out = customer_onboarding();
    // The manifest should include Bus with "provided_by": "runtime"
    assert!(
        out.contains(r#""provided_by": "runtime""#),
        "runtime-provided deps not in manifest:\n{}",
        grep(&out, "manifest.json")
    );
    assert!(
        out.contains(r#""trait": "Bus""#),
        "Bus trait not in manifest:\n{}",
        grep(&out, "Bus")
    );
    // AuthService should also appear with "provided_by": "runtime" and a strategy
    assert!(
        out.contains(r#""trait": "AuthService""#),
        "AuthService trait not in manifest:\n{}",
        grep(&out, "AuthService")
    );
    assert!(
        out.contains(r#""strategy": "bus""#),
        "strategy field not in manifest for AuthService:\n{}",
        grep(&out, "strategy")
    );
}

// ─── TypeScript codegen tests ────────────────────────────────────────────────

fn generate_ts_example(src: &str) -> String {
    let mut reg = veil_ir::LayerRegistry::builtin();
    reg.load_content("ddd", include_str!("../../../examples/ddd.layer"))
        .expect("ddd layer should load");
    let tokens = veil_parser::lex(src);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse failed");
    let project = veil_codegen::generate_ts(&sol, &reg);
    project
        .files
        .iter()
        .map(|f| format!("// ==== {} ====\n{}", f.path, f.content))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ts_struct_generates_interface() {
    let out = generate_ts_example(include_str!("../../../examples/customer_onboarding.veil"));
    assert!(out.contains("export interface Customer"), "struct not mapped to TS interface");
    assert!(out.contains("id: string"), "UUID field not mapped to string");
    assert!(out.contains("created: Date"), "DateTime field not mapped to Date");
}

#[test]
fn ts_trait_generates_interface_with_async_methods() {
    let out = generate_ts_example(include_str!("../../../examples/customer_onboarding.veil"));
    assert!(out.contains("export interface CustomerRepo"), "trait not mapped to TS interface");
    assert!(out.contains("save(c: Customer): Promise<void>"), "Res! not mapped to Promise<void>");
    assert!(out.contains("find(id: string): Promise<Customer | null>"), "Res!<Opt<T>> not mapped to Promise<T | null>");
}

#[test]
fn ts_generates_project_scaffolding() {
    let out = generate_ts_example(include_str!("../../../examples/customer_onboarding.veil"));
    assert!(out.contains("package.json"), "package.json not generated");
    assert!(out.contains("tsconfig.json"), "tsconfig.json not generated");
    assert!(out.contains("\"typescript\": \"^5.4.0\""), "typescript dep not in package.json");
    assert!(out.contains("export * from './types'"), "index.ts re-exports missing");
}

#[test]
fn ts_type_mapping_covers_all_primitives() {
    use veil_codegen::typescript::type_to_ts;
    use veil_ir::ast::TypeExpr;

    assert_eq!(type_to_ts(&TypeExpr::Named("Str".into())), "string");
    assert_eq!(type_to_ts(&TypeExpr::Named("Int".into())), "number");
    assert_eq!(type_to_ts(&TypeExpr::Named("F64".into())), "number");
    assert_eq!(type_to_ts(&TypeExpr::Named("Bool".into())), "boolean");
    assert_eq!(type_to_ts(&TypeExpr::Named("UUID".into())), "string");
    assert_eq!(type_to_ts(&TypeExpr::Named("DateTime".into())), "Date");
    assert_eq!(type_to_ts(&TypeExpr::Named("Json".into())), "Record<string, unknown>");
    assert_eq!(type_to_ts(&TypeExpr::Named("Bytes".into())), "Uint8Array");

    // Constructors
    assert_eq!(type_to_ts(&TypeExpr::Result(None)), "Promise<void>");
    assert_eq!(
        type_to_ts(&TypeExpr::Result(Some(Box::new(TypeExpr::Named("Customer".into()))))),
        "Promise<Customer>"
    );
    assert_eq!(
        type_to_ts(&TypeExpr::Optional(Box::new(TypeExpr::Named("Str".into())))),
        "string | null"
    );
    assert_eq!(
        type_to_ts(&TypeExpr::List(Box::new(TypeExpr::Named("Int".into())))),
        "number[]"
    );
    assert_eq!(
        type_to_ts(&TypeExpr::Map(
            Box::new(TypeExpr::Named("Str".into())),
            Box::new(TypeExpr::Named("Int".into()))
        )),
        "Map<string, number>"
    );
}

#[test]
fn rich_enum_variants_parse_and_generate() {
    let layer = "\
pkg test v1
  construct Ctx
    keyword ctx
    maps_to mod
    allowed_in top
  construct Status
    keyword status
    maps_to enum
    allowed_in Ctx";
    let app = "\
sol TestApp
  use test
  ctx Core
    status Message
      Text(Str)
      Image(Str, Int, Int)
      Empty";
    let out = generate_with_layer("test", layer, app);
    // Tuple variant with types
    assert!(out.contains("Text(String)"), "tuple variant not generated:\n{}", grep(&out, "Text"));
    assert!(out.contains("Image(String, i64, i64)"), "multi-type tuple variant not generated:\n{}", grep(&out, "Image"));
    // Unit variant still works
    assert!(out.contains("Empty,"), "unit variant missing:\n{}", grep(&out, "Empty"));
}
