//! Package adapt — resolve chains, path addressing, and flatten merge.
//!
//! Design contract: [`docs/ADAPT.md`]. Stories ADP-002 … ADP-011.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::span::Span;

/// Platform packages that must not be specialized via `adapt` (use only).
pub const ADAPT_DENYLIST: &[&str] = &["dlx_core"];

/// Error from adapt resolve / merge.
#[derive(Debug, Clone)]
pub struct AdaptError {
    pub code: String,
    pub message: String,
    pub span: Span,
}

impl AdaptError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, span: Span) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span,
        }
    }
}

impl std::fmt::Display for AdaptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AdaptError {}

/// Result of flattening an adapt chain.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Flattened package (leaf name; items = fully merged).
    pub package: Package,
    /// Chain names root → leaf.
    pub chain: Vec<String>,
    /// Final symbol name → contributing package names (outermost last).
    pub provenance: HashMap<String, Vec<String>>,
}

/// Convert a package to a Solution for check/codegen (drops adapt metadata).
pub fn package_as_solution(pkg: &Package) -> Solution {
    Solution {
        name: pkg.name.clone(),
        span: pkg.span,
        uses: pkg.uses.clone(),
        links: pkg.links.clone(),
        items: pkg.items.clone(),
        expose: pkg.expose.clone(),
    }
}

/// Search for `{name}.veil` under the given directories (first hit wins).
pub fn find_package_source(name: &str, search_paths: &[PathBuf]) -> Option<PathBuf> {
    let filename = format!("{name}.veil");
    for dir in search_paths {
        let candidate = dir.join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Default search paths relative to a leaf file and optional project/hub roots.
pub fn default_adapt_search_paths(
    leaf_path: &Path,
    extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(parent) = leaf_path.parent() {
        paths.push(parent.to_path_buf());
        // sibling adapt/ and clients/ dirs
        paths.push(parent.join("adapt"));
        if let Some(gp) = parent.parent() {
            paths.push(gp.to_path_buf());
            paths.push(gp.join("examples"));
            paths.push(gp.join("adapt"));
        }
    }
    for e in extra {
        if !paths.contains(e) {
            paths.push(e.clone());
        }
    }
    // CWD examples/ often used in the monorepo
    let cwd_examples = PathBuf::from("examples");
    if cwd_examples.is_dir() && !paths.contains(&cwd_examples) {
        paths.push(cwd_examples);
    }
    let cwd_adapt = PathBuf::from("examples/adapt");
    if cwd_adapt.is_dir() && !paths.contains(&cwd_adapt) {
        paths.push(cwd_adapt);
    }
    paths
}

/// True if this package name is forbidden as an adapt base.
pub fn is_adapt_denied(name: &str) -> bool {
    ADAPT_DENYLIST.iter().any(|d| *d == name)
}

/// Build ordered chain [Base0, …, Leaf] by following single `adapt` edges.
///
/// Diamond (multiple adapts on one package) → error (ADP-003 default).
/// Cycle → error with path. Denylist bases → error.
pub fn build_adapt_chain(
    leaf: &Package,
    mut load: impl FnMut(&str) -> Result<Package, String>,
) -> Result<Vec<Package>, AdaptError> {
    // Walk from leaf toward root, then reverse.
    let mut reverse: Vec<Package> = Vec::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut path_names: Vec<String> = Vec::new();

    let mut current = leaf.clone();
    loop {
        if !visiting.insert(current.name.clone()) {
            path_names.push(current.name.clone());
            return Err(AdaptError::new(
                "ADP-C7",
                format!(
                    "adapt cycle detected: {}",
                    path_names.join(" → ")
                ),
                current.span,
            ));
        }
        path_names.push(current.name.clone());

        match current.adapts.len() {
            0 => {
                reverse.push(current);
                break;
            }
            1 => {
                let base_name = current.adapts[0].package_name.clone();
                let base_span = current.adapts[0].span;
                if is_adapt_denied(&base_name) {
                    return Err(AdaptError::new(
                        "ADP-C2",
                        format!(
                            "cannot adapt platform package '{base_name}' (use only)"
                        ),
                        base_span,
                    ));
                }
                reverse.push(current);
                current = load(&base_name).map_err(|e| {
                    AdaptError::new(
                        "ADP-C1",
                        format!("adapt base '{base_name}' not found: {e}"),
                        base_span,
                    )
                })?;
            }
            _ => {
                return Err(AdaptError::new(
                    "ADP-C7",
                    format!(
                        "package '{}' has multiple `adapt` bases (diamond); \
                         use a linear product line or explicit order (not yet implemented)",
                        current.name
                    ),
                    current.adapts[0].span,
                ));
            }
        }
    }

    reverse.reverse();
    Ok(reverse)
}

/// Flatten chain: seed with Base0, apply each subsequent package's patches + new items.
pub fn merge_adapt_chain(chain: &[Package]) -> Result<MergeResult, AdaptError> {
    if chain.is_empty() {
        return Err(AdaptError::new(
            "ADP-C1",
            "empty adapt chain",
            Span::new(0, 0),
        ));
    }

    let chain_names: Vec<String> = chain.iter().map(|p| p.name.clone()).collect();
    let mut merged = chain[0].clone();
    // Clear adapt meta on working copy — leaf identity applied at end.
    merged.adapts.clear();
    merged.patches.clear();

    let mut provenance: HashMap<String, Vec<String>> = HashMap::new();
    for name in top_level_names(&merged) {
        provenance.insert(name, vec![chain[0].name.clone()]);
    }

    for pkg in &chain[1..] {
        // Apply patches in source order.
        for patch in &pkg.patches {
            apply_patch(&mut merged, patch)?;
        }
        // Merge new top-level items (implicit add).
        for item in &pkg.items {
            if let Some(name) = item_name(item) {
                if top_level_names(&merged).contains(&name) {
                    // Already present (from base or earlier ins) — skip re-add;
                    // patches own structural changes.
                    continue;
                }
                provenance
                    .entry(name.clone())
                    .or_default()
                    .push(pkg.name.clone());
            }
            merged.items.push(item.clone());
        }
        // Union uses/links (dedupe by package/crate name).
        for u in &pkg.uses {
            if !merged.uses.iter().any(|x| x.package_name == u.package_name) {
                merged.uses.push(u.clone());
            }
        }
        for l in &pkg.links {
            if !merged.links.iter().any(|x| x.name == l.name) {
                merged.links.push(l.clone());
            }
        }
        // Leaf expose wins if present.
        if pkg.expose.is_some() {
            merged.expose = pkg.expose.clone();
        }
        // Record patch touch as provenance for renamed/ins targets is approximate:
        // re-scan all names after this package.
        for name in top_level_names(&merged) {
            provenance
                .entry(name)
                .or_default()
                .push(pkg.name.clone());
        }
    }

    // Dedup provenance lists while preserving order.
    for v in provenance.values_mut() {
        let mut seen = HashSet::new();
        v.retain(|n| seen.insert(n.clone()));
    }

    let leaf = chain.last().unwrap();
    merged.name = leaf.name.clone();
    merged.version = leaf.version.clone();
    merged.span = leaf.span;
    merged.metadata = leaf.metadata.clone();
    // Keep leaf adapt decls for IDE badge / serialize of source — but merged
    // product is a flat package without patches.
    merged.adapts.clear();
    merged.patches.clear();

    Ok(MergeResult {
        package: merged,
        chain: chain_names,
        provenance,
    })
}

/// High-level: load chain from leaf + loader, then merge.
pub fn merge_adapted_package(
    leaf: &Package,
    load: impl FnMut(&str) -> Result<Package, String>,
) -> Result<MergeResult, AdaptError> {
    if leaf.adapts.is_empty() && leaf.patches.is_empty() {
        // No adapt work — return leaf as-is.
        let mut provenance = HashMap::new();
        for name in top_level_names(leaf) {
            provenance.insert(name, vec![leaf.name.clone()]);
        }
        return Ok(MergeResult {
            package: leaf.clone(),
            chain: vec![leaf.name.clone()],
            provenance,
        });
    }
    let chain = build_adapt_chain(leaf, load)?;
    merge_adapt_chain(&chain)
}

// ─── Path addressing (ADP-004) ─────────────────────────────────────────────

fn top_level_names(pkg: &Package) -> Vec<String> {
    pkg.items.iter().filter_map(item_name).collect()
}

fn item_name(item: &TopLevelItem) -> Option<String> {
    match item {
        TopLevelItem::Construct(c) => Some(c.name.clone()),
        TopLevelItem::Function(f) => Some(f.name.clone()),
        TopLevelItem::Flow(f) => Some(f.name.clone()),
        TopLevelItem::TypeAlias { name, .. } => Some(name.clone()),
        TopLevelItem::Const { name, .. } => Some(name.clone()),
        TopLevelItem::Static { name, .. } => Some(name.clone()),
        TopLevelItem::Lang(_) => None,
    }
}

/// Find a construct by name anywhere in the package (top-level or nested children).
fn find_construct_mut<'a>(pkg: &'a mut Package, name: &str) -> Option<&'a mut Construct> {
    for item in &mut pkg.items {
        if let TopLevelItem::Construct(c) = item {
            if let Some(found) = find_construct_in_mut(c, name) {
                return Some(found);
            }
        }
    }
    None
}

fn find_construct_in_mut<'a>(c: &'a mut Construct, name: &str) -> Option<&'a mut Construct> {
    if c.name == name {
        return Some(c);
    }
    for child in &mut c.children {
        if let Some(found) = find_construct_in_mut(child, name) {
            return Some(found);
        }
    }
    // Also search fns? No — methods are FnDef.
    None
}

fn find_construct_ref<'a>(pkg: &'a Package, name: &str) -> Option<&'a Construct> {
    for item in &pkg.items {
        if let TopLevelItem::Construct(c) = item {
            if let Some(found) = find_construct_in_ref(c, name) {
                return Some(found);
            }
        }
    }
    None
}

fn find_construct_in_ref<'a>(c: &'a Construct, name: &str) -> Option<&'a Construct> {
    if c.name == name {
        return Some(c);
    }
    for child in &c.children {
        if let Some(found) = find_construct_in_ref(child, name) {
            return Some(found);
        }
    }
    None
}

/// Resolve path existence (for diagnostics / tests).
pub fn path_exists(pkg: &Package, path: &AdaptPath) -> bool {
    resolve_path_target(pkg, path).is_ok()
}

fn resolve_path_target(pkg: &Package, path: &AdaptPath) -> Result<(), AdaptError> {
    if path.segments.is_empty() {
        return Err(AdaptError::new(
            "ADP-C3",
            "empty adapt path",
            Span::new(0, 0),
        ));
    }
    let first = match &path.segments[0] {
        AdaptPathSeg::Name(n) => n.as_str(),
        other => {
            return Err(AdaptError::new(
                "ADP-C3",
                format!("path must start with a name, got {:?}", other),
                Span::new(0, 0),
            ));
        }
    };

    // Free fn?
    if path.segments.len() == 1 {
        if pkg.items.iter().any(|i| matches!(i, TopLevelItem::Function(f) if f.name == first)) {
            return Ok(());
        }
        if find_construct_ref(pkg, first).is_some() {
            return Ok(());
        }
        return Err(AdaptError::new(
            "ADP-C3",
            format!("adapt path not found: {}", path.display()),
            Span::new(0, 0),
        ));
    }

    let c = find_construct_ref(pkg, first).ok_or_else(|| {
        AdaptError::new(
            "ADP-C3",
            format!("adapt path not found: {}", path.display()),
            Span::new(0, 0),
        )
    })?;

    for seg in &path.segments[1..] {
        match seg {
            AdaptPathSeg::Step(sname) => {
                let has = c.steps.iter().any(|st| match st {
                    FlowStep::Step(sd) => sd.name == *sname,
                    _ => false,
                });
                if !has {
                    return Err(AdaptError::new(
                        "ADP-C3",
                        format!("step '{sname}' not found on '{}'", c.name),
                        Span::new(0, 0),
                    ));
                }
            }
            AdaptPathSeg::Fn(fname) => {
                let has = c.fns.iter().any(|f| f.name == *fname)
                    || c.children.iter().any(|ch| ch.name == *fname && ch.shape == crate::layer::Shape::Fn);
                if !has {
                    return Err(AdaptError::new(
                        "ADP-C3",
                        format!("fn '{fname}' not found on '{}'", c.name),
                        Span::new(0, 0),
                    ));
                }
            }
            AdaptPathSeg::Name(n) => {
                // Nested construct by name under children
                if find_construct_in_ref(c, n).is_none() {
                    return Err(AdaptError::new(
                        "ADP-C3",
                        format!("nested '{n}' not found under '{}'", c.name),
                        Span::new(0, 0),
                    ));
                }
            }
        }
    }
    Ok(())
}

// ─── Patch application ─────────────────────────────────────────────────────

fn apply_patch(pkg: &mut Package, patch: &AdaptPatch) -> Result<(), AdaptError> {
    match patch {
        AdaptPatch::Ren {
            path,
            new_name,
            span,
        } => apply_ren(pkg, path, new_name, *span),
        AdaptPatch::Omit { path, span } => apply_omit(pkg, path, *span),
        AdaptPatch::Ins {
            path,
            position: _,
            items,
            span,
        } => apply_ins(pkg, path, items, *span),
        AdaptPatch::Rpl {
            path,
            steps,
            body,
            span,
        } => apply_rpl(pkg, path, steps, body, *span),
        AdaptPatch::Rfn {
            path,
            steps,
            body,
            span,
        } => apply_rfn(pkg, path, steps, body, *span),
    }
}

fn apply_ren(
    pkg: &mut Package,
    path: &AdaptPath,
    new_name: &str,
    span: Span,
) -> Result<(), AdaptError> {
    let old = match path.segments.first() {
        Some(AdaptPathSeg::Name(n)) if path.segments.len() == 1 => n.clone(),
        _ => {
            return Err(AdaptError::new(
                "ADP-C3",
                format!("ren only supports top-level symbol paths, got {}", path.display()),
                span,
            ));
        }
    };

    // Collision
    if top_level_names(pkg).iter().any(|n| n == new_name) {
        return Err(AdaptError::new(
            "ADP-C6",
            format!("ren target name '{new_name}' already exists"),
            span,
        ));
    }

    let mut found = false;
    for item in &mut pkg.items {
        match item {
            TopLevelItem::Construct(c) if c.name == old => {
                c.name = new_name.to_string();
                found = true;
            }
            TopLevelItem::Function(f) if f.name == old => {
                f.name = new_name.to_string();
                found = true;
            }
            TopLevelItem::Flow(f) if f.name == old => {
                f.name = new_name.to_string();
                found = true;
            }
            TopLevelItem::TypeAlias { name, .. } if name == &old => {
                *name = new_name.to_string();
                found = true;
            }
            _ => {}
        }
    }

    // Nested rename by deep search
    if !found {
        if let Some(c) = find_construct_mut(pkg, &old) {
            c.name = new_name.to_string();
            found = true;
        }
    }

    if !found {
        return Err(AdaptError::new(
            "ADP-C3",
            format!("ren path not found: {}", path.display()),
            span,
        ));
    }

    // Rewrite references in merged IR (identifiers + call targets).
    rewrite_name_in_package(pkg, &old, new_name);

    // Expose entries
    if let Some(expose) = &mut pkg.expose {
        for node in &mut expose.nodes {
            if node.name == old {
                node.name = new_name.to_string();
            }
        }
    }

    Ok(())
}

fn rewrite_name_in_package(pkg: &mut Package, old: &str, new: &str) {
    for item in &mut pkg.items {
        rewrite_name_in_item(item, old, new);
    }
}

fn rewrite_name_in_item(item: &mut TopLevelItem, old: &str, new: &str) {
    match item {
        TopLevelItem::Construct(c) => rewrite_name_in_construct(c, old, new),
        TopLevelItem::Function(f) => {
            for e in &mut f.body {
                rewrite_name_in_expr(e, old, new);
            }
        }
        TopLevelItem::Flow(flow) => {
            for st in &mut flow.steps {
                rewrite_name_in_flow_step(st, old, new);
            }
        }
        _ => {}
    }
}

fn rewrite_name_in_construct(c: &mut Construct, old: &str, new: &str) {
    for child in &mut c.children {
        rewrite_name_in_construct(child, old, new);
    }
    for f in &mut c.fns {
        for e in &mut f.body {
            rewrite_name_in_expr(e, old, new);
        }
    }
    for st in &mut c.steps {
        rewrite_name_in_flow_step(st, old, new);
    }
    if let Some(re) = &mut c.return_expr {
        rewrite_name_in_expr(re, old, new);
    }
}

fn rewrite_name_in_flow_step(st: &mut FlowStep, old: &str, new: &str) {
    match st {
        FlowStep::Step(sd) => {
            for e in &mut sd.body {
                rewrite_name_in_expr(e, old, new);
            }
            for sb in &mut sd.sub_blocks {
                for e in &mut sb.body {
                    rewrite_name_in_expr(e, old, new);
                }
            }
        }
        FlowStep::Parallel(par) => {
            for s in &mut par.steps {
                for e in &mut s.body {
                    rewrite_name_in_expr(e, old, new);
                }
            }
        }
        FlowStep::Match(_) => {}
    }
}

fn rewrite_name_in_expr(e: &mut Expr, old: &str, new: &str) {
    match e {
        Expr::Ident(n) if n == old => *n = new.to_string(),
        Expr::FieldAccess(base, _) => rewrite_name_in_expr(base, old, new),
        Expr::Call(c) => {
            if c.target == old {
                c.target = new.to_string();
            }
            if let Some(recv) = &mut c.receiver {
                rewrite_name_in_expr(recv, old, new);
            }
            for a in &mut c.args {
                rewrite_name_in_expr(a, old, new);
            }
        }
        Expr::BinaryOp(b) => {
            rewrite_name_in_expr(&mut b.left, old, new);
            rewrite_name_in_expr(&mut b.right, old, new);
        }
        Expr::UnaryOp(u) => rewrite_name_in_expr(&mut u.expr, old, new),
        Expr::IfExpr(i) => {
            rewrite_name_in_expr(&mut i.condition, old, new);
            for x in &mut i.then_body {
                rewrite_name_in_expr(x, old, new);
            }
            if let Some(else_body) = &mut i.else_body {
                for x in else_body {
                    rewrite_name_in_expr(x, old, new);
                }
            }
        }
        Expr::Assign(_, v, _) | Expr::MutAssign(_, v, _) => rewrite_name_in_expr(v, old, new),
        Expr::Return(inner) | Expr::Await(inner) | Expr::Try(inner) => {
            rewrite_name_in_expr(inner, old, new)
        }
        Expr::StructLit(name, fields) => {
            if name == old {
                *name = new.to_string();
            }
            for (_, v) in fields {
                rewrite_name_in_expr(v, old, new);
            }
        }
        Expr::Match(scrut, arms) => {
            rewrite_name_in_expr(scrut, old, new);
            for arm in arms {
                for x in &mut arm.body {
                    rewrite_name_in_expr(x, old, new);
                }
            }
        }
        Expr::ArrayLit(items) | Expr::Tuple(items) | Expr::Loop(items) => {
            for x in items {
                rewrite_name_in_expr(x, old, new);
            }
        }
        Expr::Action(a) => {
            if a.target == old {
                a.target = new.to_string();
            }
            for x in &mut a.args {
                rewrite_name_in_expr(x, old, new);
            }
            if let Some(c) = &mut a.condition {
                rewrite_name_in_expr(c, old, new);
            }
        }
        Expr::Stock
        | Expr::StringLit(_)
        | Expr::IntLit(_)
        | Expr::FloatLit(_)
        | Expr::BoolLit(_)
        | Expr::Break
        | Expr::Continue => {}
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } | Expr::Closure { body, .. } => {
            for x in body {
                rewrite_name_in_expr(x, old, new);
            }
        }
        Expr::WhileLet { expr, body, .. } => {
            rewrite_name_in_expr(expr, old, new);
            for x in body {
                rewrite_name_in_expr(x, old, new);
            }
        }
        Expr::LetPattern(_, expr, _) => rewrite_name_in_expr(expr, old, new),
        Expr::StringInterp(parts) => {
            for p in parts {
                if let crate::ast::StringPart::Expr(x) = p {
                    rewrite_name_in_expr(x, old, new);
                }
            }
        }
        // Catch remaining variants without exhaustive failure on future adds
        _ => {}
    }
}

fn apply_omit(pkg: &mut Package, path: &AdaptPath, span: Span) -> Result<(), AdaptError> {
    if path.segments.is_empty() {
        return Err(AdaptError::new("ADP-C3", "omit empty path", span));
    }

    // omit Construct.step name
    if path.segments.len() == 2 {
        if let (AdaptPathSeg::Name(cname), AdaptPathSeg::Step(sname)) =
            (&path.segments[0], &path.segments[1])
        {
            let c = find_construct_mut(pkg, cname).ok_or_else(|| {
                AdaptError::new(
                    "ADP-C3",
                    format!("omit path not found: {}", path.display()),
                    span,
                )
            })?;
            let before = c.steps.len();
            c.steps.retain(|st| match st {
                FlowStep::Step(sd) => sd.name != *sname,
                _ => true,
            });
            if c.steps.len() == before {
                return Err(AdaptError::new(
                    "ADP-C3",
                    format!("omit step not found: {}", path.display()),
                    span,
                ));
            }
            return Ok(());
        }
        if let (AdaptPathSeg::Name(cname), AdaptPathSeg::Fn(fname)) =
            (&path.segments[0], &path.segments[1])
        {
            let c = find_construct_mut(pkg, cname).ok_or_else(|| {
                AdaptError::new(
                    "ADP-C3",
                    format!("omit path not found: {}", path.display()),
                    span,
                )
            })?;
            let before = c.fns.len();
            c.fns.retain(|f| f.name != *fname);
            if c.fns.len() == before {
                // try children
                let before_ch = c.children.len();
                c.children.retain(|ch| ch.name != *fname);
                if c.children.len() == before_ch {
                    return Err(AdaptError::new(
                        "ADP-C3",
                        format!("omit fn not found: {}", path.display()),
                        span,
                    ));
                }
            }
            return Ok(());
        }
    }

    // omit top-level / nested by name
    if path.segments.len() == 1 {
        if let AdaptPathSeg::Name(name) = &path.segments[0] {
            let before = pkg.items.len();
            pkg.items.retain(|item| item_name(item).as_deref() != Some(name.as_str()));
            if pkg.items.len() != before {
                // Also strip expose
                if let Some(expose) = &mut pkg.expose {
                    expose.nodes.retain(|n| n.name != *name);
                }
                return Ok(());
            }
            // Nested: remove child from parent
            if remove_nested_construct(pkg, name) {
                return Ok(());
            }
            return Err(AdaptError::new(
                "ADP-C3",
                format!("omit path not found: {}", path.display()),
                span,
            ));
        }
    }

    Err(AdaptError::new(
        "ADP-C3",
        format!("unsupported omit path: {}", path.display()),
        span,
    ))
}

fn remove_nested_construct(pkg: &mut Package, name: &str) -> bool {
    for item in &mut pkg.items {
        if let TopLevelItem::Construct(c) = item {
            if remove_child_named(c, name) {
                return true;
            }
        }
    }
    false
}

fn remove_child_named(c: &mut Construct, name: &str) -> bool {
    let before = c.children.len();
    c.children.retain(|ch| ch.name != name);
    if c.children.len() != before {
        return true;
    }
    for ch in &mut c.children {
        if remove_child_named(ch, name) {
            return true;
        }
    }
    false
}

fn apply_ins(
    pkg: &mut Package,
    path: &AdaptPath,
    items: &[AdaptInsItem],
    span: Span,
) -> Result<(), AdaptError> {
    let cname = match path.segments.first() {
        Some(AdaptPathSeg::Name(n)) if path.segments.len() == 1 => n.clone(),
        _ => {
            return Err(AdaptError::new(
                "ADP-C3",
                format!("ins path must be a construct name, got {}", path.display()),
                span,
            ));
        }
    };

    let c = find_construct_mut(pkg, &cname).ok_or_else(|| {
        AdaptError::new(
            "ADP-C3",
            format!("ins path not found: {}", path.display()),
            span,
        )
    })?;

    for item in items {
        match item {
            AdaptInsItem::Step { step, position } => {
                insert_step(&mut c.steps, step.clone(), position)?;
            }
            AdaptInsItem::Function(f) => {
                if c.fns.iter().any(|x| x.name == f.name) {
                    return Err(AdaptError::new(
                        "ADP-C6",
                        format!("ins fn '{}' already exists on '{}'", f.name, cname),
                        span,
                    ));
                }
                c.fns.push(f.clone());
            }
            AdaptInsItem::Construct(child) => {
                if c.children.iter().any(|x| x.name == child.name) {
                    return Err(AdaptError::new(
                        "ADP-C6",
                        format!(
                            "ins construct '{}' already exists on '{}'",
                            child.name, cname
                        ),
                        span,
                    ));
                }
                c.children.push(child.clone());
            }
        }
    }
    Ok(())
}

fn insert_step(
    steps: &mut Vec<FlowStep>,
    step: FlowStep,
    position: &StepPosition,
) -> Result<(), AdaptError> {
    match position {
        StepPosition::AtEnd => {
            steps.push(step);
        }
        StepPosition::AtStart => {
            steps.insert(0, step);
        }
        StepPosition::Before(name) => {
            let idx = steps.iter().position(|st| match st {
                FlowStep::Step(sd) => sd.name == *name,
                _ => false,
            });
            match idx {
                Some(i) => steps.insert(i, step),
                None => {
                    return Err(AdaptError::new(
                        "ADP-C3",
                        format!("ins before unknown step '{name}'"),
                        Span::new(0, 0),
                    ));
                }
            }
        }
        StepPosition::After(name) => {
            let idx = steps.iter().position(|st| match st {
                FlowStep::Step(sd) => sd.name == *name,
                _ => false,
            });
            match idx {
                Some(i) => steps.insert(i + 1, step),
                None => {
                    return Err(AdaptError::new(
                        "ADP-C3",
                        format!("ins after unknown step '{name}'"),
                        Span::new(0, 0),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn apply_rpl(
    pkg: &mut Package,
    path: &AdaptPath,
    steps: &[FlowStep],
    body: &[Expr],
    span: Span,
) -> Result<(), AdaptError> {
    if body_contains_stock_steps(steps) || body.iter().any(expr_contains_stock) {
        return Err(AdaptError::new(
            "ADP-C5",
            "stock is illegal inside rpl (use rfn to refine with stock)",
            span,
        ));
    }
    replace_body(pkg, path, steps, body, span, /*is_rfn*/ false)
}

fn apply_rfn(
    pkg: &mut Package,
    path: &AdaptPath,
    steps: &[FlowStep],
    body: &[Expr],
    span: Span,
) -> Result<(), AdaptError> {
    // Snapshot prior body, then expand stock in new body.
    let prior = snapshot_body(pkg, path, span)?;
    let mut new_steps = steps.to_vec();
    let mut new_body = body.to_vec();
    expand_stock_in_steps(&mut new_steps, &prior, span)?;
    expand_stock_in_exprs(&mut new_body, &prior, span)?;
    replace_body(pkg, path, &new_steps, &new_body, span, /*is_rfn*/ true)
}

#[derive(Clone)]
struct BodySnapshot {
    steps: Vec<FlowStep>,
    body: Vec<Expr>,
    return_expr: Option<Expr>,
}

fn snapshot_body(
    pkg: &Package,
    path: &AdaptPath,
    span: Span,
) -> Result<BodySnapshot, AdaptError> {
    let cname = match path.segments.first() {
        Some(AdaptPathSeg::Name(n)) => n.as_str(),
        _ => {
            return Err(AdaptError::new(
                "ADP-C3",
                format!("invalid path {}", path.display()),
                span,
            ));
        }
    };

    if path.segments.len() == 1 {
        // Free function?
        if let Some(TopLevelItem::Function(f)) = pkg
            .items
            .iter()
            .find(|i| matches!(i, TopLevelItem::Function(f) if f.name == cname))
        {
            return Ok(BodySnapshot {
                steps: Vec::new(),
                body: f.body.clone(),
                return_expr: None,
            });
        }
        if let Some(c) = find_construct_ref(pkg, cname) {
            return Ok(BodySnapshot {
                steps: c.steps.clone(),
                body: Vec::new(),
                return_expr: c.return_expr.as_ref().map(|e| *e.clone()),
            });
        }
    }

    if path.segments.len() == 2 {
        if let (AdaptPathSeg::Name(cn), AdaptPathSeg::Fn(fn_name)) =
            (&path.segments[0], &path.segments[1])
        {
            let c = find_construct_ref(pkg, cn).ok_or_else(|| {
                AdaptError::new("ADP-C3", format!("path not found: {}", path.display()), span)
            })?;
            if let Some(f) = c.fns.iter().find(|f| f.name == *fn_name) {
                return Ok(BodySnapshot {
                    steps: Vec::new(),
                    body: f.body.clone(),
                    return_expr: None,
                });
            }
        }
        if let (AdaptPathSeg::Name(cn), AdaptPathSeg::Step(sname)) =
            (&path.segments[0], &path.segments[1])
        {
            let c = find_construct_ref(pkg, cn).ok_or_else(|| {
                AdaptError::new("ADP-C3", format!("path not found: {}", path.display()), span)
            })?;
            if let Some(FlowStep::Step(sd)) = c.steps.iter().find(|st| match st {
                FlowStep::Step(sd) => sd.name == *sname,
                _ => false,
            }) {
                return Ok(BodySnapshot {
                    steps: Vec::new(),
                    body: sd.body.clone(),
                    return_expr: None,
                });
            }
        }
    }

    Err(AdaptError::new(
        "ADP-C3",
        format!("rfn/rpl path not found: {}", path.display()),
        span,
    ))
}

fn replace_body(
    pkg: &mut Package,
    path: &AdaptPath,
    steps: &[FlowStep],
    body: &[Expr],
    span: Span,
    _is_rfn: bool,
) -> Result<(), AdaptError> {
    let cname = match path.segments.first() {
        Some(AdaptPathSeg::Name(n)) => n.clone(),
        _ => {
            return Err(AdaptError::new(
                "ADP-C3",
                format!("invalid path {}", path.display()),
                span,
            ));
        }
    };

    if path.segments.len() == 1 {
        // Free fn
        for item in &mut pkg.items {
            if let TopLevelItem::Function(f) = item {
                if f.name == cname {
                    if !body.is_empty() {
                        f.body = body.to_vec();
                    } else if !steps.is_empty() {
                        // Flatten steps into body exprs
                        f.body = flatten_steps_to_exprs(steps);
                    } else {
                        f.body.clear();
                    }
                    return Ok(());
                }
            }
        }
        if let Some(c) = find_construct_mut(pkg, &cname) {
            // Signature check ADP-C8: we keep inputs as-is (body only).
            if !steps.is_empty() || body.is_empty() {
                c.steps = steps.to_vec();
            }
            if !body.is_empty() && steps.is_empty() {
                // Expression-only replace → single synthetic step if construct is fn-shaped
                c.steps = vec![FlowStep::Step(StepDef {
                    name: "body".into(),
                    span,
                    body: body.to_vec(),
                    refs: Vec::new(),
                    sub_blocks: Vec::new(),
                })];
            }
            c.return_expr = None;
            return Ok(());
        }
        return Err(AdaptError::new(
            "ADP-C3",
            format!("rfn/rpl path not found: {}", path.display()),
            span,
        ));
    }

    if path.segments.len() == 2 {
        if let AdaptPathSeg::Fn(fn_name) = &path.segments[1] {
            let c = find_construct_mut(pkg, &cname).ok_or_else(|| {
                AdaptError::new("ADP-C3", format!("path not found: {}", path.display()), span)
            })?;
            if let Some(f) = c.fns.iter_mut().find(|f| f.name == *fn_name) {
                f.body = if !body.is_empty() {
                    body.to_vec()
                } else {
                    flatten_steps_to_exprs(steps)
                };
                return Ok(());
            }
        }
        if let AdaptPathSeg::Step(sname) = &path.segments[1] {
            let c = find_construct_mut(pkg, &cname).ok_or_else(|| {
                AdaptError::new("ADP-C3", format!("path not found: {}", path.display()), span)
            })?;
            if let Some(FlowStep::Step(sd)) = c.steps.iter_mut().find(|st| match st {
                FlowStep::Step(sd) => sd.name == *sname,
                _ => false,
            }) {
                sd.body = if !body.is_empty() {
                    body.to_vec()
                } else {
                    flatten_steps_to_exprs(steps)
                };
                return Ok(());
            }
        }
    }

    Err(AdaptError::new(
        "ADP-C3",
        format!("rfn/rpl path not found: {}", path.display()),
        span,
    ))
}

fn flatten_steps_to_exprs(steps: &[FlowStep]) -> Vec<Expr> {
    let mut out = Vec::new();
    for st in steps {
        if let FlowStep::Step(sd) = st {
            out.extend(sd.body.clone());
        }
    }
    out
}

fn body_contains_stock_steps(steps: &[FlowStep]) -> bool {
    steps.iter().any(|st| match st {
        FlowStep::Step(sd) => sd.body.iter().any(expr_contains_stock),
        FlowStep::Parallel(par) => par
            .steps
            .iter()
            .any(|s| s.body.iter().any(expr_contains_stock)),
        FlowStep::Match(_) => false,
    })
}

fn expr_contains_stock(e: &Expr) -> bool {
    match e {
        Expr::Stock => true,
        Expr::FieldAccess(b, _) => expr_contains_stock(b),
        Expr::Call(c) => {
            c.args.iter().any(expr_contains_stock)
                || c.receiver.as_ref().map(|r| expr_contains_stock(r)).unwrap_or(false)
        }
        Expr::BinaryOp(b) => expr_contains_stock(&b.left) || expr_contains_stock(&b.right),
        Expr::UnaryOp(u) => expr_contains_stock(&u.expr),
        Expr::IfExpr(i) => {
            expr_contains_stock(&i.condition)
                || i.then_body.iter().any(expr_contains_stock)
                || i.else_body
                    .as_ref()
                    .map(|b| b.iter().any(expr_contains_stock))
                    .unwrap_or(false)
        }
        Expr::Assign(_, v, _) | Expr::MutAssign(_, v, _) => expr_contains_stock(v),
        Expr::Return(i) | Expr::Await(i) | Expr::Try(i) => expr_contains_stock(i),
        Expr::StructLit(_, fields) => fields.iter().any(|(_, v)| expr_contains_stock(v)),
        Expr::ArrayLit(xs) | Expr::Tuple(xs) | Expr::Loop(xs) => {
            xs.iter().any(expr_contains_stock)
        }
        Expr::Match(s, arms) => {
            expr_contains_stock(s) || arms.iter().any(|a| a.body.iter().any(expr_contains_stock))
        }
        Expr::Action(a) => {
            a.args.iter().any(expr_contains_stock)
                || a.condition.as_ref().map(|c| expr_contains_stock(c)).unwrap_or(false)
        }
        Expr::ForLoop { body, .. } | Expr::WhileLoop { body, .. } | Expr::Closure { body, .. } => {
            body.iter().any(expr_contains_stock)
        }
        Expr::WhileLet { expr, body, .. } => {
            expr_contains_stock(expr) || body.iter().any(expr_contains_stock)
        }
        Expr::LetPattern(_, expr, _) => expr_contains_stock(expr),
        Expr::StringInterp(parts) => parts.iter().any(|p| {
            matches!(p, crate::ast::StringPart::Expr(e) if expr_contains_stock(e))
        }),
        _ => false,
    }
}

fn expand_stock_in_steps(
    steps: &mut [FlowStep],
    prior: &BodySnapshot,
    span: Span,
) -> Result<(), AdaptError> {
    for st in steps.iter_mut() {
        if let FlowStep::Step(sd) = st {
            expand_stock_in_exprs(&mut sd.body, prior, span)?;
        }
    }
    Ok(())
}

/// Expand `stock` placeholders. Hygienic: prior locals get `stock_` prefix on collision.
fn expand_stock_in_exprs(
    exprs: &mut Vec<Expr>,
    prior: &BodySnapshot,
    span: Span,
) -> Result<(), AdaptError> {
    let mut out = Vec::new();
    for e in exprs.drain(..) {
        expand_one(e, prior, span, &mut out)?;
    }
    *exprs = out;
    Ok(())
}

fn expand_one(
    e: Expr,
    prior: &BodySnapshot,
    span: Span,
    out: &mut Vec<Expr>,
) -> Result<(), AdaptError> {
    match e {
        Expr::Stock => {
            // Statement form: splice prior steps/body with hygiene.
            let mut prior_exprs = if !prior.steps.is_empty() {
                flatten_steps_to_exprs(&prior.steps)
            } else {
                prior.body.clone()
            };
            if let Some(re) = &prior.return_expr {
                // Don't add bare return as stock value in statement form — keep as ret.
                prior_exprs.push(Expr::Return(Box::new(re.clone())));
            }
            let refining_names = collect_assign_names(out);
            let hygienic = apply_hygiene(prior_exprs, &refining_names);
            out.extend(hygienic);
        }
        Expr::Assign(name, rhs, ty) if matches!(rhs.as_ref(), Expr::Stock) => {
            // Expression form: `x = stock` binds last value of prior body.
            let mut prior_exprs = if !prior.steps.is_empty() {
                flatten_steps_to_exprs(&prior.steps)
            } else {
                prior.body.clone()
            };
            let refining_names = {
                let mut n = collect_assign_names(out);
                n.insert(name.clone());
                n
            };
            let mut hygienic = apply_hygiene(prior_exprs.drain(..).collect(), &refining_names);

            // Determine value: last return or last expr.
            let value = extract_stock_value(&mut hygienic, prior);
            out.extend(hygienic);
            out.push(Expr::Assign(name, Box::new(value), ty));
        }
        Expr::MutAssign(name, rhs, ty) if matches!(rhs.as_ref(), Expr::Stock) => {
            let mut prior_exprs = if !prior.steps.is_empty() {
                flatten_steps_to_exprs(&prior.steps)
            } else {
                prior.body.clone()
            };
            let refining_names = {
                let mut n = collect_assign_names(out);
                n.insert(name.clone());
                n
            };
            let mut hygienic = apply_hygiene(prior_exprs.drain(..).collect(), &refining_names);
            let value = extract_stock_value(&mut hygienic, prior);
            out.extend(hygienic);
            out.push(Expr::MutAssign(name, Box::new(value), ty));
        }
        other => {
            // Recurse into nested structures that may contain stock.
            let mut e = other;
            rewrite_nested_stock(&mut e, prior, span)?;
            out.push(e);
        }
    }
    Ok(())
}

fn extract_stock_value(hygienic: &mut Vec<Expr>, prior: &BodySnapshot) -> Expr {
    if let Some(re) = &prior.return_expr {
        return re.clone();
    }
    // Pop trailing return if present
    if let Some(Expr::Return(inner)) = hygienic.last().cloned() {
        hygienic.pop();
        return *inner;
    }
    // Last non-unit expr as value — if last is assign, use its name
    if let Some(last) = hygienic.last().cloned() {
        match last {
            Expr::Assign(n, _, _) | Expr::MutAssign(n, _, _) => Expr::Ident(n),
            Expr::Return(inner) => {
                hygienic.pop();
                *inner
            }
            other => {
                // Keep statement and also use Unit? Prefer Ident of last assign.
                // If it's a call etc., leave it and use Unit.
                let _ = other;
                Expr::Tuple(Vec::new())
            }
        }
    } else {
        Expr::Tuple(Vec::new())
    }
}

fn collect_assign_names(exprs: &[Expr]) -> HashSet<String> {
    let mut set = HashSet::new();
    for e in exprs {
        match e {
            Expr::Assign(n, _, _) | Expr::MutAssign(n, _, _) => {
                set.insert(n.clone());
            }
            _ => {}
        }
    }
    set
}

fn apply_hygiene(exprs: Vec<Expr>, refining_names: &HashSet<String>) -> Vec<Expr> {
    let prior_names = collect_assign_names(&exprs);
    let mut rename: HashMap<String, String> = HashMap::new();
    for n in &prior_names {
        if refining_names.contains(n) {
            rename.insert(n.clone(), format!("stock_{n}"));
        }
    }
    if rename.is_empty() {
        return exprs;
    }
    exprs
        .into_iter()
        .map(|e| rename_locals(e, &rename))
        .collect()
}

fn rename_locals(e: Expr, map: &HashMap<String, String>) -> Expr {
    match e {
        Expr::Ident(n) => Expr::Ident(map.get(&n).cloned().unwrap_or(n)),
        Expr::Assign(n, rhs, ty) => {
            let n2 = map.get(&n).cloned().unwrap_or(n);
            Expr::Assign(n2, Box::new(rename_locals(*rhs, map)), ty)
        }
        Expr::MutAssign(n, rhs, ty) => {
            let n2 = map.get(&n).cloned().unwrap_or(n);
            Expr::MutAssign(n2, Box::new(rename_locals(*rhs, map)), ty)
        }
        Expr::Return(i) => Expr::Return(Box::new(rename_locals(*i, map))),
        Expr::Call(mut c) => {
            c.args = c.args.into_iter().map(|a| rename_locals(a, map)).collect();
            if let Some(r) = c.receiver {
                c.receiver = Some(Box::new(rename_locals(*r, map)));
            }
            Expr::Call(c)
        }
        Expr::BinaryOp(mut b) => {
            b.left = Box::new(rename_locals(*b.left, map));
            b.right = Box::new(rename_locals(*b.right, map));
            Expr::BinaryOp(b)
        }
        Expr::FieldAccess(b, f) => Expr::FieldAccess(Box::new(rename_locals(*b, map)), f),
        other => other,
    }
}

fn rewrite_nested_stock(
    e: &mut Expr,
    prior: &BodySnapshot,
    span: Span,
) -> Result<(), AdaptError> {
    // If stock appears nested (e.g. in if body), expand those bodies.
    match e {
        Expr::IfExpr(i) => {
            expand_stock_in_exprs(&mut i.then_body, prior, span)?;
            if let Some(else_body) = &mut i.else_body {
                expand_stock_in_exprs(else_body, prior, span)?;
            }
            if matches!(i.condition.as_ref(), Expr::Stock) {
                return Err(AdaptError::new(
                    "ADP-C4",
                    "stock as condition is not supported; use `x = stock` then branch",
                    span,
                ));
            }
        }
        Expr::Stock => {
            return Err(AdaptError::new(
                "ADP-C4",
                "nested bare stock not supported; use statement-level stock or `x = stock`",
                span,
            ));
        }
        _ => {}
    }
    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Shape;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn empty_pkg(name: &str) -> Package {
        Package {
            name: name.into(),
            version: None,
            span: sp(),
            metadata: vec![],
            uses: vec![],
            links: vec![],
            adapts: vec![],
            patches: vec![],
            items: vec![],
            expose: None,
        }
    }

    fn svc(name: &str, steps: Vec<(&str, Vec<Expr>)>) -> Construct {
        let mut c = Construct::new("svc", "Service", Shape::Fn, name.into(), sp());
        c.steps = steps
            .into_iter()
            .map(|(n, body)| {
                FlowStep::Step(StepDef {
                    name: n.into(),
                    span: sp(),
                    body,
                    refs: vec![],
                    sub_blocks: vec![],
                })
            })
            .collect();
        c
    }

    #[test]
    fn denylist_dlx_core() {
        assert!(is_adapt_denied("dlx_core"));
        assert!(!is_adapt_denied("wear_test"));
    }

    #[test]
    fn chain_linear_three_level() {
        let mut base = empty_pkg("stock");
        base.items
            .push(TopLevelItem::Construct(svc("CreateX", vec![("go", vec![])])));

        let mut mid = empty_pkg("regional");
        mid.adapts.push(AdaptDecl {
            package_name: "stock".into(),
            span: sp(),
        });

        let mut leaf = empty_pkg("acme");
        leaf.adapts.push(AdaptDecl {
            package_name: "regional".into(),
            span: sp(),
        });

        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("stock".into(), base);
        map.insert("regional".into(), mid.clone());

        let chain = build_adapt_chain(&leaf, |n| {
            map.get(n)
                .cloned()
                .ok_or_else(|| format!("missing {n}"))
        })
        .unwrap();
        assert_eq!(
            chain.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["stock", "regional", "acme"]
        );
    }

    #[test]
    fn chain_cycle_errors() {
        let mut a = empty_pkg("a");
        a.adapts.push(AdaptDecl {
            package_name: "b".into(),
            span: sp(),
        });
        let mut b = empty_pkg("b");
        b.adapts.push(AdaptDecl {
            package_name: "a".into(),
            span: sp(),
        });
        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("a".into(), a.clone());
        map.insert("b".into(), b);
        let err = build_adapt_chain(&a, |n| {
            map.get(n)
                .cloned()
                .ok_or_else(|| format!("missing {n}"))
        })
        .unwrap_err();
        assert_eq!(err.code, "ADP-C7");
        assert!(err.message.contains("cycle"));
    }

    #[test]
    fn chain_denylist_errors() {
        let mut leaf = empty_pkg("bad");
        leaf.adapts.push(AdaptDecl {
            package_name: "dlx_core".into(),
            span: sp(),
        });
        let err = build_adapt_chain(&leaf, |_| unreachable!()).unwrap_err();
        assert_eq!(err.code, "ADP-C2");
    }

    #[test]
    fn path_service_and_step() {
        let mut pkg = empty_pkg("p");
        pkg.items.push(TopLevelItem::Construct(svc(
            "CreateX",
            vec![
                ("validate", vec![]),
                ("persist", vec![]),
            ],
        )));
        assert!(path_exists(&pkg, &AdaptPath::from_name("CreateX")));
        let mut p = AdaptPath::from_name("CreateX");
        p.segments.push(AdaptPathSeg::Step("persist".into()));
        assert!(path_exists(&pkg, &p));
        let mut bad = AdaptPath::from_name("CreateX");
        bad.segments.push(AdaptPathSeg::Step("nope".into()));
        assert!(!path_exists(&pkg, &bad));
    }

    #[test]
    fn ins_step_after() {
        let mut base = empty_pkg("stock");
        base.items.push(TopLevelItem::Construct(svc(
            "CreateX",
            vec![("validate", vec![]), ("persist", vec![])],
        )));
        let mut leaf = empty_pkg("client");
        leaf.adapts.push(AdaptDecl {
            package_name: "stock".into(),
            span: sp(),
        });
        leaf.patches.push(AdaptPatch::Ins {
            path: AdaptPath::from_name("CreateX"),
            position: StepPosition::AtEnd,
            items: vec![AdaptInsItem::Step {
                step: FlowStep::Step(StepDef {
                    name: "audit".into(),
                    span: sp(),
                    body: vec![Expr::Ident("Ok".into())],
                    refs: vec![],
                    sub_blocks: vec![],
                }),
                position: StepPosition::After("persist".into()),
            }],
            span: sp(),
        });
        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("stock".into(), base);
        let result = merge_adapted_package(&leaf, |n| {
            map.get(n).cloned().ok_or_else(|| format!("missing {n}"))
        })
        .unwrap();
        let c = find_construct_ref(&result.package, "CreateX").unwrap();
        let names: Vec<_> = c
            .steps
            .iter()
            .filter_map(|s| match s {
                FlowStep::Step(sd) => Some(sd.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["validate", "persist", "audit"]);
    }

    #[test]
    fn rpl_rejects_stock() {
        let mut base = empty_pkg("stock");
        base.items
            .push(TopLevelItem::Construct(svc("CreateX", vec![("go", vec![])])));
        let mut leaf = empty_pkg("client");
        leaf.adapts.push(AdaptDecl {
            package_name: "stock".into(),
            span: sp(),
        });
        leaf.patches.push(AdaptPatch::Rpl {
            path: AdaptPath::from_name("CreateX"),
            steps: vec![FlowStep::Step(StepDef {
                name: "go".into(),
                span: sp(),
                body: vec![Expr::Stock],
                refs: vec![],
                sub_blocks: vec![],
            })],
            body: vec![],
            span: sp(),
        });
        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("stock".into(), base);
        let err = merge_adapted_package(&leaf, |n| {
            map.get(n).cloned().ok_or_else(|| format!("missing {n}"))
        })
        .unwrap_err();
        assert_eq!(err.code, "ADP-C5");
    }

    #[test]
    fn rfn_expands_stock_assign() {
        let mut base = empty_pkg("stock");
        base.items.push(TopLevelItem::Construct(svc(
            "CreateX",
            vec![(
                "go",
                vec![
                    Expr::Assign("v".into(), Box::new(Expr::IntLit(1)), None),
                    Expr::Return(Box::new(Expr::Ident("v".into()))),
                ],
            )],
        )));
        let mut leaf = empty_pkg("client");
        leaf.adapts.push(AdaptDecl {
            package_name: "stock".into(),
            span: sp(),
        });
        leaf.patches.push(AdaptPatch::Rfn {
            path: AdaptPath::from_name("CreateX"),
            steps: vec![FlowStep::Step(StepDef {
                name: "wrap".into(),
                span: sp(),
                body: vec![
                    Expr::Assign("init".into(), Box::new(Expr::Stock), None),
                    Expr::Return(Box::new(Expr::Ident("init".into()))),
                ],
                refs: vec![],
                sub_blocks: vec![],
            })],
            body: vec![],
            span: sp(),
        });
        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("stock".into(), base);
        let result = merge_adapted_package(&leaf, |n| {
            map.get(n).cloned().ok_or_else(|| format!("missing {n}"))
        })
        .unwrap();
        let c = find_construct_ref(&result.package, "CreateX").unwrap();
        // No residual stock
        let flat = flatten_steps_to_exprs(&c.steps);
        assert!(!flat.iter().any(expr_contains_stock));
        // Has stock body assign + bind
        assert!(flat.iter().any(|e| matches!(e, Expr::Assign(n, _, _) if n == "v" || n == "init")));
    }

    #[test]
    fn omit_service() {
        let mut base = empty_pkg("stock");
        base.items
            .push(TopLevelItem::Construct(svc("Keep", vec![])));
        base.items
            .push(TopLevelItem::Construct(svc("Drop", vec![])));
        let mut leaf = empty_pkg("client");
        leaf.adapts.push(AdaptDecl {
            package_name: "stock".into(),
            span: sp(),
        });
        leaf.patches.push(AdaptPatch::Omit {
            path: AdaptPath::from_name("Drop"),
            span: sp(),
        });
        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("stock".into(), base);
        let result = merge_adapted_package(&leaf, |n| {
            map.get(n).cloned().ok_or_else(|| format!("missing {n}"))
        })
        .unwrap();
        assert!(find_construct_ref(&result.package, "Keep").is_some());
        assert!(find_construct_ref(&result.package, "Drop").is_none());
    }

    #[test]
    fn ren_symbol() {
        let mut base = empty_pkg("stock");
        base.items
            .push(TopLevelItem::Construct(svc("ListThings", vec![])));
        let mut leaf = empty_pkg("client");
        leaf.adapts.push(AdaptDecl {
            package_name: "stock".into(),
            span: sp(),
        });
        leaf.patches.push(AdaptPatch::Ren {
            path: AdaptPath::from_name("ListThings"),
            new_name: "ListPrograms".into(),
            span: sp(),
        });
        let mut map: HashMap<String, Package> = HashMap::new();
        map.insert("stock".into(), base);
        let result = merge_adapted_package(&leaf, |n| {
            map.get(n).cloned().ok_or_else(|| format!("missing {n}"))
        })
        .unwrap();
        assert!(find_construct_ref(&result.package, "ListPrograms").is_some());
        assert!(find_construct_ref(&result.package, "ListThings").is_none());
    }
}
