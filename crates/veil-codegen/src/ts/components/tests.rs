//! Tests for the Svelte 5 component generation module.

use super::*;
use veil_ir::ast::*;
use veil_ir::Span;
use veil_ir::layer::{ReactivityPolicy, Shape};

// ─── Test Helpers ────────────────────────────────────────────────────────────

fn svelte5_registry() -> LayerRegistry {
    let mut registry = LayerRegistry::default();
    registry.reactivity_policy = ReactivityPolicy {
        props_call: "$props()".to_string(),
        state_line: "let {name} = $state<{type}>({default})".to_string(),
        derived_line: "let {name} = $derived({expr})".to_string(),
        effect_sync: "$effect(() => {{ // {name}\n{body}\n  }})".to_string(),
        effect_async: "$effect(() => {{ // {name}\n    void (async () => {{\n{body}\n    }})();\n  }})".to_string(),
        bindable: "$bindable()".to_string(),
        bindable_default: "$bindable({default})".to_string(),
    };
    registry
}

fn empty_registry() -> LayerRegistry {
    LayerRegistry::default()
}

fn make_component(name: &str) -> Construct {
    Construct::new("component", "Component", Shape::Struct, name.to_string(), Span::default())
}

fn make_field(name: &str, ty: &str) -> Field {
    Field {
        annotations: vec![],
        name: name.to_string(),
        type_expr: TypeExpr::Named(ty.to_string()),
        default_expr: None,
        span: Span::default(),
    }
}

fn make_field_with_default(name: &str, ty: &str, default: Expr) -> Field {
    Field {
        annotations: vec![],
        name: name.to_string(),
        type_expr: TypeExpr::Named(ty.to_string()),
        default_expr: Some(default),
        span: Span::default(),
    }
}

fn make_named_block(keyword: &str, fields: Vec<Field>) -> NamedBlock {
    NamedBlock {
        keyword: keyword.to_string(),
        shape: Shape::Struct,
        name: None,
        fields,
        variants: vec![],
        transitions: vec![],
        span: Span::default(),
    }
}

// ─── Script Section Tests ────────────────────────────────────────────────────

#[test]
fn test_props_with_svelte5_runes() {
    let mut comp = make_component("UserCard");
    comp.blocks.push(make_named_block("props", vec![
        make_field("name", "Str"),
        make_field("email", "Str"),
    ]));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("interface Props {"));
    assert!(result.content.contains("name: string;"));
    assert!(result.content.contains("email: string;"));
    assert!(result.content.contains("let { name, email }: Props = $props();"));
    assert!(result.path.contains("UserCard.svelte"));
}

#[test]
fn test_props_with_optional_fields() {
    let mut comp = make_component("Card");
    comp.blocks.push(make_named_block("props", vec![
        make_field("title", "Str"),
        make_field_with_default("subtitle", "Str", Expr::StringLit("default".to_string())),
    ]));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("title: string;"));
    assert!(result.content.contains("subtitle?: string;"));
    assert!(result.content.contains("subtitle = \"default\""));
}

#[test]
fn test_state_with_svelte5_runes() {
    let mut comp = make_component("Counter");
    comp.blocks.push(make_named_block("state", vec![
        make_field_with_default("count", "Int", Expr::IntLit(0)),
        make_field_with_default("label", "Str", Expr::StringLit("clicks".to_string())),
    ]));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("let count = $state<number>(0)"));
    assert!(result.content.contains("let label = $state<string>(\"clicks\")"));
}

#[test]
fn test_derived_with_svelte5_runes() {
    let mut comp = make_component("DoubleCounter");
    comp.blocks.push(make_named_block("state", vec![
        make_field_with_default("count", "Int", Expr::IntLit(0)),
    ]));
    comp.blocks.push(make_named_block("derived", vec![
        make_field_with_default("doubled", "Int", Expr::BinaryOp(BinaryOpExpr {
            left: Box::new(Expr::Ident("count".to_string())),
            op: BinOp::Mul,
            right: Box::new(Expr::IntLit(2)),
        })),
    ]));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("$state<number>(0)"));
    assert!(result.content.contains("let doubled = $derived(count * 2)"));
}

#[test]
fn test_effects() {
    let mut comp = make_component("Logger");
    comp.effects.push(EffectBlock {
        name: "log_count".to_string(),
        body: vec![Expr::Call(CallExpr {
            target: "console".to_string(),
            method: "log".to_string(),
            args: vec![Expr::Ident("count".to_string())],
            receiver: None,
            sugar: None,
            span: Span::default(),
        })],
        cleanup: vec![],
        span: Span::default(),
    });

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("$effect("));
    assert!(result.content.contains("log_count"));
}

#[test]
fn test_methods() {
    let mut comp = make_component("Clicker");
    comp.fns.push(FnDef {
        name: "handle_click".to_string(),
        span: Span::default(),
        params: vec![],
        return_type: None,
        annotations: vec![],
        body: vec![Expr::Assign(
            "count".to_string(),
            Box::new(Expr::BinaryOp(BinaryOpExpr {
                left: Box::new(Expr::Ident("count".to_string())),
                op: BinOp::Add,
                right: Box::new(Expr::IntLit(1)),
            })),
            None,
        )],
        steps: vec![],
        layer_provided: false,
    });

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("function handle_click()"));
}

#[test]
fn test_no_reactivity_policy() {
    let mut comp = make_component("PlainComp");
    comp.blocks.push(make_named_block("state", vec![
        make_field_with_default("value", "Str", Expr::StringLit("hello".to_string())),
    ]));

    let registry = empty_registry();
    let result = gen_svelte_component(&comp, &registry);

    // Without reactivity policy, should emit plain let bindings
    assert!(result.content.contains("let value: string = \"hello\";"));
    assert!(!result.content.contains("$state"));
}

// ─── Template Section Tests ──────────────────────────────────────────────────

#[test]
fn test_template_from_raw_block() {
    let mut comp = make_component("Greeter");
    comp.raw_blocks.push(("template".to_string(), "<h1>Hello {name}</h1>".to_string()));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("<h1>Hello {name}</h1>"));
}

#[test]
fn test_empty_template_shell() {
    let comp = make_component("Empty");
    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("veil-shell"));
    assert!(result.content.contains("empty template shell"));
}

#[test]
fn test_component_auto_import() {
    let mut comp = make_component("Dashboard");
    comp.raw_blocks.push((
        "template".to_string(),
        "<div><UserCard name={user.name} /><StatWidget count={total} /></div>".to_string(),
    ));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("import UserCard from './UserCard.svelte'"));
    assert!(result.content.contains("import StatWidget from './StatWidget.svelte'"));
    // Should not import itself
    assert!(!result.content.contains("import Dashboard"));
}

// ─── Template Parser Tests ───────────────────────────────────────────────────

#[test]
fn test_parse_simple_element() {
    let nodes = parse_template_to_html_exprs("<div class=\"card\">Hello</div>");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Element { tag, attrs, children } => {
            assert_eq!(tag, "div");
            assert_eq!(attrs.len(), 1);
            assert_eq!(attrs[0].name, "class");
            assert_eq!(children.len(), 1);
        }
        _ => panic!("Expected Element, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_interpolation() {
    let nodes = parse_template_to_html_exprs("{count}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Interpolation(expr) => assert_eq!(expr, "count"),
        _ => panic!("Expected Interpolation, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_if_block() {
    let nodes = parse_template_to_html_exprs("{#if visible}<p>Shown</p>{/if}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Conditional { condition, then_children, else_children } => {
            assert_eq!(condition, "visible");
            assert!(!then_children.is_empty());
            assert!(else_children.is_none());
        }
        _ => panic!("Expected Conditional, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_if_else_block() {
    let nodes = parse_template_to_html_exprs("{#if logged_in}<p>Welcome</p>{:else}<p>Login</p>{/if}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Conditional { condition, then_children, else_children } => {
            assert_eq!(condition, "logged_in");
            assert!(!then_children.is_empty());
            assert!(else_children.is_some());
        }
        _ => panic!("Expected Conditional, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_each_block() {
    let nodes = parse_template_to_html_exprs("{#each items as item}<li>{item.name}</li>{/each}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Loop { binding, iterable, key, children } => {
            assert_eq!(binding, "item");
            assert_eq!(iterable, "items");
            assert!(key.is_none());
            assert!(!children.is_empty());
        }
        _ => panic!("Expected Loop, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_each_with_key() {
    let nodes = parse_template_to_html_exprs("{#each users as user (user.id)}<span>{user.name}</span>{/each}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Loop { binding, iterable, key, .. } => {
            assert_eq!(binding, "user");
            assert_eq!(iterable, "users");
            assert_eq!(key.as_deref(), Some("user.id"));
        }
        _ => panic!("Expected Loop, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_render_call() {
    let nodes = parse_template_to_html_exprs("{@render children()}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::RenderCall { name, args } => {
            assert_eq!(name, "children");
            assert!(args.is_empty() || (args.len() == 1 && args[0].is_empty()));
        }
        _ => panic!("Expected RenderCall, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_raw_html() {
    let nodes = parse_template_to_html_exprs("{@html content}");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::RawHtml(expr) => assert_eq!(expr, "content"),
        _ => panic!("Expected RawHtml, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_void_element() {
    let nodes = parse_template_to_html_exprs("<input type=\"text\" />");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Element { tag, attrs, children } => {
            assert_eq!(tag, "input");
            assert_eq!(attrs.len(), 1);
            assert!(children.is_empty());
        }
        _ => panic!("Expected Element, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_dynamic_attr() {
    let nodes = parse_template_to_html_exprs("<button class={cls}>Click</button>");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        HtmlExpr::Element { tag, attrs, .. } => {
            assert_eq!(tag, "button");
            assert!(attrs.iter().any(|a| a.name == "class"));
            // Dynamic value
            assert!(attrs.iter().any(|a| matches!(&a.value, HtmlAttrValue::Dynamic(e) if e == "cls")));
        }
        _ => panic!("Expected Element, got {:?}", nodes[0]),
    }
}

// ─── Style Section Tests ─────────────────────────────────────────────────────

#[test]
fn test_style_from_raw_block() {
    let mut comp = make_component("Styled");
    comp.raw_blocks.push((
        "style".to_string(),
        ".card { padding: 1rem; border: 1px solid #ccc; }".to_string(),
    ));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("<style>"));
    assert!(result.content.contains("padding: 1rem"));
    assert!(result.content.contains("</style>"));
}

#[test]
fn test_empty_style_section() {
    let comp = make_component("NoStyle");
    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    assert!(result.content.contains("<style>"));
    assert!(result.content.contains("TODO: Add component styles"));
    assert!(result.content.contains("</style>"));
}

// ─── CSS Parser Tests ────────────────────────────────────────────────────────

#[test]
fn test_parse_simple_css_rule() {
    let nodes = parse_css_to_exprs(".card { padding: 1rem; margin: 0.5rem; }");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        CssExpr::Rule { selector, declarations } => {
            assert_eq!(selector, ".card");
            assert_eq!(declarations.len(), 2);
            assert_eq!(declarations[0].property, "padding");
            assert_eq!(declarations[0].value, "1rem");
            assert_eq!(declarations[1].property, "margin");
            assert_eq!(declarations[1].value, "0.5rem");
        }
        _ => panic!("Expected Rule, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_media_query() {
    let css = "@media (min-width: 768px) { .card { padding: 2rem; } }";
    let nodes = parse_css_to_exprs(css);
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        CssExpr::MediaQuery { condition, rules } => {
            assert_eq!(condition, "(min-width: 768px)");
            assert_eq!(rules.len(), 1);
        }
        _ => panic!("Expected MediaQuery, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_import() {
    let nodes = parse_css_to_exprs("@import \"reset.css\";");
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        CssExpr::Import(path) => assert_eq!(path, "reset.css"),
        _ => panic!("Expected Import, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_multiple_rules() {
    let css = ".a { color: red; }\n.b { color: blue; }";
    let nodes = parse_css_to_exprs(css);
    assert_eq!(nodes.len(), 2);
    assert!(matches!(&nodes[0], CssExpr::Rule { selector, .. } if selector == ".a"));
    assert!(matches!(&nodes[1], CssExpr::Rule { selector, .. } if selector == ".b"));
}

#[test]
fn test_parse_keyframes() {
    let css = "@keyframes fade-in { from { opacity: 0; } to { opacity: 1; } }";
    let nodes = parse_css_to_exprs(css);
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        CssExpr::Keyframes { name, steps } => {
            assert_eq!(name, "fade-in");
            assert_eq!(steps.len(), 2);
            assert_eq!(steps[0].0, "from");
            assert_eq!(steps[1].0, "to");
        }
        _ => panic!("Expected Keyframes, got {:?}", nodes[0]),
    }
}

#[test]
fn test_parse_custom_property() {
    let css = ":root { --color-primary: #3b82f6; --spacing: 1rem; }";
    let nodes = parse_css_to_exprs(css);
    // Parsed as a Rule with declarations
    assert_eq!(nodes.len(), 1);
    match &nodes[0] {
        CssExpr::Rule { selector, declarations } => {
            assert_eq!(selector, ":root");
            assert!(declarations.iter().any(|d| d.property == "--color-primary"));
        }
        _ => panic!("Expected Rule, got {:?}", nodes[0]),
    }
}

// ─── Full Component Integration Tests ────────────────────────────────────────

#[test]
fn test_full_component_with_all_sections() {
    let mut comp = make_component("TodoItem");

    // Props
    comp.blocks.push(make_named_block("props", vec![
        make_field("text", "Str"),
        make_field("done", "Bool"),
    ]));

    // State
    comp.blocks.push(make_named_block("state", vec![
        make_field_with_default("editing", "Bool", Expr::BoolLit(false)),
    ]));

    // Template
    comp.raw_blocks.push((
        "template".to_string(),
        "<li class=\"todo\">{text}</li>".to_string(),
    ));

    // Style
    comp.raw_blocks.push((
        "style".to_string(),
        ".todo { padding: 0.5rem; }".to_string(),
    ));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    // Verify all three sections present
    assert!(result.content.contains("<script lang=\"ts\">"));
    assert!(result.content.contains("</script>"));
    assert!(result.content.contains("<li"));
    assert!(result.content.contains("<style>"));
    assert!(result.content.contains("</style>"));

    // Verify correct script content
    assert!(result.content.contains("interface Props"));
    assert!(result.content.contains("$props()"));
    assert!(result.content.contains("$state<boolean>(false)"));

    // Verify path
    assert_eq!(result.path, "src/lib/components/TodoItem.svelte");
}

#[test]
fn test_full_component_output_structure() {
    let mut comp = make_component("Simple");
    comp.blocks.push(make_named_block("props", vec![
        make_field("value", "Str"),
    ]));
    comp.raw_blocks.push(("template".to_string(), "<p>{value}</p>".to_string()));
    comp.raw_blocks.push(("style".to_string(), "p { color: red; }".to_string()));

    let registry = svelte5_registry();
    let result = gen_svelte_component(&comp, &registry);

    // Verify correct ordering: script, then template, then style
    let script_pos = result.content.find("<script").unwrap();
    let script_end = result.content.find("</script>").unwrap();
    let style_pos = result.content.find("<style>").unwrap();
    let style_end = result.content.find("</style>").unwrap();

    assert!(script_pos < script_end);
    assert!(script_end < style_pos);
    assert!(style_pos < style_end);

    // Template should be between script and style
    let template_pos = result.content.find("<p>{value}</p>").unwrap();
    assert!(script_end < template_pos);
    assert!(template_pos < style_pos);
}
