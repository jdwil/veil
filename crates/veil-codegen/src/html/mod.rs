//! HTML5 intermediate representation for VEIL codegen.
//!
//! `HtmlExpr` represents the structure of an HTML template as a typed tree.
//! The `emit_html` function walks this tree and produces valid HTML5 text.
//!
//! Expression slots (interpolations, conditions, loop iterables, event handlers)
//! use `String` to hold the expression text. When the full TS lowering pipeline
//! is wired, these will reference `TsExpr` nodes directly.

pub mod emit;
#[cfg(test)]
mod tests;

/// A node in the HTML expression tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlExpr {
    /// An HTML element: `<tag attrs...>children...</tag>`
    Element {
        tag: String,
        attrs: Vec<HtmlAttr>,
        children: Vec<HtmlExpr>,
    },
    /// Raw text content (HTML-escaped on emit).
    Text(String),
    /// An interpolated expression: `{expression}` in template.
    Interpolation(String),
    /// Conditional rendering: `{#if cond}...{:else}...{/if}`
    Conditional {
        condition: String,
        then_children: Vec<HtmlExpr>,
        else_children: Option<Vec<HtmlExpr>>,
    },
    /// Loop rendering: `{#each iterable as binding (key)}...{/each}`
    Loop {
        binding: String,
        iterable: String,
        key: Option<String>,
        children: Vec<HtmlExpr>,
    },
    /// A component reference: `<Component prop={val}>children</Component>`
    Component {
        name: String,
        props: Vec<(String, String)>,
        children: Vec<HtmlExpr>,
    },
    /// A slot: `<slot name="x">fallback</slot>`
    Slot {
        name: Option<String>,
        fallback: Vec<HtmlExpr>,
    },
    /// A snippet definition: `{#snippet name(params)}...{/snippet}`
    Snippet {
        name: String,
        params: Vec<String>,
        children: Vec<HtmlExpr>,
    },
    /// A render call: `{@render name(args)}`
    RenderCall {
        name: String,
        args: Vec<String>,
    },
    /// Raw HTML injection: `{@html expr}`
    RawHtml(String),
    /// An HTML comment: `<!-- text -->`
    Comment(String),
}

/// An attribute on an HTML element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlAttr {
    pub name: String,
    pub value: HtmlAttrValue,
}

/// The value of an HTML attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlAttrValue {
    /// A static string value: `class="card"`
    Static(String),
    /// A dynamic expression value: `class={expr}`
    Dynamic(String),
    /// A two-way binding: `bind:value`
    Bind(String),
    /// An event handler: `on:click={handler}`
    Event(String, String),
    /// A spread: `{...props}`
    Spread(String),
    /// A boolean attribute (no value): `disabled`
    Boolean,
}

/// HTML5 void elements that must not have closing tags.
pub const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
    "param", "source", "track", "wbr",
];

/// Returns true if the tag is a void element (self-closing, no end tag).
pub fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag.to_lowercase().as_str())
}
