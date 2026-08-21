//! CSS emission — walks a `CssExpr` tree and produces valid CSS text.

use super::{CssDeclaration, CssExpr};

/// Emit a `CssExpr` tree as a CSS string.
///
/// `indent` controls the base indentation level (number of spaces = indent * 2).
pub fn emit_css(expr: &CssExpr, indent: usize) -> String {
    let mut buf = String::new();
    emit_node(expr, indent, &mut buf);
    buf
}

/// Emit a list of `CssExpr` nodes, joining with double newlines.
pub fn emit_css_nodes(nodes: &[CssExpr], indent: usize) -> String {
    let mut buf = String::new();
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 {
            buf.push_str("\n\n");
        }
        emit_node(node, indent, &mut buf);
    }
    buf
}

fn pad(indent: usize) -> String {
    "  ".repeat(indent)
}

fn emit_node(expr: &CssExpr, indent: usize, buf: &mut String) {
    match expr {
        CssExpr::Rule { selector, declarations } => {
            emit_rule(selector, declarations, indent, buf);
        }
        CssExpr::MediaQuery { condition, rules } => {
            emit_media_query(condition, rules, indent, buf);
        }
        CssExpr::CustomProperty { name, value } => {
            let prefix = pad(indent);
            buf.push_str(&prefix);
            buf.push_str(name);
            buf.push_str(": ");
            buf.push_str(value);
            buf.push(';');
        }
        CssExpr::Keyframes { name, steps } => {
            emit_keyframes(name, steps, indent, buf);
        }
        CssExpr::Import(path) => {
            let prefix = pad(indent);
            buf.push_str(&prefix);
            buf.push_str("@import \"");
            buf.push_str(path);
            buf.push_str("\";");
        }
        CssExpr::Nest { parent, children } => {
            emit_nest(parent, children, indent, buf);
        }
        CssExpr::Raw(text) => {
            let prefix = pad(indent);
            buf.push_str(&prefix);
            buf.push_str(text);
        }
    }
}

fn emit_rule(selector: &str, declarations: &[CssDeclaration], indent: usize, buf: &mut String) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str(selector);
    buf.push_str(" {");

    if declarations.is_empty() {
        buf.push('}');
        return;
    }

    buf.push('\n');
    for decl in declarations {
        emit_declaration(decl, indent + 1, buf);
        buf.push('\n');
    }
    buf.push_str(&prefix);
    buf.push('}');
}

fn emit_declaration(decl: &CssDeclaration, indent: usize, buf: &mut String) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str(&decl.property);
    buf.push_str(": ");
    buf.push_str(&decl.value);
    buf.push(';');
}

fn emit_media_query(condition: &str, rules: &[CssExpr], indent: usize, buf: &mut String) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str("@media ");
    buf.push_str(condition);
    buf.push_str(" {");

    if rules.is_empty() {
        buf.push('}');
        return;
    }

    buf.push('\n');
    for (i, rule) in rules.iter().enumerate() {
        if i > 0 {
            buf.push_str("\n\n");
        }
        emit_node(rule, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push('}');
}

fn emit_keyframes(
    name: &str,
    steps: &[(String, Vec<CssDeclaration>)],
    indent: usize,
    buf: &mut String,
) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str("@keyframes ");
    buf.push_str(name);
    buf.push_str(" {");

    if steps.is_empty() {
        buf.push('}');
        return;
    }

    buf.push('\n');
    for (i, (step_selector, declarations)) in steps.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        emit_rule(step_selector, declarations, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push('}');
}

fn emit_nest(parent: &str, children: &[CssExpr], indent: usize, buf: &mut String) {
    let prefix = pad(indent);
    buf.push_str(&prefix);
    buf.push_str(parent);
    buf.push_str(" {");

    if children.is_empty() {
        buf.push('}');
        return;
    }

    buf.push('\n');
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            buf.push_str("\n\n");
        }
        emit_node(child, indent + 1, buf);
    }
    buf.push('\n');
    buf.push_str(&prefix);
    buf.push('}');
}
