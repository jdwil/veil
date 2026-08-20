//! Unit tests for CSS emission.

use super::emit::{emit_css, emit_css_nodes};
use super::{CssDeclaration, CssExpr};

#[test]
fn simple_rule() {
    let expr = CssExpr::Rule {
        selector: ".card".into(),
        declarations: vec![CssDeclaration {
            property: "padding".into(),
            value: "1rem".into(),
        }],
    };
    let expected = "\
.card {
  padding: 1rem;
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn rule_multiple_declarations() {
    let expr = CssExpr::Rule {
        selector: ".button".into(),
        declarations: vec![
            CssDeclaration {
                property: "background".into(),
                value: "#3b82f6".into(),
            },
            CssDeclaration {
                property: "color".into(),
                value: "white".into(),
            },
            CssDeclaration {
                property: "border-radius".into(),
                value: "0.5rem".into(),
            },
        ],
    };
    let expected = "\
.button {
  background: #3b82f6;
  color: white;
  border-radius: 0.5rem;
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn rule_no_declarations() {
    let expr = CssExpr::Rule {
        selector: ".empty".into(),
        declarations: vec![],
    };
    assert_eq!(emit_css(&expr, 0), ".empty {}");
}

#[test]
fn media_query() {
    let expr = CssExpr::MediaQuery {
        condition: "(min-width: 768px)".into(),
        rules: vec![CssExpr::Rule {
            selector: ".card".into(),
            declarations: vec![CssDeclaration {
                property: "display".into(),
                value: "grid".into(),
            }],
        }],
    };
    let expected = "\
@media (min-width: 768px) {
  .card {
    display: grid;
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn media_query_multiple_rules() {
    let expr = CssExpr::MediaQuery {
        condition: "(max-width: 480px)".into(),
        rules: vec![
            CssExpr::Rule {
                selector: ".nav".into(),
                declarations: vec![CssDeclaration {
                    property: "display".into(),
                    value: "none".into(),
                }],
            },
            CssExpr::Rule {
                selector: ".menu".into(),
                declarations: vec![CssDeclaration {
                    property: "display".into(),
                    value: "block".into(),
                }],
            },
        ],
    };
    let expected = "\
@media (max-width: 480px) {
  .nav {
    display: none;
  }

  .menu {
    display: block;
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn media_query_empty() {
    let expr = CssExpr::MediaQuery {
        condition: "(prefers-color-scheme: dark)".into(),
        rules: vec![],
    };
    assert_eq!(emit_css(&expr, 0), "@media (prefers-color-scheme: dark) {}");
}

#[test]
fn custom_property() {
    let expr = CssExpr::CustomProperty {
        name: "--color-primary".into(),
        value: "#3b82f6".into(),
    };
    assert_eq!(emit_css(&expr, 0), "--color-primary: #3b82f6;");
}

#[test]
fn custom_property_indented() {
    let expr = CssExpr::CustomProperty {
        name: "--gap".into(),
        value: "1rem".into(),
    };
    assert_eq!(emit_css(&expr, 1), "  --gap: 1rem;");
}

#[test]
fn keyframes() {
    let expr = CssExpr::Keyframes {
        name: "fade-in".into(),
        steps: vec![
            (
                "from".into(),
                vec![CssDeclaration {
                    property: "opacity".into(),
                    value: "0".into(),
                }],
            ),
            (
                "to".into(),
                vec![CssDeclaration {
                    property: "opacity".into(),
                    value: "1".into(),
                }],
            ),
        ],
    };
    let expected = "\
@keyframes fade-in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn keyframes_percentage_steps() {
    let expr = CssExpr::Keyframes {
        name: "slide".into(),
        steps: vec![
            (
                "0%".into(),
                vec![CssDeclaration {
                    property: "transform".into(),
                    value: "translateX(-100%)".into(),
                }],
            ),
            (
                "50%".into(),
                vec![CssDeclaration {
                    property: "transform".into(),
                    value: "translateX(0)".into(),
                }],
            ),
            (
                "100%".into(),
                vec![CssDeclaration {
                    property: "transform".into(),
                    value: "translateX(100%)".into(),
                }],
            ),
        ],
    };
    let expected = "\
@keyframes slide {
  0% {
    transform: translateX(-100%);
  }
  50% {
    transform: translateX(0);
  }
  100% {
    transform: translateX(100%);
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn keyframes_empty() {
    let expr = CssExpr::Keyframes {
        name: "noop".into(),
        steps: vec![],
    };
    assert_eq!(emit_css(&expr, 0), "@keyframes noop {}");
}

#[test]
fn import() {
    let expr = CssExpr::Import("reset.css".into());
    assert_eq!(emit_css(&expr, 0), "@import \"reset.css\";");
}

#[test]
fn import_url() {
    let expr = CssExpr::Import("https://fonts.googleapis.com/css?family=Roboto".into());
    assert_eq!(
        emit_css(&expr, 0),
        "@import \"https://fonts.googleapis.com/css?family=Roboto\";"
    );
}

#[test]
fn nest_simple() {
    let expr = CssExpr::Nest {
        parent: ".card".into(),
        children: vec![CssExpr::Rule {
            selector: "&:hover".into(),
            declarations: vec![CssDeclaration {
                property: "background".into(),
                value: "#f0f0f0".into(),
            }],
        }],
    };
    let expected = "\
.card {
  &:hover {
    background: #f0f0f0;
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn nest_multiple_children() {
    let expr = CssExpr::Nest {
        parent: ".nav".into(),
        children: vec![
            CssExpr::Rule {
                selector: "& a".into(),
                declarations: vec![CssDeclaration {
                    property: "color".into(),
                    value: "blue".into(),
                }],
            },
            CssExpr::Rule {
                selector: "& a:hover".into(),
                declarations: vec![CssDeclaration {
                    property: "color".into(),
                    value: "darkblue".into(),
                }],
            },
        ],
    };
    let expected = "\
.nav {
  & a {
    color: blue;
  }

  & a:hover {
    color: darkblue;
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn nest_empty() {
    let expr = CssExpr::Nest {
        parent: ".x".into(),
        children: vec![],
    };
    assert_eq!(emit_css(&expr, 0), ".x {}");
}

#[test]
fn raw() {
    let expr = CssExpr::Raw("/* fallback for unsupported features */".into());
    assert_eq!(emit_css(&expr, 0), "/* fallback for unsupported features */");
}

#[test]
fn raw_indented() {
    let expr = CssExpr::Raw("content: '';".into());
    assert_eq!(emit_css(&expr, 2), "    content: '';");
}

#[test]
fn emit_multiple_nodes() {
    let nodes = vec![
        CssExpr::Import("reset.css".into()),
        CssExpr::Rule {
            selector: "body".into(),
            declarations: vec![
                CssDeclaration {
                    property: "margin".into(),
                    value: "0".into(),
                },
                CssDeclaration {
                    property: "font-family".into(),
                    value: "sans-serif".into(),
                },
            ],
        },
        CssExpr::Rule {
            selector: ".container".into(),
            declarations: vec![CssDeclaration {
                property: "max-width".into(),
                value: "1200px".into(),
            }],
        },
    ];
    let expected = "\
@import \"reset.css\";

body {
  margin: 0;
  font-family: sans-serif;
}

.container {
  max-width: 1200px;
}";
    assert_eq!(emit_css_nodes(&nodes, 0), expected);
}

#[test]
fn indented_rule() {
    let expr = CssExpr::Rule {
        selector: "p".into(),
        declarations: vec![CssDeclaration {
            property: "color".into(),
            value: "red".into(),
        }],
    };
    // indent=1 means 2 spaces prefix on selector and closing brace,
    // 4 spaces on declarations.
    let expected = "  p {\n    color: red;\n  }";
    assert_eq!(emit_css(&expr, 1), expected);
}

#[test]
fn complex_selector() {
    let expr = CssExpr::Rule {
        selector: "div.card > h2:first-child".into(),
        declarations: vec![CssDeclaration {
            property: "margin-top".into(),
            value: "0".into(),
        }],
    };
    let expected = "\
div.card > h2:first-child {
  margin-top: 0;
}";
    assert_eq!(emit_css(&expr, 0), expected);
}

#[test]
fn nested_media_in_nest() {
    // CSS nesting with a media query inside
    let expr = CssExpr::Nest {
        parent: ".layout".into(),
        children: vec![
            CssExpr::Rule {
                selector: "& .sidebar".into(),
                declarations: vec![CssDeclaration {
                    property: "width".into(),
                    value: "200px".into(),
                }],
            },
            CssExpr::MediaQuery {
                condition: "(max-width: 768px)".into(),
                rules: vec![CssExpr::Rule {
                    selector: "& .sidebar".into(),
                    declarations: vec![CssDeclaration {
                        property: "width".into(),
                        value: "100%".into(),
                    }],
                }],
            },
        ],
    };
    let expected = "\
.layout {
  & .sidebar {
    width: 200px;
  }

  @media (max-width: 768px) {
    & .sidebar {
      width: 100%;
    }
  }
}";
    assert_eq!(emit_css(&expr, 0), expected);
}
