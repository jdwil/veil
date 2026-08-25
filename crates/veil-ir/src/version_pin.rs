//! Version pin diagnostics (LAY-011 / LAY-012).
//!
//! LAY-011: Warn when a `use` statement references a stub or library project
//! without a version pin (`use aws_sdk_dynamodb` without `@1.140.0`).
//!
//! LAY-012: Error when a version pin doesn't match the resolved stub's declared
//! version (`use aws_sdk_dynamodb@1.139.0` but stub says `1.140.0`).
//!
//! Platform layers (ddd, rust, harness, etc.) never need versions — they ship
//! with the runtime.

use crate::ast::Solution;
use crate::diagnostics::{Diagnostic, Severity};
use crate::layer::LayerRegistry;
use crate::platform_layers::is_platform_layer_name;

/// Check version pins on all `use` imports.
///
/// - Stubs without version → LAY-011 warning
/// - Library packages without version → LAY-011 warning
/// - Platform layers → no warning regardless
/// - Version mismatch (pinned ≠ resolved) → LAY-012 error
pub fn check_version_pins(sol: &Solution, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for u in &sol.uses {
        // Platform layers never need versions — skip entirely.
        if is_platform_layer_name(&u.package_name) {
            continue;
        }

        // Also skip if this resolves to a layer loaded into the registry
        // (product layers loaded from .layer files — these are part of the local
        // compilation unit and don't need version pins).
        let is_loaded_layer = registry
            .layers
            .iter()
            .any(|l| l == &u.package_name || l.replace('-', "_") == u.package_name);

        // Check if this resolves to a stub.
        let use_key = u.package_name.replace('-', "_");
        let matched_stub = registry.stubs.iter().find(|s| {
            s.name == u.package_name
                || s.name.replace('-', "_") == use_key
                || s.alias.as_deref() == Some(&u.package_name)
        });

        if let Some(stub) = matched_stub {
            // This is a stub import — version pin applies.
            match &u.version {
                None => {
                    // LAY-011: no version pin on stub import.
                    diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "'{}' imported without version pin — using latest",
                            u.package_name
                        ),
                        node_id: None,
                        node_name: Some(u.package_name.clone()),
                        code: "LAY-011".to_string(),
                        constraint: "LAY-011".to_string(),
                        parent: None,
                        hint: Some(format!(
                            "pin with `use {}@{}` to ensure reproducible builds",
                            u.package_name, stub.version
                        )),
                        span_start: Some(u.span.start),
                        span_end: Some(u.span.end),
                    });
                }
                Some(pinned) => {
                    // LAY-012: version mismatch check.
                    if !stub.version.is_empty() && pinned != &stub.version {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "stub '{}' version mismatch — requested @{} but runtime has {}",
                                u.package_name, pinned, stub.version
                            ),
                            node_id: None,
                            node_name: Some(u.package_name.clone()),
                            code: "LAY-012".to_string(),
                            constraint: "LAY-012".to_string(),
                            parent: None,
                            hint: Some(format!(
                                "update your version pin or install the correct stub version"
                            )),
                            span_start: Some(u.span.start),
                            span_end: Some(u.span.end),
                        });
                    }
                }
            }
        } else if !is_loaded_layer {
            // Not a stub, not a loaded layer, not a platform layer — likely a
            // library project import. Warn if no version pin.
            // (Only warn if the import isn't already flagged as unresolved by
            // check_names — skip entirely if we can't resolve it at all.)
            if u.version.is_none() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "'{}' imported without version pin — using latest",
                        u.package_name
                    ),
                    node_id: None,
                    node_name: Some(u.package_name.clone()),
                    code: "LAY-011".to_string(),
                    constraint: "LAY-011".to_string(),
                    parent: None,
                    hint: Some(format!(
                        "pin with `use {}@<version>` to ensure reproducible builds",
                        u.package_name
                    )),
                    span_start: Some(u.span.start),
                    span_end: Some(u.span.end),
                });
            }
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Solution, UseImport};
    use crate::layer::LayerRegistry;
    use crate::span::Span;

    fn empty_sol_with_uses(uses: Vec<UseImport>) -> Solution {
        Solution {
            name: "Test".to_string(),
            span: Span::new(0, 0),
            uses,
            links: Vec::new(),
            items: Vec::new(),
            expose: None,
            guidance: Vec::new(),
        }
    }

    fn use_import(name: &str, version: Option<&str>) -> UseImport {
        UseImport {
            package_name: name.to_string(),
            alias: None,
            version: version.map(|v| v.to_string()),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn platform_layer_no_warning() {
        let sol = empty_sol_with_uses(vec![
            use_import("ddd", None),
            use_import("rust", None),
            use_import("harness", None),
        ]);
        let registry = LayerRegistry::default();
        let diags = check_version_pins(&sol, &registry);
        // Platform layers should produce zero diagnostics.
        let lay011: Vec<_> = diags.iter().filter(|d| d.code == "LAY-011").collect();
        assert!(lay011.is_empty(), "platform layers should not warn: {:?}", lay011);
    }

    #[test]
    fn stub_without_version_warns() {
        let sol = empty_sol_with_uses(vec![use_import("aws_sdk_dynamodb", None)]);
        let mut registry = LayerRegistry::default();
        registry.stubs.push(crate::layer::StubCrate {
            name: "aws_sdk_dynamodb".to_string(),
            version: "1.140.0".to_string(),
            ..Default::default()
        });
        let diags = check_version_pins(&sol, &registry);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "LAY-011");
        assert!(diags[0].hint.as_ref().unwrap().contains("@1.140.0"));
    }

    #[test]
    fn stub_with_matching_version_no_warning() {
        let sol = empty_sol_with_uses(vec![use_import("aws_sdk_dynamodb", Some("1.140.0"))]);
        let mut registry = LayerRegistry::default();
        registry.stubs.push(crate::layer::StubCrate {
            name: "aws_sdk_dynamodb".to_string(),
            version: "1.140.0".to_string(),
            ..Default::default()
        });
        let diags = check_version_pins(&sol, &registry);
        assert!(diags.is_empty(), "matching version should not warn: {:?}", diags);
    }

    #[test]
    fn stub_version_mismatch_errors() {
        let sol = empty_sol_with_uses(vec![use_import("aws_sdk_dynamodb", Some("1.139.0"))]);
        let mut registry = LayerRegistry::default();
        registry.stubs.push(crate::layer::StubCrate {
            name: "aws_sdk_dynamodb".to_string(),
            version: "1.140.0".to_string(),
            ..Default::default()
        });
        let diags = check_version_pins(&sol, &registry);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "LAY-012");
        assert!(matches!(diags[0].severity, Severity::Error));
        assert!(diags[0].message.contains("1.139.0"));
        assert!(diags[0].message.contains("1.140.0"));
    }

    #[test]
    fn library_without_version_warns() {
        let sol = empty_sol_with_uses(vec![use_import("dlx_bus", None)]);
        let registry = LayerRegistry::default();
        let diags = check_version_pins(&sol, &registry);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "LAY-011");
    }
}
