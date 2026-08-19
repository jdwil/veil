use veil_ir::ast::*;
use veil_ir::layer::StmtShape;
use crate::rust::to_snake;
use super::*;

/// Classify a `guard` failure message → DomainError variant.
/// Real input validation stays Validation (400).
pub fn guard_error_variant(msg: &str) -> &'static str {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("not found")
        || lower.contains("cross-tenant")
        || lower.contains("access denied")
        || lower.contains("forbidden")
        || lower.contains("unauthorized")
        || (lower.contains("denied") && !lower.contains("validation"))
    {
        "NotFound"
    } else {
        "Validation"
    }
}

/// Interpolate a statement `lowers_to` template for the given target.
///
/// Variables: `{args}`, `{argN}`, `{dep}`, `{self}`, `{named.key}`, `{body}`.
pub fn interpolate_action_template(
    template: &str,
    a: &ActionExpr,
    ctx: &GenCtx,
    translate_expr: &dyn Fn(&Expr, &GenCtx) -> String,
) -> String {
    let mut result = template.to_string();

    let args_str = if !a.named_args.is_empty() {
        // Prefer a single struct-like arg when named fields were used as payload.
        let fields = a
            .named_args
            .iter()
            .map(|(k, v)| format!("{}: {}", k, translate_expr(v, ctx)))
            .collect::<Vec<_>>()
            .join(", ");
        if a.target.is_empty() {
            format!("{{ {} }}", fields)
        } else {
            format!("{} {{ {} }}", a.target, fields)
        }
    } else if !a.args.is_empty() {
        a.args
            .iter()
            .map(|e| translate_expr(e, ctx))
            .collect::<Vec<_>>()
            .join(", ")
    } else if !a.target.is_empty() {
        a.target.clone()
    } else {
        String::new()
    };
    result = result.replace("{args}", &args_str);

    for (i, arg) in a.args.iter().enumerate() {
        let rendered = translate_expr(arg, ctx);
        result = result.replace(&format!("{{arg{i}}}"), &rendered);
    }
    // Also expose named-args as arg indices after positionals.
    for (i, (_k, v)) in a.named_args.iter().enumerate() {
        let idx = a.args.len() + i;
        let rendered = translate_expr(v, ctx);
        result = result.replace(&format!("{{arg{idx}}}"), &rendered);
    }

    if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
        if let Some(dep_type) = &spec.requires_dep {
            let dep_field = ctx.deps_field_for(dep_type);
            result = result.replace("{dep}", &dep_field);
        } else if let Some(port) = &spec.port_target {
            let dep_field = ctx.deps_field_for(port);
            result = result.replace("{dep}", &dep_field);
        }
    }
    // Bare `{dep}` left unresolved → snake of keyword (last resort).
    if result.contains("{dep}") {
        result = result.replace("{dep}", &to_snake(&a.keyword));
    }

    result = result.replace("{self}", "self");

    for (key, val) in &a.named_args {
        let rendered = translate_expr(val, ctx);
        result = result.replace(&format!("{{named.{key}}}"), &rendered);
    }

    if result.contains("{body}") {
        let body_str = a
            .body
            .iter()
            .map(|e| translate_expr(e, ctx))
            .collect::<Vec<_>>()
            .join("; ");
        result = result.replace("{body}", &body_str);
    }

    // Condition/message helpers for If-shaped statements with templates.
    if let Some(cond) = a.condition.as_deref() {
        result = result.replace("{condition}", &translate_expr(cond, ctx));
    }
    if let Some(msg) = &a.message {
        let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
        result = result.replace("{message}", &format!("\"{escaped}\""));
    }

    if let Some(binding) = &a.result_binding {
        format!("let {binding} = {result}")
    } else {
        result
    }
}

/// Translate a layer-defined Action that was NOT desugared (e.g. emit, guard).
pub fn translate_action(a: &ActionExpr, ctx: &GenCtx) -> String {
    // Prefer explicit per-target lowering templates from the layer.
    if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
        if let Some(template) = spec.lowers_to.get("rust") {
            return interpolate_action_template(template, a, ctx, &expr_to_rust);
        }
        // Port.method fallback when Action was kept (e.g. has lowers_to for other
        // targets only) — emit a deps call mirroring the desugared path.
        if let (Some(port), Some(method)) = (&spec.port_target, &spec.port_method) {
            let dep = ctx.deps_field_for(port);
            let rref = if ctx.routing_traits.contains(port) {
                if ctx.routing_ref.is_empty() {
                    format!("deps.{}", dep)
                } else {
                    ctx.routing_ref.clone()
                }
            } else if ctx.in_method {
                format!("self.{}", dep)
            } else {
                format!("deps.{}", dep)
            };
            let args_str = if !a.named_args.is_empty() {
                let fields = a
                    .named_args
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} {{ {} }}", a.target, fields)
            } else if !a.args.is_empty() {
                a.args
                    .iter()
                    .map(|e| expr_to_rust(e, ctx))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else if !a.target.is_empty() {
                a.target.clone()
            } else {
                String::new()
            };
            let call = format!("{rref}.{}({args_str}).await?", to_snake(method));
            return if let Some(binding) = &a.result_binding {
                format!("let {binding} = {call}")
            } else {
                call
            };
        }
    }

    match a.shape {
        StmtShape::If => {
            // guard: the condition must hold for the flow to continue.
            let msg = a.message.as_deref().unwrap_or("precondition failed");
            let msg_escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
            let err_var = guard_error_variant(msg);
            match a.condition.as_deref() {
                // Fallible-call guard (`guard call X.method(...)`): the call
                // returns a Result that must be Ok — map_err with policy variant.
                Some(cond @ Expr::Call(c))
                    if !c.method.is_empty()
                        && (ctx.name_to_shape.contains_key(&c.target)
                            || ctx.fallible_methods.contains(&c.method)
                            || c.method == "validate") =>
                {
                    let call_str = expr_to_rust(cond, ctx);
                    // translate_call may already append `?`; strip it so our
                    // map_err drives the propagation.
                    let base = call_str
                        .strip_suffix(".await?")
                        .or_else(|| call_str.strip_suffix('?'))
                        .unwrap_or(&call_str);
                    if err_var == "NotFound" {
                        format!("{base}.map_err(|_| {})?", ctx.error_model.not_found_path())
                    } else {
                        format!(
                            "{base}.map_err(|_| {}(\"{msg_escaped}\".to_string()))?",
                            ctx.error_model.validation_path()
                        )
                    }
                }
                Some(cond @ Expr::Await(_)) => {
                    let call_str = expr_to_rust(cond, ctx);
                    let base = call_str.strip_suffix('?').unwrap_or(&call_str);
                    if err_var == "NotFound" {
                        format!("{base}.map_err(|_| {})?", ctx.error_model.not_found_path())
                    } else {
                        format!(
                            "{base}.map_err(|_| {}(\"{msg_escaped}\".to_string()))?",
                            ctx.error_model.validation_path()
                        )
                    }
                }
                // Boolean guard: the condition must evaluate to true.
                Some(cond) => {
                    let cond_str = expr_to_rust(cond, ctx);
                    // Suppress redundant `.is_some()` guards only when we *know*
                    // the local is not Option (e.g. after explicit force-present / require).
                    // Portable bang (ACS-010) does NOT auto-ok_or on find! — Opt stays Opt.
                    if let Expr::Call(c) = cond {
                        if c.method == "is_some" && ctx.locals.contains(&c.target) {
                            let var_type = ctx.local_types.get(&c.target);
                            let is_option = var_type
                                .map(|t| t.starts_with("Option<") || t == "Option")
                                .unwrap_or(true); // unknown → keep guard
                            if !is_option {
                                return format!(
                                    "/* guard {:?} — local is not Option (already forced present) */",
                                    msg_escaped
                                );
                            }
                            // is_none → NotFound when message is resource-missing
                            let err = if err_var == "NotFound" {
                                ctx.error_model.not_found_path()
                            } else {
                                format!("{}(\"{msg_escaped}\".to_string())", ctx.error_model.validation_path())
                            };
                            return format!(
                                "if {}.is_none() {{ return Err({err}); }}",
                                c.target
                            );
                        }
                    }
                    let err = if err_var == "NotFound" {
                        ctx.error_model.not_found_path()
                    } else {
                        format!("{}(\"{msg_escaped}\".to_string())", ctx.error_model.validation_path())
                    };
                    format!(
                        "if !({}) {{ return Err({err}); }}",
                        cond_str
                    )
                }
                None => format!("/* guard: {} (no condition) */", msg_escaped),
            }
        }
        StmtShape::Call | StmtShape::Assign | StmtShape::Infix | StmtShape::Block => {
            // Remaining actions (emit) — handle based on keyword-like semantics.
            // For now, emit as a comment + placeholder.
            let args_str = if !a.named_args.is_empty() {
                let fields = a.named_args.iter()
                    .map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx)))
                    .collect::<Vec<_>>().join(", ");
                format!("{} {{ {} }}", a.target, fields)
            } else if !a.args.is_empty() {
                a.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ")
            } else {
                a.target.clone()
            };
            let core = format!("/* {} {} */", a.keyword, args_str);
            if let Some(binding) = &a.result_binding {
                format!("let {binding} = {core}")
            } else {
                core
            }
        }
    }
}

/// Translate a full statement (expression at statement position) with semicolons.
pub fn stmt_to_rust(expr: &Expr, ctx: &mut GenCtx) -> String {
    match expr {
        Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) => {
            let s = expr_to_rust(expr, ctx);
            // Field assigns (`wt.name = x`) are not new locals.
            if !name.contains('.') {
                ctx.locals.insert(name.clone());
                // Prefer explicit type annotation (`mut x: Json = …`) so field
                // access on serde_json::Value lowers to indexing.
                if let Some(ty) = ty_ann {
                    ctx.local_types
                        .insert(name.clone(), crate::rust::type_to_rust(ty));
                } else if let Some(t) = infer_expr_type(rhs, ctx) {
                    ctx.local_types.insert(name.clone(), t);
                }
            }
            format!("    {};", s)
        }
        Expr::Action(a) if a.result_binding.is_some() => {
            let name = a.result_binding.as_ref().unwrap().clone();
            ctx.locals.insert(name.clone());
            if let Some(t) = infer_expr_type(expr, ctx) {
                ctx.local_types.insert(name, t);
            }
            format!("    {};", expr_to_rust(expr, ctx))
        }
        _ => {
            let rust = expr_to_rust(expr, ctx);
            if is_rust_block_stmt(expr) {
                indent_lines(&rust, "    ")
            } else {
                format!("    {};", rust.trim_end_matches(';'))
            }
        }
    }
}
