use veil_ir::ast::*;
use veil_ir::layer::LayerRegistry;
use super::*;

/// RT-001/004: InProcessBus + handler registry, driven only by the routing
/// trait's method surface (no hard-coded `Bus` / dispatch|invoke|request names).
pub fn gen_inprocess_bus_impl(
    routing_trait: &Construct,
    trait_names: &std::collections::HashSet<String>,
) -> String {
    let trait_name = &routing_trait.name;
    let mut out = String::from(
        r#"// ─── InProcessBus (local harness, RT-001 / RT-004) ─────────────────────────
// Methods generated from the layer-declared routing trait surface.
use std::collections::HashMap;
use std::sync::Arc;
use futures::future::BoxFuture;
use futures::FutureExt;

type BusHandler = Arc<
    dyn Fn(serde_json::Value) -> BoxFuture<'static, Result<serde_json::Value, DomainError>>
        + Send
        + Sync,
>;

/// In-process message bus for local multi-context runs.
#[derive(Clone, Default)]
pub struct InProcessBus {
    handlers: Arc<std::sync::Mutex<HashMap<String, BusHandler>>>,
}

impl InProcessBus {
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Register a handler for a message type name (manifest `handlers` keys).
    pub fn register<F, Fut>(&self, name: impl Into<String>, f: F)
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<serde_json::Value, DomainError>> + Send + 'static,
    {
        let name = name.into();
        let handler: BusHandler = Arc::new(move |v| f(v).boxed());
        self.handlers
            .lock()
            .expect("bus lock")
            .insert(name, handler);
    }

    fn lookup(&self, type_name: &str) -> Option<BusHandler> {
        self.handlers
            .lock()
            .expect("bus lock")
            .get(type_name)
            .cloned()
    }
}

"#,
    );
    out.push_str(&format!(
        "#[async_trait]\nimpl {trait_name} for InProcessBus {{\n"
    ));
    for method in &routing_trait.methods {
        let mname = to_snake(&method.name);
        let params_sig: Vec<String> = method
            .params
            .iter()
            .map(|p| {
                format!(
                    "{}: {}",
                    to_snake(&p.name),
                    param_type_to_rust(&p.type_expr, trait_names)
                )
            })
            .collect();
        let params_joined = params_sig.join(", ");
        let sep = if params_joined.is_empty() { "" } else { ", " };
        let ret = match &method.return_type {
            Some(t) => format!(" -> {}", type_to_rust_with_traits(t, trait_names)),
            None => String::new(),
        };
        // First Json/Value-like param is the envelope (type field + payload).
        let envelope = method
            .params
            .iter()
            .find(|p| {
                let r = type_to_rust(&p.type_expr);
                r.contains("Value") || r == "serde_json::Value"
            })
            .map(|p| to_snake(&p.name))
            .or_else(|| method.params.first().map(|p| to_snake(&p.name)));

        let unit_result = matches!(
            &method.return_type,
            None | Some(TypeExpr::Result(None))
        );
        let body = if let Some(env) = envelope {
            if unit_result {
                // Fire-and-forget (dispatch-style)
                format!(
                    r#"        let type_name = {env}
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(handler) = self.lookup(&type_name) {{
            let payload = {env}.clone();
            tokio::spawn(async move {{
                let _ = handler(payload).await;
            }});
        }}
        Ok(())"#
                )
            } else {
                // Request/response (invoke-style)
                format!(
                    r#"        let type_name = {env}
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let handler = self
            .lookup(&type_name)
            .ok_or(DomainError::NotFound)?;
        handler({env}).await"#
                )
            }
        } else if unit_result {
            "        Ok(())".to_string()
        } else {
            "        Err(DomainError::External(\"no envelope param\".into()))".to_string()
        };
        out.push_str(&format!(
            "    async fn {mname}(&self{sep}{params_joined}){ret} {{\n{body}\n    }}\n\n"
        ));
    }
    out.push_str("}\n");
    out
}

/// RT-008: AllowAllAuth from the configured auth trait + Principal-like structs.
pub fn gen_allow_all_auth_impl(
    auth_trait: &Construct,
    structs: &[&Construct],
    trait_names: &std::collections::HashSet<String>,
) -> String {
    let trait_name = &auth_trait.name;
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
            Some(t) => format!(" -> {}", type_to_rust_with_traits(t, trait_names)),
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
            Some(_) => "        Err(DomainError::External(\"allow-all: unsupported return\".into()))"
                .to_string(),
        };
        // Prefix unused params with underscore when not referenced in body.
        let params_for_sig: String = method
            .params
            .iter()
            .map(|p| {
                let pn = to_snake(&p.name);
                let ty = param_type_to_rust(&p.type_expr, trait_names);
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

pub fn gen_register_handlers_module(handler_names: &[String]) -> String {
    let mut out = String::from(
        "//! CAP-003: generated Bus handler registry.\n\
         //! Host calls `register_all` once to wire names → dispatch.\n\n",
    );
    out.push_str("/// All Bus message types exported by this workspace.\n");
    out.push_str("pub const HANDLER_NAMES: &[&str] = &[\n");
    for n in handler_names {
        out.push_str(&format!("    \"{n}\",\n"));
    }
    out.push_str("];\n\n");
    out.push_str(
        "/// Register every generated handler name with a host-supplied registrar.\n\
         ///\n\
         /// The host provides the actual dispatch (ports / platform). This module\n\
         /// only owns the name list so trampoline code never hardcodes it.\n\
         pub fn register_all<F>(mut register: F)\n\
         where\n\
             F: FnMut(&'static str),\n\
         {\n\
             for name in HANDLER_NAMES {\n\
                 register(name);\n\
             }\n\
         }\n\n\
         /// Number of handlers in this workspace.\n\
         pub fn handler_count() -> usize {\n\
             HANDLER_NAMES.len()\n\
         }\n",
    );
    out
}
