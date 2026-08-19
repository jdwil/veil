use std::collections::{HashMap, HashSet};
use veil_ir::ast::*;


/// Names that need `let mut` on first bind: explicit `mut`, reassignment,
/// field write (`x.f = …`), or receiver of a known mutating method (`x.push`).
pub fn analyze_mut_locals(body: &[Expr]) -> HashSet<String> {
    let mut needs = HashSet::new();
    let mut bound = HashSet::new();
    for e in body {
        walk_mut_needs(e, &mut needs, &mut bound);
    }
    needs
}

/// How many times each ident is *read* (not bound) in `body`.
pub fn count_ident_uses(body: &[Expr]) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for e in body {
        walk_ident_reads(e, &mut m);
    }
    m
}

pub fn count_ident_uses_in_steps(steps: &[FlowStep]) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for step in steps {
        match step {
            FlowStep::Step(s) => {
                for e in &s.body {
                    walk_ident_reads(e, &mut m);
                }
            }
            FlowStep::Parallel(par) => {
                for s in &par.steps {
                    for e in &s.body {
                        walk_ident_reads(e, &mut m);
                    }
                }
            }
            FlowStep::Match(md) => {
                walk_ident_reads(&md.expr, &mut m);
                for arm in &md.arms {
                    for e in &arm.body {
                        walk_ident_reads(e, &mut m);
                    }
                }
            }
        }
    }
    m
}

pub fn walk_ident_reads(e: &Expr, m: &mut HashMap<String, usize>) {
    match e {
        Expr::Ident(n) => {
            *m.entry(n.clone()).or_insert(0) += 1;
        }
        Expr::FieldAccess(base, _) => walk_ident_reads(base, m),
        Expr::Call(c) => {
            if !c.target.is_empty()
                && c.target
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_lowercase() || ch == '_')
            {
                *m.entry(c.target.clone()).or_insert(0) += 1;
            }
            if let Some(r) = &c.receiver {
                walk_ident_reads(r, m);
            }
            for a in &c.args {
                walk_ident_reads(a, m);
            }
        }
        Expr::BinaryOp(op) => {
            walk_ident_reads(&op.left, m);
            walk_ident_reads(&op.right, m);
        }
        Expr::UnaryOp(op) => walk_ident_reads(&op.expr, m),
        Expr::IfExpr(ie) => {
            walk_ident_reads(&ie.condition, m);
            for x in &ie.then_body {
                walk_ident_reads(x, m);
            }
            if let Some(eb) = &ie.else_body {
                for x in eb {
                    walk_ident_reads(x, m);
                }
            }
        }
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            if name.contains('.')
                && let Some(base) = name.split('.').next() {
                    *m.entry(base.to_string()).or_insert(0) += 1;
                }
            walk_ident_reads(rhs, m);
        }
        Expr::Return(inner)
        | Expr::Await(inner)
        | Expr::Try(inner)
        | Expr::Require(inner)
        | Expr::Cast(inner, _) => walk_ident_reads(inner, m),
        Expr::Action(a) => {
            for a in &a.args {
                walk_ident_reads(a, m);
            }
            for (_, e) in &a.named_args {
                walk_ident_reads(e, m);
            }
        }
        Expr::StructLit(_, fields) | Expr::StructUpdate { fields, .. } => {
            if let Expr::StructUpdate { base, .. } = e {
                walk_ident_reads(base, m);
            }
            for (_, v) in fields {
                walk_ident_reads(v, m);
            }
        }
        Expr::Match(s, arms) => {
            walk_ident_reads(s, m);
            for arm in arms {
                for x in &arm.body {
                    walk_ident_reads(x, m);
                }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            walk_ident_reads(iterable, m);
            bump_loop_reads(body, m);
        }
        Expr::WhileLoop { condition, body } => {
            walk_ident_reads(condition, m);
            bump_loop_reads(body, m);
        }
        Expr::WhileLet { expr: scrut, body, .. } => {
            walk_ident_reads(scrut, m);
            bump_loop_reads(body, m);
        }
        Expr::Loop(body) => {
            bump_loop_reads(body, m);
        }
        Expr::IfLet {
            expr: scrut,
            then_body,
            else_body,
            ..
        } => {
            walk_ident_reads(scrut, m);
            for x in then_body {
                walk_ident_reads(x, m);
            }
            if let Some(eb) = else_body {
                for x in eb {
                    walk_ident_reads(x, m);
                }
            }
        }
        Expr::DoBlock(body) | Expr::Closure { body, .. } => {
            for x in body {
                walk_ident_reads(x, m);
            }
        }
        Expr::Tuple(items) | Expr::ArrayLit(items) => {
            for x in items {
                walk_ident_reads(x, m);
            }
        }
        Expr::Index(a, b) => {
            walk_ident_reads(a, m);
            walk_ident_reads(b, m);
        }
        Expr::LetPattern(_, rhs, _) => walk_ident_reads(rhs, m),
        Expr::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(x) = p {
                    walk_ident_reads(x, m);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_ident_reads(s, m);
            }
            if let Some(en) = end {
                walk_ident_reads(en, m);
            }
        }
        _ => {}
    }
}

/// Reads inside a loop run many times — never treat them as last-use moves.
/// Bindings introduced *in* the loop are per-iteration and keep their real count.
pub fn bump_loop_reads(body: &[Expr], m: &mut HashMap<String, usize>) {
    let mut inner = HashMap::new();
    for x in body {
        walk_ident_reads(x, &mut inner);
    }
    let local = bindings_introduced(body);
    for (k, v) in inner {
        let n = if local.contains(&k) { v } else { v.max(2) };
        *m.entry(k).or_insert(0) += n;
    }
}

pub fn bindings_introduced(body: &[Expr]) -> HashSet<String> {
    let mut s = HashSet::new();
    for e in body {
        collect_bindings(e, &mut s);
    }
    s
}

pub fn collect_bindings(e: &Expr, s: &mut HashSet<String>) {
    match e {
        Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) => {
            if !name.contains('.') {
                s.insert(name.clone());
            }
            collect_bindings(rhs, s);
        }
        Expr::ForLoop { binding, index, body, iterable } => {
            s.insert(binding.clone());
            if let Some(i) = index {
                s.insert(i.clone());
            }
            collect_bindings(iterable, s);
            for x in body {
                collect_bindings(x, s);
            }
        }
        Expr::IfExpr(ie) => {
            collect_bindings(&ie.condition, s);
            for x in &ie.then_body {
                collect_bindings(x, s);
            }
            if let Some(eb) = &ie.else_body {
                for x in eb {
                    collect_bindings(x, s);
                }
            }
        }
        Expr::Match(_, arms) => {
            for arm in arms {
                for x in &arm.body {
                    collect_bindings(x, s);
                }
            }
        }
        Expr::WhileLoop { condition, body } => {
            collect_bindings(condition, s);
            for x in body {
                collect_bindings(x, s);
            }
        }
        Expr::Loop(body) | Expr::DoBlock(body) => {
            for x in body {
                collect_bindings(x, s);
            }
        }
        _ => {}
    }
}

/// Union of mut-local needs across flow steps (locals persist across steps).
pub fn analyze_mut_locals_in_steps(steps: &[FlowStep]) -> HashSet<String> {
    let mut needs = HashSet::new();
    let mut bound = HashSet::new();
    for step in steps {
        match step {
            FlowStep::Step(s) => {
                for e in &s.body {
                    walk_mut_needs(e, &mut needs, &mut bound);
                }
            }
            FlowStep::Parallel(par) => {
                for s in &par.steps {
                    for e in &s.body {
                        walk_mut_needs(e, &mut needs, &mut bound);
                    }
                }
            }
            FlowStep::Match(m) => {
                walk_mut_needs(&m.expr, &mut needs, &mut bound);
                for arm in &m.arms {
                    walk_mut_needs_forked(&arm.body, &mut needs, &bound);
                }
            }
        }
    }
    needs
}

/// Walk a control-flow branch with its own bind set (SL-020).
///
/// Sibling match/if arms that first-bind the same name must not look like
/// reassignment of one local — that over-marks `let mut` (`unused_mut`).
pub fn walk_mut_needs_forked(body: &[Expr], needs: &mut HashSet<String>, bound: &HashSet<String>) {
    let mut branch_bound = bound.clone();
    for e in body {
        walk_mut_needs(e, needs, &mut branch_bound);
    }
}

/// Collection / builder methods that require `&mut self` in Rust.
const MUTATING_METHODS: &[&str] = &[
    "push",
    "insert",
    "extend",
    "append",
    "remove",
    "clear",
    "pop",
    "retain",
    "truncate",
    "resize",
    "swap_remove",
    "drain",
    "entry",
    "get_mut",
    "or_insert",
    "or_insert_with",
    "or_default",
    "and_modify",
];

pub fn walk_mut_needs(expr: &Expr, needs: &mut HashSet<String>, bound: &mut HashSet<String>) {
    match expr {
        Expr::MutAssign(name, rhs, _) => {
            walk_mut_needs(rhs, needs, bound);
            needs.insert(name.clone());
            bound.insert(name.clone());
        }
        Expr::Assign(name, rhs, _) => {
            walk_mut_needs(rhs, needs, bound);
            if let Some((base, _)) = name.split_once('.') {
                // Field write on a local → base must be mut.
                needs.insert(base.to_string());
            } else if bound.contains(name) {
                needs.insert(name.clone());
            } else {
                bound.insert(name.clone());
            }
        }
        Expr::Call(call) => {
            for a in &call.args {
                walk_mut_needs(a, needs, bound);
            }
            if let Some(recv) = &call.receiver {
                walk_mut_needs(recv, needs, bound);
            }
            // `out.insert(...)` / `out.push(...)` — receiver needs mut.
            let method = call.method.trim_end_matches(['!', '?']);
            if !method.is_empty() && MUTATING_METHODS.contains(&method) {
                if !call.target.is_empty() && !call.target.contains('.') {
                    needs.insert(call.target.clone());
                } else if let Some(recv) = &call.receiver
                    && let Expr::Ident(n) = recv.as_ref() {
                        needs.insert(n.clone());
                    }
            }
        }
        Expr::IfExpr(ie) => {
            walk_mut_needs(&ie.condition, needs, bound);
            walk_mut_needs_forked(&ie.then_body, needs, bound);
            if let Some(eb) = &ie.else_body {
                walk_mut_needs_forked(eb, needs, bound);
            }
        }
        Expr::IfLet {
            expr: scrut,
            then_body,
            else_body,
            ..
        } => {
            walk_mut_needs(scrut, needs, bound);
            walk_mut_needs_forked(then_body, needs, bound);
            if let Some(eb) = else_body {
                walk_mut_needs_forked(eb, needs, bound);
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            walk_mut_needs(iterable, needs, bound);
            for e in body {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::WhileLoop { condition, body, .. } => {
            walk_mut_needs(condition, needs, bound);
            for e in body {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::WhileLet { expr: scrut, body, .. } => {
            walk_mut_needs(scrut, needs, bound);
            for e in body {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::Loop(body) => {
            for e in body {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::DoBlock(body) => {
            for e in body {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::Match(scrut, arms) => {
            walk_mut_needs(scrut, needs, bound);
            for arm in arms {
                let mut arm_bound = bound.clone();
                if let Some(g) = &arm.guard {
                    walk_mut_needs(g, needs, &mut arm_bound);
                }
                for e in &arm.body {
                    walk_mut_needs(e, needs, &mut arm_bound);
                }
            }
        }
        Expr::Return(inner)
        | Expr::Await(inner)
        | Expr::Try(inner)
        | Expr::Require(inner)
        | Expr::Cast(inner, _)
        | Expr::FieldAccess(inner, _) => {
            walk_mut_needs(inner, needs, bound);
        }
        Expr::UnaryOp(u) => walk_mut_needs(&u.expr, needs, bound),
        Expr::BinaryOp(bin) => {
            walk_mut_needs(&bin.left, needs, bound);
            walk_mut_needs(&bin.right, needs, bound);
        }
        Expr::Index(base, idx) => {
            walk_mut_needs(base, needs, bound);
            walk_mut_needs(idx, needs, bound);
        }
        Expr::StructLit(_, fields) => {
            for (_, v) in fields {
                walk_mut_needs(v, needs, bound);
            }
        }
        Expr::StructUpdate { fields, base, .. } => {
            for (_, v) in fields {
                walk_mut_needs(v, needs, bound);
            }
            walk_mut_needs(base, needs, bound);
        }
        Expr::ArrayLit(items) | Expr::Tuple(items) => {
            for e in items {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::Closure { body, .. } => {
            for e in body {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::Action(a) => {
            for e in &a.args {
                walk_mut_needs(e, needs, bound);
            }
            for (_, v) in &a.named_args {
                walk_mut_needs(v, needs, bound);
            }
            if let Some(c) = &a.condition {
                walk_mut_needs(c, needs, bound);
            }
        }
        Expr::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    walk_mut_needs(e, needs, bound);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_mut_needs(s, needs, bound);
            }
            if let Some(e) = end {
                walk_mut_needs(e, needs, bound);
            }
        }
        Expr::LetPattern(_, rhs, _) => {
            walk_mut_needs(rhs, needs, bound);
        }
        _ => {}
    }
}
