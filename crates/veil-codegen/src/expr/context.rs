//! Expression translator — converts VEIL AST Expr to Rust source code.
//!
//! Fully shape-driven: the translator uses `GenCtx.name_to_shape` to decide
//! how to emit a Call (port call → deps.x.method().await?, struct call →
//! Type::new(args), local → target.method(args)).

use std::collections::{HashMap, HashSet};

use veil_ir::ast::*;
use veil_ir::layer::{Shape, LayerRegistry};

use crate::rust::to_snake;

use super::types::{extract_inner_type, type_name_simple, register_enum_variant, rust_field_is_defaultable};

/// Error model: names for the domain error type and its variants.
/// Populated from layer-declared error types (ddd.layer `DomainError`).
/// Lets codegen emit `ErrorType::Variant(...)` without hardcoding names.
#[derive(Clone)]
pub struct ErrorModel {
    /// The error type name (e.g. "DomainError").
    pub type_name: String,
    /// The "not found" variant name (e.g. "NotFound").
    pub not_found: String,
    /// The "validation" variant name (e.g. "Validation").
    pub validation: String,
    /// The "external" variant name (e.g. "External").
    pub external: String,
}

impl Default for ErrorModel {
    fn default() -> Self {
        ErrorModel {
            type_name: "DomainError".to_string(),
            not_found: "NotFound".to_string(),
            validation: "Validation".to_string(),
            external: "External".to_string(),
        }
    }
}

impl ErrorModel {
    /// Full path for not-found variant: `DomainError::NotFound`
    pub fn not_found_path(&self) -> String {
        format!("{}::{}", self.type_name, self.not_found)
    }
    /// Full path for validation variant: `DomainError::Validation`
    pub fn validation_path(&self) -> String {
        format!("{}::{}", self.type_name, self.validation)
    }
    /// Full path for external variant: `DomainError::External`
    pub fn external_path(&self) -> String {
        format!("{}::{}", self.type_name, self.external)
    }
}

// ─── Sub-structs ─────────────────────────────────────────────────────────────

/// Type resolution context — method signatures, local variable types, struct fields.
#[derive(Clone)]
pub struct TypeContext {
    /// Method return types: (ConstructName, method_name) → inner type name.
    /// For Result<T>, stores T. For Result<()>, stores "()".
    pub method_returns: HashMap<(String, String), String>,
    /// Method parameter types: (ConstructName, method_name) → vec of param type strings.
    /// Used to decide whether Option<T> args should be auto-unwrapped at the call site.
    pub method_params: HashMap<(String, String), Vec<String>>,
    /// Ref-pass parameters: (TypeName, method_name) → vec of bools per param position.
    /// When true, codegen emits `&arg` instead of `arg.clone()` for that param.
    pub ref_params: HashMap<(String, String), Vec<bool>>,
    /// Inferred types for local variables: var_name → type_name.
    pub local_types: HashMap<String, String>,
    /// Struct field maps: type_name → vec of (field_name, field_type_name).
    pub struct_fields: HashMap<String, Vec<(String, String)>>,
}

impl TypeContext {
    pub fn new() -> Self {
        TypeContext {
            method_returns: HashMap::new(),
            method_params: HashMap::new(),
            ref_params: HashMap::new(),
            local_types: HashMap::new(),
            struct_fields: HashMap::new(),
        }
    }
}

/// Ownership and mutability context — tracks mutable locals, ident usage counts,
/// ref-element bindings, and borrow-preferring fields.
#[derive(Clone)]
pub struct OwnershipContext {
    /// Locals whose first binding must be `let mut` (reassigned, field-written,
    /// or receiver of a known mutating method). Plain `Assign` without this
    /// set emits immutable `let`. Explicit `mut x = …` always uses `let mut`.
    pub mut_locals: HashSet<String>,
    /// How many times each ident is read in the enclosing fn. 0/1 → move, no clone.
    pub ident_uses: HashMap<String, usize>,
    /// Loop bindings that are `&T` (shared-ref for). Owned slots must `.clone()`.
    pub ref_elem_locals: HashSet<String>,
    /// Fields that should use borrow (`&self.field`) instead of clone (`self.field.clone()`).
    /// Populated from stub harness_field types (Pool, Arc, Client) and adapter field types.
    pub borrow_fields: HashSet<String>,
}

impl OwnershipContext {
    pub fn new() -> Self {
        OwnershipContext {
            mut_locals: HashSet::new(),
            ident_uses: HashMap::new(),
            ref_elem_locals: HashSet::new(),
            borrow_fields: HashSet::new(),
        }
    }
}

/// Stub context — crate/type mappings, fallibility metadata, free functions.
#[derive(Clone)]
pub struct StubContext {
    /// Maps stub struct names to (crate_name, original_type_name) so codegen
    /// generates qualified paths like `aws_sdk_s3::Client::new()` when VEIL
    /// writes `S3Client.new()` (aliased) or `Client.new()` (unaliased).
    pub stub_type_crate: HashMap<String, (String, String)>,
    /// Stub free-fn constructors: type name → typed free-fn name + type-param template.
    /// From stub struct metadata `typed_variant` / `typed_type_params` (e.g. query_as).
    pub stub_typed_ctors: HashMap<String, (String, String)>,
    /// Methods whose stub return type is `Res!` / fallible (e.g. builder `send`).
    pub fallible_methods: HashSet<String>,
    /// Methods that exist with a NON-fallible return on at least one stub type.
    /// When a method is in both `fallible_methods` and `non_fallible_methods`,
    /// the untyped-receiver fallback must NOT apply the fallible suffix (ambiguous).
    pub non_fallible_methods: HashSet<String>,
    /// Per-type fallible method tracking: (TypeName, method_name) pairs where the
    /// method IS fallible on that specific type. Used for precise disambiguation.
    pub type_fallible_methods: HashSet<(String, String)>,
    /// Methods whose stub return type is async AND fallible (e.g. `BoxFuture<Res!<...>>`
    /// or declared with `Res!` on a struct that acts as an executor).
    /// These get `.await.map_err(...)?` instead of just `?`.
    pub async_fallible_methods: HashSet<String>,
    /// Stub package free-fn roots: use-alias / crate name → rust crate ident.
    /// e.g. `crypto` / `relay_crypto` / `relay-crypto` → `relay_crypto`.
    pub stub_pkg_crate: HashMap<String, String>,
    /// Stub free functions: (rust_crate, fn_name_without_bang) → fallible (Res!).
    pub stub_free_fns: HashMap<(String, String), bool>,
}

impl StubContext {
    pub fn new() -> Self {
        StubContext {
            stub_type_crate: HashMap::new(),
            stub_typed_ctors: HashMap::new(),
            fallible_methods: HashSet::new(),
            non_fallible_methods: HashSet::new(),
            type_fallible_methods: HashSet::new(),
            async_fallible_methods: HashSet::new(),
            stub_pkg_crate: HashMap::new(),
            stub_free_fns: HashMap::new(),
        }
    }
}

/// Routing context — envelope routing configuration and bus return types.
#[derive(Clone)]
pub struct RoutingContext {
    /// Expression that names the primary routing-trait instance for envelope
    /// routing. Derived from layer routing traits: `deps.<snake(Trait)>` in a
    /// flow; the injected param name inside a runtime-delegated step method.
    /// Empty when no routing traits are loaded.
    pub routing_ref: String,
    /// Names of traits used as message-routing ports (from layer statement
    /// `maps_to Trait.method`). Calls to these use `routing_ref` instead of
    /// `deps.<name>`.
    pub routing_traits: HashSet<String>,
    /// Whether cross-boundary calls use message-envelope routing (JSON) via
    /// layer-declared routing traits. Opt-in when loaded layers declare statement
    /// targets that are routing ports (INV-003).
    pub envelope_routing: bool,
    /// Bus message name → Rust success type for `invoke`/`request` decode
    /// (e.g. `"Reconcile"` → `"ReconcileResult"`). Json/unit stay as Value.
    pub bus_returns: HashMap<String, String>,
}

impl RoutingContext {
    pub fn new() -> Self {
        RoutingContext {
            routing_ref: String::new(),
            routing_traits: HashSet::new(),
            envelope_routing: false,
            bus_returns: HashMap::new(),
        }
    }
}

// ─── GenCtx ──────────────────────────────────────────────────────────────────

/// Code generation context — carries name resolution and type information.
pub struct GenCtx {
    /// All constructs in the solution by name → shape.
    pub name_to_shape: HashMap<String, Shape>,
    /// Locals accumulated in the current scope (let-bound variables).
    pub locals: HashSet<String>,
    /// Fields of the enclosing type (when inside a method body with a `self` receiver).
    pub self_fields: HashSet<String>,
    /// Whether we're inside a method body that uses `self.` for field access.
    pub in_method: bool,
    /// Type resolution: method signatures, local types, struct fields.
    pub types: TypeContext,
    /// Ownership and mutability tracking.
    pub ownership: OwnershipContext,
    /// Stub crate/type mappings and fallibility metadata.
    pub stubs: StubContext,
    /// Envelope routing configuration and bus return types.
    pub routing: RoutingContext,
    /// Names of known async free functions (layer-declared coordinators and
    /// package free fns). Calls to these need `.await?`.
    pub async_fns: HashSet<String>,
    /// Names backed by a threaded JSON state bag (multi-step runtime-delegated
    /// constructs). A read of such a name becomes `state["name"]`; an assignment
    /// writes `state["name"] = ...` so step impls can share results.
    pub state_locals: HashSet<String>,
    /// Expected Rust return type of the enclosing fn (e.g. `Result<Option<T>, DomainError>`).
    /// Used to wrap `ret x` as `Ok(Some(x))` when returning Option.
    pub expected_return_rust: Option<String>,
    /// When true, this expression is the value of an `Opt<T>` method (last
    /// expr or match/if arm). Wrap domain values in `Some`, `null`/`()` in
    /// `None`. Control-flow nodes apply the wrap to their arm values only.
    pub option_value_wrap: bool,
    /// Struct types whose smart ctor is zero-arg (every field fillable from
    /// INV-002 / collection / nested defaults) and thus implement `Default`.
    /// `Type.new(a, b)` on these lowers to a positional struct update + `..Default`.
    pub defaultable_types: HashSet<String>,
    /// Trait (or port) name → `Deps` field name. Preference: dependency-role
    /// input name (`@dep provider_repo: ApiProviderRepo` → `provider_repo`),
    /// else `to_snake(Trait)`. Shared by application emission, harness wiring,
    /// and port-call lowering so all three agree.
    pub dep_fields: HashMap<String, String>,
    /// Domain type names defined in the crate currently being generated
    /// (structs/enums in this context). Used so typed bus decode never emits
    /// foreign domain paths (tools cannot name storage::Repo).
    pub local_domain_types: HashSet<String>,
    /// Rust type names for self_fields (adapter struct fields). Keyed by field
    /// name. Used to detect Map/HashMap fields that require interior mutability
    /// (`tokio::sync::RwLock`) and reference-passing for key arguments.
    pub self_field_types: HashMap<String, String>,
    /// Layer statement specs by keyword — used for `lowers_to` template emission.
    pub statement_specs: HashMap<String, veil_ir::layer::StatementSpec>,
    /// Bare enum variant → enum type (`Healthy` → `DaemonStatus`).
    pub enum_variants: HashMap<String, String>,
    /// Unit-only enums (`enum TargetVariant { Api, Consumer }`). These derive
    /// `Copy` — never `.clone()` them.
    pub unit_enums: HashSet<String>,
    /// Known module/crate names for qualified path resolution (e.g. `serde_json::from_str`).
    /// Populated from loaded stub crate names + `std`. Replaces the old hardcoded array.
    pub known_modules: HashSet<String>,
    /// Error model: type name and variant names for domain errors. Populated from
    /// layer-declared error types. Replaces hardcoded `DomainError::*` strings.
    pub error_model: ErrorModel,
}

impl GenCtx {
    pub fn new(name_to_shape: HashMap<String, Shape>) -> Self {
        GenCtx {
            name_to_shape,
            locals: HashSet::new(),
            self_fields: HashSet::new(),
            in_method: false,
            types: TypeContext::new(),
            ownership: OwnershipContext::new(),
            stubs: StubContext::new(),
            routing: RoutingContext::new(),
            async_fns: HashSet::new(),
            state_locals: HashSet::new(),
            expected_return_rust: None,
            option_value_wrap: false,
            defaultable_types: HashSet::new(),
            dep_fields: HashMap::new(),
            local_domain_types: HashSet::new(),
            self_field_types: HashMap::new(),
            statement_specs: HashMap::new(),
            enum_variants: HashMap::new(),
            unit_enums: HashSet::new(),
            known_modules: HashSet::new(),
            error_model: ErrorModel::default(),
        }
    }

    /// `Deps` field for a trait/port call target (PascalCase trait or already-snake field).
    pub fn deps_field_for(&self, target: &str) -> String {
        if let Some(f) = self.dep_fields.get(target) {
            return f.clone();
        }
        // Target may already be the field name (dependency-role input registered as Trait).
        if self.dep_fields.values().any(|v| v == target) {
            return target.to_string();
        }
        to_snake(target)
    }

    /// Stable primary routing-trait name (sorted; HashSet order is arbitrary).
    pub fn primary_routing_trait(&self) -> Option<&str> {
        let mut names: Vec<&str> = self.routing.routing_traits.iter().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names.first().copied()
    }

    /// Default routing access path: `deps.<snake(Trait)>`, or empty if none.
    pub fn default_routing_ref_as_dep(&self) -> String {
        self.primary_routing_trait()
            .map(|t| format!("deps.{}", to_snake(t)))
            .unwrap_or_default()
    }

    /// Is this name a known trait-shaped construct (port/repo/integration)?
    pub fn is_trait_target(&self, name: &str) -> bool {
        matches!(self.name_to_shape.get(name), Some(Shape::Trait))
    }

    /// Is this name a known struct-shaped construct?
    pub fn is_struct_target(&self, name: &str) -> bool {
        matches!(self.name_to_shape.get(name), Some(Shape::Struct))
    }

    /// Is this name a known local variable?
    pub fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }

    /// Get the type of a local variable (if inferred).
    pub fn local_type(&self, name: &str) -> Option<&str> {
        self.types.local_types.get(name).map(|s| s.as_str())
    }

    /// Infer the return type of calling target.method().
    /// Returns the inner type (unwrapping Result).
    /// Call-site method names may include bang/query suffixes (`find!`); keys are
    /// stored without them (signature name is `find`, bang only wraps return type).
    pub fn return_type_of(&self, target: &str, method: &str) -> Option<&str> {
        let method_key = method.trim_end_matches(['!', '?']);
        let keys = [
            method.to_string(),
            method_key.to_string(),
            format!("{method_key}!"),
        ];
        for m in &keys {
            if let Some(t) = self.types.method_returns.get(&(target.to_string(), m.clone())) {
                return Some(t.as_str());
            }
        }
        // If target is a local, look up its type and check struct methods
        if let Some(type_name) = self.types.local_types.get(target) {
            for m in &keys {
                if let Some(t) = self
                    .types.method_returns
                    .get(&(type_name.clone(), m.clone()))
                {
                    return Some(t.as_str());
                }
            }
        }
        // Dep local registered as Trait shape but not in local_types — try snake_case target
        // (already covered by direct key) and PascalCase conversion is not needed.
        None
    }

    /// Get field type for a given type and field name.
    pub fn field_type(&self, type_name: &str, field_name: &str) -> Option<&str> {
        self.types.struct_fields.get(type_name)
            .and_then(|fields| fields.iter().find(|(n, _)| n == field_name))
            .map(|(_, t)| t.as_str())
    }

    /// Get the type of a method's parameter at a given position.
    /// Returns the type string (e.g. "Option<String>", "ApiProvider", "Json").
    /// Used to decide whether an Option<T> arg should be auto-unwrapped.
    pub fn param_type_at(&self, target: &str, method: &str, index: usize) -> Option<&str> {
        let method_key = method.trim_end_matches(['!', '?']);
        let keys = [
            (target.to_string(), method_key.to_string()),
            (to_snake(target), method_key.to_string()),
        ];
        for key in &keys {
            if let Some(params) = self.types.method_params.get(key) {
                return params.get(index).map(|s| s.as_str());
            }
        }
        None
    }
}

/// Build a GenCtx populated with type information from the solution's constructs and loaded stubs.
pub fn build_ctx_from_solution(solution: &Solution, name_to_shape: HashMap<String, Shape>, registry: &LayerRegistry) -> GenCtx {
    let mut ctx = GenCtx::new(name_to_shape);

    fn visit_constructs(c: &Construct, ctx: &mut GenCtx, registry: &LayerRegistry) {
        // Bus handler return types: svc/tool/handler name → Rust success type.
        if c.shape == Shape::Fn
            && let Some(rt) = &c.return_type {
                let rust = crate::rust::type_to_rust(rt);
                // Strip outer Result if present (VEIL return is the success type).
                let inner = rust
                    .strip_prefix("Result<")
                    .and_then(|s| s.rsplit_once(", "))
                    .map(|(a, _)| a.trim().to_string())
                    .unwrap_or(rust);
                if inner != "()" && !inner.is_empty() {
                    let msg = registry.bus_message_name(&c.name);
                    ctx.routing.bus_returns.insert(msg, inner.clone());
                    ctx.routing.bus_returns.insert(c.name.clone(), inner);
                }
            }
        // Record method return types for trait-shaped constructs
        if c.shape == Shape::Trait {
            for method in &c.methods {
                let ret_type = method.return_type.as_ref()
                    .map(extract_inner_type)
                    .unwrap_or_else(|| "()".to_string());
                let bare_method = method.name.trim_end_matches(['!', '?']).to_string();
                // Res! / Result, or a bang on the signature, is fallible.
                // Unit methods without bang must not get `.await?`.
                let is_result = matches!(method.return_type, Some(TypeExpr::Result(_)));
                if is_result || method.name.ends_with('!') {
                    ctx.stubs.type_fallible_methods
                        .insert((c.name.clone(), bare_method.clone()));
                    ctx.stubs.type_fallible_methods
                        .insert((to_snake(&c.name), bare_method.clone()));
                    ctx.stubs.type_fallible_methods
                        .insert((c.name.clone(), method.name.clone()));
                }
                // Register under PascalCase trait name (e.g. "CohortRepo", "find")
                ctx.types.method_returns.insert(
                    (c.name.clone(), method.name.clone()),
                    ret_type.clone(),
                );
                ctx.types.method_returns
                    .insert((c.name.clone(), bare_method.clone()), ret_type.clone());
                // Also register under snake_case dep name (e.g. "cohort_repo", "find")
                // so lookups from @dep variable names resolve without conversion
                ctx.types.method_returns.insert(
                    (to_snake(&c.name), method.name.clone()),
                    ret_type.clone(),
                );
                ctx.types.method_returns
                    .insert((to_snake(&c.name), bare_method.clone()), ret_type.clone());
                // Record parameter types for each method so call-site arg coercion
                // can check whether a port expects Option<T> vs T.
                let param_types: Vec<String> = method.params.iter()
                    .map(|p| type_name_simple(&p.type_expr))
                    .collect();
                ctx.types.method_params.insert(
                    (c.name.clone(), method.name.clone()),
                    param_types.clone(),
                );
                ctx.types.method_params.insert(
                    (to_snake(&c.name), method.name.clone()),
                    param_types.clone(),
                );
                ctx.types.method_params.insert(
                    (c.name.clone(), bare_method.clone()),
                    param_types.clone(),
                );
                ctx.types.method_params.insert(
                    (to_snake(&c.name), bare_method),
                    param_types,
                );
                // Type aliases (WearTestRepo = EntityRepo<WearTest>) share methods —
                // also register under any alias that monomorphizes this trait.
            }
        }

        // Bare enum variants (`Healthy`) → `DaemonStatus::Healthy`.
        if c.shape == Shape::Enum {
            let unit_only = if !c.rich_variants.is_empty() {
                c.rich_variants
                    .iter()
                    .all(|v| matches!(v, EnumVariant::Unit(_)))
            } else {
                !c.variants.is_empty()
            };
            if unit_only {
                ctx.unit_enums.insert(c.name.clone());
            }
            for v in &c.variants {
                register_enum_variant(ctx, v, &c.name);
            }
            for v in &c.rich_variants {
                register_enum_variant(ctx, v.name(), &c.name);
            }
        }
        for block in &c.blocks {
            if block.shape == Shape::Enum {
                let enum_name = block
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{}State", c.name));
                for v in &block.variants {
                    register_enum_variant(ctx, v, &enum_name);
                }
            }
        }

        // Record fields for struct-shaped constructs
        if c.shape == Shape::Struct {
            let mut fields: Vec<(String, String)> = c.fields.iter()
                .map(|f| (f.name.clone(), type_name_simple(&f.type_expr)))
                .collect();
            // Also include block fields (root block, etc.)
            for block in &c.blocks {
                if block.shape != Shape::Enum {
                    for f in &block.fields {
                        fields.push((f.name.clone(), type_name_simple(&f.type_expr)));
                    }
                }
            }
            ctx.types.struct_fields.insert(c.name.clone(), fields);

            // Record struct constructors: Type.new → Type (or Result<Type> for invariant types)
            let has_invariant = c
                .annotations
                .iter()
                .any(|a| registry.is_invariant_annotation(&a.name));
            let new_ret = if has_invariant {
                format!("Result<{}>", c.name)
            } else {
                c.name.clone()
            };
            ctx.types.method_returns.insert(
                (c.name.clone(), "new".to_string()),
                new_ret,
            );
        }

        for child in &c.children {
            visit_constructs(child, ctx, registry);
        }
    }

    for item in &solution.items {
        if let TopLevelItem::Construct(c) = item {
            visit_constructs(c, &mut ctx, registry);
        }
    }

    // Layer `declare` structs (DeployContext, …) are not solution items.
    for item in crate::rust::parse_layer_declare_items(registry) {
        if let TopLevelItem::Construct(c) = item {
            ctx.name_to_shape.entry(c.name.clone()).or_insert(c.shape);
            visit_constructs(&c, &mut ctx, registry);
        }
    }

    // Type aliases like `type WearTestRepo = EntityRepo<WearTest>` are ports
    // for call resolution (deps.wear_test_repo) and share method return types
    // with the generic base trait (find → Option after monomorphize).
    for item in &solution.items {
        if let TopLevelItem::TypeAlias { name, target } = item {
            if let TypeExpr::Generic(base, args) = target {
                ctx.name_to_shape.insert(name.clone(), Shape::Trait);
                // Copy method_returns from base trait, monomorphizing T → arg.
                let entity = args
                    .first()
                    .map(|a| match a {
                        TypeExpr::Named(n) => n.clone(),
                        _ => "T".into(),
                    })
                    .unwrap_or_else(|| "T".into());
                let base_keys: Vec<_> = ctx
                    .types.method_returns
                    .keys()
                    .filter(|(t, _)| t == base || t == &to_snake(base))
                    .cloned()
                    .collect();
                for (t, method) in base_keys {
                    if let Some(ret) = ctx.types.method_returns.get(&(t, method.clone())).cloned() {
                        // Option<T> → Option<WearTest>
                        let mono = ret.replace("<T>", &format!("<{entity}>")).replace(
                            "Option<T>",
                            &format!("Option<{entity}>"),
                        );
                        let mono = if mono == "T" {
                            entity.clone()
                        } else {
                            mono
                        };
                        ctx.types.method_returns
                            .insert((name.clone(), method.clone()), mono.clone());
                        ctx.types.method_returns
                            .insert((to_snake(name), method), mono);
                    }
                }
            } else if let TypeExpr::Named(base) = target {
                ctx.name_to_shape.insert(name.clone(), Shape::Trait);
                let base_keys: Vec<_> = ctx
                    .types.method_returns
                    .keys()
                    .filter(|(t, _)| t == base || t == &to_snake(base))
                    .cloned()
                    .collect();
                for (t, method) in base_keys {
                    if let Some(ret) = ctx.types.method_returns.get(&(t, method.clone())).cloned() {
                        ctx.types.method_returns
                            .insert((name.clone(), method.clone()), ret.clone());
                        ctx.types.method_returns
                            .insert((to_snake(name), method), ret);
                    }
                }
            }
        }
    }

    // Register stub crate type information
    for stub in &registry.stubs {
        let rust_crate = stub.name.replace('-', "_");
        // Package free-fn roots: bare crate, snake, and use-alias all resolve.
        ctx.stubs.stub_pkg_crate
            .insert(stub.name.clone(), rust_crate.clone());
        ctx.stubs.stub_pkg_crate
            .insert(rust_crate.clone(), rust_crate.clone());
        if let Some(alias) = &stub.alias {
            ctx.stubs.stub_pkg_crate
                .insert(alias.clone(), rust_crate.clone());
        }
        for ff in &stub.free_fns {
            let bare = ff.name.trim_end_matches(['!', '?']).to_string();
            let ret = ff.return_type.as_deref().unwrap_or("()");
            let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<") || ret.contains("Res!");
            if fallible {
                ctx.stubs.fallible_methods.insert(ff.name.clone());
                ctx.stubs.fallible_methods.insert(bare.clone());
            } else {
                ctx.stubs.non_fallible_methods.insert(ff.name.clone());
                ctx.stubs.non_fallible_methods.insert(bare.clone());
            }
            ctx.stubs.stub_free_fns
                .insert((rust_crate.clone(), bare.clone()), fallible);
            // Register return type for type inference (crate name acts as "type"):
            let inner = if ret.starts_with("Res!<") {
                ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
            } else if ret == "Res!" {
                "()"
            } else {
                ret
            };
            ctx.types.method_returns.insert(
                (stub.name.clone(), bare),
                inner.to_string(),
            );
        }
        for s in &stub.structs {
            // Compute the aliased name for this type
            let type_name = if let Some(alias) = &stub.alias {
                let cap_alias = alias.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() + &alias[1..];
                format!("{}{}", cap_alias, s.name)
            } else {
                s.name.clone()
            };
            // Register struct methods under the aliased name
            for method in &s.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<")
                    || ret.contains("Res!");
                let is_async_fallible = ret.contains("BoxFuture") && ret.contains("Res!")
                    || (fallible && method.params.iter().any(|p| {
                        // Methods taking an executor param (e.g. `executor: E`) are async
                        p.0 == "executor" || p.0 == "pool"
                    }))
                    || (fallible && stub.async_methods.contains(&method.name));
                if fallible {
                    ctx.stubs.fallible_methods.insert(method.name.clone());
                    ctx.stubs.type_fallible_methods.insert((type_name.clone(), method.name.clone()));
                } else {
                    ctx.stubs.non_fallible_methods.insert(method.name.clone());
                }
                if is_async_fallible {
                    ctx.stubs.async_fallible_methods.insert(method.name.clone());
                }
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.types.method_returns.insert(
                    (type_name.clone(), method.name.clone()),
                    inner.to_string(),
                );
                let param_types: Vec<String> = method
                    .params
                    .iter()
                    .map(|(_, ty, _)| {
                        let te = veil_ir::edit::parse_type_str(ty);
                        type_name_simple(&te)
                    })
                    .collect();
                ctx.types.method_params
                    .insert((type_name.clone(), method.name.clone()), param_types);
                // Track ref-pass parameters for this method
                let has_any_ref = method.params.iter().any(|p| p.2);
                if has_any_ref {
                    let ref_flags: Vec<bool> = method.params.iter().map(|p| p.2).collect();
                    ctx.types.ref_params.insert(
                        (type_name.clone(), method.name.clone()),
                        ref_flags,
                    );
                }
            }
            // Register as a known struct with qualified crate path from stub
            // metadata (`types_module` / `root_types`) — never crate-family hardcoding.
            ctx.name_to_shape.insert(type_name.clone(), Shape::Struct);
            let crate_name = stub.name.replace('-', "_");
            let path_type = stub.rust_type_path(&s.name);
            ctx.stubs.stub_type_crate
                .insert(type_name.clone(), (crate_name.clone(), path_type.clone()));
            // Crate-qualified keys so `aws_sdk_sns.Client` is not confused with
            // `aws_sdk_dynamodb.Client`.
            ctx.stubs.stub_type_crate.insert(
                format!("{crate_name}.{}", s.name),
                (crate_name.clone(), path_type.clone()),
            );
            ctx.stubs.stub_type_crate.insert(
                format!("{}::{}", crate_name, s.name),
                (crate_name.clone(), path_type.clone()),
            );
            ctx.name_to_shape
                .insert(format!("{crate_name}.{}", s.name), Shape::Struct);
            // Bare name only if unique — last-write-wins is how all four AWS
            // `Client` types collapsed to DynamoDB.
            let bare_hits = registry
                .stubs
                .iter()
                .filter(|st| {
                    st.structs.iter().any(|x| x.name == s.name)
                        || st.harness_fields.contains_key(&s.name)
                })
                .count();
            if bare_hits <= 1 {
                ctx.stubs.stub_type_crate
                    .entry(s.name.clone())
                    .or_insert_with(|| (crate_name.clone(), path_type.clone()));
            }
            // Also register under the bare (unaliased) name so VEIL source can use
            // `AttributeValue.S(pk)` even when the stub is aliased (e.g. `use ... as ddb`).
            if stub.alias.is_some() && s.name != type_name && bare_hits <= 1 {
                ctx.name_to_shape.entry(s.name.clone()).or_insert(Shape::Struct);
                ctx.stubs.stub_type_crate
                    .entry(s.name.clone())
                    .or_insert_with(|| (crate_name, path_type));
            }
            // Typed free-fn constructor (e.g. Query → query_as) from stub metadata.
            if let Some(ref typed_fn) = s.typed_variant {
                let params = s
                    .typed_type_params
                    .clone()
                    .unwrap_or_else(|| "_, return_type".into());
                ctx.stubs.stub_typed_ctors
                    .insert(type_name, (typed_fn.clone(), params.clone()));
                // Also key by bare stub type name (unaliased)
                ctx.stubs.stub_typed_ctors
                    .insert(s.name.clone(), (typed_fn.clone(), params));
            }
        }
        for i in &stub.impls {
            for method in &i.methods {
                let ret = method.return_type.as_deref().unwrap_or("()");
                let fallible = ret.starts_with("Res!") || ret.starts_with("Res!<");
                if fallible {
                    ctx.stubs.fallible_methods.insert(method.name.clone());
                    ctx.stubs.type_fallible_methods.insert((i.target.clone(), method.name.clone()));
                } else {
                    ctx.stubs.non_fallible_methods.insert(method.name.clone());
                }
                let inner = if ret.starts_with("Res!<") {
                    ret.strip_prefix("Res!<").unwrap_or(ret).strip_suffix('>').unwrap_or(ret)
                } else if ret == "Res!" {
                    "()"
                } else {
                    ret
                };
                ctx.types.method_returns.insert(
                    (i.target.clone(), method.name.clone()),
                    inner.to_string(),
                );
            }
        }
    }

    // Populate routing traits from the layer registry so call generation can
    // identify message-routing ports without hardcoding trait names.
    ctx.routing.routing_traits = registry.routing_traits().into_iter().collect();
    ctx.routing.routing_ref = ctx.default_routing_ref_as_dep();

    // Known modules: every loaded stub crate name + `std`. Replaces hardcoded
    // array in calls.rs — a new stub automatically becomes a known module.
    ctx.known_modules.insert("std".to_string());
    // Fallback: common crates used in codegen that may not have stubs loaded.
    for name in &["serde_json", "serde", "tokio", "tracing", "uuid", "chrono"] {
        ctx.known_modules.insert(name.to_string());
    }
    for stub in &registry.stubs {
        let rust_crate = stub.name.replace('-', "_");
        ctx.known_modules.insert(stub.name.clone());
        ctx.known_modules.insert(rust_crate);
        if let Some(alias) = &stub.alias {
            ctx.known_modules.insert(alias.clone());
        }
    }

    // Borrow fields: fields whose type should use `&self.field` instead of
    // `self.field.clone()`. Populated from stub `borrow_fields` declarations.
    // (e.g. sqlx declares `borrow_fields pool` because Executor requires &Pool.)
    for stub in &registry.stubs {
        for field in &stub.borrow_fields {
            ctx.ownership.borrow_fields.insert(field.clone());
        }
    }

    // Layer statement specs for custom `lowers_to` template emission.
    for stmt in &registry.statements {
        ctx.statement_specs.insert(stmt.keyword.clone(), stmt.clone());
    }

    // Track layer-declared free functions as async — they generate as
    // `pub async fn` and calls to them need `.await?`. Product free fns are
    // emitted in the product crate (sync unless they call async helpers).
    for item in &solution.items {
        if let TopLevelItem::Function(f) = item
            && f.layer_provided {
                ctx.async_fns.insert(f.name.clone());
            }
    }

    // Fixpoint: types whose every field is fillable without caller input
    // (scalars via constructor_policy, empty collections, nested defaultable).
    // Shape/type only — no subkind vocabulary.
    let ctor_pol = if registry.constructor_policy.auto_fields.is_empty() {
        veil_ir::layer::ConstructorPolicy::rust_defaults()
    } else {
        registry.constructor_policy.clone()
    };
    // Unit-like enums implement Default in rust codegen; treat as defaultable.
    for (name, shape) in &ctx.name_to_shape {
        if *shape == Shape::Enum {
            ctx.defaultable_types.insert(name.clone());
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        let names: Vec<String> = ctx.types.struct_fields.keys().cloned().collect();
        for type_name in names {
            if ctx.defaultable_types.contains(&type_name) {
                continue;
            }
            let fields = ctx.types.struct_fields.get(&type_name).cloned().unwrap_or_default();
            let all_ok = fields.iter().all(|(fname, fty)| {
                rust_field_is_defaultable(fname, fty, &ctor_pol, &ctx.defaultable_types)
            });
            if all_ok {
                ctx.defaultable_types.insert(type_name);
                changed = true;
            }
        }
    }

    ctx
}
