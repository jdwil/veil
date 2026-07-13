#[cfg(test)]
mod tests {
    use crate::lexer::lex;
    use crate::parser::{parse, parse_file_with_registry, parse_with_registry};
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

    /// CAP-001: `link` declares external Cargo crates.
    #[test]
    fn test_parse_link_decls() {
        let src = r#"
pkg Host
  use ddd
  link veil_server
  link veil-local path "../../crates/veil-local" features "local,http"
  link "custom-crate" path "../vendor/custom"
"#;
        let sol = parse_src(src);
        assert_eq!(sol.links.len(), 3, "expected 3 links, got {:?}", sol.links);
        assert_eq!(sol.links[0].name, "veil_server");
        assert!(sol.links[0].path.is_none());
        assert_eq!(sol.links[1].name, "veil-local");
        assert_eq!(
            sol.links[1].path.as_deref(),
            Some("../../crates/veil-local")
        );
        assert_eq!(sol.links[1].features, vec!["local".to_string(), "http".to_string()]);
        assert_eq!(sol.links[2].name, "custom-crate");
        assert_eq!(sol.links[2].path.as_deref(), Some("../vendor/custom"));

        // Round-trip via serializer
        let out = veil_ir::serialize::serialize_solution(&sol);
        assert!(out.contains("link veil_server"), "{out}");
        assert!(out.contains("link veil-local path \"../../crates/veil-local\""), "{out}");
        assert!(out.contains("features \"local, http\"") || out.contains("features \"local,http\""), "{out}");
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
        let Expr::Assign(_, rhs, _) = &body[0] else { panic!("expected assign, got {:?}", body[0]) };
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
        let Expr::Assign(_, rhs, _) = &body[0] else { panic!("expected assign") };
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
    fn test_step_and_par_usable_as_variables() {
        // Regression: `step`/`par` were reserved tokens and broke as loop/var
        // names. They are now layer vocabulary (idents), recognized contextually.
        let src = "\
sol S
  ctx C
    svc F
      step iterate
        for step in items
          call step.run(bus)
        par = compute()";
        let sol = parse_src(src);
        let ctx = find_construct(&sol.items, "C");
        let svc = &ctx.children[0];
        let FlowStep::Step(step) = &svc.steps[0] else { panic!("expected step") };
        assert_eq!(step.name, "iterate");
        // The for-loop binding named `step`, the `.run` call on it, and the
        // `par` assignment all parse without treating them as keywords.
        assert!(step.body.iter().any(|e| matches!(e, Expr::ForLoop { binding, .. } if binding == "step")));
        assert!(step.body.iter().any(|e| matches!(e, Expr::Assign(n, _, _) if n == "par")));
    }

    #[test]
    fn test_named_target_call_keeps_target() {
        // `Repo.find(id)` must keep the named target for codegen resolution.
        let body = step_body("lead = Repo.find(id)");
        let Expr::Assign(_, rhs, _) = &body[0] else { panic!("expected assign") };
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

    /// SER-005: SetBody parses real expressions (not opaque Idents).
    #[test]
    fn test_edit_set_body_parses_real_exprs() {
        use veil_ir::serialize::serialize_solution;
        let src = r#"
pkg App
  use ddd
  svc Greet
    step run
      x = 0
"#;
        let mut sol = parse_src(src);
        let svc = find_construct(&sol.items, "Greet");
        let FlowStep::Step(step) = &svc.steps[0] else {
            panic!("expected step");
        };
        let step_span = step.span.start;
        crate::apply_edits(
            &mut sol,
            &[veil_ir::EditOp::SetBody {
                span_start: step_span,
                body: vec![
                    "name = \"world\"".into(),
                    "UserRepo.save(name)".into(),
                ],
            }],
            &ddd_registry(),
        )
        .expect("set_body");

        let svc = find_construct(&sol.items, "Greet");
        let FlowStep::Step(step) = &svc.steps[0] else {
            panic!("expected step");
        };
        assert!(
            matches!(&step.body[0], Expr::Assign(n, _, None) if n == "name"),
            "expected Assign, got {:?}",
            step.body[0]
        );
        assert!(
            matches!(&step.body[1], Expr::Call(c) if c.target == "UserRepo" && c.method == "save"),
            "expected Call, got {:?}",
            step.body[1]
        );

        let emitted = serialize_solution(&sol);
        assert!(emitted.contains("name = \"world\""), "emit lost assign:\n{}", emitted);
        assert!(
            emitted.contains("UserRepo.save(name)"),
            "emit lost call:\n{}",
            emitted
        );
        assert!(!emitted.contains("call UserRepo"), "must not emit call kw:\n{}", emitted);
        // Re-parse edited source cleanly.
        let _ = parse_src(&emitted);
    }

    /// SER-005: invalid body text fails the edit (no opaque Ident fallback).
    #[test]
    fn test_edit_set_body_invalid_returns_error() {
        let src = r#"
pkg App
  use ddd
  svc Greet
    step run
      x = 0
"#;
        let mut sol = parse_src(src);
        let svc = find_construct(&sol.items, "Greet");
        let FlowStep::Step(step) = &svc.steps[0] else {
            panic!("expected step");
        };
        let step_span = step.span.start;
        let err = crate::apply_edits(
            &mut sol,
            &[veil_ir::EditOp::SetBody {
                span_start: step_span,
                body: vec!["((( not valid".into()],
            }],
            &ddd_registry(),
        );
        assert!(err.is_err(), "expected InvalidBody, got {:?}", err);
        // Original body preserved.
        let svc = find_construct(&sol.items, "Greet");
        let FlowStep::Step(step) = &svc.steps[0] else {
            panic!();
        };
        assert!(
            matches!(&step.body[0], Expr::Assign(n, _, _) if n == "x"),
            "body corrupted: {:?}",
            step.body
        );
    }

    #[test]
    fn test_parse_expr_str_handles_if() {
        let e = crate::parse_expr_str(
            "if true\n  x = 1\nelse\n  x = 0",
            &ddd_registry(),
        )
        .expect("parse if");
        assert!(matches!(e, Expr::IfExpr(_)), "got {:?}", e);
    }

    /// SER-006: delete construct persists through re-serialize.
    #[test]
    fn test_edit_delete_construct_roundtrips() {
        use veil_ir::edit::{apply_edits, EditOp};
        use veil_ir::serialize::serialize_solution;
        let src = r#"
pkg App
  use ddd
  ctx Identity
    val Email
      addr: Str
    val Phone
      number: Str
"#;
        let mut sol = parse_src(src);
        let ctx = find_construct(&sol.items, "Identity");
        let phone_span = ctx
            .children
            .iter()
            .find(|c| c.name == "Phone")
            .expect("Phone")
            .span
            .start;
        apply_edits(
            &mut sol,
            &[EditOp::DeleteConstruct {
                span_start: phone_span,
            }],
        )
        .expect("delete");
        let emitted = serialize_solution(&sol);
        assert!(emitted.contains("val Email"), "Email must remain:\n{}", emitted);
        assert!(!emitted.contains("Phone"), "Phone must be gone:\n{}", emitted);
        assert!(!emitted.contains("number: Str"), "Phone fields gone:\n{}", emitted);
        let reparsed = parse_src(&emitted);
        let ctx2 = find_construct(&reparsed.items, "Identity");
        assert_eq!(ctx2.children.len(), 1);
        assert_eq!(ctx2.children[0].name, "Email");
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

    /// SER-001: field annotations (@dep) and defaults survive parse → emit → parse.
    #[test]
    fn test_roundtrip_preserves_field_annotations_and_defaults() {
        use veil_ir::serialize::serialize_solution;

        let mut reg = LayerRegistry::builtin();
        reg.load_content("di", include_str!("../../../layers/di.layer"))
            .expect("di layer");
        reg.load_content("ddd", include_str!("../../../examples/ddd.layer"))
            .ok();

        let src = r#"
pkg DiRoundtrip
  use di
  struct PgTenantRepo
    @dep
    @env(DATABASE_URL)
    pool: Pool
    count: Int = 0
  svc Create
    input
      @dep
      repo: PgTenantRepo
      name: Str
"#;
        let tokens = lex(src);
        let sol1 = parse_with_registry(&tokens, reg.clone()).expect("parse 1");
        let emitted = serialize_solution(&sol1);
        assert!(
            emitted.contains("@dep"),
            "emit dropped @dep:\n{}",
            emitted
        );
        assert!(
            emitted.contains("@env(DATABASE_URL)"),
            "emit dropped @env:\n{}",
            emitted
        );
        assert!(
            emitted.contains("count: Int = 0"),
            "emit dropped default:\n{}",
            emitted
        );

        let tokens2 = lex(&emitted);
        let sol2 = parse_with_registry(&tokens2, reg).expect("parse 2 after emit");

        fn walk<'a>(c: &'a Construct, name: &str) -> Option<&'a Construct> {
            if c.name == name {
                return Some(c);
            }
            c.children.iter().find_map(|ch| walk(ch, name))
        }
        fn find_named<'a>(items: &'a [TopLevelItem], name: &str) -> Option<&'a Construct> {
            items.iter().find_map(|i| match i {
                TopLevelItem::Construct(c) => walk(c, name),
                _ => None,
            })
        }

        let repo = find_named(&sol2.items, "PgTenantRepo").expect("PgTenantRepo after roundtrip");
        let pool = repo
            .fields
            .iter()
            .find(|f| f.name == "pool")
            .expect("pool field");
        assert!(
            pool.annotations.iter().any(|a| a.name == "dep"),
            "pool lost @dep: {:?}",
            pool.annotations
        );
        assert!(
            pool.annotations
                .iter()
                .any(|a| a.name == "env" && a.args.iter().any(|x| x.contains("DATABASE_URL"))),
            "pool lost @env: {:?}",
            pool.annotations
        );
        let count = repo
            .fields
            .iter()
            .find(|f| f.name == "count")
            .expect("count field");
        assert!(
            matches!(count.default_expr, Some(Expr::IntLit(0))),
            "count lost default: {:?}",
            count.default_expr
        );

        let create = find_named(&sol2.items, "Create").expect("Create svc");
        let repo_in = create
            .inputs
            .iter()
            .find(|f| f.name == "repo")
            .expect("repo input");
        assert!(
            repo_in.annotations.iter().any(|a| a.name == "dep"),
            "input lost @dep: {:?}",
            repo_in.annotations
        );
    }

    /// SER-002: control-flow bodies re-serialize and re-parse without `"..."`.
    #[test]
    fn test_roundtrip_control_flow_bodies() {
        use veil_ir::serialize::serialize_solution;

        let src = r#"
pkg Ctrl
  use ddd
  fn Demo
    step s
      if x > 0
        y = 1
      else
        y = 0
      if let Some(v) = opt
        process(v)
      while let Some(v) = it
        process(v)
      while running
        break
      loop
        break
      for i in items
        process(i)
      match x
        A if n > 0 ->
          y = 1
        _ ->
          y = 0
      mut total: Int = 0
"#;
        let sol1 = parse_with_registry(&lex(src), ddd_registry()).expect("parse control flow");
        let emitted = serialize_solution(&sol1);
        assert!(
            !emitted.contains("..."),
            "placeholder in emit:\n{}",
            emitted
        );
        assert!(emitted.contains("if x > 0"), "if lost:\n{}", emitted);
        assert!(emitted.contains("else"), "else lost:\n{}", emitted);
        assert!(
            emitted.contains("if let Some") && emitted.contains("= opt"),
            "if let lost:\n{}",
            emitted
        );
        assert!(
            emitted.contains("while let Some") && emitted.contains("= it"),
            "while let lost:\n{}",
            emitted
        );
        assert!(emitted.contains("while running"), "while lost:\n{}", emitted);
        assert!(emitted.contains("loop"), "loop lost:\n{}", emitted);
        assert!(emitted.contains("for i in items"), "for lost:\n{}", emitted);
        assert!(emitted.contains("match x"), "match lost:\n{}", emitted);
        assert!(
            emitted.contains("if n > 0") || emitted.contains("A if"),
            "match guard lost:\n{}",
            emitted
        );
        assert!(
            emitted.contains("mut total: Int = 0"),
            "typed mut lost:\n{}",
            emitted
        );

        let sol2 = parse_with_registry(&lex(&emitted), ddd_registry()).expect("reparse control flow");
        // Spot-check a nested if survived as IfExpr
        fn find_fn<'a>(items: &'a [TopLevelItem], name: &str) -> Option<&'a Construct> {
            items.iter().find_map(|i| match i {
                TopLevelItem::Construct(c) if c.name == name => Some(c),
                TopLevelItem::Construct(c) => c.children.iter().find(|ch| ch.name == name),
                _ => None,
            })
        }
        let demo = find_fn(&sol2.items, "Demo").expect("Demo");
        let FlowStep::Step(step) = &demo.steps[0] else {
            panic!("expected step");
        };
        assert!(
            step.body.iter().any(|e| matches!(e, Expr::IfExpr(_))),
            "if not reparsed: {:?}",
            step.body
        );
        assert!(
            step.body.iter().any(|e| matches!(e, Expr::Match(_, _))),
            "match not reparsed: {:?}",
            step.body
        );
        assert!(
            step.body
                .iter()
                .any(|e| matches!(e, Expr::MutAssign(n, _, Some(_)) if n == "total")),
            "mut typed not reparsed: {:?}",
            step.body
        );
    }

    /// SER-003: typed immutable assign `name: Type = expr` round-trips.
    #[test]
    fn test_typed_assign_roundtrip() {
        use veil_ir::serialize::serialize_solution;
        let src = r#"
pkg T
  use ddd
  svc S
    step s
      cohort: CohortDTO = request GetCohort{id: x}
      members: List<M> = request GetMembers{id: y}
"#;
        let once = {
            let sol = parse_with_registry(&lex(src), ddd_registry()).expect("parse");
            serialize_solution(&sol)
        };
        assert!(
            once.contains("cohort: CohortDTO ="),
            "typed assign lost:\n{}",
            once
        );
        assert!(
            once.contains("members: List<M> ="),
            "generic typed assign lost:\n{}",
            once
        );
        assert!(
            !once.lines().any(|l| l.trim() == "cohort"),
            "bare name leak:\n{}",
            once
        );
        let twice = {
            let sol = parse_with_registry(&lex(&once), ddd_registry()).expect("reparse");
            serialize_solution(&sol)
        };
        assert_eq!(once, twice, "typed assign not idempotent");
    }

    /// SER-003: second emit is a no-op on a clean canonical tree.
    #[test]
    fn test_emit_idempotent_hello_world() {
        use veil_ir::serialize::serialize_solution;
        let src = include_str!("../../../examples/hello_world.veil");
        let once = {
            let sol = parse_with_registry(&lex(src), ddd_registry()).expect("parse");
            serialize_solution(&sol)
        };
        let twice = {
            let sol = parse_with_registry(&lex(&once), ddd_registry()).expect("reparse");
            serialize_solution(&sol)
        };
        assert_eq!(once, twice, "emit not idempotent");
        assert!(once.starts_with("pkg "), "canonical pkg keyword: {}", &once[..20.min(once.len())]);
        assert!(!once.contains("\ncall "), "must not emit call keyword:\n{}", once);
    }

    /// SER-003: di_example remains idempotent (no call-call churn).
    #[test]
    fn test_emit_idempotent_di_example() {
        use veil_ir::serialize::serialize_solution;
        let mut reg = LayerRegistry::builtin();
        for (name, content) in [
            ("base", include_str!("../../../layers/base.layer")),
            ("di", include_str!("../../../layers/di.layer")),
            ("rust", include_str!("../../../layers/rust.layer")),
        ] {
            reg.load_content(name, content).expect(name);
        }
        let src = include_str!("../../../examples/di_example.veil");
        let once = {
            let sol = parse_with_registry(&lex(src), reg.clone()).expect("parse");
            serialize_solution(&sol)
        };
        let twice = {
            let sol = parse_with_registry(&lex(&once), reg).expect("reparse");
            serialize_solution(&sol)
        };
        assert_eq!(once, twice, "di_example not idempotent");
        assert!(!once.contains("call call"), "call keyword doubled:\n{}", once);
        assert!(
            once.contains("self.pool.") || once.contains("self.pool"),
            "adapter body missing:\n{}",
            once
        );
    }

    /// SER-001: di_example.veil field @dep survives round-trip.
    #[test]
    fn test_di_example_preserves_dep_on_roundtrip() {
        use veil_ir::serialize::serialize_solution;

        let mut reg = LayerRegistry::builtin();
        for (name, path) in [
            ("base", "../../../layers/base.layer"),
            ("di", "../../../layers/di.layer"),
            ("rust", "../../../layers/rust.layer"),
        ] {
            // layers/ paths
            let content = match name {
                "base" => include_str!("../../../layers/base.layer"),
                "di" => include_str!("../../../layers/di.layer"),
                "rust" => include_str!("../../../layers/rust.layer"),
                _ => "",
            };
            let _ = path;
            reg.load_content(name, content).expect(name);
        }

        let src = include_str!("../../../examples/di_example.veil");
        let sol1 = parse_with_registry(&lex(src), reg.clone()).expect("parse di_example");
        let emitted = serialize_solution(&sol1);
        assert!(
            emitted.contains("@dep"),
            "di_example emit lost @dep:\n{}",
            emitted
        );
        // @main on bootstrap
        assert!(
            emitted.contains("@main") || emitted.contains("@main\n"),
            "di_example emit lost @main:\n{}",
            emitted
        );

        let sol2 = parse_with_registry(&lex(&emitted), reg).expect("reparse di_example");
        fn walk<'a>(c: &'a Construct, name: &str) -> Option<&'a Construct> {
            if c.name == name {
                return Some(c);
            }
            c.children.iter().find_map(|ch| walk(ch, name))
        }
        let repo = sol2
            .items
            .iter()
            .find_map(|i| match i {
                TopLevelItem::Construct(c) => walk(c, "PgTenantRepo"),
                _ => None,
            })
            .expect("PgTenantRepo");
        let pool = repo.fields.iter().find(|f| f.name == "pool").expect("pool");
        assert!(
            pool.annotations.iter().any(|a| a.name == "dep"),
            "PgTenantRepo.pool lost @dep after round-trip: {:?}",
            pool.annotations
        );
    }

    // ─── ADP: package adapt ─────────────────────────────────────────────

    #[test]
    fn parse_adapt_and_patches() {
        let src = r#"
pkg Client
  use base
  adapt stock_pkg
  ren ListThings ListPrograms
  omit Legacy
  ins CreateThing
    step audit after persist
      ret Ok
  rfn CreateThing
    step wrap
      init = stock
      ret init
  rpl Archive
    step
      ret Ok
  svc Extra
    step go
      ret Ok
"#;
        let tokens = lex(src);
        let file = parse_file_with_registry(&tokens, ddd_registry()).expect("parse adapt");
        let veil_ir::VeilFile::Package(pkg) = file else {
            panic!("expected package");
        };
        assert_eq!(pkg.adapts.len(), 1);
        assert_eq!(pkg.adapts[0].package_name, "stock_pkg");
        assert_eq!(pkg.patches.len(), 5);
        assert!(matches!(pkg.patches[0], veil_ir::AdaptPatch::Ren { .. }));
        assert!(matches!(pkg.patches[1], veil_ir::AdaptPatch::Omit { .. }));
        assert!(matches!(pkg.patches[2], veil_ir::AdaptPatch::Ins { .. }));
        assert!(matches!(pkg.patches[3], veil_ir::AdaptPatch::Rfn { .. }));
        assert!(matches!(pkg.patches[4], veil_ir::AdaptPatch::Rpl { .. }));
        // stock expr present in rfn
        if let veil_ir::AdaptPatch::Rfn { steps, .. } = &pkg.patches[3] {
            let has_stock = steps.iter().any(|st| match st {
                veil_ir::FlowStep::Step(sd) => {
                    sd.body.iter().any(|e| matches!(e, veil_ir::Expr::Stock)
                        || matches!(e, veil_ir::Expr::Assign(_, rhs, _) if matches!(rhs.as_ref(), veil_ir::Expr::Stock)))
                }
                _ => false,
            });
            assert!(has_stock, "expected stock in rfn body");
        }
        // serialize round-trip preserves adapt
        let emitted = veil_ir::serialize_package(&pkg);
        assert!(emitted.contains("adapt stock_pkg"));
        assert!(emitted.contains("ren ListThings ListPrograms"));
        assert!(emitted.contains("omit Legacy"));
        assert!(emitted.contains("ins CreateThing"));
        assert!(emitted.contains("rfn CreateThing"));
        assert!(emitted.contains("stock"));
    }

    #[test]
    fn parse_adapt_path_step_fn() {
        let src = r#"
pkg P
  omit CreateThing.step persist
  ren Initiative.fn mark_vip mark_vip_client
"#;
        let tokens = lex(src);
        let file = parse_file_with_registry(&tokens, ddd_registry()).expect("parse paths");
        let veil_ir::VeilFile::Package(pkg) = file else {
            panic!("expected package");
        };
        assert_eq!(pkg.patches.len(), 2);
        if let veil_ir::AdaptPatch::Omit { path, .. } = &pkg.patches[0] {
            assert_eq!(path.display(), "CreateThing.step persist");
        } else {
            panic!("omit");
        }
        if let veil_ir::AdaptPatch::Ren { path, new_name, .. } = &pkg.patches[1] {
            assert_eq!(path.display(), "Initiative.fn mark_vip");
            assert_eq!(new_name, "mark_vip_client");
        } else {
            panic!("ren");
        }
    }
}
