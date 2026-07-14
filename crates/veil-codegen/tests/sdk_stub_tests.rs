//! Stub/SDK adapter lowering — generic (no engine hardcoding of crate families).

use veil_ir::LayerRegistry;

fn generate_with_stub(stub_src: &str, app_src: &str) -> String {
    let mut reg = LayerRegistry::builtin();
    reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
        .expect("ddd");
    if let Some(stub) = veil_ir::parse_stub_file(stub_src) {
        reg.stubs.push(stub);
    }
    let tokens = veil_parser::lex(app_src);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse");
    let project = veil_codegen::generate(&sol, &reg);
    project
        .files
        .iter()
        .map(|f| format!("// ==== {} ====\n{}", f.path, f.content))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Minimal stub: types under `types::`, Client at root, harness_field recipe.
const MINI_SDK_STUB: &str = r#"
stub example-sdk 1.0.0
cargo_deps helper-crate=1
types_module types
root_types Client

harness_field Client """
{
    example_sdk::Client::from_env()
}
"""

  struct Client
    fn put_item() -> PutItemFluentBuilder
    fn from_env() -> Self

  struct PutItemFluentBuilder
    fn table_name(input: Str) -> Self
    fn item(k: Str, v: AttributeValue) -> Self
    fn send() -> Res!<PutItemOutput>

  struct PutItemOutput

  enum AttributeValue
    S(Str)
    N(Str)
"#;

#[test]
fn attribute_value_s_keeps_pascal_case_and_types_module() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port ThingRepo
        save!(id: Id, name: Str)

    group infrastructure
      impl SdkThingRepo for ThingRepo
        @dep
        @field(client: Client)
        @env(TABLE_NAME)

        impl save(id, name)
          self.client.put_item().table_name(self.table).item("id", AttributeValue.S(id.to_string())).item("name", AttributeValue.S(name)).send()
          ret Ok
"#;
    let out = generate_with_stub(MINI_SDK_STUB, app);
    assert!(
        out.contains("example_sdk::types::AttributeValue::S("),
        "types_module must qualify AttributeValue:\n{}",
        out.lines()
            .filter(|l| l.contains("Attribute") || l.contains("put_item"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(!out.contains("AttributeValue::s("));
    assert!(
        out.contains("self.client.put_item()"),
        "self.client must lower to field access"
    );
    assert!(
        out.contains(".send().await") && out.contains("map_err"),
        "send must be async+fallible"
    );
    assert!(
        out.contains("pub client: example_sdk::Client"),
        "Client stays at crate root via root_types"
    );
    assert!(!out.contains("not configured"));
}

#[test]
fn harness_uses_stub_harness_field_not_engine_hardcode() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port ThingRepo
        save!(id: Id)

    group application
      @main
      svc CreateThing
        input
          id: Id
        step persist
          ThingRepo.save!(id)
          ret Ok

    group infrastructure
      impl SdkThingRepo for ThingRepo
        @dep
        @field(client: Client)
        @env(TABLE_NAME)

        impl save(id)
          self.client.put_item().table_name(self.table).item("id", AttributeValue.S(id.to_string())).send()
          ret Ok
"#;
    let out = generate_with_stub(MINI_SDK_STUB, app);
    assert!(
        out.contains("example_sdk::Client::from_env()"),
        "harness must paste stub harness_field recipe:\n{}",
        out.lines()
            .filter(|l| l.contains("Client") || l.contains("from_env") || l.contains("harness"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Companion cargo_deps appear in workspace / bin
    assert!(
        out.contains("helper-crate") || out.contains("example-sdk"),
        "stub cargo_deps / crate must appear in Cargo.toml"
    );
    // Engine must not invent aws-specific symbols
    assert!(
        !out.contains("aws_config") && !out.contains("aws_sdk_dynamodb"),
        "engine must not hardcode AWS crates"
    );
}
