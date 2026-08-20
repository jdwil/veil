use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::super::*;
use super::*;
use super::super::rust_ir::{
    apply_finish, as_str_owned, await_of, borrow_of, bytes_from_hex_ir, bytes_from_str_ir,
    clone_of, compile_error, field, fn_call, ident, lower_owned, lower_value, map_err_debug,
    map_err_display, map_err_to_string, method, ok_or_not_found, owned_str, parse_i64, ret_err,
    some_of, to_string_of, try_of, utf8_lossy_string, CallFinish, RustExpr, RustType,
};

const OPTION_METHODS: &[&str] = &[
    "is_some", "is_none", "unwrap", "unwrap_or", "unwrap_or_else",
    "unwrap_or_default", "map", "and_then", "or_else", "ok_or",
    "ok_or_else", "as_ref", "as_mut", "take", "replace", "clone",
    "expect", "filter", "flatten", "zip",
];

fn call_recv_lookup_name(call: &CallExpr) -> Option<String> {
    if !call.target.is_empty() {
        return Some(call.target.clone());
    }
    match call.receiver.as_deref() {
        Some(Expr::Ident(n)) => Some(n.clone()),
        Some(Expr::FieldAccess(_, field)) => Some(field.clone()),
        Some(Expr::Call(inner)) => call_recv_lookup_name(inner),
        _ => None,
    }
}

fn finished_method(
    recv: RustExpr,
    name: impl Into<String>,
    args: Vec<RustExpr>,
    finish: CallFinish,
    ctx: &GenCtx,
) -> RustExpr {
    apply_finish(method(recv, name, args), finish, &ctx.error_model)
}

fn now_iso8601() -> RustExpr {
    method(fn_call("Utc::now", vec![]), "to_rfc3339", vec![])
}

fn auto_unwrap_option_recv(recv: &Expr, recv_node: RustExpr, method: &str, ctx: &GenCtx) -> RustExpr {
    let Expr::Ident(name) = recv else {
        return recv_node;
    };
    let Some(ty) = ctx.local_type(name) else {
        return recv_node;
    };
    if !ty.starts_with("Option<") {
        return recv_node;
    }
    let bare = method.trim_end_matches(['!', '?']);
    if !OPTION_METHODS.contains(&bare) {
        return ok_or_not_found(clone_of(recv_node), ctx);
    }
    let non_consuming = ["is_some", "is_none", "as_ref", "as_mut", "clone"];
    if !non_consuming.contains(&bare) {
        clone_of(recv_node)
    } else {
        recv_node
    }
}

fn unwrap_or_string_arg(recv: RustExpr, method: &str, lit: &str, owned_default: bool, finish: CallFinish, ctx: &GenCtx) -> RustExpr {
    let arg = if owned_default {
        owned_str(lit)
    } else {
        RustExpr::StringLit(lit.to_string())
    };
    finished_method(recv, to_snake(method), vec![arg], finish, ctx)
}

fn utf8_lossy_field(field_name: &str) -> RustExpr {
    fn_call(
        "String::from_utf8_lossy",
        vec![borrow_of(field(ident("__out"), field_name))],
    )
}

fn stderr_tail(n: i64) -> RustExpr {
    method(
        method(
            method(
                method(
                    method(method(ident("__err"), "chars", vec![]), "rev", vec![]),
                    "take",
                    vec![RustExpr::IntLit(n)],
                ),
                "collect::<String>",
                vec![],
            ),
            "chars",
            vec![],
        ),
        "rev",
        vec![],
    )
}

fn process_run_ir(prog: RustExpr, args: RustExpr, cwd: RustExpr, hard: bool, ctx: &GenCtx) -> RustExpr {
    let ext = ctx.error_model.external_path();
    let lets = vec![
        RustExpr::Let {
            name: "__prog".to_string(),
            mutable: false,
            ty: Some("String".to_string()),
            value: Box::new(to_string_of(prog)),
        },
        RustExpr::Let {
            name: "__args".to_string(),
            mutable: false,
            ty: Some("String".to_string()),
            value: Box::new(to_string_of(args)),
        },
        RustExpr::Let {
            name: "__cwd".to_string(),
            mutable: false,
            ty: Some("String".to_string()),
            value: Box::new(to_string_of(cwd)),
        },
        RustExpr::Let {
            name: "__argv".to_string(),
            mutable: false,
            ty: Some("Vec<&str>".to_string()),
            value: Box::new(method(
                method(ident("__args"), "split_whitespace", vec![]),
                "collect",
                vec![],
            )),
        },
    ];
    let command = method(
        method(
            method(
                fn_call(
                    "std::process::Command::new",
                    vec![borrow_of(ident("__prog"))],
                ),
                "args",
                vec![borrow_of(ident("__argv"))],
            ),
            "current_dir",
            vec![borrow_of(ident("__cwd"))],
        ),
        "output",
        vec![],
    );
    if hard {
        let mut stmts = lets;
        stmts.push(RustExpr::Let {
            name: "__out".to_string(),
            mutable: false,
            ty: None,
            value: Box::new(map_err_debug(command, ext.clone())),
        });
        stmts.push(RustExpr::If {
            condition: Box::new(RustExpr::UnaryOp {
                op: "!".to_string(),
                expr: Box::new(method(field(ident("__out"), "status"), "success", vec![])),
                ty: Some(RustType::Named("bool".to_string())),
            }),
            then_body: vec![
                RustExpr::Let {
                    name: "__err".to_string(),
                    mutable: false,
                    ty: None,
                    value: Box::new(utf8_lossy_field("stderr")),
                },
                RustExpr::Let {
                    name: "__tail".to_string(),
                    mutable: false,
                    ty: Some("String".to_string()),
                    value: Box::new(method(stderr_tail(2000), "collect", vec![])),
                },
                ret_err(fn_call(
                    ext,
                    vec![RustExpr::Format {
                        template: "{} failed: {}".to_string(),
                        args: vec![ident("__prog"), ident("__tail")],
                    }],
                )),
            ],
            else_body: None,
        });
        return RustExpr::Block {
            stmts,
            value: Some(Box::new(RustExpr::Format {
                template: "{} ok".to_string(),
                args: vec![ident("__prog")],
            })),
        };
    }

    let ok_success = RustExpr::Format {
        template: "{} ok: {}".to_string(),
        args: vec![
            ident("__prog"),
            method(
                method(
                    method(utf8_lossy_field("stdout"), "chars", vec![]),
                    "take",
                    vec![RustExpr::IntLit(400)],
                ),
                "collect::<String>",
                vec![],
            ),
        ],
    };
    let ok_fail = RustExpr::Block {
        stmts: vec![
            RustExpr::Let {
                name: "__err".to_string(),
                mutable: false,
                ty: None,
                value: Box::new(utf8_lossy_field("stderr")),
            },
            RustExpr::Let {
                name: "__tail".to_string(),
                mutable: false,
                ty: Some("String".to_string()),
                value: Box::new(method(stderr_tail(1200), "collect", vec![])),
            },
        ],
        value: Some(Box::new(RustExpr::Format {
            template: "{} failed: {}".to_string(),
            args: vec![ident("__prog"), ident("__tail")],
        })),
    };
    let ok_arm = RustExpr::If {
        condition: Box::new(method(field(ident("__out"), "status"), "success", vec![])),
        then_body: vec![ok_success],
        else_body: Some(vec![ok_fail]),
    };
    RustExpr::Block {
        stmts: lets,
        value: Some(Box::new(RustExpr::Match {
            scrutinee: Box::new(command),
            arms: vec![
                rust_ir::Arm {
                    pattern: "Ok(__out)".to_string(),
                    guard: None,
                    body: vec![ok_arm],
                },
                rust_ir::Arm {
                    pattern: "Err(e)".to_string(),
                    guard: None,
                    body: vec![RustExpr::Format {
                        template: "{} spawn failed: {e}".to_string(),
                        args: vec![ident("__prog")],
                    }],
                },
            ],
        })),
    }
}

/// Translate a Call expression with shape-aware name resolution into structured IR.
pub fn translate_call(call: &CallExpr, ctx: &GenCtx) -> RustExpr {
    let args_ir: Vec<RustExpr> = {
        let recv_owned = call_recv_lookup_name(call);
        let recv = recv_owned.as_deref();
        let tys = param_types_for(recv, &call.method, ctx);
        call.args
            .iter()
            .enumerate()
            .map(|(i, a)| arg_to_ir(a, tys.get(i).map(|s| s.as_str()), ctx))
            .collect()
    };

    // Built-in List methods
    let list_base = if let Some(recv) = &call.receiver {
        Some(lower_value(recv, ctx))
    } else if !call.target.is_empty()
        && !ctx.is_trait_target(&call.target)
        && (call.method == "get"
            || call.method == "len"
            || call.method == "first"
            || call.method == "first!")
        && ctx.local_type(&call.target) != Some("serde_json::Value")
    {
        Some(ident(call.target.clone()))
    } else {
        None
    };
    if let Some(base) = list_base {
        if call.method == "get" && call.args.len() == 1 {
            let is_string_arg = matches!(&call.args[0], Expr::StringLit(_));
            let arg_is_index_like = match &call.args[0] {
                Expr::IntLit(_) => true,
                Expr::Ident(n) => matches!(
                    ctx.local_type(n),
                    Some("i64")
                        | Some("i32")
                        | Some("u64")
                        | Some("u32")
                        | Some("usize")
                        | Some("isize")
                ) || is_copy_local(n, ctx),
                _ => false,
            };
            let base_is_list = if !call.target.is_empty() {
                ctx.local_type(&call.target)
                    .map(|t| t.starts_with("Vec<") || t.starts_with("&[") || t.starts_with("&mut ["))
                    .unwrap_or(false)
            } else if let Some(recv) = &call.receiver {
                if let Expr::Ident(n) = recv.as_ref() {
                    ctx.local_type(n)
                        .map(|t| t.starts_with("Vec<") || t.starts_with("&[") || t.starts_with("&mut ["))
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };
            if !is_string_arg && (arg_is_index_like || base_is_list) {
                let idx = lower_value(&call.args[0], ctx);
                let fallback = Expr::Ident(call.target.clone());
                let recv_expr = call.receiver.as_deref().unwrap_or(&fallback);
                return list_index_get_ir(base, idx, recv_expr, ctx);
            }
        }
        if (call.method == "first" || call.method == "first!") && call.args.is_empty() {
            let fallback = Expr::Ident(call.target.clone());
            let recv_expr = call.receiver.as_deref().unwrap_or(&fallback);
            return list_first_ir(base, recv_expr, ctx);
        }
        if call.method == "len" && call.args.is_empty() {
            return rust_ir::cast(method(base, "len", vec![]), "i64");
        }
    }

    // Chained method call: `<receiver>.method(args)`
    if let Some(recv) = &call.receiver {
        let mut recv_node = lower_value(recv, ctx);
        recv_node = auto_unwrap_option_recv(recv, recv_node, &call.method, ctx);

        let bare_conv = call.method.trim_end_matches(['!', '?']);
        if expr_is_json(recv, ctx)
            && call.args.is_empty()
            && matches!(bare_conv, "as_str" | "as_s" | "to_str" | "to_string")
        {
            return as_str_owned(recv_node);
        }
        if matches!(bare_conv, "to_str" | "as_str" | "to_string") && call.args.is_empty() {
            let recv_is_string =
                matches!(recv.as_ref(), Expr::Ident(n) if ctx.local_type(n) == Some("String"));
            if !recv_is_string {
                return utf8_lossy_string(recv_node);
            }
        }
        if matches!(bare_conv, "as_ref") && call.args.is_empty() && should_decode_as_ref_to_str(recv, ctx)
        {
            return utf8_lossy_string(recv_node);
        }
        if matches!(bare_conv, "as_bytes" | "to_bytes" | "into_bytes") && call.args.is_empty() {
            return method(method(recv_node, "as_ref", vec![]), "to_vec", vec![]);
        }

        if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
            if let Expr::Call(inner_call) = recv.as_ref() {
                let inner_bare = inner_call.method.trim_end_matches(['!', '?']);
                if inner_bare == "as_s"
                    || inner_bare == "as_n"
                    || (inner_bare.starts_with("as_") && inner_bare != "as_str")
                {
                    return recv_node;
                }
                if inner_bare == "get"
                    && inner_call.args.len() == 1
                    && matches!(&inner_call.args[0], Expr::StringLit(_))
                {
                    return recv_node;
                }
            }
            if matches!(&recv_node, RustExpr::Try(_) | RustExpr::MapErr { .. })
                || matches!(&recv_node, RustExpr::MethodCall { method, .. } if method == "unwrap")
            {
                return recv_node;
            }
        }

        if call.method == "get" && call.args.len() == 1
            && let Expr::StringLit(key) = &call.args[0]
        {
            return rust_ir::ok_or_else_missing(
                method(recv_node, "get", vec![RustExpr::StringLit(key.clone())]),
                key,
                ctx,
            );
        }
        if call.method == "as_str" && call.args.is_empty() {
            return as_str_owned(recv_node);
        }
        if call.args.is_empty() && method_bare(&call.method) == "as_n" {
            return parse_i64(
                apply_finish(
                    method(recv_node, "as_n", vec![]),
                    CallFinish::MapErrOwnStr,
                    &ctx.error_model,
                ),
                &ctx.error_model,
            );
        }
        if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
            return parse_i64(recv_node, &ctx.error_model);
        }
        if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
            return try_of(fn_call(
                "serde_json::from_str::<serde_json::Value>",
                vec![borrow_of(recv_node)],
            ));
        }
        if call.args.is_empty() {
            let recv_ty = infer_expr_type(recv, ctx);
            if should_own_str_result(ctx, recv_ty.as_deref(), &call.method) {
                let m = method_bare(&call.method);
                return apply_finish(
                    method(recv_node, m, vec![]),
                    CallFinish::MapErrOwnStr,
                    &ctx.error_model,
                );
            }
        }
        let suffix = receiver_call_finish(recv, &call.method, ctx);
        let m = rust_method_name(&call.method);
        let bare_m = call.method.trim_end_matches(['!', '?']);
        if bare_m == "trim" && call.args.is_empty() {
            return to_string_of(method(recv_node, "trim", vec![]));
        }
        if (bare_m == "unwrap_or" || bare_m == "unwrap_or_else") && call.args.len() == 1
            && let Expr::StringLit(s) = &call.args[0]
        {
            let owned_default = matches!(
                &recv_node,
                RustExpr::MethodCall { method, .. }
                    if method == "to_string"
                        || method == "clone"
                        || method == "map"
                        || method == "and_then"
            ) || matches!(&recv_node, RustExpr::Clone(_));
            return unwrap_or_string_arg(recv_node, &m, s, owned_default, suffix, ctx);
        }
        if bare_m == "body" && call.args.len() == 1
            && let Expr::Ident(name) = &call.args[0]
        {
            let ty = ctx.local_type(name).unwrap_or("");
            if ty == "Vec<u8>" || ty.contains("Bytes") || ty.contains("Vec<u8>") {
                return finished_method(
                    recv_node,
                    "body",
                    vec![method(ident(name.clone()), "into", vec![])],
                    suffix,
                    ctx,
                );
            }
        }
        if bare_m == "limit" && call.args.len() == 1 {
            let arg = lower_value(&call.args[0], ctx);
            let arg = match &arg {
                RustExpr::Cast { ty, .. } if ty == "i32" => arg,
                _ => rust_ir::cast(arg, "i32"),
            };
            return finished_method(recv_node, "limit", vec![arg], suffix, ctx);
        }
        let recv_lookup = match recv.as_ref() {
            Expr::Ident(name) => Some(name.as_str()),
            Expr::FieldAccess(_, field) => Some(field.as_str()),
            _ => None,
        };
        return finished_method(
            recv_node,
            m,
            clone_args_ir(recv_lookup, &call.method, &call.args, ctx),
            suffix,
            ctx,
        );
    }

    // Trait-shaped target
    if ctx.is_trait_target(&call.target) {
        let dep_name = ctx.deps_field_for(&call.target);
        let method_name = if call.method.is_empty() {
            "call"
        } else {
            &call.method
        };
        let final_args = if call.sugar.is_some() {
            vec![match call.args.first() {
                Some(Expr::StructLit(name, fields)) => json_message_ir(name, fields, ctx),
                Some(Expr::Ident(evt)) => RustExpr::JsonMacro {
                    entries: vec![("type".to_string(), RustExpr::StringLit(evt.clone()))],
                },
                _ => json_envelope_ir(&call.target, method_name, &call.args, ctx),
            }]
        } else {
            let method_key = method_name.trim_end_matches(['!', '?']);
            let param_tys = param_types_for(Some(call.target.as_str()), method_key, ctx);
            call.args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let expected = param_tys.get(i).map(|s| s.as_str());
                    match a {
                        Expr::Ident(name) if ctx.local_type(name) == Some("serde_json::Value") => {
                            clone_of(ident(name.clone()))
                        }
                        Expr::Ident(name)
                            if ctx
                                .local_type(name)
                                .map(|t| t.starts_with("Option<"))
                                .unwrap_or(false) =>
                        {
                            let expects_opt = expected
                                .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                                .unwrap_or(false);
                            if expects_opt {
                                clone_of(ident(name.clone()))
                            } else {
                                ok_or_not_found(clone_of(ident(name.clone())), ctx)
                            }
                        }
                        _ => arg_to_ir(a, expected, ctx),
                    }
                })
                .collect()
        };
        // Layer-declared method template (e.g., Bus.dispatch from bus.layer).
        // When a call targets a trait whose method has a lowers_to template,
        // interpolate the template instead of generating a normal method call.
        let method_key = method_name.trim_end_matches(['!', '?']);
        if let Some(targets) = ctx.method_lowers_to.get(&(call.target.clone(), method_key.to_string())) {
            if let Some(template) = targets.get("rust") {
                let dep = dep_name.clone();
                let args_rendered: Vec<String> = final_args.iter()
                    .map(|a| super::super::rust_ir::emit(a))
                    .collect();
                let args_str = args_rendered.join(", ");
                let mut rendered = template.clone();
                rendered = rendered.replace("{dep}", &dep);
                rendered = rendered.replace("{args}", &args_str);
                for (i, arg) in args_rendered.iter().enumerate() {
                    rendered = rendered.replace(&format!("{{arg{i}}}"), arg);
                }
                return RustExpr::LayerTemplate {
                    template: rendered.trim().to_string(),
                    bindings: vec![],
                };
            }
        }
        let has_bang = method_name.ends_with('!');
        let ret_type = ctx.return_type_of(&call.target, method_name).or_else(|| {
            ctx.dep_fields
                .iter()
                .find(|(_, v)| *v == &call.target)
                .and_then(|(trait_name, _)| ctx.return_type_of(trait_name, method_name))
        });
        let is_fallible = if has_bang {
            true
        } else {
            match ret_type {
                Some("bool") | Some("Bool") | Some("i64") | Some("f64") | Some("String")
                | Some("()") | Some("") => false,
                Some(t) if t.starts_with("Option<") || t.starts_with("Opt<") => false,
                _ => true,
            }
        };
        let prefix = if ctx.in_method && ctx.self_fields.contains(&dep_name) {
            field(ident("self"), dep_name)
        } else {
            field(ident("deps"), dep_name)
        };
        return finished_method(
            prefix,
            to_snake(method_key),
            final_args,
            if is_fallible {
                CallFinish::AwaitTry
            } else {
                CallFinish::Await
            },
            ctx,
        );
    }

    if !call.method.is_empty() {
        if let Some(lang) = lang_primitive_call(call, ctx) {
            return lang;
        }
    }

    // Struct-shaped target
    let (module_prefix, effective_target) = if call.target.contains('.') {
        let mut parts = call.target.splitn(2, '.');
        let m = parts.next().unwrap_or("").to_string();
        let t = parts.next().unwrap_or(&call.target).to_string();
        (Some(m), t)
    } else {
        (None, call.target.clone())
    };
    if ctx.is_struct_target(&effective_target)
        || ctx.stubs.stub_type_crate.contains_key(&effective_target)
        || module_prefix.as_ref().map(|m| {
            ctx.stubs
                .stub_type_crate
                .values()
                .any(|(c, _)| c.replace('-', "_") == *m || c.as_str() == m)
        })
        .unwrap_or(false)
    {
        return lower_struct_target_call(call, ctx, module_prefix, effective_target);
    }

    // `local.field.method(args)`
    if call.target.contains('.') && !call.target.starts_with("self.") {
        let first = call.target.split('.').next().unwrap_or("");
        if ctx.is_local(first) {
            let path = lower_dotted_local_path_ir(&call.target, ctx);
            let method_name = rust_method_name(&call.method);
            if call.args.is_empty()
                && matches!(method_bare(&call.method), "as_str" | "as_s" | "to_str" | "to_string")
            {
                return as_str_owned(path);
            }
            if call.args.is_empty() && method_bare(&call.method) == "first" {
                let recv_expr = Expr::Ident(first.to_string());
                return list_first_ir(path, &recv_expr, ctx);
            }
            let suffix = receiver_call_finish(&Expr::Ident(first.to_string()), &call.method, ctx);
            return finished_method(
                path,
                method_name,
                clone_args_for_method(&call.method, &call.args, ctx),
                suffix,
                ctx,
            );
        }
    }

    if ctx.in_method {
        if let Some(node) = lower_self_field_call(call, ctx) {
            return node;
        }
    }

    if ctx.is_local(&call.target) {
        return lower_local_call(call, ctx, args_ir);
    }

    if call.method.is_empty() {
        return lower_bare_call(call, ctx);
    }

    if ctx.is_local(&call.target) || ctx.name_to_shape.contains_key(&call.target) {
        return method(
            ident(call.target.clone()),
            to_snake(&call.method),
            args_ir,
        );
    }

    lower_unknown_target_call(call, ctx, args_ir)
}

fn lang_primitive_call(call: &CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    let lang = match (call.target.as_str(), call.method.as_str()) {
        ("Id", "new") | ("Id", "new_v4") | ("UUID", "new") | ("UUID", "new_v4") | ("Uuid", "new") => {
            Some(fn_call("Uuid::new_v4", vec![]))
        }
        ("Dt", "now") => Some(fn_call("Utc::now", vec![])),
        ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601") => {
            Some(now_iso8601())
        }
        ("Int", "now_unix") | ("Int", "now") => Some(method(fn_call("Utc::now", vec![]), "timestamp", vec![])),
        ("Json", "parse") if call.args.len() == 1 => Some(try_of(fn_call(
            "serde_json::from_str::<serde_json::Value>",
            vec![borrow_of(lower_value(&call.args[0], ctx))],
        ))),
        ("Json", "stringify") if call.args.len() == 1 => Some(try_of(fn_call(
            "serde_json::to_string",
            vec![borrow_of(lower_value(&call.args[0], ctx))],
        ))),
        ("Json", "null") => Some(RustExpr::JsonNull),
        ("Json", "object") => Some(rust_ir::json_object()),
        ("Json", "array") => Some(rust_ir::json_array_new()),
        _ => None,
    };
    if lang.is_some() {
        return lang;
    }

    let lang_leaf = lang_type_leaf(&call.target);
    let is_lang_primitive = matches!(
        lang_leaf,
        "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "Int"
            | "Process" | "Blob" | "Bytes"
    );
    if !(is_lang_primitive || !ctx.is_struct_target(&call.target)) {
        return None;
    }
    let method_key = call.method.trim_end_matches(['!', '?']);
    match (lang_leaf, method_key) {
        ("Dt", "now") => Some(fn_call("Utc::now", vec![])),
        ("Str", "now_iso8601") | ("Dt", "now_iso8601") | ("DateTime", "now_iso8601")
            if call.args.is_empty() =>
        {
            Some(now_iso8601())
        }
        ("Int", "now_unix") | ("Int", "now") if call.args.is_empty() => {
            Some(method(fn_call("Utc::now", vec![]), "timestamp", vec![]))
        }
        ("Uuid", "new_v4") | ("Id", "new_v4") => Some(fn_call("Uuid::new_v4", vec![])),
        ("Map", "new") => Some(fn_call("HashMap::new", vec![])),
        ("List", "new") => Some(fn_call("Vec::new", vec![])),
        ("Opt", "empty") | ("Opt", "none") => Some(ident("None")),
        ("Opt", "some") | ("Opt", "of") if call.args.len() == 1 => {
            Some(some_of(lower_value(&call.args[0], ctx)))
        }
        ("Env", "get_or") if call.args.len() == 2 => {
            let var = lower_value(&call.args[0], ctx);
            let default = match &call.args[1] {
                Expr::StringLit(s) => owned_str(s),
                other => {
                    let d = lower_value(other, ctx);
                    match &d {
                        RustExpr::MethodCall { method, .. } if method == "to_string" => d,
                        _ => to_string_of(d),
                    }
                }
            };
            Some(method(
                fn_call("std::env::var", vec![var]),
                "unwrap_or_else",
                vec![rust_ir::closure(vec!["_".to_string()], default)],
            ))
        }
        ("Env", "get_opt") if call.args.len() == 1 => Some(method(
            fn_call("std::env::var", vec![lower_value(&call.args[0], ctx)]),
            "ok",
            vec![],
        )),
        ("Json", "parse") if call.args.len() == 1 => Some(try_of(fn_call(
            "serde_json::from_str::<serde_json::Value>",
            vec![borrow_of(lower_value(&call.args[0], ctx))],
        ))),
        ("Json", "stringify") if call.args.len() == 1 => Some(try_of(fn_call(
            "serde_json::to_string",
            vec![borrow_of(lower_value(&call.args[0], ctx))],
        ))),
        ("Json", "null") => Some(RustExpr::JsonNull),
        ("Json", "object") => Some(rust_ir::json_object()),
        ("Json", "array") => Some(rust_ir::json_array_new()),
        ("Str", "from_bytes") if call.args.len() == 1 => Some(try_of(fn_call(
            "String::from_utf8",
            vec![lower_value(&call.args[0], ctx)],
        ))),
        ("Process", "run") if call.args.len() == 3 => Some(process_run_ir(
            lower_value(&call.args[0], ctx),
            lower_value(&call.args[1], ctx),
            lower_value(&call.args[2], ctx),
            call.method.ends_with('!'),
            ctx,
        )),
        ("Blob", "new") if call.args.len() == 1 => Some(fn_call(
            format!("{}::new", stub_ctor_path(ctx, &call.target)),
            vec![bytes_from_str_ir(lower_value(&call.args[0], ctx))],
        )),
        ("Bytes", "from_str") | ("Bytes", "new") if call.args.len() == 1 => {
            Some(bytes_from_str_ir(lower_value(&call.args[0], ctx)))
        }
        ("Str", "from_bytes") | ("Str", "from_utf8") if call.args.len() == 1 => {
            Some(to_string_of(fn_call(
                "String::from_utf8_lossy",
                vec![borrow_of(lower_value(&call.args[0], ctx))],
            )))
        }
        ("Blob", "from_hex") if call.args.len() == 1 => Some(fn_call(
            format!("{}::new", stub_ctor_path(ctx, &call.target)),
            vec![bytes_from_hex_ir(lower_value(&call.args[0], ctx))],
        )),
        ("Blob", "from_file") if call.args.len() == 1 => {
            let path = lower_value(&call.args[0], ctx);
            let read = map_err_to_string(
                fn_call(
                    "std::fs::read",
                    vec![method(path, "as_str", vec![])],
                ),
                ctx.error_model.external_path(),
            );
            Some(fn_call(
                format!("{}::new", stub_ctor_path(ctx, &call.target)),
                vec![read],
            ))
        }
        _ => None,
    }
}

fn lower_struct_target_call(
    call: &CallExpr,
    ctx: &GenCtx,
    module_prefix: Option<String>,
    effective_target: String,
) -> RustExpr {
    let method_name = if call.method.is_empty() {
        "new"
    } else {
        &call.method
    };
    let qualified = if let Some(prefix) = &module_prefix {
        let dotted = format!("{prefix}.{effective_target}");
        let colon = format!("{prefix}::{effective_target}");
        if let Some((crate_name, path_type)) = stub_type_parts(ctx, &dotted)
            .or_else(|| stub_type_parts(ctx, &colon))
            .or_else(|| {
                stub_type_parts(ctx, &effective_target).filter(|(c, _)| {
                    c.replace('-', "_") == *prefix || *c == prefix.as_str()
                })
            })
        {
            format!("{crate_name}::{path_type}")
        } else {
            format!("{}::{}", prefix, effective_target)
        }
    } else if let Some((crate_name, original_name)) =
        ctx.stubs.stub_type_crate.get(&effective_target)
    {
        let is_builtin = matches!(
            effective_target.as_str(),
            "String"
                | "Vec"
                | "Option"
                | "Result"
                | "Box"
                | "Arc"
                | "HashMap"
                | "HashSet"
                | "Path"
                | "PathBuf"
                | "Bytes"
                | "Duration"
                | "Instant"
        );
        if is_builtin {
            effective_target.clone()
        } else {
            format!("{}::{}", crate_name, original_name)
        }
    } else {
        effective_target.clone()
    };

    let cloned: Vec<RustExpr> = call.args.iter().map(|a| lower_owned(a, ctx)).collect();

    if method_name == "default" && call.args.is_empty() {
        return fn_call(format!("{qualified}::default"), vec![]);
    }
    if method_name == "new" {
        if let Some(module) = qualified.split("::").next() {
            let is_module_fn = qualified.contains("::")
                && module.chars().next().map(|c| c.is_lowercase()).unwrap_or(false);
            let type_leaf = qualified.split("::").last().unwrap_or("new");
            if is_module_fn && stub_new_is_module_free_fn(ctx, &effective_target, type_leaf) {
                let fn_name = to_snake(type_leaf);
                let raw_args: Vec<RustExpr> = call
                    .args
                    .iter()
                    .map(|a| match a {
                        Expr::StringLit(s) => RustExpr::StringLit(s.clone()),
                        _ => lower_value(a, ctx),
                    })
                    .collect();
                let typed_meta = ctx
                    .stubs
                    .stub_typed_ctors
                    .get(&effective_target)
                    .or_else(|| ctx.stubs.stub_typed_ctors.get(type_leaf));
                let fetch_ret = ctx
                    .types
                    .method_returns
                    .get(&(type_leaf.to_string(), "fetch_optional".into()))
                    .or_else(|| {
                        ctx.types
                            .method_returns
                            .get(&(effective_target.clone(), "fetch_optional".into()))
                    })
                    .map(|s| s.as_str());
                let fetch_is_stringish = fetch_ret.is_some_and(|r| {
                    r.contains("Str")
                        || r.contains("String")
                        || r == "Opt<Str>"
                        || r.starts_with("List<Str")
                });
                let domain_type = if fetch_is_stringish {
                    None
                } else {
                    ctx.expected_return_rust.as_ref().and_then(|ret| {
                        extract_domain_type_from_return(ret, &ctx.name_to_shape)
                    })
                };
                if let Some(domain_type) = domain_type {
                    if let Some((typed_fn, param_tmpl)) = typed_meta {
                        let tparams = expand_typed_type_params(param_tmpl, &domain_type);
                        return fn_call(format!("{module}::{typed_fn}::<{tparams}>"), raw_args);
                    }
                    let typed_struct = format!("{type_leaf}As");
                    let has_sibling = ctx.stubs.stub_type_crate.contains_key(&typed_struct)
                        || ctx.name_to_shape.contains_key(&typed_struct);
                    if has_sibling {
                        return fn_call(
                            format!("{module}::{fn_name}_as::<_, {domain_type}>"),
                            raw_args,
                        );
                    }
                }
                if fetch_is_stringish && type_leaf == "Query" {
                    let sql_is_select = call.args.first().is_some_and(|a| {
                        matches!(a, Expr::StringLit(s) if s.trim_start().to_ascii_lowercase().starts_with("select"))
                    });
                    if sql_is_select {
                        return fn_call(format!("{module}::query_scalar::<_, String>"), raw_args);
                    }
                    return fn_call(format!("{module}::query"), raw_args);
                }
                return fn_call(format!("{module}::{fn_name}"), raw_args);
            }
        }
        let has_id_field = ctx
            .types
            .struct_fields
            .get(&effective_target)
            .map(|fields| fields.iter().any(|(n, _)| n == "id"))
            .unwrap_or(false);
        let mut final_args = cloned.clone();
        if has_id_field {
            let first_is_id = matches!(&call.args.first(), Some(Expr::Ident(n)) if n == "id");
            if !first_is_id {
                final_args.insert(0, fn_call("Uuid::new_v4", vec![]));
            }
        }
        let returns_result = ctx
            .types
            .method_returns
            .get(&(effective_target.clone(), "new".to_string()))
            .map(|t| t.starts_with("Result<"))
            .unwrap_or(false);
        if ctx.defaultable_types.contains(&effective_target) && !call.args.is_empty()
            && let Some(fields) = ctx.types.struct_fields.get(&effective_target)
        {
            let mut field_iter = fields.iter().peekable();
            let mut struct_fields: Vec<(String, RustExpr)> = Vec::new();
            if let Some((fname, fty)) = field_iter.peek()
                && *fname == "id"
                && (*fty == "Uuid" || *fty == "uuid::Uuid")
            {
                struct_fields.push(("id".to_string(), fn_call("Uuid::new_v4", vec![])));
                field_iter.next();
            }
            for arg in &call.args {
                if let Some((fname, _)) = field_iter.next() {
                    struct_fields.push((to_snake(fname), lower_value(arg, ctx)));
                }
            }
            return RustExpr::StructLit {
                name: qualified.clone(),
                fields: struct_fields,
                rest: Some(Box::new(fn_call(format!("{qualified}::default"), vec![]))),
                ty: None,
            };
        }
        let ctor = fn_call(format!("{}::{}", qualified, to_snake(method_name)), final_args);
        return if returns_result { try_of(ctor) } else { ctor };
    }

    let method_bare_s = method_name.trim_end_matches(['!', '?']);
    if (effective_target == "Blob" || effective_target.ends_with("Blob"))
        && method_bare_s == "from_hex"
        && call.args.len() == 1
    {
        return fn_call(
            format!("{}::new", stub_ctor_path(ctx, "Blob")),
            vec![bytes_from_hex_ir(lower_value(&call.args[0], ctx))],
        );
    }
    if (effective_target == "Blob" || effective_target.ends_with("Blob"))
        && method_bare_s == "from_file"
        && call.args.len() == 1
    {
        let path = lower_value(&call.args[0], ctx);
        let read = map_err_to_string(
            fn_call("std::fs::read", vec![method(path, "as_str", vec![])]),
            ctx.error_model.external_path(),
        );
        return fn_call(format!("{}::new", stub_ctor_path(ctx, "Blob")), vec![read]);
    }
    if effective_target == "Process" && method_bare_s == "run" && call.args.len() == 3 {
        return process_run_ir(
            lower_value(&call.args[0], ctx),
            lower_value(&call.args[1], ctx),
            lower_value(&call.args[2], ctx),
            call.method.ends_with('!'),
            ctx,
        );
    }
    let is_pascal_ctor = method_name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false);
    if !is_pascal_ctor && !call.args.is_empty()
        && let Expr::Ident(first_arg) = &call.args[0]
        && first_arg.eq_ignore_ascii_case(&effective_target)
    {
        let rest: Vec<RustExpr> = call.args[1..].iter().map(|a| lower_value(a, ctx)).collect();
        return method(ident(first_arg.clone()), to_snake(method_name), rest);
    }
    if ctx.name_to_shape.get(effective_target.as_str()) == Some(&Shape::Enum) || is_pascal_ctor {
        let m = rust_method_name(method_name);
        return fn_call(format!("{qualified}::{m}"), cloned);
    }
    let m = rust_method_name(method_name);
    let suffix = receiver_call_finish(&Expr::Ident(effective_target.clone()), method_name, ctx);
    apply_finish(
        fn_call(format!("{qualified}::{m}"), cloned),
        suffix,
        &ctx.error_model,
    )
}

fn lower_self_field_call(call: &CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    let field_name = call
        .target
        .strip_prefix("self.")
        .unwrap_or(call.target.as_str());
    if !(ctx.self_fields.contains(field_name) || call.target.starts_with("self.")) {
        return None;
    }
    let self_field = field(ident("self"), to_snake(field_name));
    if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
        return Some(parse_i64(self_field, &ctx.error_model));
    }
    if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
        return Some(try_of(fn_call(
            "serde_json::from_str::<serde_json::Value>",
            vec![borrow_of(self_field)],
        )));
    }
    let method_name = rust_method_name(&call.method);
    let suffix = receiver_call_finish(&Expr::Ident(field_name.to_string()), &call.method, ctx);
    let field_type = ctx
        .self_field_types
        .get(field_name)
        .or_else(|| ctx.self_field_types.get(&to_snake(field_name)));
    let is_map_field = field_type
        .map(|t| t.contains("HashMap") || t.starts_with("std::collections::HashMap"))
        .unwrap_or(false);
    if is_map_field {
        let bare_method = call.method.trim_end_matches(['!', '?']);
        let lock_read = method(self_field.clone(), "read", vec![]);
        let lock_read = await_of(lock_read);
        let lock_write = await_of(method(self_field, "write", vec![]));
        return Some(match bare_method {
            "get" | "contains_key" => {
                let key = if !call.args.is_empty() {
                    vec![borrow_of(lower_value(&call.args[0], ctx))]
                } else {
                    vec![]
                };
                let call_n = method(lock_read, method_name, key);
                if bare_method == "get" {
                    method(call_n, "cloned", vec![])
                } else {
                    call_n
                }
            }
            "insert" => {
                let map_args: Vec<RustExpr> = call
                    .args
                    .iter()
                    .map(|a| match a {
                        Expr::Ident(_) | Expr::FieldAccess(_, _) => clone_of(lower_value(a, ctx)),
                        _ => lower_value(a, ctx),
                    })
                    .collect();
                method(lock_write, "insert", map_args)
            }
            "remove" => {
                let key = if !call.args.is_empty() {
                    vec![borrow_of(lower_value(&call.args[0], ctx))]
                } else {
                    vec![]
                };
                method(lock_write, "remove", key)
            }
            "values" | "keys" | "iter" | "len" | "is_empty" => method(
                lock_read,
                method_name,
                clone_args_for_method(&call.method, &call.args, ctx),
            ),
            _ => method(
                lock_write,
                method_name,
                clone_args_for_method(&call.method, &call.args, ctx),
            ),
        });
    }
    Some(finished_method(
        self_field,
        method_name,
        clone_args_for_method(&call.method, &call.args, ctx),
        suffix,
        ctx,
    ))
}

fn lower_local_call(call: &CallExpr, ctx: &GenCtx, _args_ir: Vec<RustExpr>) -> RustExpr {
    let method_name = rust_method_name(&call.method);
    let target = ident(call.target.clone());
    if call.args.is_empty()
        && matches!(
            call.method.trim_end_matches(['!', '?']),
            "to_str" | "as_str"
        )
    {
        let ty = ctx.local_type(&call.target).unwrap_or("");
        if ty != "String" {
            return utf8_lossy_string(target);
        }
    }
    if call.args.is_empty() && method_bare(&call.method) == "as_ref" {
        let recv_ident = Expr::Ident(call.target.clone());
        if should_decode_as_ref_to_str(&recv_ident, ctx) {
            return utf8_lossy_string(target);
        }
    }
    if call.args.is_empty() && method_bare(&call.method) == "parse_int" {
        return parse_i64(target, &ctx.error_model);
    }
    if call.args.is_empty() && method_bare(&call.method) == "parse_json" {
        return try_of(fn_call(
            "serde_json::from_str::<serde_json::Value>",
            vec![borrow_of(target)],
        ));
    }
    if call.args.is_empty() && method_bare(&call.method) == "as_n" {
        return parse_i64(
            apply_finish(
                method(target, "as_n", vec![]),
                CallFinish::MapErrOwnStr,
                &ctx.error_model,
            ),
            &ctx.error_model,
        );
    }
    if call.method == "get" && call.args.len() == 1
        && let Expr::StringLit(key) = &call.args[0]
    {
        return rust_ir::ok_or_else_missing(
            method(target, "get", vec![RustExpr::StringLit(key.clone())]),
            key,
            ctx,
        );
    }
    if (call.method == "unwrap" || call.method == "unwrap!") && call.args.is_empty() {
        let ty = ctx.local_type(&call.target);
        if ty.map(|t| t.starts_with("Result<")).unwrap_or(false) {
            return map_err_display(target, ctx.error_model.external_path());
        }
        let is_option = ty.map(|t| t.starts_with("Option<")).unwrap_or(true);
        if is_option {
            let enclosing_returns_option = ctx
                .expected_return_rust
                .as_ref()
                .map(|r| r.starts_with("Option<"))
                .unwrap_or(false);
            if enclosing_returns_option {
                return try_of(clone_of(target));
            }
            return ok_or_not_found(clone_of(target), ctx);
        }
        return target;
    }
    if call.method == "ok_or" && ctx.is_local(&call.target) {
        let is_option = ctx
            .local_type(&call.target)
            .map(|t| t.starts_with("Option<"))
            .unwrap_or(true);
        if !is_option {
            return target;
        }
    }
    if let Some(type_name) = ctx.local_type(&call.target) {
        if type_name == "serde_json::Value" {
            match call.method.as_str() {
                "len" => {
                    return method(
                        method(method(target, "as_array", vec![]), "map", vec![
                            rust_ir::closure(
                                vec!["a".to_string()],
                                rust_ir::cast(method(ident("a"), "len", vec![]), "i64"),
                            ),
                        ]),
                        "unwrap_or",
                        vec![RustExpr::IntLit(0)],
                    );
                }
                "is_empty" => {
                    return method(
                        method(method(target, "as_array", vec![]), "map", vec![
                            rust_ir::closure(
                                vec!["a".to_string()],
                                method(ident("a"), "is_empty", vec![]),
                            ),
                        ]),
                        "unwrap_or",
                        vec![RustExpr::BoolLit(true)],
                    );
                }
                "to_string" | "to_str" => {
                    return to_string_of(method(
                        method(target, "as_str", vec![]),
                        "unwrap_or",
                        vec![RustExpr::StringLit(String::new())],
                    ));
                }
                _ => {}
            }
        }
        if ctx.name_to_shape.get(type_name) == Some(&Shape::Trait) {
            let bare_ty = peel_dyn_trait_name(type_name).unwrap_or_else(|| type_name.to_string());
            let fallible = call.method.ends_with('!')
                || ctx
                    .stubs
                    .type_fallible_methods
                    .contains(&(bare_ty, method_name.clone()))
                || ctx
                    .stubs
                    .type_fallible_methods
                    .contains(&(type_name.to_string(), method_name.clone()));
            return finished_method(
                target,
                method_name,
                {
                    let recv_owned = call_recv_lookup_name(call);
                    clone_args_ir(recv_owned.as_deref(), &call.method, &call.args, ctx)
                },
                if fallible {
                    CallFinish::AwaitTry
                } else {
                    CallFinish::Await
                },
                ctx,
            );
        }
        if type_name.starts_with("Option<") {
            let bare_method = call.method.trim_end_matches(['!', '?']);
            if !OPTION_METHODS.contains(&bare_method) {
                return method(
                    ok_or_not_found(clone_of(target), ctx),
                    method_name,
                    clone_args_for_method(&call.method, &call.args, ctx),
                );
            }
            let non_consuming = ["is_some", "is_none", "as_ref", "as_mut", "clone"];
            if !non_consuming.contains(&bare_method) {
                let suffix =
                    receiver_call_finish(&Expr::Ident(call.target.clone()), &call.method, ctx);
                if (bare_method == "unwrap_or" || bare_method == "unwrap_or_else")
                    && call.args.len() == 1
                    && let Expr::StringLit(s) = &call.args[0]
                {
                    return unwrap_or_string_arg(clone_of(target), &method_name, s, true, suffix, ctx);
                }
                return finished_method(
                    clone_of(target),
                    method_name,
                    clone_args_for_method(&call.method, &call.args, ctx),
                    suffix,
                    ctx,
                );
            }
        }
        if ctx
            .types
            .method_returns
            .contains_key(&(type_name.to_string(), call.method.clone()))
            || ctx.types.method_returns.contains_key(&(
                type_name.to_string(),
                call.method.trim_end_matches(['!', '?']).to_string(),
            ))
        {
            let suffix =
                receiver_call_finish(&Expr::Ident(call.target.clone()), &call.method, ctx);
            return finished_method(
                target,
                method_name,
                clone_args_ir(Some(type_name), &call.method, &call.args, ctx),
                suffix,
                ctx,
            );
        }
    }
    if call.args.is_empty() {
        let recv_ty = ctx.local_type(&call.target).map(|s| s.to_string());
        if should_own_str_result(ctx, recv_ty.as_deref(), &call.method) {
            return apply_finish(
                method(target, method_bare(&call.method), vec![]),
                CallFinish::MapErrOwnStr,
                &ctx.error_model,
            );
        }
    }
    if call.method == "as_str" && call.args.is_empty() {
        return as_str_owned(target);
    }
    let iter_methods = [
        "any", "all", "find", "filter", "map", "for_each", "count", "flat_map",
    ];
    if iter_methods.contains(&method_name.as_str()) {
        return method(
            method(target, "iter", vec![]),
            method_name,
            clone_args_for_method(&call.method, &call.args, ctx),
        );
    }
    let suffix = receiver_call_finish(&Expr::Ident(call.target.clone()), &call.method, ctx);
    let bare_m = call.method.trim_end_matches(['!', '?']);
    if (bare_m == "unwrap_or" || bare_m == "unwrap_or_else") && call.args.len() == 1
        && let Expr::StringLit(s) = &call.args[0]
    {
        return unwrap_or_string_arg(target, &method_name, s, true, suffix, ctx);
    }
    if (bare_m == "push" || bare_m == "insert" || bare_m == "extend") && !call.args.is_empty()
        && let Some(Expr::Ident(arg_name)) = call.args.first()
        && let Some(ty) = ctx.local_type(arg_name)
        && ty.starts_with("Option<")
    {
        let mut args = vec![ok_or_not_found(clone_of(ident(arg_name.clone())), ctx)];
        if call.args.len() > 1 {
            args.extend(clone_args_for_method(&call.method, &call.args[1..], ctx));
        }
        return finished_method(target, method_name, args, suffix, ctx);
    }
    let target_type: Option<&str> = ctx.local_type(&call.target);
    finished_method(
        target,
        method_name,
        clone_args_ir(target_type, &call.method, &call.args, ctx),
        suffix,
        ctx,
    )
}

fn lower_bare_call(call: &CallExpr, ctx: &GenCtx) -> RustExpr {
    let bare_target = call.target.trim_end_matches(['!', '?']);
    match bare_target {
        "now" => fn_call("Utc::now", vec![]),
        "drop" => fn_call(
            "drop",
            call.args.iter().map(|a| lower_value(a, ctx)).collect(),
        ),
        _ => {
            let dep_method_match = ctx.dep_fields.iter().find_map(|(trait_name, field_name)| {
                let key = (trait_name.clone(), bare_target.to_string());
                if ctx.types.method_returns.contains_key(&key) {
                    return Some(field_name.clone());
                }
                let key2 = (field_name.clone(), bare_target.to_string());
                if ctx.types.method_returns.contains_key(&key2) {
                    return Some(field_name.clone());
                }
                if bare_target.starts_with(field_name.as_str())
                    && (bare_target.len() == field_name.len()
                        || bare_target.as_bytes().get(field_name.len()) == Some(&b'_')
                        || bare_target[field_name.len()..]
                            .chars()
                            .next()
                            .map(|c| c.is_ascii_lowercase())
                            .unwrap_or(false))
                {
                    return Some(field_name.clone());
                }
                None
            });
            if let Some(dep_field) = dep_method_match {
                return finished_method(
                    field(ident("deps"), dep_field),
                    to_snake(bare_target),
                    clone_args(&call.args, ctx),
                    CallFinish::AwaitTry,
                    ctx,
                );
            }
            let base = fn_call(to_snake(bare_target), clone_args(&call.args, ctx));
            let is_bang = call.target.ends_with('!');
            if ctx.async_fns.contains(bare_target) || ctx.async_fns.contains(&call.target) {
                try_of(await_of(base))
            } else if is_bang {
                try_of(base)
            } else {
                base
            }
        }
    }
}

fn lower_unknown_target_call(
    call: &CallExpr,
    ctx: &GenCtx,
    args_ir: Vec<RustExpr>,
) -> RustExpr {
    if call.target.contains('.') && !call.target.starts_with("self.") {
        let parts: Vec<&str> = call.target.split('.').collect();
        let struct_name = parts.last().unwrap_or(&"");
        let qualified = if let Some((crate_name, original_name)) =
            ctx.stubs.stub_type_crate.get(*struct_name).or_else(|| {
                ctx.stubs
                    .stub_type_crate
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(struct_name))
                    .map(|(_, v)| v)
            }) {
            format!("{}::{}", crate_name, original_name)
        } else {
            (*struct_name).to_string()
        };
        let m = rust_method_name(&call.method);
        let bare = call.method.trim_end_matches(['!', '?']);
        let suffix = if ctx.stubs.async_fallible_methods.contains(bare) {
            CallFinish::AwaitMapErr
        } else if ctx.stubs.fallible_methods.contains(bare) {
            CallFinish::Try
        } else {
            CallFinish::Bare
        };
        return apply_finish(fn_call(format!("{qualified}::{m}"), args_ir), suffix, &ctx.error_model);
    }
    let target_snake = to_snake(&call.target);
    if ctx.known_modules.contains(&target_snake) {
        let m = to_snake(&call.method);
        let suffix = if ctx.stubs.fallible_methods.contains(&call.method)
            || call.method == "from_str"
            || call.method == "to_string"
            || call.method == "parse"
        {
            CallFinish::Try
        } else {
            CallFinish::Bare
        };
        let needs_ref = m == "from_str" || m == "to_string" || m == "to_vec";
        let final_args = if needs_ref && call.args.len() == 1 {
            vec![borrow_of(lower_value(&call.args[0], ctx))]
        } else {
            args_ir
        };
        if target_snake == "serde_json" && m == "from_str"
            && let Some(ty) = from_str_turbofish_type(ctx)
        {
            return apply_finish(
                fn_call(format!("serde_json::from_str::<{ty}>"), final_args),
                suffix,
                &ctx.error_model,
            );
        }
        return apply_finish(
            fn_call(format!("{target_snake}::{m}"), final_args),
            suffix,
            &ctx.error_model,
        );
    }
    if let Some(rust_crate) = ctx
        .stubs
        .stub_pkg_crate
        .get(&call.target)
        .or_else(|| ctx.stubs.stub_pkg_crate.get(&target_snake))
    {
        let bare = call.method.trim_end_matches(['!', '?']);
        if let Some(&fallible) = ctx
            .stubs
            .stub_free_fns
            .get(&(rust_crate.clone(), bare.to_string()))
        {
            let m = to_snake(bare);
            let final_args: Vec<RustExpr> = call
                .args
                .iter()
                .map(|a| {
                    let node = lower_value(a, ctx);
                    match a {
                        Expr::StringLit(_) | Expr::Ident(_) | Expr::FieldAccess(_, _) => {
                            borrow_of(node)
                        }
                        _ if matches!(node, RustExpr::Borrow { .. }) => node,
                        _ => borrow_of(node),
                    }
                })
                .collect();
            let call_n = fn_call(format!("{rust_crate}::{m}"), final_args);
            return if fallible {
                map_err_to_string(call_n, ctx.error_model.external_path())
            } else {
                call_n
            };
        }
    }
    let m_clean = call.method.trim_end_matches(['!', '?']);
    let target_is_var_like = call
        .target
        .chars()
        .next()
        .map(|c| c.is_lowercase())
        .unwrap_or(false)
        && !call.target.contains('.');
    if target_is_var_like {
        let target = ident(call.target.clone());
        if m_clean == "get" && call.args.len() == 1
            && let Expr::StringLit(key) = &call.args[0]
        {
            return rust_ir::ok_or_else_missing(
                method(target, "get", vec![RustExpr::StringLit(key.clone())]),
                key,
                ctx,
            );
        }
        if (m_clean == "unwrap" || m_clean == "unwrap!") && call.args.is_empty() {
            return target;
        }
        if call.args.is_empty() && m_clean == "as_n" {
            return parse_i64(
                apply_finish(
                    method(target, "as_n", vec![]),
                    CallFinish::MapErrOwnStr,
                    &ctx.error_model,
                ),
                &ctx.error_model,
            );
        }
        if call.args.is_empty() && m_clean == "parse_int" {
            return parse_i64(target, &ctx.error_model);
        }
        if call.args.is_empty() && m_clean == "parse_json" {
            return try_of(fn_call(
                "serde_json::from_str::<serde_json::Value>",
                vec![borrow_of(target)],
            ));
        }
        if call.args.is_empty()
            && should_own_str_result(ctx, ctx.local_type(&call.target), &call.method)
        {
            return apply_finish(
                method(target, m_clean, vec![]),
                CallFinish::MapErrOwnStr,
                &ctx.error_model,
            );
        }
    }
    compile_error(format!(
        "unstubbed external `{}.{}` — install a .stub and call its types (@field + stub methods)",
        call.target, m_clean
    ))
}
