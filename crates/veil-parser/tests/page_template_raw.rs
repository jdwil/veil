#[test]
fn page_with_route_and_template_parses() {
    let src = r#"
pkg T
  use svelte5
  app A
    group pages
      page Home
        @route("/")
        template """
          <div>hi</div>
        """
"#;
    // load layers from workspace
    let mut reg = veil_ir::LayerRegistry::builtin();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../layers");
    let _ = reg.load_layer("svelte5", &dir);
    let tokens = veil_parser::lex(src);
    let file = veil_parser::parse_file_with_registry(&tokens, reg).expect("parse");
    match file {
        veil_ir::VeilFile::Package(p) => {
            // find page with raw template
            fn walk(c: &veil_ir::Construct) -> bool {
                if c.keyword == "page" && c.raw_blocks.iter().any(|(k, _)| k == "template") {
                    return true;
                }
                c.children.iter().any(walk)
            }
            let ok = p.items.iter().any(|i| match i {
                veil_ir::TopLevelItem::Construct(c) => walk(c),
                _ => false,
            });
            assert!(ok, "expected page with template raw block");
        }
        _ => panic!("expected package"),
    }
}
