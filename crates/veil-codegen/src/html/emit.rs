//! HTML5 emission — walks an `HtmlExpr` tree and produces valid HTML text.

use super::{HtmlAttr, HtmlAttrValue, HtmlExpr, is_void_element};

/// Emit an `HtmlExpr` tree as an HTML5 string.
///
/// `indent` controls the base indentation level (number of spaces = indent * 2).
/// Each nesting level adds 2 more spaces.
pub fn emit_html(expr: &HtmlExpr, indent: usize) -> String {
    let mut buf = String::new();
    emit_node(expr, indent, &mut buf);
    buf
}

/// Emit a list of `HtmlExpr` nodes, joining with newlines.
pub fn emit_html_nodes(nodes: &[HtmlExpr], indent: usize) -> String {
    let mut buf = String::new();
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(node, indent, &mut buf);
    }
    buf
}

fn pad(indent: usize) -> String {
    "  ".repeat(indent)
}

fn emit_node(expr: &HtmlExpr, indent: usize, buf: &mut String) {
    match expr {
        HtmlExpr::Element { tag, attrs, children } => {
            emit_element(tag, attrs, children, indent, buf);
        }
        HtmlExpr::Text(text) => {
            buf.push_str(&pad(indent));
            buf.push_str(&html_escape(text));
        }
        HtmlExpr::Interpolation(expr_str) => {
            buf.push_str(&pad(indent));
            buf.push('{');
            buf.push_str(expr_str);
            buf.push('}');
        }
        HtmlExpr::Conditional { condition, then_children, else_children } => {
            emit_conditional(condition, then_children, else_children.as_deref(), indent, buf);
        }
        HtmlExpr::Loop { binding, iterable, key, children } => {
            emit_loop(binding, iterable, key.as_deref(), children, indent, buf);
        }
        HtmlExpr::Component { name, props, children } => {
            emit_component(name, props, children, indent, buf);
        }
        HtmlExpr::Slot { name, fallback } => {
            emit_slot(name.as_deref(), fallback, indent, buf);
        }
        HtmlExpr::Snippet { name, params, children } => {
            emit_snippet(name, params, children, indent, buf);
        }
        HtmlExpr::RenderCall { name, args } => {
            buf.push_str(&pad(indent));
            buf.push_str("{@render ");
            buf.push_str(name);
            buf.push('(');
            buf.push_str(&args.join(", "));
            buf.push_str(")}");
        }
        HtmlExpr::RawHtml(expr_str) => {
            buf.push_str(&pad(indent));
            buf.push_str("{@html ");
            buf.push_str(expr_str);
            buf.push('}');
        }
        HtmlExpr::Comment(text) => {
            buf.push_str(&pad(indent));
            buf.push_str("<!-- ");
            buf.push_str(text);
            buf.push_str(" -->");
        }
    }
}

fn emit_element(
    tag: &str,
    attrs: &[HtmlAttr],
    children: &[HtmlExpr],
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push('<');
    buf.push_str(tag);

    for attr in attrs {
        buf.push(' ');
        emit_attr(attr, buf);
    }

    if is_void_element(tag) {
        buf.push_str(" />");
        return;
    }

    buf.push('>');

    if children.is_empty() {
        buf.push_str("</");
        buf.push_str(tag);
        buf.push('>');
        return;
    }

    // Inline a single text or interpolation child (no newlines).
    if children.len() == 1 && is_inline_child(&children[0]) {
        emit_inline_child(&children[0], buf);
        buf.push_str("</");
        buf.push_str(tag);
        buf.push('>');
        return;
    }

    // Multi-child: indent each on its own line.
    buf.push('\n');
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(child, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push_str("</");
    buf.push_str(tag);
    buf.push('>');
}

fn is_inline_child(expr: &HtmlExpr) -> bool {
    matches!(expr, HtmlExpr::Text(_) | HtmlExpr::Interpolation(_))
}

fn emit_inline_child(expr: &HtmlExpr, buf: &mut String) {
    match expr {
        HtmlExpr::Text(text) => buf.push_str(&html_escape(text)),
        HtmlExpr::Interpolation(e) => {
            buf.push('{');
            buf.push_str(e);
            buf.push('}');
        }
        _ => {}
    }
}

fn emit_attr(attr: &HtmlAttr, buf: &mut String) {
    match &attr.value {
        HtmlAttrValue::Static(val) => {
            buf.push_str(&attr.name);
            buf.push_str("=\"");
            buf.push_str(&attr_escape(val));
            buf.push('"');
        }
        HtmlAttrValue::Dynamic(expr) => {
            buf.push_str(&attr.name);
            buf.push_str("={");
            buf.push_str(expr);
            buf.push('}');
        }
        HtmlAttrValue::Bind(prop) => {
            buf.push_str("bind:");
            buf.push_str(prop);
        }
        HtmlAttrValue::Event(event, handler) => {
            buf.push_str("on:");
            buf.push_str(event);
            buf.push_str("={");
            buf.push_str(handler);
            buf.push('}');
        }
        HtmlAttrValue::Spread(expr) => {
            buf.push_str("{...");
            buf.push_str(expr);
            buf.push('}');
        }
        HtmlAttrValue::Boolean => {
            buf.push_str(&attr.name);
        }
    }
}

fn emit_conditional(
    condition: &str,
    then_children: &[HtmlExpr],
    else_children: Option<&[HtmlExpr]>,
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str("{#if ");
    buf.push_str(condition);
    buf.push('}');
    buf.push('\n');
    for (i, child) in then_children.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(child, indent + 1, buf);
    }
    if let Some(else_nodes) = else_children {
        buf.push('\n');
        buf.push_str(&prefix);
        buf.push_str("{:else}");
        buf.push('\n');
        for (i, child) in else_nodes.iter().enumerate() {
            if i > 0 {
                buf.push('\n');
            }
            emit_node(child, indent + 1, buf);
        }
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push_str("{/if}");
}

fn emit_loop(
    binding: &str,
    iterable: &str,
    key: Option<&str>,
    children: &[HtmlExpr],
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str("{#each ");
    buf.push_str(iterable);
    buf.push_str(" as ");
    buf.push_str(binding);
    if let Some(k) = key {
        buf.push_str(" (");
        buf.push_str(k);
        buf.push(')');
    }
    buf.push('}');
    buf.push('\n');
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(child, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push_str("{/each}");
}

fn emit_component(
    name: &str,
    props: &[(String, String)],
    children: &[HtmlExpr],
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push('<');
    buf.push_str(name);
    for (prop_name, prop_val) in props {
        buf.push(' ');
        buf.push_str(prop_name);
        buf.push_str("={");
        buf.push_str(prop_val);
        buf.push('}');
    }

    if children.is_empty() {
        buf.push_str(" />");
        return;
    }

    buf.push('>');
    buf.push('\n');
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(child, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push_str("</");
    buf.push_str(name);
    buf.push('>');
}

fn emit_slot(
    name: Option<&str>,
    fallback: &[HtmlExpr],
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str("<slot");
    if let Some(n) = name {
        buf.push_str(" name=\"");
        buf.push_str(n);
        buf.push('"');
    }

    if fallback.is_empty() {
        buf.push_str(" />");
        return;
    }

    buf.push('>');
    buf.push('\n');
    for (i, child) in fallback.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(child, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push_str("</slot>");
}

fn emit_snippet(
    name: &str,
    params: &[String],
    children: &[HtmlExpr],
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str("{#snippet ");
    buf.push_str(name);
    buf.push('(');
    buf.push_str(&params.join(", "));
    buf.push_str(")}");
    buf.push('\n');
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_node(child, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push_str("{/snippet}");
}

/// Escape text content for HTML (ampersand, angle brackets).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape attribute values (ampersand, quotes).
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
