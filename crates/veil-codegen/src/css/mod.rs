//! CSS intermediate representation for VEIL codegen.
//!
//! `CssExpr` represents CSS rules and at-rules as a typed tree.
//! The `emit_css` function walks this tree and produces valid CSS text.

pub mod emit;
#[cfg(test)]
mod tests;

/// A node in the CSS expression tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CssExpr {
    /// A CSS rule: `.card { padding: 1rem; }`
    Rule {
        selector: String,
        declarations: Vec<CssDeclaration>,
    },
    /// A media query: `@media (min-width: 768px) { ... }`
    MediaQuery {
        condition: String,
        rules: Vec<CssExpr>,
    },
    /// A custom property declaration: `--color-primary: #3b82f6;`
    CustomProperty {
        name: String,
        value: String,
    },
    /// A keyframes definition: `@keyframes fade-in { from { ... } to { ... } }`
    Keyframes {
        name: String,
        steps: Vec<(String, Vec<CssDeclaration>)>,
    },
    /// An @import rule: `@import "reset.css";`
    Import(String),
    /// CSS nesting: parent selector wrapping child rules.
    Nest {
        parent: String,
        children: Vec<CssExpr>,
    },
    /// Raw CSS text (escape hatch for unsupported features).
    Raw(String),
}

/// A single CSS declaration: `property: value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssDeclaration {
    pub property: String,
    pub value: String,
}
