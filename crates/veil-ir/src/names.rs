//! Unresolved name / call detection (CHK-003).
//!
//! Reports errors when calls target unknown constructs/methods, or when type
//! names are neither builtins, aliases, defined constructs, nor stubs.
//!
//! Locals are tracked so bindings are not flagged as unresolved targets.
//! Method checks on locals are best-effort when the local's type is known.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostics::{Diagnostic, Severity};
use crate::layer::{LayerRegistry, Shape};

/// Built-in VEIL type names (target-agnostic).
const BUILTIN_TYPES: &[&str] = &[
    "Str", "String", "Int", "F64", "Bool", "Bytes", "UUID", "Id", "DateTime", "Dt",
    "List", "Map", "Set", "Opt", "Res", "Json", "Any", "Unit", "Self",
];

/// Built-in free functions / intrinsics (no construct definition required).
const BUILTIN_CALLS: &[&str] = &[
    "now", "env", "panic", "todo", "unreachable", "assert", "Ok", "Err",
];

/// Index of names visible in a package for resolution.
#[derive(Debug, Default)]
struct NameIndex {
    /// Construct name → info
    constructs: HashMap<String, ConstructInfo>,
    /// Type aliases: alias → target display (only name existence matters)
    type_aliases: HashSet<String>,
    /// Free function names (layer declare / top-level fn)
    free_fns: HashSet<String>,
    /// Stub type names
    stub_types: HashSet<String>,
    /// Stub type → methods
    stub_methods: HashMap<String, HashSet<String>>,
    /// All known names for suggestions
    all_names: Vec<String>,
}

#[derive(Debug, Clone)]
struct ConstructInfo {
    /// Callable methods (trait methods, nested fns, impl methods); `!` stripped
    methods: HashSet<String>,
    /// Field name → optional type name hint
    fields: HashMap<String, Option<String>>,
}

/// Check a solution for unresolved names and calls.
pub fn check_names(sol: &Solution, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let index = build_index(sol, registry);
    let mut diagnostics = Vec::new();

    // use lines: unknown layer/stub/package → warning (cross-package deferred)
    for u in &sol.uses {
        let known = registry.layers.iter().any(|l| l == &u.package_name)
            || registry.stubs.iter().any(|s| s.name == u.package_name || s.alias.as_deref() == Some(&u.package_name))
            || index.constructs.contains_key(&u.package_name);
        if !known {
            // Layers load into registry.layers; if use succeeded at parse, layer is known.
            // Flag only if truly absent (e.g. package import not loaded).
            if !registry.layers.iter().any(|l| l.eq_ignore_ascii_case(&u.package_name))
                && !registry.stubs.iter().any(|s| s.name == u.package_name)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "import '{}' is not a loaded layer or stub — cross-package resolution deferred",
                        u.package_name
                    ),
                    node_id: None,
                    node_name: None,
                    code: "unresolved_import".to_string(),
                    constraint: "unresolved_import".to_string(),
                    parent: None,
                    hint: Some(
                        "Place a local .veil package, .layer, or .stub next to the file, or load it via the resolver"
                            .into(),
                    ),
                    span_start: None,
                    span_end: None,
                });
            }
        }
    }

    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => {
                check_construct(c, "package", &index, registry, &mut diagnostics);
            }
            TopLevelItem::Function(f) => {
                let mut scope = Scope::new();
                for p in &f.params {
                    scope.bind(&p.name, type_name_hint(&p.type_expr));
                    check_type_expr(&p.type_expr, &f.name, &index, &mut diagnostics);
                }
                if let Some(rt) = &f.return_type {
                    check_type_expr(rt, &f.name, &index, &mut diagnostics);
                }
                for e in &f.body {
                    check_expr(e, &f.name, &mut scope, &index, None, &mut diagnostics);
                }
            }
            TopLevelItem::TypeAlias { name: _, target } => {
                check_type_expr(target, "type_alias", &index, &mut diagnostics);
            }
            TopLevelItem::Flow(flow) => {
                check_flow(flow, &index, &mut diagnostics);
            }
            _ => {}
        }
    }

    diagnostics
}

fn build_index(sol: &Solution, registry: &LayerRegistry) -> NameIndex {
    let mut index = NameIndex::default();

    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => index_construct(c, &mut index),
            TopLevelItem::Function(f) => {
                index.free_fns.insert(f.name.clone());
                index.all_names.push(f.name.clone());
            }
            TopLevelItem::TypeAlias { name, .. } => {
                index.type_aliases.insert(name.clone());
                index.all_names.push(name.clone());
            }
            _ => {}
        }
    }

    for stub in &registry.stubs {
        let crate_keys: Vec<String> = std::iter::once(stub.name.clone())
            .chain(stub.alias.iter().cloned())
            .collect();
        // Crate root is a known external namespace (`sqlx.…`)
        for ck in &crate_keys {
            index.stub_types.insert(ck.clone());
            index.all_names.push(ck.clone());
        }
        for s in &stub.structs {
            index.stub_types.insert(s.name.clone());
            index.all_names.push(s.name.clone());
            let methods: HashSet<String> = s
                .methods
                .iter()
                .map(|m| strip_bang(&m.name))
                .collect();
            index.stub_methods.insert(s.name.clone(), methods.clone());
            // Qualified path used in source: sqlx.Query
            for ck in &crate_keys {
                let q = format!("{}.{}", ck, s.name);
                index.stub_types.insert(q.clone());
                index.all_names.push(q.clone());
                index.stub_methods.insert(q, methods.clone());
            }
        }
        for imp in &stub.impls {
            index.stub_types.insert(imp.target.clone());
            index.all_names.push(imp.target.clone());
            let entry = index
                .stub_methods
                .entry(imp.target.clone())
                .or_default();
            for m in &imp.methods {
                entry.insert(strip_bang(&m.name));
            }
            let methods_snapshot: HashSet<String> = index
                .stub_methods
                .get(&imp.target)
                .cloned()
                .unwrap_or_default();
            for ck in &crate_keys {
                let q = format!("{}.{}", ck, imp.target);
                index.stub_types.insert(q.clone());
                index.all_names.push(q.clone());
                index
                    .stub_methods
                    .entry(q)
                    .or_default()
                    .extend(methods_snapshot.iter().cloned());
            }
        }
    }

    index.all_names.sort();
    index.all_names.dedup();
    index
}

fn index_construct(c: &Construct, index: &mut NameIndex) {
    let mut methods = HashSet::new();
    for m in &c.methods {
        methods.insert(strip_bang(&m.name));
    }
    for f in &c.fns {
        methods.insert(strip_bang(&f.name));
    }
    for imp in &c.impls {
        methods.insert(strip_bang(&imp.method_name));
    }
    // Struct/aggregate shaped constructs always get a synthetic `new` constructor.
    if matches!(c.shape, Shape::Struct | Shape::Enum) {
        methods.insert("new".to_string());
    }

    let mut fields = HashMap::new();
    for f in &c.fields {
        fields.insert(f.name.clone(), type_name_hint(&f.type_expr));
    }
    for b in &c.blocks {
        for f in &b.fields {
            fields.insert(f.name.clone(), type_name_hint(&f.type_expr));
        }
    }

    index.constructs.insert(
        c.name.clone(),
        ConstructInfo { methods, fields },
    );
    index.all_names.push(c.name.clone());

    for child in &c.children {
        index_construct(child, index);
    }
}

fn strip_bang(name: &str) -> String {
    name.trim_end_matches('!').to_string()
}

// ─── Scope ───────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct Scope {
    /// local name → optional construct/type name when known
    locals: HashMap<String, Option<String>>,
}

impl Scope {
    fn new() -> Self {
        Scope::default()
    }

    fn bind(&mut self, name: &str, ty: Option<String>) {
        self.locals.insert(name.to_string(), ty);
    }

    fn has(&self, name: &str) -> bool {
        self.locals.contains_key(name)
    }

    fn ty(&self, name: &str) -> Option<&Option<String>> {
        self.locals.get(name)
    }

    fn child(&self) -> Scope {
        self.clone()
    }
}

// ─── Construct walk ──────────────────────────────────────────────────────────

fn check_construct(
    c: &Construct,
    parent: &str,
    index: &NameIndex,
    registry: &LayerRegistry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let location = c.name.as_str();

    // Types on fields / methods / inputs
    for f in &c.fields {
        check_type_expr(&f.type_expr, location, index, diagnostics);
    }
    for b in &c.blocks {
        for f in &b.fields {
            check_type_expr(&f.type_expr, location, index, diagnostics);
        }
    }
    for m in &c.methods {
        for p in &m.params {
            check_type_expr(&p.type_expr, location, index, diagnostics);
        }
        if let Some(rt) = &m.return_type {
            check_type_expr(rt, location, index, diagnostics);
        }
    }
    for f in &c.inputs {
        check_type_expr(&f.type_expr, location, index, diagnostics);
    }
    if let Some(rt) = &c.return_type {
        check_type_expr(rt, location, index, diagnostics);
    }

    // Nested fn methods (aggregate body)
    for fndef in &c.fns {
        let mut scope = Scope::new();
        // Self fields available as bare idents for assignment targets; also as locals.
        if let Some(info) = index.constructs.get(&c.name) {
            for (field, ty) in &info.fields {
                scope.bind(field, ty.clone());
            }
        }
        for p in &fndef.params {
            scope.bind(&p.name, type_name_hint(&p.type_expr));
            check_type_expr(&p.type_expr, &fndef.name, index, diagnostics);
        }
        if let Some(rt) = &fndef.return_type {
            check_type_expr(rt, &fndef.name, index, diagnostics);
        }
        for e in &fndef.body {
            check_expr(e, location, &mut scope, index, Some(&c.name), diagnostics);
        }
    }

    // Impl method bodies
    for imp in &c.impls {
        let mut scope = Scope::new();
        for p in &imp.params {
            scope.bind(p, None);
        }
        // Prefer the adapter/struct's own fields for `self.field`; fall back to
        // the impl target name for method resolution on self.
        if let Some(info) = index.constructs.get(&c.name) {
            for (field, ty) in &info.fields {
                scope.bind(field, ty.clone());
            }
        }
        // Also import fields from a sibling struct that shares a name prefix
        // (e.g. struct PgTenantRepo + impl PgTenantRepoImpl).
        import_related_struct_fields(&c.name, index, &mut scope);
        let self_ty = Some(c.name.as_str());
        for e in &imp.body {
            check_expr(e, location, &mut scope, index, self_ty, diagnostics);
        }
    }

    // Flow-shaped steps
    if !c.steps.is_empty() || !c.inputs.is_empty() {
        let mut scope = Scope::new();
        for f in &c.inputs {
            scope.bind(&f.name, type_name_hint(&f.type_expr));
        }
        // @dep inputs often are ports/traits — type is the trait name
        for step in &c.steps {
            check_flow_step(step, location, &mut scope, index, diagnostics);
        }
        if let Some(ret) = &c.return_expr {
            check_expr(ret, location, &mut scope, index, None, diagnostics);
        }
    }

    for child in &c.children {
        check_construct(child, &c.name, index, registry, diagnostics);
    }

    let _ = parent;
}

fn check_flow(flow: &Flow, index: &NameIndex, diagnostics: &mut Vec<Diagnostic>) {
    let mut scope = Scope::new();
    for f in &flow.inputs {
        scope.bind(&f.name, type_name_hint(&f.type_expr));
        check_type_expr(&f.type_expr, &flow.name, index, diagnostics);
    }
    for step in &flow.steps {
        check_flow_step(step, &flow.name, &mut scope, index, diagnostics);
    }
    if let Some(ret) = &flow.return_expr {
        check_expr(ret, &flow.name, &mut scope, index, None, diagnostics);
    }
}

fn check_flow_step(
    step: &FlowStep,
    location: &str,
    scope: &mut Scope,
    index: &NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match step {
        FlowStep::Step(sd) => {
            for e in &sd.body {
                check_expr(e, location, scope, index, None, diagnostics);
            }
            for sb in &sd.sub_blocks {
                for e in &sb.body {
                    check_expr(e, location, scope, index, None, diagnostics);
                }
            }
        }
        FlowStep::Parallel(par) => {
            for s in &par.steps {
                check_flow_step(&FlowStep::Step(s.clone()), location, scope, index, diagnostics);
            }
        }
        FlowStep::Match(m) => {
            check_expr(&m.expr, location, scope, index, None, diagnostics);
            for arm in &m.arms {
                let mut arm_scope = scope.child();
                if let Some(rp) = &arm.rich_pattern {
                    bind_pattern(rp, &mut arm_scope, None);
                } else {
                    bind_pattern_names(&arm.pattern, &mut arm_scope);
                }
                if let Some(g) = &arm.guard {
                    check_expr(g, location, &mut arm_scope, index, None, diagnostics);
                }
                for e in &arm.body {
                    check_expr(e, location, &mut arm_scope, index, None, diagnostics);
                }
            }
        }
    }
}

// ─── Expressions ─────────────────────────────────────────────────────────────

fn check_expr(
    expr: &Expr,
    location: &str,
    scope: &mut Scope,
    index: &NameIndex,
    self_type: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Ident(_) => {
            // Bare idents are local/field/parameter references — not call targets.
            // Unresolved bare idents are CHK-004 territory when used as values.
        }
        Expr::FieldAccess(inner, _) => {
            check_expr(inner, location, scope, index, self_type, diagnostics);
        }
        Expr::Call(call) => {
            check_call(call, location, scope, index, self_type, diagnostics);
            for a in &call.args {
                check_expr(a, location, scope, index, self_type, diagnostics);
            }
            if let Some(recv) = &call.receiver {
                check_expr(recv, location, scope, index, self_type, diagnostics);
            }
        }
        Expr::Action(action) => {
            check_action(action, location, scope, index, self_type, diagnostics);
            for a in &action.args {
                check_expr(a, location, scope, index, self_type, diagnostics);
            }
            for (_, e) in &action.named_args {
                check_expr(e, location, scope, index, self_type, diagnostics);
            }
            if let Some(cond) = &action.condition {
                check_expr(cond, location, scope, index, self_type, diagnostics);
            }
        }
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            check_expr(rhs, location, scope, index, self_type, diagnostics);
            let ty = infer_rhs_type(rhs, index, scope);
            let ann = match expr {
                Expr::Assign(_, _, Some(a)) | Expr::MutAssign(_, _, Some(a)) => Some(a),
                _ => None,
            };
            if let Some(ann) = ann {
                check_type_expr(ann, location, index, diagnostics);
                scope.bind(name, type_name_hint(ann).or(ty));
            } else {
                scope.bind(name, ty);
            }
        }
        Expr::LetPattern(pat, rhs, ty) => {
            check_expr(rhs, location, scope, index, self_type, diagnostics);
            if let Some(t) = ty {
                check_type_expr(t, location, index, diagnostics);
            }
            bind_pattern(pat, scope, ty.as_ref().and_then(type_name_hint));
        }
        Expr::BinaryOp(op) => {
            check_expr(&op.left, location, scope, index, self_type, diagnostics);
            check_expr(&op.right, location, scope, index, self_type, diagnostics);
        }
        Expr::UnaryOp(op) => {
            check_expr(&op.expr, location, scope, index, self_type, diagnostics);
        }
        Expr::IfExpr(ie) => {
            check_expr(&ie.condition, location, scope, index, self_type, diagnostics);
            let mut then_scope = scope.child();
            for e in &ie.then_body {
                check_expr(e, location, &mut then_scope, index, self_type, diagnostics);
            }
            if let Some(eb) = &ie.else_body {
                let mut else_scope = scope.child();
                for e in eb {
                    check_expr(e, location, &mut else_scope, index, self_type, diagnostics);
                }
            }
        }
        Expr::Match(scrutinee, arms) => {
            check_expr(scrutinee, location, scope, index, self_type, diagnostics);
            for arm in arms {
                let mut arm_scope = scope.child();
                if let Some(rp) = &arm.rich_pattern {
                    bind_pattern(rp, &mut arm_scope, None);
                } else {
                    bind_pattern_names(&arm.pattern, &mut arm_scope);
                }
                if let Some(g) = &arm.guard {
                    check_expr(g, location, &mut arm_scope, index, self_type, diagnostics);
                }
                for e in &arm.body {
                    check_expr(e, location, &mut arm_scope, index, self_type, diagnostics);
                }
            }
        }
        Expr::ForLoop {
            binding,
            index: idx,
            iterable,
            body,
        } => {
            check_expr(iterable, location, scope, index, self_type, diagnostics);
            let mut loop_scope = scope.child();
            loop_scope.bind(binding, None);
            if let Some(i) = idx {
                loop_scope.bind(i, Some("Int".into()));
            }
            for e in body {
                check_expr(e, location, &mut loop_scope, index, self_type, diagnostics);
            }
        }
        Expr::WhileLoop { condition, body } => {
            check_expr(condition, location, scope, index, self_type, diagnostics);
            let mut loop_scope = scope.child();
            for e in body {
                check_expr(e, location, &mut loop_scope, index, self_type, diagnostics);
            }
        }
        Expr::Loop(body) => {
            let mut loop_scope = scope.child();
            for e in body {
                check_expr(e, location, &mut loop_scope, index, self_type, diagnostics);
            }
        }
        Expr::Closure { params, body } => {
            let mut c_scope = scope.child();
            for p in params {
                c_scope.bind(p, None);
            }
            for e in body {
                check_expr(e, location, &mut c_scope, index, self_type, diagnostics);
            }
        }
        Expr::Return(e) | Expr::Await(e) | Expr::Try(e) | Expr::Index(e, _) => {
            check_expr(e, location, scope, index, self_type, diagnostics);
            if let Expr::Index(_, ix) = expr {
                check_expr(ix, location, scope, index, self_type, diagnostics);
            }
        }
        Expr::Cast(e, ty_name) => {
            check_expr(e, location, scope, index, self_type, diagnostics);
            // Cast target as bare type name
            if !is_known_type(ty_name, index) {
                push_unknown_type(ty_name, location, index, diagnostics);
            }
        }
        Expr::StructLit(name, fields) => {
            if !index.constructs.contains_key(name)
                && !index.stub_types.contains(name)
                && !index.type_aliases.contains(name)
            {
                // Events / messages often defined as struct constructs — error if missing
                if looks_like_type_name(name) {
                    push_unresolved(
                        "unresolved_type",
                        format!("unknown type or construct '{}'", name),
                        location,
                        name,
                        index,
                        diagnostics,
                    );
                }
            }
            for (_, e) in fields {
                check_expr(e, location, scope, index, self_type, diagnostics);
            }
        }
        Expr::StructUpdate { name, fields, base } => {
            if looks_like_type_name(name) && !is_known_type(name, index) {
                push_unknown_type(name, location, index, diagnostics);
            }
            for (_, e) in fields {
                check_expr(e, location, scope, index, self_type, diagnostics);
            }
            check_expr(base, location, scope, index, self_type, diagnostics);
        }
        Expr::Tuple(items) | Expr::ArrayLit(items) => {
            for e in items {
                check_expr(e, location, scope, index, self_type, diagnostics);
            }
        }
        Expr::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    check_expr(e, location, scope, index, self_type, diagnostics);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            if let Some(s) = start {
                check_expr(s, location, scope, index, self_type, diagnostics);
            }
            if let Some(e) = end {
                check_expr(e, location, scope, index, self_type, diagnostics);
            }
        }
        Expr::IfLet {
            pattern,
            expr: e,
            then_body,
            else_body,
        } => {
            check_expr(e, location, scope, index, self_type, diagnostics);
            let mut s = scope.child();
            // pattern is String in IfLet
            bind_pattern_names(pattern, &mut s);
            for x in then_body {
                check_expr(x, location, &mut s, index, self_type, diagnostics);
            }
            if let Some(eb) = else_body {
                let mut es = scope.child();
                for x in eb {
                    check_expr(x, location, &mut es, index, self_type, diagnostics);
                }
            }
        }
        Expr::WhileLet {
            pattern,
            expr: e,
            body,
        } => {
            check_expr(e, location, scope, index, self_type, diagnostics);
            let mut s = scope.child();
            bind_pattern_names(pattern, &mut s);
            for x in body {
                check_expr(x, location, &mut s, index, self_type, diagnostics);
            }
        }
        Expr::StringLit(_)
        | Expr::IntLit(_)
        | Expr::FloatLit(_)
        | Expr::BoolLit(_)
        | Expr::Break
        | Expr::Continue => {}
    }
}

fn check_call(
    call: &CallExpr,
    location: &str,
    scope: &Scope,
    index: &NameIndex,
    self_type: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Method chain: receiver.method(...)
    if let Some(recv) = &call.receiver {
        let method = strip_bang(&call.method);
        if method.is_empty() {
            return;
        }
        if let Some(type_name) = receiver_type_name(recv, scope, self_type, location, index) {
            check_method_on_type(&type_name, &method, location, index, diagnostics);
        }
        // Unknown receiver type → skip method check (CHK-004 will tighten)
        return;
    }

    let target = call.target.as_str();
    if target.is_empty() {
        return;
    }

    // Intrinsic: now(), env!(...)
    if call.method.is_empty() && is_builtin_call(target) {
        return;
    }

    // self.field.method or self.field (stored as target "self.pool")
    if let Some(field_path) = target.strip_prefix("self.") {
        let field = field_path.split('.').next().unwrap_or(field_path);
        let ty = lookup_self_field_type(field, self_type, location, index, scope);
        if !call.method.is_empty() {
            if let Some(ty) = ty {
                check_method_on_type(&ty, &strip_bang(&call.method), location, index, diagnostics);
            }
            // Unknown field type: do not flag as unresolved_external
        }
        return;
    }
    if target == "self" {
        // self.method(...)
        if !call.method.is_empty() {
            if let Some(st) = self_type {
                check_method_on_type(st, &strip_bang(&call.method), location, index, diagnostics);
            }
        }
        return;
    }

    // Local binding used as callable / namespace
    if scope.has(target) {
        if !call.method.is_empty() {
            let method = strip_bang(&call.method);
            if let Some(Some(ty)) = scope.ty(target) {
                check_method_on_type(ty, &method, location, index, diagnostics);
            }
        }
        return;
    }

    // Free function
    if call.method.is_empty() && index.free_fns.contains(target) {
        return;
    }

    // Construct / type / stub
    if let Some(info) = index.constructs.get(target) {
        if !call.method.is_empty() {
            let method = strip_bang(&call.method);
            if !info.methods.contains(&method) && method != "new" {
                check_method_on_type(target, &method, location, index, diagnostics);
            }
        }
        return;
    }

    if index.stub_types.contains(target) {
        if !call.method.is_empty() {
            let method = strip_bang(&call.method);
            if let Some(methods) = index.stub_methods.get(target) {
                if !methods.contains(&method) && method != "new" {
                    push_unresolved(
                        "unresolved_method",
                        format!("unknown method '{}' on stub type '{}'", call.method, target),
                        location,
                        &method,
                        index,
                        diagnostics,
                    );
                }
            }
        }
        return;
    }

    // Unknown target
    if looks_like_type_name(target) {
        push_unresolved(
            "unresolved_name",
            format!("unknown name '{}' in call", target),
            location,
            target,
            index,
            diagnostics,
        );
    } else {
        // lowercase external (http, sqlx, …) without stub — warning
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: format!(
                "call target '{}' is not a known construct, local, or stub — treat as external",
                target
            ),
            node_id: None,
            node_name: Some(location.to_string()),
            code: "unresolved_external".to_string(),
            constraint: "unresolved_external".to_string(),
            parent: None,
            hint: Some(format!(
                "add a .stub for '{}' or a construct/port with that name",
                target
            )),
            span_start: None,
            span_end: None,
        });
    }
}

fn check_action(
    action: &ActionExpr,
    location: &str,
    scope: &Scope,
    index: &NameIndex,
    self_type: Option<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use crate::layer::StmtShape;
    match action.shape {
        StmtShape::Call => {
            // Desugared or bare: treat like CallExpr
            let call = CallExpr {
                target: action.target.clone(),
                method: action.method.clone(),
                args: action.args.clone(),
                receiver: None,
                sugar: Some(action.keyword.clone()),
                span: action.span,
            };
            // For sugar like emit/dispatch, target is often a struct/event name
            if !action.target.is_empty() {
                check_call(&call, location, scope, index, self_type, diagnostics);
            }
        }
        StmtShape::If => {
            // condition already walked by caller
        }
    }
}

/// Built-in methods on container types (name-resolution only).
fn is_container_method(type_name: &str, method: &str) -> bool {
    match type_name {
        "List" | "Vec" => matches!(
            method,
            "get" | "at" | "len" | "length" | "count" | "is_empty" | "push" | "pop" | "contains"
        ),
        "Opt" | "Option" => matches!(method, "is_some" | "is_none" | "unwrap" | "unwrap_or"),
        "Res" | "Result" => matches!(method, "is_ok" | "is_err" | "unwrap" | "unwrap_or"),
        "Map" | "HashMap" => matches!(method, "get" | "insert" | "contains" | "len" | "is_empty"),
        _ => false,
    }
}

fn check_method_on_type(
    type_name: &str,
    method: &str,
    location: &str,
    index: &NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if is_container_method(type_name, method) {
        return;
    }
    if let Some(info) = index.constructs.get(type_name) {
        if !info.methods.contains(method) && method != "new" {
            let mut d = Diagnostic {
                severity: Severity::Error,
                message: format!("unknown method '{}' on '{}'", method, type_name),
                node_id: None,
                node_name: Some(location.to_string()),
                code: "unresolved_method".to_string(),
                constraint: "unresolved_method".to_string(),
                parent: None,
                hint: suggest_from_set(method, &info.methods).map(|s| format!("did you mean '{}'?", s)),
                span_start: None,
                span_end: None,
            };
            if d.hint.is_none() && !info.methods.is_empty() {
                let mut ms: Vec<_> = info.methods.iter().cloned().collect();
                ms.sort();
                d.hint = Some(format!("available: {}", ms.join(", ")));
            }
            diagnostics.push(d);
        }
        return;
    }
    if let Some(methods) = index.stub_methods.get(type_name) {
        if !methods.contains(method) && method != "new" {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: format!("unknown method '{}' on stub type '{}'", method, type_name),
                node_id: None,
                node_name: Some(location.to_string()),
                code: "unresolved_method".to_string(),
                constraint: "unresolved_method".to_string(),
                parent: None,
                hint: suggest_from_set(method, methods).map(|s| format!("did you mean '{}'?", s)),
                span_start: None,
                span_end: None,
            });
        }
    }
}

// ─── Types ───────────────────────────────────────────────────────────────────

fn check_type_expr(
    ty: &TypeExpr,
    location: &str,
    index: &NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match ty {
        TypeExpr::Named(name) => {
            if !is_known_type(name, index) {
                push_unknown_type(name, location, index, diagnostics);
            }
        }
        TypeExpr::Generic(name, args) => {
            // List/Map/Opt/Res/Set or user generic
            if !BUILTIN_TYPES.contains(&name.as_str())
                && !index.constructs.contains_key(name)
                && !index.type_aliases.contains(name)
                && !index.stub_types.contains(name)
            {
                push_unknown_type(name, location, index, diagnostics);
            }
            for a in args {
                check_type_expr(a, location, index, diagnostics);
            }
        }
        TypeExpr::Result(inner) => {
            if let Some(t) = inner {
                check_type_expr(t, location, index, diagnostics);
            }
        }
        TypeExpr::Optional(t) | TypeExpr::List(t) | TypeExpr::Set(t) | TypeExpr::Dyn(t)
        | TypeExpr::ImplTrait(t) | TypeExpr::Array(t, _) | TypeExpr::Ref(t, _) => {
            check_type_expr(t, location, index, diagnostics);
        }
        TypeExpr::Map(k, v) => {
            check_type_expr(k, location, index, diagnostics);
            check_type_expr(v, location, index, diagnostics);
        }
        TypeExpr::Tuple(items) => {
            for t in items {
                check_type_expr(t, location, index, diagnostics);
            }
        }
        TypeExpr::FnPtr(args, ret) => {
            for a in args {
                check_type_expr(a, location, index, diagnostics);
            }
            if let Some(r) = ret {
                check_type_expr(r, location, index, diagnostics);
            }
        }
    }
}

fn is_known_type(name: &str, index: &NameIndex) -> bool {
    BUILTIN_TYPES.contains(&name)
        || index.constructs.contains_key(name)
        || index.type_aliases.contains(name)
        || index.stub_types.contains(name)
}

fn is_builtin_call(name: &str) -> bool {
    let base = name.trim_end_matches('!');
    BUILTIN_CALLS.contains(&base)
}

fn looks_like_type_name(name: &str) -> bool {
    name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

fn type_name_hint(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Named(n) => Some(n.clone()),
        // Keep container identity so List/Opt methods resolve (get/len, is_some, …)
        TypeExpr::Generic(n, _) if n == "List" || n == "Vec" => Some("List".into()),
        TypeExpr::Generic(n, _) if n == "Opt" || n == "Option" => Some("Opt".into()),
        TypeExpr::Generic(n, _) if n == "Res" || n == "Result" => Some("Res".into()),
        TypeExpr::Generic(n, args) => args.first().and_then(type_name_hint).or_else(|| Some(n.clone())),
        TypeExpr::List(_) => Some("List".into()),
        TypeExpr::Optional(_) => Some("Opt".into()),
        TypeExpr::Result(_) => Some("Res".into()),
        _ => None,
    }
}

fn infer_rhs_type(expr: &Expr, index: &NameIndex, scope: &Scope) -> Option<String> {
    match expr {
        Expr::Call(call) if !call.target.is_empty() && call.method.is_empty() => {
            if index.constructs.contains_key(&call.target) {
                Some(call.target.clone())
            } else {
                None
            }
        }
        Expr::Call(call) if !call.target.is_empty() && strip_bang(&call.method) == "new" => {
            Some(call.target.clone())
        }
        Expr::Call(call) if !call.target.is_empty() => {
            // Port/repo method — unknown return without type check; leave None
            let _ = scope;
            None
        }
        Expr::StructLit(name, _) => Some(name.clone()),
        Expr::Ident(n) => scope.ty(n).and_then(|t| t.clone()),
        _ => None,
    }
}

fn receiver_type_name(
    recv: &Expr,
    scope: &Scope,
    self_type: Option<&str>,
    location: &str,
    index: &NameIndex,
) -> Option<String> {
    match recv {
        Expr::Ident(n) if n == "self" => self_type.map(|s| s.to_string()),
        Expr::Ident(n) => scope.ty(n).and_then(|t| t.clone()),
        Expr::FieldAccess(inner, field) => {
            if let Expr::Ident(n) = inner.as_ref() {
                if n == "self" {
                    return lookup_self_field_type(field, self_type, location, index, scope);
                }
            }
            let base = receiver_type_name(inner, scope, self_type, location, index)?;
            index
                .constructs
                .get(&base)
                .and_then(|info| info.fields.get(field).cloned())
                .flatten()
                .or_else(|| scope.ty(field).and_then(|t| t.clone()))
        }
        _ => None,
    }
}

fn lookup_self_field_type(
    field: &str,
    self_type: Option<&str>,
    location: &str,
    index: &NameIndex,
    scope: &Scope,
) -> Option<String> {
    if let Some(Some(ty)) = scope.ty(field) {
        return Some(ty.clone());
    }
    for candidate in [self_type, Some(location)].into_iter().flatten() {
        if let Some(info) = index.constructs.get(candidate) {
            if let Some(ty) = info.fields.get(field) {
                return ty.clone();
            }
        }
    }
    // Unique field name across package
    let mut found = None;
    for info in index.constructs.values() {
        if let Some(ty) = info.fields.get(field) {
            if found.is_some() {
                return None; // ambiguous
            }
            found = ty.clone();
        }
    }
    found
}

fn import_related_struct_fields(impl_name: &str, index: &NameIndex, scope: &mut Scope) {
    // PgTenantRepoImpl → try PgTenantRepo, PgTenantRepoImpl without Impl suffix
    let candidates = [
        impl_name.strip_suffix("Impl").unwrap_or(impl_name),
        impl_name.strip_suffix("Adapter").unwrap_or(impl_name),
    ];
    for name in candidates {
        if name == impl_name {
            continue;
        }
        if let Some(info) = index.constructs.get(name) {
            for (field, ty) in &info.fields {
                if !scope.has(field) {
                    scope.bind(field, ty.clone());
                }
            }
        }
    }
}

// ─── Patterns ────────────────────────────────────────────────────────────────

fn bind_pattern(pat: &Pattern, scope: &mut Scope, ty: Option<String>) {
    match pat {
        Pattern::Ident(n) => {
            if n != "_" {
                scope.bind(n, ty);
            }
        }
        Pattern::Tuple(parts) => {
            for p in parts {
                bind_pattern(p, scope, None);
            }
        }
        Pattern::Struct(_, fields, _) => {
            for (name, inner) in fields {
                if let Some(p) = inner {
                    bind_pattern(p, scope, None);
                } else {
                    scope.bind(name, None);
                }
            }
        }
        Pattern::Variant(_, fields) => {
            for p in fields {
                bind_pattern(p, scope, None);
            }
        }
        Pattern::Or(parts) => {
            for p in parts {
                bind_pattern(p, scope, None);
            }
        }
        Pattern::Literal(_) | Pattern::Wildcard | Pattern::Rest => {}
    }
}

fn bind_pattern_names(pattern: &str, scope: &mut Scope) {
    // Match arm patterns are often stored as strings in some AST nodes.
    // Extract simple identifiers.
    for token in pattern.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if !token.is_empty()
            && token != "_"
            && token.chars().next().map(|c| c.is_lowercase() || c == '_').unwrap_or(false)
        {
            scope.bind(token, None);
        }
    }
}

// ─── Diagnostics helpers ─────────────────────────────────────────────────────

fn push_unknown_type(
    name: &str,
    location: &str,
    index: &NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    push_unresolved(
        "unresolved_type",
        format!("unknown type '{}'", name),
        location,
        name,
        index,
        diagnostics,
    );
}

fn push_unresolved(
    code: &str,
    message: String,
    location: &str,
    needle: &str,
    index: &NameIndex,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let hint = suggest_name(needle, &index.all_names).map(|s| format!("did you mean '{}'?", s));
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        message,
        node_id: None,
        node_name: Some(location.to_string()),
        code: code.to_string(),
        constraint: code.to_string(),
        parent: None,
        hint,
        span_start: None,
        span_end: None,
    });
}

fn suggest_name(needle: &str, candidates: &[String]) -> Option<String> {
    let set: HashSet<String> = candidates.iter().cloned().collect();
    suggest_from_set(needle, &set)
}

fn suggest_from_set(needle: &str, candidates: &HashSet<String>) -> Option<String> {
    let needle_l = needle.to_lowercase();
    let mut best: Option<(usize, String)> = None;
    for c in candidates {
        let d = edit_distance(&needle_l, &c.to_lowercase());
        if d > 0 && d <= 2 {
            if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
                best = Some((d, c.clone()));
            }
        }
        // prefix match
        if c.to_lowercase().starts_with(&needle_l) && needle.len() >= 2 {
            if best.as_ref().map(|(bd, _)| *bd > 1).unwrap_or(true) {
                best = Some((1, c.clone()));
            }
        }
    }
    best.map(|(_, s)| s)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{ConstructSpec, Shape, Visual};
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
            presentation: Default::default(),
        }
    }

    fn reg_with(constructs: Vec<ConstructSpec>) -> LayerRegistry {
        let mut reg = LayerRegistry::builtin();
        for s in constructs {
            if let Some(i) = reg.constructs.iter().position(|c| c.keyword == s.keyword) {
                reg.constructs[i] = s;
            } else {
                reg.constructs.push(s);
            }
        }
        reg
    }

    fn sol(items: Vec<TopLevelItem>) -> Solution {
        Solution {
            name: "T".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            links: vec![],
            items,
            expose: None,
        }
    }

    #[test]
    fn happy_path_known_port_method() {
        let reg = reg_with(vec![
            spec("port", "Port", Shape::Trait),
            spec("svc", "Service", Shape::Fn),
        ]);
        let mut port = Construct::new("port", "Port", Shape::Trait, "UserRepo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "save!".into(),
            params: Vec::new(),
            return_type: None,
            span: Span::new(0, 0),
        });
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "Create".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(step_with_body(vec![Expr::Call(CallExpr {
            target: "UserRepo".into(),
            method: "save".into(),
            args: Vec::new(),
            receiver: None,
            sugar: None,
            span: Span::new(0, 0),
        })])));
        let diags = check_names(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg,
        );
        assert!(
            !diags.iter().any(|d| d.code == "unresolved_name" || d.code == "unresolved_method"),
            "{:?}",
            diags
        );
    }

    fn step_with_body(body: Vec<Expr>) -> StepDef {
        StepDef {
            name: "go".into(),
            span: Span::new(0, 0),
            body,
            refs: Vec::new(),
            sub_blocks: Vec::new(),
        }
    }

    #[test]
    fn missing_port_method_errors() {
        let reg = reg_with(vec![
            spec("port", "Port", Shape::Trait),
            spec("svc", "Service", Shape::Fn),
        ]);
        let mut port = Construct::new("port", "Port", Shape::Trait, "UserRepo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "save!".into(),
            params: Vec::new(),
            return_type: None,
            span: Span::new(0, 0),
        });
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "Create".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(step_with_body(vec![Expr::Call(CallExpr {
            target: "UserRepo".into(),
            method: "find".into(),
            args: Vec::new(),
            receiver: None,
            sugar: None,
            span: Span::new(0, 0),
        })])));
        let diags = check_names(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg,
        );
        assert!(
            diags.iter().any(|d| d.code == "unresolved_method" && d.message.contains("find")),
            "{:?}",
            diags
        );
        let d = diags.iter().find(|d| d.code == "unresolved_method").unwrap();
        assert!(
            d.hint.as_deref().map(|h| h.contains("save")).unwrap_or(false),
            "hint={:?}",
            d.hint
        );
    }

    #[test]
    fn typo_in_construct_name_suggests() {
        let reg = reg_with(vec![
            spec("port", "Port", Shape::Trait),
            spec("svc", "Service", Shape::Fn),
        ]);
        let port = Construct::new("port", "Port", Shape::Trait, "UserRepo".into(), Span::new(0, 0));
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "Create".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(step_with_body(vec![Expr::Call(CallExpr {
            target: "UserRepoo".into(),
            method: "save".into(),
            args: Vec::new(),
            receiver: None,
            sugar: None,
            span: Span::new(0, 0),
        })])));
        let diags = check_names(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(svc),
            ]),
            &reg,
        );
        let d = diags
            .iter()
            .find(|d| d.code == "unresolved_name")
            .expect("unresolved_name");
        assert!(
            d.hint.as_deref().map(|h| h.contains("UserRepo")).unwrap_or(false),
            "hint={:?}",
            d.hint
        );
    }

    #[test]
    fn unknown_type_on_field_errors() {
        let reg = reg_with(vec![spec("agg", "Aggregate", Shape::Struct)]);
        let mut agg = Construct::new(
            "agg",
            "Aggregate",
            Shape::Struct,
            "Customer".into(),
            Span::new(0, 0),
        );
        agg.fields.push(Field {
            annotations: Vec::new(),
            name: "x".into(),
            type_expr: TypeExpr::Named("NotARealType".into()),
            default_expr: None,
            span: Span::new(0, 0),
        });
        let diags = check_names(&sol(vec![TopLevelItem::Construct(agg)]), &reg);
        assert!(
            diags.iter().any(|d| d.code == "unresolved_type"),
            "{:?}",
            diags
        );
    }

    #[test]
    fn local_binding_not_flagged_as_unresolved() {
        let reg = reg_with(vec![
            spec("port", "Port", Shape::Trait),
            spec("svc", "Service", Shape::Fn),
            spec("agg", "Aggregate", Shape::Struct),
        ]);
        let mut port = Construct::new("port", "Port", Shape::Trait, "UserRepo".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "find!".into(),
            params: Vec::new(),
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named("User".into())))),
            span: Span::new(0, 0),
        });
        let mut user = Construct::new("agg", "Aggregate", Shape::Struct, "User".into(), Span::new(0, 0));
        user.fns.push(FnDef {
            name: "greet".into(),
            span: Span::new(0, 0),
            params: Vec::new(),
            return_type: None,
            annotations: Vec::new(),
            body: Vec::new(),
            layer_provided: false,
        });
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "Greet".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(step_with_body(vec![
            Expr::Assign(
                "user".into(),
                Box::new(Expr::Call(CallExpr {
                    target: "User".into(),
                    method: "new".into(),
                    args: Vec::new(),
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 0),
                })),
                None,
            ),
            Expr::Call(CallExpr {
                target: String::new(),
                method: "greet".into(),
                args: Vec::new(),
                receiver: Some(Box::new(Expr::Ident("user".into()))),
                sugar: None,
                span: Span::new(0, 0),
            }),
        ])));
        let diags = check_names(
            &sol(vec![
                TopLevelItem::Construct(port),
                TopLevelItem::Construct(user),
                TopLevelItem::Construct(svc),
            ]),
            &reg,
        );
        assert!(
            !diags.iter().any(|d| d.severity == Severity::Error),
            "{:?}",
            diags
        );
    }
}
