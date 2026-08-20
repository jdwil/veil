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
async_methods send

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
          self.client.put_item().table_name(self.table_name).item("id", AttributeValue.S(id.to_string())).item("name", AttributeValue.S(name)).send()
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
    let save_body: String = {
        let lines: Vec<&str> = out.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.contains("impl ThingRepo for") || l.contains("SdkThingRepo"))
            .and_then(|i| {
                lines[i..].iter().position(|l| l.contains("async fn save(")).map(|j| i + j)
            })
            .or_else(|| {
                lines.iter().rposition(|l| l.contains("async fn save(") && l.contains("id:"))
            })
            .unwrap_or(0);
        lines[start..start.saturating_add(25).min(lines.len())].join("\n")
    };
    assert!(
        save_body.contains(".send().await"),
        "stub async_methods on this type must await the call:\n{save_body}"
    );
    assert!(
        out.contains("pub client: example_sdk::Client"),
        "Client stays at crate root via root_types"
    );
    assert!(
        out.contains("pub table_name:") || out.contains("self.table_name"),
        "@env(TABLE_NAME) must be the full snake field table_name:\n{}",
        out.lines()
            .filter(|l| l.contains("table") || l.contains("env"))
            .collect::<Vec<_>>()
            .join("\n")
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
          self.client.put_item().table_name(self.table_name).item("id", AttributeValue.S(id.to_string())).send()
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

/// Row-driver policy lives on the stub — engine applies derives without naming the crate.
const ROWDB_STUB: &str = r#"
stub rowdb 1.0.0
cargo_features runtime
row_type_derives rowdb::FromRow
wrapper_type_derives rowdb::Type
wrapper_type_attrs rowdb(transparent)
codegen_imports rowdb::Pool

  struct Query
    typed_variant query_as
    typed_type_params _, return_type
    fn new(sql: Str) -> Self
    fn bind(value: T) -> Self
    fn fetch_all(executor: E) -> Res!<List<O>>

  struct QueryAs
    fn bind(value: T) -> Self
    fn fetch_all(executor: E) -> Res!<List<O>>

  struct Pool
    fn connect(url: Str) -> Res!<Self>
"#;

#[test]
fn stub_row_type_derives_on_domain_structs() {
    let app = r#"
pkg DbApp
  use ddd
  use rowdb

  ctx Store
    group domain
      val Email
        addr: Str

      val ThingDTO
        id: Id
        name: Str
        email: Email

      port ThingRepo
        list!() -> List<ThingDTO>

    group infrastructure
      impl PgThingRepo for ThingRepo
        @dep
        @env(DATABASE_URL)
        impl list()
          rows = rowdb.Query.new("SELECT * FROM things").fetch_all!(pool)
          ret rows
"#;
    let out = generate_with_stub(ROWDB_STUB, app);
    assert!(
        out.contains("rowdb::FromRow"),
        "multi-field DTO must get row_type_derives:\n{}",
        out.lines()
            .filter(|l| l.contains("ThingDTO") || l.contains("derive") || l.contains("FromRow"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("rowdb::Type") && out.contains("rowdb(transparent)"),
        "single-field wrapper must get wrapper derives/attrs:\n{}",
        out.lines()
            .filter(|l| l.contains("Email") || l.contains("Type") || l.contains("transparent"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("use rowdb::Pool"),
        "codegen_imports must appear in adapters"
    );
    // No sqlx symbols — engine must not inject a specific driver
    assert!(
        !out.contains("sqlx::"),
        "must not hardcode sqlx when using a different stub:\n{}",
        out
    );
}

#[test]
fn stub_typed_variant_constructor() {
    let app = r#"
pkg DbApp
  use ddd
  use rowdb

  ctx Store
    group domain
      val ThingDTO
        id: Id
        name: Str

      port ThingRepo
        list!() -> List<ThingDTO>

    group infrastructure
      impl PgThingRepo for ThingRepo
        @dep
        @env(DATABASE_URL)
        impl list()
          rows = rowdb.Query.new("SELECT 1").fetch_all!(pool)
          ret rows
"#;
    let out = generate_with_stub(ROWDB_STUB, app);
    assert!(
        out.contains("rowdb::query_as::<_, ThingDTO>")
            || out.contains("rowdb::query_as::<_, ThingDTO>("),
        "typed_variant must emit free-fn with domain type:\n{}",
        out.lines()
            .filter(|l| l.contains("query") || l.contains("Query") || l.contains("ThingDTO"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// When a stub/struct method shares a name with a port method, suffix choice
/// follows the receiver's Shape (Struct → sync map_err; not trait .await?).
#[test]
fn stub_method_name_collision_with_port_uses_sync_suffix() {
    let stub = r#"
stub facade_store path:../facade_store
  struct Facade
    fn get_version(root: Str, id: Str) -> Res!<Str>
    fn package_root(root: Str, id: Str) -> Str
"#;
    let app = r#"
pkg Collide
  use ddd
  use facade_store

  ctx Ext
    group domain
      port VersionPort
        get_version!(id: Id) -> Opt<Str>
        package_root!(id: Id) -> Str

    group infrastructure
      adapter FileVersion for VersionPort
        @env(ROOT_DIR)
        impl get_version(id)
          raw = Facade.get_version!(self.dir, f"{id}")
          if raw == ""
            ret null
          ret raw
        impl package_root(id)
          ret Facade.package_root(self.dir, f"{id}")
"#;
    let out = generate_with_stub(stub, app);
    // Facade is a stub struct — must not be lowered as port .await?
    assert!(
        out.contains("facade_store::Facade::get_version")
            && out.contains("map_err(|e| DomainError::External"),
        "sync Res! stub method must map_err, not await:\n{}",
        out.lines()
            .filter(|l| l.contains("Facade") || l.contains("get_version") || l.contains("await"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("Facade::get_version") || !out.lines().any(|l| {
            l.contains("Facade::get_version") && l.contains(".await")
        }),
        "Facade::get_version must not use .await (port name collision):\n{}",
        out
    );
    // Non-Res! package_root: no await on Facade
    let pkg_lines: Vec<_> = out
        .lines()
        .filter(|l| l.contains("package_root"))
        .collect();
    assert!(
        pkg_lines.iter().any(|l| l.contains("Facade::package_root")),
        "expected Facade::package_root call:\n{}",
        pkg_lines.join("\n")
    );
    assert!(
        !pkg_lines
            .iter()
            .any(|l| l.contains("Facade::package_root") && l.contains(".await")),
        "Facade::package_root must not await:\n{}",
        pkg_lines.join("\n")
    );
}

#[test]
fn invented_crate_name_is_unstubbed_not_empty_hook() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port ThingRepo
        save!(id: Id)

    group infrastructure
      impl SdkThingRepo for ThingRepo
        @dep
        impl save(id)
          aws_sns.publish!({ topic: "t" })
          ret Ok
"#;
    let out = generate_with_stub(MINI_SDK_STUB, app);
    assert!(
        out.contains("unstubbed external") && out.contains("compile_error!"),
        "invented aws_sns must fail closed:\n{}",
        out.lines()
            .filter(|l| l.contains("aws") || l.contains("unstubbed") || l.contains("hook"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("fn aws_sns_publish("),
        "must not emit no-op hook:\n{}",
        out
    );
}

fn generate_with_stubs(stubs: &[&str], app_src: &str) -> String {
    let mut reg = LayerRegistry::builtin();
    reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
        .expect("ddd");
    for stub_src in stubs {
        if let Some(stub) = veil_ir::parse_stub_file(stub_src) {
            reg.stubs.push(stub);
        }
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

const SNS_STUB: &str = r#"
stub aws-sdk-sns 1.0.0
root_types Client
harness_field Client """
{ aws_sdk_sns::Client::from_env() }
"""

  struct Client
    fn publish() -> PublishFluentBuilder

  struct PublishFluentBuilder
    fn topic_arn(input: Str) -> Self
    fn message(input: Str) -> Self
    fn send() -> Res!
"#;

const DDB_STUB: &str = r#"
stub aws-sdk-dynamodb 1.0.0
root_types Client
harness_field Client """
{ aws_sdk_dynamodb::Client::from_env() }
"""

  struct Client
    fn put_item() -> PutItemFluentBuilder

  struct PutItemFluentBuilder
    fn table_name(input: Str) -> Self
    fn send() -> Res!
"#;

#[test]
fn qualified_client_fields_do_not_collapse_to_last_stub() {
    let app = r#"
pkg BusApp
  use ddd
  use aws_sdk_sns
  use aws_sdk_dynamodb

  ctx Store
    group domain
      port SnsPort
        publish!(topic: Str, body: Str)
      port DdbPort
        put!(id: Str)

    group infrastructure
      impl SnsAd for SnsPort
        @field(sns: aws_sdk_sns.Client)
        impl publish(topic, body)
          self.sns.publish().topic_arn(topic).message(body).send!()
          ret Ok
      impl DdbAd for DdbPort
        @field(ddb: aws_sdk_dynamodb.Client)
        impl put(id)
          self.ddb.put_item().table_name(id).send!()
          ret Ok
"#;
    let out = generate_with_stubs(&[SNS_STUB, DDB_STUB], app);
    assert!(
        out.contains("pub sns: aws_sdk_sns::Client"),
        "SNS field must be sns crate Client:\n{}",
        out.lines()
            .filter(|l| l.contains("sns") || l.contains("Client"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("pub ddb: aws_sdk_dynamodb::Client"),
        "DDB field must be dynamodb crate Client:\n{}",
        out.lines()
            .filter(|l| l.contains("ddb") || l.contains("Client"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("pub sns: aws_sdk_dynamodb::Client"),
        "SNS must not steal DynamoDB Client"
    );
    assert!(
        out.contains("self.sns.publish()") && out.contains("self.ddb.put_item()"),
        "fields must keep crate-specific names:\n{}",
        out
    );
}

#[test]
fn adapter_dep_port_field_lowers_to_arc_dyn_and_self_call() {
    let app = r#"
pkg OrchApp
  use ddd
  use aws_sdk_sns

  ctx Store
    group domain
      port SnsClient
        publish!(topic: Str, body: Str)
      port EventListener
        on_event!(topic: Str, body: Str)

    group infrastructure
      impl SnsAd for SnsClient
        @field(sns: aws_sdk_sns.Client)
        impl publish(topic, body)
          self.sns.publish().topic_arn(topic).message(body).send!()
          ret Ok
      impl EventOrch for EventListener
        @dep sns_client: SnsClient
        impl on_event(topic, body)
          sns_client.publish!(topic, body)
          ret Ok
"#;
    let out = generate_with_stubs(&[SNS_STUB], app);
    assert!(
        out.contains("pub sns_client: std::sync::Arc<dyn SnsClient + Send + Sync>"),
        "@dep field must be Arc<dyn Port>:\n{}",
        out.lines()
            .filter(|l| l.contains("sns_client") || l.contains("struct EventOrch"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("self.sns_client.publish(") && out.contains(".await?"),
        "port field call must lower to self.field.method().await?:\n{}",
        out.lines()
            .filter(|l| l.contains("sns_client") || l.contains("publish"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("unstubbed external `sns_client"),
        "must not treat injected port as unstubbed:\n{}",
        out
    );
}

#[test]
fn product_port_reusing_layer_declare_name_does_not_starve_shared() {
    let app = r#"
pkg AuthShadow
  use ddd

  ctx Store
    group domain
      port AuthService
        validate_token(token: Str) -> Res!<Principal>
      port ThingRepo
        save!(id: Id)

    group infrastructure
      impl NoopThing for ThingRepo
        impl save(id)
          ret Ok
"#;
    let out = generate_with_stubs(&[], app);
    let shared = out
        .split("// ==== crates/veil_shared/src/lib.rs ====")
        .nth(1)
        .unwrap_or("");
    assert!(
        shared.contains("pub trait AuthService") && shared.contains("async fn validate_token"),
        "veil_shared must emit layer AuthService even when product defines port AuthService:\n{}",
        shared.lines().take(80).collect::<Vec<_>>().join("\n")
    );
    assert!(
        !shared.contains("pub trait Bus"),
        "DDD must not inject Bus into veil_shared:\n{}",
        shared.lines().take(40).collect::<Vec<_>>().join("\n")
    );
    let ports = out
        .split("crates/")
        .filter(|s| s.contains("/src/ports/mod.rs"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ports.contains("pub trait AuthService"),
        "product port AuthService stays in the product crate:\n{}",
        ports.lines().take(50).collect::<Vec<_>>().join("\n")
    );
    assert!(
        !ports.contains("pub use veil_shared::*;"),
        "must not glob-import veil_shared when product also defines AuthService:\n{}",
        ports.lines().take(30).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn bare_enum_variant_is_qualified() {
    let app = r#"
pkg Shop
  use ddd

  ctx Store
    group domain
      enum StockState
        Ready
        SoldOut
      port StockRepo
        mark!(state: StockState)
    group application
      handler MarkReady
        input
          @dep stock_repo: StockRepo
        step go
          stock_repo.mark!(Ready)
          ret Ok
    group infrastructure
      impl MemStock for StockRepo
        impl mark(state)
          label = match state
            Ready -> "ready"
            SoldOut -> "sold"
          ret Ok
"#;
    let out = generate_with_stubs(&[], app);
    assert!(
        out.contains("StockState::Ready"),
        "bare variant Ready must qualify as StockState::Ready:\n{}",
        out.lines()
            .filter(|l| l.contains("Ready") || l.contains("mark") || l.contains("match"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("StockState::SoldOut"),
        "match arms must qualify SoldOut:\n{}",
        out.lines()
            .filter(|l| l.contains("Sold") || l.contains("match") || l.contains("Ready"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.lines().any(|l| l.trim_start().starts_with("Ready =>")),
        "bare Ready => must not appear in match:\n{out}"
    );
}

#[test]
fn check_flags_bang_in_unit_from_source() {
    let app = r#"
pkg P
  use ddd
  ctx C
    group domain
      port Bus
        dispatch(envelope: Str) -> ()
      port Sns
        publish!(msg: Str)
    group infrastructure
      adapter AwsBus for Bus
        impl dispatch(envelope)
          Sns.publish!(envelope)
"#;
    let mut reg = LayerRegistry::builtin();
    reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
        .expect("ddd");
    let tokens = veil_parser::lex(app);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse");
    let result = veil_ir::check::check_solution(&sol, &reg);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == "bang_in_unit_fn"),
        "{:?}",
        result
            .diagnostics
            .iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn env_space_form_and_map_lit_and_stub_alias() {
    let app = r#"
pkg StoreApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port ThingRepo
        save!(name: Str, attrs: Map<Str, Str>)
        put_raw!(item: Map<Str, AttributeValue>)

    group infrastructure
      adapter SdkThing for ThingRepo
        @field(client: Client)
        @env TABLE_NAME
        impl save(name, attrs)
          self.client.put_item().table_name(self.table_name).item("name", AttributeValue.S(name)).send!()
          ret Ok
        impl put_raw(item)
          self.client.put_item().table_name(self.table_name).set_item(item).send!()
          ret Ok
"#;
    let out = generate_with_stub(MINI_SDK_STUB, app);
    assert!(
        out.contains("pub table_name:") && out.contains("self.table_name"),
        "@env TABLE_NAME (no parens) must become self.table_name:\n{}",
        out.lines()
            .filter(|l| l.contains("table") || l.contains("env"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("pub type AttributeValue = example_sdk::types::AttributeValue"),
        "stub AttributeValue must alias the crate type, not String:\n{}",
        out.lines()
            .filter(|l| l.contains("AttributeValue") || l.contains("pub type"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn map_literal_lowers_to_hashmap_not_json() {
    let app = r#"
pkg BusApp
  use ddd

  ctx Store
    group domain
      port SnsClient
        publish!(topic: Str, message: Str, attributes: Map<Str, Str>)
      port Broadcaster
        fanout!(name: Str)

    group infrastructure
      adapter Fan for Broadcaster
        @dep sns_client: SnsClient
        impl fanout(name)
          sns_client.publish!("t", name, { event_name: name })
          ret Ok
"#;
    let out = generate_with_stubs(&[], app);
    assert!(
        out.contains("HashMap::new()") && out.contains("__m.insert(\"event_name\""),
        "Map<Str,Str> literal must be HashMap, not json!:\n{}",
        out.lines()
            .filter(|l| l.contains("event_name") || l.contains("json!") || l.contains("HashMap"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("serde_json::json!({ \"event_name\""),
        "must not emit json! for a Map param:\n{out}"
    );
}

#[test]
fn unit_port_without_bang_does_not_question_mark() {
    let app = r#"
pkg UnitApp
  use ddd

  ctx Store
    group domain
      port Bus
        dispatch(envelope: Str) -> ()
      handler HandleIt
        input
          envelope: Str
          @dep bus: Bus
        step go
          bus.dispatch(envelope)
"#;
    let out = generate_with_stubs(&[], app);
    let dispatch_lines: Vec<_> = out
        .lines()
        .filter(|l| l.contains("dispatch"))
        .collect();
    assert!(
        dispatch_lines.iter().any(|l| l.contains(".await") && !l.contains(".await?")),
        "unit dispatch without ! must be .await, not .await?:\n{}",
        dispatch_lines.join("\n")
    );
}

const NOISE_PUBLISH_STUB: &str = r#"
stub noise-sdk 1.0.0
types_module types
root_types Client

  struct Client
    fn publish() -> PublishFluentBuilder

  struct PublishFluentBuilder
    fn send() -> Res!<PublishOutput>

  struct PublishOutput
"#;

#[test]
fn map_literal_on_port_stays_hashmap_when_stub_also_has_publish() {
    let app = r#"
pkg Shop
  use ddd
  use noise_sdk

  ctx Store
    group domain
      port Publisher
        publish!(topic: Str, message: Str, attributes: Map<Str, Str>)
      port Broadcaster
        fanout!(name: Str)

    group infrastructure
      adapter Fan for Broadcaster
        @dep publisher: Publisher
        impl fanout(name)
          publisher.publish!("t", name, { event_name: name })
          ret Ok
"#;
    let out = generate_with_stubs(&[NOISE_PUBLISH_STUB], app);
    assert!(
        out.contains("HashMap::new()") && out.contains("__m.insert(\"event_name\""),
        "port Map param must stay HashMap even when a stub also has publish():\n{}",
        out.lines()
            .filter(|l| l.contains("event_name") || l.contains("json!") || l.contains("HashMap") || l.contains("publish"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("serde_json::json!({ \"event_name\""),
        "must not emit json! for a port Map param:\n{out}"
    );
}

#[test]
fn blob_new_uses_stub_type_path_not_vec_u8() {
    let stub = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Blob
    path primitives
    fn new(data: Bytes) -> Self

  struct Client
    fn invoke() -> InvokeFluentBuilder

  struct InvokeFluentBuilder
    fn payload(input: Blob) -> Self
    fn send() -> Res!<InvokeOutput>

  struct InvokeOutput
    fn payload() -> Opt<Blob>
"#;
    let app = r#"
pkg FnApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Runner
        run!(body: Str) -> Str

    group infrastructure
      adapter SdkRunner for Runner
        @field(client: example_sdk.Client)
        impl run(body)
          result = self.client.invoke().payload(Blob.new(body)).send!()
          ret "ok"
"#;
    let out = generate_with_stub(stub, app);
    assert!(
        out.contains("example_sdk::primitives::Blob::new("),
        "Blob.new must use the stub path, not Vec<u8>:\n{}",
        out.lines()
            .filter(|l| l.contains("Blob") || l.contains("into_bytes") || l.contains("payload"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("__s.into_bytes()\n                    })"),
        "must not emit a bare Vec<u8> as the payload:\n{out}"
    );
}

#[test]
fn blob_to_str_and_ret_unit_as_none() {
    let stub = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Blob
    path primitives
    fn new(data: Bytes) -> Self

  struct Client
    fn invoke() -> InvokeFluentBuilder

  struct InvokeFluentBuilder
    fn payload(input: Blob) -> Self
    fn send() -> Res!<InvokeOutput>

  struct InvokeOutput
    fn payload() -> Opt<Blob>
"#;
    let app = r#"
pkg FnApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      val Token
        id: Str
      port Runner
        run!(body: Str) -> Str
        find!(id: Str) -> Opt<Token>
    group infrastructure
      adapter SdkRunner for Runner
        @field(client: example_sdk.Client)
        impl run(body)
          result = self.client.invoke().payload(Blob.new(body)).send!()
          blob = require result.payload
          ret blob.to_str()
        impl find(id)
          result = self.client.invoke().payload(Blob.new(id)).send!()
          item = result.payload
          if item.is_some
            ret Token { id: id }
          ret ()
"#;
    let out = generate_with_stub(stub, app);
    assert!(
        out.contains("from_utf8_lossy") && out.contains(".as_ref()"),
        "blob.to_str() must decode utf-8:\n{}",
        out.lines()
            .filter(|l| l.contains("payload") || l.contains("utf8") || l.contains("to_str"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let find_fn: String = {
        let lines: Vec<&str> = out.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.contains("async fn find(") && l.contains("Token"))
            .expect("find fn");
        lines[start..start.saturating_add(40).min(lines.len())].join("\n")
    };
    assert!(
        find_fn.contains("return Ok(None)") || out.contains("return Ok(None)"),
        "ret () on Opt port must be Ok(None):\n{find_fn}"
    );
}

#[test]
fn crate_qualified_blob_new_is_type_new_not_module_fn() {
    let stub = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Blob
    path primitives
    fn new(data: Bytes) -> Self

  struct Client
    fn invoke() -> InvokeFluentBuilder

  struct InvokeFluentBuilder
    fn payload(input: Blob) -> Self
    fn send() -> Res!<InvokeOutput>

  struct InvokeOutput
    fn payload() -> Opt<Blob>
"#;
    let app = r#"
pkg FnApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Runner
        run!(body: Str) -> Str
    group infrastructure
      adapter SdkRunner for Runner
        @field(client: example_sdk.Client)
        impl run(body)
          result = self.client.invoke().payload(example_sdk.Blob.new(body)).send!()
          ret (require result.payload()).to_str()
"#;
    let out = generate_with_stub(stub, app);
    assert!(
        out.contains("example_sdk::primitives::Blob::new("),
        "crate.Blob.new must be Type::new, not crate::blob():\n{}",
        out.lines()
            .filter(|l| l.contains("blob") || l.contains("Blob") || l.contains("payload"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("example_sdk::blob("),
        "sqlx Query.new free-fn heuristic must not steal Blob.new:\n{out}"
    );
}

#[test]
fn opt_match_last_expr_wraps_some_and_none_value() {
    let stub = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Client
    fn send() -> Res!
"#;
    let app = r#"
pkg FnApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      enum Mode
        Sync
        Async
      val Token
        id: Str
      port Runner
        go!(mode: Mode, id: Str) -> Opt<Token>
    group infrastructure
      adapter SdkRunner for Runner
        @field(client: example_sdk.Client)
        impl go(mode, id)
          match mode
            Sync -> Token { id: id }
            Async ->
              self.client.send!()
              null
"#;
    let out = generate_with_stub(stub, app);
    let go_fn: String = {
        let lines: Vec<&str> = out.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.contains("async fn go("))
            .expect("go fn");
        lines[start..start.saturating_add(50).min(lines.len())].join("\n")
    };
    assert!(
        go_fn.contains("match mode") && !go_fn.contains("match Some(mode)"),
        "scrutinee must not be Some-wrapped:\n{go_fn}"
    );
    assert!(
        go_fn.contains("Some(Token") || go_fn.contains("Some( Token"),
        "Sync arm of Opt method must wrap Some:\n{go_fn}"
    );
    assert!(
        go_fn.contains("None") && !go_fn.contains("None;"),
        "Async arm last null must be value None, not None;:\n{go_fn}"
    );
    assert!(
        !go_fn.contains("}.map_err"),
        "match last-expr must not get Result map_err just because arms await:\n{go_fn}"
    );
}

#[test]
fn check_flags_stub_getter_returned_as_domain() {
    let stub = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Client
    fn get_item() -> GetItemFluentBuilder

  struct GetItemFluentBuilder
    fn send() -> Res!<GetItemOutput>

  struct GetItemOutput
    fn item() -> Opt<HashMap<Str, AttributeValue>>

  enum AttributeValue
    S(Str)
"#;
    let app = r#"
pkg Shop
  use ddd
  use example_sdk

  ctx Store
    group domain
      val Record
        name: Str
      port Routes
        get_route!(name: Str) -> Opt<Record>
    group infrastructure
      adapter SdkRoutes for Routes
        @field(client: example_sdk.Client)
        impl get_route(name)
          result = self.client.get_item().send!()
          ret result.item
"#;
    let mut reg = LayerRegistry::builtin();
    reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
        .expect("ddd");
    if let Some(s) = veil_ir::parse_stub_file(stub) {
        reg.stubs.push(s);
    }
    let tokens = veil_parser::lex(app);
    let sol = veil_parser::parse_with_registry(&tokens, reg.clone()).expect("parse");
    let result = veil_ir::check::check_solution(&sol, &reg);
    assert!(
        result.diagnostics.iter().any(|d| d.code == "type_mismatch"
            && d.message.contains("Record")
            && (d.message.contains("Map") || d.message.contains("HashMap"))),
        "{:?}",
        result
            .diagnostics
            .iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect::<Vec<_>>()
    );
}

const AV_STUB: &str = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Client
    fn get_item() -> GetItemFluentBuilder

  struct GetItemFluentBuilder
    fn send() -> Res!<GetItemOutput>

  struct GetItemOutput
    fn item() -> Opt<HashMap<Str, AttributeValue>>

  enum AttributeValue
    S(Str)
    fn as_s() -> Res!<Str>
    fn as_n() -> Res!<Str>

  struct MessageAttributeValue
    fn builder() -> MessageAttributeValueBuilder

  struct MessageAttributeValueBuilder
    fn data_type(input: Str) -> Self
    fn string_value(input: Str) -> Self
    fn build() -> Res!<MessageAttributeValue>
"#;

#[test]
fn res_str_getter_owns_string_and_debug_map_err() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      val Record
        name: Str
      port Routes
        load!(id: Str) -> Opt<Record>
    group infrastructure
      adapter SdkRoutes for Routes
        @field(client: example_sdk.Client)
        impl load(id)
          result = self.client.get_item().send!()
          item = require result.item()
          name = require item.get("name").as_s!()
          ret Record { name: name }
"#;
    let out = generate_with_stub(AV_STUB, app);
    let as_s_lines: String = out
        .lines()
        .filter(|l| l.contains("as_s") || l.contains("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        out.contains(".as_s()") && out.contains("s.to_string()") && out.contains("{e:?}"),
        "Res!<Str> getter must own String and Debug-map_err:\n{as_s_lines}"
    );
    assert!(
        !as_s_lines.contains("e.to_string()"),
        "must not Display-map_err stub errors:\n{as_s_lines}"
    );
    assert!(
        !out.contains(".as_s!()"),
        "bang is VEIL sugar, not a Rust method:\n{out}"
    );
}

#[test]
fn match_string_patterns_on_res_str_keeps_try() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      enum Kind
        Healthy
        Dead
      val Record
        kind: Kind
      port Routes
        load!(id: Str) -> Opt<Record>
    group infrastructure
      adapter SdkRoutes for Routes
        @field(client: example_sdk.Client)
        impl load(id)
          result = self.client.get_item().send!()
          item = require result.item()
          status_str = require item.get("status")
          kind = match status_str.as_s!()
            "Healthy" -> Kind.Healthy
            _ -> Kind.Dead
          ret Record { kind: kind }
"#;
    let out = generate_with_stub(AV_STUB, app);
    let has_try_then_as_str = out.contains("?.as_str()")
        || (out.contains("s.to_string()") && out.contains(".as_str()"));
    assert!(
        has_try_then_as_str,
        "string-pattern match must unwrap Res then as_str, not as_str on Result:\n{}",
        out.lines()
            .filter(|l| l.contains("as_s") || l.contains("as_str") || l.contains("match"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn require_bang_port_unwraps_remaining_option() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      val Record
        endpoint: Str
      port Routes
        get_route!(name: Str) -> Opt<Record>
      port Bus
        invoke!(name: Str) -> Str
    group infrastructure
      adapter SdkBus for Bus
        @dep routing_table: Routes
        impl invoke(name)
          route = require routing_table.get_route!(name)
          ret route.endpoint
      adapter SdkRoutes for Routes
        @field(client: example_sdk.Client)
        impl get_route(name)
          ret null
"#;
    let out = generate_with_stub(AV_STUB, app);
    assert!(
        out.contains("get_route(") && out.contains(".ok_or(DomainError::NotFound)?"),
        "require on bang port that returns Opt must unwrap Option after Res:\n{}",
        out.lines()
            .filter(|l| {
                l.contains("get_route") || l.contains("ok_or") || l.contains("endpoint")
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn string_concat_uses_format() {
    let app = r###"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Routes
        put!(svc: Str, handler: Str)
    group infrastructure
      adapter SdkRoutes for Routes
        @field(client: example_sdk.Client)
        impl put(svc, handler)
          key = "LISTENER#" + svc + "#" + handler
          ret Ok
"###;
    let out = generate_with_stub(AV_STUB, app);
    assert!(
        out.contains("format!(\"{}{}{}{}\"") || out.contains("format!(\"{}{}\""),
        "Str + Str must lower to format!, not Rust +:\n{}",
        out.lines()
            .filter(|l| l.contains("LISTENER") || l.contains("format") || l.contains('+'))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("\"LISTENER#\".to_string() +") && !out.contains("\"LISTENER#\" +"),
        "must not emit String + String:\n{out}"
    );
}

#[test]
fn pkg_qualified_type_uses_rust_type_path() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Svc
        publish!(name: Str)
    group infrastructure
      adapter SdkSvc for Svc
        @field(client: example_sdk.Client)
        impl publish(name)
          attr = example_sdk.MessageAttributeValue.builder().data_type("String").string_value(name).build!()
          ret Ok
"#;
    let out = generate_with_stub(AV_STUB, app);
    assert!(
        out.contains("example_sdk::types::MessageAttributeValue::builder()"),
        "pkg.Type must use rust_type_path (types_module):\n{}",
        out.lines()
            .filter(|l| l.contains("MessageAttribute") || l.contains("builder"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("example_sdk::MessageAttributeValue::builder()"),
        "must not drop types_module:\n{out}"
    );
}

#[test]
fn str_now_iso8601_is_rfc3339() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Clock
        stamp!() -> Str
    group infrastructure
      adapter SysClock for Clock
        impl stamp()
          now = Str.now_iso8601()
          ret now
"#;
    let out = generate_with_stub(AV_STUB, app);
    assert!(
        out.contains("Utc::now().to_rfc3339()"),
        "Str.now_iso8601 must be chrono rfc3339:\n{}",
        out.lines()
            .filter(|l| l.contains("now") || l.contains("iso") || l.contains("compile_error"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("unstubbed external"),
        "must not treat Str.now_iso8601 as an external stub:\n{out}"
    );
}

#[test]
fn blob_as_ref_in_str_position_decodes_utf8() {
    let stub = r#"
stub example-sdk 1.0.0
types_module types
root_types Client
async_methods send

  struct Blob
    path primitives
    fn new(data: Bytes) -> Self
    fn as_ref() -> Str

  struct Client
    fn invoke() -> InvokeFluentBuilder

  struct InvokeFluentBuilder
    fn payload(input: Blob) -> Self
    fn send() -> Res!<InvokeOutput>

  struct InvokeOutput
    fn payload() -> Opt<Blob>
"#;
    let app = r#"
pkg FnApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Runner
        run!(body: Str) -> Str
    group infrastructure
      adapter SdkRunner for Runner
        @field(client: example_sdk.Client)
        impl run(body)
          result = self.client.invoke().payload(Blob.new(body)).send!()
          response_blob = require result.payload()
          ret response_blob.as_ref()
"#;
    let out = generate_with_stub(stub, app);
    assert!(
        out.contains("from_utf8_lossy") && out.contains(".as_ref()"),
        "as_ref in a Str slot must decode utf-8, not return &[u8]:\n{}",
        out.lines()
            .filter(|l| l.contains("as_ref") || l.contains("utf8") || l.contains("response_blob"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !out.contains("return Ok(response_blob.as_ref())")
            && !out.contains("Ok(response_blob.as_ref())"),
        "must not emit raw Blob.as_ref() as String:\n{out}"
    );
}

#[test]
fn field_reuse_across_two_puts_clones() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      val Route
        message_name: Str
        handler_name: Str
      port Routes
        put!(entry: Route)
    group infrastructure
      adapter SdkRoutes for Routes
        @field(client: example_sdk.Client)
        impl put(entry)
          self.client.put_item().item("pk", AttributeValue.S(entry.message_name)).send()
          self.client.put_item().item("pk", AttributeValue.S(entry.message_name)).item("handler", AttributeValue.S(entry.handler_name)).send()
          ret Ok
"#;
    let out = generate_with_stub(MINI_SDK_STUB, app);
    let put_fn: String = {
        let lines: Vec<&str> = out.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.contains("async fn put(") && l.contains("Route"))
            .expect("put fn");
        lines[start..start.saturating_add(50).min(lines.len())].join("\n")
    };
    assert!(
        put_fn.contains("entry.message_name.clone()")
            && put_fn.contains("entry.handler_name.clone()"),
        "reused struct fields must clone (VEIL values are reusable):\n{put_fn}"
    );
    assert!(
        put_fn.matches("entry.message_name.clone()").count() >= 2,
        "both put_item uses of message_name must clone:\n{put_fn}"
    );
}

#[test]
fn rustdoc_type_param_url_accepts_str() {
    let stub = r#"
stub example-http 1.0.0
  struct Client
    fn post(url: U) -> RequestBuilder
  struct RequestBuilder
    fn body(body: T) -> Self
    fn send() -> Res!<Response>
  struct Response
    fn text() -> Res!<Str>
"#;
    let app = r#"
pkg SdkApp
  use ddd
  use example_http

  ctx Store
    group domain
      port Http
        post!(url: Str, body: Str) -> Str
    group infrastructure
      adapter Req for Http
        @field(http: example_http.Client)
        impl post(url, body)
          resp = self.http.post(url).body(body).send!()
          ret resp.text!()
"#;
    let out = generate_with_stub(stub, app);
    assert!(
        !out.contains("unstubbed external"),
        "generic U/T params must lower: {out}"
    );
    assert!(
        out.contains(".post(") && out.contains(".body("),
        "must emit reqwest-style chain:\n{out}"
    );
}

#[test]
fn int_now_unix_and_parse_int_and_as_n_are_i64() {
    let app = r#"
pkg SdkApp
  use ddd
  use example_sdk

  ctx Store
    group domain
      port Clock
        age!(raw: Str) -> Int
    group infrastructure
      adapter SysClock for Clock
        impl age(raw)
          now = Int.now_unix()
          n = raw.parse_int()
          ret now - n
"#;
    let out = generate_with_stub(AV_STUB, app);
    assert!(
        out.contains("Utc::now().timestamp()"),
        "Int.now_unix must be a unix timestamp:\n{}",
        out.lines()
            .filter(|l| l.contains("now") || l.contains("unix") || l.contains("compile_error"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        out.contains("parse::<i64>()"),
        "parse_int must parse i64:\n{out}"
    );
    assert!(
        !out.contains("unstubbed external"),
        "must not treat Int.now_unix as an external stub:\n{out}"
    );
}
