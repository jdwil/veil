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

use std::collections::HashMap;

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
        TypeExpr::Named(n) if n.is_empty() => Ty::Unknown,
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
        TypeExpr::Tuple(items) => Ty::Tuple(items.iter().map(ty_from_type_expr).collect()),
        TypeExpr::Array(t, _) => Ty::List(Box::new(ty_from_type_expr(t))),
        TypeExpr::Ref(t, _) | TypeExpr::Dyn(t) | TypeExpr::ImplTrait(t) => ty_from_type_expr(t),
        TypeExpr::FnPtr(_, ret) => ret
            .as_ref()
            .map(|t| ty_from_type_expr(t))
            .unwrap_or(Ty::Unit),
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

/// Are two types compatible for assignment / arg passing?
fn compatible(expected: &Ty, actual: &Ty) -> bool {
    if expected.is_unknown() || actual.is_unknown() {
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
        (Ty::Opt(e), a) => compatible(e, a),
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
    /// Enum variant names (unit / data)
    variants: Vec<String>,
}

#[derive(Debug, Default)]
struct TypeEnv {
    types: HashMap<String, TypeInfo>,
    free_fns: HashMap<String, MethodSig>,
}

fn build_type_env(sol: &Solution, _registry: &LayerRegistry) -> TypeEnv {
    let mut env = TypeEnv::default();
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => index_construct_types(c, &mut env),
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

fn method_sig_from_params(params: &[(String, TypeExpr)], ret: Option<&TypeExpr>) -> MethodSig {
    MethodSig {
        param_names: params.iter().map(|(n, _)| n.clone()).collect(),
        params: params.iter().map(|(_, t)| ty_from_type_expr(t)).collect(),
        ret: ret.map(ty_from_type_expr).unwrap_or(Ty::Unit),
    }
}

fn index_construct_types(c: &Construct, env: &mut TypeEnv) {
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
        index_construct_types(child, env);
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
                check_construct_types(c, &env, &mut diagnostics);
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

    diagnostics
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

fn check_construct_types(c: &Construct, env: &TypeEnv, diagnostics: &mut Vec<Diagnostic>) {
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
        for p in &imp.params {
            scope.bind(p, Ty::Unknown);
        }
        for e in &imp.body {
            infer_expr(e, &mut scope, env, Some(&c.name), &c.name, diagnostics);
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
        check_construct_types(child, env, diagnostics);
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
            check_match_arms(&scrut_ty, &m.arms, scope, env, location, diagnostics);
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
        Expr::Ident(name) => {
            if name == "self" {
                return self_type
                    .map(|s| Ty::Named(s.to_string()))
                    .unwrap_or(Ty::Unknown);
            }
            scope.get(name)
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
            check_match_arms(&st, arms, scope, env, location, diagnostics);
            Ty::Unknown
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
            match bt {
                Ty::List(e) => *e,
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
        Ty::Named(n) => env
            .types
            .get(n)
            .and_then(|info| info.fields.get(field).cloned())
            .unwrap_or(Ty::Unknown),
        Ty::Opt(inner) => field_ty_of(inner, field, env),
        _ => Ty::Unknown,
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
        let _ = infer_expr(recv, scope, env, self_type, location, diagnostics);
        let base = match recv.as_ref() {
            Expr::Ident(n) if n == "self" => self_type.map(|s| s.to_string()),
            Expr::Ident(n) => match scope.get(n) {
                Ty::Named(t) => Some(t),
                _ => None,
            },
            Expr::FieldAccess(inner, field) => {
                let bt = infer_expr(inner, scope, env, self_type, location, diagnostics);
                match field_ty_of(&bt, field, env) {
                    Ty::Named(t) => Some(t),
                    _ => None,
                }
            }
            _ => None,
        };
        let method = call.method.trim_end_matches('!');
        if let Some(type_name) = base {
            if let Some(sig) = env.types.get(&type_name).and_then(|i| i.methods.get(method)) {
                check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                return sig.ret.clone();
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
            if let Some(sig) = env.types.get(&type_name).and_then(|i| i.methods.get(method)) {
                check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                return sig.ret.clone();
            }
        }
        return Ty::Unknown;
    }

    // Local.method
    if scope.locals.contains_key(&call.target) && !call.method.is_empty() {
        if let Ty::Named(type_name) = scope.get(&call.target) {
            let method = call.method.trim_end_matches('!');
            if let Some(sig) = env.types.get(&type_name).and_then(|i| i.methods.get(method)) {
                check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
                return sig.ret.clone();
            }
        }
        return Ty::Unknown;
    }

    // Free function
    if call.method.is_empty() {
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
        if method == "new" {
            if let Some(sig) = info.methods.get("new") {
                // skip strict arg check for new
                return sig.ret.clone();
            }
            return Ty::Named(call.target.clone());
        }
        if let Some(sig) = info.methods.get(method) {
            check_args(sig, &arg_tys, location, Some(call.span), diagnostics);
            return sig.ret.clone();
        }
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
        diagnostics.push(diag(
            Severity::Error,
            "arg_count_mismatch",
            format!(
                "expected {} argument(s), found {}",
                sig.params.len(),
                arg_tys.len()
            ),
            location,
            span,
            None,
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
                None,
            ));
        }
    }
}

fn check_match_arms(
    scrut_ty: &Ty,
    arms: &[MatchArm],
    scope: &Scope,
    env: &TypeEnv,
    location: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let variants: Option<&[String]> = match scrut_ty {
        Ty::Named(n) => env.types.get(n).map(|i| i.variants.as_slice()),
        _ => None,
    };

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
            let gt = infer_expr(g, &mut arm_scope, env, None, location, diagnostics);
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
        for e in &arm.body {
            infer_expr(e, &mut arm_scope, env, None, location, diagnostics);
        }
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
            annotations: Vec::new(),
            runtime: None,
            tgt: String::new(),
            dg: String::new(),
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
            items,
            expose: None,
        }
    }

    fn step(body: Vec<Expr>) -> FlowStep {
        FlowStep::Step(StepDef {
            name: "s".into(),
            span: Span::new(0, 0),
            body,
            refs: Vec::new(),
            sub_blocks: Vec::new(),
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
}
