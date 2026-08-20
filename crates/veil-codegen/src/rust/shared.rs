use veil_ir::ast::*;
use veil_ir::layer::LayerRegistry;
use super::*;

/// RT-008: AllowAllAuth from the configured auth trait + Principal-like structs.
pub fn gen_allow_all_auth_impl(
    auth_trait: &Construct,
    structs: &[&Construct],
    trait_names: &std::collections::HashSet<String>,
    registry: &LayerRegistry,
) -> String {
    let trait_name = &auth_trait.name;
    let err_type = registry.error_model.as_ref().map(|em| em.type_name.as_str()).unwrap_or("__VEIL_NO_ERROR_MODEL__");
    // Prefer a layer-provided struct referenced by any method return type.
    let principal = auth_trait
        .methods
        .iter()
        .find_map(|m| {
            let inner = match &m.return_type {
                Some(TypeExpr::Result(Some(i))) => i.as_ref(),
                Some(t) => t,
                None => return None,
            };
            if let TypeExpr::Named(n) = inner {
                structs.iter().find(|s| s.name == *n).copied()
            } else {
                None
            }
        })
        .or_else(|| {
            structs
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case("principal"))
                .copied()
        });

    let mut out = format!(
        r#"/// Dev/local {trait_name} — allows all tokens and permissions (RT-008).
/// Host harnesses replace this with Cognito/Auth0/etc. via `provided_by: runtime`.
pub struct AllowAllAuth;

#[async_trait]
impl {trait_name} for AllowAllAuth {{
"#
    );
    for method in &auth_trait.methods {
        let mname = to_snake(&method.name);
        let ret = match &method.return_type {
            Some(t) => format!(" -> {}", type_to_rust_with_traits(t, trait_names, err_type)),
            None => String::new(),
        };
        // First Str-like param is treated as token for principal identity.
        let token_param = method.params.iter().find(|p| {
            matches!(
                type_to_rust(&p.type_expr).as_str(),
                "String" | "&str" | "str"
            )
        });
        let ret_inner = match &method.return_type {
            Some(TypeExpr::Result(Some(i))) => Some(i.as_ref()),
            Some(TypeExpr::Result(None)) | None => None,
            Some(t) => Some(t),
        };
        let body = match ret_inner {
            Some(TypeExpr::Named(n)) if n == "Bool" || n == "bool" => {
                "        Ok(true)".to_string()
            }
            Some(TypeExpr::Named(n)) => {
                if let Some(pstruct) = principal.filter(|s| s.name == *n) {
                    let token_expr = token_param
                        .map(|p| {
                            let pn = to_snake(&p.name);
                            format!(
                                "if {pn}.is_empty() {{ \"anonymous\".into() }} else {{ {pn} }}"
                            )
                        })
                        .unwrap_or_else(|| "\"anonymous\".into()".to_string());
                    let mut fields = Vec::new();
                    for f in &pstruct.fields {
                        let fname = to_snake(&f.name);
                        let ft = type_to_rust(&f.type_expr);
                        let val = if fname == "id" || fname.ends_with("_id") {
                            token_expr.clone()
                        } else if ft.starts_with("Vec<") {
                            if ft.contains("String") {
                                "vec![\"local\".into()]".to_string()
                            } else {
                                "vec![]".to_string()
                            }
                        } else if ft.contains("HashMap") {
                            "std::collections::HashMap::new()".to_string()
                        } else if ft == "String" {
                            "String::new()".to_string()
                        } else if ft == "bool" {
                            "false".to_string()
                        } else if ft == "i64" {
                            "0".to_string()
                        } else {
                            format!("{ft}::default()")
                        };
                        fields.push(format!("            {fname}: {val},"));
                    }
                    format!(
                        "        Ok({n} {{\n{}\n        }})",
                        fields.join("\n")
                    )
                } else {
                    format!("        Ok({n}::default())")
                }
            }
            None => "        Ok(())".to_string(),
            Some(_) => {
                let external_path = registry.error_model.as_ref()
                    .and_then(|em| em.variant_path("external"))
                    .unwrap_or_else(|| "__VEIL_NO_ERROR_MODEL__::__NO_EXTERNAL__".to_string());
                format!("        Err({external_path}(\"allow-all: unsupported return\".into()))")
            }
        };
        // Prefix unused params with underscore when not referenced in body.
        let params_for_sig: String = method
            .params
            .iter()
            .map(|p| {
                let pn = to_snake(&p.name);
                let ty = param_type_to_rust(&p.type_expr, trait_names, err_type);
                let used = token_param
                    .map(|t| to_snake(&t.name) == pn)
                    .unwrap_or(false)
                    && matches!(
                        ret_inner,
                        Some(TypeExpr::Named(n)) if principal.map(|s| s.name == *n).unwrap_or(false)
                    );
                if used {
                    format!("{pn}: {ty}")
                } else {
                    format!("_{pn}: {ty}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sep2 = if params_for_sig.is_empty() { "" } else { ", " };
        out.push_str(&format!(
            "    async fn {mname}(&self{sep2}{params_for_sig}){ret} {{\n{body}\n    }}\n\n"
        ));
    }
    out.push_str("}\n");
    out
}

/// Generate the shared library crate that all context crates depend on. It
/// owns the common error types and layer-provided top-level traits, so there
/// is exactly one definition of each across the workspace.
/// CAP-003: handler message names from application fns across modules.
pub fn collect_handler_names(
    solution: &Solution,
    modules: &[&Construct],
    registry: &LayerRegistry,
) -> Vec<String> {
    let mut names = Vec::new();
    for module in modules {
        let flat = flatten_module(module, registry);
        for f in &flat.fns {
            if veil_ir::is_deploy_hook(f, registry) {
                continue;
            }
            let message = registry.bus_message_name(&f.name);
            if !names.contains(&message) {
                names.push(message);
            }
        }
    }
    for item in &solution.items {
        if let TopLevelItem::Function(f) = item {
            let message = registry.bus_message_name(&f.name);
            if !names.contains(&message) {
                names.push(message);
            }
        }
    }
    names.sort();
    names
}


