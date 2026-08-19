//! Basic expression type checking (CHK-004).
//!
//! MVP checks when types are known:
//! - assignment / annotated `mut` compatibility
//! - call argument counts and types vs method/fn params
//! - `?` (try) requires a fallible (`Res!` / `Res!<T>`) value
//! - `await` flags obviously non-async values (scalars) as warnings
//! - match arms vs enum variants when the scrutinee type is a known enum
//! - bare field names: conventional inference; report when still unknown
//!
//! Unknown types are not errors (avoid false positives until inference grows).
//! Limitations are encoded as diagnostic codes and hints.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostics::{Diagnostic, Severity};
use crate::layer::{LayerRegistry, Shape};
use crate::span::Span;

// ─── Type representation ─────────────────────────────────────────────────────

/// Simplified type for checking. Unknown is compatible with everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// Named type after normalization (Str, Int, User, …)
    Named(String),
    Opt(Box<Ty>),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    /// Fallible: Res! (unit) or Res!<T>
    Res(Option<Box<Ty>>),
    Tuple(Vec<Ty>),
    /// Unit / void / ()
    Unit,
    /// Not enough information
    Unknown,
}

impl Ty {
    fn display(&self) -> String {
        match self {
            Ty::Named(n) => n.clone(),
            Ty::Opt(t) => format!("Opt<{}>", t.display()),
            Ty::List(t) => format!("List<{}>", t.display()),
            Ty::Map(k, v) => format!("Map<{}, {}>", k.display(), v.display()),
            Ty::Set(t) => format!("Set<{}>", t.display()),
            Ty::Res(None) => "Res!".into(),
            Ty::Res(Some(t)) => format!("Res!<{}>", t.display()),
            Ty::Tuple(ts) => {
                let inner = ts.iter().map(|t| t.display()).collect::<Vec<_>>().join(", ");
                format!("({})", inner)
            }
            Ty::Unit => "()".into(),
            Ty::Unknown => "?".into(),
        }
    }

    fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    fn is_res(&self) -> bool {
        matches!(self, Ty::Res(_))
    }

    fn is_scalar(&self) -> bool {
        matches!(
            self,
            Ty::Named(n) if matches!(
                n.as_str(),
                "Str" | "String" | "Int" | "F64" | "Bool" | "Bytes" | "UUID" | "Id" | "DateTime" | "Dt" | "Json"
            )
        )
    }
}

/// Convert AST type expr → Ty.
fn ty_from_type_expr(te: &TypeExpr) -> Ty {
    match te {
        TypeExpr::Named(n) if n.is_empty() || n == "Unknown" || n == "_" => Ty::Unknown,
        TypeExpr::Named(n) => Ty::Named(normalize_type_name(n)),
        TypeExpr::Generic(name, args) => match name.as_str() {
            "Opt" | "Option" => Ty::Opt(Box::new(
                args.first().map(ty_from_type_expr).unwrap_or(Ty::Unknown),
            )),
            "List" | "Vec" => Ty::List(Box::new(
                args.first().map(ty_from_type_expr).unwrap_or(Ty::Unknown),
            )),
            "Set" | "HashSet" => Ty::Set(Box::new(
                args.first().map(ty_from_type_expr).unwrap_or(Ty::Unknown),
            )),
            "Map" | "HashMap" => Ty::Map(
                Box::new(args.first().map(ty_from_type_expr).unwrap_or(Ty::Unknown)),
                Box::new(args.get(1).map(ty_from_type_expr).unwrap_or(Ty::Unknown)),
            ),
            "Res" | "Result" => Ty::Res(args.first().map(|a| Box::new(ty_from_type_expr(a)))),
            other => {
                // User generic Type<A,B>
                let _ = other;
                Ty::Named(normalize_type_name(name))
            }
        },
        TypeExpr::Result(inner) => Ty::Res(inner.as_ref().map(|t| Box::new(ty_from_type_expr(t)))),
        TypeExpr::Optional(t) => Ty::Opt(Box::new(ty_from_type_expr(t))),
        TypeExpr::List(t) => Ty::List(Box::new(ty_from_type_expr(t))),
        TypeExpr::Set(t) => Ty::Set(Box::new(ty_from_type_expr(t))),
        TypeExpr::Map(k, v) => Ty::Map(
            Box::new(ty_from_type_expr(k)),
            Box::new(ty_from_type_expr(v)),
        ),
        TypeExpr::Tuple(items) if items.is_empty() => Ty::Unit,
        TypeExpr::Tuple(items) => Ty::Tuple(items.iter().map(ty_from_type_expr).collect()),
        TypeExpr::Array(t, _) => Ty::List(Box::new(ty_from_type_expr(t))),
        TypeExpr::Ref(t, _) | TypeExpr::Dyn(t) | TypeExpr::ImplTrait(t) => ty_from_type_expr(t),
        TypeExpr::FnPtr(_, ret) => ret
            .as_ref()
            .map(|t| ty_from_type_expr(t))
            .unwrap_or(Ty::Unit),
        TypeExpr::LitStr(_) => Ty::Named("Str".into()),
    }
}

fn normalize_type_name(n: &str) -> String {
    match n {
        "String" => "Str".into(),
        "Uuid" | "uuid" => "Id".into(),
        "UUID" => "Id".into(),
        "DateTime" | "DateTime<Utc>" => "Dt".into(),
        "i64" | "i32" | "u64" | "usize" => "Int".into(),
        "f64" | "f32" => "F64".into(),
        "bool" => "Bool".into(),
        other => other.to_string(),
    }
}

/// Conventional bare-field inference (aligned with codegen `infer_field_type`, VEIL names).
/// Returns None when the name is ambiguous / no convention applies.
pub fn infer_field_ty_from_name(name: &str) -> Option<Ty> {
    if name.is_empty() {
        return None;
    }
    if name == "id" || name.ends_with("_id") {
        return Some(Ty::Named("Id".into()));
    }
    if name.ends_with("_at")
        || name == "created"
        || name == "updated"
        || name == "deleted"
        || name == "expires"
        || name == "timestamp"
    {
        return Some(Ty::Named("Dt".into()));
    }
    if name.starts_with("is_")
        || name.starts_with("has_")
        || name.starts_with("can_")
        || name == "active"
        || name == "enabled"
        || name == "verified"
    {
        return Some(Ty::Named("Bool".into()));
    }
    if matches!(
        name,
        "count" | "total" | "amount" | "quantity" | "score" | "age" | "size" | "length" | "retries"
    ) {
        return Some(Ty::Named("Int".into()));
    }
    if matches!(
        name,
        "email"
            | "url"
            | "name"
            | "title"
            | "description"
            | "message"
            | "reason"
            | "path"
            | "key"
            | "token"
            | "code"
            | "addr"
    ) {
        return Some(Ty::Named("Str".into()));
    }
    None
}

/// Resolve a field's effective type: explicit type, or bare-name convention.
/// If bare and convention fails → Unknown + optional diagnostic from caller.
fn field_effective_ty(field: &Field) -> (Ty, bool /* was_bare_unknown */) {
    match &field.type_expr {
        TypeExpr::Named(n) if n.is_empty() || n == &field.name => {
            // Shorthand / bare
            if let Some(ty) = infer_field_ty_from_name(&field.name) {
                (ty, false)
            } else if n.is_empty() {
                (Ty::Unknown, true)
            } else if n == &field.name {
                // Named type equal to field name — could be domain type Customer
                // or bare shorthand. If it looks like a type (Capitalized) and
                // convention missed, treat as Named type construct.
                if n.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                    && infer_field_ty_from_name(n).is_none()
                {
                    // Ambiguous: bare `foo` vs type Foo. Convention: if name
                    // matches known type-shaped convention only. Otherwise if
                    // it's the same as field name and Capitalized, codegen
                    // treats as infer→String for unknown. Report unknown bare.
                    if is_conventional_only_via_codegen_default(n) {
                        (Ty::Named("Str".into()), false)
                    } else {
                        // Domain type with same name as field is rare; prefer Named.
                        (Ty::Named(normalize_type_name(n)), false)
                    }
                } else {
                    (Ty::Unknown, true)
                }
            } else {
                (Ty::Named(normalize_type_name(n)), false)
            }
        }
        other => (ty_from_type_expr(other), false),
    }
}

fn is_conventional_only_via_codegen_default(_n: &str) -> bool {
    // Codegen defaults unknown bare fields to String — we treat that as unknown
    // for agents rather than silently assuming Str (story: report ambiguous).
    false
}

/// rustdoc `.stub` files collapse `impl IntoUrl` / type parameters to a
/// single uppercase letter (`U`, `T`, `B`). Those are not constructable
/// types — any VEIL value is a legal argument.
fn is_stub_type_param(name: &str) -> bool {
    let n = name.trim();
    n.len() == 1 && n.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Are two types compatible for assignment / arg passing?
fn compatible(expected: &Ty, actual: &Ty) -> bool {
    if expected.is_unknown() || actual.is_unknown() {
        return true;
    }
    if matches!(expected, Ty::Named(n) if is_stub_type_param(n)) {
        return true;
    }
    // Json / Any accept structured domain values (Bus payloads, etc.)
    if matches!(expected, Ty::Named(n) if n == "Json" || n == "Any") {
        return true;
    }
    if matches!(actual, Ty::Named(n) if n == "Json" || n == "Any") {
        return true;
    }
    match (expected, actual) {
        (Ty::Named(a), Ty::Named(b)) => a == b,
        (Ty::Opt(e), Ty::Opt(a)) => compatible(e, a),
        // Allow T where Opt<T> expected (Some coercion) — common in agents
        (Ty::Opt(e), a) if !is_unit_ty(a) => compatible(e, a),
        // `ret ()` / unit arm = None when Opt<T> is expected
        (Ty::Opt(_), a) if is_unit_ty(a) => true,
        (Ty::List(e), Ty::List(a)) => compatible(e, a),
        (Ty::Set(e), Ty::Set(a)) => compatible(e, a),
        (Ty::Map(ek, ev), Ty::Map(ak, av)) => compatible(ek, ak) && compatible(ev, av),
        (Ty::Res(e), Ty::Res(a)) => match (e, a) {
            (None, None) => true,
            (Some(e), Some(a)) => compatible(e, a),
            (None, Some(_)) => true, // Res!<T> usable as Res!
            (Some(_), None) => false,
        },
        (Ty::Tuple(e), Ty::Tuple(a)) if e.len() == a.len() => {
            e.iter().zip(a.iter()).all(|(x, y)| compatible(x, y))
        }
        (Ty::Unit, Ty::Unit) => true,
        (Ty::Unit, Ty::Tuple(ts)) | (Ty::Tuple(ts), Ty::Unit) if ts.is_empty() => true,
        (Ty::Tuple(a), Ty::Tuple(b)) if a.is_empty() && b.is_empty() => true,
        // Res!<T> not assignable to T without ?
        _ => false,
    }
}

// ─── Environment ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MethodSig {
    params: Vec<Ty>,
    /// Parameter names (for messages)
    param_names: Vec<String>,
    ret: Ty,
}

#[derive(Debug, Clone, Default)]
struct TypeInfo {
    fields: HashMap<String, Ty>,
    methods: HashMap<String, MethodSig>,
    /// Zero-arg stub getters used as field access (`result.item` → `fn item()`).
    getters: HashMap<String, Ty>,
    /// Enum variant names (unit / data)
    variants: Vec<String>,
}

#[derive(Debug, Default)]
struct TypeEnv {
    types: HashMap<String, TypeInfo>,
    free_fns: HashMap<String, MethodSig>,
    /// Bare stub type names defined by two or more loaded crates.
    ambiguous_stub_types: HashSet<String>,
}

fn build_type_env(sol: &Solution, registry: &LayerRegistry) -> TypeEnv {
    let mut env = TypeEnv::default();
    index_stubs(registry, &mut env);
    index_layer_declarations(registry, &mut env);
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => index_construct_types(c, &mut env, registry),
            TopLevelItem::Function(f) => {
                env.free_fns.insert(
                    f.name.clone(),
                    method_sig_from_params(
                        &f.params.iter().map(|p| (p.name.clone(), p.type_expr.clone())).collect::<Vec<_>>(),
                        f.return_type.as_ref(),
                    ),
                );
            }
            TopLevelItem::TypeAlias { name, target } => {
                // Alias as transparent Named to target display
                let mut info = TypeInfo::default();
                info.fields.insert("__alias".into(), ty_from_type_expr(target));
                env.types.insert(name.clone(), info);
            }
            _ => {}
        }
    }
    env
}

fn is_veil_primitive_name(n: &str) -> bool {
    matches!(
        n,
        "Str" | "String" | "Int" | "F64" | "Bool" | "Bytes" | "UUID" | "Id"
            | "DateTime" | "Dt" | "List" | "Map" | "Set" | "Opt" | "Res" | "Json"
            | "Any" | "Unit" | "Self" | "T" | "E" | "O"
    )
}

fn rewrite_stub_ty(ty: Ty, crate_key: &str, self_type: &str, def_count: &HashMap<String, usize>) -> Ty {
    match ty {
        Ty::Named(n) if n == "Self" => Ty::Named(self_type.to_string()),
        Ty::Named(n) if !is_veil_primitive_name(&n) && def_count.get(&n).copied().unwrap_or(0) > 1 => {
            Ty::Named(format!("{crate_key}.{n}"))
        }
        Ty::Opt(t) => Ty::Opt(Box::new(rewrite_stub_ty(*t, crate_key, self_type, def_count))),
        Ty::List(t) => Ty::List(Box::new(rewrite_stub_ty(*t, crate_key, self_type, def_count))),
        Ty::Set(t) => Ty::Set(Box::new(rewrite_stub_ty(*t, crate_key, self_type, def_count))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(rewrite_stub_ty(*k, crate_key, self_type, def_count)),
            Box::new(rewrite_stub_ty(*v, crate_key, self_type, def_count)),
        ),
        Ty::Res(Some(t)) => Ty::Res(Some(Box::new(rewrite_stub_ty(*t, crate_key, self_type, def_count)))),
        Ty::Tuple(ts) => Ty::Tuple(
            ts.into_iter()
                .map(|t| rewrite_stub_ty(t, crate_key, self_type, def_count))
                .collect(),
        ),
        other => other,
    }
}

fn stub_ty_from_str(
    raw: &str,
    crate_key: &str,
    self_type: &str,
    def_count: &HashMap<String, usize>,
) -> Ty {
    let src = if raw == "Self" { self_type } else { raw };
    let te = crate::edit::parse_type_str(src);
    rewrite_stub_ty(ty_from_type_expr(&te), crate_key, self_type, def_count)
}

fn stub_method_sig(
    m: &crate::layer::StubMethod,
    crate_key: &str,
    self_type: &str,
    def_count: &HashMap<String, usize>,
) -> MethodSig {
    MethodSig {
        param_names: m.params.iter().map(|(n, _, _)| n.clone()).collect(),
        params: m
            .params
            .iter()
            .map(|(_, ty, _)| stub_ty_from_str(ty, crate_key, self_type, def_count))
            .collect(),
        ret: m
            .return_type
            .as_deref()
            .map(|r| stub_ty_from_str(r, crate_key, self_type, def_count))
            .unwrap_or(Ty::Unit),
    }
}

/// Index stub structs/impls so fluent SDK calls can be type-checked.
fn index_stubs(registry: &LayerRegistry, env: &mut TypeEnv) {
    let mut def_count: HashMap<String, usize> = HashMap::new();
    for stub in &registry.stubs {
        let mut seen = HashSet::new();
        for s in &stub.structs {
            if seen.insert(s.name.clone()) {
                *def_count.entry(s.name.clone()).or_default() += 1;
            }
        }
        for imp in &stub.impls {
            if seen.insert(imp.target.clone()) {
                *def_count.entry(imp.target.clone()).or_default() += 1;
            }
        }
    }
    for (name, n) in &def_count {
        if *n > 1 {
            env.ambiguous_stub_types.insert(name.clone());
        }
    }

    for stub in &registry.stubs {
        let crate_key = stub.name.replace('-', "_");
        let mut crate_keys = vec![crate_key.clone()];
        if let Some(alias) = &stub.alias {
            crate_keys.push(alias.replace('-', "_"));
        }

        let mut methods_by_type: HashMap<String, Vec<crate::layer::StubMethod>> = HashMap::new();
        for s in &stub.structs {
            methods_by_type
                .entry(s.name.clone())
                .or_default()
                .extend(s.methods.iter().cloned());
        }
        for imp in &stub.impls {
            methods_by_type
                .entry(imp.target.clone())
                .or_default()
                .extend(imp.methods.iter().cloned());
        }

        for (type_name, methods) in methods_by_type {
            let mut info = TypeInfo::default();
            for m in &methods {
                let sig = stub_method_sig(m, &crate_key, &type_name, &def_count);
                if m.params.is_empty()
                    && !matches!(
                        m.return_type.as_deref(),
                        None | Some("()") | Some("Self") | Some("self")
                    )
                {
                    info.getters.insert(m.name.clone(), sig.ret.clone());
                }
                info.methods.insert(m.name.clone(), sig);
                if m.name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    info.variants.push(m.name.clone());
                }
            }
            for ck in &crate_keys {
                env.types.insert(format!("{ck}.{type_name}"), info.clone());
            }
            if def_count.get(&type_name).copied().unwrap_or(0) <= 1 {
                env.types.insert(type_name, info);
            }
        }

        for f in &stub.free_fns {
            env.free_fns
                .insert(f.name.clone(), stub_method_sig(f, &crate_key, "", &def_count));
        }
    }
}

fn method_sig_from_params(params: &[(String, TypeExpr)], ret: Option<&TypeExpr>) -> MethodSig {
    MethodSig {
        param_names: params.iter().map(|(n, _)| n.clone()).collect(),
        params: params.iter().map(|(_, t)| ty_from_type_expr(t)).collect(),
        ret: ret.map(ty_from_type_expr).unwrap_or(Ty::Unit),
    }
}

fn index_construct_types(c: &Construct, env: &mut TypeEnv, registry: &LayerRegistry) {
    let mut info = TypeInfo::default();

    for f in &c.fields {
        let (ty, _) = field_effective_ty(f);
        info.fields.insert(f.name.clone(), ty);
    }
    for b in &c.blocks {
        for f in &b.fields {
            let (ty, _) = field_effective_ty(f);
            info.fields.insert(f.name.clone(), ty);
        }
    }
    // Adapter wiring: @field / @dep / @env become self.fields.
    for ann in &c.annotations {
        if registry.is_adapter_field_annotation(&ann.name)
            || registry.is_dependency_annotation(&ann.name)
        {
            for arg in &ann.args {
                if let Some((n, t)) = arg.split_once(':') {
                    let n = n.trim();
                    let te = crate::edit::parse_type_str(t.trim());
                    info.fields.insert(n.to_string(), ty_from_type_expr(&te));
                }
            }
        }
        if registry.is_adapter_env_annotation(&ann.name) {
            for arg in &ann.args {
                if arg.contains("DATABASE") {
                    info.fields.insert("pool".into(), Ty::Named("Pool".into()));
                }
                let snake = arg.to_ascii_lowercase();
                info.fields.insert(snake.clone(), Ty::Named("Str".into()));
                if let Some(short) = snake.rsplit('_').next() {
                    if short != snake {
                        info.fields.insert(short.to_string(), Ty::Named("Str".into()));
                    }
                }
            }
        }
    }

    for m in &c.methods {
        let name = m.name.trim_end_matches('!').to_string();
        let params: Vec<(String, TypeExpr)> = m
            .params
            .iter()
            .map(|p| (p.name.clone(), p.type_expr.clone()))
            .collect();
        let mut sig = method_sig_from_params(&params, m.return_type.as_ref());
        // save! implies Res!
        if m.name.ends_with('!') && !sig.ret.is_res() {
            if matches!(sig.ret, Ty::Unit) {
                sig.ret = Ty::Res(None);
            } else {
                sig.ret = Ty::Res(Some(Box::new(sig.ret)));
            }
        }
        info.methods.insert(name, sig);
    }

    for f in &c.fns {
        let name = f.name.trim_end_matches('!').to_string();
        let params: Vec<(String, TypeExpr)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), p.type_expr.clone()))
            .collect();
        info.methods
            .insert(name, method_sig_from_params(&params, f.return_type.as_ref()));
    }

    // Synthetic new() for structs
    if matches!(c.shape, Shape::Struct | Shape::Enum) {
        let field_tys: Vec<Ty> = c
            .fields
            .iter()
            .chain(c.blocks.iter().flat_map(|b| b.fields.iter()))
            .map(|f| field_effective_ty(f).0)
            .collect();
        // new takes "required" fields loosely as Unknown params for MVP
        let _ = field_tys;
        info.methods.insert(
            "new".into(),
            MethodSig {
                params: Vec::new(), // varargs-ish — don't check arg count for new
                param_names: Vec::new(),
                ret: Ty::Named(c.name.clone()),
            },
        );
    }

    if c.shape == Shape::Enum {
        info.variants = c.variants.clone();
        for rv in &c.rich_variants {
            match rv {
                EnumVariant::Unit(n) | EnumVariant::Tuple(n, _) | EnumVariant::Struct(n, _) => {
                    if !info.variants.contains(n) {
                        info.variants.push(n.clone());
                    }
                }
            }
        }
    }

    env.types.insert(c.name.clone(), info);

    for child in &c.children {
        index_construct_types(child, env, registry);
    }
}

// ─── Scope ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct Scope {
    locals: HashMap<String, Ty>,
}

impl Scope {
    fn bind(&mut self, name: &str, ty: Ty) {
        self.locals.insert(name.to_string(), ty);
    }
    fn get(&self, name: &str) -> Ty {
        self.locals.get(name).cloned().unwrap_or(Ty::Unknown)
    }
    fn child(&self) -> Scope {
        self.clone()
    }
}

// ─── Public entry ────────────────────────────────────────────────────────────

/// Run basic type checking. Returns diagnostics (errors and warnings).
pub fn check_types(sol: &Solution, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let env = build_type_env(sol, registry);
    let mut diagnostics = Vec::new();

    // Bare fields with no convention → warning once per field
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            check_bare_fields(c, &mut diagnostics);
        }
    }

    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => {
                check_construct_types(c, &env, registry, &mut diagnostics);
            }
            TopLevelItem::Function(f) => {
                let mut scope = Scope::default();
                for p in &f.params {
                    scope.bind(&p.name, ty_from_type_expr(&p.type_expr));
                }
                for e in &f.body {
                    infer_expr(e, &mut scope, &env, None, &f.name, &mut diagnostics);
                }
            }
            TopLevelItem::Flow(flow) => {
                let mut scope = Scope::default();
                for inp in &flow.inputs {
                    let (ty, _) = field_effective_ty(inp);
                    scope.bind(&inp.name, ty);
                }
                for step in &flow.steps {
                    check_flow_step_types(step, &mut scope, &env, &flow.name, &mut diagnostics);
                }
            }
            _ => {}
        }
    }

    // Cross-context invoke/request must name a real service/tool in this solution.
    // `dispatch Evt` is fire-and-forget: events do not need a handler.
    let handlers = collect_bus_handler_names(sol, registry);
    let events = collect_event_names(sol, registry);
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            walk_construct_for_missing_handlers(
                c,
                &handlers,
                &events,
                registry,
                &mut diagnostics,
            );
        }
    }

    diagnostics
}

/// Bus message names that have an application fn (svc/tool/handler/…).
fn collect_bus_handler_names(sol: &Solution, registry: &LayerRegistry) -> HashSet<String> {
    let mut names = HashSet::new();
    fn visit(c: &Construct, registry: &LayerRegistry, names: &mut HashSet<String>) {
        if c.shape == Shape::Fn && !crate::is_deploy_hook(c, registry) {
            names.insert(registry.bus_message_name(&c.name));
            names.insert(c.name.clone());
        }
        for child in &c.children {
            visit(child, registry, names);
        }
        for f in &c.fns {
            names.insert(registry.bus_message_name(&f.name));
            names.insert(f.name.clone());
        }
    }
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => visit(c, registry, &mut names),
            TopLevelItem::Function(f) => {
                names.insert(registry.bus_message_name(&f.name));
                names.insert(f.name.clone());
            }
            TopLevelItem::Flow(f) => {
                names.insert(registry.bus_message_name(&f.name));
                names.insert(f.name.clone());
            }
            _ => {}
        }
    }
    names
}

fn collect_event_names(sol: &Solution, registry: &LayerRegistry) -> HashSet<String> {
    let mut names = HashSet::new();
    fn visit(c: &Construct, registry: &LayerRegistry, names: &mut HashSet<String>) {
        if c.keyword == "evt"
            || c.subkind.eq_ignore_ascii_case("Event")
            || registry.is_a(&c.keyword, "Event")
            || registry.is_a(&c.subkind, "Event")
        {
            names.insert(c.name.clone());
            names.insert(registry.bus_message_name(&c.name));
        }
        for child in &c.children {
            visit(child, registry, names);
        }
    }
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            visit(c, registry, &mut names);
        }
    }
    names
}

fn walk_construct_for_missing_handlers(
    c: &Construct,
    handlers: &HashSet<String>,
    events: &HashSet<String>,
    registry: &LayerRegistry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for step in &c.steps {
        if let FlowStep::Step(s) = step {
            for e in &s.body {
                walk_expr_for_missing_handlers(e, &c.name, handlers, events, registry, diagnostics);
            }
        }
    }
    for f in &c.fns {
        for e in &f.body {
            walk_expr_for_missing_handlers(e, &f.name, handlers, events, registry, diagnostics);
        }
    }
    for child in &c.children {
        walk_construct_for_missing_handlers(child, handlers, events, registry, diagnostics);
    }
}

fn walk_expr_for_missing_handlers(
    expr: &Expr,
    location: &str,
    handlers: &HashSet<String>,
    events: &HashSet<String>,
    registry: &LayerRegistry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call(call) => {
            // Desugared `invoke Msg{…}` → Call{target:Bus, method:invoke, sugar, args:[StructLit]}
            let sugar = call.sugar.as_deref().unwrap_or("");
            if matches!(sugar, "invoke" | "request" | "dispatch")
                || (call.target == "Bus"
                    && matches!(
                        call.method.trim_end_matches(['!', '?']),
                        "invoke" | "request" | "dispatch"
                    ))
            {
                if let Some(msg) = bus_message_from_call_args(&call.args) {
                    let canonical = registry.bus_message_name(&msg);
                    let is_dispatch = sugar == "dispatch"
                        || call.method.trim_end_matches(['!', '?']) == "dispatch";
                    let is_event = events.contains(&msg) || events.contains(&canonical);
                    if is_dispatch && is_event {
                        // Domain event fire-and-forget — no application handler required.
                    } else if !handlers.contains(&msg) && !handlers.contains(&canonical) {
                        diagnostics.push(diag(
                            Severity::Error,
                            "missing_handler",
                            format!(
                                "{sugar} target '{msg}' has no svc/tool/handler in this package"
                            ),
                            location,
                            Some(call.span),
                            Some(
                                "declare a service/tool with that name, or fix the invoke payload type"
                                    .into(),
                            ),
                        ));
                    }
                }
            }
            for a in &call.args {
                walk_expr_for_missing_handlers(a, location, handlers, events, registry, diagnostics);
            }
        }
        Expr::Assign(_, rhs, _) | Expr::MutAssign(_, rhs, _) | Expr::Return(rhs) => {
            walk_expr_for_missing_handlers(rhs, location, handlers, events, registry, diagnostics);
        }
        Expr::IfExpr(ie) => {
            walk_expr_for_missing_handlers(&ie.condition, location, handlers, events, registry, diagnostics);
            for e in &ie.then_body {
                walk_expr_for_missing_handlers(e, location, handlers, events, registry, diagnostics);
            }
            if let Some(eb) = &ie.else_body {
                for e in eb {
                    walk_expr_for_missing_handlers(e, location, handlers, events, registry, diagnostics);
                }
            }
        }
        Expr::Match(scrut, arms) => {
            walk_expr_for_missing_handlers(scrut, location, handlers, events, registry, diagnostics);
            for arm in arms {
                for e in &arm.body {
                    walk_expr_for_missing_handlers(e, location, handlers, events, registry, diagnostics);
                }
            }
        }
        Expr::Action(a) => {
            // Non-desugared invoke (no port_target) still carries the message name.
            let is_dispatch = a.keyword == "dispatch";
            let is_event = events.contains(&a.target)
                || events.contains(&registry.bus_message_name(&a.target));
            if matches!(a.keyword.as_str(), "invoke" | "request" | "dispatch")
                && !a.target.is_empty()
                && !(is_dispatch && is_event)
                && !handlers.contains(&a.target)
                && !handlers.contains(&registry.bus_message_name(&a.target))
            {
                diagnostics.push(diag(
                    Severity::Error,
                    "missing_handler",
                    format!(
                        "{} target '{}' has no svc/tool/handler in this package",
                        a.keyword, a.target
                    ),
                    location,
                    Some(a.span),
                    Some(
                        "declare a service/tool with that name, or fix the invoke target".into(),
                    ),
                ));
            }
        }
        _ => {}
    }
}

fn bus_message_from_call_args(args: &[Expr]) -> Option<String> {
    match args.first() {
        Some(Expr::StructLit(name, _)) => Some(name.clone()),
        Some(Expr::Ident(name)) => Some(name.clone()),
        _ => None,
    }
}

fn check_bare_fields(c: &Construct, diagnostics: &mut Vec<Diagnostic>) {
    for f in c.fields.iter().chain(c.blocks.iter().flat_map(|b| b.fields.iter())) {
        let bare = matches!(
            &f.type_expr,
            TypeExpr::Named(n) if n.is_empty() || n == &f.name
        );
        if !bare {
            continue;
        }
        if infer_field_ty_from_name(&f.name).is_some() {
            continue;
        }
        // Capitalized name equal to itself may be domain type — only flag empty type
        if matches!(&f.type_expr, TypeExpr::Named(n) if n.is_empty())
            || (matches!(&f.type_expr, TypeExpr::Named(n) if n == &f.name)
                && f.name.chars().next().map(|c| c.is_lowercase()).unwrap_or(false))
        {
            diagnostics.push(diag(
                Severity::Warning,
                "ambiguous_field_type",
                format!(
                    "field '{}' has no type and no naming convention — type is unknown",
                    f.name
                ),
                &c.name,
                Some(f.span),
                Some("add an explicit type (e.g. `name: Str`) or use a conventional name (id, email, count, …)".into()),
            ));
        }
    }
    for child in &c.children {
        check_bare_fields(child, diagnostics);
    }
}

fn looks_like_env_var(name: &str) -> bool {
    name.contains('_')
        && name.chars().any(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_')
}

fn expr_has_bang_or_try(expr: &Expr) -> bool {
    match expr {
        Expr::Try(e) => {
            let _ = e;
            true
        }
        Expr::Call(c) => {
            c.method.ends_with('!')
                || c.target.ends_with('!')
                || c.args.iter().any(expr_has_bang_or_try)
                || c.receiver
                    .as_ref()
                    .map(|r| expr_has_bang_or_try(r))
                    .unwrap_or(false)
        }
        Expr::Action(a) => {
            a.keyword.ends_with('!')
                || a.target.ends_with('!')
                || a.args.iter().any(expr_has_bang_or_try)
                || a.named_args.iter().any(|(_, e)| expr_has_bang_or_try(e))
        }
        Expr::Assign(_, r, _) | Expr::MutAssign(_, r, _) | Expr::Return(r) | Expr::Await(r) => {
            expr_has_bang_or_try(r)
        }
        Expr::FieldAccess(b, _) | Expr::Index(b, _) => expr_has_bang_or_try(b),
        Expr::IfExpr(ie) => {
            expr_has_bang_or_try(&ie.condition)
                || ie.then_body.iter().any(expr_has_bang_or_try)
                || ie
                    .else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_has_bang_or_try))
                    .unwrap_or(false)
        }
        Expr::Match(s, arms) => {
            expr_has_bang_or_try(s) || arms.iter().any(|a| a.body.iter().any(expr_has_bang_or_try))
        }
        Expr::BinaryOp(op) => expr_has_bang_or_try(&op.left) || expr_has_bang_or_try(&op.right),
        Expr::StructLit(_, fields) => fields.iter().any(|(_, e)| expr_has_bang_or_try(e)),
        Expr::ArrayLit(items) | Expr::Tuple(items) => items.iter().any(expr_has_bang_or_try),
        _ => false,
    }
}

fn is_unit_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Unit => true,
        Ty::Tuple(ts) if ts.is_empty() => true,
        Ty::Named(n) if n == "()" || n == "Unit" => true,
        _ => false,
    }
}

/// Unit `Res!` / `Res!<()>`: an implicit last statement is a side-effect
/// (`send!()`); codegen wraps `Ok(())`. An explicit `ret x` must be unit.
/// Non-unit `Res!<T>` always requires `actual` to match `T`.
fn return_compatible(expected: &Ty, actual: &Ty, last_was_return: bool) -> bool {
    if compatible(expected, actual) {
        return true;
    }
    match expected {
        Ty::Res(None) => {
            if last_was_return {
                is_unit_ty(actual)
                    || matches!(actual, Ty::Res(None))
                    || matches!(actual, Ty::Res(Some(t)) if is_unit_ty(t))
            } else {
                true
            }
        }
        Ty::Res(Some(inner)) if is_unit_ty(inner) => {
            if last_was_return {
                is_unit_ty(actual)
                    || matches!(actual, Ty::Res(None))
                    || matches!(actual, Ty::Res(Some(t)) if is_unit_ty(t))
            } else {
                true
            }
        }
        Ty::Res(Some(inner)) => compatible(inner, actual),
        _ => false,
    }
}

fn check_construct_types(
    c: &Construct,
    env: &TypeEnv,
    registry: &LayerRegistry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if c.shape == Shape::Impl {
        for ann in &c.annotations {
            if ann.args.is_empty()
                && (registry.is_adapter_env_annotation(&ann.name)
                    || registry.is_adapter_field_annotation(&ann.name))
            {
                diagnostics.push(diag(
                    Severity::Error,
                    "annotation_missing_args",
                    format!("@{0} needs arguments — write `@{0}(NAME)`", ann.name),
                    &c.name,
                    Some(ann.span),
                    Some(format!(
                        "`@{}(TABLE_NAME)` or `@{}(sns: aws_sdk_sns.Client)`",
                        ann.name, ann.name
                    )),
                ));
            }
        }
    }

    // Nested methods
    for fndef in &c.fns {
        let mut scope = Scope::default();
        if let Some(info) = env.types.get(&c.name) {
            for (name, ty) in &info.fields {
                scope.bind(name, ty.clone());
            }
        }
        for p in &fndef.params {
            scope.bind(&p.name, ty_from_type_expr(&p.type_expr));
        }
        for e in &fndef.body {
            infer_expr(e, &mut scope, env, Some(&c.name), &c.name, diagnostics);
        }
    }

    for imp in &c.impls {
        let mut scope = Scope::default();
        // Import fields from related struct
        import_impl_fields(&c.name, env, &mut scope);
        if let Some(info) = env.types.get(&c.name) {
            for (name, ty) in &info.fields {
                scope.bind(name, ty.clone());
            }
        }
        let method = imp.method_name.trim_end_matches('!');
        let trait_sig = c
            .target
            .as_ref()
            .and_then(|t| env.types.get(t))
            .and_then(|i| i.methods.get(method));
        for (i, p) in imp.params.iter().enumerate() {
            let ty = trait_sig
                .and_then(|s| s.params.get(i).cloned())
                .unwrap_or(Ty::Unknown);
            scope.bind(p, ty);
        }
        let mut last_ty = Ty::Unit;
        let mut last_was_return = false;
        for e in &imp.body {
            last_was_return = matches!(e, Expr::Return(_));
            last_ty = match e {
                Expr::Return(inner) => {
                    infer_expr(inner, &mut scope, env, Some(&c.name), &c.name, diagnostics)
                }
                other => infer_expr(other, &mut scope, env, Some(&c.name), &c.name, diagnostics),
            };
        }
        if let Some(sig) = trait_sig {
            if is_unit_ty(&sig.ret) && imp.body.iter().any(expr_has_bang_or_try) {
                diagnostics.push(diag(
                    Severity::Error,
                    "bang_in_unit_fn",
                    format!(
                        "'{}' returns () but the body uses `!` or `?` — rustc will reject `?` in a unit fn",
                        imp.method_name
                    ),
                    &c.name,
                    Some(imp.span),
                    Some(
                        "declare the port method with `!` (e.g. `save!(…) -> ()`) so it is Res!, or drop the bang"
                            .into(),
                    ),
                ));
            } else if !last_ty.is_unknown()
                && !sig.ret.is_unknown()
                && !return_compatible(&sig.ret, &last_ty, last_was_return)
            {
                diagnostics.push(diag(
                    Severity::Error,
                    "type_mismatch",
                    format!(
                        "'{}' returns {} but the body produces {}",
                        imp.method_name,
                        sig.ret.display(),
                        last_ty.display()
                    ),
                    &c.name,
                    Some(imp.span),
                    Some(return_mismatch_hint(&sig.ret, &last_ty)),
                ));
            }
        }
    }

    if !c.steps.is_empty() || !c.inputs.is_empty() {
        let mut scope = Scope::default();
        for inp in &c.inputs {
            let (ty, _) = field_effective_ty(inp);
            scope.bind(&inp.name, ty);
        }
        for step in &c.steps {
            check_flow_step_types(step, &mut scope, env, &c.name, diagnostics);
        }
        if let Some(ret) = &c.return_expr {
            infer_expr(ret, &mut scope, env, None, &c.name, diagnostics);
        }
    }

    for child in &c.children {
        check_construct_types(child, env, registry, diagnostics);
    }
}

fn import_impl_fields(impl_name: &str, env: &TypeEnv, scope: &mut Scope) {
    for suffix in ["Impl", "Adapter"] {
        if let Some(base) = impl_name.strip_suffix(suffix) {
            if let Some(info) = env.types.get(base) {
                for (n, t) in &info.fields {
                    scope.bind(n, t.clone());
                }
            }
        }
    }
}

fn check_flow_step_types(
    step: &FlowStep,
    scope: &mut Scope,
    env: &TypeEnv,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match step {
        FlowStep::Step(sd) => {
            for e in &sd.body {
                infer_expr(e, scope, env, None, location, diagnostics);
            }
            for sb in &sd.sub_blocks {
                for e in &sb.body {
                    infer_expr(e, scope, env, None, location, diagnostics);
                }
            }
        }
        FlowStep::Parallel(par) => {
            for s in &par.steps {
                check_flow_step_types(&FlowStep::Step(s.clone()), scope, env, location, diagnostics);
            }
        }
        FlowStep::Match(m) => {
            let scrut_ty = infer_expr(&m.expr, scope, env, None, location, diagnostics);
            check_match_arms(&scrut_ty, &m.arms, scope, env, None, location, diagnostics);
        }
    }
}

// ─── Inference ───────────────────────────────────────────────────────────────

fn infer_expr(
    expr: &Expr,
    scope: &mut Scope,
    env: &TypeEnv,
    self_type: Option<&str>,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Ty {
    match expr {
        Expr::StringLit(_) | Expr::StringInterp(_) => Ty::Named("Str".into()),
        Expr::IntLit(_) => Ty::Named("Int".into()),
        Expr::FloatLit(_) => Ty::Named("F64".into()),
        Expr::BoolLit(_) => Ty::Named("Bool".into()),
        Expr::Break | Expr::Continue => Ty::Unit,
        Expr::Stock => Ty::Unit, // expanded before typecheck of merged IR
        Expr::Ident(name) => {
            if name == "null" || name == "None" {
                return Ty::Opt(Box::new(Ty::Unknown));
            }
            if name == "self" {
                return self_type
                    .map(|s| Ty::Named(s.to_string()))
                    .unwrap_or(Ty::Unknown);
            }
            let t = scope.get(name);
            if t.is_unknown() {
                if let Some(st) = self_type {
                    if let Some(info) = env.types.get(st) {
                        let snake = name.to_ascii_lowercase();
                        if looks_like_env_var(name)
                            && (info.fields.contains_key(&snake)
                                || info.fields.contains_key(name))
                        {
                            diagnostics.push(diag(
                                Severity::Error,
                                "env_use_self_field",
                                format!(
                                    "'{name}' is an @env var — write `self.{snake}` (full lowercased name)"
                                ),
                                location,
                                None,
                                Some(format!("@env({name}) is available as self.{snake}")),
                            ));
                            return Ty::Named("Str".into());
                        }
                        if looks_like_env_var(name) {
                            diagnostics.push(diag(
                                Severity::Error,
                                "unknown_env_ident",
                                format!(
                                    "'{name}' looks like an env var but is not declared on this adapter"
                                ),
                                location,
                                None,
                                Some(format!(
                                    "declare `@env({name})` and write `self.{snake}`"
                                )),
                            ));
                            return Ty::Named("Str".into());
                        }
                        if let Some(fty) = info
                            .fields
                            .get(name)
                            .or_else(|| info.fields.get(&snake))
                        {
                            diagnostics.push(diag(
                                Severity::Warning,
                                "adapter_field_needs_self",
                                format!("'{name}' is an adapter field — write `self.{name}`"),
                                location,
                                None,
                                Some(format!(
                                    "codegen maps this to self.{}, but prefer the explicit form",
                                    snake
                                )),
                            ));
                            return fty.clone();
                        }
                    }
                } else if looks_like_env_var(name) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "unknown_env_ident",
                        format!("'{name}' looks like an env var — it is not in scope"),
                        location,
                        None,
                        Some(format!(
                            "declare `@env({name})` on the adapter and write `self.{}`",
                            name.to_ascii_lowercase()
                        )),
                    ));
                    return Ty::Named("Str".into());
                }
            }
            t
        }
        Expr::FieldAccess(inner, field) => {
            let base = infer_expr(inner, scope, env, self_type, location, diagnostics);
            field_ty_of(&base, field, env)
        }
        Expr::Call(call) => infer_call(call, scope, env, self_type, location, diagnostics),
        Expr::Action(a) => {
            // Walk args; guards are bool conditions
            for arg in &a.args {
                infer_expr(arg, scope, env, self_type, location, diagnostics);
            }
            for (_, e) in &a.named_args {
                infer_expr(e, scope, env, self_type, location, diagnostics);
            }
            if let Some(cond) = &a.condition {
                let ct = infer_expr(cond, scope, env, self_type, location, diagnostics);
                if !ct.is_unknown() && !compatible(&Ty::Named("Bool".into()), &ct) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "type_mismatch",
                        format!(
                            "guard/condition expected Bool, found {}",
                            ct.display()
                        ),
                        location,
                        Some(a.span),
                        None,
                    ));
                }
            }
            Ty::Unknown
        }
        Expr::Assign(name, rhs, ann) => {
            let rhs_ty = infer_expr(rhs, scope, env, self_type, location, diagnostics);
            if let Some(te) = ann {
                let expected = ty_from_type_expr(te);
                if !compatible(&expected, &rhs_ty) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "type_mismatch",
                        format!(
                            "'{}' annotated as {} but initializer is {}",
                            name,
                            expected.display(),
                            rhs_ty.display()
                        ),
                        location,
                        None,
                        None,
                    ));
                }
                scope.bind(name, expected);
            } else {
                let prev = scope.get(name);
                if !prev.is_unknown() && !rhs_ty.is_unknown() && !compatible(&prev, &rhs_ty) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "type_mismatch",
                        format!(
                            "cannot assign {} to '{}' (expected {})",
                            rhs_ty.display(),
                            name,
                            prev.display()
                        ),
                        location,
                        None,
                        None,
                    ));
                } else if prev.is_unknown() {
                    scope.bind(name, rhs_ty.clone());
                }
            }
            // Reassignment keeps previous type (when unannotated)
            Ty::Unit
        }
        Expr::MutAssign(name, rhs, ann) => {
            let rhs_ty = infer_expr(rhs, scope, env, self_type, location, diagnostics);
            if let Some(te) = ann {
                let expected = ty_from_type_expr(te);
                if !compatible(&expected, &rhs_ty) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "type_mismatch",
                        format!(
                            "mut '{}' annotated as {} but initializer is {}",
                            name,
                            expected.display(),
                            rhs_ty.display()
                        ),
                        location,
                        None,
                        None,
                    ));
                }
                scope.bind(name, expected);
            } else {
                scope.bind(name, rhs_ty);
            }
            Ty::Unit
        }
        Expr::LetPattern(pat, rhs, ann) => {
            let rhs_ty = infer_expr(rhs, scope, env, self_type, location, diagnostics);
            if let Some(te) = ann {
                let expected = ty_from_type_expr(te);
                if !compatible(&expected, &rhs_ty) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "type_mismatch",
                        format!(
                            "pattern binding expected {}, found {}",
                            expected.display(),
                            rhs_ty.display()
                        ),
                        location,
                        None,
                        None,
                    ));
                }
                bind_pattern_ty(pat, &expected, scope);
            } else {
                bind_pattern_ty(pat, &rhs_ty, scope);
            }
            Ty::Unit
        }
        Expr::BinaryOp(op) => {
            let l = infer_expr(&op.left, scope, env, self_type, location, diagnostics);
            let r = infer_expr(&op.right, scope, env, self_type, location, diagnostics);
            use BinOp::*;
            match op.op {
                Eq | NotEq | Lt | Gt | LtEq | GtEq | And | Or => {
                    if !l.is_unknown() && !r.is_unknown() {
                        // Comparison: allow same types; logical needs Bool
                        if matches!(op.op, And | Or) {
                            let b = Ty::Named("Bool".into());
                            if !compatible(&b, &l) || !compatible(&b, &r) {
                                diagnostics.push(diag(
                                    Severity::Error,
                                    "type_mismatch",
                                    format!(
                                        "logical operator requires Bool operands (found {}, {})",
                                        l.display(),
                                        r.display()
                                    ),
                                    location,
                                    None,
                                    None,
                                ));
                            }
                        }
                    }
                    Ty::Named("Bool".into())
                }
                Add | Sub | Mul | Div | Mod => {
                    if !l.is_unknown() && !r.is_unknown() && !compatible(&l, &r) {
                        // Allow Int/F64 mix as F64? Keep strict for MVP
                        if !(l.is_scalar() && r.is_scalar() && numeric_pair(&l, &r)) {
                            diagnostics.push(diag(
                                Severity::Error,
                                "type_mismatch",
                                format!(
                                    "binary operator on incompatible types {} and {}",
                                    l.display(),
                                    r.display()
                                ),
                                location,
                                None,
                                None,
                            ));
                        }
                    }
                    if matches!(l, Ty::Named(ref n) if n == "F64")
                        || matches!(r, Ty::Named(ref n) if n == "F64")
                    {
                        Ty::Named("F64".into())
                    } else if !l.is_unknown() {
                        l
                    } else {
                        r
                    }
                }
            }
        }
        Expr::UnaryOp(op) => {
            let t = infer_expr(&op.expr, scope, env, self_type, location, diagnostics);
            match op.op {
                UnaryOp::Not => {
                    if !t.is_unknown() && !compatible(&Ty::Named("Bool".into()), &t) {
                        diagnostics.push(diag(
                            Severity::Error,
                            "type_mismatch",
                            format!("`!` requires Bool, found {}", t.display()),
                            location,
                            None,
                            None,
                        ));
                    }
                    Ty::Named("Bool".into())
                }
                UnaryOp::Neg => t,
            }
        }
        Expr::IfExpr(ie) => {
            let ct = infer_expr(&ie.condition, scope, env, self_type, location, diagnostics);
            if !ct.is_unknown() && !compatible(&Ty::Named("Bool".into()), &ct) {
                diagnostics.push(diag(
                    Severity::Error,
                    "type_mismatch",
                    format!("if condition expected Bool, found {}", ct.display()),
                    location,
                    None,
                    None,
                ));
            }
            let mut ts = scope.child();
            let mut then_ty = Ty::Unit;
            for e in &ie.then_body {
                then_ty = infer_expr(e, &mut ts, env, self_type, location, diagnostics);
            }
            if let Some(eb) = &ie.else_body {
                let mut es = scope.child();
                for e in eb {
                    infer_expr(e, &mut es, env, self_type, location, diagnostics);
                }
            }
            then_ty
        }
        Expr::Match(scrutinee, arms) => {
            let st = infer_expr(scrutinee, scope, env, self_type, location, diagnostics);
            check_match_arms(&st, arms, scope, env, self_type, location, diagnostics)
        }
        Expr::Return(e) => {
            infer_expr(e, scope, env, self_type, location, diagnostics);
            Ty::Unit
        }
        Expr::Await(e) => {
            let t = infer_expr(e, scope, env, self_type, location, diagnostics);
            if t.is_scalar() {
                diagnostics.push(diag(
                    Severity::Warning,
                    "await_on_scalar",
                    format!("await on scalar type {} is unusual", t.display()),
                    location,
                    None,
                    Some("await is for async/fallible operations".into()),
                ));
            }
            // Await unwraps Res/async to inner if Res
            match t {
                Ty::Res(Some(inner)) => *inner,
                Ty::Res(None) => Ty::Unit,
                other => other,
            }
        }
        Expr::Require(e) => {
            let t = infer_expr(e, scope, env, self_type, location, diagnostics);
            // require force-presents Opt and Res (ACS-010). Already-T is left as-is.
            // Json field/index require extracts a present Str (SL-027) — same as codegen.
            match t {
                Ty::Opt(inner) => *inner,
                Ty::Res(Some(inner)) => *inner,
                Ty::Res(None) => Ty::Unit,
                other if is_json_ty(&other) => Ty::Named("Str".into()),
                other => other,
            }
        }
        Expr::Try(e) => {
            let t = infer_expr(e, scope, env, self_type, location, diagnostics);
            if !t.is_unknown() && !t.is_res() {
                diagnostics.push(diag(
                    Severity::Error,
                    "try_on_non_result",
                    format!("`?` requires Res! / Res!<T>, found {}", t.display()),
                    location,
                    None,
                    Some("only fallible values can use ?".into()),
                ));
                return t;
            }
            match t {
                Ty::Res(Some(inner)) => *inner,
                Ty::Res(None) => Ty::Unit,
                other => other,
            }
        }
        Expr::StructLit(name, fields) => {
            // Bare `{ k: v, … }` is a map/record literal, not a named struct.
            // Adapters pass these as Map<Str, Str> attributes / DDB items.
            if name.is_empty() {
                let mut val_ty = Ty::Unknown;
                for (_, fexpr) in fields {
                    let ft = infer_expr(fexpr, scope, env, self_type, location, diagnostics);
                    if val_ty.is_unknown() {
                        val_ty = ft;
                    }
                }
                return Ty::Map(Box::new(Ty::Named("Str".into())), Box::new(val_ty));
            }
            if let Some(info) = env.types.get(name) {
                for (fname, fexpr) in fields {
                    let ft = infer_expr(fexpr, scope, env, self_type, location, diagnostics);
                    if let Some(expected) = info.fields.get(fname) {
                        if !compatible(expected, &ft) {
                            diagnostics.push(diag(
                                Severity::Error,
                                "type_mismatch",
                                format!(
                                    "field '{}' of {} expected {}, found {}",
                                    fname,
                                    name,
                                    expected.display(),
                                    ft.display()
                                ),
                                location,
                                None,
                                None,
                            ));
                        }
                    }
                }
            } else {
                for (_, fexpr) in fields {
                    infer_expr(fexpr, scope, env, self_type, location, diagnostics);
                }
            }
            Ty::Named(name.clone())
        }
        Expr::StructUpdate { name, fields, base } => {
            infer_expr(base, scope, env, self_type, location, diagnostics);
            for (_, fexpr) in fields {
                infer_expr(fexpr, scope, env, self_type, location, diagnostics);
            }
            Ty::Named(name.clone())
        }
        Expr::Tuple(items) => {
            Ty::Tuple(
                items
                    .iter()
                    .map(|e| infer_expr(e, scope, env, self_type, location, diagnostics))
                    .collect(),
            )
        }
        Expr::ArrayLit(items) => {
            let mut elem = Ty::Unknown;
            for e in items {
                let t = infer_expr(e, scope, env, self_type, location, diagnostics);
                if elem.is_unknown() {
                    elem = t;
                } else if !t.is_unknown() && !compatible(&elem, &t) {
                    diagnostics.push(diag(
                        Severity::Error,
                        "type_mismatch",
                        format!(
                            "array elements must have same type ({} vs {})",
                            elem.display(),
                            t.display()
                        ),
                        location,
                        None,
                        None,
                    ));
                }
            }
            Ty::List(Box::new(elem))
        }
        Expr::Index(base, idx) => {
            let bt = infer_expr(base, scope, env, self_type, location, diagnostics);
            let it = infer_expr(idx, scope, env, self_type, location, diagnostics);
            match bt {
                Ty::List(e) => {
                    if !it.is_unknown() && !compatible(&Ty::Named("Int".into()), &it) {
                        diagnostics.push(diag(
                            Severity::Error,
                            "type_mismatch",
                            format!("index must be Int, found {}", it.display()),
                            location,
                            None,
                            None,
                        ));
                    }
                    *e
                }
                Ty::Map(k, v) => {
                    if !it.is_unknown() && !compatible(&k, &it) {
                        diagnostics.push(diag(
                            Severity::Error,
                            "type_mismatch",
                            format!("map key must be {}, found {}", k.display(), it.display()),
                            location,
                            None,
                            None,
                        ));
                    }
                    *v
                }
                Ty::Named(n) if n == "Str" => Ty::Named("Str".into()),
                other => other,
            }
        }
        Expr::ForLoop {
            binding,
            index: idx,
            iterable,
            body,
        } => {
            let it = infer_expr(iterable, scope, env, self_type, location, diagnostics);
            let mut ls = scope.child();
            let elem = match it {
                Ty::List(e) => *e,
                Ty::Set(e) => *e,
                _ => Ty::Unknown,
            };
            ls.bind(binding, elem);
            if let Some(i) = idx {
                ls.bind(i, Ty::Named("Int".into()));
            }
            for e in body {
                infer_expr(e, &mut ls, env, self_type, location, diagnostics);
            }
            Ty::Unit
        }
        Expr::WhileLoop { condition, body } => {
            let ct = infer_expr(condition, scope, env, self_type, location, diagnostics);
            if !ct.is_unknown() && !compatible(&Ty::Named("Bool".into()), &ct) {
                diagnostics.push(diag(
                    Severity::Error,
                    "type_mismatch",
                    format!("while condition expected Bool, found {}", ct.display()),
                    location,
                    None,
                    None,
                ));
            }
            let mut ls = scope.child();
            for e in body {
                infer_expr(e, &mut ls, env, self_type, location, diagnostics);
            }
            Ty::Unit
        }
        Expr::Loop(body) => {
            let mut ls = scope.child();
            for e in body {
                infer_expr(e, &mut ls, env, self_type, location, diagnostics);
            }
            Ty::Unit
        }
        Expr::DoBlock(body) => {
            let mut ls = scope.child();
            for e in body {
                infer_expr(e, &mut ls, env, self_type, location, diagnostics);
            }
            Ty::Unit
        }
        Expr::Closure { params, body } => {
            let mut cs = scope.child();
            for p in params {
                cs.bind(p, Ty::Unknown);
            }
            for e in body {
                infer_expr(e, &mut cs, env, self_type, location, diagnostics);
            }
            Ty::Unknown
        }
        Expr::Range { start, end, .. } => {
            if let Some(s) = start {
                infer_expr(s, scope, env, self_type, location, diagnostics);
            }
            if let Some(e) = end {
                infer_expr(e, scope, env, self_type, location, diagnostics);
            }
            Ty::List(Box::new(Ty::Named("Int".into())))
        }
        Expr::Cast(e, ty_name) => {
            infer_expr(e, scope, env, self_type, location, diagnostics);
            Ty::Named(normalize_type_name(ty_name))
        }
        Expr::IfLet {
            pattern,
            expr: e,
            then_body,
            else_body,
        } => {
            let t = infer_expr(e, scope, env, self_type, location, diagnostics);
            let mut ts = scope.child();
            bind_string_pattern(pattern, &t, &mut ts);
            for x in then_body {
                infer_expr(x, &mut ts, env, self_type, location, diagnostics);
            }
            if let Some(eb) = else_body {
                let mut es = scope.child();
                for x in eb {
                    infer_expr(x, &mut es, env, self_type, location, diagnostics);
                }
            }
            Ty::Unit
        }
        Expr::WhileLet {
            pattern,
            expr: e,
            body,
        } => {
            let t = infer_expr(e, scope, env, self_type, location, diagnostics);
            let mut ts = scope.child();
            bind_string_pattern(pattern, &t, &mut ts);
            for x in body {
                infer_expr(x, &mut ts, env, self_type, location, diagnostics);
            }
            Ty::Unit
        }
    }
}

fn numeric_pair(a: &Ty, b: &Ty) -> bool {
    matches!(
        (a, b),
        (Ty::Named(x), Ty::Named(y))
            if matches!((x.as_str(), y.as_str()),
                ("Int", "Int") | ("F64", "F64") | ("Int", "F64") | ("F64", "Int"))
    )
}

fn field_ty_of(base: &Ty, field: &str, env: &TypeEnv) -> Ty {
    match base {
        Ty::Named(n) if n == "Json" => Ty::Named("Json".into()),
        Ty::Named(n) => env
            .types
            .get(n)
            .and_then(|info| {
                info.fields
                    .get(field)
                    .cloned()
                    .or_else(|| info.getters.get(field).cloned())
            })
            .unwrap_or(Ty::Unknown),
        Ty::Opt(_) if field == "is_some" || field == "is_none" => Ty::Named("Bool".into()),
        Ty::Opt(inner) => field_ty_of(inner, field, env),
        Ty::Res(Some(inner)) => field_ty_of(inner, field, env),
        _ => Ty::Unknown,
    }
}

fn is_str_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Named(n) if n == "Str" || n == "String")
}

fn is_json_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Named(n) if n == "Json")
}

/// Index layer `declare struct` fields so hook inputs like `DeployContext` typecheck.
fn index_layer_declarations(registry: &LayerRegistry, env: &mut TypeEnv) {
    for decl in &registry.declarations {
        let mut current: Option<(String, TypeInfo)> = None;
        let flush = |env: &mut TypeEnv, current: &mut Option<(String, TypeInfo)>| {
            if let Some((name, info)) = current.take() {
                env.types.entry(name).or_insert(info);
            }
        };
        for line in decl.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let mut started = false;
            for prefix in ["struct ", "enum ", "trait ", "port "] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    flush(env, &mut current);
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        current = Some((name, TypeInfo::default()));
                    }
                    started = true;
                    break;
                }
            }
            if started {
                continue;
            }
            if t.starts_with("fn ") {
                flush(env, &mut current);
                continue;
            }
            if let Some((_, info)) = current.as_mut() {
                if let Some((fname, fty)) = t.split_once(':') {
                    let fname = fname.trim();
                    if !fname.is_empty()
                        && fname
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_')
                    {
                        info.fields.insert(
                            fname.to_string(),
                            ty_from_type_expr(&crate::edit::parse_type_str(fty.trim())),
                        );
                    }
                }
            }
        }
        flush(env, &mut current);
    }
}

fn is_bytes_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Named(n) if n == "Bytes" || n == "Vec<u8>")
}

fn is_blob_name(name: &str) -> bool {
    name == "Blob" || name.ends_with(".Blob") || name.ends_with("::Blob")
}

fn is_blob_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Named(n) => is_blob_name(n),
        _ => false,
    }
}

/// List / Map / Set methods. `get` matches codegen: HashMap.get("lit") is
/// unwrapped, so the type is `V` not `Opt<V>`.
fn collection_method_ty(recv: &Ty, method: &str) -> Option<Ty> {
    match recv {
        Ty::List(elem) => match method {
            "get" | "at" | "index" => Some(*elem.clone()),
            "len" | "length" | "count" => Some(Ty::Named("Int".into())),
            "is_empty" | "contains" => Some(Ty::Named("Bool".into())),
            "push" | "append" | "insert" | "extend" => Some(Ty::Unit),
            _ => None,
        },
        Ty::Map(k, v) => match method {
            "get" | "at" | "index" => Some(*v.clone()),
            "len" | "length" | "count" => Some(Ty::Named("Int".into())),
            "is_empty" | "contains_key" | "contains" => Some(Ty::Named("Bool".into())),
            "insert" | "extend" => Some(Ty::Unit),
            "remove" => Some(Ty::Opt(v.clone())),
            "keys" => Some(Ty::List(k.clone())),
            "values" => Some(Ty::List(v.clone())),
            _ => None,
        },
        Ty::Set(_elem) => match method {
            "len" | "length" | "count" => Some(Ty::Named("Int".into())),
            "is_empty" | "contains" => Some(Ty::Named("Bool".into())),
            "insert" | "remove" | "extend" => Some(Ty::Unit),
            _ => None,
        },
        _ => None,
    }
}

/// Language conversions: Str ↔ Bytes ↔ Blob. Used from receiver and Type.method.
fn conversion_result(recv: &Ty, method: &str) -> Option<Ty> {
    match method {
        "as_bytes" | "to_bytes" | "into_bytes" if is_str_ty(recv) || is_blob_ty(recv) => {
            Some(Ty::Named("Bytes".into()))
        }
        "to_str" | "as_str" | "to_string" if is_bytes_ty(recv) || is_blob_ty(recv) => {
            Some(Ty::Named("Str".into()))
        }
        // Blob.as_ref() is a bytes view in Rust; VEIL Str context decodes utf-8.
        "as_ref" if is_blob_ty(recv) => Some(Ty::Named("Str".into())),
        "parse_int" if is_str_ty(recv) => Some(Ty::Named("Int".into())),
        "parse_json" if is_str_ty(recv) => Some(Ty::Named("Json".into())),
        "as_str" | "as_s" if is_json_ty(recv) => Some(Ty::Opt(Box::new(Ty::Named("Str".into())))),
        "as_n" => Some(Ty::Named("Int".into())),
        _ => None,
    }
}

/// Unwrap return type for a bang `!` call under **ACS-010 portable law** (engine default):
/// strip Res only — `Opt` stays `Opt`. Force-present is explicit `.unwrap()` /
/// `require`, not an invisible side effect of `!`.
///
/// Dual-loop Rust codegen matches: `.await?` only; no automatic `.ok_or(NotFound)?`.
/// Obsolete ACS-001 helper: [`unwrap_bang_return_transitional`] (tests/comparison only).
fn unwrap_bang_return(ty: Ty) -> Ty {
    unwrap_bang_return_portable(ty)
}

/// **Obsolete ACS-001 transitional:** Res!<Opt<T>> → T, Res!<T> → T, Opt<T> → T.
/// Not used by the typechecker default. Kept for comparison tests only.
pub fn unwrap_bang_return_transitional(ty: Ty) -> Ty {
    let inner = match ty {
        Ty::Res(Some(t)) => *t,
        Ty::Res(None) => Ty::Unit,
        other => other,
    };
    match inner {
        Ty::Opt(t) => *t,
        other => other,
    }
}

/// ACS-010 portable law (current default): bang = try/Res only. Opt stays Opt.
/// Force-present is a separate construct (`require` / `.unwrap()` / layer policy) — not `!`.
/// Res!<Opt<T>> → Opt<T>, Res!<T> → T, Opt<T> → Opt<T>.
pub fn unwrap_bang_return_portable(ty: Ty) -> Ty {
    match ty {
        Ty::Res(Some(t)) => *t,
        Ty::Res(None) => Ty::Unit,
        other => other,
    }
}

fn infer_call(
    call: &CallExpr,
    scope: &mut Scope,
    env: &TypeEnv,
    self_type: Option<&str>,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Ty {
    // Infer args first
    let arg_tys: Vec<Ty> = call
        .args
        .iter()
        .map(|a| infer_expr(a, scope, env, self_type, location, diagnostics))
        .collect();

    if let Some(recv) = &call.receiver {
        let recv_ty = infer_expr(recv, scope, env, self_type, location, diagnostics);
        let method = call.method.trim_end_matches('!');
        if let Some(ty) = conversion_result(&recv_ty, method) {
            return ty;
        }
        if let Some(ty) = collection_method_ty(&recv_ty, method) {
            return ty;
        }
        let type_name = match &recv_ty {
            Ty::Named(t) => Some(t.clone()),
            _ => None,
        };
        let is_bang = call.method.ends_with('!');
        if let Some(type_name) = type_name {
            if let Some(sig) = env.types.get(&type_name).and_then(|i| i.methods.get(method)) {
                check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                let ret = sig.ret.clone();
                // Bang: codegen emits `?` (Res!) and often `.ok_or()?` (Opt)
                if is_bang {
                    return unwrap_bang_return(ret);
                }
                return ret;
            }
            if env.ambiguous_stub_types.contains(&type_name) {
                diagnostics.push(diag(
                    Severity::Error,
                    "ambiguous_stub_type",
                    format!(
                        "stub type '{type_name}' is defined by more than one crate — qualify it"
                    ),
                    location,
                    Some(call.span),
                    Some("e.g. aws_sdk_sns.Client / aws_sdk_sns.MessageAttributeValue".into()),
                ));
            }
        }
        return Ty::Unknown;
    }

    // Intrinsic
    if call.method.is_empty() && matches!(call.target.as_str(), "now" | "now!") {
        return Ty::Named("Dt".into());
    }
    if call.method.is_empty() && call.target.trim_end_matches('!') == "env" {
        return Ty::Named("Str".into());
    }

    // self.field.method as target "self.pool"
    if let Some(rest) = call.target.strip_prefix("self.") {
        let field = rest.split('.').next().unwrap_or(rest);
        let fty = self_type
            .and_then(|st| env.types.get(st))
            .and_then(|i| i.fields.get(field).cloned())
            .or_else(|| {
                // related struct fields already in scope
                match scope.get(field) {
                    Ty::Unknown => None,
                    t => Some(t),
                }
            });
        if let Some(Ty::Named(type_name)) = fty {
            let method = call.method.trim_end_matches('!');
            let is_bang = call.method.ends_with('!');
            if let Some(sig) = env.types.get(&type_name).and_then(|i| i.methods.get(method)) {
                check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                let ret = sig.ret.clone();
                if is_bang {
                    return unwrap_bang_return(ret);
                }
                return ret;
            }
        }
        return Ty::Unknown;
    }

    // Local.method (incl. List/Opt/Res intrinsics)
    if scope.locals.contains_key(&call.target) && !call.method.is_empty() {
        let method = call.method.trim_end_matches('!');
        let is_bang = call.method.ends_with('!');
        match scope.get(&call.target) {
            Ty::Named(type_name) => {
                if let Some(ty) = conversion_result(&Ty::Named(type_name.clone()), method) {
                    return ty;
                }
                if let Some(sig) = env.types.get(&type_name).and_then(|i| i.methods.get(method)) {
                    check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                    let ret = sig.ret.clone();
                    // Bang calls: codegen adds `?` (Res only; Opt preserved — ACS-010)
                    if is_bang {
                        return unwrap_bang_return(ret);
                    }
                    return ret;
                }
            }
            Ty::List(_) | Ty::Map(_, _) | Ty::Set(_) => {
                return collection_method_ty(&scope.get(&call.target), method)
                    .unwrap_or(Ty::Unknown);
            }
            Ty::Opt(inner) => {
                return match method {
                    "unwrap" | "expect" => *inner,
                    "is_some" | "is_none" => Ty::Named("Bool".into()),
                    _ => Ty::Unknown,
                };
            }
            Ty::Res(inner) => {
                return match method {
                    "unwrap" | "expect" => inner.map(|b| *b).unwrap_or(Ty::Unit),
                    "is_ok" | "is_err" => Ty::Named("Bool".into()),
                    _ => Ty::Unknown,
                };
            }
            // Opt methods on non-Opt (ACS-010: bang no longer forces Opt→T)
            other
                if matches!(
                    method,
                    "is_some" | "is_none" | "unwrap_or" | "unwrap_or_else"
                ) && !matches!(other, Ty::Unknown) =>
            {
                diagnostics.push(diag(
                    Severity::Error,
                    "opt_method_on_non_opt",
                    format!(
                        "method '{method}' requires Opt<_>, found {}",
                        other.display()
                    ),
                    location,
                    Some(call.span),
                    Some(
                        "bang (!) only unwraps Res! — Opt stays Opt. \
                         Call is_some/is_none on Opt, or .unwrap() to force T."
                            .into(),
                    ),
                ));
                return Ty::Unknown;
            }
            _ => {}
        }
        return Ty::Unknown;
    }

    // Free function
    if call.method.is_empty() {
        // Result constructors
        if call.target == "Ok" {
            return Ty::Res(arg_tys.first().cloned().map(Box::new));
        }
        if call.target == "Err" {
            return Ty::Res(None);
        }
        if let Some(sig) = env.free_fns.get(&call.target) {
            check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
            return sig.ret.clone();
        }
        // Construct used as fn? shape Fn construct
        if let Some(info) = env.types.get(&call.target) {
            // Prefer method "" no - it's a fn-shaped construct invoked by name
            if let Some(sig) = info.methods.get(&call.target) {
                check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                return sig.ret.clone();
            }
        }
    }

    // Type.method / Port.method
    if let Some(info) = env.types.get(&call.target) {
        if call.method.is_empty() {
            // Type() constructor-like
            return Ty::Named(call.target.clone());
        }
        let method = call.method.trim_end_matches('!');
        let is_bang = call.method.ends_with('!');
        if method == "new" {
            // Blob.new(Str) is utf-8 sugar for Blob.new(Bytes.from_str(s)).
            if is_blob_name(&call.target)
                && arg_tys.len() == 1
                && (is_str_ty(&arg_tys[0]) || is_bytes_ty(&arg_tys[0]) || arg_tys[0].is_unknown())
            {
                return info
                    .methods
                    .get("new")
                    .map(|s| s.ret.clone())
                    .unwrap_or_else(|| Ty::Named(call.target.clone()));
            }
            if let Some(sig) = info.methods.get("new") {
                // Synthetic VEIL-struct new() has no params — skip arity.
                // Stub constructors (`Blob.new(data: Bytes)`) must check args.
                if !sig.params.is_empty() {
                    check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                }
                return sig.ret.clone();
            }
            return Ty::Named(call.target.clone());
        }
        if is_blob_name(&call.target) {
            if let Some(ty) = conversion_result(&Ty::Named("Blob".into()), method) {
                return ty;
            }
        }
        if call.target == "Bytes"
            || call.target == "Str"
            || call.target == "Dt"
            || call.target == "DateTime"
            || call.target == "Int"
            || call.target == "Json"
        {
            match (call.target.as_str(), method) {
                ("Bytes", "from_str") | ("Bytes", "new") if arg_tys.len() == 1 => {
                    return Ty::Named("Bytes".into());
                }
                ("Str", "from_bytes") | ("Str", "from_utf8") if arg_tys.len() == 1 => {
                    return Ty::Named("Str".into());
                }
                ("Str", "now_iso8601")
                | ("Dt", "now_iso8601")
                | ("DateTime", "now_iso8601")
                    if arg_tys.is_empty() =>
                {
                    return Ty::Named("Str".into());
                }
                ("Int", "now_unix") | ("Int", "now") if arg_tys.is_empty() => {
                    return Ty::Named("Int".into());
                }
                ("Json", "parse") if arg_tys.len() == 1 => {
                    return Ty::Named("Json".into());
                }
                ("Json", "stringify") if arg_tys.len() == 1 => {
                    return Ty::Named("Str".into());
                }
                _ => {}
            }
        }
        if let Some(sig) = info.methods.get(method) {
            check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
            let ret = sig.ret.clone();
            // Bang: ACS-010 portable — strip Res only (Opt preserved)
            if is_bang {
                return unwrap_bang_return(ret);
            }
            return ret;
        }
    } else if env.ambiguous_stub_types.contains(&call.target) {
        diagnostics.push(diag(
            Severity::Error,
            "ambiguous_stub_type",
            format!(
                "stub type '{}' is defined by more than one crate — qualify it",
                call.target
            ),
            location,
            Some(call.span),
            Some("e.g. aws_sdk_sns.MessageAttributeValue.builder()".into()),
        ));
    }

    Ty::Unknown
}

fn check_args(
    sig: &MethodSig,
    arg_tys: &[Ty],
    location: &str,
    span: Option<Span>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if sig.params.is_empty() {
        // e.g. new() — don't enforce arity
        return;
    }
    if arg_tys.len() != sig.params.len() {
        let expected_sig = sig
            .param_names
            .iter()
            .zip(sig.params.iter())
            .map(|(n, t)| format!("{n}: {}", t.display()))
            .collect::<Vec<_>>()
            .join(", ");
        let json_bus = sig.params.len() == 1
            && matches!(sig.params.first(), Some(Ty::Named(n)) if n == "Json");
        let hint = if json_bus {
            Some("this method takes one Json argument".into())
        } else {
            Some(
                "stub_search the method. Fluent setters often take (key, stubValue), \
                 not a Map<Str, Str>. Incremental: .item(k, AttributeValue.S(v))"
                    .into(),
            )
        };
        diagnostics.push(diag(
            Severity::Error,
            "arg_count_mismatch",
            format!(
                "expected {} argument(s), found {} ({expected_sig})",
                sig.params.len(),
                arg_tys.len()
            ),
            location,
            span,
            hint,
        ));
        return;
    }
    for (i, (expected, actual)) in sig.params.iter().zip(arg_tys.iter()).enumerate() {
        if !compatible(expected, actual) {
            let pname = sig
                .param_names
                .get(i)
                .map(|s| s.as_str())
                .unwrap_or("?");
            let hint = match (expected, actual) {
                (Ty::Map(_, ev), Ty::Map(_, av)) if ev.as_ref() != av.as_ref() => Some(format!(
                    "stub maps use {} values — construct them (Type.Variant(x) or Type.builder()…build()), not Map<Str, Str>",
                    ev.display()
                )),
                (Ty::Named(n), _) if is_stub_type_param(n) => Some(format!(
                    "`{n}` is a rustdoc type parameter (impl Trait), not a stub type — pass Str / the value you have"
                )),
                (Ty::Named(n), _) if !is_veil_primitive_name(n) => Some(format!(
                    "stub_search '{n}' and construct it (e.g. {n}.S(s), {n}.builder()…build(), Blob.new(bytes))"
                )),
                _ => None,
            };
            diagnostics.push(diag(
                Severity::Error,
                "type_mismatch",
                format!(
                    "argument '{}' expected {}, found {}",
                    pname,
                    expected.display(),
                    actual.display()
                ),
                location,
                span,
                hint,
            ));
        }
    }
}

fn check_match_arms(
    scrut_ty: &Ty,
    arms: &[MatchArm],
    scope: &Scope,
    env: &TypeEnv,
    self_type: Option<&str>,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Ty {
    let variants: Option<&[String]> = match scrut_ty {
        Ty::Named(n) => env.types.get(n).map(|i| i.variants.as_slice()),
        _ => None,
    };

    let mut unified: Option<Ty> = None;
    let mut conflict = false;

    for arm in arms {
        let mut arm_scope = scope.child();
        if let Some(rp) = &arm.rich_pattern {
            bind_pattern_ty(rp, scrut_ty, &mut arm_scope);
            if let Some(vars) = variants {
                match rp {
                    Pattern::Ident(name)
                        if name != "_"
                            && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                            && !vars.iter().any(|x| x == name) =>
                    {
                        diagnostics.push(diag(
                            Severity::Error,
                            "unknown_variant",
                            format!("unknown variant '{}' for type {}", name, scrut_ty.display()),
                            location,
                            Some(arm.span),
                            Some(format!("variants: {}", vars.join(", "))),
                        ));
                    }
                    Pattern::Variant(name, _) if !vars.iter().any(|x| x == name) => {
                        diagnostics.push(diag(
                            Severity::Error,
                            "unknown_variant",
                            format!("unknown variant '{}' for type {}", name, scrut_ty.display()),
                            location,
                            Some(arm.span),
                            Some(format!("variants: {}", vars.join(", "))),
                        ));
                    }
                    _ => {}
                }
            }
        } else if let Some(vars) = variants {
            // String pattern — first token as variant if Capitalized
            let pat = arm.pattern.trim();
            let variant = pat.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
            if !variant.is_empty()
                && variant != "_"
                && variant.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                && !vars.iter().any(|x| x == variant)
            {
                diagnostics.push(diag(
                    Severity::Error,
                    "unknown_variant",
                    format!(
                        "unknown variant '{}' for type {}",
                        variant,
                        scrut_ty.display()
                    ),
                    location,
                    Some(arm.span),
                    Some(format!("variants: {}", vars.join(", "))),
                ));
            }
            bind_string_pattern(&arm.pattern, scrut_ty, &mut arm_scope);
        } else {
            bind_string_pattern(&arm.pattern, scrut_ty, &mut arm_scope);
        }

        if let Some(g) = &arm.guard {
            let gt = infer_expr(g, &mut arm_scope, env, self_type, location, diagnostics);
            if !gt.is_unknown() && !compatible(&Ty::Named("Bool".into()), &gt) {
                diagnostics.push(diag(
                    Severity::Error,
                    "type_mismatch",
                    format!("match guard expected Bool, found {}", gt.display()),
                    location,
                    Some(arm.span),
                    None,
                ));
            }
        }
        let mut arm_ty = Ty::Unit;
        let mut arm_returns = false;
        for e in &arm.body {
            arm_returns = matches!(e, Expr::Return(_));
            arm_ty = match e {
                Expr::Return(inner) => {
                    infer_expr(inner, &mut arm_scope, env, self_type, location, diagnostics)
                }
                other => infer_expr(other, &mut arm_scope, env, self_type, location, diagnostics),
            };
        }
        // `ret` leaves the match — do not unify with continuing arms
        // (`Ok x -> i = i + 1` vs `Err e -> ret Err(e)`).
        if arm_returns {
            continue;
        }
        match unified.take() {
            None => unified = Some(arm_ty),
            Some(prev) if prev.is_unknown() => unified = Some(arm_ty),
            Some(prev) if arm_ty.is_unknown() => unified = Some(prev),
            Some(prev) if compatible(&prev, &arm_ty) => unified = Some(prev),
            Some(prev) if compatible(&arm_ty, &prev) => unified = Some(arm_ty),
            Some(prev) => {
                conflict = true;
                diagnostics.push(diag(
                    Severity::Error,
                    "type_mismatch",
                    format!(
                        "match arms have incompatible types: {} vs {}",
                        prev.display(),
                        arm_ty.display()
                    ),
                    location,
                    Some(arm.span),
                    Some(
                        "every arm must produce the same type — wrap a Str in the domain value; \
                         a fire-and-forget arm that returns Opt<T> ends with `null` (not the unit call)"
                            .into(),
                    ),
                ));
                unified = Some(Ty::Unknown);
            }
        }
    }
    if conflict {
        Ty::Unknown
    } else {
        unified.unwrap_or(Ty::Unknown)
    }
}

fn bind_pattern_ty(pat: &Pattern, ty: &Ty, scope: &mut Scope) {
    match pat {
        Pattern::Ident(n) if n != "_" => {
            // Opt unwrap for Some(x)
            scope.bind(n, ty.clone());
        }
        Pattern::Tuple(parts) => {
            if let Ty::Tuple(ts) = ty {
                for (p, t) in parts.iter().zip(ts.iter()) {
                    bind_pattern_ty(p, t, scope);
                }
            } else {
                for p in parts {
                    bind_pattern_ty(p, &Ty::Unknown, scope);
                }
            }
        }
        Pattern::Struct(_, fields, _) => {
            for (name, inner) in fields {
                let ft = match ty {
                    Ty::Named(n) => {
                        // need env — unknown
                        let _ = n;
                        Ty::Unknown
                    }
                    _ => Ty::Unknown,
                };
                if let Some(p) = inner {
                    bind_pattern_ty(p, &ft, scope);
                } else {
                    scope.bind(name, ft);
                }
            }
        }
        Pattern::Variant(_, fields) => {
            for p in fields {
                bind_pattern_ty(p, &Ty::Unknown, scope);
            }
        }
        Pattern::Or(parts) => {
            for p in parts {
                bind_pattern_ty(p, ty, scope);
            }
        }
        _ => {}
    }
}

fn bind_string_pattern(pattern: &str, ty: &Ty, scope: &mut Scope) {
    // Some(x), Ok(v), bare idents
    for token in pattern.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if token.is_empty() || token == "_" {
            continue;
        }
        if token.chars().next().map(|c| c.is_lowercase() || c == '_').unwrap_or(false) {
            // unwrap Opt/Res for Some/Ok
            let inner = match ty {
                Ty::Opt(t) | Ty::Res(Some(t)) => t.as_ref().clone(),
                other => other.clone(),
            };
            scope.bind(token, inner);
        }
    }
}

// ─── Diagnostics ─────────────────────────────────────────────────────────────

fn return_mismatch_hint(expected: &Ty, actual: &Ty) -> String {
    match (expected, actual) {
        (Ty::Res(Some(inner)), act) if matches!(inner.as_ref(), Ty::Opt(_)) && is_unit_ty(act) => {
            "use `ret value` for Some and `ret null` (or `ret ()`) for None — then map stub fields into the domain value".into()
        }
        (Ty::Opt(_), act) if is_unit_ty(act) => {
            "use `ret value` for Some and `ret null` for None".into()
        }
        (Ty::Res(Some(inner)), Ty::Named(n))
            if matches!(inner.as_ref(), Ty::Named(e) if e == "Str") && is_blob_name(n) =>
        {
            "decode with `blob.to_str()` (utf-8) or `Str.from_bytes(blob.as_bytes())`".into()
        }
        (Ty::Named(e), Ty::Named(a)) if e == "Str" && is_blob_name(a) => {
            "decode with `blob.to_str()` (utf-8)".into()
        }
        _ => "map the value to the port type (do not return a stub output field as a domain value)"
            .into(),
    }
}

fn diag(
    severity: Severity,
    code: &str,
    message: String,
    location: &str,
    span: Option<Span>,
    hint: Option<String>,
) -> Diagnostic {
    Diagnostic {
        severity,
        message,
        node_id: None,
        node_name: Some(location.to_string()),
        code: code.to_string(),
        constraint: code.to_string(),
        parent: None,
        hint,
        span_start: span.map(|s| s.start),
        span_end: span.map(|s| s.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{ConstructSpec, Visual};
    use crate::span::Span;

    fn empty_visual() -> Visual {
        Visual {
            icon: String::new(),
            color: String::new(),
            label: String::new(),
        }
    }

    fn spec(kw: &str, name: &str, shape: Shape) -> ConstructSpec {
        ConstructSpec {
            name: name.to_string(),
            keyword: kw.to_string(),
            maps_to: shape.name().to_string(),
            shape,
            layer: "test".to_string(),
            desc: String::new(),
            contains: Vec::new(),
            blocks: Vec::new(),
            raw_block_keywords: Vec::new(),
            constraints: Vec::new(),
            allowed_in: "any".to_string(),
            group: String::new(),
            visual: empty_visual(),
            au: false,
                is_step: false,
                step_fields: Vec::new(),
            annotations: Vec::new(),
            runtime: None,
            tgt: String::new(),
            dg: String::new(),
            presentation: Default::default(),
            roles: Vec::new(),
            config_keys: Vec::new(),
        }
    }

    fn reg() -> LayerRegistry {
        let mut r = LayerRegistry::builtin();
        for s in [
            spec("port", "Port", Shape::Trait),
            spec("svc", "Service", Shape::Fn),
            spec("agg", "Aggregate", Shape::Struct),
            spec("enum", "Enum", Shape::Enum),
        ] {
            if let Some(i) = r.constructs.iter().position(|c| c.keyword == s.keyword) {
                r.constructs[i] = s;
            } else {
                r.constructs.push(s);
            }
        }
        r
    }

    fn sol(items: Vec<TopLevelItem>) -> Solution {
        Solution {
            name: "T".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            links: vec![],
            items,
            expose: None,
            guidance: Vec::new(),
        }
    }

    fn step(body: Vec<Expr>) -> FlowStep {
        FlowStep::Step(StepDef {
            name: "s".into(),
            span: Span::new(0, 0),
            body,
            refs: Vec::new(),
            sub_blocks: Vec::new(), kind: None, fields: Vec::new(), edges: Vec::new(),
        })
    }

    #[test]
    fn assignment_type_mismatch() {
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.steps.push(step(vec![
            Expr::MutAssign(
                "x".into(),
                Box::new(Expr::IntLit(1)),
                Some(TypeExpr::Named("Str".into())),
            ),
        ]));
        let diags = check_types(&sol(vec![TopLevelItem::Construct(svc)]), &reg());
        assert!(
            diags.iter().any(|d| d.code == "type_mismatch"),
            "{:?}",
            diags
        );
    }

    #[test]
    fn unnamed_struct_lit_is_map() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Sns".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "publish!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "attributes".into(),
                type_expr: TypeExpr::Map(
                    Box::new(TypeExpr::Named("Str".into())),
                    Box::new(TypeExpr::Named("Str".into())),
                ),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "AwsSns".into(),
            Span::new(0, 0),
        );
        ad.impls.push(MethodImpl {
            method_name: "publish".into(),
            params: Vec::new(),
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: "Sns".into(),
                method: "publish".into(),
                args: vec![Expr::StructLit(
                    String::new(),
                    vec![("event".into(), Expr::StringLit("x".into()))],
                )],
                receiver: None,
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &reg(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "bare {{ k: v }} must type as Map: {:?}",
            diags
        );
    }

    #[test]
    fn call_arg_type_and_count() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Repo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "save!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "user".into(),
                type_expr: TypeExpr::Named("User".into()),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.steps.push(step(vec![Expr::Call(CallExpr {
            target: "Repo".into(),
            method: "save".into(),
            args: vec![Expr::IntLit(1)],
            receiver: None,
            sugar: None,
            span: Span::new(10, 20),
        })]));
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg(),
        );
        assert!(
            diags.iter().any(|d| d.code == "type_mismatch" && d.message.contains("user")),
            "{:?}",
            diags
        );
    }

    /// ACS-010: portable (default) vs obsolete transitional unwrap helpers.
    #[test]
    fn bang_unwrap_transitional_vs_portable() {
        let res_opt = Ty::Res(Some(Box::new(Ty::Opt(Box::new(Ty::Named("User".into()))))));
        let res_t = Ty::Res(Some(Box::new(Ty::Named("User".into()))));
        let opt = Ty::Opt(Box::new(Ty::Named("User".into())));

        // Obsolete ACS-001 transitional: Opt forced to T
        assert_eq!(
            unwrap_bang_return_transitional(res_opt.clone()).display(),
            "User"
        );
        assert_eq!(unwrap_bang_return_transitional(opt.clone()).display(), "User");
        assert_eq!(unwrap_bang_return_transitional(res_t.clone()).display(), "User");

        // Current ACS-010 portable: bang strips Res only
        assert_eq!(
            unwrap_bang_return_portable(res_opt).display(),
            "Opt<User>"
        );
        assert_eq!(unwrap_bang_return_portable(opt).display(), "Opt<User>");
        assert_eq!(unwrap_bang_return_portable(res_t).display(), "User");
    }

    /// ACS-010 portable: bang keeps Opt — passing Opt to a param expecting T is mismatch.
    #[test]
    fn bang_call_keeps_opt_type_mismatch_without_unwrap() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Repo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "find!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "id".into(),
                type_expr: TypeExpr::Named("Id".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named("User".into())))),
        });
        port.methods.push(Method {
            name: "save!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "user".into(),
                type_expr: TypeExpr::Named("User".into()),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.inputs.push(Field {
            name: "repo".into(),
            type_expr: TypeExpr::Named("Repo".into()),
            span: Span::new(0, 0),
            annotations: Vec::new(),
            default_expr: None,
        });
        svc.inputs.push(Field {
            name: "id".into(),
            type_expr: TypeExpr::Named("Id".into()),
            span: Span::new(0, 0),
            annotations: Vec::new(),
            default_expr: None,
        });
        svc.steps.push(step(vec![
            Expr::Assign(
                "u".into(),
                Box::new(Expr::Call(CallExpr {
                    target: "repo".into(),
                    method: "find!".into(),
                    args: vec![Expr::Ident("id".into())],
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 0),
                })),
                None,
            ),
            Expr::Call(CallExpr {
                target: "repo".into(),
                method: "save!".into(),
                args: vec![Expr::Ident("u".into())],
                receiver: None,
                sugar: None,
                span: Span::new(10, 20),
            }),
        ]));
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg(),
        );
        assert!(
            diags.iter().any(|d| d.code == "type_mismatch"),
            "portable bang keeps Opt — save(User) with Opt should mismatch: {:?}",
            diags
        );
    }

    /// ACS-010: after find!, is_some is valid (value is still Opt).
    #[test]
    fn bang_call_portable_allows_is_some_on_opt() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Repo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "find!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "id".into(),
                type_expr: TypeExpr::Named("Id".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named("User".into())))),
        });
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.inputs.push(Field {
            name: "repo".into(),
            type_expr: TypeExpr::Named("Repo".into()),
            span: Span::new(0, 0),
            annotations: Vec::new(),
            default_expr: None,
        });
        svc.inputs.push(Field {
            name: "id".into(),
            type_expr: TypeExpr::Named("Id".into()),
            span: Span::new(0, 0),
            annotations: Vec::new(),
            default_expr: None,
        });
        svc.steps.push(step(vec![
            Expr::Assign(
                "u".into(),
                Box::new(Expr::Call(CallExpr {
                    target: "repo".into(),
                    method: "find!".into(),
                    args: vec![Expr::Ident("id".into())],
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 0),
                })),
                None,
            ),
            Expr::IfExpr(IfExprData {
                condition: Box::new(Expr::Call(CallExpr {
                    target: "u".into(),
                    method: "is_some".into(),
                    args: vec![],
                    receiver: None,
                    sugar: None,
                    span: Span::new(5, 10),
                })),
                then_body: vec![Expr::Return(Box::new(Expr::Call(CallExpr {
                    target: "u".into(),
                    method: "unwrap".into(),
                    args: vec![],
                    receiver: None,
                    sugar: None,
                    span: Span::new(10, 15),
                })))],
                else_body: None,
            }),
        ]));
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "opt_method_on_non_opt"),
            "is_some/unwrap on Opt after bang must be allowed: {:?}",
            diags
        );
    }

    #[test]
    fn try_on_non_result_errors() {
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.steps.push(step(vec![Expr::Try(Box::new(Expr::IntLit(1)))]));
        let diags = check_types(&sol(vec![TopLevelItem::Construct(svc)]), &reg());
        assert!(
            diags.iter().any(|d| d.code == "try_on_non_result"),
            "{:?}",
            diags
        );
    }

    #[test]
    fn try_on_res_ok() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Repo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "load!".into(),
            span: Span::new(0, 0),
            params: Vec::new(),
            return_type: Some(TypeExpr::Result(Some(Box::new(TypeExpr::Named("User".into()))))),
        });
        // load! with return Res already — also ! suffix
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.steps.push(step(vec![Expr::Try(Box::new(Expr::Call(CallExpr {
            target: "Repo".into(),
            method: "load".into(),
            args: Vec::new(),
            receiver: None,
            sugar: None,
            span: Span::new(0, 0),
        })))]));
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "try_on_non_result"),
            "{:?}",
            diags
        );
    }

    #[test]
    fn match_unknown_variant() {
        let mut en = Construct::new("enum", "Enum", Shape::Enum, "Status".into(), Span::new(0, 0));
        en.variants = vec!["Pending".into(), "Active".into()];
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.steps.push(step(vec![
            Expr::MutAssign(
                "s".into(),
                Box::new(Expr::Ident("Pending".into())),
                Some(TypeExpr::Named("Status".into())),
            ),
            Expr::Match(
                Box::new(Expr::Ident("s".into())),
                vec![MatchArm {
                    pattern: "Nope".into(),
                    rich_pattern: Some(Pattern::Ident("Nope".into())),
                    guard: None,
                    span: Span::new(1, 5),
                    body: vec![Expr::IntLit(0)],
                }],
            ),
        ]));
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(en),
                TopLevelItem::Construct(svc),
            ]),
            &reg(),
        );
        assert!(
            diags.iter().any(|d| d.code == "unknown_variant"),
            "{:?}",
            diags
        );
    }

    #[test]
    fn bare_field_convention_and_ambiguous() {
        let mut agg = Construct::new(
            "agg",
            "Aggregate",
            Shape::Struct,
            "User".into(),
            Span::new(0, 0),
        );
        // conventional
        agg.fields.push(Field {
            annotations: Vec::new(),
            name: "email".into(),
            type_expr: TypeExpr::Named("email".into()),
            default_expr: None,
            span: Span::new(0, 0),
        });
        // ambiguous lowercase bare
        agg.fields.push(Field {
            annotations: Vec::new(),
            name: "xyzzy".into(),
            type_expr: TypeExpr::Named("xyzzy".into()),
            default_expr: None,
            span: Span::new(0, 0),
        });
        let diags = check_types(&sol(vec![TopLevelItem::Construct(agg)]), &reg());
        assert!(
            diags
                .iter()
                .any(|d| d.code == "ambiguous_field_type" && d.message.contains("xyzzy")),
            "{:?}",
            diags
        );
        assert!(
            !diags
                .iter()
                .any(|d| d.code == "ambiguous_field_type" && d.message.contains("email")),
            "email should infer Str: {:?}",
            diags
        );
    }

    #[test]
    fn infer_field_conventions() {
        assert_eq!(
            infer_field_ty_from_name("id"),
            Some(Ty::Named("Id".into()))
        );
        assert_eq!(
            infer_field_ty_from_name("created"),
            Some(Ty::Named("Dt".into()))
        );
        assert_eq!(
            infer_field_ty_from_name("is_active"),
            Some(Ty::Named("Bool".into()))
        );
        assert!(infer_field_ty_from_name("xyzzy").is_none());
    }

    fn stub_put_item() -> crate::layer::StubCrate {
        let src = r#"
stub example-sdk 1.0.0
types_module types
root_types Client

  struct Client
    fn put_item() -> PutItemFluentBuilder

  struct PutItemFluentBuilder
    fn table_name(input: Str) -> Self
    fn item(k: Str, v: AttributeValue) -> Self
    fn set_item(input: Opt<HashMap<Str, AttributeValue>>) -> Self
    fn send() -> Res!<PutItemOutput>

  struct PutItemOutput

  enum AttributeValue
    S(Str)
    N(Str)
"#;
        crate::layer::parse_stub_file(src).expect("stub")
    }

    fn ddd_reg_with_stub() -> LayerRegistry {
        let mut r = LayerRegistry::builtin();
        r.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        r.stubs.push(stub_put_item());
        r
    }

    #[test]
    fn stub_set_item_rejects_map_of_str() {
        let mut adapter = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRepo".into(),
            Span::new(0, 0),
        );
        adapter.annotations.push(Annotation {
            name: "field".into(),
            args: vec!["ddb: example_sdk.Client".into()],
            span: Span::new(0, 0),
        });
        adapter.impls.push(MethodImpl {
            method_name: "save".into(),
            params: vec!["id".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: String::new(),
                method: "set_item".into(),
                args: vec![Expr::StructLit(
                    String::new(),
                    vec![("pk".into(), Expr::StringLit("id-1".into()))],
                )],
                receiver: Some(Box::new(Expr::Call(CallExpr {
                    target: String::new(),
                    method: "put_item".into(),
                    args: Vec::new(),
                    receiver: Some(Box::new(Expr::FieldAccess(
                        Box::new(Expr::Ident("self".into())),
                        "ddb".into(),
                    ))),
                    sugar: None,
                    span: Span::new(0, 0),
                }))),
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        let diags = check_types(
            &sol(vec![TopLevelItem::Construct(adapter)]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags.iter().any(|d| d.code == "type_mismatch"
                && d.message.contains("AttributeValue")
                && d.message.contains("Str")),
            "Map<Str, Str> must not satisfy HashMap<Str, AttributeValue>: {diags:?}"
        );
    }

    #[test]
    fn stub_item_pair_accepts_attribute_value_s() {
        let mut adapter = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRepo".into(),
            Span::new(0, 0),
        );
        adapter.annotations.push(Annotation {
            name: "field".into(),
            args: vec!["ddb: example_sdk.Client".into()],
            span: Span::new(0, 0),
        });
        adapter.impls.push(MethodImpl {
            method_name: "save".into(),
            params: vec!["id".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: String::new(),
                method: "item".into(),
                args: vec![
                    Expr::StringLit("id".into()),
                    Expr::Call(CallExpr {
                        target: "AttributeValue".into(),
                        method: "S".into(),
                        args: vec![Expr::Ident("id".into())],
                        receiver: None,
                        sugar: None,
                        span: Span::new(0, 0),
                    }),
                ],
                receiver: Some(Box::new(Expr::Call(CallExpr {
                    target: String::new(),
                    method: "put_item".into(),
                    args: Vec::new(),
                    receiver: Some(Box::new(Expr::FieldAccess(
                        Box::new(Expr::Ident("self".into())),
                        "ddb".into(),
                    ))),
                    sugar: None,
                    span: Span::new(0, 0),
                }))),
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        let diags = check_types(
            &sol(vec![TopLevelItem::Construct(adapter)]),
            &ddd_reg_with_stub(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch" || d.code == "arg_count_mismatch"),
            "AttributeValue.S + item(k,v) must type-check: {diags:?}"
        );
    }

    #[test]
    fn env_all_caps_ident_warns_to_use_self_field() {
        let mut adapter = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRepo".into(),
            Span::new(0, 0),
        );
        adapter.annotations.push(Annotation {
            name: "env".into(),
            args: vec!["TABLE_NAME".into()],
            span: Span::new(0, 0),
        });
        adapter.impls.push(MethodImpl {
            method_name: "save".into(),
            params: Vec::new(),
            span: Span::new(0, 0),
            body: vec![Expr::Ident("TABLE_NAME".into())],
        });
        let diags = check_types(
            &sol(vec![TopLevelItem::Construct(adapter)]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags.iter().any(|d| d.code == "env_use_self_field"
                && matches!(d.severity, Severity::Error)),
            "{diags:?}"
        );
    }

    #[test]
    fn unknown_all_caps_ident_is_error() {
        let mut adapter = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRepo".into(),
            Span::new(0, 0),
        );
        adapter.impls.push(MethodImpl {
            method_name: "save".into(),
            params: Vec::new(),
            span: Span::new(0, 0),
            body: vec![Expr::Ident("SNS_TOPIC_ARN".into())],
        });
        let diags = check_types(
            &sol(vec![TopLevelItem::Construct(adapter)]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags.iter().any(|d| d.code == "unknown_env_ident"),
            "{diags:?}"
        );
    }

    #[test]
    fn empty_env_annotation_is_error() {
        let mut adapter = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRepo".into(),
            Span::new(0, 0),
        );
        adapter.annotations.push(Annotation {
            name: "env".into(),
            args: Vec::new(),
            span: Span::new(0, 0),
        });
        let diags = check_types(
            &sol(vec![TopLevelItem::Construct(adapter)]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags.iter().any(|d| d.code == "annotation_missing_args"),
            "{diags:?}"
        );
    }

    #[test]
    fn bang_in_unit_port_method_is_error() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Bus".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "dispatch".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "envelope".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        port.methods.push(Method {
            name: "publish!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "msg".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "AwsBus".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Bus".into());
        ad.impls.push(MethodImpl {
            method_name: "dispatch".into(),
            params: vec!["envelope".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: "Bus".into(),
                method: "publish!".into(),
                args: vec![Expr::Ident("envelope".into())],
                receiver: None,
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags.iter().any(|d| d.code == "bang_in_unit_fn"),
            "{diags:?}"
        );
    }

    #[test]
    fn impl_return_map_is_not_domain_type() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Routes".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "get_route".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "name".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "RoutingEntry".into(),
            )))),
        });
        let mut store = Construct::new("port", "Port", Shape::Trait, "Items".into(), Span::new(0, 0));
        store.methods.push(Method {
            name: "get_item!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "key".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Map(
                Box::new(TypeExpr::Named("Str".into())),
                Box::new(TypeExpr::Named("AttributeValue".into())),
            )))),
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "DdbRoutes".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Routes".into());
        ad.impls.push(MethodImpl {
            method_name: "get_route".into(),
            params: vec!["name".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: "Items".into(),
                method: "get_item!".into(),
                args: vec![Expr::Ident("name".into())],
                receiver: None,
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(store),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags.iter().any(|d| d.code == "type_mismatch"
                && d.message.contains("RoutingEntry")
                && d.message.contains("Map")),
            "{diags:?}"
        );
    }

    #[test]
    fn stub_new_accepts_str_as_bytes_sugar() {
        let src = r#"
stub example-sdk 1.0.0
  struct Blob
    fn new(data: Bytes) -> Self
"#;
        let mut r = LayerRegistry::builtin();
        r.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        r.stubs.push(crate::layer::parse_stub_file(src).expect("stub"));
        let mut adapter = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "Lam".into(),
            Span::new(0, 0),
        );
        adapter.impls.push(MethodImpl {
            method_name: "invoke".into(),
            params: vec!["payload".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: "Blob".into(),
                method: "new".into(),
                args: vec![Expr::Ident("payload".into())],
                receiver: None,
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        adapter.impls[0].body.insert(
            0,
            Expr::Assign(
                "payload".into(),
                Box::new(Expr::StringLit("x".into())),
                Some(TypeExpr::Named("Str".into())),
            ),
        );
        let diags = check_types(&sol(vec![TopLevelItem::Construct(adapter)]), &r);
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "Blob.new(Str) is utf-8 sugar: {diags:?}"
        );
    }

    fn stub_get_item() -> crate::layer::StubCrate {
        let src = r#"
stub example-sdk 1.0.0
types_module types
root_types Client

  struct Client
    fn get_item() -> GetItemFluentBuilder

  struct GetItemFluentBuilder
    fn send() -> Res!<GetItemOutput>

  struct GetItemOutput
    fn item() -> Opt<HashMap<Str, AttributeValue>>

  enum AttributeValue
    S(Str)
    fn as_s() -> Res!<Str>
"#;
        crate::layer::parse_stub_file(src).expect("stub")
    }

    fn ddd_reg_with_get_item() -> LayerRegistry {
        let mut r = LayerRegistry::builtin();
        r.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        r.stubs.push(stub_get_item());
        r
    }

    #[test]
    fn stub_output_getter_field_is_not_domain_type() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Routes".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "get_route!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "name".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "Record".into(),
            )))),
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRoutes".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Routes".into());
        ad.annotations.push(Annotation {
            name: "field".into(),
            args: vec!["client: example_sdk.Client".into()],
            span: Span::new(0, 0),
        });
        ad.impls.push(MethodImpl {
            method_name: "get_route".into(),
            params: vec!["name".into()],
            span: Span::new(0, 0),
            body: vec![
                Expr::Assign(
                    "result".into(),
                    Box::new(Expr::Call(CallExpr {
                        target: String::new(),
                        method: "send!".into(),
                        args: Vec::new(),
                        receiver: Some(Box::new(Expr::Call(CallExpr {
                            target: String::new(),
                            method: "get_item".into(),
                            args: Vec::new(),
                            receiver: Some(Box::new(Expr::FieldAccess(
                                Box::new(Expr::Ident("self".into())),
                                "client".into(),
                            ))),
                            sugar: None,
                            span: Span::new(0, 0),
                        }))),
                        sugar: None,
                        span: Span::new(0, 0),
                    })),
                    None,
                ),
                Expr::Return(Box::new(Expr::FieldAccess(
                    Box::new(Expr::Ident("result".into())),
                    "item".into(),
                ))),
            ],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_get_item(),
        );
        assert!(
            diags.iter().any(|d| d.code == "type_mismatch"
                && d.message.contains("Record")
                && d.message.contains("Map")),
            "{diags:?}"
        );
    }

    #[test]
    fn match_arms_incompatible_types_are_error() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Worker".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "run!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "mode".into(),
                type_expr: TypeExpr::Named("Mode".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "Token".into(),
            )))),
        });
        let mut mode = Construct::new("enum", "Enum", Shape::Enum, "Mode".into(), Span::new(0, 0));
        mode.variants = vec!["Fast".into(), "Slow".into()];
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "MemWorker".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Worker".into());
        ad.impls.push(MethodImpl {
            method_name: "run".into(),
            params: vec!["mode".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Match(
                Box::new(Expr::Ident("mode".into())),
                vec![
                    MatchArm {
                        pattern: "Fast".into(),
                        rich_pattern: None,
                        guard: None,
                        span: Span::new(0, 0),
                        body: vec![Expr::StringLit("tok".into())],
                    },
                    MatchArm {
                        pattern: "Slow".into(),
                        rich_pattern: None,
                        guard: None,
                        span: Span::new(0, 0),
                        body: vec![Expr::Tuple(Vec::new())],
                    },
                ],
            )],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(mode),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == "type_mismatch" && d.message.contains("incompatible")),
            "{diags:?}"
        );
    }

    #[test]
    fn match_return_arm_does_not_conflict_with_unit_arm() {
        let mut mode = Construct::new("enum", "Enum", Shape::Enum, "Mode".into(), Span::new(0, 0));
        mode.variants = vec!["Fast".into(), "Slow".into()];
        let mut fn_ = Construct::new("fn", "Fn", Shape::Fn, "go".into(), Span::new(0, 0));
        fn_.fns.push(FnDef {
            name: "go".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "mode".into(),
                type_expr: TypeExpr::Named("Mode".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Result(None)),
            annotations: Vec::new(),
            body: vec![Expr::Match(
                Box::new(Expr::Ident("mode".into())),
                vec![
                    MatchArm {
                        pattern: "Fast".into(),
                        rich_pattern: None,
                        guard: None,
                        span: Span::new(0, 0),
                        body: vec![Expr::Assign(
                            "i".into(),
                            Box::new(Expr::IntLit(1)),
                            None,
                        )],
                    },
                    MatchArm {
                        pattern: "Slow".into(),
                        rich_pattern: None,
                        guard: None,
                        span: Span::new(0, 0),
                        body: vec![Expr::Return(Box::new(Expr::Call(CallExpr {
                            target: "Err".into(),
                            method: String::new(),
                            args: vec![Expr::StringLit("no".into())],
                            receiver: None,
                            sugar: None,
                            span: Span::new(0, 0),
                        })))],
                    },
                ],
            )],
            steps: Vec::new(),
            layer_provided: false,
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(mode),
                TopLevelItem::Construct(fn_),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            !diags
                .iter()
                .any(|d| d.code == "type_mismatch" && d.message.contains("incompatible")),
            "ret on one match arm must not conflict with a continuing arm: {diags:?}"
        );
    }

    #[test]
    fn unit_res_implicit_send_is_compatible() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Sink".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "put!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "id".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkSink".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Sink".into());
        ad.annotations.push(Annotation {
            name: "field".into(),
            args: vec!["ddb: example_sdk.Client".into()],
            span: Span::new(0, 0),
        });
        ad.impls.push(MethodImpl {
            method_name: "put".into(),
            params: vec!["id".into()],
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: String::new(),
                method: "send!".into(),
                args: Vec::new(),
                receiver: Some(Box::new(Expr::Call(CallExpr {
                    target: String::new(),
                    method: "put_item".into(),
                    args: Vec::new(),
                    receiver: Some(Box::new(Expr::FieldAccess(
                        Box::new(Expr::Ident("self".into())),
                        "ddb".into(),
                    ))),
                    sugar: None,
                    span: Span::new(0, 0),
                }))),
                sugar: None,
                span: Span::new(0, 0),
            })],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "implicit send!() last line of put! -> () must be allowed: {diags:?}"
        );
    }

    #[test]
    fn ret_unit_satisfies_opt_port() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Routes".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "get_route!".into(),
            span: Span::new(0, 0),
            params: Vec::new(),
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "Record".into(),
            )))),
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "MemRoutes".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Routes".into());
        ad.impls.push(MethodImpl {
            method_name: "get_route".into(),
            params: Vec::new(),
            span: Span::new(0, 0),
            body: vec![Expr::Return(Box::new(Expr::Tuple(Vec::new())))],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "ret () must mean None for Opt<T>: {diags:?}"
        );
    }

    #[test]
    fn blob_to_str_is_str() {
        let src = r#"
stub example-sdk 1.0.0
  struct Blob
    fn new(data: Bytes) -> Self
"#;
        let mut r = LayerRegistry::builtin();
        r.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        r.stubs.push(crate::layer::parse_stub_file(src).expect("stub"));
        let mut port = Construct::new("port", "Port", Shape::Trait, "Runner".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "run!".into(),
            span: Span::new(0, 0),
            params: Vec::new(),
            return_type: Some(TypeExpr::Named("Str".into())),
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRun".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Runner".into());
        ad.impls.push(MethodImpl {
            method_name: "run".into(),
            params: Vec::new(),
            span: Span::new(0, 0),
            body: vec![
                Expr::Assign(
                    "blob".into(),
                    Box::new(Expr::Call(CallExpr {
                        target: "Blob".into(),
                        method: "new".into(),
                        args: vec![Expr::StringLit("x".into())],
                        receiver: None,
                        sugar: None,
                        span: Span::new(0, 0),
                    })),
                    None,
                ),
                Expr::Return(Box::new(Expr::Call(CallExpr {
                    target: String::new(),
                    method: "to_str".into(),
                    args: Vec::new(),
                    receiver: Some(Box::new(Expr::Ident("blob".into()))),
                    sugar: None,
                    span: Span::new(0, 0),
                }))),
            ],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &r,
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "blob.to_str() must satisfy Str: {diags:?}"
        );
    }

    #[test]
    fn rustdoc_type_param_accepts_str() {
        let src = r#"
stub example-http 1.0.0
  struct Client
    fn post(url: U) -> RequestBuilder
  struct RequestBuilder
    fn body(body: T) -> Self
    fn send() -> Res!<Response>
  struct Response
    fn text() -> Res!<Str>
"#;
        let mut r = LayerRegistry::builtin();
        r.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        r.stubs.push(crate::layer::parse_stub_file(src).expect("stub"));
        let mut port = Construct::new("port", "Port", Shape::Trait, "Http".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "post!".into(),
            span: Span::new(0, 0),
            params: vec![
                Param {
                    name: "url".into(),
                    type_expr: TypeExpr::Named("Str".into()),
                    span: Span::new(0, 0),
                },
                Param {
                    name: "body".into(),
                    type_expr: TypeExpr::Named("Str".into()),
                    span: Span::new(0, 0),
                },
            ],
            return_type: Some(TypeExpr::Named("Str".into())),
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "Req".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Http".into());
        ad.annotations.push(Annotation {
            name: "field".into(),
            args: vec!["http: example-http.Client".into()],
            span: Span::new(0, 0),
        });
        ad.impls.push(MethodImpl {
            method_name: "post".into(),
            params: vec!["url".into(), "body".into()],
            span: Span::new(0, 0),
            body: vec![
                Expr::Call(CallExpr {
                    target: String::new(),
                    method: "post".into(),
                    args: vec![Expr::Ident("url".into())],
                    receiver: Some(Box::new(Expr::FieldAccess(
                        Box::new(Expr::Ident("self".into())),
                        "http".into(),
                    ))),
                    sugar: None,
                    span: Span::new(0, 0),
                }),
                Expr::Return(Box::new(Expr::Ident("body".into()))),
            ],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &r,
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "reqwest-style U/T params must accept Str: {diags:?}"
        );
    }

    #[test]
    fn str_now_iso8601_is_str() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Clock".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "stamp!".into(),
            span: Span::new(0, 0),
            params: Vec::new(),
            return_type: Some(TypeExpr::Named("Str".into())),
        });
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SysClock".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Clock".into());
        ad.impls.push(MethodImpl {
            method_name: "stamp".into(),
            params: Vec::new(),
            span: Span::new(0, 0),
            body: vec![Expr::Return(Box::new(Expr::Call(CallExpr {
                target: "Str".into(),
                method: "now_iso8601".into(),
                args: Vec::new(),
                receiver: None,
                sugar: None,
                span: Span::new(0, 0),
            })))],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_stub(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "Str.now_iso8601() must be Str: {diags:?}"
        );
    }

    #[test]
    fn deploy_context_fields_and_json_parse_typecheck() {
        let mut r = LayerRegistry::builtin();
        r.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        let _ = r.load_content("deploy", include_str!("../../../layers/deploy.layer"));
        let mut hook = Construct::new(
            "hook",
            "DeployHook",
            Shape::Fn,
            "OnDeploy".into(),
            Span::new(0, 0),
        );
        hook.inputs.push(Field {
            annotations: Vec::new(),
            name: "context".into(),
            type_expr: TypeExpr::Named("DeployContext".into()),
            default_expr: None,
            span: Span::new(0, 0),
        });
        hook.steps.push(step(vec![
            Expr::MutAssign(
                "name".into(),
                Box::new(Expr::FieldAccess(
                    Box::new(Expr::Ident("context".into())),
                    "service_name".into(),
                )),
                Some(TypeExpr::Named("Str".into())),
            ),
            Expr::MutAssign(
                "parsed".into(),
                Box::new(Expr::Call(CallExpr {
                    target: "Json".into(),
                    method: "parse".into(),
                    args: vec![Expr::StringLit("{}".into())],
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 0),
                })),
                Some(TypeExpr::Named("Json".into())),
            ),
        ]));
        let diags = check_types(&sol(vec![TopLevelItem::Construct(hook)]), &r);
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "DeployContext.service_name and Json.parse must typecheck: {diags:?}"
        );
    }

    #[test]
    fn map_get_and_index_are_value_type() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Routes".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "get_route!".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "name".into(),
                type_expr: TypeExpr::Named("Str".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "Str".into(),
            )))),
        });
        let get = |recv: Expr, method: &str, args: Vec<Expr>| {
            Expr::Call(CallExpr {
                target: String::new(),
                method: method.into(),
                args,
                receiver: Some(Box::new(recv)),
                sugar: None,
                span: Span::new(0, 0),
            })
        };
        let mut ad = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "SdkRoutes".into(),
            Span::new(0, 0),
        );
        ad.target = Some("Routes".into());
        ad.annotations.push(Annotation {
            name: "field".into(),
            args: vec!["client: example_sdk.Client".into()],
            span: Span::new(0, 0),
        });
        ad.impls.push(MethodImpl {
            method_name: "get_route".into(),
            params: vec!["name".into()],
            span: Span::new(0, 0),
            body: vec![
                Expr::Assign(
                    "result".into(),
                    Box::new(get(
                        get(
                            Expr::FieldAccess(Box::new(Expr::Ident("self".into())), "client".into()),
                            "get_item",
                            Vec::new(),
                        ),
                        "send!",
                        Vec::new(),
                    )),
                    None,
                ),
                Expr::Assign(
                    "map".into(),
                    Box::new(Expr::Require(Box::new(get(
                        Expr::Ident("result".into()),
                        "item",
                        Vec::new(),
                    )))),
                    None,
                ),
                Expr::Return(Box::new(get(
                    get(
                        Expr::Ident("map".into()),
                        "get",
                        vec![Expr::StringLit("endpoint".into())],
                    ),
                    "as_s!",
                    Vec::new(),
                ))),
            ],
        });
        let diags = check_types(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(ad),
            ]),
            &ddd_reg_with_get_item(),
        );
        assert!(
            !diags.iter().any(|d| d.code == "type_mismatch"),
            "map.get(\"k\").as_s() must be Str: {diags:?}"
        );
    }
}
