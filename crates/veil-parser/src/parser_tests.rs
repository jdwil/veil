#[cfg(test)]
mod tests {
    use crate::lexer::lex;
    use crate::parser::parse;
    use veil_ir::ast::*;

    fn parse_src(src: &str) -> Solution {
        let tokens = lex(src);
        parse(&tokens).expect("parse failed")
    }

    #[test]
    fn test_parse_empty_solution() {
        let sol = parse_src("sol MyApp");
        assert_eq!(sol.name, "MyApp");
        assert!(sol.items.is_empty());
    }

    #[test]
    fn test_parse_solution_with_context() {
        let src = "sol App\n  ctx Users";
        let sol = parse_src(src);
        assert_eq!(sol.name, "App");
        assert_eq!(sol.items.len(), 1);
        match &sol.items[0] {
            TopLevelItem::Context(ctx) => assert_eq!(ctx.name, "Users"),
            other => panic!("expected Context, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_value_object() {
        let src = "sol App\n  ctx Identity\n    val Email\n      addr: Str";
        let sol = parse_src(src);
        let ctx = match &sol.items[0] {
            TopLevelItem::Context(ctx) => ctx,
            _ => panic!("expected context"),
        };
        let vo = match &ctx.items[0] {
            ContextItem::ValueObject(vo) => vo,
            _ => panic!("expected value object"),
        };
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
      id: UUID
      email: Email

      evt CustomerCreated
        id email created

      cmd CreateCustomer
        email: Email
        phone: Phone
        -> Res!<Customer>";
        let sol = parse_src(src);
        let ctx = match &sol.items[0] {
            TopLevelItem::Context(c) => c,
            _ => panic!("expected context"),
        };
        let agg = match &ctx.items[0] {
            ContextItem::Aggregate(a) => a,
            _ => panic!("expected aggregate"),
        };
        assert_eq!(agg.name, "Customer");
        assert_eq!(agg.fields.len(), 2);
        assert_eq!(agg.events.len(), 1);
        assert_eq!(agg.events[0].name, "CustomerCreated");
        assert_eq!(agg.events[0].fields.len(), 3); // id, email, created (shorthand)
        assert_eq!(agg.commands.len(), 1);
        assert_eq!(agg.commands[0].name, "CreateCustomer");
        assert_eq!(agg.commands[0].fields.len(), 2);
        assert!(agg.commands[0].return_type.is_some());
    }

    #[test]
    fn test_parse_result_type() {
        let src = "\
sol App
  ctx X
    agg Y
      cmd DoThing
        -> Res!<Customer>";
        let sol = parse_src(src);
        let ctx = match &sol.items[0] {
            TopLevelItem::Context(c) => c,
            _ => panic!("expected context"),
        };
        let agg = match &ctx.items[0] {
            ContextItem::Aggregate(a) => a,
            _ => panic!("expected aggregate"),
        };
        let rt = agg.commands[0].return_type.as_ref().unwrap();
        match rt {
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
        let ctx = match &sol.items[0] {
            TopLevelItem::Context(c) => c,
            _ => panic!("expected context"),
        };
        let port = match &ctx.items[0] {
            ContextItem::Port(p) => p,
            _ => panic!("expected port"),
        };
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
    @env TWILIO_SID TWILIO_TOKEN
    impl send_sms(phone, msg)
      http.post(\"api.twilio.com/Messages\", {To: phone.number})";
        let sol = parse_src(src);
        let adapter = match &sol.items[0] {
            TopLevelItem::Adapter(a) => a,
            _ => panic!("expected adapter"),
        };
        assert_eq!(adapter.name, "SmsTwilio");
        assert_eq!(adapter.target_port, "Notifier");
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
    fn test_parse_full_example() {
        let src = include_str!("../../../examples/customer_onboarding.veil");
        let tokens = lex(src);
        let result = parse(&tokens);
        assert!(result.is_ok(), "Full example failed to parse: {:?}", result.err());
        let sol = result.unwrap();
        assert_eq!(sol.name, "CustomerOnboarding");
        // Should have: lang, ctx Identity, ctx Billing, flow Onboard, adapter x2
        assert!(sol.items.len() >= 5, "Expected at least 5 items, got {}", sol.items.len());
    }
}
