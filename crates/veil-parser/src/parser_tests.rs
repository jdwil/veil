#[cfg(test)]
mod tests {
    use crate::lexer::lex;
    use crate::parser::{parse, parse_with_registry};
    use veil_ir::ast::*;
    use veil_ir::layer::{LayerRegistry, Shape};

    /// Registry loaded from the real ddd.layer file — proves the engine
    /// learns its entire vocabulary from layer content at runtime.
    fn ddd_registry() -> LayerRegistry {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../examples/ddd.layer"))
            .expect("ddd layer should resolve");
        reg
    }

    fn parse_src(src: &str) -> Solution {
        let tokens = lex(src);
        parse_with_registry(&tokens, ddd_registry()).expect("parse failed")
    }

    fn find_construct<'a>(items: &'a [TopLevelItem], name: &str) -> &'a Construct {
        items
            .iter()
            .find_map(|i| match i {
                TopLevelItem::Construct(c) if c.name == name => Some(c),
                _ => None,
            })
            .unwrap_or_else(|| panic!("construct '{}' not found", name))
    }

    #[test]
    fn test_parse_empty_solution() {
        let sol = parse_src("sol MyApp");
        assert_eq!(sol.name, "MyApp");
        // The ddd.layer declares infrastructure (Bus/SagaStep traits) and the
        // saga coordinator functions, so they get injected. No user-authored
        // items exist; injected items are constructs or functions.
        assert!(sol.items.iter().all(|item| matches!(
            item,
            TopLevelItem::Construct(_) | TopLevelItem::Function(_)
        )));
        // The saga coordinator is injected as a layer-provided function.
        assert!(sol.items.iter().any(|item| matches!(
            item,
            TopLevelItem::Function(f) if f.name == "run_saga" && f.layer_provided
        )));
    }

    #[test]
    fn test_parse_solution_with_context() {
        let src = "sol App\n  ctx Users";
        let sol = parse_src(src);
        assert_eq!(sol.name, "App");
        let ctx = find_construct(&sol.items, "Users");
        assert_eq!(ctx.keyword, "ctx");
        assert_eq!(ctx.subkind, "Context");
        assert_eq!(ctx.shape, Shape::Mod);
    }

    #[test]
    fn test_parse_value_object() {
        let src = "sol App\n  ctx Identity\n    val Email\n      addr: Str";
        let sol = parse_src(src);
        let ctx = find_construct(&sol.items, "Identity");
        let vo = &ctx.children[0];
        assert_eq!(vo.keyword, "val");
        assert_eq!(vo.subkind, "ValueObject");
        assert_eq!(vo.shape, Shape::Struct);
        assert_eq!(vo.name, "Email");
        assert_eq!(vo.fields.len(), 1);
        assert_eq!(vo.fields[0].name, "addr");
        match &vo.fields[0].type_expr {
            TypeExpr::Named(n) => assert_eq!(n, "Str"),
            other => panic!("expected Named type, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_aggregate_with_events_and_commands() {
        let src = "\
sol App
  ctx Identity
    agg Customer
      root
        id: UUID
        email: Email

      evt CustomerCreated
        id email created

      cmd CreateCustomer
        email: Email
        phone: Phone
        -> Res!<Customer>";
        let sol = parse_src(src);
        let ctx = find_construct(&sol.items, "Identity");
        let agg = &ctx.children[0];
        assert_eq!(agg.subkind, "Aggregate");
        assert_eq!(agg.name, "Customer");
        // root fields land in a named block declared by the layer
        let root = agg.blocks.iter().find(|b| b.keyword == "root").expect("root block");
        assert_eq!(root.fields.len(), 2);
        // events and commands are generic children with layer subkinds
        let evt = agg.children.iter().find(|c| c.subkind == "Event").expect("event");
        assert_eq!(evt.name, "CustomerCreated");
        assert_eq!(evt.fields.len(), 3); // id, email, created (shorthand)
        let cmd = agg.children.iter().find(|c| c.subkind == "Command").expect("command");
        assert_eq!(cmd.name, "CreateCustomer");
        assert_eq!(cmd.fields.len(), 2);
        assert!(cmd.return_type.is_some());
    }

    #[test]
    fn test_parse_result_type() {
        let src = "\
sol App
  ctx X
    agg Y
      root
        id: UUID
      cmd DoThing
        -> Res!<Customer>";
        let sol = parse_src(src);
        let ctx = find_construct(&sol.items, "X");
        let agg = &ctx.children[0];
        let cmd = agg.children.iter().find(|c| c.subkind == "Command").unwrap();
        match cmd.return_type.as_ref().unwrap() {
            TypeExpr::Result(Some(inner)) => match inner.as_ref() {
                TypeExpr::Named(n) => assert_eq!(n, "Customer"),
                other => panic!("expected Named inside Result, got {:?}", other),
            },
            other => panic!("expected Result type, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_port() {
        let src = "\
sol App
  ctx Identity
    port Notifier
      send_sms(phone: Phone, msg: Str) -> Res!
      send_email(email: Email, subj: Str, body: Str) -> Res!";
        let sol = parse_src(src);
        let ctx = find_construct(&sol.items, "Identity");
        let port = &ctx.children[0];
        assert_eq!(port.subkind, "Port");
        assert_eq!(port.shape, Shape::Trait);
        assert_eq!(port.name, "Notifier");
        assert_eq!(port.methods.len(), 2);
        assert_eq!(port.methods[0].name, "send_sms");
        assert_eq!(port.methods[0].params.len(), 2);
        assert!(port.methods[0].return_type.is_some());
        assert_eq!(port.methods[1].name, "send_email");
        assert_eq!(port.methods[1].params.len(), 3);
    }

    #[test]
    fn test_parse_adapter() {
        let src = "\
sol App
  adapter SmsTwilio for Notifier
    @env(TWILIO_SID, TWILIO_TOKEN)
    impl send_sms(phone, msg)
      http.post(\"api.twilio.com/Messages\", {To: phone.number})";
        let sol = parse_src(src);
        let adapter = find_construct(&sol.items, "SmsTwilio");
        assert_eq!(adapter.subkind, "Adapter");
        assert_eq!(adapter.shape, Shape::Impl);
        assert_eq!(adapter.target.as_deref(), Some("Notifier"));
        assert_eq!(adapter.annotations.len(), 1);
        assert_eq!(adapter.annotations[0].name, "env");
        assert_eq!(adapter.annotations[0].args, vec!["TWILIO_SID", "TWILIO_TOKEN"]);
        assert_eq!(adapter.impls.len(), 1);
        assert_eq!(adapter.impls[0].method_name, "send_sms");
        assert_eq!(adapter.impls[0].params, vec!["phone", "msg"]);
    }

    #[test]
    fn test_parse_lang_block() {
        let src = "\
sol App
  lang
    Customer: person signing up for the platform
    KYC: Know Your Customer verification";
        let sol = parse_src(src);
        let lang = match &sol.items[0] {
            TopLevelItem::Lang(l) => l,
            _ => panic!("expected lang block"),
        };
        assert_eq!(lang.entries.len(), 2);
        assert_eq!(lang.entries[0].term, "Customer");
        assert_eq!(lang.entries[0].definition, "person signing up for the platform");
        assert_eq!(lang.entries[1].term, "KYC");
    }

    #[test]
    fn test_parse_flow_basic() {
        let src = "\
sol App
  flow Onboard
    @async @trace
    input
      email: Email
      name: Str
    step create_user
      c = call UserRepo.save(User.new(email))
    ret c.id";
        let sol = parse_src(src);
        let flow = match &sol.items[0] {
            TopLevelItem::Flow(f) => f,
            _ => panic!("expected flow"),
        };
        assert_eq!(flow.name, "Onboard");
        assert_eq!(flow.annotations.len(), 2);
        assert_eq!(flow.inputs.len(), 2);
        assert_eq!(flow.steps.len(), 1);
        assert!(flow.return_expr.is_some());
    }

    #[test]
    fn test_layer_statements_parse_as_actions() {
        let src = "\
sol App
  ctx C
    svc S
      step go
        dispatch UserCreated{id, email}
        guard x == 1, \"must be one\"";
        let sol = parse_src(src);
        let ctx = find_construct(&sol.items, "C");
        let svc = &ctx.children[0];
        assert_eq!(svc.subkind, "DomainService");
        let FlowStep::Step(step) = &svc.steps[0] else { panic!("expected step") };
        // dispatch is desugared to Expr::Call targeting Bus.dispatch with sugar preserved
        let Expr::Call(dispatch) = &step.body[0] else { panic!("expected Call (desugared dispatch)") };
        assert_eq!(dispatch.target, "Bus");
        assert_eq!(dispatch.method, "dispatch");
        assert_eq!(dispatch.sugar.as_deref(), Some("dispatch"));
        // The arg is a StructLit: UserCreated{id, email}
        assert_eq!(dispatch.args.len(), 1);
        assert!(matches!(&dispatch.args[0], Expr::StructLit(name, fields) if name == "UserCreated" && fields.len() == 2));
        // guard stays as Expr::Action (If shape, no port_target)
        let Expr::Action(guard) = &step.body[1] else { panic!("expected action") };
        assert_eq!(guard.keyword, "guard");
        assert!(guard.condition.is_some());
        assert_eq!(guard.message.as_deref(), Some("must be one"));
    }

    #[test]
    fn test_saga_with_compensate_and_ctx_refs() {
        let src = "\
sol App
  orchestrator O
    group application
      export saga Onboard
        contexts Identity, Billing
        input
          email: Email
        step create
          ctx Identity
          c = call Customer.new(email)
          compensate
            call CustomerRepo.delete(c.id)";
        let sol = parse_src(src);
        let orch = find_construct(&sol.items, "O");
        assert_eq!(orch.subkind, "Orchestrator");
        let group = &orch.children[0];
        assert_eq!(group.shape, Shape::Group);
        let saga = &group.children[0];
        assert_eq!(saga.subkind, "Saga");
        assert!(saga.exported);
        assert_eq!(saga.refs[0].keyword, "contexts");
        assert_eq!(saga.refs[0].values, vec!["Identity", "Billing"]);
        let FlowStep::Step(step) = &saga.steps[0] else { panic!("expected step") };
        assert_eq!(step.refs[0].keyword, "ctx");
        assert_eq!(step.refs[0].values, vec!["Identity"]);
        assert_eq!(step.sub_blocks.len(), 1);
        assert_eq!(step.sub_blocks[0].keyword, "compensate");
        assert_eq!(step.sub_blocks[0].body.len(), 1);
    }

    #[test]
    fn test_unknown_keyword_is_error_without_layer() {
        // Without the ddd layer, "ctx" is not a known construct.
        let tokens = lex("sol App\n  ctx Users");
        let result = parse(&tokens);
        assert!(result.is_err(), "ctx should be unknown to the bare engine");
    }

    #[test]
    fn test_stacked_layer_resolves_transitively() {
        // crm.layer maps pipeline->ctx->mod and lead->agg->struct.
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../examples/ddd.layer"))
            .expect("ddd layer");
        reg.load_content("crm", include_str!("../../../examples/crm.layer"))
            .expect("crm layer");

        let src = "\
sol Sales
  pipeline Outbound
    group domain
      lead Prospect
        root
          id: UUID
        signal LeadQualified
          id score: Int
      integration Enrichment
        enrich(domain: Str) -> Res!<Company>
      svc Qualify
        step s
          notify LeadQualified{id}";
        let tokens = lex(src);
        let sol = parse_with_registry(&tokens, reg).expect("stacked parse failed");
        let pipeline = find_construct(&sol.items, "Outbound");
        assert_eq!(pipeline.keyword, "pipeline");
        assert_eq!(pipeline.subkind, "Pipeline");
        assert_eq!(pipeline.shape, Shape::Mod); // pipeline -> ctx -> mod

        let group = &pipeline.children[0];
        let lead = &group.children[0];
        assert_eq!(lead.subkind, "Lead");
        assert_eq!(lead.shape, Shape::Struct); // lead -> agg -> struct
        assert_eq!(lead.blocks[0].keyword, "root"); // inherited block decl works
        let signal = &lead.children[0];
        assert_eq!(signal.subkind, "Signal"); // signal -> evt -> struct

        let integration = &group.children[1];
        assert_eq!(integration.subkind, "Integration");
        assert_eq!(integration.shape, Shape::Trait); // integration -> port -> trait

        // notify -> dispatch -> Bus.dispatch statement chain (desugared)
        let svc = &group.children[2];
        let FlowStep::Step(step) = &svc.steps[0] else { panic!("expected step") };
        let Expr::Call(notify) = &step.body[0] else { panic!("expected Call (desugared notify)") };
        assert_eq!(notify.target, "Bus");
        assert_eq!(notify.method, "dispatch");
        assert_eq!(notify.sugar.as_deref(), Some("notify"));
    }

    /// Extract the body expressions of the first step of `svc S` in a minimal
    /// solution, so expression-level parsing can be asserted directly.
    fn step_body(body_line: &str) -> Vec<Expr> {
        let src = format!(
            "sol App\n  ctx C\n    svc S\n      step go\n        {}",
            body_line
        );
        let sol = parse_src(&src);
        let ctx = find_construct(&sol.items, "C");
        let svc = &ctx.children[0];
        let FlowStep::Step(step) = &svc.steps[0] else { panic!("expected step") };
        step.body.clone()
    }

    #[test]
    fn test_paren_args_preserve_all_argument_kinds() {
        // Regression: parse_paren_args used to drop floats/bools and split
        // binary expressions into multiple args.
        let body = step_body("x = calc(1.5, true, y + 1)");
        let Expr::Assign(_, rhs) = &body[0] else { panic!("expected assign, got {:?}", body[0]) };
        let Expr::Call(call) = rhs.as_ref() else { panic!("expected call rhs") };
        assert_eq!(call.args.len(), 3, "expected exactly 3 args, got {:?}", call.args);
        assert!(matches!(&call.args[0], Expr::FloatLit(f) if (*f - 1.5).abs() < 1e-9));
        assert!(matches!(&call.args[1], Expr::BoolLit(true)));
        assert!(matches!(&call.args[2], Expr::BinaryOp(_)), "third arg should be `y + 1`, got {:?}", call.args[2]);
    }

    #[test]
    fn test_method_chaining_builds_single_expression() {
        // Regression: `a.b().c()` used to parse as two separate statements.
        let body = step_body("y = items.map(f).collect()");
        assert_eq!(body.len(), 1, "chain should be one statement, got {:?}", body);
        let Expr::Assign(_, rhs) = &body[0] else { panic!("expected assign") };
        // Outer call is `.collect()` with a receiver = `items.map(f)`.
        let Expr::Call(outer) = rhs.as_ref() else { panic!("expected call") };
        assert_eq!(outer.method, "collect");
        assert!(outer.target.is_empty());
        let recv = outer.receiver.as_ref().expect("collect should have a receiver");
        let Expr::Call(inner) = recv.as_ref() else { panic!("receiver should be items.map(f)") };
        assert_eq!(inner.target, "items");
        assert_eq!(inner.method, "map");
        assert_eq!(inner.args.len(), 1);
    }

    #[test]
    fn test_named_target_call_keeps_target() {
        // `Repo.find(id)` must keep the named target for codegen resolution.
        let body = step_body("lead = Repo.find(id)");
        let Expr::Assign(_, rhs) = &body[0] else { panic!("expected assign") };
        let Expr::Call(call) = rhs.as_ref() else { panic!("expected call") };
        assert_eq!(call.target, "Repo");
        assert_eq!(call.method, "find");
        assert!(call.receiver.is_none());
    }

    #[test]
    fn test_parse_full_example() {
        let src = include_str!("../../../examples/customer_onboarding.veil");
        let tokens = lex(src);
        let result = parse_with_registry(&tokens, ddd_registry());
        assert!(result.is_ok(), "Full example failed to parse: {:?}", result.err());
        let sol = result.unwrap();
        assert_eq!(sol.name, "CustomerOnboarding");
        // lang, ctx Identity, ctx Billing, orchestrator Onboarding
        assert!(sol.items.len() >= 4, "Expected at least 4 items, got {}", sol.items.len());
    }

    #[test]
    fn test_roundtrip_preserves_statement_sugar_and_hides_bus() {
        use veil_ir::serialize::serialize_solution;
        let src = include_str!("../../../examples/customer_onboarding.veil");
        let sol = {
            let tokens = lex(src);
            parse_with_registry(&tokens, ddd_registry()).expect("parse failed")
        };
        let emitted = serialize_solution(&sol);
        // Statement sugar survives: `dispatch Evt{...}`, not `call Bus.dispatch(...)`.
        assert!(emitted.contains("dispatch CustomerCreated"), "sugar lost:\n{}", emitted);
        assert!(!emitted.contains("call Bus.dispatch"), "sugar was desugared in output");
        // The injected Bus port is layer-provided and must not be written back.
        assert!(!emitted.contains("trait Bus"), "injected Bus leaked into source");
        assert!(!emitted.contains("port Bus"), "injected Bus leaked into source");
    }

    #[test]
    fn test_edit_rename_roundtrips_through_serializer() {
        use veil_ir::edit::{apply_edits, EditOp};
        use veil_ir::serialize::serialize_solution;
        let src = "sol App\n  ctx Identity\n    port Notifier\n      send(msg: Str) -> Res!";
        let mut sol = parse_src(src);
        // Locate the port's span start (as the viewer would from the IR node).
        let ctx = find_construct(&sol.items, "Identity");
        let port_span = ctx.children[0].span.start;
        assert_eq!(ctx.children[0].name, "Notifier");
        // Apply a rename edit and re-serialize.
        apply_edits(&mut sol, &[EditOp::Rename { span_start: port_span, name: "Alerts".to_string() }])
            .expect("edit should apply");
        let emitted = serialize_solution(&sol);
        assert!(emitted.contains("port Alerts"), "rename not serialized:\n{}", emitted);
        assert!(!emitted.contains("Notifier"), "old name still present:\n{}", emitted);
        // The edited source must re-parse cleanly.
        let reparsed = parse_src(&emitted);
        let ctx2 = find_construct(&reparsed.items, "Identity");
        assert_eq!(ctx2.children[0].name, "Alerts");
    }

    #[test]
    fn test_edit_set_fields_changes_struct() {
        use veil_ir::edit::{apply_edits, EditOp, FieldSpec};
        use veil_ir::serialize::serialize_solution;
        let src = "sol App\n  ctx Identity\n    val Email\n      addr: Str";
        let mut sol = parse_src(src);
        let ctx = find_construct(&sol.items, "Identity");
        let vo_span = ctx.children[0].span.start;
        apply_edits(&mut sol, &[EditOp::SetFields {
            span_start: vo_span,
            fields: vec![
                FieldSpec { name: "addr".to_string(), type_str: "Str".to_string() },
                FieldSpec { name: "verified".to_string(), type_str: "Bool".to_string() },
            ],
        }]).expect("edit should apply");
        let emitted = serialize_solution(&sol);
        assert!(emitted.contains("verified: Bool"), "new field missing:\n{}", emitted);
    }

    #[test]
    fn test_roundtrip_is_idempotent() {
        use veil_ir::serialize::serialize_solution;
        let src = include_str!("../../../examples/customer_onboarding.veil");
        let emit_once = {
            let tokens = lex(src);
            let sol = parse_with_registry(&tokens, ddd_registry()).expect("parse 1 failed");
            serialize_solution(&sol)
        };
        let emit_twice = {
            let tokens = lex(&emit_once);
            let sol = parse_with_registry(&tokens, ddd_registry()).expect("re-parse failed");
            serialize_solution(&sol)
        };
        // A load→save cycle must reach a fixed point so editing never drifts.
        assert_eq!(emit_once, emit_twice, "serializer is not idempotent");
    }
}
