//! Mechanical rewrite: `@route` / implicit Deps → `endpoint` / `deps` / `compose`.
//!
//! Dry-run by default (CLI `--write` applies). See DESIGN_CONFIGURABLE_HARNESS.md.

use veil_ir::ast::{
    Annotation, Construct, Field, NamedBlock, Solution, TopLevelItem, TypeExpr, UseImport,
};
use veil_ir::layer::{LayerRegistry, Shape};
use veil_ir::span::Span;

use crate::rust::{
    build_name_to_shape, collect_deps_field_map, flatten_module, http_routable_services,
    rest_route_for_service,
};

#[derive(Debug, Default)]
pub struct MigrateReport {
    pub endpoints: Vec<String>,
    pub id_rewrites: usize,
    pub deps_inserted: Vec<String>,
    pub ambiguous_adapters: Vec<String>,
    pub stripped_routes: usize,
}

impl MigrateReport {
    pub fn summary(&self) -> String {
        format!(
            "endpoints={} :id_rewrites={} @dep_inserted={} stripped_@route={} ambiguous_adapters={}",
            self.endpoints.len(),
            self.id_rewrites,
            self.deps_inserted.len(),
            self.stripped_routes,
            self.ambiguous_adapters.len()
        )
    }
}

/// Mutate `sol` in place. Caller serializes.
pub fn migrate_harness(sol: &mut Solution, registry: &LayerRegistry) -> MigrateReport {
    let mut report = MigrateReport::default();
    if !sol.uses.iter().any(|u| u.package_name == "harness") {
        sol.uses.push(UseImport {
            package_name: "harness".into(),
            alias: None,
            span: Span::new(0, 0),
        });
    }

    let name_to_shape = build_name_to_shape(sol, registry);
    let module_idxs: Vec<usize> = sol
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            TopLevelItem::Construct(c) if c.shape == Shape::Mod => Some(i),
            _ => None,
        })
        .collect();

    for i in module_idxs {
        let TopLevelItem::Construct(module) = &mut sol.items[i] else {
            continue;
        };
        migrate_module(module, registry, &name_to_shape, &mut report);
    }
    report
}

fn migrate_module(
    module: &mut Construct,
    registry: &LayerRegistry,
    name_to_shape: &std::collections::HashMap<String, Shape>,
    report: &mut MigrateReport,
) {
    let flat = flatten_module(module, registry);
    let routable = http_routable_services(&flat.fns, registry);
    let (_deps_set, dep_fields) = collect_deps_field_map(&flat.fns, registry, name_to_shape);

    let existing_handlers: std::collections::HashSet<String> = collect_existing_endpoint_handlers(module)
        .into_iter()
        .collect();

    let mut new_endpoints: Vec<Construct> = Vec::new();
    for svc in &routable {
        if existing_handlers.contains(&svc.name) {
            continue;
        }
        let (method, path) = rest_route_for_service(svc, registry);
        let (path, n) = rewrite_express_params(&path);
        report.id_rewrites += n;
        let binds = infer_binds(svc, &method, &path, registry);
        new_endpoints.push(make_endpoint(svc, &method, &path, binds));
        report.endpoints.push(format!(
            "{} {} {} -> {}",
            module.name,
            method.to_ascii_uppercase(),
            path,
            svc.name
        ));
    }

    // @dep on handlers
    insert_deps_on_handlers(module, registry, &dep_fields, report);

    // Strip @route (role:http_route) on fn-shaped children
    report.stripped_routes += strip_http_routes(module, registry);

    if !dep_fields.is_empty() && !module_has_role(module, registry, "deps_bundle") {
        let deps_name = format!("{}Deps", module.name);
        module_presentation(module)
            .children
            .push(make_deps(&deps_name, &dep_fields));
        if !module_has_role(module, registry, "compose") {
            let compose_name = format!("{}Local", module.name);
            let (wires, ambiguous) = pick_wires(module, registry, &dep_fields);
            report.ambiguous_adapters.extend(ambiguous);
            module_presentation(module)
                .children
                .push(make_compose(&compose_name, &deps_name, wires));
        }
    }

    if !new_endpoints.is_empty() {
        let pres = module_presentation(module);
        pres.children.extend(new_endpoints);
    }
}

fn collect_existing_endpoint_handlers(module: &Construct) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(c: &Construct, out: &mut Vec<String>) {
        if c.keyword == "endpoint" || c.subkind == "HttpEndpoint" {
            if let Some(h) = c
                .fields
                .iter()
                .find(|f| f.name == "handle")
                .and_then(|f| match &f.type_expr {
                    TypeExpr::Named(n) => Some(n.clone()),
                    _ => None,
                })
            {
                out.push(h);
            }
        }
        for ch in &c.children {
            walk(ch, out);
        }
    }
    walk(module, &mut out);
    out
}

fn module_has_role(module: &Construct, registry: &LayerRegistry, role: &str) -> bool {
    fn walk(c: &Construct, registry: &LayerRegistry, role: &str) -> bool {
        if registry.construct_has_role(c, role) {
            return true;
        }
        c.children.iter().any(|ch| walk(ch, registry, role))
    }
    walk(module, registry, role)
}

fn module_presentation(module: &mut Construct) -> &mut Construct {
    if let Some(i) = module
        .children
        .iter()
        .position(|c| c.shape == Shape::Group && c.name == "presentation")
    {
        return &mut module.children[i];
    }
    module.children.push(Construct::new(
        "group",
        "Group",
        Shape::Group,
        "presentation".into(),
        Span::new(0, 0),
    ));
    module.children.last_mut().unwrap()
}

fn make_endpoint(svc: &Construct, method: &str, path: &str, binds: Vec<Field>) -> Construct {
    let mut ep = Construct::new(
        "endpoint",
        "HttpEndpoint",
        Shape::Struct,
        format!("{}Http", svc.name),
        Span::new(0, 0),
    );
    ep.fields.push(field("method", TypeExpr::Named(method.to_ascii_uppercase())));
    ep.fields.push(field("path", TypeExpr::LitStr(path.to_string())));
    ep.fields
        .push(field("handle", TypeExpr::Named(svc.name.clone())));
    if !binds.is_empty() {
        ep.blocks.push(NamedBlock {
            keyword: "bind".into(),
            shape: Shape::Struct,
            name: None,
            fields: binds,
            variants: Vec::new(),
            transitions: Vec::new(),
            span: Span::new(0, 0),
        });
    }
    ep
}

fn make_deps(name: &str, dep_fields: &std::collections::HashMap<String, String>) -> Construct {
    let mut c = Construct::new(
        "deps",
        "DepsBundle",
        Shape::Struct,
        name.into(),
        Span::new(0, 0),
    );
    let mut pairs: Vec<(&String, &String)> = dep_fields.iter().collect();
    pairs.sort_by(|a, b| a.1.cmp(b.1));
    for (trait_name, field_name) in pairs {
        c.fields
            .push(field(field_name, TypeExpr::Named(trait_name.clone())));
    }
    c
}

fn make_compose(
    name: &str,
    bundle: &str,
    wires: Vec<Field>,
) -> Construct {
    let mut c = Construct::new(
        "compose",
        "ComposeRoot",
        Shape::Struct,
        name.into(),
        Span::new(0, 0),
    );
    c.fields
        .push(field("bundle", TypeExpr::Named(bundle.into())));
    if !wires.is_empty() {
        c.blocks.push(NamedBlock {
            keyword: "wire".into(),
            shape: Shape::Struct,
            name: None,
            fields: wires,
            variants: Vec::new(),
            transitions: Vec::new(),
            span: Span::new(0, 0),
        });
    }
    c
}

fn pick_wires(
    module: &Construct,
    registry: &LayerRegistry,
    dep_fields: &std::collections::HashMap<String, String>,
) -> (Vec<Field>, Vec<String>) {
    let flat = flatten_module(module, registry);
    let mut wires = Vec::new();
    let mut ambiguous = Vec::new();
    let routing = registry.routing_traits();
    for (trait_name, field_name) in dep_fields {
        if routing.iter().any(|t| t == trait_name) || registry.is_auth_service_trait(trait_name)
        {
            wires.push(field(
                field_name,
                TypeExpr::Named("provided_runtime".into()),
            ));
            continue;
        }
        let adapters: Vec<&&Construct> = flat
            .impls
            .iter()
            .filter(|ad| ad.target.as_deref() == Some(trait_name.as_str()))
            .collect();
        match adapters.as_slice() {
            [ad] => wires.push(field(field_name, TypeExpr::Named(ad.name.clone()))),
            [] => {}
            many => {
                ambiguous.push(format!(
                    "{trait_name}: {}",
                    many.iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join("|")
                ));
            }
        }
    }
    (wires, ambiguous)
}

fn infer_binds(
    svc: &Construct,
    method: &str,
    path: &str,
    registry: &LayerRegistry,
) -> Vec<Field> {
    let params = brace_params(path);
    let mut binds = Vec::new();
    for inp in &svc.inputs {
        if registry.field_is_dependency(inp) {
            continue;
        }
        let src = if params.iter().any(|p| p == &inp.name) {
            "path"
        } else if inp.name == "tenant_id" {
            "tenant"
        } else if matches!(method.to_ascii_lowercase().as_str(), "get" | "delete" | "head")
        {
            "query"
        } else {
            "body"
        };
        binds.push(field(&inp.name, TypeExpr::Named(src.into())));
    }
    binds
}

fn insert_deps_on_handlers(
    module: &mut Construct,
    registry: &LayerRegistry,
    dep_fields: &std::collections::HashMap<String, String>,
    report: &mut MigrateReport,
) {
    fn walk(
        c: &mut Construct,
        registry: &LayerRegistry,
        dep_fields: &std::collections::HashMap<String, String>,
        report: &mut MigrateReport,
    ) {
        if c.shape == Shape::Fn {
            for (trait_name, field_name) in dep_fields {
                let already = c.inputs.iter().any(|i| {
                    registry.field_is_dependency(i)
                        && matches!(&i.type_expr, TypeExpr::Named(n) if n == trait_name)
                });
                if already {
                    continue;
                }
                let uses = c.inputs.iter().any(|i| {
                    matches!(&i.type_expr, TypeExpr::Named(n) if n == trait_name)
                }) || construct_mentions_ident(c, trait_name);
                if !uses {
                    continue;
                }
                let mut f = field(field_name, TypeExpr::Named(trait_name.clone()));
                f.annotations.push(Annotation {
                    name: "dep".into(),
                    args: Vec::new(),
                    span: Span::new(0, 0),
                });
                c.inputs.push(f);
                report
                    .deps_inserted
                    .push(format!("{}@{field_name}: {trait_name}", c.name));
            }
        }
        for ch in &mut c.children {
            walk(ch, registry, dep_fields, report);
        }
    }
    walk(module, registry, dep_fields, report);
}

fn construct_mentions_ident(c: &Construct, ident: &str) -> bool {
    fn expr_mentions(e: &veil_ir::ast::Expr, ident: &str) -> bool {
        match e {
            veil_ir::ast::Expr::Ident(n) => n == ident,
            veil_ir::ast::Expr::Call(call) => {
                call.target == ident
                    || call
                        .receiver
                        .as_ref()
                        .is_some_and(|r| expr_mentions(r, ident))
                    || call.args.iter().any(|a| expr_mentions(a, ident))
            }
            veil_ir::ast::Expr::FieldAccess(base, _) => expr_mentions(base, ident),
            _ => false,
        }
    }
    c.steps.iter().any(|st| match st {
        veil_ir::ast::FlowStep::Step(s) => s.body.iter().any(|e| expr_mentions(e, ident)),
        _ => false,
    })
}

fn strip_http_routes(module: &mut Construct, registry: &LayerRegistry) -> usize {
    let mut n = 0;
    fn walk(c: &mut Construct, registry: &LayerRegistry, n: &mut usize) {
        let before = c.annotations.len();
        c.annotations
            .retain(|a| !registry.is_http_route_annotation(&a.name));
        *n += before - c.annotations.len();
        for ch in &mut c.children {
            walk(ch, registry, n);
        }
    }
    walk(module, registry, &mut n);
    n
}

fn rewrite_express_params(path: &str) -> (String, usize) {
    let mut out = String::new();
    let mut n = 0;
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ':'
            && (i == 0 || chars[i - 1] == '/')
            && i + 1 < chars.len()
            && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_')
        {
            i += 1;
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            out.push('{');
            out.push_str(&name);
            out.push('}');
            n += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    (out, n)
}

fn brace_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(s) = rest.find('{') {
        let after = &rest[s + 1..];
        if let Some(e) = after.find('}') {
            let name = after[..e].trim();
            if !name.is_empty() {
                out.push(name.to_string());
            }
            rest = &after[e + 1..];
        } else {
            break;
        }
    }
    out
}

fn field(name: &str, ty: TypeExpr) -> Field {
    Field {
        annotations: Vec::new(),
        name: name.into(),
        type_expr: ty,
        default_expr: None,
        span: Span::new(0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_colon_id() {
        let (p, n) = rewrite_express_params("/api/items/:id");
        assert_eq!(p, "/api/items/{id}");
        assert_eq!(n, 1);
    }

    #[test]
    fn migrate_l1_crud_emits_endpoints() {
        let src = r#"
pkg LadderL1
  use ddd
  ctx Catalog
    group domain
      port ItemRepo
        find!(id: Id) -> Opt<Item>
    group application
      @route("GET /api/items/:id")
      svc GetItem
        input
          id: Id
          @dep item_repo: ItemRepo
        ret id
    group infrastructure
      impl MemItemRepo for ItemRepo
        impl find(id)
          ret null
"#;
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .unwrap();
        reg.load_content("harness", include_str!("../../../layers/harness.layer"))
            .unwrap();
        let tokens = veil_parser::lex(src);
        let mut sol = veil_parser::parse_with_registry(&tokens, reg.clone()).unwrap();
        let report = migrate_harness(&mut sol, &reg);
        assert!(
            report.endpoints.iter().any(|e| e.contains("/api/items/{id}")),
            "{:?}",
            report.endpoints
        );
        assert!(report.id_rewrites >= 1, "{report:?}");
        let out = veil_ir::serialize::serialize_solution(&sol);
        assert!(out.contains("endpoint GetItemHttp"), "{out}");
        assert!(out.contains("path: \"/api/items/{id}\""), "{out}");
        assert!(!out.contains("@route"), "{out}");
        assert!(out.contains("use harness"), "{out}");
    }
}
