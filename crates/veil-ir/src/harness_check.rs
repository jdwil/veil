//! Typecheck declared `deps` / `compose` / `endpoint` constructs.
//!
//! INV-001: match construct **roles**, never DDD keywords. Protocol tokens
//! (HTTP verbs, bind sources) are allowed engine knowledge.

use crate::ast::{Construct, Field, Solution, TopLevelItem, TypeExpr};
use crate::diagnostics::{Diagnostic, Severity};
use crate::harness::{is_http_verb, CompatMode, EmitBin};
use crate::layer::{LayerRegistry, Shape};

/// Check authored harness constructs. Unknown handler / bad path / missing
/// wire are errors when the author wrote the construct (even in compat=auto).
pub fn check_harness(sol: &Solution, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let all = collect_all_constructs(sol);
    let endpoints: Vec<&Construct> = all
        .iter()
        .copied()
        .filter(|c| registry.construct_has_role(c, "http_endpoint"))
        .collect();
    let deps_bundles: Vec<&Construct> = all
        .iter()
        .copied()
        .filter(|c| registry.construct_has_role(c, "deps_bundle"))
        .collect();
    let composes: Vec<&Construct> = all
        .iter()
        .copied()
        .filter(|c| registry.construct_has_role(c, "compose"))
        .collect();

    if deps_bundles.len() > 1 {
        diags.push(Diagnostic::error(
            "harness_multiple_deps",
            format!(
                "v1 allows one deps bundle per package; found {}",
                deps_bundles
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            deps_bundles.first().map(|c| c.name.clone()),
        ));
    }

    let profile = registry
        .harness_policy
        .profile
        .as_deref()
        .unwrap_or("axum_http");
    if !is_known_profile(profile) {
        diags.push(Diagnostic::error(
            "harness_profile_unknown",
            format!("unknown harness profile '{profile}'"),
            None,
        ));
    }

    for ep in &endpoints {
        check_endpoint(ep, &all, registry, &mut diags);
    }
    for compose in &composes {
        check_compose(compose, &deps_bundles, &all, registry, &mut diags);
    }

    let mut seen_routes: Vec<(String, String, String)> = Vec::new();
    let prefix = registry.harness_policy.path_prefix.clone();
    for ep in &endpoints {
        let Some((method, path)) = endpoint_method_path(ep) else {
            continue;
        };
        let path = apply_prefix(&path, prefix.as_deref());
        if let Some((om, op, other)) = seen_routes
            .iter()
            .find(|(m, p, _)| m == &method && p == &path)
        {
            let _ = (om, op);
            diags.push(Diagnostic::error(
                "harness_duplicate_route",
                format!(
                    "endpoints '{}' and '{}' share {method} {path}",
                    ep.name, other
                ),
                Some(ep.name.clone()),
            ));
        } else {
            seen_routes.push((method, path, ep.name.clone()));
        }
    }

    let emit = registry.harness_policy.emit_bin.unwrap_or(EmitBin::OnEntry);
    let compat = registry.harness_policy.compat.unwrap_or(CompatMode::Auto);
    for c in &all {
        if registry.construct_has_http_route(c) {
            let d = match compat {
                CompatMode::Off => Diagnostic::error(
                    "harness_route_annotation_removed",
                    format!(
                        "'{}' still has a role:http_route annotation; declare `endpoint` instead (API @route was removed)",
                        c.name
                    ),
                    Some(c.name.clone()),
                ),
                CompatMode::Auto => Diagnostic::warning(
                    "harness_route_annotation_removed",
                    format!(
                        "'{}' has a leftover role:http_route annotation; prefer `endpoint` (compat still synthesizes)",
                        c.name
                    ),
                    Some(c.name.clone()),
                ),
            };
            diags.push(d);
        }
    }
    if registry.codegen_http_from_toml {
        diags.push(Diagnostic::warning(
            "harness_http_codegen_unused",
            "[codegen] http_* does not drive codegen after the flip; declare `endpoint` or set [harness] compat = \"auto\"",
            None,
        ));
    }
    if emit == EmitBin::OnEntry && !endpoints.is_empty() && composes.is_empty() {
        let d = match compat {
            CompatMode::Off => Diagnostic::error(
                "harness_emit_bin_without_compose",
                "endpoints are declared but there is no compose root",
                endpoints.first().map(|c| c.name.clone()),
            ),
            CompatMode::Auto => Diagnostic::warning(
                "harness_emit_bin_without_compose",
                "endpoints are declared but there is no compose root (compat will synthesize)",
                endpoints.first().map(|c| c.name.clone()),
            ),
        };
        diags.push(d);
    }

    diags
}

fn is_known_profile(p: &str) -> bool {
    matches!(p, "axum_http" | "axum_rpc" | "product_host")
}

fn apply_prefix(path: &str, prefix: Option<&str>) -> String {
    let Some(pre) = prefix.filter(|s| !s.is_empty()) else {
        return path.to_string();
    };
    if path.starts_with(pre) {
        path.to_string()
    } else {
        format!("{}{}", pre.trim_end_matches('/'), path)
    }
}

fn check_endpoint(
    ep: &Construct,
    all: &[&Construct],
    registry: &LayerRegistry,
    diags: &mut Vec<Diagnostic>,
) {
    let methods: Vec<&Field> = ep.fields.iter().filter(|f| f.name == "method").collect();
    let paths: Vec<&Field> = ep.fields.iter().filter(|f| f.name == "path").collect();
    let handles: Vec<&Field> = ep.fields.iter().filter(|f| f.name == "handle").collect();
    if methods.len() > 1 || paths.len() > 1 || handles.len() > 1 {
        diags.push(Diagnostic::error(
            "harness_endpoint_dup_spec",
            format!(
                "endpoint '{}' has both a compact header and method/path/handle fields",
                ep.name
            ),
            Some(ep.name.clone()),
        ));
    }

    match methods.first().and_then(|f| type_ident(&f.type_expr)) {
        None => diags.push(Diagnostic::error(
            "harness_endpoint_bad_method",
            format!("endpoint '{}' is missing method", ep.name),
            Some(ep.name.clone()),
        )),
        Some(m) if !is_http_verb(m) => diags.push(Diagnostic::error(
            "harness_endpoint_bad_method",
            format!("endpoint '{}' has invalid HTTP method '{m}'", ep.name),
            Some(ep.name.clone()),
        )),
        Some(_) => {}
    }

    let path = paths.first().and_then(|f| type_ident(&f.type_expr));
    match path {
        None => diags.push(Diagnostic::error(
            "harness_endpoint_bad_path",
            format!("endpoint '{}' is missing path", ep.name),
            Some(ep.name.clone()),
        )),
        Some(p) if !valid_endpoint_path(p) => diags.push(Diagnostic::error(
            "harness_endpoint_bad_path",
            format!("endpoint '{}' path '{p}' must start with / and use {{brace}} params only", ep.name),
            Some(ep.name.clone()),
        )),
        Some(_) => {}
    }

    let handler_name = handles.first().and_then(|f| type_ident(&f.type_expr));
    let handler = handler_name.and_then(|n| all.iter().copied().find(|c| c.name == n));
    match (handler_name, handler) {
        (None, _) => diags.push(Diagnostic::error(
            "harness_endpoint_unknown_handler",
            format!("endpoint '{}' is missing handle", ep.name),
            Some(ep.name.clone()),
        )),
        (Some(n), None) => diags.push(Diagnostic::error(
            "harness_endpoint_unknown_handler",
            format!("endpoint '{}' handle '{n}' is not a construct in this package", ep.name),
            Some(ep.name.clone()),
        )),
        (Some(n), Some(h)) if h.shape != Shape::Fn => diags.push(Diagnostic::error(
            "harness_endpoint_unknown_handler",
            format!("endpoint '{n}' does not name a fn-shaped construct"),
            Some(ep.name.clone()),
        )),
        (Some(_), Some(h)) => {
            check_binds(ep, h, registry, path, diags);
        }
    }
}

fn check_binds(
    ep: &Construct,
    handler: &Construct,
    registry: &LayerRegistry,
    path: Option<&str>,
    diags: &mut Vec<Diagnostic>,
) {
    let bind_fields: Vec<&Field> = ep
        .blocks
        .iter()
        .filter(|b| b.keyword == "bind")
        .flat_map(|b| b.fields.iter())
        .collect();
    let http_inputs: Vec<&Field> = handler
        .inputs
        .iter()
        .filter(|i| !registry.field_is_dependency(i))
        .collect();

    for bf in &bind_fields {
        if !http_inputs.iter().any(|i| i.name == bf.name) {
            diags.push(Diagnostic::error(
                "harness_bind_unknown_input",
                format!(
                    "endpoint '{}' binds '{}' which is not an HTTP input of '{}'",
                    ep.name, bf.name, handler.name
                ),
                Some(ep.name.clone()),
            ));
        }
        if let Some(src) = type_ident(&bf.type_expr) {
            if !matches!(src, "path" | "query" | "body" | "header" | "tenant") {
                diags.push(Diagnostic::error(
                    "harness_bind_unknown_input",
                    format!(
                        "endpoint '{}' bind '{}: {src}' is not path|query|body|header|tenant",
                        ep.name, bf.name
                    ),
                    Some(ep.name.clone()),
                ));
            }
        }
    }

    let bind_defaults = registry.harness_policy.bind_defaults;
    let require_binds = matches!(bind_defaults, Some(crate::harness::BindDefaults::None))
        || registry.harness_policy.compat == Some(CompatMode::Off);
    if require_binds {
        for inp in &http_inputs {
            if !bind_fields.iter().any(|b| b.name == inp.name) {
                diags.push(Diagnostic::error(
                    "harness_bind_missing",
                    format!(
                        "endpoint '{}' is missing bind for handler input '{}'",
                        ep.name, inp.name
                    ),
                    Some(ep.name.clone()),
                ));
            }
        }
    }

    if let Some(p) = path {
        for param in path_params(p) {
            let bound = bind_fields.iter().any(|b| {
                b.name == param
                    && type_ident(&b.type_expr).is_some_and(|s| s == "path")
            });
            if !bound {
                diags.push(Diagnostic::error(
                    "harness_bind_unused_path_param",
                    format!(
                        "endpoint '{}' path param '{{{param}}}' has no `bind {param}: path`",
                        ep.name
                    ),
                    Some(ep.name.clone()),
                ));
            }
        }
    }
}

fn check_compose(
    compose: &Construct,
    deps_bundles: &[&Construct],
    all: &[&Construct],
    registry: &LayerRegistry,
    diags: &mut Vec<Diagnostic>,
) {
    let bundle_name = compose
        .fields
        .iter()
        .find(|f| f.name == "bundle")
        .and_then(|f| type_ident(&f.type_expr));
    let bundle = match bundle_name {
        None => {
            diags.push(Diagnostic::error(
                "harness_compose_unknown_bundle",
                format!("compose '{}' is missing bundle", compose.name),
                Some(compose.name.clone()),
            ));
            return;
        }
        Some(n) => match deps_bundles.iter().copied().find(|d| d.name == n) {
            Some(b) => b,
            None => {
                diags.push(Diagnostic::error(
                    "harness_compose_unknown_bundle",
                    format!("compose '{}' bundle '{n}' is not a deps construct", compose.name),
                    Some(compose.name.clone()),
                ));
                return;
            }
        },
    };

    for f in &bundle.fields {
        if !is_trait_like(f, all, registry) {
            diags.push(Diagnostic::error(
                "harness_deps_unknown_trait",
                format!(
                    "deps '{}' field '{}' type is not a trait-shaped construct",
                    bundle.name, f.name
                ),
                Some(bundle.name.clone()),
            ));
        }
    }

    let wires: Vec<&Field> = compose
        .blocks
        .iter()
        .filter(|b| b.keyword == "wire")
        .flat_map(|b| b.fields.iter())
        .collect();

    for df in &bundle.fields {
        if !wires.iter().any(|w| w.name == df.name) {
            diags.push(Diagnostic::error(
                "harness_compose_missing_field",
                format!(
                    "compose '{}' is missing wire for deps field '{}'",
                    compose.name, df.name
                ),
                Some(compose.name.clone()),
            ));
        }
    }

    for w in &wires {
        let Some(target) = type_ident(&w.type_expr) else {
            continue;
        };
        let Some(df) = bundle.fields.iter().find(|f| f.name == w.name) else {
            continue;
        };
        let trait_name = type_ident(&df.type_expr).unwrap_or("");
        if is_provided_runtime_ident(target, registry) {
            if !trait_is_runtime_provided(trait_name, all, registry) {
                diags.push(Diagnostic::error(
                    "harness_provided_runtime_not_marked",
                    format!(
                        "compose '{}' wires '{}: {target}' but trait '{trait_name}' is not runtime-provided",
                        compose.name, w.name
                    ),
                    Some(compose.name.clone()),
                ));
            }
            continue;
        }
        let adapter = all.iter().copied().find(|c| c.name == target);
        match adapter {
            None => diags.push(Diagnostic::error(
                "harness_compose_unknown_adapter",
                format!(
                    "compose '{}' wire '{}' names unknown adapter '{target}'",
                    compose.name, w.name
                ),
                Some(compose.name.clone()),
            )),
            Some(ad) if ad.shape != Shape::Impl => diags.push(Diagnostic::error(
                "harness_compose_unknown_adapter",
                format!("'{target}' is not an adapter (impl-shaped)"),
                Some(compose.name.clone()),
            )),
            Some(ad) => {
                if let Some(for_trait) = &ad.target {
                    if for_trait != trait_name && !trait_name.is_empty() {
                        diags.push(Diagnostic::error(
                            "harness_compose_adapter_trait_mismatch",
                            format!(
                                "adapter '{target}' implements '{for_trait}', not '{trait_name}'"
                            ),
                            Some(compose.name.clone()),
                        ));
                    }
                }
            }
        }
    }
}

pub(crate) fn type_ident(ty: &TypeExpr) -> Option<&str> {
    match ty {
        TypeExpr::Named(n) | TypeExpr::LitStr(n) => Some(n.as_str()),
        _ => None,
    }
}

fn valid_endpoint_path(p: &str) -> bool {
    if p.contains("://") {
        return false;
    }
    if !(p.starts_with('/') || p.starts_with('{')) {
        return false;
    }
    if p.contains(':') && !p.contains("://") {
        // Express-style :id is rejected
        if p.split('/').any(|seg| seg.starts_with(':')) {
            return false;
        }
    }
    true
}

fn path_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            let name = after[..end].trim();
            if !name.is_empty() {
                out.push(name.to_string());
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    out
}

fn endpoint_method_path(ep: &Construct) -> Option<(String, String)> {
    let method = ep
        .fields
        .iter()
        .find(|f| f.name == "method")
        .and_then(|f| type_ident(&f.type_expr))?
        .to_ascii_uppercase();
    let path = ep
        .fields
        .iter()
        .find(|f| f.name == "path")
        .and_then(|f| type_ident(&f.type_expr))?
        .to_string();
    Some((method, path))
}

fn is_trait_like(f: &Field, all: &[&Construct], registry: &LayerRegistry) -> bool {
    let Some(name) = type_ident(&f.type_expr) else {
        return false;
    };
    if let Some(c) = all.iter().copied().find(|c| c.name == name) {
        return c.shape == Shape::Trait;
    }
    registry
        .constructs
        .iter()
        .any(|s| (s.name == name || s.keyword == name) && s.shape == Shape::Trait)
        || registry
            .declarations
            .iter()
            .any(|d| d.contains(&format!("trait {name}")))
}

fn is_provided_runtime_ident(name: &str, registry: &LayerRegistry) -> bool {
    if name == "provided_runtime" {
        return true;
    }
    registry.constructs.iter().any(|s| {
        (s.keyword == name || s.name == name)
            && s.roles.iter().any(|r| r == "runtime_provider")
    })
}

fn trait_is_runtime_provided(trait_name: &str, all: &[&Construct], registry: &LayerRegistry) -> bool {
    if trait_name.is_empty() {
        return false;
    }
    if registry
        .harness_policy
        .provided_runtime_traits
        .iter()
        .any(|t| t == trait_name)
    {
        return true;
    }
    if registry.routing_traits().iter().any(|t| t == trait_name) {
        return true;
    }
    if registry.is_auth_service_trait(trait_name) {
        return true;
    }
    if let Some(c) = all.iter().copied().find(|c| c.name == trait_name) {
        if registry.construct_has_role(c, "runtime_provider") {
            return true;
        }
    }
    registry.constructs.iter().any(|s| {
        (s.name == trait_name || s.keyword == trait_name)
            && s.roles.iter().any(|r| r == "runtime_provider")
    })
}

fn collect_all_constructs(sol: &Solution) -> Vec<&Construct> {
    let mut out = Vec::new();
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            walk_construct(c, &mut out);
        }
    }
    out
}

fn walk_construct<'a>(c: &'a Construct, out: &mut Vec<&'a Construct>) {
    out.push(c);
    for child in &c.children {
        walk_construct(child, out);
    }
}

/// Whether `names.rs` should skip `check_type_expr` for this field.
pub fn skip_name_check_for_field(
    owner: &Construct,
    field: &Field,
    block_keyword: Option<&str>,
    registry: &LayerRegistry,
) -> bool {
    if matches!(block_keyword, Some("bind")) {
        return true;
    }
    if matches!(block_keyword, Some("wire")) {
        if let Some(n) = type_ident(&field.type_expr) {
            return is_provided_runtime_ident(n, registry);
        }
        return false;
    }
    let keys = registry.construct_config_keys(owner);
    if keys.iter().any(|k| k == &field.name) {
        // handle / bundle name real constructs — still check.
        return field.name != "handle" && field.name != "bundle";
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Construct, Field, NamedBlock, Solution, TopLevelItem};
    use crate::layer::LayerRegistry;
    use crate::span::Span;

    fn field(name: &str, ty: &str) -> Field {
        Field {
            annotations: Vec::new(),
            name: name.into(),
            type_expr: if ty.starts_with('/') {
                TypeExpr::LitStr(ty.into())
            } else {
                TypeExpr::Named(ty.into())
            },
            default_expr: None,
            span: Span::new(0, 0),
        }
    }

    fn harness_reg() -> LayerRegistry {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("harness", include_str!("../../../layers/harness.layer"))
            .expect("harness");
        reg
    }

    #[test]
    fn bad_method_and_unknown_handler() {
        let reg = harness_reg();
        let mut ep = Construct::new(
            "endpoint",
            "HttpEndpoint",
            Shape::Struct,
            "BadHttp".into(),
            Span::new(0, 0),
        );
        ep.fields = vec![
            field("method", "FOO"),
            field("path", "/api/x"),
            field("handle", "Missing"),
        ];
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            links: Vec::new(),
            items: vec![TopLevelItem::Construct(ep)],
            expose: None,
            guidance: Vec::new(),
        };
        let diags = check_harness(&sol, &reg);
        assert!(
            diags.iter().any(|d| d.code == "harness_endpoint_bad_method"),
            "{:?}",
            diags
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == "harness_endpoint_unknown_handler"),
            "{:?}",
            diags
        );
    }

    #[test]
    fn bind_path_param_and_good_handler() {
        let reg = harness_reg();
        let mut handler = Construct::new(
            "fn",
            "Fn",
            Shape::Fn,
            "GetItem".into(),
            Span::new(0, 0),
        );
        handler.inputs = vec![field("id", "Str")];
        let mut ep = Construct::new(
            "endpoint",
            "HttpEndpoint",
            Shape::Struct,
            "GetItemHttp".into(),
            Span::new(0, 0),
        );
        ep.fields = vec![
            field("method", "GET"),
            field("path", "/api/items/{id}"),
            field("handle", "GetItem"),
        ];
        ep.blocks.push(NamedBlock {
            keyword: "bind".into(),
            shape: Shape::Struct,
            name: None,
            fields: vec![field("id", "path")],
            variants: Vec::new(),
            transitions: Vec::new(),
            span: Span::new(0, 0),
        });
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            links: Vec::new(),
            items: vec![
                TopLevelItem::Construct(handler),
                TopLevelItem::Construct(ep),
            ],
            expose: None,
            guidance: Vec::new(),
        };
        let diags = check_harness(&sol, &reg);
        assert!(
            !diags.iter().any(|d| d.severity == Severity::Error
                && d.code.starts_with("harness_endpoint")
                || d.code.starts_with("harness_bind")),
            "{:?}",
            diags
        );
    }

    #[test]
    fn leftover_http_route_is_error_when_compat_off() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content(
            "legacy_route",
            "pkg legacy_route v1\n  construct Svc\n    kw svc\n    mt fn\n    ann\n      route: \"HTTP\" method_path role:http_route\n",
        )
        .unwrap();
        reg.harness_policy.compat = Some(CompatMode::Off);
        let mut svc = Construct::new(
            "svc",
            "Svc",
            Shape::Fn,
            "ListItems".into(),
            Span::new(0, 0),
        );
        svc.annotations.push(crate::ast::Annotation {
            name: "route".into(),
            args: vec!["GET /api/items".into()],
            span: Span::new(0, 0),
        });
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            links: Vec::new(),
            items: vec![TopLevelItem::Construct(svc)],
            expose: None,
            guidance: Vec::new(),
        };
        let diags = check_harness(&sol, &reg);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "harness_route_annotation_removed"
                    && d.severity == Severity::Error),
            "{:?}",
            diags
        );
    }
}
