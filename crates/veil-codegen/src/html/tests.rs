//! Unit tests for HTML emission.

use super::emit::{emit_html, emit_html_nodes};
use super::{HtmlAttr, HtmlAttrValue, HtmlExpr};

#[test]
fn text_node() {
    let expr = HtmlExpr::Text("Hello, world!".into());
    assert_eq!(emit_html(&expr, 0), "Hello, world!");
}

#[test]
fn text_node_escapes_html() {
    let expr = HtmlExpr::Text("<script>alert('xss')</script>".into());
    assert_eq!(
        emit_html(&expr, 0),
        "&lt;script&gt;alert('xss')&lt;/script&gt;"
    );
}

#[test]
fn text_node_with_indent() {
    let expr = HtmlExpr::Text("indented".into());
    assert_eq!(emit_html(&expr, 2), "    indented");
}

#[test]
fn simple_element_no_children() {
    let expr = HtmlExpr::Element {
        tag: "div".into(),
        attrs: vec![],
        children: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<div></div>");
}

#[test]
fn element_with_static_attr() {
    let expr = HtmlExpr::Element {
        tag: "div".into(),
        attrs: vec![HtmlAttr {
            name: "class".into(),
            value: HtmlAttrValue::Static("card".into()),
        }],
        children: vec![HtmlExpr::Text("Hello".into())],
    };
    assert_eq!(emit_html(&expr, 0), "<div class=\"card\">Hello</div>");
}

#[test]
fn element_with_dynamic_attr() {
    let expr = HtmlExpr::Element {
        tag: "span".into(),
        attrs: vec![HtmlAttr {
            name: "class".into(),
            value: HtmlAttrValue::Dynamic("activeClass".into()),
        }],
        children: vec![HtmlExpr::Text("hi".into())],
    };
    assert_eq!(emit_html(&expr, 0), "<span class={activeClass}>hi</span>");
}

#[test]
fn element_with_boolean_attr() {
    let expr = HtmlExpr::Element {
        tag: "input".into(),
        attrs: vec![
            HtmlAttr {
                name: "type".into(),
                value: HtmlAttrValue::Static("text".into()),
            },
            HtmlAttr {
                name: "disabled".into(),
                value: HtmlAttrValue::Boolean,
            },
        ],
        children: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<input type=\"text\" disabled />");
}

#[test]
fn void_element_self_closes() {
    let expr = HtmlExpr::Element {
        tag: "br".into(),
        attrs: vec![],
        children: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<br />");
}

#[test]
fn void_element_img() {
    let expr = HtmlExpr::Element {
        tag: "img".into(),
        attrs: vec![
            HtmlAttr {
                name: "src".into(),
                value: HtmlAttrValue::Static("/logo.png".into()),
            },
            HtmlAttr {
                name: "alt".into(),
                value: HtmlAttrValue::Static("Logo".into()),
            },
        ],
        children: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<img src=\"/logo.png\" alt=\"Logo\" />");
}

#[test]
fn element_with_multiple_children_indented() {
    let expr = HtmlExpr::Element {
        tag: "ul".into(),
        attrs: vec![],
        children: vec![
            HtmlExpr::Element {
                tag: "li".into(),
                attrs: vec![],
                children: vec![HtmlExpr::Text("One".into())],
            },
            HtmlExpr::Element {
                tag: "li".into(),
                attrs: vec![],
                children: vec![HtmlExpr::Text("Two".into())],
            },
        ],
    };
    let expected = "\
<ul>
  <li>One</li>
  <li>Two</li>
</ul>";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn nested_elements() {
    let expr = HtmlExpr::Element {
        tag: "div".into(),
        attrs: vec![HtmlAttr {
            name: "class".into(),
            value: HtmlAttrValue::Static("wrapper".into()),
        }],
        children: vec![HtmlExpr::Element {
            tag: "p".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Text("Nested".into())],
        }],
    };
    let expected = "\
<div class=\"wrapper\">
  <p>Nested</p>
</div>";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn interpolation() {
    let expr = HtmlExpr::Interpolation("user.name".into());
    assert_eq!(emit_html(&expr, 0), "{user.name}");
}

#[test]
fn element_with_interpolation_child() {
    let expr = HtmlExpr::Element {
        tag: "span".into(),
        attrs: vec![],
        children: vec![HtmlExpr::Interpolation("count".into())],
    };
    assert_eq!(emit_html(&expr, 0), "<span>{count}</span>");
}

#[test]
fn conditional_without_else() {
    let expr = HtmlExpr::Conditional {
        condition: "isVisible".into(),
        then_children: vec![HtmlExpr::Element {
            tag: "p".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Text("Visible!".into())],
        }],
        else_children: None,
    };
    let expected = "\
{#if isVisible}
  <p>Visible!</p>
{/if}";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn conditional_with_else() {
    let expr = HtmlExpr::Conditional {
        condition: "loggedIn".into(),
        then_children: vec![HtmlExpr::Text("Welcome".into())],
        else_children: Some(vec![HtmlExpr::Text("Please log in".into())]),
    };
    let expected = "\
{#if loggedIn}
  Welcome
{:else}
  Please log in
{/if}";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn loop_without_key() {
    let expr = HtmlExpr::Loop {
        binding: "item".into(),
        iterable: "items".into(),
        key: None,
        children: vec![HtmlExpr::Element {
            tag: "li".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Interpolation("item.name".into())],
        }],
    };
    let expected = "\
{#each items as item}
  <li>{item.name}</li>
{/each}";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn loop_with_key() {
    let expr = HtmlExpr::Loop {
        binding: "user".into(),
        iterable: "users".into(),
        key: Some("user.id".into()),
        children: vec![HtmlExpr::Element {
            tag: "span".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Interpolation("user.email".into())],
        }],
    };
    let expected = "\
{#each users as user (user.id)}
  <span>{user.email}</span>
{/each}";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn component_no_children() {
    let expr = HtmlExpr::Component {
        name: "Button".into(),
        props: vec![
            ("label".into(), "\"Click me\"".into()),
            ("onClick".into(), "handleClick".into()),
        ],
        children: vec![],
    };
    assert_eq!(
        emit_html(&expr, 0),
        "<Button label={\"Click me\"} onClick={handleClick} />"
    );
}

#[test]
fn component_with_children() {
    let expr = HtmlExpr::Component {
        name: "Card".into(),
        props: vec![("title".into(), "\"My Card\"".into())],
        children: vec![HtmlExpr::Text("Card body".into())],
    };
    let expected = "\
<Card title={\"My Card\"}>
  Card body
</Card>";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn slot_default_no_fallback() {
    let expr = HtmlExpr::Slot {
        name: None,
        fallback: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<slot />");
}

#[test]
fn slot_named_with_fallback() {
    let expr = HtmlExpr::Slot {
        name: Some("header".into()),
        fallback: vec![HtmlExpr::Text("Default header".into())],
    };
    let expected = "\
<slot name=\"header\">
  Default header
</slot>";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn snippet() {
    let expr = HtmlExpr::Snippet {
        name: "row".into(),
        params: vec!["item".into(), "index".into()],
        children: vec![HtmlExpr::Element {
            tag: "tr".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Interpolation("item.name".into())],
        }],
    };
    let expected = "\
{#snippet row(item, index)}
  <tr>{item.name}</tr>
{/snippet}";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn render_call() {
    let expr = HtmlExpr::RenderCall {
        name: "row".into(),
        args: vec!["item".into(), "i".into()],
    };
    assert_eq!(emit_html(&expr, 0), "{@render row(item, i)}");
}

#[test]
fn render_call_no_args() {
    let expr = HtmlExpr::RenderCall {
        name: "header".into(),
        args: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "{@render header()}");
}

#[test]
fn raw_html() {
    let expr = HtmlExpr::RawHtml("content".into());
    assert_eq!(emit_html(&expr, 0), "{@html content}");
}

#[test]
fn comment() {
    let expr = HtmlExpr::Comment("TODO: fix this".into());
    assert_eq!(emit_html(&expr, 0), "<!-- TODO: fix this -->");
}

#[test]
fn bind_attr() {
    let expr = HtmlExpr::Element {
        tag: "input".into(),
        attrs: vec![HtmlAttr {
            name: "value".into(),
            value: HtmlAttrValue::Bind("inputValue".into()),
        }],
        children: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<input bind:inputValue />");
}

#[test]
fn event_attr() {
    let expr = HtmlExpr::Element {
        tag: "button".into(),
        attrs: vec![HtmlAttr {
            name: "click".into(),
            value: HtmlAttrValue::Event("click".into(), "handleClick".into()),
        }],
        children: vec![HtmlExpr::Text("Go".into())],
    };
    assert_eq!(
        emit_html(&expr, 0),
        "<button on:click={handleClick}>Go</button>"
    );
}

#[test]
fn spread_attr() {
    let expr = HtmlExpr::Element {
        tag: "div".into(),
        attrs: vec![HtmlAttr {
            name: "".into(),
            value: HtmlAttrValue::Spread("restProps".into()),
        }],
        children: vec![],
    };
    assert_eq!(emit_html(&expr, 0), "<div {...restProps}></div>");
}

#[test]
fn attr_escapes_quotes() {
    let expr = HtmlExpr::Element {
        tag: "div".into(),
        attrs: vec![HtmlAttr {
            name: "title".into(),
            value: HtmlAttrValue::Static("He said \"hello\"".into()),
        }],
        children: vec![],
    };
    assert_eq!(
        emit_html(&expr, 0),
        "<div title=\"He said &quot;hello&quot;\"></div>"
    );
}

#[test]
fn emit_multiple_nodes() {
    let nodes = vec![
        HtmlExpr::Comment("Header".into()),
        HtmlExpr::Element {
            tag: "h1".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Text("Title".into())],
        },
        HtmlExpr::Element {
            tag: "p".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Text("Body".into())],
        },
    ];
    let expected = "\
<!-- Header -->
<h1>Title</h1>
<p>Body</p>";
    assert_eq!(emit_html_nodes(&nodes, 0), expected);
}

#[test]
fn deeply_nested_indentation() {
    let expr = HtmlExpr::Element {
        tag: "div".into(),
        attrs: vec![],
        children: vec![HtmlExpr::Element {
            tag: "section".into(),
            attrs: vec![],
            children: vec![HtmlExpr::Element {
                tag: "p".into(),
                attrs: vec![],
                children: vec![HtmlExpr::Text("Deep".into())],
            }],
        }],
    };
    let expected = "\
<div>
  <section>
    <p>Deep</p>
  </section>
</div>";
    assert_eq!(emit_html(&expr, 0), expected);
}

#[test]
fn indented_at_base_level() {
    let expr = HtmlExpr::Element {
        tag: "p".into(),
        attrs: vec![],
        children: vec![HtmlExpr::Text("hi".into())],
    };
    assert_eq!(emit_html(&expr, 1), "  <p>hi</p>");
}
